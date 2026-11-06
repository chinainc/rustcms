//! 后台缺口补齐的数据访问（批次 E）。
//!
//! 覆盖 Go 版 `controller/manageController` 里 Rust 侧还缺的几组后台动作：
//!
//! | 能力 | Go 入口 | 本文件函数 |
//! |------|---------|-----------|
//! | 找回密码三步 | `FindPasswordChooseWay` / `FindPasswordVerify` / `FindPasswordReset` | `find_password_choose` / `find_password_state` / `find_password_mark_verified` / `find_password_clear` |
//! | 文档批量删图 | `ArchiveDeleteImage` | `delete_archive_image` |
//! | 全站补图片 alt | `ArchiveFillImgAlt` | `fill_all_archive_img_alt` |
//! | 定时发布计划 | `UpdateArchiveReleasePlan` | `set_release_plan` |
//! | 批量打标签 | `UpdateArchiveTags` | `set_archive_tags` |
//! | 远程地址登记附件 | `AttachmentAddRemoteUrl` | `add_remote_urls` |
//! | 导入用 Excel 模板 | `GetQuickImportExcelTemplate` | `build_excel_template` |
//!
//! 约定：
//! - 表名 / 列名与 Go 版一致（`archives` / `archive_drafts` / `archive_data` /
//!   `tags` / `tag_data` / `attachments`），两个实现可以共用同一个数据库；
//! - 动态 SQL 一律用 `sqlx::query` / `sqlx::query_as`，不用编译期宏；
//! - 无符号整型列读出来先 `CAST(... AS SIGNED)`。

use std::sync::OnceLock;

use chrono::{DateTime, Duration, Local, Timelike};
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;

use super::{attachment, json_string_list, ModelError};

/// 找回密码的验证方式：文件令牌。
pub const FIND_WAY_FILE: &str = "file";
/// 找回密码的验证方式：域名 TXT 解析。
pub const FIND_WAY_DNS: &str = "dns";

/// 文档状态：计划发布（对齐 Go 的 `config.ContentStatusPlan`）。
const CONTENT_STATUS_PLAN: u8 = 2;

// ---------------------------------------------------------------------------
// 找回密码
// ---------------------------------------------------------------------------

/// 找回密码的一次性状态（对齐 Go 的 `currentSite.FindPasswordInfo`）。
#[derive(Debug, Clone, Serialize)]
pub struct FindPasswordInfo {
    /// 校验令牌（用户要写进文件 / DNS TXT 的内容）。
    pub token: String,
    /// DNS 方式要解析的主机名（`_rustcms.<域名>`）；文件方式为空串。
    pub host: String,
    /// 验证方式：`file` / `dns`。
    pub way: String,
    /// 过期时间（Unix 秒）。
    ///
    /// Go 的 `FindPasswordInfo.End` 带 `json:"-"`，不对外下发；
    /// 这里保持一致，避免响应多出 `end` 字段。
    #[serde(skip_serializing)]
    pub end: i64,
    /// 是否已通过校验。
    pub verified: bool,
}

/// 全进程共享的找回密码状态（Go 版存在 `Website.FindPasswordInfo` 里，
/// Rust 的 `AppState` 不方便加字段，这里用一个全局槽位代替）。
fn find_password_slot() -> &'static tokio::sync::Mutex<Option<FindPasswordInfo>> {
    static SLOT: OnceLock<tokio::sync::Mutex<Option<FindPasswordInfo>>> = OnceLock::new();
    SLOT.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// 生成找回密码令牌。
///
/// Go 是 `md5(站点密钥 + 纳秒时间戳)`；这里额外掺入随机数，避免同纳秒并发请求撞令牌。
pub fn make_find_password_token(secret: &str) -> String {
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let salt: u64 = rand::random();
    attachment::md5_hex(format!("{secret}{nanos}{salt}").as_bytes())
}

/// 发起找回密码：选定验证方式并生成一次性令牌。
pub async fn find_password_choose(
    secret: &str,
    way: &str,
    base_url: &str,
) -> Result<FindPasswordInfo, ModelError> {
    if way != FIND_WAY_FILE && way != FIND_WAY_DNS {
        return Err(ModelError::invalid("无效的验证方式"));
    }

    let mut host = String::new();
    if way == FIND_WAY_DNS {
        let hostname = url_hostname(base_url);
        if hostname.is_empty() {
            return Err(ModelError::invalid("域名解析失败"));
        }
        host = format!("_rustcms.{hostname}");
    }

    let info = FindPasswordInfo {
        token: make_find_password_token(secret),
        host,
        way: way.to_string(),
        // Go 是 59 分钟
        end: chrono::Utc::now().timestamp() + 59 * 60,
        verified: false,
    };
    let mut slot = find_password_slot().lock().await;
    *slot = Some(info.clone());
    Ok(info)
}

/// 读取当前的找回密码状态；没有或已过期返回 `None`。
pub async fn find_password_state() -> Option<FindPasswordInfo> {
    let mut slot = find_password_slot().lock().await;
    let expired = slot
        .as_ref()
        .map(|info| info.end < chrono::Utc::now().timestamp())
        .unwrap_or(true);
    if expired {
        *slot = None;
        return None;
    }
    slot.clone()
}

/// 把当前状态标记为「已通过校验」；没有状态返回 `false`。
pub async fn find_password_mark_verified() -> bool {
    let mut slot = find_password_slot().lock().await;
    match slot.as_mut() {
        Some(info) => {
            info.verified = true;
            true
        }
        None => false,
    }
}

/// 清空找回密码状态（重置密码成功后调用）。
pub async fn find_password_clear() {
    let mut slot = find_password_slot().lock().await;
    *slot = None;
}

/// 用系统 `nslookup` 查询域名的 TXT 记录。
///
/// 对齐 Go 的 `net.LookupTXT`（都走系统解析器），避免引入 DNS 依赖。
/// 命令不存在/启动失败单独返回错误，便于前端提示安装。
fn lookup_txt_records(host: &str) -> Result<Vec<String>, String> {
    let output = std::process::Command::new("nslookup")
        .args(["-type=TXT", host])
        .output()
        .map_err(|_| "系统缺少 nslookup 命令".to_string())?;
    let text = String::from_utf8_lossy(&output.stdout);
    // nslookup 输出把 TXT 内容放在双引号里，逐个提取引号内的片段
    let mut records = Vec::new();
    let mut current: Option<String> = None;
    for ch in text.chars() {
        match current {
            Some(ref mut value) => {
                if ch == '"' {
                    records.push(value.clone());
                    current = None;
                } else {
                    value.push(ch);
                }
            }
            None => {
                if ch == '"' {
                    current = Some(String::new());
                }
            }
        }
    }
    Ok(records)
}

/// DNS 方式校验：首条 TXT 记录必须等于令牌（对齐 Go 的 `FindPasswordVerify`）。
/// 返回 `Err` 表示系统缺少命令等环境问题，`Ok(false)` 才是记录不存在/不匹配。
pub async fn dns_txt_matches(host: &str, token: &str) -> Result<bool, String> {
    if host.is_empty() || token.is_empty() {
        return Ok(false);
    }
    let host = host.to_string();
    let token = token.to_string();
    match tokio::task::spawn_blocking(move || lookup_txt_records(&host)).await {
        Ok(Ok(records)) => Ok(records
            .first()
            .map(|record| record.trim() == token)
            .unwrap_or(false)),
        Ok(Err(msg)) => Err(msg),
        Err(_) => Err("DNS 查询任务执行失败".to_string()),
    }
}

/// 取 URL 里的主机名（不含协议、端口与路径）。
pub fn url_hostname(raw: &str) -> String {
    let rest = match raw.find("://") {
        Some(index) => &raw[index + 3..],
        None => raw,
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or("");
    host.trim().to_string()
}

/// 读站点密钥（`websites.token_secret`）；没有站点记录时返回空串。
pub async fn site_token_secret(pool: &MySqlPool) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT token_secret FROM websites ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or_default()
}

/// 读站点前台地址（`settings.system` 里的 `front_url` / `base_url`）。
pub async fn site_base_url(pool: &MySqlPool) -> String {
    let system = super::site_setting::get_section(pool, super::site_setting::SYSTEM)
        .await
        .unwrap_or_else(|_| json!({}));
    system
        .get("front_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| system.get("base_url").and_then(Value::as_str))
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string()
}

// ---------------------------------------------------------------------------
// 文档：批量删图 / 补 alt / 定时计划 / 标签
// ---------------------------------------------------------------------------

/// 删除文档相册里的第 `image_index` 张图，返回是否真的删掉了。
///
/// 对齐 Go 的 `ArchiveDeleteImage`：先找草稿（`archive_drafts`），找不到再找正式表
/// （`archives`），把该下标的图片从 `images` 里摘掉再写回。
pub async fn delete_archive_image(
    pool: &MySqlPool,
    id: i64,
    image_index: usize,
) -> Result<bool, ModelError> {
    let (table, raw) = match fetch_images(pool, "archive_drafts", id).await? {
        Some(images) => ("archive_drafts", images),
        None => match fetch_images(pool, "archives", id).await? {
            Some(images) => ("archives", images),
            None => return Err(ModelError::invalid("文档不存在")),
        },
    };

    let mut list = json_string_list(raw.as_deref());
    if image_index >= list.len() {
        return Err(ModelError::invalid("图片不存在"));
    }
    list.remove(image_index);

    let value = serde_json::to_string(&list)?;
    let sql = format!("UPDATE {table} SET images = ? WHERE id = ?");
    sqlx::query(&sql)
        .bind(value)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(true)
}

/// 读某张表里文档的 `images` 列；行不存在返回 `None`。
async fn fetch_images(
    pool: &MySqlPool,
    table: &str,
    id: i64,
) -> Result<Option<Option<String>>, ModelError> {
    let sql = format!("SELECT images FROM {table} WHERE id = ?");
    let row: Option<(Option<String>,)> = sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(images,)| images))
}

/// 为正文里缺失 alt 的图片补上标题，返回 `(处理过的文档数, 补上的图片数)`。
///
/// 与 Go 的 `FillAllArchiveImgAlt` 的差异：Go 只从 `archive_drafts` 取标题，
/// 所以已发布内容（只存在于 `archives`）会被跳过；这里正式表优先、草稿表兜底。
pub async fn fill_all_archive_img_alt(pool: &MySqlPool) -> Result<(usize, usize), ModelError> {
    let rows: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT id, content FROM archive_data")
            .fetch_all(pool)
            .await?;

    let mut docs = 0usize;
    let mut imgs = 0usize;
    for (id, content) in rows {
        let Some(content) = content else {
            continue;
        };
        if content.trim().is_empty() {
            continue;
        }
        let Some(title) = archive_title(pool, id).await else {
            continue;
        };
        if title.trim().is_empty() {
            continue;
        }
        let (new_content, filled) = fill_content_img_alt(&content, &title);
        if filled == 0 {
            continue;
        }
        sqlx::query("UPDATE archive_data SET content = ? WHERE id = ?")
            .bind(&new_content)
            .bind(id)
            .execute(pool)
            .await?;
        docs += 1;
        imgs += filled;
    }
    Ok((docs, imgs))
}

/// 取文档标题（正式表优先，其次草稿表）。
async fn archive_title(pool: &MySqlPool, id: i64) -> Option<String> {
    let published: Option<(String,)> = sqlx::query_as("SELECT title FROM archives WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    if let Some((title,)) = published {
        return Some(title);
    }
    let draft: Option<(String,)> = sqlx::query_as("SELECT title FROM archive_drafts WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    draft.map(|(title,)| title)
}

/// 给 HTML 里没有 alt（或 alt 为空）的 `<img>` 补 alt，返回 `(新内容, 补图数量)`。
///
/// 规则与 Go 的 `provider.FillContentImgAlt` 一致：
/// - 第 1 张图的 alt 是标题，之后依次是「标题-2」「标题-3」…
/// - 已有非空 alt 的图不动（用户显式设置优先）
/// - 标题做 HTML 转义，防止破坏属性引号
pub fn fill_content_img_alt(content: &str, title: &str) -> (String, usize) {
    let title = title.trim();
    if content.is_empty() || title.is_empty() {
        return (content.to_string(), 0);
    }
    let escaped = escape_html(title);
    let tag_re = match Regex::new(r"(?i)<img\b[^>]*>") {
        Ok(re) => re,
        Err(_) => return (content.to_string(), 0),
    };
    let alt_re = match Regex::new(r#"(?i)\balt\s*=\s*(["'])([^"']*)(["'])"#) {
        Ok(re) => re,
        Err(_) => return (content.to_string(), 0),
    };

    let mut index = 0usize;
    let mut filled = 0usize;
    let out = tag_re.replace_all(content, |caps: &regex::Captures| {
        let tag = caps.get(0).map(|m| m.as_str()).unwrap_or("");
        index += 1;
        let alt = if index > 1 {
            format!("{escaped}-{index}")
        } else {
            escaped.clone()
        };
        match alt_re.captures(tag) {
            Some(m) => {
                let value = m.get(2).map(|v| v.as_str()).unwrap_or("");
                if !value.trim().is_empty() {
                    // 已有非空 alt：保留用户显式设置
                    return tag.to_string();
                }
                // 空 alt：整段替换，保留原来的引号风格
                let quote = m.get(1).map(|v| v.as_str()).unwrap_or("\"");
                let whole = m.get(0).expect("group 0 一定存在");
                let start = whole.start();
                let end = whole.end();
                filled += 1;
                format!(
                    "{}alt={}{}{}{}",
                    &tag[..start],
                    quote,
                    alt,
                    quote,
                    &tag[end..]
                )
            }
            None => {
                // 没有 alt 属性：紧跟 `<img` 之后插入
                let lower = tag.to_ascii_lowercase();
                let insert_at = lower.find("<img").map(|i| i + 4).unwrap_or(0);
                filled += 1;
                format!("{} alt=\"{}\"{}", &tag[..insert_at], alt, &tag[insert_at..])
            }
        }
    });

    (out.into_owned(), filled)
}

/// HTML 转义（对齐 Go 的 `html.EscapeString`）。
pub fn escape_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '\'' => out.push_str("&#39;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            _ => out.push(ch),
        }
    }
    out
}

/// 批量设置定时发布计划，返回改动的文档数。
///
/// 对齐 Go 的 `UpdateArchiveReleasePlan`：
/// - `daily_limit <= 0` 时用文档总数当每日发布量；
/// - `end_hour <= start_hour` 时结束时间自动 +1 小时；
/// - 只改草稿表（`archive_drafts`）的 `created_time` / `updated_time` / `status = 2`。
pub async fn set_release_plan(
    pool: &MySqlPool,
    ids: &[i64],
    daily_limit: i64,
    start_hour: i64,
    end_hour: i64,
) -> Result<usize, ModelError> {
    if ids.is_empty() {
        return Err(ModelError::invalid("无可操作的文档"));
    }

    let mut end_hour = end_hour;
    if end_hour <= start_hour {
        end_hour = start_hour + 1;
    }
    let mut daily_limit = daily_limit;
    if daily_limit < 1 {
        daily_limit = ids.len() as i64;
    }
    let gap = (end_hour - start_hour) * 3600 / daily_limit;

    let now = Local::now();
    let mut day_num = 0i64;
    if end_hour < now.hour() as i64 {
        // 当天的时间窗已经过了，从明天开始
        day_num += 1;
    }
    let mut start_time = beginning_of_day(now) + Duration::days(day_num) + Duration::hours(start_hour);
    if start_time < now {
        start_time = now;
    }

    let mut count = 0usize;
    for id in ids {
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM archive_drafts WHERE id = ?")
            .bind(*id)
            .fetch_optional(pool)
            .await?;
        if exists.is_none() {
            // 文档不存在，跳过
            continue;
        }
        let ts = start_time.timestamp();
        sqlx::query(
            "UPDATE archive_drafts SET created_time = ?, updated_time = ?, status = ? WHERE id = ?",
        )
        .bind(ts)
        .bind(ts)
        .bind(CONTENT_STATUS_PLAN)
        .bind(*id)
        .execute(pool)
        .await?;
        count += 1;

        start_time = start_time + Duration::seconds(gap);
        if start_time.hour() as i64 >= end_hour {
            day_num += 1;
            start_time =
                beginning_of_day(now) + Duration::days(day_num) + Duration::hours(start_hour);
        }
    }
    Ok(count)
}

/// 当天零点（本地时区）。
fn beginning_of_day(now: DateTime<Local>) -> DateTime<Local> {
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| naive.and_local_timezone(Local).earliest())
        .unwrap_or(now)
}

/// 批量给文档打标签，返回新增的关联数。
///
/// 对齐 Go 的 `UpdateArchiveTags`：标签不存在时自动创建，`tag_data` 里已有的关联不重复插入。
pub async fn set_archive_tags(
    pool: &MySqlPool,
    ids: &[i64],
    tags: &[String],
) -> Result<usize, ModelError> {
    if ids.is_empty() {
        return Err(ModelError::invalid("无可操作的文档"));
    }
    let names: Vec<String> = tags
        .iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();
    if names.is_empty() {
        return Err(ModelError::invalid("请选择标签"));
    }

    let mut tag_ids: Vec<u32> = Vec::new();
    for name in &names {
        tag_ids.push(ensure_tag(pool, name).await?);
    }

    let mut count = 0usize;
    for id in ids {
        for tag_id in &tag_ids {
            let exists: Option<i64> =
                sqlx::query_scalar("SELECT id FROM tag_data WHERE tag_id = ? AND item_id = ? LIMIT 1")
                    .bind(*tag_id)
                    .bind(*id)
                    .fetch_optional(pool)
                    .await?;
            if exists.is_some() {
                continue;
            }
            sqlx::query("INSERT INTO tag_data (tag_id, item_id) VALUES (?, ?)")
                .bind(*tag_id)
                .bind(*id)
                .execute(pool)
                .await?;
            count += 1;
        }
    }
    Ok(count)
}

/// 按标题找标签，没有就创建一个，返回标签 ID。
async fn ensure_tag(pool: &MySqlPool, title: &str) -> Result<u32, ModelError> {
    let existing: Option<(u32,)> = sqlx::query_as("SELECT id FROM tags WHERE title = ? LIMIT 1")
        .bind(title)
        .fetch_optional(pool)
        .await?;
    if let Some((id,)) = existing {
        return Ok(id);
    }

    let now = chrono::Utc::now().timestamp();
    let token = unique_tag_token(pool, title).await?;
    let letter = title
        .chars()
        .next()
        .filter(|ch| ch.is_ascii_alphabetic())
        .map(|ch| ch.to_ascii_uppercase().to_string())
        .unwrap_or_default();
    let result = sqlx::query(
        "INSERT INTO tags (created_time, updated_time, title, category_id, seo_title, keywords, \
            url_token, description, first_letter, template, logo, status) \
         VALUES (?, ?, ?, 0, '', '', ?, '', ?, '', '', 1)",
    )
    .bind(now)
    .bind(now)
    .bind(title)
    .bind(&token)
    .bind(&letter)
    .execute(pool)
    .await?;
    Ok(result.last_insert_id() as u32)
}

/// 生成一个没被占用的标签别名。
async fn unique_tag_token(pool: &MySqlPool, title: &str) -> Result<String, ModelError> {
    let mut base = String::new();
    let mut last_dash = false;
    for ch in title.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_alphanumeric() {
            base.push(ch);
            last_dash = false;
        } else if !last_dash {
            base.push('-');
            last_dash = true;
        }
    }
    let base = base.trim_matches('-').chars().take(150).collect::<String>();
    let base = if base.is_empty() {
        format!("tag-{}", chrono::Utc::now().timestamp_millis())
    } else {
        base
    };

    let mut index = 0u32;
    loop {
        let candidate = if index == 0 {
            base.clone()
        } else {
            format!("{base}-{index}")
        };
        let exists: Option<u32> = sqlx::query_scalar("SELECT id FROM tags WHERE url_token = ? LIMIT 1")
            .bind(&candidate)
            .fetch_optional(pool)
            .await?;
        if exists.is_none() {
            return Ok(candidate);
        }
        index += 1;
        if index > 500 {
            return Ok(format!("{base}-{}", chrono::Utc::now().timestamp_millis()));
        }
    }
}

// ---------------------------------------------------------------------------
// 附件：远程地址登记
// ---------------------------------------------------------------------------

/// 按远程 URL 批量登记附件，返回新增条数。
///
/// 对齐 Go 的 `AddRemoteUrls`：
/// - 只接受 `http(s)://` 或 `//` 开头的地址；
/// - `地址|文件名` 的写法用竖线后一段当文件名；
/// - 同一地址（MD5 相同）只登记一次；
/// - 远程附件固定 `is_remote = 1` / `is_image = 1` / `status = 1`。
pub async fn add_remote_urls(
    pool: &MySqlPool,
    urls: &[String],
    category_id: u32,
) -> Result<usize, ModelError> {
    let mut count = 0usize;
    for raw in urls {
        let item = raw.trim();
        if !(item.starts_with("http") || item.starts_with("//")) {
            continue;
        }
        let mut location = item.to_string();
        let mut file_name = remote_file_name(item);
        let splits: Vec<&str> = item.split('|').collect();
        if splits.len() > 1 {
            location = splits[0].trim().to_string();
            file_name = splits[1].trim().to_string();
        }
        if location.is_empty() {
            continue;
        }
        let file_name: String = file_name.chars().take(250).collect();

        let md5 = attachment::md5_hex(location.as_bytes());
        let exists: Option<u32> =
            sqlx::query_scalar("SELECT id FROM attachments WHERE file_md5 = ? LIMIT 1")
                .bind(&md5)
                .fetch_optional(pool)
                .await?;
        if exists.is_some() {
            continue;
        }

        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO attachments (created_time, updated_time, user_id, file_name, file_location, \
                file_size, file_md5, width, height, category_id, is_image, status, watermark, \
                is_remote, logo) \
             VALUES (?, ?, 0, ?, ?, 0, ?, 0, 0, ?, 1, 1, 0, 1, '')",
        )
        .bind(now)
        .bind(now)
        .bind(&file_name)
        .bind(&location)
        .bind(&md5)
        .bind(category_id)
        .execute(pool)
        .await?;
        count += 1;
    }
    Ok(count)
}

/// 远程地址推导出的文件名（去掉查询串与扩展名，对齐 Go 的 `filepath.Base` + `Split(".")[0]`）。
fn remote_file_name(raw: &str) -> String {
    let base = raw.split('?').next().unwrap_or("");
    let base = base.rsplit('/').next().unwrap_or("");
    base.split('.').next().unwrap_or("").to_string()
}

// ---------------------------------------------------------------------------
// 批量导入用的 Excel 模板
// ---------------------------------------------------------------------------

/// 生成批量导入用的 Excel 模板（最小 xlsx：单表 `Sheet1`，第一行字段名、第二行示例值）。
///
/// 项目里没有 xlsx 依赖，这里手写一个「仅存储、不压缩」的 ZIP 包（xlsx 本质是 zip + xml）。
/// `columns` 是 `(字段名, 示例值)` 列表。
pub fn build_excel_template(columns: &[(String, String)]) -> Vec<u8> {
    let entries: Vec<(&str, Vec<u8>)> = vec![
        (
            "[Content_Types].xml",
            CONTENT_TYPES_XML.as_bytes().to_vec(),
        ),
        ("_rels/.rels", ROOT_RELS_XML.as_bytes().to_vec()),
        ("xl/workbook.xml", WORKBOOK_XML.as_bytes().to_vec()),
        (
            "xl/_rels/workbook.xml.rels",
            WORKBOOK_RELS_XML.as_bytes().to_vec(),
        ),
        (
            "xl/worksheets/sheet1.xml",
            sheet_xml(columns).into_bytes(),
        ),
    ];
    zip_store(&entries)
}

const CONTENT_TYPES_XML: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
    "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">",
    "<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>",
    "<Default Extension=\"xml\" ContentType=\"application/xml\"/>",
    "<Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>",
    "<Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>",
    "</Types>"
);

const ROOT_RELS_XML: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
    "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>",
    "</Relationships>"
);

const WORKBOOK_XML: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
    "<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" ",
    "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">",
    "<sheets><sheet name=\"Sheet1\" sheetId=\"1\" r:id=\"rId1\"/></sheets>",
    "</workbook>"
);

const WORKBOOK_RELS_XML: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
    "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/>",
    "</Relationships>"
);

/// 生成 `xl/worksheets/sheet1.xml`（两行：第一行字段名、第二行示例值）。
fn sheet_xml(columns: &[(String, String)]) -> String {
    let header: String = columns
        .iter()
        .enumerate()
        .map(|(index, (field, _))| {
            format!(
                "<c r=\"{}{}\" t=\"inlineStr\"><is><t>{}</t></is></c>",
                column_letters(index),
                1,
                xml_escape(field)
            )
        })
        .collect();
    let sample: String = columns
        .iter()
        .enumerate()
        .map(|(index, (_, value))| {
            format!(
                "<c r=\"{}{}\" t=\"inlineStr\"><is><t>{}</t></is></c>",
                column_letters(index),
                2,
                xml_escape(value)
            )
        })
        .collect();

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
<sheetData><row r=\"1\">{header}</row><row r=\"2\">{sample}</row></sheetData></worksheet>"
    )
}

/// 列序号转 Excel 列名：0 → A、25 → Z、26 → AA。
fn column_letters(mut index: usize) -> String {
    let mut out: Vec<char> = Vec::new();
    index += 1;
    while index > 0 {
        let rest = (index - 1) % 26;
        out.push((b'A' + rest as u8) as char);
        index = (index - 1) / 26;
    }
    out.iter().rev().collect()
}

/// XML 文本转义。
fn xml_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// CRC32（ZIP 用）。
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// 打一个「仅存储、不压缩」的 ZIP 包。
fn zip_store(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    let mut count: u16 = 0;

    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        let name_bytes = name.as_bytes();

        // 本地文件头
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // 解压所需版本
        out.extend_from_slice(&0u16.to_le_bytes()); // 通用标志
        out.extend_from_slice(&0u16.to_le_bytes()); // 压缩方式：存储
        out.extend_from_slice(&0u16.to_le_bytes()); // 修改时间
        out.extend_from_slice(&0x21u16.to_le_bytes()); // 修改日期（1980-01-01）
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // 扩展字段长度
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(data);

        // 中央目录
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // 生成版本
        central.extend_from_slice(&20u16.to_le_bytes()); // 解压所需版本
        central.extend_from_slice(&0u16.to_le_bytes()); // 通用标志
        central.extend_from_slice(&0u16.to_le_bytes()); // 压缩方式
        central.extend_from_slice(&0u16.to_le_bytes()); // 修改时间
        central.extend_from_slice(&0x21u16.to_le_bytes()); // 修改日期
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // 扩展字段长度
        central.extend_from_slice(&0u16.to_le_bytes()); // 注释长度
        central.extend_from_slice(&0u16.to_le_bytes()); // 起始磁盘
        central.extend_from_slice(&0u16.to_le_bytes()); // 内部属性
        central.extend_from_slice(&0u32.to_le_bytes()); // 外部属性
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);

        count += 1;
    }

    let cd_offset = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);

    // 中央目录结束记录
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // 当前磁盘
    out.extend_from_slice(&0u16.to_le_bytes()); // 中央目录起始磁盘
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // 注释长度
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn extracts_hostname() {
        assert_eq!(url_hostname("https://www.example.com/abc"), "www.example.com");
        assert_eq!(url_hostname("http://example.com:8080/x"), "example.com");
        assert_eq!(url_hostname("example.com"), "example.com");
        assert_eq!(url_hostname(""), "");
    }

    #[test]
    fn escapes_html() {
        assert_eq!(escape_html("a<b>&\"'"), "a&lt;b&gt;&amp;&#34;&#39;");
    }

    #[test]
    fn fills_missing_alt_only() {
        let (out, filled) = fill_content_img_alt(
            r#"<p><img src="1.png"><img src="2.png" alt="已有"></p>"#,
            "标题",
        );
        assert_eq!(filled, 1);
        assert!(out.contains(r#"alt="标题""#));
        assert!(out.contains(r#"alt="已有""#));
    }

    #[test]
    fn numbers_alt_per_image() {
        let (out, filled) = fill_content_img_alt(
            r#"<img src="1.png"><img src="2.png">"#,
            "标题",
        );
        assert_eq!(filled, 2);
        assert!(out.contains(r#"alt="标题""#));
        assert!(out.contains(r#"alt="标题-2""#));
    }

    #[test]
    fn builds_xlsx_package() {
        let bytes = build_excel_template(&[
            ("title".to_string(), "示例标题".to_string()),
            ("content".to_string(), "示例内容".to_string()),
        ]);
        assert_eq!(&bytes[0..4], b"PK\x03\x04");
        // 中央目录结束记录
        assert!(bytes
            .windows(4)
            .any(|window| window == [0x50, 0x4b, 0x05, 0x06]));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("xl/worksheets/sheet1.xml"));
    }

    #[tokio::test]
    async fn choose_way_is_validated() {
        let err = find_password_choose("", "sms", "https://example.com")
            .await
            .expect_err("非法验证方式应报错");
        assert!(err.is_invalid());
    }

    #[tokio::test]
    async fn choose_and_clear_password_state() {
        let info = find_password_choose("secret", FIND_WAY_FILE, "https://example.com")
            .await
            .expect("文件验证方式应通过");
        assert_eq!(info.way, FIND_WAY_FILE);
        assert!(info.host.is_empty());
        assert!(!info.verified);
        assert!(find_password_state().await.is_some());

        assert!(find_password_mark_verified().await);
        let state = find_password_state().await.expect("状态仍在");
        assert!(state.verified);

        find_password_clear().await;
        assert!(find_password_state().await.is_none());
    }

    #[tokio::test]
    async fn dns_way_builds_host() {
        let info = find_password_choose("secret", FIND_WAY_DNS, "https://www.example.com/")
            .await
            .expect("DNS 验证方式应通过");
        assert_eq!(info.host, "_rustcms.www.example.com");
        find_password_clear().await;
    }

    #[tokio::test]
    async fn delete_image_rejects_out_of_range() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let err = delete_archive_image(&pool, i64::MAX, 0)
            .await
            .expect_err("不存在的文档应报错");
        assert!(err.is_invalid());
    }

    #[tokio::test]
    async fn release_plan_requires_ids() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let err = set_release_plan(&pool, &[], 1, 0, 1)
            .await
            .expect_err("空文档列表应报错");
        assert!(err.is_invalid());
    }

    #[tokio::test]
    async fn tags_require_names() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let err = set_archive_tags(&pool, &[1], &[])
            .await
            .expect_err("空标签应报错");
        assert!(err.is_invalid());
    }
}

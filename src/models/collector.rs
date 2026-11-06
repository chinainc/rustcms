//! 采集配置与关键词（`settings.collector` + `keywords` 表）。
//!
//! 对齐 Go 版 `provider/collector.go`：
//! - 配置整份存在 `settings` 的 `collector` 键里（JSON）；
//! - `focus = true` 时整份覆盖，`false` 时只覆盖请求里出现的字段；
//! - 「内容替换」会真正对 `archive_data.content` / `archives.title` / `description`
//!   执行 SQL `REPLACE`（与 Go 的 `ReplaceArticles` 等价，但不做 HTML 解析）。

use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;

use rustcms_ext::http;

use super::archive_write::{self, ArchiveInput, SavedArchive};
use super::material::Material;
use super::{category, setting, ModelError};

/// 采集配置在 `settings` 里的键名。
pub const SETTING_KEY: &str = "collector";

/// 读采集配置；没有配置时返回空对象。
pub async fn get_setting(pool: &MySqlPool) -> anyhow::Result<Value> {
    Ok(setting::get_json(pool, SETTING_KEY)
        .await?
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({})))
}

/// 保存采集配置。
///
/// `focus = true`：整份替换（后台「保存配置」）；
/// `focus = false`：只覆盖请求里出现的字段（「保存替换词」这类局部更新）。
pub async fn save_setting(pool: &MySqlPool, value: &Value, focus: bool) -> anyhow::Result<Value> {
    if focus || !value.is_object() {
        setting::save_json(pool, SETTING_KEY, value).await?;
        return Ok(value.clone());
    }
    setting::merge_json(pool, SETTING_KEY, value).await
}

/// 一组「待替换词 → 目标词」。
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ReplaceKeyword {
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
}

/// 批量替换内容里的关键词，返回受影响行数合计。
///
/// 与 Go 的 `ReplaceArticles` 一样做「全表替换」，但不解析 HTML。
pub async fn replace_content(pool: &MySqlPool, pairs: &[ReplaceKeyword]) -> anyhow::Result<usize> {
    let pairs: Vec<&ReplaceKeyword> = pairs
        .iter()
        .filter(|pair| !pair.from.trim().is_empty())
        .collect();
    if pairs.is_empty() {
        return Ok(0);
    }

    let mut total = 0usize;
    for pair in pairs {
        // 正文
        let result = sqlx::query("UPDATE archive_data SET content = REPLACE(content, ?, ?)")
            .bind(&pair.from)
            .bind(&pair.to)
            .execute(pool)
            .await?;
        total += result.rows_affected() as usize;

        // 标题与描述
        let result = sqlx::query(
            "UPDATE archives SET title = REPLACE(title, ?, ?), \
                description = REPLACE(description, ?, ?)",
        )
        .bind(&pair.from)
        .bind(&pair.to)
        .bind(&pair.from)
        .bind(&pair.to)
        .execute(pool)
        .await?;
        total += result.rows_affected() as usize;
    }
    Ok(total)
}

/// 关键词。
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Keyword {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub title: String,
    pub status: u8,
    pub category_id: u32,
    pub level: i32,
    pub article_count: i32,
    pub has_dig: i8,
    pub last_time: i64,
}

/// 按标题取关键词。
pub async fn get_keyword_by_title(
    pool: &MySqlPool,
    title: &str,
) -> anyhow::Result<Option<Keyword>> {
    Ok(
        sqlx::query_as::<_, Keyword>("SELECT * FROM keywords WHERE title = ? LIMIT 1")
            .bind(title)
            .fetch_optional(pool)
            .await?,
    )
}

/// 按 ID 取关键词。
pub async fn get_keyword(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Keyword>> {
    Ok(sqlx::query_as::<_, Keyword>("SELECT * FROM keywords WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// 关键词不存在则创建，返回 ID。
pub async fn ensure_keyword(pool: &MySqlPool, title: &str) -> Result<u32, ModelError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ModelError::invalid("请填写关键词"));
    }
    if let Some(found) = get_keyword_by_title(pool, title).await? {
        return Ok(found.id);
    }
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT IGNORE INTO keywords (created_time, updated_time, title, status, category_id, \
            level, article_count, has_dig, last_time) VALUES (?, ?, ?, 1, 0, 0, 0, 0, 0)",
    )
    .bind(now)
    .bind(now)
    .bind(title)
    .execute(pool)
    .await?;
    get_keyword_by_title(pool, title)
        .await?
        .map(|item| item.id)
        .ok_or_else(|| ModelError::invalid("关键词创建失败"))
}

/// 关键词插件配置在 `settings` 里的键名。
pub const KEYWORD_SETTING: &str = "keyword";

/// 读关键词插件配置。
pub async fn keyword_setting_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = super::site_setting::get_section(pool, KEYWORD_SETTING).await?;
    if let Value::Object(map) = &mut value {
        map.entry("auto_dig").or_insert(json!(false));
        map.entry("language").or_insert(json!("zh"));
        map.entry("max_count").or_insert(json!(100000));
        map.entry("title_exclude").or_insert(json!([]));
        map.entry("title_replace").or_insert(json!([]));
    }
    Ok(value)
}

/// 保存关键词插件配置（字段级 merge）。
pub async fn keyword_setting_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    super::site_setting::save_section(pool, KEYWORD_SETTING, patch).await
}

/// 关键词分页列表，返回 `(总数, 本页记录)`。
pub async fn list_keywords(
    pool: &MySqlPool,
    keyword: &str,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Keyword>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;
    let keyword = keyword.trim();

    if keyword.is_empty() {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM keywords")
            .fetch_one(pool)
            .await?;
        let rows = sqlx::query_as::<_, Keyword>(
            "SELECT * FROM keywords ORDER BY id DESC LIMIT ? OFFSET ?",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        return Ok((total, rows));
    }

    let like = format!("%{keyword}%");
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM keywords WHERE title LIKE ?")
        .bind(&like)
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as::<_, Keyword>(
        "SELECT * FROM keywords WHERE title LIKE ? ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(&like)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok((total, rows))
}

/// 全部关键词（导出用）。
pub async fn all_keywords(pool: &MySqlPool) -> anyhow::Result<Vec<Keyword>> {
    Ok(
        sqlx::query_as::<_, Keyword>("SELECT * FROM keywords ORDER BY id ASC")
            .fetch_all(pool)
            .await?,
    )
}

/// 更新单条关键词（标题重复时报错）。
pub async fn update_keyword(
    pool: &MySqlPool,
    id: u32,
    title: &str,
    category_id: u32,
) -> Result<Keyword, ModelError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ModelError::invalid("请填写关键词"));
    }
    if get_keyword(pool, id).await?.is_none() {
        return Err(ModelError::invalid("关键词不存在"));
    }
    if let Some(exists) = get_keyword_by_title(pool, title).await? {
        if exists.id != id {
            return Err(ModelError::invalid(format!("关键词 {title} 已存在")));
        }
    }

    sqlx::query("UPDATE keywords SET title = ?, category_id = ?, updated_time = ? WHERE id = ?")
        .bind(title)
        .bind(category_id)
        .bind(chrono::Utc::now().timestamp())
        .bind(id)
        .execute(pool)
        .await?;

    get_keyword(pool, id)
        .await?
        .ok_or_else(|| ModelError::invalid("关键词不存在"))
}

/// 批量新增关键词（按换行分隔，自动去重），返回新增成功的记录。
pub async fn save_keywords_batch(
    pool: &MySqlPool,
    text: &str,
    category_id: u32,
) -> Result<Vec<Keyword>, ModelError> {
    let mut saved = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in text.split('\n') {
        let title = line.trim();
        if title.is_empty() || !seen.insert(title.to_string()) {
            continue;
        }
        if get_keyword_by_title(pool, title).await?.is_some() {
            continue;
        }
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT IGNORE INTO keywords (created_time, updated_time, title, status, category_id, \
                level, article_count, has_dig, last_time) VALUES (?, ?, ?, 1, ?, 0, 0, 0, 0)",
        )
        .bind(now)
        .bind(now)
        .bind(title)
        .bind(category_id)
        .execute(pool)
        .await?;
        if let Some(item) = get_keyword_by_title(pool, title).await? {
            saved.push(item);
        }
    }
    Ok(saved)
}

/// 删除单条关键词，返回是否真的删掉。
pub async fn delete_keyword(pool: &MySqlPool, id: u32) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM keywords WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// 批量删除关键词，返回删除条数。
pub async fn delete_keywords(pool: &MySqlPool, ids: &[u32]) -> anyhow::Result<u64> {
    let mut deleted = 0u64;
    for id in ids {
        deleted += sqlx::query("DELETE FROM keywords WHERE id = ?")
            .bind(*id)
            .execute(pool)
            .await?
            .rows_affected();
    }
    Ok(deleted)
}

/// 清空全部关键词，返回删除条数。
pub async fn delete_all_keywords(pool: &MySqlPool) -> anyhow::Result<u64> {
    Ok(sqlx::query("DELETE FROM keywords WHERE id > 0")
        .execute(pool)
        .await?
        .rows_affected())
}

// ---------------------------- 任务状态 ----------------------------

/// 采集任务状态存放在 `settings` 的键名。
pub const COLLECTOR_TASK_KEY: &str = "collector_jobs";
/// AI 自动写作任务状态存放在 `settings` 的键名。
pub const AIGENERATE_TASK_KEY: &str = "ai_generate_jobs";
/// 每个任务键最多保留的记录数。
const MAX_TASKS: usize = 20;
/// 采集正文最大字符数，避免把整站 HTML 写进库。
const MAX_CONTENT_CHARS: usize = 200_000;

static TASK_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 一条异步任务记录（采集 / 关键词挖掘 / AI 生成）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskJob {
    #[serde(default)]
    pub id: String,
    /// 任务类型：collect / start / dig / ai_generate
    #[serde(default)]
    pub kind: String,
    /// running / finished / failed
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub done: i64,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub created_time: i64,
    #[serde(default)]
    pub updated_time: i64,
}

async fn load_tasks(pool: &MySqlPool, key: &str) -> Vec<TaskJob> {
    setting::get_json(pool, key)
        .await
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value::<Vec<TaskJob>>(value).ok())
        .unwrap_or_default()
}

async fn store_tasks(pool: &MySqlPool, key: &str, tasks: &[TaskJob]) -> anyhow::Result<()> {
    let value = serde_json::to_value(tasks)?;
    setting::save_json(pool, key, &value).await
}

/// 新建一条任务记录，状态为 `running`。
pub async fn task_create(pool: &MySqlPool, key: &str, kind: &str) -> anyhow::Result<TaskJob> {
    let _guard = TASK_LOCK.lock().await;
    let now = chrono::Utc::now();
    let job = TaskJob {
        id: format!(
            "{}-{:04x}",
            now.timestamp_millis(),
            rand::random::<u16>()
        ),
        kind: kind.to_string(),
        status: "running".to_string(),
        total: 0,
        done: 0,
        message: String::new(),
        created_time: now.timestamp(),
        updated_time: now.timestamp(),
    };
    let mut tasks = load_tasks(pool, key).await;
    tasks.insert(0, job.clone());
    tasks.truncate(MAX_TASKS);
    store_tasks(pool, key, &tasks).await?;
    Ok(job)
}

/// 更新任务进度 / 状态，返回更新后的记录。
pub async fn task_update(
    pool: &MySqlPool,
    key: &str,
    job_id: &str,
    status: &str,
    done: i64,
    total: i64,
    message: &str,
) -> anyhow::Result<Option<TaskJob>> {
    let _guard = TASK_LOCK.lock().await;
    let mut tasks = load_tasks(pool, key).await;
    let mut updated = None;
    for task in tasks.iter_mut() {
        if task.id == job_id {
            task.status = status.to_string();
            task.done = done;
            task.total = total;
            task.message = message.to_string();
            task.updated_time = chrono::Utc::now().timestamp();
            updated = Some(task.clone());
            break;
        }
    }
    if updated.is_some() {
        store_tasks(pool, key, &tasks).await?;
    }
    Ok(updated)
}

/// 任务列表（新的在前），便于排查与状态查询。
pub async fn task_list(pool: &MySqlPool, key: &str) -> Vec<TaskJob> {
    load_tasks(pool, key).await
}

// ---------------------------- 配置读取 ----------------------------

/// 读取字符串配置，缺省时返回 `default`。
pub fn cfg_str(config: &Value, key: &str, default: &str) -> String {
    config
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .trim()
        .to_string()
}

/// 读取整数配置，缺省时返回 `default`。
pub fn cfg_i64(config: &Value, key: &str, default: i64) -> i64 {
    config.get(key).and_then(Value::as_i64).unwrap_or(default)
}

/// 读取字符串数组配置。
pub fn cfg_str_list(config: &Value, key: &str) -> Vec<String> {
    config
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------- HTTP 抓取 ----------------------------

const HTTP_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/122.0 Safari/537.36";

/// 发起 GET 请求并返回文本；非 2xx 视为失败。
pub async fn http_get_text(url: &str, timeout: Duration) -> anyhow::Result<String> {
    let response = http::send(&http::Request::get(url)
        .with_header("User-Agent", HTTP_USER_AGENT)
        .with_header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .with_timeout(timeout))
    .await?;
    if !response.is_success() {
        anyhow::bail!("请求 {url} 返回 HTTP {}", response.status);
    }
    Ok(response.text())
}

fn url_encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

// ---------------------------- HTML 解析 ----------------------------

fn regex_cached(slot: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    slot.get_or_init(|| Regex::new(pattern).expect("内置正则应合法"))
}

static RE_H1: OnceLock<Regex> = OnceLock::new();
static RE_TITLE: OnceLock<Regex> = OnceLock::new();
static RE_OG_TITLE: OnceLock<Regex> = OnceLock::new();
static RE_SCRIPT: OnceLock<Regex> = OnceLock::new();
static RE_STYLE: OnceLock<Regex> = OnceLock::new();
static RE_NOSCRIPT: OnceLock<Regex> = OnceLock::new();
static RE_COMMENT: OnceLock<Regex> = OnceLock::new();
static RE_P: OnceLock<Regex> = OnceLock::new();
static RE_LINK: OnceLock<Regex> = OnceLock::new();
static RE_RSS_ITEM: OnceLock<Regex> = OnceLock::new();
static RE_RSS_TITLE: OnceLock<Regex> = OnceLock::new();
static RE_RSS_LINK: OnceLock<Regex> = OnceLock::new();
static RE_BAIDU_SUGGEST: OnceLock<Regex> = OnceLock::new();
static RE_QUOTED: OnceLock<Regex> = OnceLock::new();

fn decode_entities(input: &str) -> String {
    input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn strip_inline_tags(html: &str) -> String {
    let text = archive_write::strip_tags(html);
    decode_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 去掉脚本 / 样式 / 注释，避免把 JS 当正文。
fn remove_noise(html: &str) -> String {
    let html = regex_cached(&RE_SCRIPT, r"(?is)<script\b[^>]*>.*?</script>")
        .replace_all(html, " ");
    let html = regex_cached(&RE_STYLE, r"(?is)<style\b[^>]*>.*?</style>").replace_all(&html, " ");
    let html = regex_cached(&RE_NOSCRIPT, r"(?is)<noscript\b[^>]*>.*?</noscript>")
        .replace_all(&html, " ");
    let html = regex_cached(&RE_COMMENT, r"(?s)<!--.*?-->").replace_all(&html, " ");
    html.to_string()
}

/// 提取页面标题：优先 h1，其次 og:title，最后 `<title>`。
pub fn extract_title(html: &str) -> String {
    let cleaned = remove_noise(html);
    for (slot, pattern) in [
        (&RE_H1, r"(?is)<h1[^>]*>(.*?)</h1>"),
        (
            &RE_OG_TITLE,
            r#"(?is)<meta\s+[^>]*property=["']og:title["'][^>]*content=["']([^"']*)["']"#,
        ),
        (&RE_TITLE, r"(?is)<title[^>]*>(.*?)</title>"),
    ] {
        if let Some(captures) = regex_cached(slot, pattern).captures(&cleaned) {
            let title = strip_inline_tags(&captures[1]);
            if !title.is_empty() {
                return title;
            }
        }
    }
    String::new()
}

/// 提取正文：取所有 `<p>` 段落；没有段落时退回全文纯文本。
pub fn extract_content(html: &str) -> String {
    let cleaned = remove_noise(html);
    let mut paragraphs = Vec::new();
    for captures in regex_cached(&RE_P, r"(?is)<p[^>]*>(.*?)</p>").captures_iter(&cleaned) {
        let text = strip_inline_tags(&captures[1]);
        if text.chars().count() >= 5 {
            paragraphs.push(text);
        }
    }
    let content = if paragraphs.is_empty() {
        let text = strip_inline_tags(&cleaned);
        if text.is_empty() {
            String::new()
        } else {
            format!("<p>{text}</p>")
        }
    } else {
        paragraphs
            .iter()
            .map(|text| format!("<p>{text}</p>"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    content.chars().take(MAX_CONTENT_CHARS).collect()
}

/// 从 HTML 里提取可访问的 http(s) 链接（标题 + 地址）。
pub fn extract_links(html: &str) -> Vec<(String, String)> {
    let cleaned = remove_noise(html);
    let mut links = Vec::new();
    for captures in regex_cached(&RE_LINK, r#"(?is)<a\s[^>]*href=["']([^"']+)["'][^>]*>(.*?)</a>"#)
        .captures_iter(&cleaned)
    {
        let url = captures[1].trim();
        if !url.starts_with("http://") && !url.starts_with("https://") {
            continue;
        }
        let title = strip_inline_tags(&captures[2]);
        links.push((url.to_string(), title));
    }
    links
}

/// 解析 Bing RSS 搜索结果。
pub fn parse_bing_rss(xml: &str) -> Vec<(String, String)> {
    let mut items = Vec::new();
    for captures in regex_cached(&RE_RSS_ITEM, r"(?is)<item\b[^>]*>(.*?)</item>").captures_iter(xml)
    {
        let item = captures[1]
            .replace("<![CDATA[", "")
            .replace("]]>", "");
        let title = regex_cached(&RE_RSS_TITLE, r"(?is)<title\b[^>]*>(.*?)</title>")
            .captures(&item)
            .map(|caps| strip_inline_tags(&caps[1]))
            .unwrap_or_default();
        let link = regex_cached(&RE_RSS_LINK, r"(?is)<link\b[^>]*>(.*?)</link>")
            .captures(&item)
            .map(|caps| strip_inline_tags(&caps[1]))
            .unwrap_or_default();
        if link.starts_with("http") {
            items.push((link, title));
        }
    }
    items
}

/// 按关键词搜索可采集的候选链接。
///
/// 优先 Bing RSS（结构稳定），失败时退回百度搜索页链接。
pub async fn search_links(keyword: &str, from_website: &str, limit: usize) -> Vec<(String, String)> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Vec::new();
    }
    let limit = limit.clamp(1, 20);
    let encoded = url_encode(keyword);

    let mut links = Vec::new();
    let rss_url = format!("https://www.bing.com/search?q={encoded}&format=rss&count={limit}");
    if let Ok(xml) = http_get_text(&rss_url, Duration::from_secs(15)).await {
        links = parse_bing_rss(&xml);
    }
    if links.is_empty() {
        let search_url = format!("https://www.baidu.com/s?wd={encoded}&rn={limit}");
        if let Ok(html) = http_get_text(&search_url, Duration::from_secs(15)).await {
            links = extract_links(&html);
        }
    }

    let domain = from_website
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_ascii_lowercase();
    let mut seen = std::collections::HashSet::new();
    links.retain(|(url, _title)| {
        let lower = url.to_ascii_lowercase();
        if !lower.starts_with("http") {
            return false;
        }
        // 过滤搜索引擎自身的导航链接
        if lower.contains("bing.com/search")
            || lower.contains("baidu.com/s?")
            || lower.contains("/search?")
        {
            return false;
        }
        if !domain.is_empty() && !lower.contains(&domain) {
            return false;
        }
        seen.insert(lower)
    });
    links.truncate(limit);
    links
}

/// 抓取单个页面并解析出标题与正文。
pub async fn fetch_article(url: &str) -> anyhow::Result<(String, String)> {
    let html = http_get_text(url, Duration::from_secs(20)).await?;
    let title = extract_title(&html);
    let content = extract_content(&html);
    if content.trim().is_empty() {
        anyhow::bail!("页面无可提取正文：{url}");
    }
    Ok((title, content))
}

// ---------------------------- 文章入库 ----------------------------

/// 取内容分类：优先关键词 / 配置里指定的分类，否则取第一个内容分类。
pub async fn resolve_category_id(pool: &MySqlPool, preferred: u32) -> anyhow::Result<Option<u32>> {
    if preferred > 0 && category::get(pool, preferred).await?.is_some() {
        return Ok(Some(preferred));
    }
    let found: Option<u32> = sqlx::query_scalar(
        "SELECT id FROM categories WHERE module_id > 0 AND `type` = 1 ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(found)
}

/// 保存一篇采集 / 生成的文章，返回写入结果。
pub async fn save_article(
    pool: &MySqlPool,
    title: &str,
    content: &str,
    keywords: &str,
    category_id: u32,
    origin_url: &str,
) -> Result<SavedArchive, ModelError> {
    let input = ArchiveInput {
        title: title.to_string(),
        content: content.to_string(),
        category_id,
        keywords: keywords.to_string(),
        origin_url: origin_url.to_string(),
        ..Default::default()
    };
    archive_write::create(pool, &input).await
}

/// 记录关键词本次采集时间与采集数量。
pub async fn touch_keyword(pool: &MySqlPool, id: u32, article_count: i64) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "UPDATE keywords SET last_time = ?, article_count = article_count + ?, updated_time = ? \
            WHERE id = ?",
    )
    .bind(now)
    .bind(article_count)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 待采集的关键词（从未采集过、状态启用）。
pub async fn pending_keywords(pool: &MySqlPool, limit: usize) -> anyhow::Result<Vec<Keyword>> {
    let limit = limit.clamp(1, 50) as i64;
    Ok(sqlx::query_as::<_, Keyword>(
        "SELECT * FROM keywords WHERE last_time = 0 AND status = 1 ORDER BY id ASC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

// ---------------------------- 采集执行 ----------------------------

/// 解析配置里的「待替换词 → 目标词」。
fn config_replacements(config: &Value) -> Vec<ReplaceKeyword> {
    config
        .get("content_replace")
        .and_then(|value| serde_json::from_value::<Vec<ReplaceKeyword>>(value.clone()).ok())
        .unwrap_or_default()
}

/// 采集配置里指定的默认分类：`category_id` 优先，其次 `category_ids` 首个。
fn config_category_id(config: &Value) -> u32 {
    let id = cfg_i64(config, "category_id", 0);
    if id > 0 {
        return id as u32;
    }
    config
        .get("category_ids")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32
}

fn apply_replacements(value: &str, pairs: &[ReplaceKeyword]) -> String {
    let mut out = value.to_string();
    for pair in pairs {
        if !pair.from.trim().is_empty() {
            out = out.replace(&pair.from, &pair.to);
        }
    }
    out
}

/// 按关键词执行一轮采集，返回本次新增的文章数。
pub async fn collect_keyword_articles(
    pool: &MySqlPool,
    keyword: &Keyword,
    config: &Value,
) -> anyhow::Result<usize> {
    let channels = cfg_i64(config, "channels", 2).clamp(1, 10) as usize;
    let from_website = cfg_str(config, "from_website", "");
    let title_min = cfg_i64(config, "title_min_length", 0).max(0) as usize;
    let content_min = cfg_i64(config, "content_min_length", 0).max(0) as usize;
    let excludes = cfg_str_list(config, "title_exclude");
    let pairs = config_replacements(config);
    let Some(category_id) = resolve_category_id(pool, config_category_id(config)).await? else {
        anyhow::bail!("未找到可用的内容分类");
    };

    let candidates = search_links(&keyword.title, &from_website, channels * 3).await;
    let mut saved = 0usize;
    for (url, _link_title) in candidates {
        if saved >= channels {
            break;
        }
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM archives WHERE origin_url = ? LIMIT 1")
            .bind(&url)
            .fetch_optional(pool)
            .await?;
        if exists.is_some() {
            continue;
        }
        let Ok((title, content)) = fetch_article(&url).await else {
            continue;
        };
        let title = apply_replacements(&title, &pairs);
        if title.is_empty() || title.chars().count() < title_min {
            continue;
        }
        if excludes.iter().any(|word| title.contains(word)) {
            continue;
        }
        let content = apply_replacements(&content, &pairs);
        let plain = archive_write::strip_tags(&content);
        if content_min > 0 && plain.chars().count() < content_min {
            continue;
        }
        match save_article(pool, &title, &content, &keyword.title, category_id, &url).await {
            Ok(_) => saved += 1,
            Err(err) => {
                tracing::warn!("采集入库失败（{url}）：{err}");
            }
        }
    }

    touch_keyword(pool, keyword.id, saved as i64).await?;
    Ok(saved)
}

/// 按关键词的采集配置逐个执行待采集关键词，返回 `(关键词数, 新增文章数)`。
pub async fn collect_pending_articles(
    pool: &MySqlPool,
    config: &Value,
) -> anyhow::Result<(usize, usize)> {
    let keywords = pending_keywords(pool, 20).await?;
    let mut article_total = 0usize;
    for keyword in &keywords {
        article_total += collect_keyword_articles(pool, keyword, config).await?;
    }
    Ok((keywords.len(), article_total))
}

// ---------------------------- 关键词挖掘 ----------------------------

/// 解析百度联想词 JSONP：`cb({q:"x",s:["a","b"]})`。
pub fn parse_baidu_suggest(text: &str) -> Vec<String> {
    let Some(captures) = regex_cached(&RE_BAIDU_SUGGEST, r#"(?s)"s"\s*:\s*\[(.*?)\]"#).captures(text)
    else {
        return Vec::new();
    };
    regex_cached(&RE_QUOTED, r#""([^"]+)""#)
        .captures_iter(&captures[1])
        .map(|caps| caps[1].trim().to_string())
        .filter(|word| !word.is_empty())
        .collect()
}

/// 解析 360 联想词：`{"result":[{"word":"x"}]}`。
pub fn parse_360_suggest(text: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    value
        .get("result")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("word").and_then(Value::as_str))
                .map(|word| word.trim().to_string())
                .filter(|word| !word.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// 获取关键词的联想词；`language = "en"` 时走 Google，否则走百度 / 360。
pub async fn suggest_keywords(word: &str, language: &str) -> Vec<String> {
    let word = word.trim();
    if word.is_empty() {
        return Vec::new();
    }
    let encoded = url_encode(word);

    if language == "en" {
        let url = format!(
            "https://suggestqueries.google.com/complete/search?client=firefox&q={encoded}"
        );
        if let Ok(text) = http_get_text(&url, Duration::from_secs(10)).await {
            if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) {
                if let Some(Value::Array(words)) = items.get(1) {
                    return words
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|word| word.trim().to_string())
                        .filter(|word| !word.is_empty())
                        .collect();
                }
            }
        }
        return Vec::new();
    }

    let url = format!("https://suggestion.baidu.com/su?wd={encoded}&p=3&cb=cb");
    if let Ok(text) = http_get_text(&url, Duration::from_secs(10)).await {
        let words = parse_baidu_suggest(&text);
        if !words.is_empty() {
            return words;
        }
    }

    let url = format!("https://sug.so.360.cn/suggest?word={encoded}&encodein=utf-8&encodeout=utf-8");
    match http_get_text(&url, Duration::from_secs(10)).await {
        Ok(text) => parse_360_suggest(&text),
        Err(_) => Vec::new(),
    }
}

/// 执行一轮关键词挖掘：取 `has_dig = 0` 的关键词，抓联想词并入库。
///
/// 返回 `(本轮处理的关键词数, 新增关键词数)`。
pub async fn dig_keywords(
    pool: &MySqlPool,
    limit: usize,
    language: &str,
    max_count: i64,
) -> anyhow::Result<(usize, usize)> {
    if max_count > 0 {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM keywords")
            .fetch_one(pool)
            .await?;
        if total >= max_count {
            return Ok((0, 0));
        }
    }

    let limit = limit.clamp(1, 30) as i64;
    let rows: Vec<(u32, String)> = sqlx::query_as(
        "SELECT id, title FROM keywords WHERE has_dig = 0 AND status = 1 ORDER BY id ASC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    // 全部挖掘过则重置标记，开启下一轮
    if rows.is_empty() {
        sqlx::query("UPDATE keywords SET has_dig = 0 WHERE id > 0")
            .execute(pool)
            .await?;
        return Ok((0, 0));
    }

    let mut added = 0usize;
    for (id, title) in &rows {
        let words = suggest_keywords(title, language).await;
        if !words.is_empty() {
            let saved = save_keywords_batch(pool, &words.join("\n"), 0).await?;
            added += saved.len();
        }
        let now = chrono::Utc::now().timestamp();
        sqlx::query("UPDATE keywords SET has_dig = 1, updated_time = ? WHERE id = ?")
            .bind(now)
            .bind(*id)
            .execute(pool)
            .await?;
    }
    Ok((rows.len(), added))
}

// ---------------------------- 问答组合 ----------------------------

/// 用素材库里的素材组装一篇问答组合文章。
///
/// 与 Go 版一致：至少需要 3 条同关键词素材，否则视为内容不足。
pub async fn combination_article(pool: &MySqlPool, keyword: &str) -> anyhow::Result<Value> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        anyhow::bail!("请填写关键词");
    }

    let materials: Vec<Material> = sqlx::query_as(
        "SELECT * FROM materials WHERE keyword = ? AND status = 1 ORDER BY id ASC LIMIT 20",
    )
    .bind(keyword)
    .fetch_all(pool)
    .await?;

    let materials: Vec<Material> = materials
        .into_iter()
        .filter(|item| {
            item.content
                .as_deref()
                .map(|content| !content.trim().is_empty())
                .unwrap_or(false)
        })
        .collect();
    if materials.len() < 3 {
        anyhow::bail!(
            "有效内容不足（关键词「{keyword}」仅有 {} 条素材，至少需要 3 条）",
            materials.len()
        );
    }

    let mut title = keyword.to_string();
    if title.chars().count() < 10 {
        title = materials[0].title.clone();
    }

    let mut blocks = Vec::new();
    for item in &materials {
        let content = item.content.as_deref().unwrap_or_default().trim();
        blocks.push(format!("<h3>{}</h3>\n<p>{}</p>", item.title, content));
    }

    Ok(json!({
        "title": title,
        "content": blocks.join("\n"),
        "keywords": keyword,
        "material_count": materials.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn deserializes_replace_pairs() {
        let pairs: Vec<ReplaceKeyword> =
            serde_json::from_str(r#"[{"from":"a","to":"b"},{"from":"","to":"x"}]"#).unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].from, "a");
        assert!(pairs[1].from.is_empty());
    }

    #[tokio::test]
    async fn setting_roundtrip_and_restore() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, SETTING_KEY).await.unwrap();

        let saved = save_setting(&pool, &json!({"channels": 3, "save_type": 1}), true)
            .await
            .unwrap();
        assert_eq!(saved["channels"], json!(3));

        // 局部更新：只改 save_type，channels 保留
        let merged = save_setting(&pool, &json!({"save_type": 0}), false)
            .await
            .unwrap();
        assert_eq!(merged["save_type"], json!(0));
        assert_eq!(merged["channels"], json!(3));

        match backup {
            Some(raw) => setting::save_raw(&pool, SETTING_KEY, &raw).await.unwrap(),
            None => setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
    }

    #[tokio::test]
    async fn ensure_keyword_is_idempotent() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let title = "rustcms-测试关键词";
        let first = ensure_keyword(&pool, title).await.unwrap();
        let second = ensure_keyword(&pool, title).await.unwrap();
        assert_eq!(first, second);

        let _ = sqlx::query("DELETE FROM keywords WHERE title = ?")
            .bind(title)
            .execute(&pool)
            .await;
    }
}

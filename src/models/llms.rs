//! llms.txt 插件（`settings.llms` + `public/llms.txt` / `public/llms-full.txt`）。
//!
//! 对齐 Go 版 `pluginLLMs.go` + `provider/llms.go`：
//! - 配置存 `settings.llms`（`config.PluginLLMsConfig` 的 20 个字段）；
//! - build 是**站内数据拼装**：单页（type=3 分类）+ 各模块分类 + 分类下的文章，
//!   写出 `llms.txt`（链接清单）与 `llms-full.txt`（正文导出，前 1000 条），
//!   不做任何外呼；
//! - 构建状态在 Go 里挂在 Website 内存字段上，这里用一个进程级互斥量保存，
//!   语义与 Go 修正版一致：无任务 = 已完成（`/status` 回 finished）；
//! - `HtmlToMarkdown` 用简化实现（覆盖常见标签），Go 依赖第三方库做完整转换。

use std::path::Path;
use std::sync::Mutex;

use serde::Serialize;
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;

use super::{archive, category, extra, module, setting, site_setting, url, ModelError};

/// LLMs 配置键（Go `provider.LLMsSettingKey`）。
pub const SETTING_KEY: &str = "llms";

/// 构建状态（Go `provider.LLMsBuildStatus`，去掉不可序列化的 timer）。
#[derive(Debug, Clone, Serialize)]
pub struct BuildStatus {
    /// 0 = 未开始，1 = 进行中，2 = 已完成
    pub status: i32,
    pub percent: i64,
    pub current: i64,
    pub total: i64,
}

static BUILD_STATUS: Mutex<Option<BuildStatus>> = Mutex::new(None);

/// 当前构建状态；`None` 表示没有进行中/刚结束的任务（= 已完成）。
pub fn build_status() -> Option<BuildStatus> {
    BUILD_STATUS.lock().ok().and_then(|guard| guard.clone())
}

fn set_status(status: Option<BuildStatus>) {
    if let Ok(mut guard) = BUILD_STATUS.lock() {
        *guard = status;
    }
}

/// 清空构建状态（测试隔离用：状态是进程级单例）。
#[cfg(test)]
pub fn build_status_reset() {
    set_status(None);
}

/// 配置完整默认值（对应 Go `config.PluginLLMsConfig` 的零值）。
fn default_config() -> Value {
    json!({
        "open": false,
        "update_frequency": 0,
        "last_update": 0,
        "file_url": "",
        "file_status": false,
        "max_post_per_type": 0,
        "max_words": 0,
        "include_metadata": false,
        "include_description": false,
        "include_category": false,
        "include_tag": false,
        "include_extra": false,
        "exclude_module_ids": [],
        "exclude_category_ids": [],
        "exclude_page_ids": [],
        "llms_title": "",
        "llms_description": "",
        "llms_after_description": "",
        "llms_end_description": "",
    })
}

/// 读 llms.txt 插件配置，并附上文件的生成状态 / 更新时间 / 访问地址
/// （Go `PluginGetLLMsSetting`）。
pub async fn setting_get(pool: &MySqlPool, public_dir: &Path) -> anyhow::Result<Value> {
    let stored = site_setting::get_section(pool, SETTING_KEY).await?;
    let mut base = default_config();
    if let (Value::Object(base_map), Value::Object(stored_map)) = (&mut base, &stored) {
        for (key, value) in stored_map {
            base_map.insert(key.clone(), value.clone());
        }
    }

    let llms_file = public_dir.join("llms.txt");
    if llms_file.is_file() {
        if let Ok(meta) = std::fs::metadata(&llms_file) {
            if let Ok(time) = meta.modified() {
                let datetime: chrono::DateTime<chrono::Local> = time.into();
                if let Value::Object(map) = &mut base {
                    map.insert("file_status".into(), json!(true));
                    map.insert("last_update".into(), json!(datetime.timestamp()));
                    let base_url = site_setting::get_section(pool, site_setting::SYSTEM)
                        .await?
                        .get("base_url")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim_end_matches('/')
                        .to_string();
                    map.insert("file_url".into(), json!(format!("{base_url}/llms.txt")));
                }
            }
        } else if let Value::Object(map) = &mut base {
            map.insert("file_status".into(), json!(false));
        }
    } else if let Value::Object(map) = &mut base {
        map.insert("file_status".into(), json!(false));
    }
    Ok(base)
}

/// 保存 llms.txt 插件配置（字段级 merge）。
pub async fn setting_save(
    pool: &MySqlPool,
    patch: &Value,
    public_dir: &Path,
) -> anyhow::Result<Value> {
    site_setting::save_section(pool, SETTING_KEY, patch).await?;
    setting_get(pool, public_dir).await
}

/// 生成 llms.txt 与 llms-full.txt（Go `provider.LLMsBuild`）。
///
/// 需先开启插件；生成完成后把 `last_update` / `file_status` 写回配置
/// （Go 只改内存，落库是为了让「设置」页立即读到最新状态）。
pub async fn build(pool: &MySqlPool, public_dir: &Path) -> Result<usize, ModelError> {
    let config = setting::get_json_or_empty(pool, SETTING_KEY)
        .await
        .map_err(|err| ModelError::Db(err))?;
    if !config.get("open").and_then(Value::as_bool).unwrap_or(false) {
        return Err(ModelError::invalid("LLMs 插件未开启"));
    }

    set_status(Some(BuildStatus {
        status: 1,
        percent: 0,
        current: 0,
        total: 0,
    }));
    match build_inner(pool, public_dir, &config).await {
        Ok(count) => {
            set_status(Some(BuildStatus {
                status: 2,
                percent: 100,
                current: 0,
                total: 0,
            }));
            Ok(count)
        }
        Err(err) => {
            // 失败时不再挂「进行中」状态，/status 回到「已完成」语义
            set_status(None);
            Err(err)
        }
    }
}

async fn build_inner(
    pool: &MySqlPool,
    public_dir: &Path,
    config: &Value,
) -> Result<usize, ModelError> {
    let flag = |key: &str| config.get(key).and_then(Value::as_bool).unwrap_or(false);
    let exclude_modules = id_set(config.get("exclude_module_ids"));
    let exclude_categories = id_set(config.get("exclude_category_ids"));
    let exclude_pages = id_set(config.get("exclude_page_ids"));

    let mut limit = config
        .get("max_post_per_type")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if limit == 0 {
        limit = 100;
    }
    let mut max_words = config.get("max_words").and_then(Value::as_i64).unwrap_or(0);
    if max_words == 0 {
        max_words = 250;
    }

    let system = site_setting::get_section(pool, site_setting::SYSTEM).await?;
    let base_url = system
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string();
    let site_name = system
        .get("site_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let index = site_setting::get_section(pool, site_setting::INDEX).await?;
    let index_description = index
        .get("seo_description")
        .and_then(Value::as_str)
        .unwrap_or("");

    let llms_title = str_or(config, "llms_title");
    let llms_description = str_or(config, "llms_description");
    let llms_after = str_or(config, "llms_after_description");
    let llms_end = str_or(config, "llms_end_description");

    let mode = url::load_mode(pool).await.map_err(|err| ModelError::Db(err))?;
    let categories = category::list_all(pool).await.map_err(|err| ModelError::Db(err))?;
    let modules = module::list(pool).await.map_err(|err| ModelError::Db(err))?;
    let module_tokens: std::collections::HashMap<u32, String> = modules
        .iter()
        .map(|item| (item.id, item.url_token.clone()))
        .collect();
    let modules_by_id: std::collections::HashMap<u32, &module::Module> =
        modules.iter().map(|item| (item.id, item)).collect();

    let archive_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM archives")
        .fetch_one(pool)
        .await?;
    let total = categories.len() as i64 + archive_total;
    let mut current: i64 = 0;
    let percent = |current: i64, total: i64| -> i64 {
        if total > 0 {
            current * 100 / total
        } else {
            0
        }
    };
    set_status(Some(BuildStatus {
        status: 1,
        percent: 0,
        current: 0,
        total,
    }));

    std::fs::create_dir_all(public_dir)
        .map_err(|err| ModelError::invalid(format!("写入失败: {err}")))?;
    let file_path = public_dir.join("llms.txt");
    let full_file_path = public_dir.join("llms-full.txt");
    let full_file_url = format!("{base_url}/llms-full.txt");

    let mut out = String::new();
    let mut full_out = String::new();

    // 标题 + 描述
    if !llms_title.is_empty() {
        out.push_str(&format!("# {llms_title}\n\n"));
    } else {
        out.push_str(&format!("# {site_name}\n\n"));
    }
    if !llms_description.is_empty() {
        out.push_str(&format!("> {llms_description}\n\n"));
    } else {
        out.push_str(&format!("> {index_description}\n\n"));
    }
    if !llms_after.is_empty() {
        out.push_str(&format!("> {llms_after}\n\n"));
    }

    out.push_str("## Full Content Export\n\n");
    out.push_str(&format!("- **URL**: {full_file_url}\n\n"));

    // 单页
    out.push_str("## Page\n\n");
    for page in &categories {
        if page.kind != 3 {
            continue;
        }
        if exclude_pages.contains(&page.id) {
            continue;
        }
        current += 1;
        set_status(Some(BuildStatus {
            status: 1,
            percent: percent(current, total),
            current,
            total,
        }));
        let link = format!("{base_url}{}", url::page_url(mode, &page.url_token, 1));
        let mut line = format!("- [{}]({link})", page.title);
        if !page.description.is_empty() {
            line.push_str(&format!(": {}", parse_description(&page.description, max_words)));
        }
        out.push_str(&line);
        out.push('\n');

        let mut full_data = format!("---\ntitle: {}\n", page.title);
        if flag("include_metadata") {
            full_data.push_str(&format!(
                "- published: {}\n- modified: {}\n",
                format_day(page.created_time.unwrap_or(0)),
                format_day(page.updated_time.unwrap_or(0)),
            ));
        }
        if flag("include_description") && !page.description.is_empty() {
            full_data.push_str(&format!(
                "- description: {}\n",
                parse_description(&page.description, max_words)
            ));
        }
        full_data.push_str("---\n\n");
        full_data.push_str(&html_to_markdown(page.content.as_deref().unwrap_or("")));
        full_out.push_str(&full_data);
        full_out.push_str("\n\n");
    }
    out.push('\n');

    // 各模块分类 + 分类下的文章
    for item in &modules {
        if exclude_modules.contains(&item.id) {
            continue;
        }
        for cat in &categories {
            if cat.module_id != item.id || cat.kind == 3 {
                continue;
            }
            if exclude_categories.contains(&cat.id) {
                continue;
            }
            current += 1;
            set_status(Some(BuildStatus {
                status: 1,
                percent: percent(current, total),
                current,
                total,
            }));
            out.push_str(&format!("## {}\n\n", cat.title));
            let module_token =
                module_tokens.get(&item.id).cloned().unwrap_or_default();
            let link = format!(
                "{base_url}{}",
                url::category_url(mode, &cat.url_token, &module_token, cat.id, 1)
            );
            let mut line = format!("[{}]({link})", cat.title);
            if !cat.description.is_empty() {
                line.push_str(&format!(" {}", parse_description(&cat.description, max_words)));
            }
            out.push_str(&line);
            out.push_str("\n\n");

            let query = archive::ArchiveQuery::new()
                .module(item.id)
                .categories(vec![cat.id]);
            let page = archive::list(pool, &query, archive::Order::IdDesc, 1, limit as u32)
                .await
                .map_err(|err| ModelError::Db(err))?;
            for article in &page.list {
                current += 1;
                set_status(Some(BuildStatus {
                    status: 1,
                    percent: percent(current, total),
                    current,
                    total,
                }));
                let link = format!(
                    "{base_url}{}",
                    url::archive_url(
                        mode,
                        &cat.url_token,
                        &module_token,
                        article.id,
                        &article.url_token,
                        article.category_id,
                        1
                    )
                );
                let mut line = format!("- [{}]({link})", article.title);
                if !article.description.is_empty() {
                    line.push_str(&format!(
                        ": {}",
                        parse_description(&article.description, max_words)
                    ));
                }
                out.push_str(&line);
                out.push('\n');

                // 前 1000 条才写入 full 导出（与 Go 一致）
                if current >= 1000 {
                    continue;
                }
                let mut full_data = format!("---\ntitle: {}\n", article.title);
                if flag("include_metadata") {
                    full_data.push_str(&format!(
                        "- published: {}\n- modified: {}\n",
                        format_day(article.created_time.unwrap_or(0)),
                        format_day(article.updated_time.unwrap_or(0)),
                    ));
                }
                if flag("include_description") && !article.description.is_empty() {
                    full_data.push_str(&format!(
                        "- description: {}\n",
                        parse_description(&article.description, max_words)
                    ));
                }
                if flag("include_category") {
                    full_data.push_str(&format!("- category: {}\n", cat.title));
                }
                if flag("include_tag") {
                    let tags: Vec<(String,)> = sqlx::query_as(
                        "SELECT t.title FROM tag_data d JOIN tags t ON t.id = d.tag_id \
                         WHERE d.item_id = ?",
                    )
                    .bind(article.id)
                    .fetch_all(pool)
                    .await?;
                    if !tags.is_empty() {
                        let names: Vec<String> =
                            tags.into_iter().map(|(title,)| title).collect();
                        full_data.push_str(&format!("- tags: {}\n", names.join(", ")));
                    }
                }
                if flag("include_extra") {
                    if let Some(mod_item) = modules_by_id.get(&article.module_id) {
                        let params = extra::load(pool, mod_item, article.id).await?;
                        for (name, value) in &params {
                            let text = match value {
                                Value::String(text) => text.clone(),
                                other => other.to_string(),
                            };
                            if !text.is_empty() {
                                full_data.push_str(&format!("- {name}: {text}\n"));
                            }
                        }
                    }
                }
                full_data.push_str("---\n\n");
                let content = archive::content(pool, article.id)
                    .await
                    .map_err(|err| ModelError::Db(err))?
                    .unwrap_or_default();
                full_data.push_str(&html_to_markdown(&content));
                full_out.push_str(&full_data);
                full_out.push_str("\n\n");
            }
            out.push('\n');
        }
    }

    if !llms_end.is_empty() {
        out.push_str(&format!("> {llms_end}\n"));
    }

    std::fs::write(&file_path, &out)
        .map_err(|err| ModelError::invalid(format!("写入失败: {err}")))?;
    std::fs::write(&full_file_path, &full_out)
        .map_err(|err| ModelError::invalid(format!("写入失败: {err}")))?;

    // 把生成时间与文件状态写回配置（Go 改内存对象，这里落库以达到同样可见效果）
    let now = chrono::Utc::now().timestamp();
    setting::merge_json(
        pool,
        SETTING_KEY,
        &json!({"last_update": now, "file_status": true}),
    )
    .await
    .map_err(|err| ModelError::Db(err))?;

    Ok(current as usize)
}

fn id_set(value: Option<&Value>) -> std::collections::HashSet<u32> {
    value
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| item.as_u64().map(|id| id as u32))
                .collect()
        })
        .unwrap_or_default()
}

fn str_or(config: &Value, key: &str) -> String {
    config
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// 时间戳 → `2006-01-02`（Go 版 full 导出里的日期格式）。
fn format_day(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|time| time.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// 截取摘要（Go `library.ParseDescription`）：按 rune 截断后，
/// 若 50 字符窗口内有句读符号，则在最后一个句读处截断。
pub fn parse_description(content: &str, num: i64) -> String {
    let num = num.max(0) as usize;
    let runes: Vec<char> = content.chars().collect();
    if runes.len() > num {
        let cut: String = runes[..num].iter().collect();
        let punct: &str = " !,.:;?~。？！，、；：…";
        let last = cut
            .char_indices()
            .filter(|(_, ch)| punct.contains(*ch))
            .map(|(index, _)| index)
            .next_back();
        let Some(last) = last else {
            return cut;
        };
        // Go: lastIndex >= num-50（按字节下标）
        if last as i64 >= num as i64 - 50 {
            cut[..last].to_string()
        } else {
            cut
        }
    } else {
        content.to_string()
    }
}

/// HTML → Markdown（简化版 Go `library.HtmlToMarkdown`）。
///
/// 非 HTML 内容原样返回；HTML 走「常见块级/行内标签转换 + 剩余标签剥离 +
/// 基础实体解码」，足以覆盖 CMS 正文里最常见的结构。
pub fn html_to_markdown(html: &str) -> String {
    let trimmed = html.trim();
    if !trimmed.starts_with('<') {
        return html.to_string();
    }

    let mut out = trimmed.to_string();

    // 图片 / 链接先行（避免内部文本先被改写）
    if let Ok(re) = regex::Regex::new(r#"(?is)<img[^>]*?alt=["']([^"']*)["'][^>]*?src=["']([^"']*)["'][^>]*>"#) {
        out = re.replace_all(&out, "![${1}](${2})").into_owned();
    }
    if let Ok(re) = regex::Regex::new(r#"(?is)<img[^>]*?src=["']([^"']*)["'][^>]*?alt=["']([^"']*)["'][^>]*>"#) {
        out = re.replace_all(&out, "![${2}](${1})").into_owned();
    }
    if let Ok(re) = regex::Regex::new(r#"(?is)<img[^>]*?src=["']([^"']*)["'][^>]*>"#) {
        out = re.replace_all(&out, "![](${1})").into_owned();
    }
    if let Ok(re) = regex::Regex::new(r#"(?is)<a[^>]*?href=["']([^"']*)["'][^>]*>(.*?)</a>"#) {
        out = re.replace_all(&out, "[$2]($1)").into_owned();
    }

    // 标题
    for level in 1..=6usize {
        let tag = format!("h{level}");
        let prefix = "#".repeat(level);
        if let Ok(re) = regex::Regex::new(&format!(r#"(?is)<{tag}[^>]*>(.*?)</{tag}>"#)) {
            out = re.replace_all(&out, format!("\n{prefix} $1\n")).into_owned();
        }
    }

    // 列表项 / 换行 / 块级结构
    if let Ok(re) = regex::Regex::new(r"(?is)<li[^>]*>") {
        out = re.replace_all(&out, "\n- ").into_owned();
    }
    if let Ok(re) = regex::Regex::new(r"(?is)<br\s*/?>") {
        out = re.replace_all(&out, "\n").into_owned();
    }
    for tag in ["p", "div", "ul", "ol", "table", "tr", "blockquote", "pre"] {
        if let Ok(re) = regex::Regex::new(&format!(r"(?is)</?{tag}[^>]*>")) {
            out = re.replace_all(&out, "\n").into_owned();
        }
    }

    // 行内强调
    if let Ok(re) = regex::Regex::new(r#"(?is)<(?:strong|b)[^>]*>(.*?)</(?:strong|b)>"#) {
        out = re.replace_all(&out, "**$1**").into_owned();
    }
    if let Ok(re) = regex::Regex::new(r#"(?is)<(?:em|i)[^>]*>(.*?)</(?:em|i)>"#) {
        out = re.replace_all(&out, "*$1*").into_owned();
    }

    // 剥掉剩余标签 + 解码基础实体
    if let Ok(re) = regex::Regex::new(r"(?is)<[^>]+>") {
        out = re.replace_all(&out, "").into_owned();
    }
    out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");

    // 压掉多余空行
    if let Ok(re) = regex::Regex::new(r"\n{3,}") {
        out = re.replace_all(&out, "\n\n").into_owned();
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_truncates_like_go() {
        assert_eq!(parse_description("短描述", 10), "短描述");
        let long = "a".repeat(100);
        assert_eq!(parse_description(&long, 50), "a".repeat(50));
        // 句读截断：50 字符窗口内有句号时在句号处断
        let text = format!("{}.{}", "a".repeat(30), "b".repeat(40));
        assert_eq!(parse_description(&text, 50), "a".repeat(30));
    }

    #[test]
    fn html_to_markdown_converts_common_tags() {
        assert_eq!(html_to_markdown("纯文本"), "纯文本");
        assert_eq!(
            html_to_markdown("<p>hello <strong>world</strong></p>"),
            "hello **world**"
        );
        assert_eq!(html_to_markdown("<h2>标题</h2><p>正文</p>"), "## 标题\n\n正文");
        assert_eq!(
            html_to_markdown(r#"<a href="https://x.com">链接</a>"#),
            "[链接](https://x.com)"
        );
        assert_eq!(
            html_to_markdown(r#"<img src="/a.png" alt="图">"#),
            "![图](/a.png)"
        );
        assert_eq!(html_to_markdown("<ul><li>一</li><li>二</li></ul>"), "- 一\n- 二");
        assert_eq!(
            html_to_markdown("<p>a &amp; b</p>"),
            "a & b"
        );
    }

    #[tokio::test]
    async fn setting_roundtrip_and_file_status() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, SETTING_KEY).await.unwrap();
        let public_dir =
            std::env::temp_dir().join(format!("rustcms-llms-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&public_dir);
        std::fs::create_dir_all(&public_dir).unwrap();

        let loaded = setting_get(&pool, &public_dir).await.unwrap();
        assert_eq!(loaded["open"], json!(false), "默认配置：{loaded}");
        assert_eq!(loaded["file_status"], json!(false));
        assert_eq!(loaded["exclude_module_ids"], json!([]));

        // 未开启时 build 被拒
        let err = build(&pool, &public_dir).await.unwrap_err();
        assert!(err.to_string().contains("未开启"), "{err}");

        // 文件不存在 → file_status=false；造一个文件 → true + last_update
        let saved = setting_save(
            &pool,
            &json!({"open": true, "max_words": 120}),
            &public_dir,
        )
        .await
        .unwrap();
        assert_eq!(saved["open"], json!(true));
        assert_eq!(saved["max_words"], json!(120));

        std::fs::write(public_dir.join("llms.txt"), "# t").unwrap();
        let loaded = setting_get(&pool, &public_dir).await.unwrap();
        assert_eq!(loaded["file_status"], json!(true));
        assert!(loaded["last_update"].as_i64().unwrap() > 0);
        assert!(loaded["file_url"].as_str().unwrap().ends_with("/llms.txt"));

        // build 真实生成两个文件
        let count = build(&pool, &public_dir).await.unwrap();
        assert!(count > 0);
        let txt = std::fs::read_to_string(public_dir.join("llms.txt")).unwrap();
        assert!(txt.starts_with("# "), "llms.txt 应以标题开头：{txt}");
        assert!(txt.contains("## Full Content Export"));
        assert!(txt.contains("## Page"));
        assert!(public_dir.join("llms-full.txt").is_file());
        // build 后配置里的时间与状态已更新
        let after = setting_get(&pool, &public_dir).await.unwrap();
        assert!(after["last_update"].as_i64().unwrap() > 0);
        // 状态：完成后回到「已完成」
        let status = build_status().unwrap();
        assert_eq!(status.status, 2);
        assert_eq!(status.percent, 100);

        match backup {
            Some(raw) => setting::save_raw(&pool, SETTING_KEY, &raw).await.unwrap(),
            None => setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
        let _ = std::fs::remove_dir_all(&public_dir);
    }
}

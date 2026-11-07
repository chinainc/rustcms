//! SEO 类插件：伪静态（rewrite）、robots.txt、sitemap。
//!
//! 对齐 Go 版：
//! - `pluginRewrite.go`：配置存 `settings.rewrite`（`{mode, patten}`），字段级 merge；
//! - `pluginRobots.go`：内容存**文件** `public/robots.txt`；
//! - `pluginSitemap.go`：配置存 `settings.sitemap`，生成时间写
//!   `cache/sitemap-time.log`，产物是 `public/sitemap.xml` 或 `public/sitemap.txt`。

use std::path::Path;
use std::sync::{OnceLock, RwLock};

use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;

use super::{category, module, setting, site_setting, url, ModelError};

/// 伪静态配置键。
pub const REWRITE: &str = "rewrite";
/// Sitemap 配置键。
pub const SITEMAP: &str = "sitemap";

/// robots.txt 默认内容（文件不存在时返回）。
pub const DEFAULT_ROBOTS: &str = "User-agent: *\nAllow: /\n";

/// 读伪静态配置。
pub async fn rewrite_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, REWRITE).await?;
    if !value.get("mode").is_some_and(Value::is_number) {
        if let Value::Object(map) = &mut value {
            map.insert("mode".into(), json!(0));
        }
    }
    if !value.get("patten").is_some_and(Value::is_string) {
        if let Value::Object(map) = &mut value {
            map.insert("patten".into(), json!(""));
        }
    }
    Ok(value)
}

/// 保存伪静态配置（字段级 merge）。
pub async fn rewrite_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    let saved = site_setting::save_section(pool, REWRITE, patch).await?;
    Ok(saved)
}

/// 读 robots.txt（缺失时返回默认内容）。
pub fn robots_get(public_dir: &Path) -> String {
    std::fs::read_to_string(public_dir.join("robots.txt")).unwrap_or_else(|_| DEFAULT_ROBOTS.to_string())
}

/// 写 robots.txt。
pub fn robots_save(public_dir: &Path, content: &str) -> Result<(), ModelError> {
    std::fs::create_dir_all(public_dir)
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    std::fs::write(public_dir.join("robots.txt"), content)
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))
}

/// sitemap 生成时间戳（`cache/sitemap-time.log` 的 mtime）。
pub fn sitemap_time(cache_dir: &Path) -> i64 {
    std::fs::metadata(cache_dir.join("sitemap-time.log"))
        .and_then(|meta| meta.modified())
        .map(|time| {
            let datetime: chrono::DateTime<chrono::Local> = time.into();
            datetime.timestamp()
        })
        .unwrap_or(0)
}

/// 读 sitemap 配置（补全缺省数组与派生字段）。
pub async fn sitemap_get(pool: &MySqlPool, cache_dir: &Path) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, SITEMAP).await?;
    let system = site_setting::get_section(pool, site_setting::SYSTEM).await?;

    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("txt")
        .to_string();
    let base = system
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim_end_matches('/');
    let front = system
        .get("front_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| base.to_string());

    if let Value::Object(map) = &mut value {
        map.entry("auto_build").or_insert(json!(0));
        map.insert("type".into(), json!(kind));
        map.insert("page_size".into(), map.get("page_size").cloned().unwrap_or(json!(20000)));
        map.entry("exclude_tag").or_insert(json!(false));
        for key in ["exclude_module_ids", "exclude_category_ids", "exclude_page_ids"] {
            if !map.get(key).is_some_and(Value::is_array) {
                map.insert(key.into(), json!([]));
            }
        }
        map.insert("updated_time".into(), json!(sitemap_time(cache_dir)));
        map.insert("sitemap_url".into(), json!(format!("{front}/sitemap.{kind}")));
    }
    Ok(value)
}

/// 保存 sitemap 配置（字段级 merge，`type` 只允许 `xml` / `txt`）。
pub async fn sitemap_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    let mut patch = patch.clone();
    if let Value::Object(map) = &mut patch {
        if let Some(kind) = map.get("type").and_then(Value::as_str) {
            let kind = if kind == "xml" { "xml" } else { "txt" };
            map.insert("type".into(), json!(kind));
        }
        if let Some(size) = map.get("page_size").and_then(Value::as_i64) {
            if size <= 0 {
                map.remove("page_size");
            }
        }
    }
    site_setting::save_section(pool, SITEMAP, &patch).await
}

fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
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

/// 生成 sitemap 文件，返回写入的 URL 数量。
pub async fn sitemap_build(
    pool: &MySqlPool,
    public_dir: &Path,
    cache_dir: &Path,
) -> Result<usize, ModelError> {
    let config = sitemap_get(pool, cache_dir).await?;
    let kind = config
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("txt")
        .to_string();
    let exclude_modules = id_set(config.get("exclude_module_ids"));
    let exclude_categories = id_set(config.get("exclude_category_ids"));
    let exclude_pages = id_set(config.get("exclude_page_ids"));

    let system = site_setting::get_section(pool, site_setting::SYSTEM).await?;
    let base = system
        .get("front_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| system.get("base_url").and_then(Value::as_str))
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string();
    if base.is_empty() {
        return Err(ModelError::invalid("请先配置站点网址"));
    }

    let mode = url::load_mode(pool).await.unwrap_or(0);
    let modules = module::list(pool).await?;
    let module_tokens: std::collections::HashMap<u32, String> = modules
        .iter()
        .map(|item| (item.id, item.url_token.clone()))
        .collect();

    let categories = category::list_all(pool).await?;
    let category_tokens: std::collections::HashMap<u32, String> = categories
        .iter()
        .map(|item| (item.id, item.url_token.clone()))
        .collect();

    let mut urls: Vec<String> = Vec::new();
    urls.push(format!("{base}/"));

    // 栏目 + 单页
    for item in &categories {
        if item.status != 1 {
            continue;
        }
        if item.kind == 3 {
            if exclude_pages.contains(&item.id) {
                continue;
            }
            urls.push(format!(
                "{base}{}",
                url::page_url(mode, &item.url_token, 1)
            ));
            continue;
        }
        if item.kind != 1 || exclude_categories.contains(&item.id) {
            continue;
        }
        let module_token = module_tokens
            .get(&item.module_id)
            .cloned()
            .unwrap_or_default();
        urls.push(format!(
            "{base}{}",
            url::category_url(mode, &item.url_token, &module_token, item.id, 1)
        ));
    }

    // 内容
    let rows: Vec<(i64, u32, String)> =
        sqlx::query_as("SELECT id, category_id, url_token FROM archives ORDER BY id ASC")
            .fetch_all(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("查询内容失败: {err}")))?;

    for (id, category_id, token) in rows {
        if exclude_categories.contains(&category_id) {
            continue;
        }
        let Some(item) = categories.iter().find(|item| item.id == category_id) else {
            continue;
        };
        if item.status != 1 || exclude_modules.contains(&item.module_id) {
            continue;
        }
        let module_token = module_tokens
            .get(&item.module_id)
            .cloned()
            .unwrap_or_default();
        let catname = category_tokens
            .get(&category_id)
            .cloned()
            .unwrap_or_default();
        urls.push(format!(
            "{base}{}",
            url::archive_url(mode, &catname, &module_token, id, &token, category_id, 1)
        ));
    }

    std::fs::create_dir_all(public_dir)
        .map_err(|err| ModelError::invalid(format!("写入失败: {err}")))?;

    let body = if kind == "xml" {
        let mut output = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
        );
        for link in &urls {
            output.push_str(&format!("  <url><loc>{}</loc></url>\n", xml_escape(link)));
        }
        output.push_str("</urlset>\n");
        output
    } else {
        urls.join("\n")
    };

    std::fs::write(public_dir.join(format!("sitemap.{kind}")), body)
        .map_err(|err| ModelError::invalid(format!("写入失败: {err}")))?;

    // 记录生成时间
    let _ = std::fs::create_dir_all(cache_dir);
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let _ = std::fs::write(cache_dir.join("sitemap-time.log"), stamp);

    Ok(urls.len())
}

/// 读取 sitemap 配置并补上最新的生成时间与地址（供接口回包用）。
pub async fn sitemap_payload(pool: &MySqlPool, cache_dir: &Path) -> anyhow::Result<Value> {
    sitemap_get(pool, cache_dir).await
}

/// 确保 `settings.sitemap` 存在（用于测试/首次访问）。
pub async fn sitemap_ensure(pool: &MySqlPool) -> anyhow::Result<()> {
    if setting::get_raw(pool, SITEMAP).await?.is_none() {
        setting::save_json(pool, SITEMAP, &json!({"type": "txt", "auto_build": 0})).await?;
    }
    Ok(())
}

// ─────────────────────────── 时间因子 / 结构化数据 / 谷歌认证 ───────────────────────────

/// 时间因子（定时发布）配置键。
pub const TIME_FACTOR: &str = "time_factor";
/// 结构化数据配置键。
pub const JSON_LD: &str = "json_ld";
/// 谷歌认证配置键。
pub const GOOGLE_AUTH: &str = "google_auth";

/// 读时间因子配置（补齐全部默认值）。
pub async fn timefactor_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, TIME_FACTOR).await?;
    if let Value::Object(map) = &mut value {
        map.entry("open").or_insert(json!(false));
        map.entry("module_ids").or_insert(json!([]));
        map.entry("types").or_insert(json!([]));
        map.entry("start_day").or_insert(json!(0));
        map.entry("end_day").or_insert(json!(0));
        map.entry("daily_update").or_insert(json!(0));
        map.entry("category_ids").or_insert(json!([]));
        map.entry("do_publish").or_insert(json!(false));
        map.entry("release_open").or_insert(json!(false));
        map.entry("daily_limit").or_insert(json!(0));
        map.entry("start_time").or_insert(json!(0));
        map.entry("end_time").or_insert(json!(0));
        map.entry("random").or_insert(json!(false));
        // 运行期计数：只读，保存时会被原值覆盖
        map.entry("today_count").or_insert(json!(0));
        map.entry("last_sent").or_insert(json!(0));
        map.entry("today_update").or_insert(json!(0));
        map.entry("last_update").or_insert(json!(0));
    }
    Ok(value)
}

/// 保存时间因子配置：`update_running` 丢弃，运行期计数保留原值。
pub async fn timefactor_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    let current = site_setting::get_section(pool, TIME_FACTOR).await?;
    let mut patch = patch.clone();
    if let Value::Object(map) = &mut patch {
        map.remove("update_running");
        for key in ["today_count", "last_sent", "today_update", "last_update"] {
            if let Some(value) = current.get(key) {
                map.insert(key.to_string(), value.clone());
            }
        }
    }
    site_setting::save_section(pool, TIME_FACTOR, &patch).await
}

/// 结构化数据的完整默认值（对应 Go 的零值）。
fn default_jsonld() -> Value {
    json!({
        "open": false,
        "about_page_id": 0,
        "contact_page_id": 0,
        "include_homepage": false,
        "include_search": false,
        "include_author": false,
        "author": "",
        "author_url": "",
        "include_breadcrumb": false,
        "include_comments": false,
        "module": [],
        "category": [],
        "default_brand": "",
        "default_image": "",
        "data_type": 0,
        "organization_type": "",
        "organization_name": "",
        "organization_legal_name": "",
        "organization_url": "",
        "person_name": "",
        "person_job_title": "",
        "person_image": "",
        "contact_type": "",
        "contact_number": "",
        "contact_url": "",
        "logo_image": "",
        "social_profiles": [],
        "opening_day_of_week": [],
        "opening_start_time": "",
        "opening_end_time": "",
        "price_range": "",
        "geo_latitude": "",
        "geo_longitude": "",
        "street_address": "",
        "address_locality": "",
        "address_region": "",
        "postal_code": "",
        "address_country": "",
    })
}

/// 读结构化数据配置。
pub async fn jsonld_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let stored = site_setting::get_section(pool, JSON_LD).await?;
    let mut base = default_jsonld();
    if let (Value::Object(base_map), Value::Object(stored_map)) = (&mut base, &stored) {
        for (key, value) in stored_map {
            base_map.insert(key.clone(), value.clone());
        }
    }
    Ok(base)
}

/// 保存结构化数据配置（字段级 merge）。
pub async fn jsonld_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    site_setting::save_section(pool, JSON_LD, patch).await?;
    jsonld_get(pool).await
}

/// 读谷歌认证配置。
pub async fn google_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, GOOGLE_AUTH).await?;
    if let Value::Object(map) = &mut value {
        map.entry("redirect_url").or_insert(json!(""));
        map.entry("client_id").or_insert(json!(""));
        map.entry("client_secret").or_insert(json!(""));
    }
    Ok(value)
}

/// 保存谷歌认证配置（字段级 merge）。
pub async fn google_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    site_setting::save_section(pool, GOOGLE_AUTH, patch).await?;
    google_get(pool).await
}

/// 搜索引擎推送配置键。
pub const PUSH: &str = "push";

/// 推送记录最多保留的条数。
const PUSH_LOG_LIMIT: usize = 20;

/// 读搜索引擎推送配置（`logs` 是运行记录，不参与配置保存）。
pub async fn push_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, PUSH).await?;
    if let Value::Object(map) = &mut value {
        map.entry("baidu_api").or_insert(json!(""));
        map.entry("bing_api").or_insert(json!(""));
        map.entry("google_json").or_insert(json!(""));
        map.entry("js_code").or_insert(json!(""));
        if !map.get("js_codes").is_some_and(Value::is_array) {
            map.insert("js_codes".into(), json!([]));
        }
        if !map.get("logs").is_some_and(Value::is_array) {
            map.insert("logs".into(), json!([]));
        }
    }
    Ok(value)
}

/// 保存推送配置：字段级 merge，`logs` 保持原值不被前端覆盖。
pub async fn push_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    if !patch.is_object() {
        return Err(anyhow::anyhow!("配置必须是 JSON 对象"));
    }
    let current = push_get(pool).await?;
    let mut patch = patch.clone();
    if let Value::Object(map) = &mut patch {
        if let Some(logs) = current.get("logs") {
            map.insert("logs".into(), logs.clone());
        }
    }
    site_setting::save_section(pool, PUSH, &patch).await?;
    push_get(pool).await
}

/// 最近推送记录（新记录在前）。
pub async fn push_logs(pool: &MySqlPool) -> anyhow::Result<Vec<Value>> {
    let value = push_get(pool).await?;
    let mut logs = value
        .get("logs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    logs.truncate(PUSH_LOG_LIMIT);
    Ok(logs)
}

/// 追加一条推送记录（只保留最近 20 条）。
pub async fn push_log_add(pool: &MySqlPool, entry: Value) -> anyhow::Result<()> {
    let mut logs = push_logs(pool).await?;
    logs.insert(0, entry);
    logs.truncate(PUSH_LOG_LIMIT);
    site_setting::save_section(pool, PUSH, &json!({ "logs": logs })).await?;
    Ok(())
}

/// 存储（附件 / 静态资源）配置键。
pub const STORAGE: &str = "storage";

/// 读存储配置。
pub async fn storage_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, STORAGE).await?;
    if let Value::Object(map) = &mut value {
        map.entry("storage_type").or_insert(json!("local"));
        map.entry("storage_url").or_insert(json!(""));
        map.entry("keep_local").or_insert(json!(false));
        map.entry("ftp_port").or_insert(json!(21));
        map.entry("ssh_port").or_insert(json!(22));
        if map
            .get("storage_type")
            .and_then(Value::as_str)
            .map(|value| value.trim().is_empty())
            .unwrap_or(true)
        {
            map.insert("storage_type".into(), json!("local"));
        }
    }
    Ok(value)
}

/// 保存存储配置：`storage_url` 补尾部 `/`，FTP / SSH 的 webroot 去掉尾部斜杠。
/// 与静态缓存不同，这里的 `keep_local` 由用户决定，不做强制。
pub async fn storage_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    if !patch.is_object() {
        return Err(anyhow::anyhow!("配置必须是 JSON 对象"));
    }
    let mut patch = patch.clone();
    if let Value::Object(map) = &mut patch {
        if let Some(url) = map.get("storage_url").and_then(Value::as_str) {
            let url = normalize_storage_url(url);
            map.insert("storage_url".into(), json!(url));
        }
        for key in ["ftp_webroot", "ssh_webroot"] {
            if let Some(raw) = map.get(key).and_then(Value::as_str) {
                let trimmed = raw.trim().trim_end_matches(['/', '\\']).to_string();
                map.insert(key.into(), json!(trimmed));
            }
        }
    }
    site_setting::save_section(pool, STORAGE, &patch).await?;
    storage_get(pool).await
}

/// 防刷（限流）配置键。
pub const LIMITER: &str = "limiter";
/// 防采集干扰码配置键。
pub const INTERFERENCE: &str = "interference";
/// 垃圾评论过滤（Akismet + reCAPTCHA）配置键。
pub const AKISMET: &str = "akismet";
/// 全文索引配置键。
pub const FULLTEXT: &str = "fulltext";
/// 内容导入 API 配置键。
pub const IMPORT_API: &str = "import_api";
/// 静态缓存配置键。
pub const HTML_CACHE: &str = "html_cache";

/// 读 Akismet / reCAPTCHA 配置。
pub async fn akismet_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, AKISMET).await?;
    if let Value::Object(map) = &mut value {
        map.entry("open").or_insert(json!(false));
        map.entry("api_key").or_insert(json!(""));
        if !map.get("check_type").is_some_and(Value::is_array) {
            map.insert("check_type".into(), json!([]));
        }
        map.entry("recaptcha_open").or_insert(json!(false));
        map.entry("recaptcha_site_key").or_insert(json!(""));
        map.entry("recaptcha_private_key").or_insert(json!(""));
    }
    Ok(value)
}

/// 保存 Akismet 配置（字段级 merge）。
pub async fn akismet_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    site_setting::save_section(pool, AKISMET, patch).await?;
    akismet_get(pool).await
}

/// 读全文索引配置。
pub async fn fulltext_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, FULLTEXT).await?;
    if let Value::Object(map) = &mut value {
        for key in ["open", "use_content", "use_category", "use_tag", "initialed"] {
            map.entry(key).or_insert(json!(false));
        }
        if !map.get("modules").is_some_and(Value::is_array) {
            map.insert("modules".into(), json!([]));
        }
        map.entry("engine").or_insert(json!("default"));
        map.entry("engine_url").or_insert(json!(""));
        map.entry("engine_user").or_insert(json!(""));
        map.entry("engine_pass").or_insert(json!(""));
        map.entry("ranking_score").or_insert(json!(0));
        map.entry("contain_length").or_insert(json!(0));
    }
    Ok(value)
}

/// 保存全文索引配置（字段级 merge）。
pub async fn fulltext_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    site_setting::save_section(pool, FULLTEXT, patch).await?;
    fulltext_get(pool).await
}

/// 读内容导入 API 配置。
pub async fn import_api_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, IMPORT_API).await?;
    if let Value::Object(map) = &mut value {
        map.entry("token").or_insert(json!(""));
        map.entry("link_token").or_insert(json!(""));
    }
    Ok(value)
}

/// 保存内容导入 API 配置（字段级 merge）。
pub async fn import_api_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    site_setting::save_section(pool, IMPORT_API, patch).await?;
    import_api_get(pool).await
}

/// 存储地址归一：非空时补尾部 `/`。
fn normalize_storage_url(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() || raw.ends_with('/') {
        raw.to_string()
    } else {
        format!("{raw}/")
    }
}

/// 读静态缓存配置。
pub async fn html_cache_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, HTML_CACHE).await?;
    if let Value::Object(map) = &mut value {
        map.entry("open").or_insert(json!(false));
        map.entry("index_cache").or_insert(json!(0));
        map.entry("category_cache").or_insert(json!(0));
        map.entry("detail_cache").or_insert(json!(0));
        map.entry("last_build_time").or_insert(json!(0));
        map.entry("last_push_time").or_insert(json!(0));
        map.entry("error_msg").or_insert(json!(""));
        map.entry("storage_type").or_insert(json!("local"));
        map.entry("storage_url").or_insert(json!(""));
        map.entry("keep_local").or_insert(json!(false));
    }
    Ok(value)
}

/// 保存静态缓存配置：`storage_url` 补尾部 `/`，`keep_local` 恒定 false（与 Go 一致）。
pub async fn html_cache_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    if !patch.is_object() {
        return Err(anyhow::anyhow!("配置必须是 JSON 对象"));
    }
    let mut patch = patch.clone();
    if let Value::Object(map) = &mut patch {
        if let Some(url) = map.get("storage_url").and_then(Value::as_str) {
            let url = normalize_storage_url(url);
            map.insert("storage_url".into(), json!(url));
        }
        map.insert("keep_local".into(), json!(false));
    }
    site_setting::save_section(pool, HTML_CACHE, &patch).await?;
    html_cache_get(pool).await
}

/// 读限流配置（补齐默认值）。
pub async fn limiter_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, LIMITER).await?;
    if let Value::Object(map) = &mut value {
        map.entry("open").or_insert(json!(false));
        for key in ["white_ips", "black_ips", "block_agents", "allow_prefixes"] {
            if !map.get(key).is_some_and(Value::is_array) {
                map.insert(key.into(), json!([]));
            }
        }
        map.entry("max_requests").or_insert(json!(0));
        map.entry("block_hours").or_insert(json!(0));
        map.entry("is_allow_spider").or_insert(json!(false));
        map.entry("ban_empty_refer").or_insert(json!(false));
        map.entry("ban_empty_agent").or_insert(json!(false));
        map.entry("mem_limit").or_insert(json!(false));
        map.entry("mem_percent").or_insert(json!(0));
    }
    Ok(value)
}

/// 保存限流配置（字段级 merge），并立即重建进程内限流器。
pub async fn limiter_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    site_setting::save_section(pool, LIMITER, patch).await?;
    limiter_reload(pool).await
}

/// 前端限流判定结果。
pub struct LimitBlock {
    pub status: u16,
    pub body: &'static str,
}

/// 进程内限流运行时（对齐 Go `Website.Limiter`，重启即清空）。
struct LimiterRuntime {
    open: bool,
    white_ips: Vec<String>,
    black_ips: Vec<String>,
    allow_spider: bool,
    ban_empty_agent: bool,
    ban_empty_refer: bool,
    blocker: rustcms_ext::ratelimit::IpBlocker,
}

impl Default for LimiterRuntime {
    fn default() -> Self {
        Self {
            open: false,
            white_ips: Vec::new(),
            black_ips: Vec::new(),
            allow_spider: false,
            ban_empty_agent: false,
            ban_empty_refer: false,
            blocker: rustcms_ext::ratelimit::IpBlocker::new(Default::default()),
        }
    }
}

static LIMITER_RUNTIME: OnceLock<RwLock<LimiterRuntime>> = OnceLock::new();

fn limiter_runtime() -> &'static RwLock<LimiterRuntime> {
    LIMITER_RUNTIME.get_or_init(|| RwLock::new(LimiterRuntime::default()))
}

/// 配置里的布尔值兼容 `true` / `1` / `"1"` 三种写法。
fn limiter_bool(value: &Value, key: &str) -> bool {
    match value.get(key) {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_i64().unwrap_or(0) != 0,
        Some(Value::String(text)) => matches!(text.trim(), "1" | "true" | "True" | "yes"),
        _ => false,
    }
}

fn limiter_string_list(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 按当前设置重建限流器（保存设置与进程启动时调用）。
pub async fn limiter_reload(pool: &MySqlPool) -> anyhow::Result<Value> {
    let config = limiter_get(pool).await?;
    let max_requests = config["max_requests"].as_i64().unwrap_or(0).max(1) as u32;
    let block_hours = config["block_hours"].as_i64().unwrap_or(0).max(1) as u64;
    let runtime = LimiterRuntime {
        open: limiter_bool(&config, "open"),
        white_ips: limiter_string_list(&config, "white_ips"),
        black_ips: limiter_string_list(&config, "black_ips"),
        allow_spider: limiter_bool(&config, "is_allow_spider"),
        ban_empty_agent: limiter_bool(&config, "ban_empty_agent"),
        ban_empty_refer: limiter_bool(&config, "ban_empty_refer"),
        blocker: rustcms_ext::ratelimit::IpBlocker::new(
            rustcms_ext::ratelimit::BlockConfig {
                threshold: max_requests,
                window: std::time::Duration::from_secs(5 * 60),
                block_for: std::time::Duration::from_secs(block_hours * 3600),
            },
        ),
    };
    *limiter_runtime()
        .write()
        .unwrap_or_else(|err| err.into_inner()) = runtime;
    Ok(config)
}

/// 私网 / 本机地址不做频率限制（对齐 Go `Limiter.isPrivateIP`）。
fn is_private_ip(ip: &str) -> bool {
    ip.starts_with("10.")
        || ip.starts_with("172.")
        || ip.starts_with("192.168.")
        || ip.starts_with("127.")
        || ip == "localhost"
        || ip == "::1"
}

/// 简单蜘蛛识别（Go 侧是完整的 UA 关键字库，这里覆盖常见搜索引擎）。
fn looks_like_spider(user_agent: &str) -> bool {
    let ua = user_agent.to_ascii_lowercase();
    [
        "bot", "spider", "crawler", "slurp", "baiduspider", "googlebot", "bingbot", "yandex",
        "sogou", "bytespider", "petalbot",
    ]
    .iter()
    .any(|keyword| ua.contains(keyword))
}

fn ipv4_u32(text: &str) -> Option<u32> {
    let mut parts = text.split('.');
    let mut value = 0u32;
    for _ in 0..4 {
        let part: u32 = parts.next()?.parse().ok()?;
        if part > 255 {
            return None;
        }
        value = (value << 8) | part;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(value)
}

/// IP 与单地址 / CIDR 段匹配（IPv4；IPv6 只支持精确匹配）。
fn ip_matches(ip: &str, pattern: &str) -> bool {
    let pattern = pattern.trim();
    let Some((network, bits)) = pattern.split_once('/') else {
        return pattern == ip;
    };
    let (Some(address), Some(network), Ok(bits)) =
        (ipv4_u32(ip), ipv4_u32(network), bits.parse::<u32>())
    else {
        return false;
    };
    if bits > 32 {
        return false;
    }
    let mask = if bits == 0 {
        0
    } else {
        u32::MAX << (32 - bits)
    };
    address & mask == network & mask
}

/// 前台请求限流判定（对齐 Go `controller.UseLimiter`）。
///
/// 返回 `None` 表示放行；返回 `Some` 时由调用方按 `status` / `body` 直接响应。
/// 内存占用阈值限流（Go 的 `MemPercent`）暂未实现：Rust 标准库没有系统内存
/// 占用 API，且不影响其余判定。
pub fn limiter_check(
    ip: Option<&str>,
    user_agent: &str,
    referer: &str,
    path: &str,
) -> Option<LimitBlock> {
    let runtime = limiter_runtime()
        .read()
        .unwrap_or_else(|err| err.into_inner());
    if !runtime.open {
        return None;
    }

    // 静态资源 / 后台 / 上传目录跳过；其中 static、uploads 可单独禁空 Referer
    let lower_path = path.to_ascii_lowercase();
    if lower_path.starts_with("/static")
        || lower_path.starts_with("/system")
        || lower_path.starts_with("/uploads")
        || lower_path.starts_with("/favicon.ico")
        || lower_path.ends_with("/api/import")
    {
        if (lower_path.starts_with("/static") || lower_path.starts_with("/uploads"))
            && runtime.ban_empty_refer
            && referer.trim().is_empty()
        {
            return Some(LimitBlock {
                status: 403,
                body: "Empty referer is not allowed.",
            });
        }
        return None;
    }

    if runtime.allow_spider && looks_like_spider(user_agent) {
        return None;
    }

    let Some(ip) = ip.map(str::trim).filter(|value| !value.is_empty()) else {
        return None;
    };
    if runtime.white_ips.iter().any(|pattern| ip_matches(ip, pattern)) {
        return None;
    }
    if runtime.ban_empty_agent && user_agent.trim().is_empty() {
        return Some(LimitBlock {
            status: 403,
            body: "Empty user agent is not allowed.",
        });
    }
    if runtime
        .black_ips
        .iter()
        .any(|pattern| ip_matches(ip, pattern))
        || runtime.blocker.is_blocked(ip)
    {
        return Some(LimitBlock {
            status: 403,
            body: "Your IP is blocked.",
        });
    }
    // Go 对私网 / 本机地址不计数，避免内网调试触发误封
    if is_private_ip(ip) {
        return None;
    }
    if runtime.blocker.hit(ip).is_allowed() {
        None
    } else {
        Some(LimitBlock {
            status: 429,
            body: "Too many requests from this IP.",
        })
    }
}

/// 当前被封禁的 IP 列表（对齐 Go `GetBlockIPs` 的 `{ip, block_time}`）。
pub fn limiter_blocked_ips() -> Vec<Value> {
    let runtime = limiter_runtime()
        .read()
        .unwrap_or_else(|err| err.into_inner());
    if !runtime.open {
        return Vec::new();
    }
    runtime
        .blocker
        .blocked_entries()
        .into_iter()
        .map(|(ip, block_time)| json!({"ip": ip, "block_time": block_time}))
        .collect()
}

/// 解除某个 IP 的临时封禁，返回它原本是否处于封禁状态。
pub fn limiter_unblock(ip: &str) -> bool {
    let ip = ip.trim();
    if ip.is_empty() {
        return false;
    }
    limiter_runtime()
        .write()
        .unwrap_or_else(|err| err.into_inner())
        .blocker
        .unblock(ip)
}

/// 读防采集干扰码配置。
pub async fn interference_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, INTERFERENCE).await?;
    if let Value::Object(map) = &mut value {
        map.entry("open").or_insert(json!(false));
        map.entry("mode").or_insert(json!(0));
        map.entry("disable_selection").or_insert(json!(false));
        map.entry("disable_copy").or_insert(json!(false));
        map.entry("disable_right_click").or_insert(json!(false));
    }
    Ok(value)
}

/// 保存防采集干扰码配置（字段级 merge）。
pub async fn interference_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    site_setting::save_section(pool, INTERFERENCE, patch).await?;
    interference_get(pool).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn escapes_xml() {
        assert_eq!(xml_escape("a&b<c>\"d\"'e'"), "a&amp;b&lt;c&gt;&quot;d&quot;&apos;e&apos;");
    }

    #[test]
    fn parses_id_sets() {
        let set = id_set(Some(&json!([1, 2, "x", 3])));
        assert_eq!(set.len(), 3);
        assert!(set.contains(&2));
        assert!(id_set(None).is_empty());
        assert!(id_set(Some(&json!("nope"))).is_empty());
    }

    #[test]
    fn robots_roundtrip_in_temp_dir() {
        let dir = std::env::temp_dir().join(format!("rustcms-robots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // 缺失时返回默认内容
        assert_eq!(robots_get(&dir), DEFAULT_ROBOTS);
        robots_save(&dir, "User-agent: *\nDisallow: /admin\n").unwrap();
        assert!(robots_get(&dir).contains("Disallow: /admin"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn rewrite_roundtrip_and_restore() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, REWRITE).await.unwrap();

        let saved = rewrite_save(&pool, &json!({"mode": 3, "patten": ""}))
            .await
            .unwrap();
        assert_eq!(saved["mode"], json!(3));

        let loaded = rewrite_get(&pool).await.unwrap();
        assert_eq!(loaded["mode"], json!(3));
        assert!(loaded["patten"].is_string());

        match backup {
            Some(raw) => setting::save_raw(&pool, REWRITE, &raw).await.unwrap(),
            None => setting::remove(&pool, REWRITE).await.unwrap(),
        }
    }

    #[tokio::test]
    async fn sitemap_config_and_build() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, SITEMAP).await.unwrap();
        let dir = std::env::temp_dir().join(format!("rustcms-sitemap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let public = dir.join("public");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&public).unwrap();
        std::fs::create_dir_all(&cache).unwrap();

        // 非法 type 归一成 txt
        let saved = sitemap_save(&pool, &json!({"type": "rss"})).await.unwrap();
        assert_eq!(saved["type"], json!("txt"));

        let built = sitemap_build(&pool, &public, &cache).await;
        match built {
            Ok(count) => {
                assert!(count >= 1);
                assert!(public.join("sitemap.txt").is_file());
                assert!(sitemap_time(&cache) > 0);
            }
            // 站点未配置 base_url 时返回明确错误
            Err(err) => assert!(err.to_string().contains("站点网址"), "{err}"),
        }

        // xml 模式
        sitemap_save(&pool, &json!({"type": "xml"})).await.unwrap();
        if let Ok(_) = sitemap_build(&pool, &public, &cache).await {
            let raw = std::fs::read_to_string(public.join("sitemap.xml")).unwrap();
            assert!(raw.contains("<urlset"), "{raw}");
        }

        match backup {
            Some(raw) => setting::save_raw(&pool, SITEMAP, &raw).await.unwrap(),
            None => setting::remove(&pool, SITEMAP).await.unwrap(),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn config_plugins_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let mut backups = Vec::new();
        for key in [TIME_FACTOR, JSON_LD, GOOGLE_AUTH] {
            backups.push((key, setting::get_raw(&pool, key).await.unwrap()));
        }

        // 时间因子：默认值补齐 + 运行期计数保留
        let loaded = timefactor_get(&pool).await.unwrap();
        assert_eq!(loaded["open"], json!(false));
        assert!(loaded["module_ids"].is_array());
        setting::save_json(
            &pool,
            TIME_FACTOR,
            &json!({"today_count": 7, "last_update": 123}),
        )
        .await
        .unwrap();
        let saved = timefactor_save(&pool, &json!({"open": true, "daily_update": 5}))
            .await
            .unwrap();
        assert_eq!(saved["open"], json!(true));
        assert_eq!(saved["daily_update"], json!(5));
        assert_eq!(saved["today_count"], json!(7), "运行期计数应保留");
        assert_eq!(saved["last_update"], json!(123));

        // 结构化数据：默认字段齐全 + 字段级 merge
        let jsonld = jsonld_get(&pool).await.unwrap();
        assert!(jsonld.get("address_country").is_some());
        assert!(jsonld.get("module").is_some());
        let saved = jsonld_save(
            &pool,
            &json!({"open": true, "organization_name": "示例组织", "data_type": 1}),
        )
        .await
        .unwrap();
        assert_eq!(saved["open"], json!(true));
        assert_eq!(saved["organization_name"], json!("示例组织"));
        assert_eq!(saved["data_type"], json!(1));
        // 未传字段保持默认
        assert_eq!(saved["street_address"], json!(""));

        // 谷歌认证
        let google = google_get(&pool).await.unwrap();
        assert_eq!(google["client_id"], json!(""));
        let saved = google_save(&pool, &json!({"client_id": "abc.apps.googleusercontent.com"}))
            .await
            .unwrap();
        assert_eq!(saved["client_id"], json!("abc.apps.googleusercontent.com"));

        for (key, raw) in backups {
            match raw {
                Some(raw) => setting::save_raw(&pool, key, &raw).await.unwrap(),
                None => setting::remove(&pool, key).await.unwrap(),
            }
        }
    }

    #[tokio::test]
    async fn limiter_and_interference_defaults() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let mut backups = Vec::new();
        for key in [LIMITER, INTERFERENCE] {
            backups.push((key, setting::get_raw(&pool, key).await.unwrap()));
        }

        // 限流：数组字段与布尔默认值
        let loaded = limiter_get(&pool).await.unwrap();
        assert_eq!(loaded["open"], json!(false));
        assert!(loaded["white_ips"].is_array());
        assert!(loaded["block_agents"].is_array());
        assert_eq!(loaded["max_requests"], json!(0));
        assert!(loaded.get("mem_percent").is_some());

        let saved = limiter_save(&pool, &json!({"open": true, "max_requests": 60}))
            .await
            .unwrap();
        assert_eq!(saved["open"], json!(true));
        assert_eq!(saved["max_requests"], json!(60));
        // 未传字段保持默认
        assert_eq!(saved["is_allow_spider"], json!(false));

        // 干扰码
        let loaded = interference_get(&pool).await.unwrap();
        assert_eq!(loaded["mode"], json!(0));
        assert_eq!(loaded["disable_copy"], json!(false));

        let saved = interference_save(
            &pool,
            &json!({"open": true, "mode": 1, "disable_copy": true}),
        )
        .await
        .unwrap();
        assert_eq!(saved["mode"], json!(1));
        assert_eq!(saved["disable_copy"], json!(true));
        assert_eq!(saved["disable_selection"], json!(false));

        for (key, raw) in backups {
            match raw {
                Some(raw) => setting::save_raw(&pool, key, &raw).await.unwrap(),
                None => setting::remove(&pool, key).await.unwrap(),
            }
        }
    }
}

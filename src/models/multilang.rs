//! 多语言站点（`settings.multi_lang`）。
//!
//! 数据以 `settings.multi_lang` 为准，结构对齐 Go 的 `config.PluginMultiLangConfig`：
//! ```json
//! {"open":false,"type":"directory","auto_translate":false,
//!  "site_type":"multi","show_main_dir":false,
//!  "sub_sites":[{"id":2,"language":"en","language_icon":""}]}
//! ```
//!
//! 说明：
//! - `default_language` 只是**展示字段**，真正的值在 `settings.system.language`；
//! - 子站点只维护配置，**不删站点数据**（与 Go 的 `RemoveMultiLangSite` 一致）；
//! - 内容同步需要跨站点复制 + 翻译外呼，未实现。

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;

use super::{site_setting, website, ModelError};

/// 多语言配置键。
pub const SETTING_KEY: &str = "multi_lang";

/// 子站点保存入参。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SubSiteInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub parent_id: u32,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub language_icon: String,
}

/// 翻译缓存文件。
#[derive(Debug, Clone, Serialize)]
pub struct CacheFile {
    /// 相对 `cache/multiLang` 的路径
    pub path: String,
    pub size: u64,
    pub last_mod: String,
}

fn format_time(time: std::time::SystemTime) -> String {
    let datetime: chrono::DateTime<chrono::Local> = time.into();
    datetime.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn sub_sites(value: &Value) -> Vec<Value> {
    value
        .get("sub_sites")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// 读多语言配置（补默认值 + `default_language`）。
pub async fn config_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, SETTING_KEY).await?;
    let system = site_setting::get_section(pool, site_setting::SYSTEM).await?;
    let language = system
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("zh")
        .to_string();

    if let Value::Object(map) = &mut value {
        map.entry("open").or_insert(json!(false));
        map.entry("type").or_insert(json!("directory"));
        map.entry("auto_translate").or_insert(json!(false));
        map.entry("site_type").or_insert(json!("multi"));
        map.entry("show_main_dir").or_insert(json!(false));
        if !map.get("sub_sites").is_some_and(Value::is_array) {
            map.insert("sub_sites".into(), json!([]));
        }
        map.insert("default_language".into(), json!(language));
    }
    Ok(value)
}

/// 保存多语言配置（字段级 merge）。
///
/// - `site_type` 空值归一为 `multi`；
/// - `default_language` 同步写回 `settings.system.language`；
/// - `site_type = single` 时，把与默认语言相同的子站点清掉。
pub async fn config_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    if !patch.is_object() {
        return Err(anyhow::anyhow!("配置必须是 JSON 对象"));
    }
    let mut patch = patch.clone();
    let default_language = patch
        .get("default_language")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    if let Value::Object(map) = &mut patch {
        map.remove("default_language");
        if let Some(site_type) = map.get("site_type").and_then(Value::as_str) {
            if site_type.trim().is_empty() {
                map.insert("site_type".into(), json!("multi"));
            }
        }
    }

    let saved = site_setting::save_section(pool, SETTING_KEY, &patch).await?;

    // 默认语言回写系统配置
    if let Some(language) = default_language.as_ref() {
        let mut system = site_setting::get_section(pool, site_setting::SYSTEM).await?;
        if let Value::Object(map) = &mut system {
            map.insert("language".into(), json!(language));
        }
        site_setting::save_section(pool, site_setting::SYSTEM, &system).await?;
    }

    // single 模式下，默认语言不该出现在 sub_sites 里
    if saved.get("site_type").and_then(Value::as_str) == Some("single") {
        if let Some(language) = default_language.as_ref() {
            let mut kept: Vec<Value> = sub_sites(&saved)
                .into_iter()
                .filter(|item| item.get("language").and_then(Value::as_str) != Some(language.as_str()))
                .collect();
            if kept.len() != sub_sites(&saved).len() {
                if let Value::Object(map) = &mut patch {
                    map.insert("sub_sites".into(), Value::Array(std::mem::take(&mut kept)));
                }
                return site_setting::save_section(pool, SETTING_KEY, &patch).await;
            }
        }
    }

    config_get(pool).await
}

/// 子站点列表（补上 `websites` 里的名称与根目录）。
pub async fn sites(pool: &MySqlPool) -> anyhow::Result<Vec<Value>> {
    let config = config_get(pool).await?;
    let mut result = Vec::new();
    for mut item in sub_sites(&config) {
        let id = item.get("id").and_then(Value::as_u64).unwrap_or(0) as u32;
        if let Ok(Some(site)) = website::get(pool, id).await {
            if let Value::Object(map) = &mut item {
                map.insert("name".into(), json!(site.name));
                map.insert("root_path".into(), json!(site.root_path));
                map.insert("status".into(), json!(site.status));
            }
        }
        result.push(item);
    }
    Ok(result)
}

/// 可用的多语言站点：`websites` 中排除已被占用的 id 与本站（id = 1）。
pub async fn valid_sites(pool: &MySqlPool) -> anyhow::Result<Vec<website::Website>> {
    let config = config_get(pool).await?;
    let used: std::collections::HashSet<u32> = sub_sites(&config)
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_u64).map(|id| id as u32))
        .collect();

    let (_, rows) = website::list(pool, "", 1, 200).await?;
    Ok(rows
        .into_iter()
        .filter(|site| site.id != 1 && !used.contains(&site.id))
        .collect())
}

/// 新增 / 更新子站点（按 `id` 或 `language` 去重）。
pub async fn site_save(pool: &MySqlPool, input: &SubSiteInput) -> Result<Value, ModelError> {
    let language = input.language.trim();
    if language.is_empty() {
        return Err(ModelError::invalid("请选择语言"));
    }
    if input.id == 0 {
        return Err(ModelError::invalid("请选择站点"));
    }

    let mut config = config_get(pool).await.map_err(|err| {
        ModelError::invalid(format!("读取多语言配置失败: {err}"))
    })?;
    let mut list = sub_sites(&config);

    let position = list.iter().position(|item| {
        item.get("id").and_then(Value::as_u64) == Some(input.id as u64)
            || item.get("language").and_then(Value::as_str) == Some(language)
    });

    let entry = json!({
        "id": input.id,
        "language": language,
        "language_icon": input.language_icon.trim(),
    });
    match position {
        Some(index) => list[index] = entry.clone(),
        None => list.push(entry.clone()),
    }

    if let Value::Object(map) = &mut config {
        map.insert("sub_sites".into(), Value::Array(list));
    }
    site_setting::save_section(pool, SETTING_KEY, &config)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    Ok(entry)
}

/// 移除子站点（只删配置，不删站点数据）。
pub async fn site_remove(
    pool: &MySqlPool,
    id: u32,
    language: &str,
) -> Result<bool, ModelError> {
    let mut config = config_get(pool).await.map_err(|err| {
        ModelError::invalid(format!("读取多语言配置失败: {err}"))
    })?;
    let before = sub_sites(&config);
    let language = language.trim();

    let kept: Vec<Value> = before
        .iter()
        .filter(|item| {
            let same_id = id > 0 && item.get("id").and_then(Value::as_u64) == Some(id as u64);
            let same_language =
                !language.is_empty() && item.get("language").and_then(Value::as_str) == Some(language);
            !same_id && !same_language
        })
        .cloned()
        .collect();

    if kept.len() == before.len() {
        return Ok(false);
    }
    if let Value::Object(map) = &mut config {
        map.insert("sub_sites".into(), Value::Array(kept));
    }
    site_setting::save_section(pool, SETTING_KEY, &config)
        .await
        .map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?;
    Ok(true)
}

/// 翻译缓存目录：`<cache>/multiLang`。
pub fn cache_root(cache_dir: &Path) -> std::path::PathBuf {
    cache_dir.join("multiLang")
}

fn list_files(root: &Path) -> Vec<CacheFile> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            out.push(CacheFile {
                path: rel.to_string_lossy().replace('\\', "/"),
                size: meta.len(),
                last_mod: meta.modified().map(format_time).unwrap_or_default(),
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn paginate(files: Vec<CacheFile>, page: i64, page_size: i64) -> (Vec<CacheFile>, i64) {
    let total = files.len() as i64;
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = ((page - 1) * page_size) as usize;
    let items = files
        .into_iter()
        .skip(offset)
        .take(page_size as usize)
        .collect();
    (items, total)
}

/// 分页列翻译缓存，可按语言目录与 URI 片段过滤。
pub fn html_caches(
    cache_dir: &Path,
    lang: &str,
    uri: &str,
    page: i64,
    page_size: i64,
) -> (Vec<CacheFile>, i64) {
    let lang = lang.trim();
    let uri = uri.trim();
    let files = list_files(&cache_root(cache_dir))
        .into_iter()
        .filter(|file| {
            if lang.is_empty() {
                true
            } else {
                file.path.starts_with(&format!("{lang}/")) || file.path.contains(&format!("/{lang}/"))
            }
        })
        .filter(|file| uri.is_empty() || file.path.contains(uri))
        .collect();
    paginate(files, page, page_size)
}

/// 分页列翻译日志（本项目日志与缓存同目录，按修改时间倒序）。
pub fn html_logs(cache_dir: &Path, page: i64, page_size: i64) -> (Vec<CacheFile>, i64) {
    let mut files = list_files(&cache_root(cache_dir));
    files.sort_by(|a, b| b.last_mod.cmp(&a.last_mod));
    paginate(files, page, page_size)
}

/// 删除翻译缓存：`all = true` 清空目录，否则按相对路径删。
pub fn html_cache_remove(cache_dir: &Path, uris: &[String], all: bool) -> usize {
    let root = cache_root(cache_dir);
    if all {
        let count = list_files(&root).len();
        let _ = std::fs::remove_dir_all(&root);
        return count;
    }

    let mut removed = 0usize;
    for uri in uris {
        let uri = uri.trim().trim_start_matches('/');
        if uri.is_empty() || uri.contains("..") {
            continue;
        }
        let path = root.join(uri);
        if path.starts_with(&root) && path.is_file() && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, setting, test_pool};

    #[tokio::test]
    async fn config_roundtrip_and_defaults() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, SETTING_KEY).await.unwrap();
        let system_backup = setting::get_raw(&pool, site_setting::SYSTEM).await.unwrap();

        let loaded = config_get(&pool).await.unwrap();
        assert_eq!(loaded["site_type"], json!("multi"));
        assert!(loaded["sub_sites"].is_array());
        assert!(loaded["default_language"].is_string());

        // site_type 空值归一
        let saved = config_save(&pool, &json!({"open": true, "site_type": ""}))
            .await
            .unwrap();
        assert_eq!(saved["open"], json!(true));
        assert_eq!(saved["site_type"], json!("multi"));

        // default_language 回写 system.language
        config_save(&pool, &json!({"default_language": "en"}))
            .await
            .unwrap();
        let system = site_setting::get_section(&pool, site_setting::SYSTEM)
            .await
            .unwrap();
        assert_eq!(system["language"], json!("en"));

        // 非对象被拒
        assert!(config_save(&pool, &json!([])).await.is_err());

        match backup {
            Some(raw) => setting::save_raw(&pool, SETTING_KEY, &raw).await.unwrap(),
            None => setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
        match system_backup {
            Some(raw) => setting::save_raw(&pool, site_setting::SYSTEM, &raw)
                .await
                .unwrap(),
            None => setting::remove(&pool, site_setting::SYSTEM).await.unwrap(),
        }
    }

    #[tokio::test]
    async fn site_list_save_remove() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, SETTING_KEY).await.unwrap();
        // 先清空子站点，避免受真实数据影响
        site_setting::save_section(&pool, SETTING_KEY, &json!({"sub_sites": []}))
            .await
            .unwrap();

        // 缺 id / 语言
        assert!(site_save(&pool, &SubSiteInput::default()).await.is_err());
        assert!(site_save(
            &pool,
            &SubSiteInput {
                id: 2,
                language: "".into(),
                ..Default::default()
            }
        )
        .await
        .is_err());

        let saved = site_save(
            &pool,
            &SubSiteInput {
                id: 2,
                language: "en".into(),
                language_icon: "/uploads/en.png".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(saved["language"], json!("en"));

        // 同语言再保存 → 覆盖而不是新增
        site_save(
            &pool,
            &SubSiteInput {
                id: 3,
                language: "en".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(sites(&pool).await.unwrap().len(), 1);

        // 可用站点里不应包含已占用的 3
        let valid = valid_sites(&pool).await.unwrap();
        assert!(!valid.iter().any(|site| site.id == 3));

        // 移除
        assert!(site_remove(&pool, 3, "").await.unwrap());
        assert!(sites(&pool).await.unwrap().is_empty());
        assert!(!site_remove(&pool, 99, "xx").await.unwrap());

        match backup {
            Some(raw) => setting::save_raw(&pool, SETTING_KEY, &raw).await.unwrap(),
            None => setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
    }

    #[test]
    fn cache_files_list_and_remove() {
        let dir = std::env::temp_dir().join(format!("rustcms-ml-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = cache_root(&dir);
        std::fs::create_dir_all(root.join("en")).unwrap();
        std::fs::create_dir_all(root.join("ja")).unwrap();
        std::fs::write(root.join("en").join("index.html"), "en").unwrap();
        std::fs::write(root.join("ja").join("about.html"), "ja").unwrap();

        let (items, total) = html_caches(&dir, "en", "", 1, 20);
        assert_eq!(total, 1);
        assert_eq!(items[0].path, "en/index.html");

        let (items, total) = html_caches(&dir, "", "about", 1, 20);
        assert_eq!(total, 1);
        assert_eq!(items[0].path, "ja/about.html");

        let (_, total) = html_logs(&dir, 1, 20);
        assert_eq!(total, 2);

        // 按路径删
        assert_eq!(html_cache_remove(&dir, &["en/index.html".into()], false), 1);
        // 穿越被忽略
        assert_eq!(html_cache_remove(&dir, &["../etc/passwd".into()], false), 0);
        // 全清
        assert_eq!(html_cache_remove(&dir, &[], true), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

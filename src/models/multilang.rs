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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::mysql::{MySqlPool, MySqlPoolOptions};
use sqlx::Row;

use super::{module, site_setting, website, ModelError};
use crate::config::MysqlConfig;

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

/// 子站点内容同步入参。
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct SiteSyncInput {
    /// 目标子站点 id
    #[serde(default)]
    pub id: u32,
    /// 强制全量同步（忽略目标站点已记录的 `sync_time`）
    #[serde(default)]
    pub focus: bool,
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

// ---------------------------- 内容同步 -------------------------------------

/// 需要同步的表（顺序对齐 Go 版 `SyncMultiLangSiteContent`）。
const SYNC_TABLES: &[&str] = &[
    "modules",
    "nav_types",
    "navs",
    "attachments",
    "categories",
    "tags",
    "tag_contents",
    "archive_data",
    "archives",
    "tag_data",
    "archive_categories",
    "archive_flags",
    "archive_relations",
];

/// 单批最多读取的行数（与 Go 的 `limitSize` 一致）。
const SYNC_BATCH: i64 = 5000;

/// MySQL 单条 SQL 的占位符上限是 65535，留一点余量。
const MAX_PLACEHOLDERS: usize = 60000;

/// 同步任务互斥：同一时间只允许一个同步在跑。
static SYNCING: AtomicBool = AtomicBool::new(false);

/// 守卫：离开作用域时自动释放同步锁（含提前 return / 报错分支）。
struct SyncGuard;

impl SyncGuard {
    fn acquire() -> Result<Self, ModelError> {
        SYNCING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Self)
            .map_err(|_| ModelError::invalid("同步任务正在进行中，请稍后再试"))
    }
}

impl Drop for SyncGuard {
    fn drop(&mut self) {
        SYNCING.store(false, Ordering::SeqCst);
    }
}

/// 单表同步结果。
#[derive(Debug, Clone, Serialize)]
pub struct TableSync {
    pub table: String,
    pub count: u64,
}

/// 同步结果汇总。
#[derive(Debug, Clone, Serialize)]
pub struct SyncReport {
    pub target_id: u32,
    /// 是否全量同步（目标站点从未同步过，或请求里显式 `focus = true`）
    pub full: bool,
    pub total: u64,
    pub tables: Vec<TableSync>,
    pub message: String,
}

/// 校验表名只含字母、数字与下划线，避免拼接 SQL 时被注入。
fn is_safe_table(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn quote(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

/// 从 `websites.mysql` 解析目标站点数据库配置。
///
/// `use_default = true` 时账号密码复用主站配置（与 Go 的 `MysqlConfig.UseDefault` 一致）。
fn target_mysql(site: &website::Website, default_mysql: &MysqlConfig) -> Result<MysqlConfig, ModelError> {
    let raw = site
        .mysql
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ModelError::invalid("目标站点未配置独立数据库，无法同步内容"))?;
    let mut config: MysqlConfig = serde_json::from_str(raw)
        .map_err(|err| ModelError::invalid(format!("目标站点数据库配置解析失败: {err}")))?;

    if config.database.trim().is_empty() {
        return Err(ModelError::invalid("目标站点未填写数据库名称，无法同步内容"));
    }
    if config.use_default {
        config.user = default_mysql.user.clone();
        config.password = default_mysql.password.clone();
        config.host = default_mysql.host.clone();
        config.port = default_mysql.port;
    }
    if config.user.trim().is_empty() {
        config.user = default_mysql.user.clone();
    }
    if config.host.trim().is_empty() {
        config.host = default_mysql.host.clone();
    }
    if config.port == 0 {
        config.port = default_mysql.port;
    }
    Ok(config)
}

/// 读取目标库某表的列名（按库内顺序）；表不存在时返回空列表。
async fn table_columns(pool: &MySqlPool, table: &str) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT COLUMN_NAME FROM information_schema.COLUMNS \
         WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
    )
    .bind(table)
    .fetch_all(pool)
    .await?)
}

/// 把源库一张表的数据逐批复制到目标库（按主键 upsert）。
///
/// 具体做法：`SELECT CAST(列 AS CHAR)` 读成字符串，再用 `INSERT ... ON DUPLICATE KEY UPDATE`
/// 写回，避免为每张表维护一套强类型结构体；MySQL 会按目标列类型自动转换。
async fn copy_table(
    source: &MySqlPool,
    target: &MySqlPool,
    table: &str,
    full: bool,
) -> Result<u64, ModelError> {
    if !is_safe_table(table) {
        return Err(ModelError::invalid(format!("表名非法：{table}")));
    }

    let columns = match table_columns(target, table).await {
        Ok(columns) if !columns.is_empty() => columns,
        Ok(_) => return Ok(0), // 目标库没有这张表，跳过
        Err(err) => return Err(ModelError::invalid(format!("读取 {table} 表结构失败: {err}"))),
    };
    if !columns.iter().any(|column| column.eq_ignore_ascii_case("id")) {
        return Ok(0); // 没有主键 id，无法增量同步
    }

    let start_id: i64 = if full {
        0
    } else {
        let sql = format!("SELECT COALESCE(CAST(MAX(id) AS SIGNED), 0) FROM {}", quote(table));
        sqlx::query_scalar::<_, i64>(&sql)
            .fetch_one(target)
            .await
            .unwrap_or(0)
    };

    let select_sql = format!(
        "SELECT {} FROM {} WHERE id > ? ORDER BY id ASC LIMIT ?",
        columns
            .iter()
            .map(|column| format!("CAST({} AS CHAR)", quote(column)))
            .collect::<Vec<_>>()
            .join(", "),
        quote(table)
    );

    // 每批行数要同时受「行数上限」和「占位符上限」约束
    let batch = (MAX_PLACEHOLDERS / columns.len().max(1)).min(SYNC_BATCH as usize) as i64;
    let batch = batch.max(1);

    let id_index = columns
        .iter()
        .position(|column| column.eq_ignore_ascii_case("id"))
        .unwrap_or(0);

    let placeholders = format!(
        "({})",
        vec!["?"; columns.len()].join(", ")
    );
    let updates = columns
        .iter()
        .map(|column| format!("{0} = VALUES({0})", quote(column)))
        .collect::<Vec<_>>()
        .join(", ");
    let column_list = columns
        .iter()
        .map(|column| quote(column))
        .collect::<Vec<_>>()
        .join(", ");
    let insert_head = format!("INSERT INTO {} ({}) VALUES ", quote(table), column_list);
    let insert_tail = format!(" ON DUPLICATE KEY UPDATE {updates}");

    let mut copied = 0u64;
    let mut cursor = start_id;
    loop {
        let rows = sqlx::query(&select_sql)
            .bind(cursor)
            .bind(batch)
            .fetch_all(source)
            .await
            .map_err(|err| ModelError::invalid(format!("读取 {table} 数据失败: {err}")))?;
        if rows.is_empty() {
            break;
        }

        let mut values: Vec<Option<String>> = Vec::with_capacity(rows.len() * columns.len());
        let mut last_id = cursor;
        for row in &rows {
            for index in 0..columns.len() {
                values.push(row.try_get::<Option<String>, _>(index).unwrap_or(None));
            }
            if let Some(Some(raw)) = values.get(values.len() - columns.len() + id_index) {
                if let Ok(id) = raw.trim().parse::<i64>() {
                    last_id = id;
                }
            }
        }

        let mut sql = String::with_capacity(insert_head.len() + placeholders.len() * rows.len());
        sql.push_str(&insert_head);
        for index in 0..rows.len() {
            if index > 0 {
                sql.push(',');
            }
            sql.push_str(&placeholders);
        }
        sql.push_str(&insert_tail);

        let mut query = sqlx::query(&sql);
        for value in values {
            query = query.bind(value);
        }
        query
            .execute(target)
            .await
            .map_err(|err| ModelError::invalid(format!("写入 {table} 数据失败: {err}")))?;

        copied += rows.len() as u64;
        if last_id <= cursor {
            // id 没前进（异常数据），避免死循环
            break;
        }
        cursor = last_id;
    }

    Ok(copied)
}

/// 把主站内容同步到指定多语言子站点。
///
/// 对齐 Go 版 `SyncMultiLangSiteContent`：同步 modules / 导航 / 附件 / 分类 / 标签 /
/// 文档及关联表；目标站点从未同步过（`sync_time = 0`）或 `focus = true` 时全量，
/// 否则只同步 id 比目标库更大的新增行。
///
/// 未实现的部分：附件文件复制（需要目标站点存储配置）与自动翻译（需要翻译引擎外呼）。
pub async fn site_sync(
    pool: &MySqlPool,
    default_mysql: &MysqlConfig,
    target_id: u32,
    focus: bool,
) -> Result<SyncReport, ModelError> {
    if target_id == 0 {
        return Err(ModelError::invalid("请选择要同步的子站点"));
    }
    if target_id == 1 {
        return Err(ModelError::invalid("不能同步主站点自身"));
    }
    let _guard = SyncGuard::acquire()?;

    let site = website::get(pool, target_id)
        .await
        .map_err(|err| ModelError::invalid(format!("读取子站点信息失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("子站点不存在"))?;
    if site.status == 0 {
        return Err(ModelError::invalid("子站点尚未初始化，无法同步内容"));
    }
    let config = target_mysql(&site, default_mysql)?;
    if config.database == default_mysql.database
        && config.host == default_mysql.host
        && config.port == default_mysql.port
    {
        return Err(ModelError::invalid("子站点与主站使用同一个数据库，无需同步"));
    }

    let target = MySqlPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&crate::db::url(&config))
        .await
        .map_err(|err| ModelError::invalid(format!("连接子站点数据库失败: {err}")))?;

    let sync_time: i64 = sqlx::query_scalar("SELECT COALESCE(CAST(sync_time AS SIGNED), 0) FROM websites WHERE id = ?")
        .bind(target_id)
        .fetch_optional(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("读取同步时间失败: {err}")))?
        .unwrap_or(0);
    let full = focus || sync_time <= 0;

    let mut tables: Vec<String> = SYNC_TABLES.iter().map(|table| table.to_string()).collect();
    if let Ok(modules) = module::list(pool).await {
        for item in modules {
            if let Ok(table) = item.safe_table() {
                if !table.is_empty() && !tables.iter().any(|known| known == table) {
                    tables.push(table.to_string());
                }
            }
        }
    }

    let mut report = SyncReport {
        target_id,
        full,
        total: 0,
        tables: Vec::new(),
        message: String::new(),
    };
    for table in &tables {
        let count = copy_table(pool, &target, table, full).await?;
        if count > 0 || SYNC_TABLES.contains(&table.as_str()) {
            report.total += count;
            report.tables.push(TableSync {
                table: table.clone(),
                count,
            });
        }
    }

    let now = chrono::Utc::now().timestamp();
    sqlx::query("UPDATE websites SET sync_time = ? WHERE id = ?")
        .bind(now)
        .bind(target_id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("更新同步时间失败: {err}")))?;

    let translated = config_get(pool)
        .await
        .map(|value| value.get("auto_translate").and_then(Value::as_bool).unwrap_or(false))
        .unwrap_or(false);
    report.message = if translated {
        "内容已同步；自动翻译需要翻译引擎外呼，尚未执行".to_string()
    } else {
        "内容同步完成".to_string()
    };
    Ok(report)
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

//! 多语言站点（`settings.multi_lang`）。
//!
//! 数据以 `settings.multi_lang` 为准，结构遵循既有约定：
//! ```json
//! {"open":false,"type":"directory","auto_translate":false,
//! "site_type":"multi","show_main_dir":false,
//! "sub_sites":[{"id":2,"language":"en","language_icon":""}]}
//! ```
//!
//! 说明：
//! - `default_language` 只是**展示字段**，真正的值在 `settings.system.language`；
//! - 子站点只维护配置，**不删站点数据**（与既有接口契约一致）；
//! - 内容同步（`site_sync`）会把主站表数据复制到子站点库，并按目标站点存储配置
//! 复制附件文件；`auto_translate` 打开时把新增行交给翻译引擎（自建引擎或云端）。

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

/// 需要同步的表（顺序遵循既有约定）。
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

/// 单批最多读取的行数（与既有接口契约一致）。
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
/// `use_default = true` 时账号密码复用主站配置（与既有接口契约一致）。
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
/// 返回 `(复制行数, 复制前目标库的最大 id)`；第二个值用于「只翻译本次新增的行」。
async fn copy_table(
    source: &MySqlPool,
    target: &MySqlPool,
    table: &str,
    full: bool,
) -> Result<(u64, i64), ModelError> {
    if !is_safe_table(table) {
        return Err(ModelError::invalid(format!("表名非法：{table}")));
    }

    let columns = match table_columns(target, table).await {
        Ok(columns) if !columns.is_empty() => columns,
        Ok(_) => return Ok((0, 0)), // 目标库没有这张表，跳过
        Err(err) => return Err(ModelError::invalid(format!("读取 {table} 表结构失败: {err}"))),
    };
    if !columns.iter().any(|column| column.eq_ignore_ascii_case("id")) {
        return Ok((0, 0)); // 没有主键 id，无法增量同步
    }

    // 目标库现有最大 id：全量同步时从 0 开始复制，翻译也覆盖全部行。
    let baseline: i64 = {
        let sql = format!("SELECT COALESCE(CAST(MAX(id) AS SIGNED), 0) FROM {}", quote(table));
        sqlx::query_scalar::<_, i64>(&sql)
            .fetch_one(target)
            .await
            .unwrap_or(0)
    };
    let start_id: i64 = if full { 0 } else { baseline };

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

    Ok((copied, baseline))
}

// ---------------------------- 附件文件复制 ---------------------------------

/// 把 `file_location` 这类相对路径规范化成安全的相对路径（拒绝 `..`）。
///
/// 附件表里的路径来自上传流程，但仍要当不可信输入处理，避免拼出站点目录之外。
fn safe_relative(path: &str) -> Option<std::path::PathBuf> {
    let trimmed = path.trim().trim_start_matches(['/', '\\']);
    if trimmed.is_empty() {
        return None;
    }
    let mut out = std::path::PathBuf::new();
    for part in trimmed.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return None;
        }
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 附件缩略图路径：同目录下 `thumb_<文件名>`（既有约定的命名）。
fn thumb_relative(rel: &std::path::Path) -> Option<std::path::PathBuf> {
    let name = rel.file_name()?.to_string_lossy().to_string();
    let parent = rel.parent().unwrap_or(std::path::Path::new(""));
    Some(parent.join(format!("thumb_{name}")))
}

/// 复制单个文件（目标已存在则跳过，返回是否真的写了）。
fn copy_file_if_missing(src: &std::path::Path, dst: &std::path::Path) -> bool {
    if !src.is_file() || dst.exists() {
        return false;
    }
    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::copy(src, dst).is_ok()
}

/// 递归复制目录（全量同步时一次性搬 `uploads`）。
fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> u64 {
    let mut copied = 0u64;
    let Ok(entries) = std::fs::read_dir(src) else {
        return copied;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        let target = dst.join(entry.file_name());
        if kind.is_dir() {
            copied += copy_dir_all(&path, &target);
        } else if copy_file_if_missing(&path, &target) {
            copied += 1;
        }
    }
    copied
}

/// 同步附件文件本体：按目标站点存储配置决定是否复制、以及复制方式。
///
/// - 目标站点是**本地存储**：全量同步时整目录搬 `public/uploads`，增量时按附件表
/// 逐个复制（含 `thumb_` 缩略图），与既有实现的分支一致；
/// - 目标站点是**云存储**：既有实现会先读本地文件再上传到目标站点存储，本项目还没有
/// 目标站点的上传能力，这里跳过并返回 0（同步报告里会说明）。
pub async fn copy_attachment_files(
    source_public: &Path,
    target_public: &Path,
    target: &MySqlPool,
    full: bool,
) -> Result<u64, ModelError> {
    let storage = super::plugin::storage_get(target)
        .await
        .map_err(|err| ModelError::invalid(format!("读取目标站点存储配置失败: {err}")))?;
    let storage_type = storage
        .get("storage_type")
        .and_then(Value::as_str)
        .unwrap_or("local")
        .trim()
        .to_string();
    if !storage_type.is_empty() && storage_type != "local" {
        // 云存储：不做本地复制（否则目标站点的图片地址与存储方式不一致）
        return Ok(0);
    }

    let mut copied = 0u64;
    if full {
        // 全量：整目录搬一次，等价 既有约定
        copied += copy_dir_all(&source_public.join("uploads"), &target_public.join("uploads"));
    }

    // 附件表里逐条补齐（缺哪个补哪个，重复执行无副作用）
    let mut cursor: i64 = 0;
    loop {
        let rows = sqlx::query(
            "SELECT id, file_location FROM attachments WHERE id > ? ORDER BY id ASC LIMIT 500",
        )
        .bind(cursor)
        .fetch_all(target)
        .await
        .map_err(|err| ModelError::invalid(format!("读取附件表失败: {err}")))?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let id: u64 = row.try_get::<u64, _>("id").unwrap_or(0);
            if id as i64 > cursor {
                cursor = id as i64;
            }
            let location: String = row
                .try_get::<Option<String>, _>("file_location")
                .unwrap_or(None)
                .unwrap_or_default();
            let Some(rel) = safe_relative(&location) else {
                continue;
            };
            let src = source_public.join(&rel);
            let dst = target_public.join(&rel);
            if copy_file_if_missing(&src, &dst) {
                copied += 1;
            }
            if let Some(thumb_rel) = thumb_relative(&rel) {
                let src = source_public.join(&thumb_rel);
                let dst = target_public.join(&thumb_rel);
                if copy_file_if_missing(&src, &dst) {
                    copied += 1;
                }
            }
        }
        if rows.len() < 500 {
            break;
        }
    }
    Ok(copied)
}

// ---------------------------- 自动翻译 -------------------------------------

/// 自动翻译上下文：源/目标语言 + 引擎配置。
///
/// 遵循既有约定：`settings.translate.engine` 非空时走**自建引擎**，
/// 否则整段提交云端 `/translate`。
struct TranslateContext {
    engine: super::translate::EngineConfig,
    from: String,
    to: String,
}

impl TranslateContext {
    /// 读源站点语言与目标站点语言（目标语言在子站点自己的库里）。
    async fn new(pool: &MySqlPool, target: &MySqlPool) -> Self {
        let source_language = site_setting::get_section(pool, site_setting::SYSTEM)
            .await
            .ok()
            .and_then(|value| {
                value
                    .get("language")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "zh-CN".to_string());
        let target_language = site_setting::get_section(target, site_setting::SYSTEM)
            .await
            .ok()
            .and_then(|value| {
                value
                    .get("language")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        Self {
            engine: super::translate::engine_config(pool).await,
            from: source_language,
            to: target_language,
        }
    }

    /// 纯文本批量翻译。
    async fn texts(&self, pool: &MySqlPool, texts: &[String]) -> Result<Vec<String>, String> {
        if self.to.trim().is_empty() {
            return Err(super::translate::ERR_NO_TO_LANGUAGE.to_string());
        }
        if self.engine.engine().is_some() {
            super::translate::translate_texts(pool, &self.engine, texts, &self.from, &self.to).await
        } else {
            super::cloud::cloud_translate(pool, texts, &self.from, &self.to).await
        }
    }

    /// 正文翻译：自建引擎只翻文本节点（标签原样保留），云端整段交给服务端。
    async fn html(&self, pool: &MySqlPool, html: &str) -> Result<String, String> {
        if html.trim().is_empty() {
            return Ok(html.to_string());
        }
        if self.engine.engine().is_some() {
            super::translate::translate_html(pool, &self.engine, html, &self.from, &self.to).await
        } else {
            let translated = super::cloud::cloud_translate(
                pool,
                &[html.to_string()],
                &self.from,
                &self.to,
            )
            .await?;
            Ok(translated.into_iter().next().unwrap_or_default())
        }
    }
}

/// 一张表的自动翻译规格：普通文本列与 HTML 正文列。
struct TranslateSpec {
    table: &'static str,
    /// 普通文本列（标题 / 摘要 / 关键词这类，逐列独立翻译）
    text_columns: &'static [&'static str],
    /// 正文列（HTML，按文本节点翻译）
    html_columns: &'static [&'static str],
}

/// 需要自动翻译的表（字段口径遵循既有约定的各段 `transReq`）。
const TRANSLATE_SPECS: &[TranslateSpec] = &[
    TranslateSpec {
        table: "modules",
        text_columns: &["title", "description", "keywords"],
        html_columns: &[],
    },
    TranslateSpec {
        table: "navs",
        text_columns: &["title", "description", "sub_title"],
        html_columns: &[],
    },
    TranslateSpec {
        table: "categories",
        text_columns: &["title", "description", "keywords"],
        html_columns: &["content"],
    },
    TranslateSpec {
        table: "tags",
        text_columns: &["title", "description", "keywords"],
        html_columns: &[],
    },
    TranslateSpec {
        table: "archives",
        text_columns: &["title", "description", "keywords"],
        html_columns: &[],
    },
    TranslateSpec {
        table: "archive_data",
        text_columns: &[],
        html_columns: &["content"],
    },
    TranslateSpec {
        table: "tag_contents",
        text_columns: &[],
        html_columns: &["content"],
    },
];

/// 把目标库里 `id > baseline` 的行翻译掉（只处理本次新同步的行）。
///
/// 单行失败只记日志不中断整次同步：翻译是增强能力，不该让内容同步整体失败。
async fn translate_rows(
    pool: &MySqlPool,
    target: &MySqlPool,
    ctx: &TranslateContext,
    spec: &TranslateSpec,
    baseline: i64,
) -> u64 {
    if spec.text_columns.is_empty() && spec.html_columns.is_empty() {
        return 0;
    }
    // 目标表缺列（老库/自定义模型）时直接跳过，不让同步报错
    let existing = table_columns(target, spec.table).await.unwrap_or_default();
    let text_columns: Vec<&str> = spec
        .text_columns
        .iter()
        .copied()
        .filter(|column| existing.iter().any(|name| name.eq_ignore_ascii_case(column)))
        .collect();
    let html_columns: Vec<&str> = spec
        .html_columns
        .iter()
        .copied()
        .filter(|column| existing.iter().any(|name| name.eq_ignore_ascii_case(column)))
        .collect();
    if text_columns.is_empty() && html_columns.is_empty() {
        return 0;
    }

    let select_columns = std::iter::once("id".to_string())
        .chain(text_columns.iter().map(|column| quote(column)))
        .chain(html_columns.iter().map(|column| quote(column)))
        .collect::<Vec<_>>()
        .join(", ");
    let mut cursor = baseline;
    let mut translated = 0u64;
    loop {
        let sql = format!(
            "SELECT {select_columns} FROM {} WHERE id > ? ORDER BY id ASC LIMIT 100",
            quote(spec.table)
        );
        let rows = match sqlx::query(&sql).bind(cursor).fetch_all(target).await {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!("自动翻译读取 {} 失败：{err}", spec.table);
                break;
            }
        };
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let id = row.try_get::<i64, _>("id").unwrap_or(0);
            if id > cursor {
                cursor = id;
            }
            // 1. 普通文本列：一次性提交，成功后按列回写
            let mut texts: Vec<String> = Vec::new();
            let mut keys: Vec<&str> = Vec::new();
            for column in &text_columns {
                let value: String = row
                    .try_get::<Option<String>, _>(*column)
                    .unwrap_or(None)
                    .unwrap_or_default();
                if value.trim().is_empty() {
                    continue;
                }
                texts.push(value);
                keys.push(column);
            }
            if !texts.is_empty() {
                match ctx.texts(pool, &texts).await {
                    Ok(result) if result.len() == texts.len() => {
                        let updates = keys
                            .iter()
                            .zip(result.iter())
                            .map(|(column, _)| format!("{} = ?", quote(column)))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let sql = format!("UPDATE {} SET {updates} WHERE id = ?", quote(spec.table));
                        let mut query = sqlx::query(&sql);
                        for value in &result {
                            query = query.bind(value);
                        }
                        if let Err(err) = query.bind(id).execute(target).await {
                            tracing::warn!("自动翻译回写 {}#{} 失败：{err}", spec.table, id);
                        } else {
                            translated += 1;
                        }
                    }
                    Ok(_) => tracing::warn!("自动翻译 {}#{} 返回数量不符，已跳过", spec.table, id),
                    Err(err) => tracing::warn!("自动翻译 {}#{} 失败：{err}", spec.table, id),
                }
            }
            // 2. 正文列：走 HTML 翻译（只翻文本节点）
            for column in &html_columns {
                let value: String = row
                    .try_get::<Option<String>, _>(*column)
                    .unwrap_or(None)
                    .unwrap_or_default();
                if value.trim().is_empty() {
                    continue;
                }
                match ctx.html(pool, &value).await {
                    Ok(content) => {
                        if content == value {
                            continue;
                        }
                        let sql = format!(
                            "UPDATE {} SET {} = ? WHERE id = ?",
                            quote(spec.table),
                            quote(column)
                        );
                        if let Err(err) = sqlx::query(&sql).bind(&content).bind(id).execute(target).await
                        {
                            tracing::warn!(
                                "自动翻译回写 {}#{}.{} 失败：{err}",
                                spec.table,
                                id,
                                column
                            );
                        } else {
                            translated += 1;
                        }
                    }
                    Err(err) => {
                        tracing::warn!("自动翻译 {}#{}.{} 失败：{err}", spec.table, id, column)
                    }
                }
            }
        }
        if rows.len() < 100 {
            break;
        }
    }
    translated
}

/// 把主站内容同步到指定多语言子站点。
///
/// 遵循既有约定：同步 modules / 导航 / 附件 / 分类 / 标签 /
/// 文档及关联表；目标站点从未同步过（`sync_time = 0`）或 `focus = true` 时全量，
/// 否则只同步 id 比目标库更大的新增行。
///
/// `source_public` 是源站点的 `public` 目录（附件文件从这里复制），
/// 目标目录取子站点自己的 `root_path/public`。
pub async fn site_sync(
    pool: &MySqlPool,
    default_mysql: &MysqlConfig,
    target_id: u32,
    focus: bool,
    source_public: &Path,
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
    // 记录每张表「复制前」的最大 id：自动翻译只处理本次新增的行
    let mut baselines: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for table in &tables {
        let (count, baseline) = copy_table(pool, &target, table, full).await?;
        baselines.insert(table.clone(), baseline);
        if count > 0 || SYNC_TABLES.contains(&table.as_str()) {
            report.total += count;
            report.tables.push(TableSync {
                table: table.clone(),
                count,
            });
        }
    }

    // 附件文件本体：目标站点本地存储时复制（云存储跳过，详见 copy_attachment_files）
    let target_public = Path::new(site.root_path.trim()).join("public");
    let copied_files =
        match copy_attachment_files(source_public, &target_public, &target, full).await {
            Ok(count) => count,
            Err(err) => {
                // 文件复制失败不影响内容同步结论，但要留痕并写进报告
                tracing::warn!("同步附件文件失败：{err}");
                0
            }
        };

    // 自动翻译：只翻本次新同步的行；单行失败不中断整次同步
    let auto_translate = config_get(pool)
        .await
        .map(|value| {
            value
                .get("auto_translate")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .unwrap_or(false);
    let mut translated_rows = 0u64;
    let mut translate_note = String::new();
    if auto_translate {
        let ctx = TranslateContext::new(pool, &target).await;
        if ctx.to.trim().is_empty() {
            translate_note = "子站点未设置语言，已跳过自动翻译".to_string();
        } else {
            for spec in TRANSLATE_SPECS {
                let Some(baseline) = baselines.get(spec.table).copied() else {
                    continue; // 这张表本次没有参与同步
                };
                translated_rows += translate_rows(pool, &target, &ctx, spec, baseline).await;
            }
        }
    }

    let now = chrono::Utc::now().timestamp();
    sqlx::query("UPDATE websites SET sync_time = ? WHERE id = ?")
        .bind(now)
        .bind(target_id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("更新同步时间失败: {err}")))?;

    let mut message = if auto_translate {
        format!("内容同步完成，自动翻译 {translated_rows} 处")
    } else {
        "内容同步完成".to_string()
    };
    if copied_files > 0 {
        message.push_str(&format!("，复制附件文件 {copied_files} 个"));
    }
    if !translate_note.is_empty() {
        message.push_str(&format!("（{translate_note}）"));
    }
    report.message = message;
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

    // ---------------------------- 附件复制纯逻辑 ----------------------------

    #[test]
    fn attachment_paths_are_sanitized() {
        // 正常路径：去掉前导斜杠并逐段拼
        assert_eq!(
            safe_relative("/uploads/202610/01/a.png").unwrap(),
            Path::new("uploads").join("202610").join("01").join("a.png")
        );
        assert_eq!(safe_relative("uploads/a.png").unwrap(), Path::new("uploads/a.png"));
        // 反斜杠形式的路径也要能处理（Windows 上传过来的记录）
        assert_eq!(
            safe_relative("uploads\\a.png").unwrap(),
            Path::new("uploads/a.png")
        );
        // 穿越、空串、只有斜杠 → 拒绝
        assert!(safe_relative("../etc/passwd").is_none());
        assert!(safe_relative("uploads/../../etc/passwd").is_none());
        assert!(safe_relative("").is_none());
        assert!(safe_relative("///").is_none());
        assert!(safe_relative("./").is_none());
    }

    #[test]
    fn thumb_path_keeps_directory() {
        let rel = Path::new("uploads/202610/a.png");
        assert_eq!(
            thumb_relative(rel).unwrap(),
            Path::new("uploads/202610/thumb_a.png")
        );
        // 根目录下的文件
        assert_eq!(thumb_relative(Path::new("a.png")).unwrap(), Path::new("thumb_a.png"));
    }

    #[test]
    fn copies_files_and_directories_idempotently() {
        let base = std::env::temp_dir().join(format!("rustcms-ml-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src");
        let dst = base.join("dst");
        std::fs::create_dir_all(src.join("uploads/202610")).unwrap();
        std::fs::write(src.join("uploads/202610/a.png"), b"img").unwrap();
        std::fs::write(src.join("uploads/202610/b.png"), b"img2").unwrap();

        // 全量：整目录搬
        let copied = copy_dir_all(&src.join("uploads"), &dst.join("uploads"));
        assert_eq!(copied, 2);
        assert!(dst.join("uploads/202610/a.png").is_file());

        // 再搬一次：已存在的不重复计数
        assert_eq!(copy_dir_all(&src.join("uploads"), &dst.join("uploads")), 0);

        // 单文件：目标存在时跳过
        assert!(!copy_file_if_missing(
            &src.join("uploads/202610/a.png"),
            &dst.join("uploads/202610/a.png")
        ));
        // 目标不存在时复制（并自动建父目录）
        let fresh = dst.join("uploads/202610/thumb_a.png");
        assert!(copy_file_if_missing(&src.join("uploads/202610/a.png"), &fresh));
        assert_eq!(std::fs::read(&fresh).unwrap(), b"img");
        // 源不存在时不写
        assert!(!copy_file_if_missing(
            &src.join("uploads/none.png"),
            &dst.join("uploads/none.png")
        ));

        let _ = std::fs::remove_dir_all(&base);
    }
}

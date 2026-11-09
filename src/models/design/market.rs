//!
//! 模板市场能力：打包下载、上传解压、演示数据备份与恢复。
//!
//! 约定：
//! - 打包：`template/<package>/**` + `public/static/<package>/**` → zip；
//! - 上传：从 `template/config.json` 读取包名，写回模板目录与静态目录；
//! - 演示数据：`template/<package>/data.db`（zip，内含各表 JSON 与附件文件）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rustcms_ext::packer::{ZipArchive, ZipWriter};
use serde_json::{json, Map, Value};
use sqlx::mysql::MySqlPool;
use sqlx::Row;

use crate::models::{extra, module, setting, site_setting, ModelError};

use super::{valid_package, DesignPackage, DesignStore};

/// 演示数据每类最多备份 200 条（遵循既有约定）。
const MAX_ROWS: usize = 200;

fn invalid(msg: impl Into<String>) -> ModelError {
    ModelError::invalid(msg)
}

fn pack_err(action: &str, err: anyhow::Error) -> ModelError {
    invalid(format!("{action}失败：{err}"))
}

// ---------------------------------------------------------------------------
// 模板打包 / 上传
// ---------------------------------------------------------------------------

/// 把模板目录与静态目录打包成 zip 字节流。
pub fn package_zip(store: &DesignStore, package: &str) -> Result<Vec<u8>, ModelError> {
    if !valid_package(package) {
        return Err(invalid("模板包名不合法"));
    }
    let template_dir = store.template_root.join(package);
    if !template_dir.is_dir() {
        return Err(invalid("模板不存在"));
    }
    let static_dir = store.static_root.join(package);
    if !static_dir.is_dir() {
        return Err(invalid("模板静态资源目录不存在"));
    }

    let mut writer = ZipWriter::new();
    add_tree(&mut writer, &template_dir, "template")?;
    add_tree(&mut writer, &static_dir, "static")?;
    Ok(writer.finish())
}

/// 递归写入目录；`.开头的文件/目录`跳过（遵循既有约定）。
fn add_tree(writer: &mut ZipWriter, root: &Path, prefix: &str) -> Result<(), ModelError> {
    let mut entries = std::fs::read_dir(root)
        .map_err(|err| invalid(format!("读取目录失败：{err}")))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|err| invalid(format!("读取目录失败：{err}")))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            add_tree(writer, &path, prefix)?;
            continue;
        }
        let data = std::fs::read(&path).map_err(|err| invalid(format!("读取文件失败：{err}")))?;
        writer
            .add_file(&format!("{prefix}/{relative}"), &data)
            .map_err(|err| pack_err("打包", err))?;
    }
    Ok(())
}

/// 上传模板 zip：先解压到临时校验包名，再落地到模板/静态目录。
///
/// `cover = "cover"` 时覆盖同名模板，否则自动追加序号。
pub fn install_zip(
    store: &DesignStore,
    data: &[u8],
    file_name: &str,
    cover: &str,
) -> Result<String, ModelError> {
    let archive = ZipArchive::open(data.to_vec()).map_err(|err| pack_err("解压", err))?;

    let mut info = DesignPackage::default();
    let mut package = Path::new(file_name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().trim().to_string())
        .unwrap_or_default();
    if let Ok(raw) = archive.read_by_name("template/config.json") {
        if let Ok(parsed) = serde_json::from_slice::<DesignPackage>(&raw) {
            info = parsed;
            let name = info.package.trim();
            if valid_package(name) {
                package = name.to_string();
            }
        }
    }
    if !valid_package(&package) {
        return Err(invalid("模板包名不合法"));
    }

    let template_dir = store.template_root.join(&package);
    if template_dir.exists() && cover != "cover" {
        let base = package.clone();
        for index in 1..=9999 {
            let candidate = format!("{base}{index}");
            if !store.template_root.join(&candidate).exists() {
                package = candidate;
                break;
            }
        }
    }

    std::fs::create_dir_all(store.template_root.join(&package))
        .map_err(|err| invalid(format!("创建模板目录失败：{err}")))?;
    std::fs::create_dir_all(store.static_root.join(&package))
        .map_err(|err| invalid(format!("创建静态目录失败：{err}")))?;

    for entry in archive.entries() {
        if entry.is_dir {
            continue;
        }
        let name = entry.name.replace('\\', "/");
        let ext = Path::new(&name)
            .extension()
            .map(|ext| format!(".{}", ext.to_string_lossy().to_ascii_lowercase()))
            .unwrap_or_default();
        if ext == ".php" {
            continue;
        }

        let target = if let Some(relative) = name.strip_prefix("template/") {
            // 模板目录只接受 html / json / db
            if !matches!(ext.as_str(), ".html" | ".json" | ".db") {
                continue;
            }
            safe_join(&store.template_root.join(&package), relative)
        } else if let Some(relative) = name.strip_prefix("static/") {
            safe_join(&store.static_root.join(&package), relative)
        } else {
            continue;
        };

        let Some(target) = target else {
            return Err(invalid(format!("模板包内路径非法：{name}")));
        };
        let bytes = archive
            .read_entry(entry)
            .map_err(|err| pack_err("解压", err))?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| invalid(format!("创建目录失败：{err}")))?;
        }
        std::fs::write(&target, bytes)
            .map_err(|err| invalid(format!("写入模板文件失败：{err}")))?;
    }

    info.package = package.clone();
    if info.name.trim().is_empty() {
        info.name = package.clone();
    }
    info.created = chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let config = store.template_root.join(&package).join("config.json");
    let raw = serde_json::to_string_pretty(&info)?;
    std::fs::write(&config, raw).map_err(|err| invalid(format!("写入模板配置失败：{err}")))?;
    Ok(package)
}

/// 把归档内相对路径安全地拼到 base 下；拒绝绝对路径、盘符与 `..`。
fn safe_join(base: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.trim().replace('\\', "/");
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() || rel.contains(':') || rel.contains('\0') {
        return None;
    }
    if rel.split('/').any(|segment| segment == "..") {
        return None;
    }
    Some(base.join(rel))
}

// ---------------------------------------------------------------------------
// 演示数据备份
// ---------------------------------------------------------------------------

/// 备份当前站点演示数据到 `template/<package>/data.db`。
pub async fn backup_data(
    store: &DesignStore,
    pool: &MySqlPool,
    package: &str,
    public_dir: &Path,
) -> Result<(), ModelError> {
    if !valid_package(package) {
        return Err(invalid("模板包名不合法"));
    }
    let template_dir = store.template_root.join(package);
    if !template_dir.is_dir() {
        return Err(invalid("模板不存在"));
    }

    let mut writer = ZipWriter::new();
    let base_url = site_setting::get_section(pool, site_setting::SYSTEM)
        .await
        .map_err(ModelError::Db)?
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string();
    let storage_url = storage_url(pool).await;

    // 模块定义：既用于输出 modules，也用于补齐 archives 的扩展字段。
    let modules = fetch_rows(pool, "modules", true, Some(MAX_ROWS), None).await?;
    let metas = module_metas(&modules);

    let anchors = fetch_rows(pool, "anchors", true, Some(MAX_ROWS), None).await?;
    if !anchors.is_empty() {
        write_json(&mut writer, "anchors", &anchors, &base_url, &storage_url)?;
        if let Some(condition) = ids_condition(&anchors, "id", "anchor_id") {
            let data = fetch_rows(pool, "anchor_data", false, None, Some(&condition)).await?;
            write_json(&mut writer, "anchorData", &data, &base_url, &storage_url)?;
        }
    }

    let mut archives = fetch_rows(pool, "archives", false, Some(MAX_ROWS), None).await?;
    if !archives.is_empty() {
        attach_extras(pool, &mut archives, &metas).await?;
        write_json(&mut writer, "archives", &archives, &base_url, &storage_url)?;
        if let Some(condition) = ids_condition(&archives, "id", "archive_id") {
            let data = fetch_rows(pool, "archive_data", false, None, Some(&condition)).await?;
            write_json(&mut writer, "archiveData", &data, &base_url, &storage_url)?;
            let data =
                fetch_rows(pool, "archive_categories", false, None, Some(&condition)).await?;
            write_json(
                &mut writer,
                "archiveCategories",
                &data,
                &base_url,
                &storage_url,
            )?;
            let data =
                fetch_rows(pool, "archive_relations", false, None, Some(&condition)).await?;
            write_json(
                &mut writer,
                "archiveRelations",
                &data,
                &base_url,
                &storage_url,
            )?;
            let data = fetch_rows(pool, "archive_flags", false, None, Some(&condition)).await?;
            write_json(
                &mut writer,
                "archiveFlags",
                &data,
                &base_url,
                &storage_url,
            )?;
        }
    }

    let attachments = fetch_rows(pool, "attachments", true, Some(MAX_ROWS), None).await?;
    if !attachments.is_empty() {
        write_json(
            &mut writer,
            "attachments",
            &attachments,
            &base_url,
            &storage_url,
        )?;
        attach_files(&mut writer, &attachments, public_dir)?;
    }

    for (name, table, status) in [
        ("categories", "categories", true),
        ("comments", "comments", true),
        ("guestbooks", "guestbooks", false),
        ("keywords", "keywords", true),
        ("links", "links", true),
        ("materials", "materials", true),
        ("materialCategories", "material_categories", true),
        ("modules", "modules", true),
        ("navs", "navs", true),
        ("navTypes", "nav_types", false),
        ("redirects", "redirects", false),
        ("tags", "tags", true),
        ("userGroups", "user_groups", true),
        ("places", "places", true),
    ] {
        if name == "modules" {
            write_json(
                &mut writer,
                "modules",
                &modules,
                &base_url,
                &storage_url,
            )?;
            continue;
        }
        let rows = fetch_rows(pool, table, status, Some(MAX_ROWS), None).await?;
        write_json(&mut writer, name, &rows, &base_url, &storage_url)?;
    }

    let materials = fetch_rows(pool, "materials", true, Some(MAX_ROWS), None).await?;
    if let Some(condition) = ids_condition(&materials, "id", "material_id") {
        let data = fetch_rows(pool, "material_data", false, None, Some(&condition)).await?;
        write_json(
            &mut writer,
            "materialData",
            &data,
            &base_url,
            &storage_url,
        )?;
    }

    let tags = fetch_rows(pool, "tags", true, Some(MAX_ROWS), None).await?;
    if !tags.is_empty() {
        if let Some(condition) = ids_condition(&tags, "id", "tag_id") {
            let data = fetch_rows(pool, "tag_data", false, None, Some(&condition)).await?;
            write_json(&mut writer, "tagData", &data, &base_url, &storage_url)?;
        }
        if let Some(condition) = ids_condition(&tags, "id", "id") {
            let data = fetch_rows(pool, "tag_contents", false, None, Some(&condition)).await?;
            write_json(
                &mut writer,
                "tagContents",
                &data,
                &base_url,
                &storage_url,
            )?;
        }
    }

    let settings = fetch_rows(
        pool,
        "settings",
        false,
        None,
        Some(
            "`key` NOT IN ('sendmail','importapi','storage','pay','weapp','wechat','anqi',\
             'cache_type','last_run_version','google_auth','akismet','translate',\
             'ai_generate','html_cache','fulltext','push')",
        ),
    )
    .await?;
    write_json(&mut writer, "settings", &settings, &base_url, &storage_url)?;

    let target = template_dir.join("data.db");
    std::fs::write(&target, writer.finish())
        .map_err(|err| invalid(format!("写入演示数据失败：{err}")))?;
    Ok(())
}

async fn storage_url(pool: &MySqlPool) -> String {
    setting::get_json(pool, "storage")
        .await
        .ok()
        .flatten()
        .and_then(|value| {
            value
                .get("storage_url")
                .and_then(Value::as_str)
                .map(|url| url.trim_end_matches('/').to_string())
        })
        .unwrap_or_default()
}

/// 从 rows 里取 ID 列生成 `col IN (...)` 条件。
fn ids_condition(rows: &[Map<String, Value>], id_key: &str, column: &str) -> Option<String> {
    let ids: Vec<String> = rows
        .iter()
        .filter_map(|row| row.get(id_key).and_then(value_id))
        .collect();
    if ids.is_empty() {
        None
    } else {
        Some(format!("`{column}` IN ({})", ids.join(",")))
    }
}

fn value_id(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => text
            .trim()
            .parse::<i64>()
            .ok()
            .map(|number| number.to_string()),
        _ => None,
    }
}

/// 抓取一张表（最多 limit 条）；`status` 条件仅在表里有该列时生效。
async fn fetch_rows(
    pool: &MySqlPool,
    table: &str,
    status_only: bool,
    limit: Option<usize>,
    condition: Option<&str>,
) -> Result<Vec<Map<String, Value>>, ModelError> {
    if !module::table_exists(pool, table).await.map_err(ModelError::Db)? {
        return Ok(Vec::new());
    }
    let columns = table_columns(pool, table).await?;
    if columns.is_empty() {
        return Ok(Vec::new());
    }

    let selects: Vec<String> = columns
        .iter()
        .map(|(name, _)| format!("CAST(`{name}` AS CHAR) AS `{name}`"))
        .collect();

    let mut conditions = Vec::new();
    if status_only && columns.iter().any(|(name, _)| name == "status") {
        conditions.push("`status` = 1".to_string());
    }
    if let Some(condition) = condition {
        conditions.push(condition.to_string());
    }

    let mut sql = format!("SELECT {} FROM `{table}`", selects.join(", "));
    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }
    if columns.iter().any(|(name, _)| name == "id") {
        sql.push_str(" ORDER BY `id` DESC");
    }
    if let Some(limit) = limit {
        sql.push_str(&format!(" LIMIT {limit}"));
    }

    let rows = sqlx::query(&sql)
        .fetch_all(pool)
        .await
        .map_err(|err| ModelError::Db(err.into()))?;
    let mut result = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut object = Map::new();
        for (index, (name, kind)) in columns.iter().enumerate() {
            let raw = row
                .try_get::<Option<String>, _>(index)
                .map_err(|err| ModelError::Db(err.into()))?;
            object.insert(
                name.clone(),
                decode_cell(table, name, kind, raw),
            );
        }
        result.push(object);
    }
    Ok(result)
}

async fn table_columns(pool: &MySqlPool, table: &str) -> Result<Vec<(String, String)>, ModelError> {
    let columns: Vec<(String, String)> = sqlx::query_as(&format!("SHOW COLUMNS FROM `{table}`"))
        .fetch_all(pool)
        .await
        .map_err(|err| ModelError::Db(err.into()))?;
    Ok(columns)
}

fn is_numeric_type(kind: &str) -> bool {
    let kind = kind.to_ascii_lowercase();
    kind.contains("int")
        || kind.contains("decimal")
        || kind.contains("numeric")
        || kind.contains("float")
        || kind.contains("double")
        || kind.contains("real")
}

fn is_json_column(table: &str, column: &str, kind: &str) -> bool {
    if kind.to_ascii_lowercase().starts_with("json") {
        return true;
    }
    matches!(
        (table, column),
        ("archives", "images")
            | ("categories", "images")
            | ("places", "images")
            | ("modules", "fields")
            | ("modules", "category_fields")
    )
}

/// 把 `CAST AS CHAR` 后的文本还原成 JSON 值。
fn decode_cell(table: &str, column: &str, kind: &str, raw: Option<String>) -> Value {
    let Some(raw) = raw else {
        return Value::Null;
    };
    if is_numeric_type(kind) {
        if let Ok(number) = raw.trim().parse::<i64>() {
            return json!(number);
        }
        if let Ok(number) = raw.trim().parse::<u64>() {
            return json!(number);
        }
        if let Ok(number) = raw.trim().parse::<f64>() {
            return json!(number);
        }
        return Value::String(raw);
    }
    if is_json_column(table, column, kind) {
        if let Ok(value) = serde_json::from_str::<Value>(&raw) {
            return value;
        }
    }
    Value::String(raw)
}

fn write_json(
    writer: &mut ZipWriter,
    name: &str,
    rows: &[Map<String, Value>],
    base_url: &str,
    storage_url: &str,
) -> Result<(), ModelError> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut raw = serde_json::to_string(rows)?;
    if !base_url.is_empty() {
        raw = raw.replace(base_url, "");
    }
    if !storage_url.is_empty() && storage_url != base_url {
        raw = raw.replace(storage_url, "");
    }
    writer
        .add_file(name, raw.as_bytes())
        .map_err(|err| pack_err("打包", err))
}

struct ModuleMeta {
    table: String,
    fields: Vec<module::CustomField>,
}

fn module_metas(modules: &[Map<String, Value>]) -> HashMap<i64, ModuleMeta> {
    let mut result = HashMap::new();
    for row in modules {
        let Some(id) = row.get("id").and_then(value_as_i64) else {
            continue;
        };
        let table = row
            .get("table_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !module::is_safe_ident(&table) {
            continue;
        }
        let fields = row
            .get("fields")
            .cloned()
            .and_then(|value| serde_json::from_value::<Vec<module::CustomField>>(value).ok())
            .unwrap_or_default();
        result.insert(id, ModuleMeta { table, fields });
    }
    result
}

fn value_as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// 给 archives 补上 `extra`（模块自定义字段），字段形态与既有实现的
/// `map[string]config.CustomField` 兼容：`{"字段名": {"value": ...}}`。
async fn attach_extras(
    pool: &MySqlPool,
    archives: &mut [Map<String, Value>],
    metas: &HashMap<i64, ModuleMeta>,
) -> Result<(), ModelError> {
    let mut groups: HashMap<i64, Vec<usize>> = HashMap::new();
    for (index, archive) in archives.iter().enumerate() {
        if let Some(module_id) = archive.get("module_id").and_then(value_as_i64) {
            groups.entry(module_id).or_default().push(index);
        }
    }

    for (module_id, indexes) in groups {
        let Some(meta) = metas.get(&module_id) else {
            continue;
        };
        if meta.fields.is_empty()
            || !module::table_exists(pool, &meta.table)
                .await
                .map_err(ModelError::Db)?
        {
            continue;
        }
        let columns = module::table_columns(pool, &meta.table)
            .await
            .map_err(|err| ModelError::Db(err.into()))?;
        let usable: Vec<&module::CustomField> = meta
            .fields
            .iter()
            .filter(|field| columns.contains(&field.field_name))
            .collect();
        if usable.is_empty() {
            continue;
        }

        let ids: Vec<String> = indexes
            .iter()
            .filter_map(|index| archives[*index].get("id").and_then(value_id))
            .collect();
        if ids.is_empty() {
            continue;
        }

        let selects: Vec<String> = usable
            .iter()
            .map(|field| {
                format!(
                    "CAST(`{}` AS CHAR) AS `{}`",
                    field.field_name, field.field_name
                )
            })
            .collect();
        let sql = format!(
            "SELECT CAST(`id` AS CHAR) AS `id`, {} FROM `{}` WHERE `id` IN ({})",
            selects.join(", "),
            meta.table,
            ids.join(",")
        );
        let rows = sqlx::query(&sql)
            .fetch_all(pool)
            .await
            .map_err(|err| ModelError::Db(err.into()))?;
        let mut values: HashMap<i64, Map<String, Value>> = HashMap::new();
        for row in rows {
            let id: Option<String> = row.try_get("id").map_err(|err| ModelError::Db(err.into()))?;
            let Some(id) = id.and_then(|value| value.trim().parse::<i64>().ok()) else {
                continue;
            };
            let mut fields = Map::new();
            for field in &usable {
                let raw: Option<String> = row
                    .try_get(field.field_name.as_str())
                    .map_err(|err| ModelError::Db(err.into()))?;
                let decoded = decode_extra(&field.kind, raw);
                fields.insert(
                    field.field_name.clone(),
                    json!({ "value": decoded }),
                );
            }
            values.insert(id, fields);
        }

        for index in indexes {
            let archive_id = archives[index].get("id").and_then(value_as_i64);
            if let Some(fields) = archive_id.and_then(|id| values.get(&id).cloned()) {
                archives[index].insert("extra".into(), Value::Object(fields));
            }
        }
    }
    Ok(())
}

fn decode_extra(kind: &str, raw: Option<String>) -> Value {
    let Some(raw) = raw else {
        return Value::Null;
    };
    match kind {
        module::TYPE_NUMBER | module::TYPE_CATEGORY => raw
            .trim()
            .parse::<i64>()
            .map(|number| json!(number))
            .unwrap_or_else(|_| Value::String(raw)),
        module::TYPE_IMAGES
        | module::TYPE_TEXTS
        | module::TYPE_ARCHIVE
        | module::TYPE_TIMELINE => serde_json::from_str::<Value>(&raw)
            .unwrap_or_else(|_| Value::String(raw)),
        _ => Value::String(raw),
    }
}

/// 把附件主文件与 `thumb_` 缩略图写进演示数据 zip。
fn attach_files(
    writer: &mut ZipWriter,
    attachments: &[Map<String, Value>],
    public_dir: &Path,
) -> Result<(), ModelError> {
    for attachment in attachments {
        let Some(location) = attachment
            .get("file_location")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let relative = location.trim_start_matches('/').replace('\\', "/");
        if !safe_join(public_dir, &relative).is_some() {
            continue;
        }
        add_public_file(writer, public_dir, &relative)?;

        let path = Path::new(&relative);
        if let (Some(parent), Some(file_name)) = (path.parent(), path.file_name()) {
            let thumb = parent
                .join(format!("thumb_{}", file_name.to_string_lossy()))
                .to_string_lossy()
                .replace('\\', "/");
            add_public_file(writer, public_dir, &thumb)?;
        }
    }
    Ok(())
}

fn add_public_file(
    writer: &mut ZipWriter,
    public_dir: &Path,
    relative: &str,
) -> Result<(), ModelError> {
    let Some(path) = safe_join(public_dir, relative) else {
        return Ok(());
    };
    if !path.is_file() {
        return Ok(());
    }
    let Ok(data) = std::fs::read(&path) else {
        return Ok(());
    };
    writer
        .add_file(&format!("/{}", relative.trim_start_matches('/')), &data)
        .map_err(|err| pack_err("打包", err))
}

// ---------------------------------------------------------------------------
// 演示数据恢复
// ---------------------------------------------------------------------------

/// 用 `template/<package>/data.db` 恢复演示数据。
pub async fn restore_data(
    store: &DesignStore,
    pool: &MySqlPool,
    package: &str,
    public_dir: &Path,
) -> Result<(), ModelError> {
    if !valid_package(package) {
        return Err(invalid("模板包名不合法"));
    }
    let data_path = store.template_root.join(package).join("data.db");
    if !data_path.is_file() {
        return Err(invalid("没有可用的演示数据"));
    }
    let bytes = std::fs::read(&data_path).map_err(|err| invalid(format!("读取演示数据失败：{err}")))?;
    let archive = ZipArchive::open(bytes).map_err(|err| pack_err("读取演示数据", err))?;

    let current_system = site_setting::get_section(pool, site_setting::SYSTEM)
        .await
        .map_err(|err| ModelError::Db(err.into()))?;

    // 必须先处理 settings 与 modules，其它表才有一致的配置/模型定义。
    if let Some(entry) = archive.find("settings") {
        let raw = archive
            .read_entry(entry)
            .map_err(|err| pack_err("读取 settings", err))?;
        let rows = parse_rows(&raw)?;
        restore_settings(pool, rows, &current_system).await?;
    }
    if let Some(entry) = archive.find("modules") {
        let raw = archive
            .read_entry(entry)
            .map_err(|err| pack_err("读取 modules", err))?;
        restore_rows(pool, "modules", parse_rows(&raw)?).await?;
        migrate_modules(pool).await?;
    }

    for entry in archive.entries() {
        if entry.is_dir || entry.name == "settings" || entry.name == "modules" {
            continue;
        }
        let raw = archive
            .read_entry(entry)
            .map_err(|err| pack_err("读取演示数据", err))?;
        if let Some(table) = table_for(&entry.name) {
            let rows = parse_rows(&raw)?;
            if table == "archives" {
                restore_archives(pool, rows).await?;
            } else {
                restore_rows(pool, table, rows).await?;
            }
            continue;
        }
        restore_public_file(public_dir, &entry.name, &raw)?;
    }
    Ok(())
}

/// 通用表数据恢复：逐行 upsert（列名过滤在 `upsert_rows` 内完成）。
async fn restore_rows(
    pool: &MySqlPool,
    table: &str,
    rows: Vec<Map<String, Value>>,
) -> Result<(), ModelError> {
    upsert_rows(pool, table, &rows).await
}

fn parse_rows(raw: &[u8]) -> Result<Vec<Map<String, Value>>, ModelError> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_slice::<Vec<Map<String, Value>>>(raw)
        .map_err(|err| invalid(format!("演示数据格式错误：{err}")))
}

/// 演示数据条目名 → Rust 表名。
fn table_for(name: &str) -> Option<&'static str> {
    Some(match name {
        "anchors" => "anchors",
        "anchorData" => "anchor_data",
        "archives" => "archives",
        "archiveData" => "archive_data",
        "attachments" => "attachments",
        "categories" => "categories",
        "archiveCategories" => "archive_categories",
        "archiveRelations" => "archive_relations",
        "archiveFlags" => "archive_flags",
        "comments" => "comments",
        "guestbooks" => "guestbooks",
        "keywords" => "keywords",
        "links" => "links",
        "materials" => "materials",
        "materialCategories" => "material_categories",
        "materialData" => "material_data",
        "modules" => "modules",
        "navs" => "navs",
        "navTypes" => "nav_types",
        "redirects" => "redirects",
        "settings" => "settings",
        "tags" => "tags",
        "tagData" => "tag_data",
        "tagContents" => "tag_contents",
        "userGroups" => "user_groups",
        "places" => "places",
        _ => return None,
    })
}

/// `settings` 需要特殊处理 system（防域名覆盖）与跳过机密的第三方配置。
async fn restore_settings(
    pool: &MySqlPool,
    rows: Vec<Map<String, Value>>,
    current_system: &Value,
) -> Result<(), ModelError> {
    for mut row in rows {
        let Some(key) = row.get("key").and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        if matches!(key.as_str(), "storage" | "importapi" | "anqi") {
            continue;
        }
        if key == site_setting::SYSTEM {
            let mut system = match row.get("value") {
                Some(Value::String(text)) => {
                    serde_json::from_str::<Value>(text).unwrap_or_else(|_| json!({}))
                }
                Some(Value::Object(map)) => Value::Object(map.clone()),
                _ => json!({}),
            };
            if let Value::Object(map) = &mut system {
                for field in [
                    "template_name",
                    "base_url",
                    "front_url",
                    "mobile_url",
                    "admin_url",
                ] {
                    if let Some(value) = current_system.get(field) {
                        if !value.is_null() {
                            map.insert(field.into(), value.clone());
                            continue;
                        }
                    }
                    if field != "template_name" {
                        map.insert(field.into(), Value::String(String::new()));
                    }
                }
            }
            row.insert("value".into(), Value::String(system.to_string()));
        }
        upsert_rows(pool, "settings", &[row]).await?;
    }
    Ok(())
}

async fn migrate_modules(pool: &MySqlPool) -> Result<(), ModelError> {
    for item in module::list(pool).await.map_err(ModelError::Db)? {
        module::migrate(pool, &item).await?;
    }
    Ok(())
}

/// 通用 upsert：按 JSON 对象里的合法列名写入，主键/唯一键冲突时覆盖。
async fn upsert_rows(
    pool: &MySqlPool,
    table: &str,
    rows: &[Map<String, Value>],
) -> Result<(), ModelError> {
    if rows.is_empty() || !module::table_exists(pool, table).await.map_err(ModelError::Db)? {
        return Ok(());
    }
    let columns: HashSet<String> = module::table_columns(pool, table)
        .await
        .map_err(|err| ModelError::Db(err.into()))?;

    for row in rows {
        let usable: Vec<(&String, &Value)> = row
            .iter()
            .filter(|(name, _)| columns.contains(name.as_str()) && module::is_safe_ident(name))
            .collect();
        if usable.is_empty() {
            continue;
        }

        let names: Vec<String> = usable.iter().map(|(name, _)| format!("`{name}`")).collect();
        let mut query = String::from("INSERT INTO `");
        query.push_str(table);
        query.push_str("` (");
        query.push_str(&names.join(", "));
        query.push_str(") VALUES (");
        let placeholders: Vec<&str> = usable.iter().map(|_| "?").collect();
        query.push_str(&placeholders.join(", "));
        query.push_str(") ON DUPLICATE KEY UPDATE ");
        let updates: Vec<String> = usable
            .iter()
            .map(|(name, _)| format!("`{name}` = VALUES(`{name}`)"))
            .collect();
        query.push_str(&updates.join(", "));

        let mut request = sqlx::query(&query);
        for (_, value) in &usable {
            request = request.bind(to_text(value));
        }
        request.execute(pool).await.map_err(|err| ModelError::Db(err.into()))?;
    }
    Ok(())
}

fn to_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(if *flag { "1" } else { "0" }.to_string()),
        other => Some(other.to_string()),
    }
}

async fn restore_archives(
    pool: &MySqlPool,
    rows: Vec<Map<String, Value>>,
) -> Result<(), ModelError> {
    let mut prepared: HashSet<u32> = HashSet::new();
    for mut row in rows {
        let extra_value = row.remove("extra");
        upsert_rows(pool, "archives", &[row.clone()]).await?;

        let Some(Value::Object(extras)) = extra_value else {
            continue;
        };
        let Some(module_id) = row.get("module_id").and_then(value_as_i64) else {
            continue;
        };
        let Some(archive_id) = row.get("id").and_then(value_as_i64) else {
            continue;
        };
        let Some(module) = module::get(pool, module_id as u32)
            .await
            .map_err(ModelError::Db)?
        else {
            continue;
        };

        let input: Map<String, Value> = extras
            .into_iter()
            .map(|(name, value)| {
                let value = value
                    .get("value")
                    .cloned()
                    .unwrap_or(value);
                (name, value)
            })
            .collect();

        if !prepared.contains(&module.id) {
            extra::prepare(pool, &module).await?;
            prepared.insert(module.id);
        }
        let columns = module::table_columns(pool, module.safe_table()?)
            .await
            .map_err(|err| ModelError::Db(err.into()))?;
        let mut tx = pool.begin().await.map_err(|err| ModelError::Db(err.into()))?;
        extra::save_in_tx(&mut tx, &module, archive_id, &input, &columns).await?;
        tx.commit().await.map_err(|err| ModelError::Db(err.into()))?;
    }
    Ok(())
}

/// 恢复附件等普通文件到 `public/` 下，拒绝任何路径穿越。
fn restore_public_file(public_dir: &Path, name: &str, raw: &[u8]) -> Result<(), ModelError> {
    let relative = name.replace('\\', "/");
    let Some(target) = safe_join(public_dir, &relative) else {
        return Err(invalid(format!("演示数据内文件路径非法：{name}")));
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| invalid(format!("创建目录失败：{err}")))?;
    }
    std::fs::write(&target, raw).map_err(|err| invalid(format!("写入文件失败：{err}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rustcms-design-market-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("template").join("demo")).unwrap();
        std::fs::create_dir_all(dir.join("public").join("static").join("demo")).unwrap();
        std::fs::write(
            dir.join("template").join("demo").join("config.json"),
            r#"{"name":"演示模板","package":"demo","version":"1.0","template_type":1}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("template").join("demo").join("index.html"),
            "<h1>hi</h1>",
        )
        .unwrap();
        std::fs::write(
            dir.join("public")
                .join("static")
                .join("demo")
                .join("a.css"),
            "body{}",
        )
        .unwrap();
        dir
    }

    #[test]
    fn packages_and_installs_template() {
        let source = temp_root("pack");
        let store = DesignStore::new(&source);
        let bytes = package_zip(&store, "demo").unwrap();
        let archive = ZipArchive::open(bytes.clone()).unwrap();
        assert!(archive.find("template/index.html").is_some());
        assert!(archive.find("static/a.css").is_some());

        let target = temp_root("install");
        let target_store = DesignStore::new(&target);
        let installed = install_zip(&target_store, &bytes, "demo.zip", "").unwrap();
        assert_eq!(installed, "demo1");
        assert!(target.join("template").join("demo1").join("index.html").is_file());
        assert!(target
            .join("public")
            .join("static")
            .join("demo1")
            .join("a.css")
            .is_file());

        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn rejects_zip_traversal() {
        let root = temp_root("slip");
        let store = DesignStore::new(&root);
        let mut writer = ZipWriter::new();
        writer
            .add_file("template/../../evil.html", b"x")
            .unwrap();
        let bytes = writer.finish();
        assert!(install_zip(&store, &bytes, "evil.zip", "").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}

//! 网站备份（`pluginBackup.go`、Go `provider/backup.go`）。
//!
//! 备份文件落 `<data>/backup/`：
//! - dump 产出 `.sql`；`.zip` 备份包（`database.sql` + `template/` + `static/`）
//!   可以导入与恢复；
//! - 备注存 sidecar 文件 `<备份文件名>.remark.json`（`{"remark": "..."}`）；
//! - 备份/恢复任务的进度是进程内存态（Go 的全局 `backupStatus`），任务结束
//!   3 秒后自动从状态查询中消失。

use serde::Serialize;
use serde_json::{json, Value};
use sqlx::mysql::{MySqlPool, MySqlRow};
use sqlx::Row;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::ModelError;

/// 备份/恢复任务的执行状态（Go `BackupStatus`）。
#[derive(Debug, Clone, Serialize)]
pub struct BackupStatus {
    pub finished: bool,
    /// `backup` | `restore`（Go `BackupTypeBackup` / `BackupTypeRestore`）。
    #[serde(rename = "type")]
    pub kind: String,
    pub percent: i32,
    pub message: String,
}

impl BackupStatus {
    fn new(kind: &str) -> Self {
        Self {
            finished: false,
            kind: kind.to_string(),
            percent: 0,
            message: String::new(),
        }
    }
}

/// 全局任务状态（进程内存态，对应 Go 的包级 `backupStatus`）。
static BACKUP_STATUS: Mutex<Option<BackupEntry>> = Mutex::new(None);

struct BackupEntry {
    status: BackupStatus,
    finished_at: Option<std::time::Instant>,
}

/// 读当前任务状态；任务结束后 3 秒内仍可见，之后视为无任务（对齐 Go 的 AfterFunc 清理）。
pub fn get_status() -> Option<BackupStatus> {
    let mut guard = BACKUP_STATUS.lock().unwrap_or_else(|err| err.into_inner());
    match guard.as_ref() {
        Some(entry) => {
            if entry.status.finished {
                let elapsed = entry
                    .finished_at
                    .map(|t| t.elapsed().as_secs())
                    .unwrap_or(0);
                if elapsed > 3 {
                    *guard = None;
                    return None;
                }
            }
            Some(entry.status.clone())
        }
        None => None,
    }
}

/// 占用一个新任务槽；已有未完成任务时报错（Go `NewBackup`）。
pub fn new_task(kind: &str) -> Result<(), ModelError> {
    let mut guard = BACKUP_STATUS.lock().unwrap_or_else(|err| err.into_inner());
    if guard
        .as_ref()
        .map(|entry| !entry.status.finished)
        .unwrap_or(false)
    {
        return Err(ModelError::invalid("已有任务在执行，请稍候"));
    }
    *guard = Some(BackupEntry {
        status: BackupStatus::new(kind),
        finished_at: None,
    });
    Ok(())
}

/// 更新当前任务状态（任务不存在时忽略）。
pub fn update_status(kind: &str, percent: i32, message: impl Into<String>) {
    let mut guard = BACKUP_STATUS.lock().unwrap_or_else(|err| err.into_inner());
    if let Some(entry) = guard.as_mut() {
        if entry.status.kind == kind {
            entry.status.percent = percent;
            entry.status.message = message.into();
        }
    }
}

/// 标记任务完成。
pub fn finish_status(kind: &str, message: impl Into<String>) {
    let mut guard = BACKUP_STATUS.lock().unwrap_or_else(|err| err.into_inner());
    if let Some(entry) = guard.as_mut() {
        if entry.status.kind == kind {
            entry.status.finished = true;
            entry.status.percent = 100;
            entry.status.message = message.into();
            entry.finished_at = Some(std::time::Instant::now());
        }
    }
}

/// 备份文件信息（Go `response.BackupInfo`）。
#[derive(Debug, Clone, Serialize)]
pub struct BackupInfo {
    pub name: String,
    pub size: i64,
    pub last_mod: i64,
    pub remark: String,
}

/// 列出 `<data>/backup/` 下的 `.sql`/`.zip` 备份（按修改时间倒序，Go `GetBackupList`）。
pub fn list(backup_dir: &Path) -> Vec<BackupInfo> {
    let Ok(files) = std::fs::read_dir(backup_dir) else {
        return Vec::new();
    };
    let mut result: Vec<BackupInfo> = files
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".sql") && !name.ends_with(".zip") {
                return None;
            }
            let meta = entry.metadata().ok()?;
            let modified = meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs() as i64;
            Some(BackupInfo {
                remark: get_remark(backup_dir, &name),
                name,
                size: meta.len() as i64,
                last_mod: modified,
            })
        })
        .collect();
    result.sort_by(|a, b| b.last_mod.cmp(&a.last_mod));
    result
}

/// 校验备份文件名并返回绝对路径（Go `sanitizeBackupPath`：防路径穿越）。
pub fn sanitize_path(backup_dir: &Path, name: &str) -> Result<PathBuf, ModelError> {
    if name.is_empty() {
        return Err(ModelError::invalid("备份文件不存在"));
    }
    let dir = backup_dir
        .canonicalize()
        .unwrap_or_else(|_| backup_dir.to_path_buf());
    let target = backup_dir.join(name);
    let cleaned = target
        .canonicalize()
        .unwrap_or_else(|_| target.clone());
    let allowed = cleaned.starts_with(&dir);
    // canonicalize 在文件不存在时退回拼接路径，此时按词法校验
    let lex_ok = !name.contains("..") && !name.contains('/') && !name.contains('\\');
    if !(allowed || lex_ok) {
        return Err(ModelError::invalid("文件路径不合法"));
    }
    Ok(cleaned)
}

/// 读备份备注（sidecar JSON；缺失/解析失败返回空串）。
pub fn get_remark(backup_dir: &Path, name: &str) -> String {
    let sidecar = backup_dir.join(format!("{name}.remark.json"));
    let Ok(raw) = std::fs::read_to_string(sidecar) else {
        return String::new();
    };
    serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|v| v["remark"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// 写备份备注（sidecar JSON，Go `SetBackupRemark`）。
pub fn set_remark(backup_dir: &Path, name: &str, remark: &str) -> Result<(), ModelError> {
    let path = sanitize_path(backup_dir, name)?;
    if !path.is_file() {
        return Err(ModelError::invalid("备份文件不存在"));
    }
    let sidecar = backup_dir.join(format!("{name}.remark.json"));
    std::fs::write(&sidecar, json!({ "remark": remark }).to_string())
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))
}

/// 删除备份文件及备注 sidecar（Go `DeleteBackupData`）。
pub fn delete(backup_dir: &Path, name: &str) -> Result<(), ModelError> {
    let path = sanitize_path(backup_dir, name)?;
    if !path.is_file() {
        return Err(ModelError::invalid("备份文件不存在"));
    }
    std::fs::remove_file(&path).map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?;
    let _ = std::fs::remove_file(backup_dir.join(format!("{name}.remark.json")));
    Ok(())
}

/// 导入备份文件（写 `<data>/backup/<name>`，Go `ImportBackupFile`）。
pub fn import(backup_dir: &Path, name: &str, bytes: &[u8]) -> Result<(), ModelError> {
    // 与 Go 一致：导入文件必须是 .sql / .zip
    if !name.ends_with(".sql") && !name.ends_with(".zip") {
        return Err(ModelError::invalid("导入的备份文件格式不正确"));
    }
    sanitize_path(backup_dir, name)?;
    std::fs::create_dir_all(backup_dir)
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    std::fs::write(backup_dir.join(name), bytes)
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))
}

/// 把 SQL 值转义（Go `library.EscapeString`：\0 \n \r \Z ' " \）。
fn escape_sql(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\0' => out.push_str("\\0"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{1a}' => out.push_str("\\Z"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            other => out.push(other),
        }
    }
    out
}

/// 判断列类型是否输出为不带引号的数字字面量。
fn column_is_numeric(db_type: &str) -> bool {
    let t = db_type.to_ascii_lowercase();
    t.contains("int")
        || t.contains("decimal")
        || t.contains("double")
        || t.contains("float")
        || t.contains("bit")
}

/// 判断列类型是否是时间/日期族（输出 CAST 成带引号的字符串）。
fn column_is_temporal(db_type: &str) -> bool {
    let t = db_type.to_ascii_lowercase();
    t.contains("date") || t.contains("time") || t.contains("year")
}

/// 从一行数据生成 SQL 值列表（字符串转义、NULL 直写、数字/时间按列类型 CAST 后处理）。
fn row_values(row: &MySqlRow, casts: &[bool], quoted: &[bool]) -> Result<Vec<String>, sqlx::Error> {
    let mut values = Vec::with_capacity(casts.len());
    for i in 0..casts.len() {
        // CAST 列与文本列都以字符串读出；二进制列按字节 latin-1 映射保真
        let raw: Option<String> = match row.try_get::<Option<String>, _>(i) {
            Ok(value) => value,
            Err(_) => {
                let bytes: Option<Vec<u8>> = row.try_get(i)?;
                bytes.map(|bytes| bytes.iter().map(|b| *b as char).collect())
            }
        };
        let Some(text) = raw else {
            values.push("NULL".into());
            continue;
        };
        if quoted[i] {
            values.push(format!("'{}'", escape_sql(&text)));
        } else {
            values.push(text);
        }
    }
    Ok(values)
}

/// 备份整站到 `<backup_dir>/<时间戳>.zip`（Go `BackupData`）。
///
/// 包内含 `database.sql`、`template/`、`static/`；跳过表名含 `_logs` 的表。
/// 进度按表数推进（0→99）。返回备份文件名。
///
/// zip 条目使用 stored（ext 只实现了解压，没有 deflate 压缩），功能等价，
/// 但体积会大于 Go 的 Deflate 包。
pub async fn dump(
    pool: &MySqlPool,
    backup_dir: &Path,
    template_dir: &Path,
    public_dir: &Path,
) -> Result<String, ModelError> {
    std::fs::create_dir_all(backup_dir)
        .map_err(|err| ModelError::invalid(format!("创建备份目录失败: {err}")))?;
    let file_name = chrono::Local::now().format("%Y%m%d%H%M%S.zip").to_string();
    let target = backup_dir.join(&file_name);

    let tables: Vec<String> = sqlx::query_scalar("SHOW TABLES")
        .fetch_all(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("读取表列表失败: {err}")))?;
    let total = tables.len().max(1);

    let mut sql = String::new();
    for (index, table) in tables.iter().enumerate() {
        update_status("backup", ((index as i32) * 99 / total as i32).min(99), format!("正在备份 {table}"));
        if table.contains("_logs") {
            continue;
        }

        // 表结构（SHOW CREATE TABLE 的第二列）
        let row: (String, String) = sqlx::query_as(&format!("SHOW CREATE TABLE `{table}`"))
            .fetch_one(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("读取表结构失败: {err}")))?;
        sql.push_str(&format!("DROP TABLE IF EXISTS `{table}`;\n"));
        sql.push_str(&row.1.trim_end().trim_end_matches(';').to_string());
        sql.push_str(";\n\n");

        // 列清单 + 类型（SHOW COLUMNS：Field, Type, ...）
        let cols: Vec<(String, String)> =
            sqlx::query_as(&format!("SHOW COLUMNS FROM `{table}`"))
                .fetch_all(pool)
                .await
                .map_err(|err| ModelError::invalid(format!("读取列信息失败: {err}")))?;
        if cols.is_empty() {
            continue;
        }
        let col_list: Vec<String> = cols.iter().map(|c| format!("`{}`", c.0)).collect();
        let selects: Vec<String> = cols
            .iter()
            .map(|c| {
                if column_is_numeric(&c.1) || column_is_temporal(&c.1) {
                    format!("CAST(`{}` AS CHAR)", c.0)
                } else {
                    format!("`{}`", c.0)
                }
            })
            .collect();
        let casts: Vec<bool> = cols
            .iter()
            .map(|c| column_is_numeric(&c.1) || column_is_temporal(&c.1))
            .collect();
        let quoted: Vec<bool> = cols.iter().map(|c| !column_is_numeric(&c.1)).collect();

        sql.push_str(&format!(
            "INSERT INTO `{table}` ({}) VALUES\n",
            col_list.join(",")
        ));
        let sql_query = format!("SELECT {} FROM `{table}`", selects.join(","));
        let rows: Vec<MySqlRow> = sqlx::query(&sql_query)
            .fetch_all(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("读取数据失败: {err}")))?;
        let mut parts = Vec::with_capacity(rows.len());
        for row in &rows {
            let values = row_values(row, &casts, &quoted)
                .map_err(|err| ModelError::invalid(format!("读取数据失败: {err}")))?;
            parts.push(format!("({})", values.join(",")));
        }
        sql.push_str(&parts.join(",\n"));
        if !parts.is_empty() {
            sql.push_str(";\n");
        }
        sql.push('\n');
    }

    let mut writer = rustcms_ext::packer::ZipWriter::new();
    writer
        .add_file("database.sql", sql.as_bytes())
        .map_err(|err| ModelError::invalid(format!("写入 backup database.sql 失败: {err}")))?;

    update_status("backup", 99, "正在备份模板文件");
    add_dir_to_zip(&mut writer, "template/", template_dir)?;

    update_status("backup", 99, "正在备份静态文件");
    let template_name = template_dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "default".to_string());
    let static_dir = public_dir.join("static").join(template_name);
    add_dir_to_zip(&mut writer, "static/", &static_dir)?;

    std::fs::write(&target, writer.finish())
        .map_err(|err| ModelError::invalid(format!("写备份文件失败: {err}")))?;
    finish_status("backup", format!("备份完成：{file_name}"));
    Ok(file_name)
}

/// 把目录下的所有文件加入 zip（条目名带 `prefix`，目录本身不记录条目）。
fn add_dir_to_zip(
    writer: &mut rustcms_ext::packer::ZipWriter,
    prefix: &str,
    dir: &Path,
) -> Result<(), ModelError> {
    fn walk(
        writer: &mut rustcms_ext::packer::ZipWriter,
        base: &Path,
        current: &Path,
        prefix: &str,
    ) -> Result<(), ModelError> {
        let Ok(entries) = std::fs::read_dir(current) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(relative) = path.strip_prefix(base) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                walk(writer, base, &path, prefix)?;
                continue;
            }
            let data = std::fs::read(&path).map_err(|err| {
                ModelError::invalid(format!("读取 {} 失败: {err}", path.display()))
            })?;
            writer
                .add_file(&format!("{prefix}{relative}"), &data)
                .map_err(|err| {
                    ModelError::invalid(format!("写入备份条目 {relative} 失败: {err}"))
                })?;
        }
        Ok(())
    }
    if dir.is_dir() {
        walk(writer, dir, dir, prefix)?;
    }
    Ok(())
}

/// 把 SQL 文本切成单条语句（Go 恢复逻辑的纯函数版）：
/// 逐行累积直到 `;\n` 或 EOF；含 `_logs\`` 的语句跳过。
/// 返回 `(待执行语句, 跳过条数)`。
pub fn split_sql_statements(content: &str) -> (Vec<String>, usize) {
    let mut statements = Vec::new();
    let mut skipped = 0;
    let mut buffer = String::new();
    for line in content.split_inclusive('\n') {
        buffer.push_str(line);
        if line.ends_with(";\n") || (line.ends_with(';') && !content.ends_with(line)) {
            let first_line = buffer.lines().next().unwrap_or_default();
            if first_line.contains("_logs`") {
                skipped += 1;
            } else {
                statements.push(buffer.trim_end().to_string());
            }
            buffer.clear();
        }
    }
    let rest = buffer.trim();
    if !rest.is_empty() {
        let first_line = rest.lines().next().unwrap_or_default();
        if !first_line.contains("_logs`") {
            statements.push(rest.to_string());
        } else {
            skipped += 1;
        }
    }
    (statements, skipped)
}

/// 从备份恢复（Go `RestoreData`）。
///
/// `.sql` 逐语句执行；`.zip` 先还原包内 `template/` 与 `static/`，
/// 再执行 `database.sql`（含 `_logs` 的语句跳过），进度按语句数推进。
pub async fn restore(
    pool: &MySqlPool,
    backup_dir: &Path,
    name: &str,
    template_dir: &Path,
    public_dir: &Path,
) -> Result<(), ModelError> {
    update_status("restore", 0, format!("正在恢复 {name}"));
    let path = sanitize_path(backup_dir, name)?;
    if !path.is_file() {
        return Err(ModelError::invalid("备份文件不存在"));
    }
    if name.ends_with(".zip") {
        return restore_from_zip(pool, &path, template_dir, public_dir).await;
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|err| ModelError::invalid(format!("读取备份失败: {err}")))?;
    execute_sql(pool, &content).await?;
    finish_status("restore", "恢复完成");
    Ok(())
}

/// 逐条执行 SQL（`split_sql_statements` 已过滤 `_logs` 表语句）。
async fn execute_sql(pool: &MySqlPool, content: &str) -> Result<(), ModelError> {
    let (statements, _) = split_sql_statements(content);
    let total = statements.len().max(1);
    for (index, statement) in statements.iter().enumerate() {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(|err| {
                ModelError::invalid(format!("恢复失败（第 {} 条语句）: {err}", index + 1))
            })?;
        update_status(
            "restore",
            ((index as i32 + 1) * 100 / total as i32).min(100),
            "正在恢复数据",
        );
    }
    Ok(())
}

/// 校验包内相对路径：拒绝空段、`.`、`..` 与盘符。
fn safe_relative_path(name: &str) -> Option<PathBuf> {
    let normalized = name.replace('\\', "/");
    let mut path = PathBuf::new();
    for segment in normalized.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." || segment.contains(':') {
            return None;
        }
        path.push(segment);
    }
    (!path.as_os_str().is_empty()).then_some(path)
}

/// 把 zip 包内某个前缀的文件还原到目标目录。
fn extract_zip_prefix(
    archive: &rustcms_ext::packer::ZipArchive,
    prefix: &str,
    dest: &Path,
) -> Result<(), ModelError> {
    for entry in archive.entries() {
        if entry.is_dir {
            continue;
        }
        let Some(relative) = entry.name.strip_prefix(prefix) else {
            continue;
        };
        let Some(relative) = safe_relative_path(relative) else {
            continue;
        };
        let target = dest.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| ModelError::invalid(format!("创建目录失败: {err}")))?;
        }
        let data = archive
            .read_entry(entry)
            .map_err(|err| ModelError::invalid(format!("解压 {} 失败: {err}", entry.name)))?;
        std::fs::write(&target, data)
            .map_err(|err| ModelError::invalid(format!("写入 {} 失败: {err}", target.display())))?;
    }
    Ok(())
}

/// 恢复 `.zip` 备份包（`database.sql` + `template/` + `static/`）。
async fn restore_from_zip(
    pool: &MySqlPool,
    path: &Path,
    template_dir: &Path,
    public_dir: &Path,
) -> Result<(), ModelError> {
    let bytes = std::fs::read(path)
        .map_err(|err| ModelError::invalid(format!("读取备份失败: {err}")))?;
    let archive = rustcms_ext::packer::ZipArchive::open(bytes)
        .map_err(|err| ModelError::invalid(format!("zip 备份包解析失败: {err}")))?;

    // 1. 模板文件还原到 template_dir
    update_status("restore", 0, "正在还原模板文件");
    extract_zip_prefix(&archive, "template/", template_dir)?;

    // 2. 模板静态资源还原到 public/static/<模板目录名>
    update_status("restore", 0, "正在还原静态资源");
    let template_name = template_dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "default".to_string());
    let static_dir = public_dir.join("static").join(template_name);
    extract_zip_prefix(&archive, "static/", &static_dir)?;

    // 3. 最后还原数据库
    let Some(entry) = archive
        .entries()
        .iter()
        .find(|entry| entry.name == "database.sql")
    else {
        return Err(ModelError::invalid("zip 备份包内缺少 database.sql"));
    };
    let data = archive
        .read_entry(entry)
        .map_err(|err| ModelError::invalid(format!("解压 database.sql 失败: {err}")))?;
    let content = String::from_utf8_lossy(&data);
    execute_sql(pool, &content).await?;
    finish_status("restore", "恢复完成");
    Ok(())
}

/// 清空整站数据（Go `CleanupWebsiteData`）。
///
/// TRUNCATE 除 `admins` / `admin_groups` / `admin_login_logs` / `admin_logs` /
/// `settings` / `websites` 外的所有表；`clean_uploads` 时同时删除
/// `public/uploads/`。缓存清理与默认数据重建由调用方处理。
pub async fn cleanup(
    pool: &MySqlPool,
    public_dir: &Path,
    clean_uploads: bool,
) -> Result<usize, ModelError> {
    let tables: Vec<String> = sqlx::query_scalar("SHOW TABLES")
        .fetch_all(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("读取表列表失败: {err}")))?;
    let mut cleaned = 0;
    for table in tables {
        if matches!(
            table.as_str(),
            "admin_groups" | "admin_login_logs" | "admin_logs" | "admins" | "settings" | "websites"
        ) {
            continue;
        }
        // 表名来自 SHOW TABLES，不存在注入面
        sqlx::query(&format!("TRUNCATE `{table}`"))
            .execute(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("清空 {table} 失败: {err}")))?;
        cleaned += 1;
    }
    if clean_uploads {
        let _ = std::fs::remove_dir_all(public_dir.join("uploads"));
    }
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_and_type_checks() {
        assert_eq!(escape_sql("a'b\nc\"d\\e\0f\r\ng"), "a\\'b\\nc\\\"d\\\\e\\0f\\r\\ng");
        assert!(column_is_numeric("bigint(20)"));
        assert!(column_is_numeric("decimal(10,2)"));
        assert!(!column_is_numeric("varchar(100)"));
        assert!(column_is_temporal("datetime"));
        assert!(!column_is_temporal("int(11) unsigned"));
        assert!(!column_is_temporal("text"));
    }

    #[test]
    fn splits_statements_and_skips_logs() {
        let sql = "DROP TABLE IF EXISTS `admin_logs`;\n\
                   CREATE TABLE `t` (id int);\n\
                   INSERT INTO `t` VALUES (1);\n\
                   INSERT INTO `t` VALUES (2);";
        let (statements, skipped) = split_sql_statements(sql);
        assert_eq!(skipped, 1);
        assert_eq!(statements.len(), 3);
        assert!(statements[0].starts_with("CREATE TABLE"));
        assert!(statements[2].ends_with("(2);"));

        // 无结尾分号的最后一条也会被带上
        let (statements, _) = split_sql_statements("INSERT INTO `t` VALUES (1);");
        assert_eq!(statements.len(), 1);
    }

    #[test]
    fn sanitize_blocks_traversal() {
        let dir = std::env::temp_dir().join("rustcms-backup-test");
        let _ = std::fs::create_dir_all(&dir);
        assert!(sanitize_path(&dir, "a.sql").is_ok());
        assert!(sanitize_path(&dir, "").is_err());
        assert!(sanitize_path(&dir, "../config.json").is_err());
        assert!(sanitize_path(&dir, "sub/dir.sql").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn backup_lifecycle_with_sidecar_remark() {
        // 与 dump 测试共享全局任务状态，串行执行
        let _guard = super::super::db_lock().await;
        let dir = std::env::temp_dir().join(format!("rustcms-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 列表为空
        assert!(list(&dir).is_empty());

        // 导入（格式校验 + 落盘）
        assert!(import(&dir, "bad.exe", b"x").is_err());
        import(&dir, "20240101000000.sql", b"SELECT 1;").unwrap();
        let files = list(&dir);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "20240101000000.sql");
        assert_eq!(files[0].size, 9);
        assert!(files[0].remark.is_empty());

        // 备注
        set_remark(&dir, "20240101000000.sql", "第一次备份").unwrap();
        assert_eq!(get_remark(&dir, "20240101000000.sql"), "第一次备份");
        assert!(set_remark(&dir, "nope.sql", "x").is_err());

        // 任务状态生命周期
        assert!(get_status().is_none());
        new_task("backup").unwrap();
        assert!(new_task("backup").is_err(), "已有任务时不能再开");
        update_status("backup", 50, "进行中");
        let status = get_status().unwrap();
        assert_eq!(status.percent, 50);
        assert_eq!(status.kind, "backup");
        assert!(!status.finished);
        finish_status("backup", "完成");
        let status = get_status().unwrap();
        assert!(status.finished);
        assert_eq!(status.percent, 100);

        // 删除（连带 sidecar）
        delete(&dir, "20240101000000.sql").unwrap();
        assert!(list(&dir).is_empty());
        assert_eq!(get_remark(&dir, "20240101000000.sql"), "");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn dump_writes_real_database_zip() {
        // 需要真实库：zip 内应含 database.sql（DROP/CREATE/INSERT，跳过 _logs）+ template/ + static/
        let Some(pool) = super::super::test_pool().await else {
            return;
        };
        let _guard = super::super::db_lock().await;
        let base = std::env::temp_dir().join(format!("rustcms-backup-dump-{}", std::process::id()));
        let dir = base.join("backup");
        let template_dir = base.join("template").join("default");
        let public_dir = base.join("public");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&template_dir).unwrap();
        std::fs::write(template_dir.join("index.html"), "tpl").unwrap();
        std::fs::create_dir_all(public_dir.join("static").join("default")).unwrap();
        std::fs::write(public_dir.join("static").join("default").join("app.css"), "css").unwrap();

        new_task("backup").unwrap();
        let name = dump(&pool, &dir, &template_dir, &public_dir).await.unwrap();
        assert!(name.ends_with(".zip"));
        assert!(get_status().unwrap().finished);

        let archive = rustcms_ext::packer::ZipArchive::open(std::fs::read(dir.join(&name)).unwrap())
            .unwrap();
        assert!(archive.find("database.sql").is_some(), "应含 database.sql");
        assert!(
            archive.find("template/index.html").is_some(),
            "应含模板文件"
        );
        assert!(
            archive.find("static/app.css").is_some(),
            "应含模板静态文件"
        );
        let content = String::from_utf8(archive.read_by_name("database.sql").unwrap()).unwrap();
        assert!(content.contains("DROP TABLE IF EXISTS `settings`"), "应包含 settings 表");
        assert!(content.contains("CREATE TABLE"), "应包含建表语句");
        // _logs 表被跳过
        assert!(!content.contains("CREATE TABLE `admin_logs`"));
        let (statements, _skipped) = split_sql_statements(&content);
        assert!(statements.len() > 3, "应有建表+插入语句");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn packs_directory_into_zip_entries() {
        let root = std::env::temp_dir().join(format!("rustcms-pack-dir-{}", std::process::id()));
        let dir = root.join("tpl");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join("sub").join("b.txt"), "b").unwrap();

        let mut writer = rustcms_ext::packer::ZipWriter::new();
        add_dir_to_zip(&mut writer, "template/", &dir).unwrap();
        let archive = rustcms_ext::packer::ZipArchive::open(writer.finish()).unwrap();
        assert!(archive.find("template/a.txt").is_some());
        assert!(archive.find("template/sub/b.txt").is_some());
        assert_eq!(archive.entries().len(), 2);

        let _ = std::fs::remove_dir_all(&root);
    }
}

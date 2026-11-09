//! 模块（内容模型）与自定义字段定义。
//!
//! 约定：
//! - `modules.fields` / `modules.category_fields` 存的是自定义字段数组的 JSON；
//! - 每个自定义字段在**模块自己的表**（`modules.table_name`，如 `article`/`product`/`case`）
//! 里对应一列，由 `Module::Migrate`（`ALTER TABLE ... ADD COLUMN`）创建；
//! - 列类型映射见 `CustomField::column_type`。

use std::collections::HashSet;

use serde::ser::SerializeSeq;
use serde::{Deserialize, Serialize, Serializer};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::ModelError;

/// 字段类型常量。
pub const TYPE_TEXT: &str = "text";
pub const TYPE_NUMBER: &str = "number";
pub const TYPE_TEXTAREA: &str = "textarea";
pub const TYPE_EDITOR: &str = "editor";
pub const TYPE_RADIO: &str = "radio";
pub const TYPE_CHECKBOX: &str = "checkbox";
pub const TYPE_SELECT: &str = "select";
pub const TYPE_IMAGE: &str = "image";
pub const TYPE_FILE: &str = "file";
pub const TYPE_IMAGES: &str = "images";
pub const TYPE_TEXTS: &str = "texts";
pub const TYPE_ARCHIVE: &str = "archive";
pub const TYPE_CATEGORY: &str = "category";
pub const TYPE_DATE: &str = "date";
pub const TYPE_TIME: &str = "time";
pub const TYPE_DATETIME: &str = "datetime";
pub const TYPE_COLOR: &str = "color";
pub const TYPE_TIMELINE: &str = "timeline";

/// 自定义字段定义。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CustomField {
    /// 字段名称（展示用）
    #[serde(default)]
    pub name: String,
    /// 字段标识（= 数据库列名）
    #[serde(default)]
    pub field_name: String,
    #[serde(default)]
    pub group: String,
    /// 字段类型
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub remark: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub is_system: bool,
    #[serde(default)]
    pub is_filter: bool,
    #[serde(default)]
    pub follow_level: bool,
    /// radio/select/checkbox 的选项配置（每行一个）
    #[serde(default)]
    pub content: String,
}

impl CustomField {
    /// 建表语句里的列定义（`config.CustomField.GetFieldColumn`）。
    pub fn column_type(&self) -> &'static str {
        match self.kind.as_str() {
            TYPE_NUMBER | TYPE_CATEGORY => "int(10)",
            TYPE_TEXTAREA | TYPE_IMAGES | TYPE_TEXTS | TYPE_ARCHIVE => "text",
            TYPE_EDITOR | TYPE_TIMELINE => "longtext",
            TYPE_CHECKBOX => "varchar(500)",
            _ => "varchar(190)",
        }
    }

    /// 数值型（写入时绑定整数）。
    pub fn is_numeric(&self) -> bool {
        matches!(self.kind.as_str(), TYPE_NUMBER | TYPE_CATEGORY)
    }

    /// 存 JSON 的数组型。
    pub fn is_json_array(&self) -> bool {
        matches!(self.kind.as_str(), TYPE_IMAGES | TYPE_TEXTS | TYPE_ARCHIVE)
    }

    /// 选项列表（按行拆分）。
    pub fn items(&self) -> Vec<String> {
        self.content
            .lines()
            .map(|line| line.trim().trim_matches('"'))
            .filter(|line| !line.is_empty())
            .map(|line| line.to_string())
            .collect()
    }
}

/// 表名/列名白名单校验：只允许字母数字下划线，最长 64。
pub fn is_safe_ident(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !name.chars().next().is_some_and(|ch| ch.is_ascii_digit())
}

/// 模块（内容模型）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Module {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub table_name: String,
    pub name: String,
    pub url_token: String,
    pub title: String,
    pub keywords: String,
    pub description: String,
    /// `[]CustomField` 的 JSON 文本，序列化时输出成数组
    #[serde(serialize_with = "serialize_fields")]
    pub fields: Option<String>,
    /// 分类扩展字段定义
    #[serde(serialize_with = "serialize_fields")]
    pub category_fields: Option<String>,
    pub is_system: u8,
    pub status: u8,
}

fn parse_fields(raw: Option<&str>) -> Vec<CustomField> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Vec::new();
    };
    if raw == "null" || raw == "[]" {
        return Vec::new();
    }
    serde_json::from_str::<Vec<CustomField>>(raw).unwrap_or_default()
}

fn serialize_fields<S: Serializer>(raw: &Option<String>, serializer: S) -> Result<S::Ok, S::Error> {
    let list = parse_fields(raw.as_deref());
    let mut seq = serializer.serialize_seq(Some(list.len()))?;
    for item in &list {
        seq.serialize_element(item)?;
    }
    seq.end()
}

impl Module {
    /// 内容字段定义。
    pub fn field_defs(&self) -> Vec<CustomField> {
        parse_fields(self.fields.as_deref())
            .into_iter()
            .filter(|field| is_safe_ident(&field.field_name))
            .collect()
    }

    /// 分类字段定义。
    pub fn category_field_defs(&self) -> Vec<CustomField> {
        parse_fields(self.category_fields.as_deref())
            .into_iter()
            .filter(|field| is_safe_ident(&field.field_name))
            .collect()
    }

    /// 模块扩展表名（已校验）。
    pub fn safe_table(&self) -> Result<&str, ModelError> {
        if is_safe_ident(&self.table_name) {
            Ok(self.table_name.as_str())
        } else {
            Err(ModelError::invalid("模块数据表名非法"))
        }
    }
}

/// 全部模块。
pub async fn list(pool: &MySqlPool) -> anyhow::Result<Vec<Module>> {
    let list = sqlx::query_as::<_, Module>("SELECT * FROM modules ORDER BY id ASC")
        .fetch_all(pool)
        .await?;
    Ok(list)
}

/// 按 ID 取模块。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Module>> {
    let module = sqlx::query_as::<_, Module>("SELECT * FROM modules WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(module)
}

/// 按表名取模块。
pub async fn get_by_table(pool: &MySqlPool, table_name: &str) -> anyhow::Result<Option<Module>> {
    let module = sqlx::query_as::<_, Module>("SELECT * FROM modules WHERE table_name = ? LIMIT 1")
        .bind(table_name)
        .fetch_optional(pool)
        .await?;
    Ok(module)
}

/// 按 url_token 取模块（前台路由用，走进程内缓存）。
pub async fn get_by_token(pool: &MySqlPool, url_token: &str) -> anyhow::Result<Option<Module>> {
    let module = sqlx::query_as::<_, Module>("SELECT * FROM modules WHERE url_token = ? LIMIT 1")
        .bind(url_token)
        .fetch_optional(pool)
        .await?;
    Ok(module)
}

/// 表里已有的列名集合。
pub async fn table_columns(pool: &MySqlPool, table_name: &str) -> anyhow::Result<HashSet<String>> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns \
         WHERE table_schema = DATABASE() AND table_name = ?",
    )
    .bind(table_name)
    .fetch_all(pool)
    .await?;
    Ok(names.into_iter().collect())
}

/// 模块扩展表是否存在。
pub async fn table_exists(pool: &MySqlPool, table_name: &str) -> anyhow::Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = ?",
    )
    .bind(table_name)
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
}

/// 建表 + 补齐缺失的自定义字段列（`model.Module.Migrate`）。
///
/// 只做 `ADD COLUMN`，不动已有列的类型（既有实现里的 MODIFY 由「重建字段」触发，
/// 那是后台高危操作，等后台模块管理做出来再补）。
pub async fn migrate(pool: &MySqlPool, module: &Module) -> Result<(), ModelError> {
    let table = module.safe_table()?.to_string();

    if !table_exists(pool, &table).await? {
        // 表名已通过 is_safe_ident 校验，这里直接拼接
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS `{table}` (`id` bigint(20) NOT NULL AUTO_INCREMENT, PRIMARY KEY (`id`)) DEFAULT CHARSET=utf8mb4"
        );
        sqlx::query(&sql).execute(pool).await?;
    }

    // 补列走 `rustcms_ext::tblsync`：列存在性判断用 `COLUMN_TYPE` 归一化后比较，
    // DDL 由 `add_column_sql` 生成（表名/列名统一加反引号）。语义仍是「只补不改」。
    let columns: Vec<rustcms_ext::tblsync::ColumnDef> = module
        .field_defs()
        .iter()
        .map(|field| {
            rustcms_ext::tblsync::ColumnDef::new(field.field_name.clone(), field.column_type())
                .with_default("NULL")
        })
        .collect();
    let migrator = rustcms_ext::tblsync::Migrator::new(pool.clone());
    migrator.ensure_columns(&table, &columns).await?;

    Ok(())
}

/// 模型的新建 / 更新入参（对应既有约定）。
#[derive(Debug, Default, Clone, Deserialize)]
pub struct ModuleInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub table_name: Option<String>,
    #[serde(default)]
    pub url_token: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub keywords: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub fields: Option<Vec<CustomField>>,
    #[serde(default)]
    pub category_fields: Option<Vec<CustomField>>,
    #[serde(default)]
    pub is_system: Option<u8>,
    #[serde(default)]
    pub status: Option<u8>,
    #[serde(default)]
    pub update_all: bool,
    #[serde(default)]
    pub partial: bool,
}

/// 表名 / URL 别名合法性：小写字母开头，后接小写字母、数字、下划线。
fn valid_ident(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

/// 字段标识合法性（既有实现用 `^[a-z][0-9a-z_]+$`，即至少两个字符）。
fn valid_field_name(value: &str) -> bool {
    value.len() >= 2 && valid_ident(value)
}

/// 新建 / 更新模型（遵循既有约定的核心语义）。
pub async fn save(pool: &MySqlPool, input: &ModuleInput) -> Result<Module, ModelError> {
    let update_all = input.update_all || !input.partial;

    let mut module = if input.id > 0 {
        match get(pool, input.id).await? {
            Some(found) => found,
            None => Module {
                id: input.id,
                created_time: None,
                updated_time: None,
                table_name: String::new(),
                name: String::new(),
                url_token: String::new(),
                title: String::new(),
                keywords: String::new(),
                description: String::new(),
                fields: None,
                category_fields: None,
                is_system: 0,
                status: 1,
            },
        }
    } else {
        Module {
            id: 0,
            created_time: None,
            updated_time: None,
            table_name: String::new(),
            name: String::new(),
            url_token: String::new(),
            title: String::new(),
            keywords: String::new(),
            description: String::new(),
            fields: None,
            category_fields: None,
            is_system: 0,
            status: 1,
        }
    };

    let old_table = module.table_name.clone();

    if update_all || input.table_name.as_deref().is_some_and(|v| !v.is_empty()) {
        let table = input.table_name.as_deref().unwrap_or_default().trim();
        if !valid_ident(table) {
            return Err(ModelError::invalid("请正确填写模型表名"));
        }
        if let Some(exists) = get_by_table(pool, table).await? {
            if exists.id != input.id {
                return Err(ModelError::invalid("模型表名已存在"));
            }
        }
        if old_table != table
            && !old_table.is_empty()
            && table_exists(pool, table).await?
        {
            return Err(ModelError::invalid("模型表名已存在"));
        }
        module.table_name = table.to_string();
    }

    if update_all || input.url_token.as_deref().is_some_and(|v| !v.is_empty()) {
        let token = input.url_token.as_deref().unwrap_or_default().trim();
        if !valid_ident(token) {
            return Err(ModelError::invalid("请正确填写 URL 别名"));
        }
        let exists: Option<u32> =
            sqlx::query_scalar("SELECT id FROM modules WHERE url_token = ? LIMIT 1")
                .bind(token)
                .fetch_optional(pool)
                .await?;
        if exists.is_some_and(|id| id != input.id) {
            return Err(ModelError::invalid("模型 URL 别名已存在"));
        }
        module.url_token = token.to_string();
    }

    if update_all || input.fields.is_some() {
        if let Some(fields) = input.fields.as_ref() {
            let taken = table_columns(pool, "archives").await?;
            for field in fields {
                if !valid_field_name(&field.field_name) {
                    return Err(ModelError::invalid(format!(
                        "{}命名不正确",
                        field.field_name
                    )));
                }
                if taken.contains(&field.field_name) {
                    return Err(ModelError::invalid(format!(
                        "{}字段已存在",
                        field.field_name
                    )));
                }
            }
            module.fields = Some(serde_json::to_string(fields)?);
        }
    }
    if update_all || input.category_fields.is_some() {
        if let Some(fields) = input.category_fields.as_ref() {
            let taken = table_columns(pool, "categories").await?;
            for field in fields {
                if !valid_field_name(&field.field_name) {
                    return Err(ModelError::invalid(format!(
                        "{}命名不正确",
                        field.field_name
                    )));
                }
                if taken.contains(&field.field_name) {
                    return Err(ModelError::invalid(format!(
                        "{}字段已存在",
                        field.field_name
                    )));
                }
            }
            module.category_fields = Some(serde_json::to_string(fields)?);
        }
    }

    if let Some(value) = input.title.as_deref() {
        if update_all || !value.is_empty() {
            module.title = value.to_string();
        }
    }
    if let Some(value) = input.name.as_deref() {
        if update_all || !value.is_empty() {
            module.name = value.to_string();
        }
    }
    if let Some(value) = input.keywords.as_deref() {
        if update_all || !value.is_empty() {
            module.keywords = value.to_string();
        }
    }
    if let Some(value) = input.description.as_deref() {
        if update_all || !value.is_empty() {
            module.description = value.to_string();
        }
    }
    if let Some(value) = input.status {
        if update_all || value > 0 {
            module.status = value;
        }
    }
    if let Some(value) = input.is_system {
        if update_all || value > 0 {
            module.is_system = value;
        }
    }

    let now = chrono::Utc::now().timestamp();
    if module.id > 0 && get(pool, module.id).await?.is_some() {
        module.updated_time = Some(now);
        sqlx::query(
            "UPDATE modules SET updated_time = ?, table_name = ?, name = ?, url_token = ?, \
                title = ?, keywords = ?, description = ?, fields = ?, category_fields = ?, \
                is_system = ?, status = ? WHERE id = ?",
        )
        .bind(module.updated_time)
        .bind(&module.table_name)
        .bind(&module.name)
        .bind(&module.url_token)
        .bind(&module.title)
        .bind(&module.keywords)
        .bind(&module.description)
        .bind(&module.fields)
        .bind(&module.category_fields)
        .bind(module.is_system)
        .bind(module.status)
        .bind(module.id)
        .execute(pool)
        .await?;
    } else {
        module.created_time = Some(now);
        module.updated_time = Some(now);
        let result = if module.id > 0 {
            sqlx::query(
                "INSERT INTO modules (id, created_time, updated_time, table_name, name, url_token, \
                    title, keywords, description, fields, category_fields, is_system, status) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(module.id)
            .bind(module.created_time)
            .bind(module.updated_time)
            .bind(&module.table_name)
            .bind(&module.name)
            .bind(&module.url_token)
            .bind(&module.title)
            .bind(&module.keywords)
            .bind(&module.description)
            .bind(&module.fields)
            .bind(&module.category_fields)
            .bind(module.is_system)
            .bind(module.status)
            .execute(pool)
            .await?
        } else {
            sqlx::query(
                "INSERT INTO modules (created_time, updated_time, table_name, name, url_token, \
                    title, keywords, description, fields, category_fields, is_system, status) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(module.created_time)
            .bind(module.updated_time)
            .bind(&module.table_name)
            .bind(&module.name)
            .bind(&module.url_token)
            .bind(&module.title)
            .bind(&module.keywords)
            .bind(&module.description)
            .bind(&module.fields)
            .bind(&module.category_fields)
            .bind(module.is_system)
            .bind(module.status)
            .execute(pool)
            .await?
        };
        module.id = result.last_insert_id() as u32;
    }

    // 表名变更时同步重命名扩展表
    if !old_table.is_empty() && old_table != module.table_name && table_exists(pool, &old_table).await? {
        let sql = format!(
            "RENAME TABLE `{}` TO `{}`",
            old_table, module.table_name
        );
        sqlx::query(&sql).execute(pool).await?;
    }

    migrate(pool, &module).await?;
    Ok(module)
}

/// 删除模型：内容、草稿、扩展表、栏目、模型行一起清掉（对齐 `provider.DeleteModule`）。
pub async fn delete(pool: &MySqlPool, id: u32) -> anyhow::Result<bool> {
    let Some(module) = get(pool, id).await? else {
        return Ok(false);
    };

    // 内容与关联表
    for table in ["archive_data", "archive_categories", "archive_flags", "archive_relations"] {
        let sql = format!(
            "DELETE FROM `{table}` WHERE id IN (SELECT id FROM archives WHERE module_id = ?)"
        );
        // archive_categories / archive_relations 的关联列名是 archive_id
        let sql = if table == "archive_categories" || table == "archive_relations" {
            format!(
                "DELETE FROM `{table}` WHERE archive_id IN (SELECT id FROM archives WHERE module_id = ?)"
            )
        } else {
            sql
        };
        // 表可能不存在（历史库），失败不阻断
        let _ = sqlx::query(&sql).bind(id).execute(pool).await;
    }
    let _ = sqlx::query("DELETE FROM archives WHERE module_id = ?")
        .bind(id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM archive_drafts WHERE module_id = ?")
        .bind(id)
        .execute(pool)
        .await;

    // 扩展表
    if !module.table_name.is_empty() && is_safe_ident(&module.table_name) {
        let sql = format!("DROP TABLE IF EXISTS `{}`", module.table_name);
        sqlx::query(&sql).execute(pool).await?;
    }

    // 模型下的栏目
    sqlx::query("DELETE FROM categories WHERE module_id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    sqlx::query("DELETE FROM modules WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(true)
}

/// 删除模型里的一个自定义字段：从 JSON 里移除，并 DROP 对应列。
pub async fn delete_field(
    pool: &MySqlPool,
    module_id: u32,
    field_name: &str,
) -> Result<bool, ModelError> {
    let Some(mut module) = get(pool, module_id).await? else {
        return Ok(false);
    };
    let mut fields = module.field_defs();
    let before = fields.len();
    fields.retain(|field| field.field_name != field_name);
    if fields.len() == before {
        return Ok(false);
    }

    if is_safe_ident(field_name) && table_exists(pool, &module.table_name).await? {
        let columns = table_columns(pool, &module.table_name).await?;
        if columns.contains(field_name) {
            let sql = format!(
                "ALTER TABLE `{}` DROP COLUMN `{}`",
                module.table_name, field_name
            );
            sqlx::query(&sql).execute(pool).await?;
        }
    }

    module.fields = Some(serde_json::to_string(&fields)?);
    sqlx::query("UPDATE modules SET fields = ?, updated_time = ? WHERE id = ?")
        .bind(&module.fields)
        .bind(chrono::Utc::now().timestamp())
        .bind(module_id)
        .execute(pool)
        .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, kind: &str) -> CustomField {
        CustomField {
            name: name.to_string(),
            field_name: name.to_string(),
            kind: kind.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn maps_column_types() {
        assert_eq!(field("a", TYPE_TEXT).column_type(), "varchar(190)");
        assert_eq!(field("a", TYPE_NUMBER).column_type(), "int(10)");
        assert_eq!(field("a", TYPE_CHECKBOX).column_type(), "varchar(500)");
        assert_eq!(field("a", TYPE_EDITOR).column_type(), "longtext");
        assert_eq!(field("a", TYPE_IMAGES).column_type(), "text");
        assert!(field("a", TYPE_NUMBER).is_numeric());
        assert!(field("a", TYPE_IMAGES).is_json_array());
    }

    #[test]
    fn validates_identifiers() {
        assert!(is_safe_ident("author_name"));
        assert!(is_safe_ident("f1"));
        assert!(!is_safe_ident(""));
        assert!(!is_safe_ident("1abc"));
        assert!(!is_safe_ident("a-b"));
        assert!(!is_safe_ident("a`b"));
        assert!(!is_safe_ident("中文"));
        assert!(!is_safe_ident(&"x".repeat(65)));
    }

    #[test]
    fn parses_field_definitions() {
        let raw = r#"[
            {"name":"作者","field_name":"author","type":"text","required":true},
            {"name":"价格","field_name":"cost","type":"number"},
            {"name":"危险","field_name":"a-b","type":"text"}
        ]"#;
        let module = Module {
            id: 1,
            created_time: None,
            updated_time: None,
            table_name: "article".into(),
            name: "文章模块".into(),
            url_token: "article".into(),
            title: String::new(),
            keywords: String::new(),
            description: String::new(),
            fields: Some(raw.to_string()),
            category_fields: None,
            is_system: 1,
            status: 1,
        };
        let defs = module.field_defs();
        // 非法标识（a-b）被过滤掉
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].field_name, "author");
        assert!(defs[0].required);
        assert_eq!(defs[1].column_type(), "int(10)");

        // null / 空串都当没有字段
        let empty = Module {
            fields: Some("null".into()),
            ..module.clone()
        };
        assert!(empty.field_defs().is_empty());
    }

    #[tokio::test]
    async fn reads_real_modules() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };

        let modules = list(&pool).await.unwrap();
        assert!(!modules.is_empty());
        let first = &modules[0];
        let by_id = get(&pool, first.id).await.unwrap().expect("模块应存在");
        assert_eq!(by_id.name, first.name);
        let by_table = get_by_table(&pool, &first.table_name)
            .await
            .unwrap()
            .expect("按表名应能取到");
        assert_eq!(by_table.id, first.id);
        assert!(get(&pool, u32::MAX).await.unwrap().is_none());

        // 模块表一定存在，且至少有 id 列
        assert!(table_exists(&pool, &first.table_name).await.unwrap());
        let columns = table_columns(&pool, &first.table_name).await.unwrap();
        assert!(columns.contains("id"));
    }

    #[tokio::test]
    async fn migrates_temp_module_table() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };

        let table = "rustcms_test_module";
        let _ = sqlx::query("DROP TABLE IF EXISTS `rustcms_test_module`")
            .execute(&pool)
            .await;

        let module = Module {
            id: 9_999,
            created_time: None,
            updated_time: None,
            table_name: table.to_string(),
            name: "测试模块".into(),
            url_token: "rustcms_test".into(),
            title: String::new(),
            keywords: String::new(),
            description: String::new(),
            fields: Some(
                r#"[{"name":"作者","field_name":"author","type":"text"},
                    {"name":"页数","field_name":"pages","type":"number"},
                    {"name":"图集","field_name":"gallery","type":"images"}]"#
                    .to_string(),
            ),
            category_fields: None,
            is_system: 0,
            status: 1,
        };

        migrate(&pool, &module).await.unwrap();
        let columns = table_columns(&pool, table).await.unwrap();
        for name in ["id", "author", "pages", "gallery"] {
            assert!(columns.contains(name), "缺列 {name}");
        }

        // 再跑一次应幂等（不报「列已存在」）
        migrate(&pool, &module).await.unwrap();

        sqlx::query("DROP TABLE IF EXISTS `rustcms_test_module`")
            .execute(&pool)
            .await
            .unwrap();
    }
}

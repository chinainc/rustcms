//! 模块扩展字段的读写。
//!
//! 值存在模块自己的表（`modules.table_name`）里，一字段一列：
//! - `number` / `category` → int
//! - `checkbox` → 逗号拼接的字符串
//! - `images` / `texts` / `archive` → JSON 数组字符串
//! - `timeline` → JSON 字符串
//! - 其余 → 字符串
//!
//! 读写两侧的转换口径必须一致。

use std::collections::HashSet;

use serde_json::{Map, Value};
use sqlx::mysql::{MySql, MySqlPool};
use sqlx::{QueryBuilder, Row, Transaction};

use super::module::{self, CustomField, Module};
use super::ModelError;

/// 写入数据库的值。
#[derive(Debug, Clone)]
enum Stored {
    Int(i64),
    Text(String),
}

fn as_i64(value: &Value) -> i64 {
    match value {
        Value::Number(number) => number.as_i64().unwrap_or(0),
        Value::String(text) => text.trim().parse::<i64>().unwrap_or(0),
        Value::Bool(flag) => i64::from(*flag),
        _ => 0,
    }
}

fn as_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        other => other.to_string(),
    }
}

fn join_commas(value: &Value) -> String {
    match value {
        Value::Array(items) => items
            .iter()
            .map(as_string)
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>()
            .join(","),
        other => as_string(other),
    }
}

fn json_text(value: &Value) -> String {
    match value {
        Value::Array(_) => value.to_string(),
        other => as_string(other),
    }
}

/// 按字段类型把输入值转成落库形态。
fn stored_value(field: &CustomField, value: &Value) -> Stored {
    if field.is_numeric() {
        return Stored::Int(as_i64(value));
    }
    match field.kind.as_str() {
        module::TYPE_CHECKBOX => Stored::Text(join_commas(value)),
        module::TYPE_IMAGES | module::TYPE_TEXTS | module::TYPE_ARCHIVE => {
            Stored::Text(json_text(value))
        }
        module::TYPE_TIMELINE => Stored::Text(value.to_string()),
        _ => Stored::Text(as_string(value)),
    }
}

/// 必填校验：缺值 / 空串 / 空数组都算没填（文案遵循既有约定）。
pub fn check_required(module: &Module, input: &Map<String, Value>) -> Result<(), ModelError> {
    for field in module.field_defs() {
        if !field.required {
            continue;
        }
        let label = if field.name.trim().is_empty() {
            field.field_name.clone()
        } else {
            field.name.clone()
        };
        let missing = match input.get(&field.field_name) {
            None | Some(Value::Null) => true,
            Some(Value::String(text)) => text.trim().is_empty(),
            Some(Value::Array(items)) => items.is_empty(),
            _ => false,
        };
        if missing {
            return Err(ModelError::invalid(format!("{label}必填")));
        }
    }
    Ok(())
}

/// 写入前准备：补建缺失的列，并返回表里现有的列集合。
pub async fn prepare(pool: &MySqlPool, module: &Module) -> Result<HashSet<String>, ModelError> {
    module::migrate(pool, module).await?;
    let columns = module::table_columns(pool, module.safe_table()?).await?;
    Ok(columns)
}

/// 在事务里写扩展字段（整份覆盖：定义里的字段都会写，缺的写空/0）。
#[allow(clippy::explicit_auto_deref)]
pub async fn save_in_tx(
    tx: &mut Transaction<'_, MySql>,
    module: &Module,
    id: i64,
    input: &Map<String, Value>,
    columns: &HashSet<String>,
) -> Result<(), ModelError> {
    let table = module.safe_table()?;
    let pairs: Vec<(String, Stored)> = module
        .field_defs()
        .into_iter()
        .filter(|field| columns.contains(&field.field_name))
        .map(|field| {
            let empty = Value::Null;
            let raw = input.get(&field.field_name).unwrap_or(&empty);
            let stored = if matches!(raw, Value::Null) {
                // 未提交的字段：数值补 0，其它补空串（与既有接口契约一致）
                if field.is_numeric() {
                    Stored::Int(0)
                } else {
                    Stored::Text(String::new())
                }
            } else {
                stored_value(&field, raw)
            };
            (field.field_name, stored)
        })
        .collect();

    if pairs.is_empty() {
        return Ok(());
    }

    let mut builder: QueryBuilder<'_, MySql> = QueryBuilder::new("");
    builder.push(format!("INSERT INTO `{table}` (`id`"));
    for (name, _) in &pairs {
        builder.push(format!(", `{name}`"));
    }
    builder.push(") VALUES (").push_bind(id);
    for (_, value) in &pairs {
        builder.push(", ");
        match value {
            Stored::Int(number) => {
                builder.push_bind(*number);
            }
            Stored::Text(text) => {
                builder.push_bind(text.clone());
            }
        }
    }
    builder.push(") ON DUPLICATE KEY UPDATE ");
    for (index, (name, value)) in pairs.iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        builder.push(format!("`{name}` = "));
        match value {
            Stored::Int(number) => {
                builder.push_bind(*number);
            }
            Stored::Text(text) => {
                builder.push_bind(text.clone());
            }
        }
    }

    builder.build().execute(&mut **tx).await?;
    Ok(())
}

/// 读取扩展字段（表里没有的列直接跳过）。
pub async fn load(
    pool: &MySqlPool,
    module: &Module,
    id: i64,
) -> Result<Map<String, Value>, ModelError> {
    let fields = module.field_defs();
    if fields.is_empty() {
        return Ok(Map::new());
    }
    let table = module.safe_table()?;
    let columns = module::table_columns(pool, table).await?;
    let usable: Vec<CustomField> = fields
        .into_iter()
        .filter(|field| columns.contains(&field.field_name))
        .collect();
    if usable.is_empty() {
        return Ok(Map::new());
    }

    let mut builder: QueryBuilder<'_, MySql> = QueryBuilder::new("SELECT ");
    for (index, field) in usable.iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        // 统一按文本取出，再在 Rust 侧按字段类型解析
        builder.push(format!(
            "CAST(`{}` AS CHAR) AS `{}`",
            field.field_name, field.field_name
        ));
    }
    builder
        .push(format!(" FROM `{table}` WHERE `id` = "))
        .push_bind(id);

    let Some(row) = builder.build().fetch_optional(pool).await? else {
        return Ok(Map::new());
    };

    let mut result = Map::new();
    for field in usable {
        let raw = row
            .try_get::<Option<String>, _>(field.field_name.as_str())
            .map_err(|err| ModelError::Db(err.into()))?;
        let Some(raw) = raw else {
            continue;
        };
        result.insert(field.field_name.clone(), decode_value(&field, &raw));
    }
    Ok(result)
}

/// 按字段类型把库里的文本还原成 JSON 值（`GetArchiveExtra`）。
fn decode_value(field: &CustomField, raw: &str) -> Value {
    if field.is_numeric() {
        return Value::from(raw.trim().parse::<i64>().unwrap_or(0));
    }
    match field.kind.as_str() {
        module::TYPE_IMAGES | module::TYPE_TEXTS | module::TYPE_ARCHIVE | module::TYPE_TIMELINE => {
            serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
        }
        _ => Value::String(raw.to_string()),
    }
}

/// 供单测断言落库形态：把整行按文本取出来。
#[cfg(test)]
pub(crate) async fn read_raw(pool: &MySqlPool, table: &str, id: i64) -> Option<Map<String, Value>> {
    use sqlx::Column;

    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns \
         WHERE table_schema = DATABASE() AND table_name = ?",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .ok()?;
    if columns.is_empty() {
        return None;
    }

    // 数值列也要按文本读，统一 CAST 成 CHAR
    let select = columns
        .iter()
        .map(|name| format!("CAST(`{name}` AS CHAR) AS `{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("SELECT {select} FROM `{table}` WHERE `id` = ?");
    let row = sqlx::query(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
        .ok()??;

    let mut map = Map::new();
    for (index, column) in row.columns().iter().enumerate() {
        let raw = row
            .try_get::<Option<String>, _>(index)
            .ok()
            .flatten()
            .unwrap_or_default();
        map.insert(column.name().to_string(), Value::String(raw));
    }
    Some(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::db_lock;
    use crate::models::test_pool;

    fn field(field_name: &str, kind: &str, required: bool) -> CustomField {
        CustomField {
            name: field_name.to_string(),
            field_name: field_name.to_string(),
            kind: kind.to_string(),
            required,
            ..Default::default()
        }
    }

    /// 中文展示名 + ASCII 列名
    fn labeled(name: &str, field_name: &str, kind: &str, required: bool) -> CustomField {
        CustomField {
            name: name.to_string(),
            field_name: field_name.to_string(),
            kind: kind.to_string(),
            required,
            ..Default::default()
        }
    }

    fn module_with(fields: Vec<CustomField>, table: &str) -> Module {
        Module {
            id: 9_998,
            created_time: None,
            updated_time: None,
            table_name: table.to_string(),
            name: "测试模块".into(),
            url_token: "rustcms_extra".into(),
            title: String::new(),
            keywords: String::new(),
            description: String::new(),
            fields: Some(serde_json::to_string(&fields).unwrap()),
            category_fields: None,
            is_system: 0,
            status: 1,
        }
    }

    #[test]
    fn converts_values_by_type() {
        let text = field("author", module::TYPE_TEXT, false);
        assert!(
            matches!(stored_value(&text, &Value::from("张三")), Stored::Text(ref v) if v == "张三")
        );

        let number = field("pages", module::TYPE_NUMBER, false);
        assert!(matches!(
            stored_value(&number, &Value::from("12")),
            Stored::Int(12)
        ));
        assert!(matches!(
            stored_value(&number, &Value::from(7)),
            Stored::Int(7)
        ));

        let checkbox = field("tags", module::TYPE_CHECKBOX, false);
        let value = Value::Array(vec![Value::from("a"), Value::from("b")]);
        assert!(matches!(stored_value(&checkbox, &value), Stored::Text(ref v) if v == "a,b"));

        let images = field("gallery", module::TYPE_IMAGES, false);
        let value = Value::Array(vec![Value::from("/a.webp")]);
        assert!(
            matches!(stored_value(&images, &value), Stored::Text(ref v) if v == r#"["/a.webp"]"#)
        );
    }

    #[test]
    fn checks_required_fields() {
        let module = module_with(
            vec![
                labeled("作者", "author", module::TYPE_TEXT, true),
                labeled("页数", "pages", module::TYPE_NUMBER, false),
            ],
            "rustcms_test_extra",
        );

        let mut input = Map::new();
        let err = check_required(&module, &input).unwrap_err();
        assert_eq!(err.to_string(), "作者必填");

        input.insert("author".into(), Value::from("   "));
        assert_eq!(
            check_required(&module, &input).unwrap_err().to_string(),
            "作者必填"
        );

        input.insert("author".into(), Value::from("张三"));
        assert!(check_required(&module, &input).is_ok());
    }

    #[tokio::test]
    async fn roundtrips_extra_fields() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };

        let table = "rustcms_test_extra";
        sqlx::query("DROP TABLE IF EXISTS `rustcms_test_extra`")
            .execute(&pool)
            .await
            .ok();

        let module = module_with(
            vec![
                field("author", module::TYPE_TEXT, true),
                field("pages", module::TYPE_NUMBER, false),
                field("tags", module::TYPE_CHECKBOX, false),
                field("gallery", module::TYPE_IMAGES, false),
            ],
            table,
        );

        let columns = prepare(&pool, &module).await.unwrap();
        assert!(columns.contains("author"));

        let id = 900_001i64;
        let mut input = Map::new();
        input.insert("author".into(), Value::from("张三"));
        input.insert("pages".into(), Value::from(12));
        input.insert(
            "tags".into(),
            Value::Array(vec![Value::from("a"), Value::from("b")]),
        );
        input.insert(
            "gallery".into(),
            Value::Array(vec![Value::from("/a.webp"), Value::from("/b.webp")]),
        );

        let mut tx = pool.begin().await.unwrap();
        save_in_tx(&mut tx, &module, id, &input, &columns)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        // 落库形态：数字存 int、checkbox 逗号、images JSON
        let raw = read_raw(&pool, table, id).await.unwrap();
        assert_eq!(raw["author"], Value::String("张三".into()));
        assert_eq!(raw["pages"], Value::String("12".into()));
        assert_eq!(raw["tags"], Value::String("a,b".into()));
        assert_eq!(
            raw["gallery"],
            Value::String(r#"["/a.webp","/b.webp"]"#.into())
        );

        // 读回：类型还原
        let loaded = load(&pool, &module, id).await.unwrap();
        assert_eq!(loaded["author"], Value::from("张三"));
        assert_eq!(loaded["pages"], Value::from(12));
        assert_eq!(loaded["gallery"][0], Value::from("/a.webp"));

        // 覆盖写：未提交的字段被清空（与既有接口契约一致）
        let mut second = Map::new();
        second.insert("author".into(), Value::from("李四"));
        let mut tx = pool.begin().await.unwrap();
        save_in_tx(&mut tx, &module, id, &second, &columns)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let loaded = load(&pool, &module, id).await.unwrap();
        assert_eq!(loaded["author"], Value::from("李四"));
        assert_eq!(loaded["pages"], Value::from(0));
        assert_eq!(loaded["tags"], Value::from(""));

        // 不存在的内容：返回空对象
        let empty = load(&pool, &module, 900_002).await.unwrap();
        assert!(empty.is_empty());

        sqlx::query("DROP TABLE IF EXISTS `rustcms_test_extra`")
            .execute(&pool)
            .await
            .ok();
    }
}

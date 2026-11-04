//! 留言（`guestbooks` 表，`pluginGuestbook.go`）。
//!
//! `status`：0 = 待审核，1 = 正常，2 = 垃圾。
//! 配置存 `settings.guestbook`，其中 `fields` 是自定义字段（含系统的
//! `user_name` / `contact` / `content`），导出时需要按字段顺序取值。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::site_setting;

/// 留言配置键。
pub const SETTING_KEY: &str = "guestbook";

/// 留言。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Guestbook {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_name: String,
    pub contact: String,
    pub content: Option<String>,
    pub ip: String,
    pub status: i8,
    pub refer: String,
    pub site_id: u32,
    pub extra_data: Option<String>,
    pub user_id: u32,
}

/// 列表筛选条件。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GuestbookFilter {
    #[serde(default)]
    pub keyword: String,
    /// `None` = 全部
    #[serde(default)]
    pub status: Option<i8>,
}

/// 读配置（补默认值）。
pub async fn setting_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, SETTING_KEY).await?;
    if let Value::Object(map) = &mut value {
        map.entry("return_message").or_insert(json!(""));
        map.entry("push_way").or_insert(json!(0));
        map.entry("site_id").or_insert(json!(0));
        map.entry("api_method").or_insert(json!(""));
        map.entry("api_url").or_insert(json!(""));
        map.entry("header_key").or_insert(json!(""));
        map.entry("header_value").or_insert(json!(""));
        if !map.get("fields").is_some_and(Value::is_array) {
            map.insert("fields".into(), json!([]));
        }
    }
    Ok(value)
}

fn field_name(field: &Value) -> String {
    field
        .get("field_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

/// 保存配置：`fields` 按 `field_name` 去重（先出现的保留）。
pub async fn setting_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    if !patch.is_object() {
        return Err(anyhow::anyhow!("配置必须是 JSON 对象"));
    }
    let mut patch = patch.clone();
    if let Value::Object(map) = &mut patch {
        if let Some(fields) = map.get("fields").and_then(Value::as_array) {
            let mut seen = std::collections::HashSet::new();
            let mut kept = Vec::new();
            for field in fields {
                let name = field_name(field);
                if name.is_empty() || seen.insert(name) {
                    kept.push(field.clone());
                }
            }
            map.insert("fields".into(), Value::Array(kept));
        }
    }
    site_setting::save_section(pool, SETTING_KEY, &patch).await?;
    setting_get(pool).await
}

/// 分页查留言，返回 `(总数, 本页记录)`（id 倒序）。
pub async fn list(
    pool: &MySqlPool,
    filter: &GuestbookFilter,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Guestbook>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;
    let keyword = filter.keyword.trim();

    let mut where_sql = String::from(" WHERE 1 = 1");
    if !keyword.is_empty() {
        where_sql.push_str(" AND (user_name LIKE ? OR contact LIKE ? OR content LIKE ?)");
    }
    if filter.status.is_some() {
        where_sql.push_str(" AND status = ?");
    }

    let like = format!("%{keyword}%");
    let count_sql = format!("SELECT COUNT(*) FROM guestbooks{where_sql}");
    let mut counter = sqlx::query_scalar::<_, i64>(&count_sql);
    if !keyword.is_empty() {
        counter = counter.bind(&like).bind(&like).bind(&like);
    }
    if let Some(status) = filter.status {
        counter = counter.bind(status);
    }
    let total = counter.fetch_one(pool).await?;

    let list_sql = format!("SELECT * FROM guestbooks{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?");
    let mut query = sqlx::query_as::<_, Guestbook>(&list_sql);
    if !keyword.is_empty() {
        query = query.bind(&like).bind(&like).bind(&like);
    }
    if let Some(status) = filter.status {
        query = query.bind(status);
    }
    let rows = query.bind(page_size).bind(offset).fetch_all(pool).await?;
    Ok((total, rows))
}

/// 按 ID 取留言。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Guestbook>> {
    Ok(
        sqlx::query_as::<_, Guestbook>("SELECT * FROM guestbooks WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 批量设置状态，返回受影响行数。
pub async fn set_status(pool: &MySqlPool, ids: &[u32], status: i8) -> anyhow::Result<u64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let mut builder = sqlx::QueryBuilder::<sqlx::MySql>::new("UPDATE guestbooks SET status = ");
    builder.push_bind(status);
    builder.push(", updated_time = ");
    builder.push_bind(chrono::Utc::now().timestamp());
    builder.push(" WHERE id IN (");
    let mut list = builder.separated(", ");
    for id in ids {
        list.push_bind(*id);
    }
    list.push_unseparated(")");
    Ok(builder.build().execute(pool).await?.rows_affected())
}

/// 删除单条。
pub async fn delete(pool: &MySqlPool, id: u32) -> anyhow::Result<bool> {
    Ok(sqlx::query("DELETE FROM guestbooks WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// 批量删除，返回删除条数。
pub async fn delete_many(pool: &MySqlPool, ids: &[u32]) -> anyhow::Result<u64> {
    let mut deleted = 0u64;
    for id in ids {
        deleted += sqlx::query("DELETE FROM guestbooks WHERE id = ?")
            .bind(*id)
            .execute(pool)
            .await?
            .rows_affected();
    }
    Ok(deleted)
}

/// 全部留言（导出用）。
pub async fn all(pool: &MySqlPool) -> anyhow::Result<Vec<Guestbook>> {
    Ok(
        sqlx::query_as::<_, Guestbook>("SELECT * FROM guestbooks ORDER BY id ASC")
            .fetch_all(pool)
            .await?,
    )
}

/// 导出表头 + 数据行（按配置里的字段顺序拼）。
pub async fn export(pool: &MySqlPool) -> anyhow::Result<(Vec<String>, Vec<Vec<Value>>)> {
    let config = setting_get(pool).await?;
    let fields = config
        .get("fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rows = all(pool).await?;

    let mut header: Vec<String> = fields
        .iter()
        .map(|field| {
            field
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .map(|name| name.to_string())
                .unwrap_or_else(|| field_name(field))
        })
        .collect();
    header.extend(
        ["Created Time", "IP", "Refer", "Site Id", "Spam"]
            .iter()
            .map(|item| item.to_string()),
    );

    let mut content = Vec::new();
    for row in &rows {
        let extra: serde_json::Map<String, Value> = row
            .extra_data
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .and_then(|value| match value {
                Value::Object(map) => Some(map),
                _ => None,
            })
            .unwrap_or_default();

        let mut item: Vec<Value> = Vec::new();
        for field in &fields {
            let is_system = field
                .get("is_system")
                .and_then(Value::as_bool)
                .or_else(|| field.get("is_system").and_then(Value::as_i64).map(|v| v != 0))
                .unwrap_or(false);
            if is_system {
                let name = field_name(field);
                let value = match name.as_str() {
                    "user_name" => row.user_name.clone(),
                    "contact" => row.contact.clone(),
                    "content" => row.content.clone().unwrap_or_default(),
                    _ => String::new(),
                };
                item.push(Value::String(value));
            } else {
                // Go 先按 `name` 取，取不到再忽略
                let name = field
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                item.push(extra.get(name).cloned().unwrap_or(Value::Null));
            }
        }

        let created = row
            .created_time
            .map(|stamp| {
                use chrono::{Local, TimeZone};
                Local
                    .timestamp_opt(stamp, 0)
                    .single()
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        item.push(Value::String(created));
        item.push(Value::String(row.ip.clone()));
        item.push(Value::String(row.refer.clone()));
        item.push(json!(row.site_id));
        item.push(json!(row.status == 2));
        content.push(item);
    }

    Ok((header, content))
}

/// 校验状态取值（0 待审 / 1 正常 / 2 垃圾）。
pub fn valid_status(status: i8) -> bool {
    (0..=2).contains(&status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, setting, test_pool};

    #[tokio::test]
    async fn guestbook_crud_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM guestbooks WHERE user_name = 'rustcms-gb'")
            .execute(&pool)
            .await;

        let now = chrono::Utc::now().timestamp();
        let id = sqlx::query(
            "INSERT INTO guestbooks (created_time, updated_time, user_name, contact, content, ip, \
                status, refer, site_id, extra_data, user_id) \
             VALUES (?, ?, 'rustcms-gb', 'hi@example.com', '留言内容', '127.0.0.1', 0, '/contact', 0, \
                '{\"wechat\":\"rustcms-wx\"}', 0)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_id() as u32;

        // 关键词 + 状态筛选
        let filter = GuestbookFilter {
            keyword: "rustcms-gb".into(),
            status: Some(0),
        };
        let (total, rows) = list(&pool, &filter, 1, 20).await.unwrap();
        assert_eq!(total, 1, "应只命中刚插入的一条");
        assert_eq!(rows[0].id, id);

        let filter = GuestbookFilter {
            keyword: String::new(),
            status: Some(1),
        };
        let (total, _) = list(&pool, &filter, 1, 20).await.unwrap();
        assert_eq!(total, 0);

        // 状态
        assert_eq!(set_status(&pool, &[id], 2).await.unwrap(), 1);
        assert_eq!(get(&pool, id).await.unwrap().unwrap().status, 2);
        assert!(valid_status(2));
        assert!(!valid_status(3));

        // 导出：系统字段 + 自定义字段
        let (header, content) = export(&pool).await.unwrap();
        let _ = (&header, &content);

        assert!(delete(&pool, id).await.unwrap());
        assert!(get(&pool, id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn setting_dedupes_fields() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, SETTING_KEY).await.unwrap();

        let saved = setting_save(
            &pool,
            &json!({
                "return_message": "ok",
                "fields": [
                    {"name": "姓名", "field_name": "user_name", "is_system": true},
                    {"name": "微信", "field_name": "wechat", "is_system": false},
                    {"name": "重复", "field_name": "wechat", "is_system": false},
                    {"name": "无名", "field_name": "", "is_system": false}
                ]
            }),
        )
        .await
        .unwrap();
        assert_eq!(saved["return_message"], json!("ok"));
        let fields = saved["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 3, "同名与空名应被去掉：{saved}");

        // 导出表头应含字段名 + 5 个固定列
        let (header, _) = export(&pool).await.unwrap();
        assert_eq!(header.len(), fields.len() + 5);
        assert_eq!(header[0], "姓名");
        assert_eq!(header[header.len() - 1], "Spam");

        match backup {
            Some(raw) => setting::save_raw(&pool, SETTING_KEY, &raw).await.unwrap(),
            None => setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
    }
}

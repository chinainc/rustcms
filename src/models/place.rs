//! 城市分站（`places` 表，`pluginPlace.go`）。
//!
//! 注意：`latitude` / `longitude` 是 `decimal(10,6)`，sqlx 不能直接解码成 `f64`，
//! 这里统一 `CAST(... AS CHAR)` 再按字符串返回。

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::{site_setting, ModelError};

/// 城市站配置键。
pub const SETTING_KEY: &str = "place";

const PLACE_COLUMNS: &str = "id, created_time, updated_time, title, seo_title, keywords, \
    url_token, description, content, parent_id, sort, template, is_inherit, images, logo, extra, \
    CAST(latitude AS CHAR) AS latitude, CAST(longitude AS CHAR) AS longitude, timezone, status";

/// 城市站。
#[derive(Debug, Clone, Default, FromRow, Serialize)]
pub struct Place {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub title: String,
    pub seo_title: String,
    pub keywords: String,
    pub url_token: String,
    pub description: String,
    pub content: Option<String>,
    pub parent_id: u32,
    pub sort: u32,
    pub template: String,
    pub is_inherit: u8,
    pub images: Option<String>,
    pub logo: String,
    pub extra: Option<String>,
    pub latitude: Option<String>,
    pub longitude: Option<String>,
    pub timezone: String,
    pub status: u8,
}

/// 城市站入参。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlaceInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub seo_title: String,
    #[serde(default)]
    pub keywords: String,
    #[serde(default)]
    pub url_token: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub parent_id: u32,
    #[serde(default)]
    pub sort: Option<u32>,
    #[serde(default)]
    pub template: String,
    #[serde(default)]
    pub is_inherit: Option<u8>,
    #[serde(default)]
    pub images: Vec<String>,
    #[serde(default)]
    pub logo: String,
    #[serde(default)]
    pub extra: Option<Value>,
    #[serde(default)]
    pub latitude: Option<f64>,
    #[serde(default)]
    pub longitude: Option<f64>,
    #[serde(default)]
    pub timezone: String,
    #[serde(default)]
    pub status: Option<u8>,
}

fn sanitize_token(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// 读城市站配置（补默认值）。
pub async fn setting_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, SETTING_KEY).await?;
    if let Value::Object(map) = &mut value {
        map.entry("open").or_insert(json!(false));
        map.entry("url_type").or_insert(json!("directory"));
        map.entry("content_type").or_insert(json!("default"));
        if !map.get("fields").is_some_and(Value::is_array) {
            map.insert("fields".into(), json!([]));
        }
        // url_type 只认 subdomain / directory
        let url_type = map
            .get("url_type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if url_type != "subdomain" && url_type != "directory" {
            map.insert("url_type".into(), json!("directory"));
        }
        // content_type 只认 default / full
        let content_type = map
            .get("content_type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if content_type != "default" && content_type != "full" {
            map.insert("content_type".into(), json!("default"));
        }
    }
    Ok(value)
}

/// 保存城市站配置（字段级 merge + `fields` 按 `field_name` 去重）。
pub async fn setting_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    if !patch.is_object() {
        return Err(anyhow::anyhow!("配置必须是 JSON 对象"));
    }
    let mut patch = patch.clone();
    if let Value::Object(map) = &mut patch {
        if let Some(fields) = map.get("fields").and_then(Value::as_array) {
            let mut seen = HashSet::new();
            let mut kept = Vec::new();
            for field in fields {
                let name = field
                    .get("field_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
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

/// 查询城市站。
///
/// 返回 `(是否树形, 记录)`：标题搜索与 `show_type = 2` 都按平铺列表返回。
pub async fn list(
    pool: &MySqlPool,
    parent_id: u32,
    title: &str,
    show_type: i32,
) -> anyhow::Result<(bool, Vec<Place>)> {
    let title = title.trim();
    let mut where_sql = String::from(" WHERE 1 = 1");
    if !title.is_empty() {
        where_sql.push_str(" AND `title` LIKE ?");
    }
    let sql = format!("SELECT {PLACE_COLUMNS} FROM places{where_sql} ORDER BY sort ASC, id ASC");

    let mut query = sqlx::query_as::<_, Place>(&sql);
    if !title.is_empty() {
        query = query.bind(format!("%{title}%"));
    }
    let mut rows = query.fetch_all(pool).await?;

    let flat = !title.is_empty() || show_type == 2;
    if flat {
        // 平铺模式下按 parent_id 过滤
        if parent_id > 0 {
            rows.retain(|item| item.parent_id == parent_id);
        }
        return Ok((false, rows));
    }

    // 树形：只保留 parent_id 匹配的根，其余挂到父节点下
    if parent_id > 0 {
        rows.retain(|item| item.parent_id == parent_id || item.id == parent_id);
    }
    Ok((true, rows))
}

/// 按 ID 取城市站。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Place>> {
    let sql = format!("SELECT {PLACE_COLUMNS} FROM places WHERE id = ?");
    Ok(sqlx::query_as::<_, Place>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// 按别名取城市站。
pub async fn get_by_token(pool: &MySqlPool, token: &str) -> anyhow::Result<Option<Place>> {
    let sql = format!("SELECT {PLACE_COLUMNS} FROM places WHERE url_token = ?");
    Ok(sqlx::query_as::<_, Place>(&sql)
        .bind(token)
        .fetch_optional(pool)
        .await?)
}

async fn token_taken(pool: &MySqlPool, token: &str, exclude: u32) -> anyhow::Result<bool> {
    let found: Option<u32> = sqlx::query_scalar("SELECT id FROM places WHERE url_token = ? LIMIT 1")
        .bind(token)
        .fetch_optional(pool)
        .await?;
    Ok(matches!(found, Some(id) if id != exclude))
}

/// 收集某节点的所有后代 id（用于阻止把父级挂到自己的子节点下）。
async fn descendant_ids(pool: &MySqlPool, root: u32) -> anyhow::Result<HashSet<u32>> {
    let rows: Vec<(u32, u32)> = sqlx::query_as("SELECT id, parent_id FROM places")
        .fetch_all(pool)
        .await?;
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (id, parent) in rows {
        children.entry(parent).or_default().push(id);
    }

    let mut found = HashSet::new();
    let mut stack = vec![root];
    while let Some(current) = stack.pop() {
        if let Some(list) = children.get(&current) {
            for id in list {
                if found.insert(*id) {
                    stack.push(*id);
                }
            }
        }
    }
    Ok(found)
}

/// 新建 / 更新城市站。
pub async fn save(pool: &MySqlPool, input: &PlaceInput) -> Result<Place, ModelError> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err(ModelError::invalid("请填写城市站名称"));
    }

    // 父级校验
    if input.parent_id > 0 {
        if get(pool, input.parent_id)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .is_none()
        {
            return Err(ModelError::invalid("上级城市站不存在"));
        }
        if input.parent_id == input.id {
            return Err(ModelError::invalid("上级城市站不能是自己"));
        }
        if input.id > 0 {
            let descendants = descendant_ids(pool, input.id)
                .await
                .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;
            if descendants.contains(&input.parent_id) {
                return Err(ModelError::invalid("上级城市站不能是自己的下级"));
            }
        }
    }

    // 别名：空则用标题推导，撞了自动加后缀
    let base_token = {
        let provided = input.url_token.trim();
        if provided.is_empty() {
            let derived = sanitize_token(title);
            if derived.is_empty() {
                format!("place-{}", chrono::Utc::now().timestamp_millis())
            } else {
                derived
            }
        } else {
            sanitize_token(provided)
        }
    };
    let mut token = base_token.clone();
    let mut suffix = 1;
    while token_taken(pool, &token, input.id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
    {
        token = format!("{base_token}-{suffix}");
        suffix += 1;
        if suffix > 999 {
            return Err(ModelError::invalid("别名冲突过多，请手动指定"));
        }
    }

    let now = chrono::Utc::now().timestamp();
    let extra = input
        .extra
        .as_ref()
        .map(|value| value.to_string())
        .unwrap_or_else(|| "{}".to_string());
    let images = serde_json::to_string(&input.images).unwrap_or_else(|_| "[]".to_string());
    let status = input.status.unwrap_or(1).min(1);
    let sort = input.sort.unwrap_or(99);

    if input.id > 0 {
        if get(pool, input.id)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .is_none()
        {
            return Err(ModelError::invalid("城市站不存在"));
        }
        sqlx::query(
            "UPDATE places SET updated_time = ?, title = ?, seo_title = ?, keywords = ?, \
                url_token = ?, description = ?, content = ?, parent_id = ?, sort = ?, template = ?, \
                is_inherit = ?, images = ?, logo = ?, extra = ?, latitude = ?, longitude = ?, \
                timezone = ?, status = ? WHERE id = ?",
        )
        .bind(now)
        .bind(title)
        .bind(input.seo_title.trim())
        .bind(input.keywords.trim())
        .bind(&token)
        .bind(input.description.trim())
        .bind(&input.content)
        .bind(input.parent_id)
        .bind(sort)
        .bind(input.template.trim())
        .bind(input.is_inherit.unwrap_or(0).min(1))
        .bind(&images)
        .bind(input.logo.trim())
        .bind(&extra)
        .bind(input.latitude.unwrap_or(0.0))
        .bind(input.longitude.unwrap_or(0.0))
        .bind(input.timezone.trim())
        .bind(status)
        .bind(input.id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;

        return get(pool, input.id)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .ok_or_else(|| ModelError::invalid("城市站不存在"));
    }

    let id = sqlx::query(
        "INSERT INTO places (created_time, updated_time, title, seo_title, keywords, url_token, \
            description, content, parent_id, sort, template, is_inherit, images, logo, extra, \
            latitude, longitude, timezone, status) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(now)
    .bind(now)
    .bind(title)
    .bind(input.seo_title.trim())
    .bind(input.keywords.trim())
    .bind(&token)
    .bind(input.description.trim())
    .bind(&input.content)
    .bind(input.parent_id)
    .bind(sort)
    .bind(input.template.trim())
    .bind(input.is_inherit.unwrap_or(0).min(1))
    .bind(&images)
    .bind(input.logo.trim())
    .bind(&extra)
    .bind(input.latitude.unwrap_or(0.0))
    .bind(input.longitude.unwrap_or(0.0))
    .bind(input.timezone.trim())
    .bind(status)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?
    .last_insert_id() as u32;

    get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("城市站创建失败"))
}

/// 删除城市站及其所有下级，返回删除条数。
pub async fn delete(pool: &MySqlPool, id: u32) -> Result<u64, ModelError> {
    if get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .is_none()
    {
        return Ok(0);
    }
    let mut ids = descendant_ids(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;
    ids.insert(id);

    let mut deleted = 0u64;
    for target in ids {
        deleted += sqlx::query("DELETE FROM places WHERE id = ?")
            .bind(target)
            .execute(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?
            .rows_affected();
    }
    Ok(deleted)
}

/// 把平铺记录组装成树。
pub fn build_tree(rows: &[Place]) -> Vec<Value> {
    let mut children: HashMap<u32, Vec<&Place>> = HashMap::new();
    let mut known: HashSet<u32> = HashSet::new();
    for row in rows {
        known.insert(row.id);
    }
    for row in rows {
        children.entry(row.parent_id).or_default().push(row);
    }

    fn node(item: &Place, children: &HashMap<u32, Vec<&Place>>) -> Value {
        let mut value = serde_json::to_value(item).unwrap_or(Value::Null);
        let kids = children
            .get(&item.id)
            .map(|list| list.iter().map(|child| node(child, children)).collect())
            .unwrap_or_else(Vec::new);
        if let Value::Object(map) = &mut value {
            map.insert("children".into(), Value::Array(kids));
        }
        value
    }

    // 根节点：父级不在本次结果集里
    rows.iter()
        .filter(|row| row.parent_id == 0 || !known.contains(&row.parent_id))
        .map(|row| node(row, &children))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, setting, test_pool};

    #[test]
    fn sanitizes_and_builds_tree() {
        assert_eq!(sanitize_token("Bei Jing"), "bei-jing");
        assert_eq!(sanitize_token("中文"), "");

        let rows = vec![
            Place {
                id: 1,
                title: "省".into(),
                url_token: "p1".into(),
                sort: 1,
                status: 1,
                ..Default::default()
            },
            Place {
                id: 2,
                parent_id: 1,
                title: "市".into(),
                url_token: "p2".into(),
                ..Default::default()
            },
        ];
        let tree = build_tree(&rows);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0]["id"], 1);
        assert_eq!(tree[0]["children"].as_array().unwrap()[0]["id"], 2);
    }

    #[tokio::test]
    async fn place_crud_and_delete_subtree() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM places WHERE title LIKE 'rustcms-place%'")
            .execute(&pool)
            .await;

        // 名称必填
        assert_eq!(
            save(&pool, &PlaceInput::default())
                .await
                .unwrap_err()
                .to_string(),
            "请填写城市站名称"
        );

        let root = save(
            &pool,
            &PlaceInput {
                title: "rustcms-place-root".into(),
                status: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(root.url_token, "rustcms-place-root");
        assert_eq!(root.sort, 99);

        let child = save(
            &pool,
            &PlaceInput {
                title: "rustcms-place-child".into(),
                parent_id: root.id,
                sort: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(child.parent_id, root.id);

        // 上级不能是自己 / 不能是自己的下级
        assert!(save(
            &pool,
            &PlaceInput {
                id: root.id,
                title: "rustcms-place-root".into(),
                parent_id: root.id,
                ..Default::default()
            }
        )
        .await
        .is_err());
        assert!(save(
            &pool,
            &PlaceInput {
                id: root.id,
                title: "rustcms-place-root".into(),
                parent_id: child.id,
                ..Default::default()
            }
        )
        .await
        .is_err());

        // 别名撞车自动加后缀
        let dup = save(
            &pool,
            &PlaceInput {
                title: "rustcms-place-root-2".into(),
                url_token: "rustcms-place-root".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(dup.url_token, "rustcms-place-root-1");

        // 树形列表
        let (tree, rows) = list(&pool, 0, "rustcms-place", 0).await.unwrap();
        assert!(!tree);
        assert_eq!(rows.len(), 3);

        let (tree, rows) = list(&pool, 0, "", 0).await.unwrap();
        assert!(tree);
        let _ = rows;

        // 删除父级连带子级
        let deleted = delete(&pool, root.id).await.unwrap();
        assert!(deleted >= 2, "应删除父级与子级，实际 {deleted}");
        assert!(get(&pool, child.id).await.unwrap().is_none());

        let _ = sqlx::query("DELETE FROM places WHERE title LIKE 'rustcms-place%'")
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    async fn setting_normalizes_url_type() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, SETTING_KEY).await.unwrap();

        let loaded = setting_get(&pool).await.unwrap();
        assert_eq!(loaded["url_type"], "directory");
        assert_eq!(loaded["content_type"], "default");
        assert!(loaded["fields"].is_array());

        let saved = setting_save(
            &pool,
            &json!({"open": true, "url_type": "weird", "content_type": "x"}),
        )
        .await
        .unwrap();
        assert_eq!(saved["open"], true);
        assert_eq!(saved["url_type"], "directory");
        assert_eq!(saved["content_type"], "default");

        let saved = setting_save(&pool, &json!({"url_type": "subdomain"})).await.unwrap();
        assert_eq!(saved["url_type"], "subdomain");

        // 字段去重
        let saved = setting_save(
            &pool,
            &json!({"fields": [
                {"name": "电话", "field_name": "phone"},
                {"name": "电话2", "field_name": "phone"}
            ]}),
        )
        .await
        .unwrap();
        assert_eq!(saved["fields"].as_array().unwrap().len(), 1);

        match backup {
            Some(raw) => setting::save_raw(&pool, SETTING_KEY, &raw).await.unwrap(),
            None => setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
    }
}

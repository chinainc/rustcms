//! 站点设置（`settings` 表里的各个配置段）。
//!
//! 对齐 Go 版 `controller/manageController/setting.go` + `provider/setting.go`：
//! - `system` / `content` / `index` / `contact` / `safe` / `ai_setting` 存在
//!   `settings` 表，值是 JSON 对象，保存走「字段级 merge」；
//! - `diy_fields` 是 JSON **数组**，保存整份覆盖；
//! - `banner` 形如 `{"banner":[{"type":"default","list":[{...}]}]}`；
//! - `cache_type` 是**纯字符串**（`file` / `memory`），不是 JSON。

use serde_json::{json, Value};
use sqlx::MySqlPool;

use super::{setting, ModelError};

pub const SYSTEM: &str = "system";
pub const CONTENT: &str = "content";
pub const INDEX: &str = "index";
pub const CONTACT: &str = "contact";
pub const SAFE: &str = "safe";
pub const BANNER: &str = "banner";
pub const DIY_FIELDS: &str = "diy_fields";
pub const CACHE_TYPE: &str = "cache_type";
pub const AI: &str = "ai_setting";

/// 读一个对象型配置段；不存在或不是对象时返回 `{}`。
pub async fn get_section(pool: &MySqlPool, key: &str) -> anyhow::Result<Value> {
    Ok(setting::get_json(pool, key)
        .await?
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({})))
}

/// 读一个数组型配置段；不存在或不是数组时返回 `[]`。
pub async fn get_array(pool: &MySqlPool, key: &str) -> anyhow::Result<Value> {
    Ok(setting::get_json(pool, key)
        .await?
        .filter(Value::is_array)
        .unwrap_or_else(|| json!([])))
}

/// 字段级合并保存（只覆盖 `patch` 里出现的顶层字段）。
pub async fn save_section(
    pool: &MySqlPool,
    key: &str,
    patch: &Value,
) -> anyhow::Result<Value> {
    if !patch.is_object() {
        return Err(anyhow::anyhow!("配置必须是 JSON 对象"));
    }
    setting::merge_json(pool, key, patch).await
}

/// 整份覆盖（数组 / 标量配置用）。
pub async fn save_value(pool: &MySqlPool, key: &str, value: &Value) -> anyhow::Result<()> {
    setting::save_json(pool, key, value).await
}

/// 读缓存类型（纯字符串，非 JSON）。
pub async fn get_cache_type(pool: &MySqlPool) -> anyhow::Result<String> {
    Ok(setting::get_raw(pool, CACHE_TYPE)
        .await?
        .unwrap_or_default()
        .trim()
        .trim_matches('"')
        .to_string())
}

/// 写缓存类型（纯字符串）。
pub async fn save_cache_type(pool: &MySqlPool, value: &str) -> anyhow::Result<()> {
    setting::save_raw(pool, CACHE_TYPE, value.trim()).await
}

/// Banner 列表（`{"banner":[...]}` 里的 `banner` 数组）。
pub async fn banner_list(pool: &MySqlPool) -> anyhow::Result<Value> {
    let config = get_section(pool, BANNER).await?;
    Ok(config
        .get("banner")
        .filter(|value| value.is_array())
        .cloned()
        .unwrap_or_else(|| json!([])))
}

fn group_list<'a>(config: &'a Value, kind: &str) -> Vec<&'a Value> {
    config
        .get("banner")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .filter(|group| group.get("type").and_then(Value::as_str) == Some(kind))
                .collect()
        })
        .unwrap_or_default()
}

fn max_item_id(group: &Value) -> i64 {
    group
        .get("list")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| item.get("id").and_then(Value::as_i64))
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

/// 新增 / 更新一条 Banner（`id = 0` 表示新增）。
///
/// 返回保存后的条目。
pub async fn banner_save(
    pool: &MySqlPool,
    item: &Value,
    type_hint: &str,
) -> Result<Value, ModelError> {
    if !item.is_object() {
        return Err(ModelError::invalid("请选择图片"));
    }
    let logo = item.get("logo").and_then(Value::as_str).unwrap_or("").trim();
    if logo.is_empty() {
        return Err(ModelError::invalid("请选择图片"));
    }

    let mut saved_item = item.clone();
    let kind = item
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(type_hint)
        .to_string();
    let kind = if kind.is_empty() { "default".to_string() } else { kind };

    if let Value::Object(map) = &mut saved_item {
        map.insert("type".into(), Value::String(kind.clone()));
    }
    let item_id = item.get("id").and_then(Value::as_i64).unwrap_or(0);

    let mut config = get_section(pool, BANNER).await?;
    if !config.get("banner").is_some_and(Value::is_array) {
        config = json!({ "banner": [] });
    }

    let groups = config["banner"].as_array_mut().expect("刚保证是数组");
    let index = groups
        .iter()
        .position(|group| group.get("type").and_then(Value::as_str) == Some(kind.as_str()));

    match index {
        Some(group_index) => {
            if !groups[group_index].get("list").is_some_and(Value::is_array) {
                groups[group_index]["list"] = json!([]);
            }
            let list = groups[group_index]["list"].as_array_mut().expect("刚保证是数组");
            if item_id == 0 {
                let next_id = list
                    .iter()
                    .filter_map(|entry| entry.get("id").and_then(Value::as_i64))
                    .max()
                    .unwrap_or(0)
                    + 1;
                if let Value::Object(map) = &mut saved_item {
                    map.insert("id".into(), json!(next_id));
                }
                list.push(saved_item.clone());
            } else {
                let target = list
                    .iter()
                    .position(|entry| entry.get("id").and_then(Value::as_i64) == Some(item_id));
                match target {
                    Some(position) => list[position] = saved_item.clone(),
                    None => {
                        // 传了 id 但不存在：按新增处理
                        if let Value::Object(map) = &mut saved_item {
                            map.insert("id".into(), json!(item_id));
                        }
                        list.push(saved_item.clone());
                    }
                }
            }
        }
        None => {
            let next_id = if item_id == 0 { 1 } else { item_id };
            if let Value::Object(map) = &mut saved_item {
                map.insert("id".into(), json!(next_id));
            }
            groups.push(json!({
                "type": kind,
                "list": [saved_item.clone()],
            }));
        }
    }

    save_value(pool, BANNER, &config).await?;
    Ok(saved_item)
}

/// 删除一条 Banner，返回是否删掉。
pub async fn banner_delete(pool: &MySqlPool, id: i64, kind: &str) -> Result<bool, ModelError> {
    if id == 0 {
        return Err(ModelError::invalid("Banner 不存在"));
    }

    let mut config = get_section(pool, BANNER).await?;
    let Some(groups) = config.get_mut("banner").and_then(Value::as_array_mut) else {
        return Ok(false);
    };

    let mut removed = false;
    let mut empty_group: Option<usize> = None;
    let mut group_type = String::new();

    for (group_index, group) in groups.iter_mut().enumerate() {
        let this_type = group
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !kind.is_empty() && this_type != kind {
            continue;
        }
        let Some(list) = group.get_mut("list").and_then(Value::as_array_mut) else {
            continue;
        };
        if let Some(position) = list
            .iter()
            .position(|entry| entry.get("id").and_then(Value::as_i64) == Some(id))
        {
            list.remove(position);
            removed = true;
            if list.is_empty() {
                empty_group = Some(group_index);
                group_type = this_type;
            }
            break;
        }
    }

    if let Some(index) = empty_group {
        if group_type != "default" {
            groups.remove(index);
        }
    }

    if !removed {
        return Ok(false);
    }
    save_value(pool, BANNER, &config).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn banner_helpers_handle_missing_fields() {
        let empty = json!({});
        assert!(group_list(&empty, "default").is_empty());
        assert_eq!(max_item_id(&empty), 0);
        assert_eq!(
            max_item_id(&json!({"list":[{"id":2},{"id":7}]})),
            7
        );
    }

    #[tokio::test]
    async fn section_roundtrip_and_restore() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let key = "__rustcms_test_section__";
        let backup = setting::get_raw(&pool, key).await.unwrap();

        let saved = save_section(&pool, key, &json!({"site_name": "A", "language": "zh"}))
            .await
            .unwrap();
        assert_eq!(saved["site_name"], json!("A"));

        // 字段级 merge：只改 site_name
        let merged = save_section(&pool, key, &json!({"site_name": "B"}))
            .await
            .unwrap();
        assert_eq!(merged["site_name"], json!("B"));
        assert_eq!(merged["language"], json!("zh"));

        // 非对象被拒
        assert!(save_section(&pool, key, &json!([])).await.is_err());

        match backup {
            Some(raw) => setting::save_raw(&pool, key, &raw).await.unwrap(),
            None => setting::remove(&pool, key).await.unwrap(),
        }
    }

    #[tokio::test]
    async fn banner_crud_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, BANNER).await.unwrap();

        // 缺少 logo 被拒
        assert!(banner_save(&pool, &json!({"type": "rustcms-test"}), "")
            .await
            .is_err());

        let saved = banner_save(
            &pool,
            &json!({"logo": "/uploads/a.webp", "link": "/x", "type": "rustcms-test"}),
            "",
        )
        .await
        .unwrap();
        assert_eq!(saved["id"], json!(1));

        let saved2 = banner_save(
            &pool,
            &json!({"logo": "/uploads/b.webp", "type": "rustcms-test"}),
            "",
        )
        .await
        .unwrap();
        assert_eq!(saved2["id"], json!(2));

        // 更新第一条
        let updated = banner_save(
            &pool,
            &json!({"id": 1, "logo": "/uploads/c.webp", "type": "rustcms-test"}),
            "",
        )
        .await
        .unwrap();
        assert_eq!(updated["logo"], json!("/uploads/c.webp"));

        let list = banner_list(&pool).await.unwrap();
        assert!(list.as_array().unwrap().iter().any(|group| {
            group["type"] == json!("rustcms-test") && group["list"].as_array().unwrap().len() == 2
        }));

        // 删除一条
        assert!(banner_delete(&pool, 2, "rustcms-test").await.unwrap());
        // 再删一条后组被清空并移除（非 default 组）
        assert!(banner_delete(&pool, 1, "rustcms-test").await.unwrap());
        let list = banner_list(&pool).await.unwrap();
        assert!(!list
            .as_array()
            .unwrap()
            .iter()
            .any(|group| group["type"] == json!("rustcms-test")));

        match backup {
            Some(raw) => setting::save_raw(&pool, BANNER, &raw).await.unwrap(),
            None => setting::remove(&pool, BANNER).await.unwrap(),
        }
    }
}

//! 站点配置（`settings` 表，`key` + `value` 两列，`value` 存 JSON 文本）。
//!
//! 对齐 Go 版 `provider.GetSettingValue` / `SaveSettingValue`：
//! - `GetSettingValue` 按 key 取一条 `value`；
//! - `SaveSettingValue` 把任意值 `json.Marshal` 后按 key upsert。
//!
//! 插件配置几乎全部存在这张表里（`sitemap` / `rewrite` / `anchor` /
//! `sensitive_words` / `push` / `order` …），所以它是后续所有插件移植的公共底座。
//!
//! 注意：`key` 是 MySQL 保留字，SQL 里必须写成 `` `key` ``。

use serde_json::Value;
use sqlx::mysql::MySqlPool;

use super::ModelError;

/// 按 key 取原始 JSON 文本。
pub async fn get_raw(pool: &MySqlPool, key: &str) -> anyhow::Result<Option<String>> {
    let raw: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE `key` = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(raw)
}

/// 按 key 取 JSON；不存在或不是合法 JSON 时返回 `None`。
pub async fn get_json(pool: &MySqlPool, key: &str) -> anyhow::Result<Option<Value>> {
    let Some(raw) = get_raw(pool, key).await? else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    Ok(serde_json::from_str::<Value>(raw).ok())
}

/// 按 key 取 JSON；不存在时返回空对象，坏 JSON 也当空对象（宽松，便于读配置）。
pub async fn get_json_or_empty(pool: &MySqlPool, key: &str) -> anyhow::Result<Value> {
    Ok(get_json(pool, key)
        .await?
        .unwrap_or_else(|| Value::Object(Default::default())))
}

/// 按 key 写入原始文本（upsert：行不存在则插入）。
pub async fn save_raw(pool: &MySqlPool, key: &str, raw: &str) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO settings (`key`, `value`) VALUES (?, ?) \
         ON DUPLICATE KEY UPDATE `value` = VALUES(`value`)",
    )
    .bind(key)
    .bind(raw)
    .execute(pool)
    .await?;
    Ok(())
}

/// 按 key 写入 JSON（`json.Marshal` 语义）。
pub async fn save_json(pool: &MySqlPool, key: &str, value: &Value) -> anyhow::Result<()> {
    let raw = serde_json::to_string(value)?;
    save_raw(pool, key, &raw).await
}

/// 读现配置 → 用 `patch` 里的**顶层字段**覆盖（Go 版 `library.MergeJSONPayload`）。
///
/// 语义是「字段级 PATCH」：只有 `patch` 里出现的键会被替换，其余保持原值；
/// 嵌套对象整块替换（对应 Go 把 payload 的字段值赋给 struct 字段）。
/// 落库并返回合并后的完整对象。
pub async fn merge_json(pool: &MySqlPool, key: &str, patch: &Value) -> anyhow::Result<Value> {
    let mut current = get_json_or_empty(pool, key).await?;
    merge_into(&mut current, patch);
    save_json(pool, key, &current).await?;
    Ok(current)
}

/// 把 `patch` 的顶层键覆盖进 `base`（`base` 不是对象时直接替换）。
pub fn merge_into(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(base_map), Value::Object(patch_map)) => {
            for (key, value) in patch_map {
                base_map.insert(key.clone(), value.clone());
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

/// 删除一个配置键（极少用，主要给测试清理）。
pub async fn remove(pool: &MySqlPool, key: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM settings WHERE `key` = ?")
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

/// 表是否存在。
async fn table_exists(pool: &MySqlPool, table: &str) -> Result<bool, ModelError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = ?",
    )
    .bind(table)
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
}

/// 列是否存在；不存在时执行 `ddl` 补列。
async fn ensure_column(
    pool: &MySqlPool,
    table: &str,
    column: &str,
    ddl: &str,
) -> Result<bool, ModelError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.columns \
         WHERE table_schema = DATABASE() AND table_name = ? AND column_name = ?",
    )
    .bind(table)
    .bind(column)
    .fetch_one(pool)
    .await?;
    if count > 0 {
        return Ok(false);
    }
    sqlx::query(ddl).execute(pool).await?;
    Ok(true)
}

/// 数据库自动迁移（对齐 Go `provider.AutoMigrateDB` 的可用子集）。
///
/// **先整库备份再执行变更**：备份失败就直接返回，不做任何 DDL。
/// 变更都是幂等的：补齐模块自定义字段表，并给 `users` 补上会员中心的 `api_token` 列。
pub async fn migrate_db(
    pool: &MySqlPool,
    backup_dir: &std::path::Path,
    template_dir: &std::path::Path,
    public_dir: &std::path::Path,
) -> Result<Value, ModelError> {
    // 1. 先备份，拿到文件名后才允许往下走
    let backup_file = super::backup::dump(pool, backup_dir, template_dir, public_dir).await?;

    // 2. 模块自定义字段表：缺失则建表，缺列则补列
    let modules = super::module::list(pool).await?;
    let mut migrated_modules = Vec::new();
    for module in &modules {
        super::module::migrate(pool, module).await?;
        migrated_modules.push(module.table_name.clone());
    }

    // 3. 会员中心新增的用户 API Token（Go `AutoMigrateMemberCenter`）
    let mut added_columns = Vec::new();
    if table_exists(pool, "users").await? {
        let added = ensure_column(
            pool,
            "users",
            "api_token",
            "ALTER TABLE `users` ADD COLUMN `api_token` varchar(64) NOT NULL DEFAULT ''",
        )
        .await?;
        if added {
            added_columns.push("users.api_token");
        }
    }

    Ok(serde_json::json!({
        "backup": backup_file,
        "modules": migrated_modules,
        "added_columns": added_columns,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn merges_top_level_fields_only() {
        let mut base = serde_json::json!({"a": 1, "b": {"x": 1}, "c": 3});
        let patch = serde_json::json!({"b": {"y": 2}});
        merge_into(&mut base, &patch);
        // b 整块替换，不是深度合并
        assert_eq!(base["b"], serde_json::json!({"y": 2}));
        // 未出现的键保持原值
        assert_eq!(base["a"], serde_json::json!(1));
        assert_eq!(base["c"], serde_json::json!(3));
    }

    #[test]
    fn merge_into_replaces_when_base_is_not_object() {
        let mut base = serde_json::json!(5);
        merge_into(&mut base, &serde_json::json!({"a": 1}));
        assert_eq!(base, serde_json::json!({"a": 1}));
    }

    #[tokio::test]
    async fn reads_real_settings_row() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        // `system` 是所有站点都会有的配置行（token_secret 就在里面）
        let raw = get_raw(&pool, "system").await.unwrap();
        if raw.is_some() {
            let value = get_json(&pool, "system").await.unwrap().expect("应是合法 JSON");
            assert!(value.is_object(), "{value}");
        }
        // 不存在的键返回 None
        assert!(get_raw(&pool, "__rustcms_not_exists__").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn saves_and_restores_setting() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let key = "__rustcms_test_setting__";
        let backup = get_raw(&pool, key).await.unwrap();

        save_json(&pool, key, &serde_json::json!({"open": 1, "name": "测试"})).await.unwrap();
        let value = get_json(&pool, key).await.unwrap().unwrap();
        assert_eq!(value["open"], serde_json::json!(1));

        // 字段级 merge：只改 open，name 保留
        let merged = merge_json(&pool, key, &serde_json::json!({"open": 0}))
            .await
            .unwrap();
        assert_eq!(merged["open"], serde_json::json!(0));
        assert_eq!(merged["name"], serde_json::json!("测试"));

        // 还原
        match backup {
            Some(raw) => save_raw(&pool, key, &raw).await.unwrap(),
            None => remove(&pool, key).await.unwrap(),
        }
    }
}

//! 分销员配置与身份。
//!
//! 配置存 `settings.retailer`，身份存 `users.is_retailer`。
//! 保存配置只认 `allow_self` / `become_retailer` 两个字段
//! （`become_retailer = 1` 表示自动成为分销员，列表只显示 `is_retailer = 1` 的会员）。

use serde_json::{json, Map, Value};
use sqlx::mysql::MySqlPool;

use super::{site_setting, ModelError};

/// 分销员配置键。
pub const RETAILER_SETTING: &str = "retailer";

/// 读分销员配置（补默认值）。
pub async fn config_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, RETAILER_SETTING).await?;
    if let Value::Object(map) = &mut value {
        map.entry("allow_self").or_insert(json!(0));
        map.entry("become_retailer").or_insert(json!(0));
    }
    Ok(value)
}

/// 保存分销员配置（字段级 merge，只落 既有实现认的两个字段）。
pub async fn config_save(pool: &MySqlPool, patch: &Value) -> Result<Value, ModelError> {
    if !patch.is_object() {
        return Err(ModelError::invalid("配置格式错误"));
    }
    let mut allowed = Map::new();
    for key in ["allow_self", "become_retailer"] {
        if let Some(item) = patch.get(key) {
            allowed.insert(key.to_string(), item.clone());
        }
    }
    site_setting::save_section(pool, RETAILER_SETTING, &Value::Object(allowed))
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    config_get(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("读取失败: {err}")))
}

/// 设置 / 取消分销员身份。
pub async fn set_retailer(
    pool: &MySqlPool,
    user_id: u32,
    is_retailer: bool,
) -> Result<(), ModelError> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;
    if exists.unwrap_or(0) == 0 {
        return Err(ModelError::invalid("用户不存在"));
    }
    let flag = if is_retailer { 1i8 } else { 0 };
    let now = chrono::Utc::now().timestamp();
    sqlx::query("UPDATE users SET is_retailer = ?, updated_time = ? WHERE id = ?")
        .bind(flag)
        .bind(now)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    Ok(())
}

/// 更新分销员真实姓名（复用会员表 `real_name`）。
pub async fn set_real_name(
    pool: &MySqlPool,
    user_id: u32,
    real_name: &str,
) -> Result<(), ModelError> {
    let real_name = real_name.trim();
    if real_name.is_empty() {
        return Err(ModelError::invalid("姓名不能为空"));
    }
    let exists: Option<i64> = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;
    if exists.unwrap_or(0) == 0 {
        return Err(ModelError::invalid("用户不存在"));
    }
    let now = chrono::Utc::now().timestamp();
    sqlx::query("UPDATE users SET real_name = ?, updated_time = ? WHERE id = ?")
        .bind(real_name)
        .bind(now)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, setting, test_pool};

    #[tokio::test]
    async fn retailer_config_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, RETAILER_SETTING).await.unwrap();

        let loaded = config_get(&pool).await.unwrap();
        assert_eq!(loaded["allow_self"], json!(0));
        assert_eq!(loaded["become_retailer"], json!(0));

        let saved = config_save(&pool, &json!({"allow_self": 1, "become_retailer": 1}))
            .await
            .unwrap();
        assert_eq!(saved["allow_self"], json!(1));
        assert_eq!(saved["become_retailer"], json!(1));

        // 非对象 / 只传部分字段
        assert!(config_save(&pool, &json!("bad")).await.is_err());
        let saved = config_save(&pool, &json!({"allow_self": 0})).await.unwrap();
        assert_eq!(saved["allow_self"], json!(0));
        assert_eq!(saved["become_retailer"], json!(1), "未传字段保留");

        match backup {
            Some(raw) => setting::save_raw(&pool, RETAILER_SETTING, &raw).await.unwrap(),
            None => setting::remove(&pool, RETAILER_SETTING).await.unwrap(),
        }
    }

    #[tokio::test]
    async fn retailer_flag_and_realname() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let user_id: u32 =
            sqlx::query_scalar("SELECT id FROM users ORDER BY id DESC LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        let origin: (i8, String) =
            sqlx::query_as("SELECT is_retailer, real_name FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_one(&pool)
                .await
                .unwrap();

        set_retailer(&pool, user_id, true).await.unwrap();
        let (flag, _): (i8, String) =
            sqlx::query_as("SELECT is_retailer, real_name FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(flag, 1);

        set_retailer(&pool, user_id, false).await.unwrap();
        let (flag, _): (i8, String) =
            sqlx::query_as("SELECT is_retailer, real_name FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(flag, 0);

        let marker = format!("分销员{user_id}测试");
        set_real_name(&pool, user_id, &marker).await.unwrap();
        let (_, real_name): (i8, String) =
            sqlx::query_as("SELECT is_retailer, real_name FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(real_name, marker);

        // 边界：用户不存在 / 空姓名
        assert!(set_retailer(&pool, 4294967295, true).await.is_err());
        assert!(set_real_name(&pool, user_id, "  ").await.is_err());

        sqlx::query("UPDATE users SET is_retailer = ?, real_name = ? WHERE id = ?")
            .bind(origin.0)
            .bind(&origin.1)
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
    }
}

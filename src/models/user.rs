//! 前台会员（`users` 表）。

use serde::Serialize;
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

/// 会员账号（只取前台登录需要的字段）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct User {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_name: String,
    #[serde(skip_serializing)]
    pub password: String,
    pub real_name: String,
    pub avatar_url: String,
    pub email: String,
    pub phone: String,
    pub group_id: u32,
    /// `users.status` 是 `tinyint(1)`，MySQL 驱动按 BOOLEAN 解析
    pub status: bool,
    pub last_login: Option<i64>,
}

/// 按用户名查会员。
pub async fn get_by_name(pool: &MySqlPool, user_name: &str) -> anyhow::Result<Option<User>> {
    let row = sqlx::query_as::<_, User>(
        "SELECT id, created_time, updated_time, user_name, password, real_name, avatar_url, \
                email, phone, group_id, status, last_login \
         FROM users WHERE user_name = ? LIMIT 1",
    )
    .bind(user_name)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// 按 ID 查会员。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<User>> {
    let row = sqlx::query_as::<_, User>(
        "SELECT id, created_time, updated_time, user_name, password, real_name, avatar_url, \
                email, phone, group_id, status, last_login \
         FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// 记录登录时间。
pub async fn touch_login(pool: &MySqlPool, id: u32) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query("UPDATE users SET last_login = ?, updated_time = ? WHERE id = ?")
        .bind(now)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::db_lock;
    use crate::models::test_pool;

    #[tokio::test]
    async fn reads_real_users() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };

        let id: Option<u32> = sqlx::query_scalar("SELECT id FROM users ORDER BY id LIMIT 1")
            .fetch_optional(&pool)
            .await
            .unwrap();
        let Some(id) = id else {
            return;
        };

        let user = get(&pool, id).await.unwrap().expect("会员应存在");
        assert_eq!(user.id, id);
        assert!(user.password.starts_with("$2"), "{}", user.password);

        let by_name = get_by_name(&pool, &user.user_name).await.unwrap();
        assert_eq!(by_name.map(|item| item.id), Some(id));

        assert!(get(&pool, u32::MAX).await.unwrap().is_none());
    }
}

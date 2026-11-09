//! 邮件订阅（`subscribers` + `subscriber_categories`）。
//!
//! 注意两张表的 `id` / 时间列都是**有符号** `int(11)`，所以用 `i32` 映射，
//! 取成 `u32`/`i64` 会解码失败。

use serde::Serialize;
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::ModelError;

/// 订阅用户。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Subscriber {
    pub id: i32,
    pub user_id: Option<i32>,
    pub email: String,
    pub category_id: Option<i32>,
    pub remark: Option<String>,
    /// 0 = 退订 / 1 = 已确认（具体含义由前台订阅流程决定）
    pub status: Option<i8>,
    pub send_times: Option<i32>,
    pub error_times: Option<i32>,
    pub created_time: Option<i32>,
    pub updated_time: Option<i32>,
}

/// 订阅分类。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct SubscriberCategory {
    pub id: i32,
    pub title: String,
    pub subscriber_count: Option<i32>,
    pub created_time: Option<i32>,
}

/// 分页查订阅用户，返回 `(总数, 本页记录)`。
pub async fn list(
    pool: &MySqlPool,
    category_id: i64,
    email: &str,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Subscriber>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;
    let email = email.trim();

    let mut where_sql = String::from(" WHERE 1 = 1");
    if category_id > 0 {
        where_sql.push_str(" AND category_id = ?");
    }
    let like = format!("%{email}%");
    if !email.is_empty() {
        where_sql.push_str(" AND email LIKE ?");
    }

    let count_sql = format!("SELECT COUNT(*) FROM subscribers{where_sql}");
    let mut counter = sqlx::query_scalar::<_, i64>(&count_sql);
    if category_id > 0 {
        counter = counter.bind(category_id);
    }
    if !email.is_empty() {
        counter = counter.bind(&like);
    }
    let total = counter.fetch_one(pool).await?;

    let list_sql = format!("SELECT * FROM subscribers{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?");
    let mut query = sqlx::query_as::<_, Subscriber>(&list_sql);
    if category_id > 0 {
        query = query.bind(category_id);
    }
    if !email.is_empty() {
        query = query.bind(&like);
    }
    let rows = query.bind(page_size).bind(offset).fetch_all(pool).await?;
    Ok((total, rows))
}

/// 按 ID 取订阅用户。
pub async fn get(pool: &MySqlPool, id: i32) -> anyhow::Result<Option<Subscriber>> {
    Ok(
        sqlx::query_as::<_, Subscriber>("SELECT * FROM subscribers WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 新增 / 更新订阅用户（邮箱必填）。
pub async fn save(pool: &MySqlPool, id: i32, email: &str) -> Result<Subscriber, ModelError> {
    let email = email.trim();
    if email.is_empty() {
        return Err(ModelError::invalid("请填写邮箱地址"));
    }
    let now = chrono::Utc::now().timestamp() as i32;

    if id > 0 {
        if get(pool, id)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .is_none()
        {
            return Err(ModelError::invalid("订阅用户不存在"));
        }
        sqlx::query("UPDATE subscribers SET email = ?, updated_time = ? WHERE id = ?")
            .bind(email)
            .bind(now)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
        return get(pool, id)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .ok_or_else(|| ModelError::invalid("订阅用户不存在"));
    }

    let new_id = sqlx::query(
        "INSERT INTO subscribers (email, status, send_times, error_times, created_time, \
            updated_time) VALUES (?, 1, 0, 0, ?, ?)",
    )
    .bind(email)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?
    .last_insert_id() as i32;

    get(pool, new_id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("订阅用户创建失败"))
}

/// 删除订阅用户。
pub async fn delete(pool: &MySqlPool, id: i32) -> anyhow::Result<bool> {
    Ok(sqlx::query("DELETE FROM subscribers WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// 全部订阅分类（按 id 升序）。
pub async fn category_list(pool: &MySqlPool) -> anyhow::Result<Vec<SubscriberCategory>> {
    Ok(sqlx::query_as::<_, SubscriberCategory>(
        "SELECT id, title, subscriber_count, created_time FROM subscriber_categories ORDER BY id ASC",
    )
    .fetch_all(pool)
    .await?)
}

/// 新增 / 更新订阅分类（名称唯一）。
pub async fn category_save(
    pool: &MySqlPool,
    id: i32,
    title: &str,
) -> Result<SubscriberCategory, ModelError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ModelError::invalid("请填写分类名称"));
    }

    let exists: Option<i32> = sqlx::query_scalar(
        "SELECT id FROM subscriber_categories WHERE title = ? LIMIT 1",
    )
    .bind(title)
    .fetch_optional(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;

    if let Some(exists_id) = exists {
        if exists_id != id {
            return Err(ModelError::invalid("分类名称已存在"));
        }
    }

    let now = chrono::Utc::now().timestamp() as i32;
    let saved_id = if id > 0 {
        sqlx::query("UPDATE subscriber_categories SET title = ? WHERE id = ?")
            .bind(title)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
        id
    } else {
        sqlx::query(
            "INSERT INTO subscriber_categories (title, subscriber_count, created_time) VALUES (?, 0, ?)",
        )
        .bind(title)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?
        .last_insert_id() as i32
    };

    let row = sqlx::query_as::<_, SubscriberCategory>(
        "SELECT id, title, subscriber_count, created_time FROM subscriber_categories WHERE id = ?",
    )
    .bind(saved_id)
    .fetch_optional(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;
    row.ok_or_else(|| ModelError::invalid("分类不存在"))
}

/// 删除订阅分类，返回是否真的删掉。
pub async fn category_delete(pool: &MySqlPool, id: i32) -> anyhow::Result<bool> {
    Ok(sqlx::query("DELETE FROM subscriber_categories WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[tokio::test]
    async fn subscriber_crud_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM subscribers WHERE email LIKE 'rustcms-sub%'")
            .execute(&pool)
            .await;

        // 邮箱必填
        assert_eq!(
            save(&pool, 0, "  ").await.unwrap_err().to_string(),
            "请填写邮箱地址"
        );

        let created = save(&pool, 0, "rustcms-sub@example.com").await.unwrap();
        assert!(created.id > 0);
        assert_eq!(created.email, "rustcms-sub@example.com");
        assert_eq!(created.status, Some(1));

        // 列表 + 邮箱筛选
        let (total, rows) = list(&pool, 0, "rustcms-sub", 1, 20).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].id, created.id);

        // 更新
        let updated = save(&pool, created.id, "rustcms-sub2@example.com")
            .await
            .unwrap();
        assert_eq!(updated.email, "rustcms-sub2@example.com");

        // 不存在的 id
        assert!(save(&pool, 999_999, "x@example.com").await.is_err());

        assert!(delete(&pool, created.id).await.unwrap());
        assert!(get(&pool, created.id).await.unwrap().is_none());
        assert!(!delete(&pool, created.id).await.unwrap());
    }

    #[tokio::test]
    async fn category_roundtrip_with_unique_title() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM subscriber_categories WHERE title LIKE 'rustcms-订阅%'")
            .execute(&pool)
            .await;

        assert_eq!(
            category_save(&pool, 0, "").await.unwrap_err().to_string(),
            "请填写分类名称"
        );

        let created = category_save(&pool, 0, "rustcms-订阅分类").await.unwrap();
        assert!(created.id > 0);

        // 同名再建 → 报错
        let err = category_save(&pool, 0, "rustcms-订阅分类").await.unwrap_err();
        assert_eq!(err.to_string(), "分类名称已存在");

        // 更新自己不算冲突
        let renamed = category_save(&pool, created.id, "rustcms-订阅分类改名")
            .await
            .unwrap();
        assert_eq!(renamed.title, "rustcms-订阅分类改名");

        let list = category_list(&pool).await.unwrap();
        assert!(list.iter().any(|item| item.id == created.id));

        assert!(category_delete(&pool, created.id).await.unwrap());
        assert!(!category_delete(&pool, created.id).await.unwrap());

        let _ = sqlx::query("DELETE FROM subscriber_categories WHERE title LIKE 'rustcms-订阅%'")
            .execute(&pool)
            .await;
    }
}

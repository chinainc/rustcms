//! 分销佣金记录。
//!
//! `commissions` 表 + `users` 联表补 `user_name`。
//! `status`：0 未提现 / 1 已提现 / -1 退款等原因失效。

use serde::{Deserialize, Serialize};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::ModelError;

/// 佣金记录（含联表 `user_name`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct CommissionRow {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_id: u32,
    pub order_id: String,
    pub order_amount: i64,
    pub amount: i64,
    pub status: i8,
    /// 库列是有符号 `int(10)`（不是 unsigned）。
    pub withdraw_id: i32,
    pub remark: String,
    pub user_name: Option<String>,
}

/// 佣金记录筛选。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CommissionFilter {
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub status: Option<i8>,
}

/// 分页查佣金记录，返回 `(总数, 本页记录)`。
pub async fn list(
    pool: &MySqlPool,
    filter: &CommissionFilter,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<CommissionRow>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;

    let mut where_sql = String::from(" WHERE 1 = 1");
    let user_like = format!("%{}%", filter.user_name.trim());
    if !filter.user_name.trim().is_empty() {
        where_sql.push_str(" AND u.user_name LIKE ?");
    }
    if filter.status.is_some() {
        where_sql.push_str(" AND c.status = ?");
    }

    let count_sql = format!(
        "SELECT COUNT(*) FROM commissions c \
         LEFT JOIN users u ON u.id = c.user_id{where_sql}"
    );
    let mut counter = sqlx::query_scalar::<_, i64>(&count_sql);
    if !filter.user_name.trim().is_empty() {
        counter = counter.bind(&user_like);
    }
    if let Some(status) = filter.status {
        counter = counter.bind(status);
    }
    let total = counter.fetch_one(pool).await?;

    let list_sql = format!(
        "SELECT c.id, c.created_time, c.updated_time, c.user_id, c.order_id, c.order_amount, \
         c.amount, c.status, c.withdraw_id, c.remark, u.user_name \
         FROM commissions c LEFT JOIN users u ON u.id = c.user_id{where_sql} \
         ORDER BY c.id DESC LIMIT ? OFFSET ?"
    );
    let mut query = sqlx::query_as::<_, CommissionRow>(&list_sql);
    if !filter.user_name.trim().is_empty() {
        query = query.bind(&user_like);
    }
    if let Some(status) = filter.status {
        query = query.bind(status);
    }
    let rows = query.bind(page_size).bind(offset).fetch_all(pool).await?;
    Ok((total, rows))
}

/// 按 ID 取佣金记录。
pub async fn get(pool: &MySqlPool, id: u32) -> Result<Option<CommissionRow>, ModelError> {
    let sql =
        "SELECT c.id, c.created_time, c.updated_time, c.user_id, c.order_id, c.order_amount, \
         c.amount, c.status, c.withdraw_id, c.remark, u.user_name \
         FROM commissions c LEFT JOIN users u ON u.id = c.user_id WHERE c.id = ?";
    sqlx::query_as::<_, CommissionRow>(sql)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[tokio::test]
    async fn commission_list_and_get() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let now = chrono::Utc::now().timestamp();
        let user_id: u32 = sqlx::query_scalar("SELECT id FROM users ORDER BY id LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();

        let row_id = sqlx::query(
            "INSERT INTO commissions (created_time, updated_time, user_id, order_id, \
                order_amount, amount, status, withdraw_id, remark) \
             VALUES (?, ?, ?, ?, 10000, 100, 0, 0, ?)",
        )
        .bind(now)
        .bind(now)
        .bind(user_id)
        .bind(format!("rustcms-commission-{now}"))
        .bind("佣金测试")
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_id() as u32;

        // 列表（含 user_name 联表）
        let filter = CommissionFilter::default();
        let (total, rows) = list(&pool, &filter, 1, 20).await.unwrap();
        assert!(total >= 1);
        let found = rows.iter().find(|row| row.id == row_id).expect("应找到新行");
        assert_eq!(found.user_id, user_id);
        assert!(found.user_name.is_some(), "应联表带出 user_name");
        assert_eq!(found.amount, 100);

        // 状态筛选
        let filter = CommissionFilter {
            status: Some(0),
            ..Default::default()
        };
        let (_, rows) = list(&pool, &filter, 1, 20).await.unwrap();
        assert!(rows.iter().all(|row| row.status == 0));

        // user_name 筛选
        let name = found.user_name.clone().unwrap();
        if !name.is_empty() {
            let piece: String = name.chars().take(1).collect();
            let filter = CommissionFilter {
                user_name: piece,
                ..Default::default()
            };
            let (_, rows) = list(&pool, &filter, 1, 50).await.unwrap();
            assert!(rows.iter().any(|row| row.id == row_id));
        }

        // 详情
        let got = get(&pool, row_id).await.unwrap().expect("应存在");
        assert_eq!(got.order_id, format!("rustcms-commission-{now}"));
        assert!(get(&pool, 0).await.unwrap().is_none());

        sqlx::query("DELETE FROM commissions WHERE id = ?")
            .bind(row_id)
            .execute(&pool)
            .await
            .unwrap();
    }
}

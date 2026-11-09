//! 用户提现记录。
//!
//! 状态：0 等待处理 / 1 已同意 / 2 已提现 / -1 提现错误（按既有约定）。
//! 申请提现：汇总该分销员 `status = 0` 的佣金，
//! 整体生成一条提现记录，并把佣金批量置为已提现（`withdraw_id` 回填）。
//! 审核通过 / 打款完成时给用户记一条「支出 / 提现」财务流水；余额提现在申请时
//! 已写入 `order_id = W<id>` 的流水，此时只把那条流水标记为已完成，不再重复记账。

use serde::Serialize;
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::ModelError;

/// 财务流水方向：支出。
const FINANCE_OUTPUT: i8 = 2;
/// 财务流水动作：提现。
const FINANCE_ACTION_WITHDRAW: i8 = 5;
/// 佣金状态：已提现。
const COMMISSION_STATUS_PAID: i8 = 1;

/// 提现记录（列表联表补 `user_name`，对应既有约定）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct UserWithdraw {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_id: u32,
    pub amount: i64,
    pub success_time: i64,
    pub withdraw_way: i8,
    pub status: i8,
    pub error_times: i32,
    pub last_time: i64,
    pub remark: String,
    /// 联表字段：详情接口与既有接口契约一致返回空串，列表接口带出用户名。
    pub user_name: String,
}

/// 分页查提现记录（`id desc`，联表补 `user_name`，既有约定）。
pub async fn list(
    pool: &MySqlPool,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<UserWithdraw>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_withdraws")
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as::<_, UserWithdraw>(
        "SELECT w.*, COALESCE(u.user_name, '') AS user_name \
         FROM user_withdraws w LEFT JOIN users u ON u.id = w.user_id \
         ORDER BY w.id DESC LIMIT ? OFFSET ?",
    )
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok((total, rows))
}

/// 按 ID 取提现记录（不联表，`user_name` 为空串，既有约定）。
pub async fn get(pool: &MySqlPool, id: u32) -> Result<Option<UserWithdraw>, ModelError> {
    sqlx::query_as::<_, UserWithdraw>(
        "SELECT w.*, '' AS user_name FROM user_withdraws w WHERE w.id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))
}

/// 为分销员申请提现，返回新建的提现记录。
pub async fn apply(pool: &MySqlPool, user_id: u32) -> Result<UserWithdraw, ModelError> {
    // SUM 返回 DECIMAL，必须 CAST 后再解码（项目约定的 schema 坑）
    let total: i64 = sqlx::query_scalar(
        "SELECT CAST(COALESCE(SUM(amount), 0) AS SIGNED) FROM commissions WHERE user_id = ? AND status = 0",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;
    if total <= 0 {
        return Err(ModelError::invalid("没有可以提现的金额"));
    }
    // 低于 2 元（200 分）不可提现到微信
    if total < 200 {
        return Err(ModelError::invalid("低于2元不可提现到微信零钱"));
    }

    let now = chrono::Utc::now().timestamp();
    let mut tx = pool
        .begin()
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    let withdraw_id = sqlx::query(
        "INSERT INTO user_withdraws (created_time, updated_time, user_id, amount, \
         success_time, withdraw_way, status, error_times, last_time, remark) \
         VALUES (?, ?, ?, ?, 0, 1, 0, 0, 0, '')",
    )
    .bind(now)
    .bind(now)
    .bind(user_id)
    .bind(total)
    .execute(&mut *tx)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?
    .last_insert_id() as u32;
    sqlx::query("UPDATE commissions SET withdraw_id = ?, status = ? WHERE user_id = ? AND status = 0")
        .bind(withdraw_id as i32)
        .bind(COMMISSION_STATUS_PAID)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;

    // 等待计划任务去处理
    get(pool, withdraw_id)
        .await?
        .ok_or_else(|| ModelError::invalid("提现记录不存在"))
}

/// 余额提现在申请时已写入 `order_id = W<id>` 的流水：把它标记为已完成。
/// 返回 true 表示已处理，调用方不要再记账。
async fn mark_balance_withdraw_done(pool: &MySqlPool, withdraw: &UserWithdraw) -> bool {
    let order_id = format!("W{}", withdraw.id);
    let found: Option<(u32, i8)> =
        sqlx::query_as("SELECT id, status FROM finances WHERE order_id = ? AND user_id = ? LIMIT 1")
            .bind(&order_id)
            .bind(withdraw.user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    let Some((finance_id, status)) = found else {
        return false;
    };
    if status != 1 {
        let _ = sqlx::query("UPDATE finances SET status = 1 WHERE id = ?")
            .bind(finance_id)
            .execute(pool)
            .await;
    }
    true
}

/// 记一条「支出 / 提现」财务流水（既有约定 / `SetUserWithdrawFinished` 共用）。
async fn record_finance(pool: &MySqlPool, withdraw: &UserWithdraw) -> Result<(), ModelError> {
    let balance: i64 = sqlx::query_scalar("SELECT balance FROM users WHERE id = ?")
        .bind(withdraw.user_id)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO finances (created_time, updated_time, user_id, direction, amount, \
         after_amount, action, order_id, status, remark) \
         VALUES (?, ?, ?, ?, ?, ?, ?, '', 1, '')",
    )
    .bind(now)
    .bind(now)
    .bind(withdraw.user_id)
    .bind(FINANCE_OUTPUT)
    .bind(withdraw.amount)
    .bind(balance)
    .bind(FINANCE_ACTION_WITHDRAW)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    Ok(())
}

/// 审核通过（状态置 1，既有约定）。
pub async fn set_approval(pool: &MySqlPool, id: u32) -> Result<(), ModelError> {
    let withdraw = get(pool, id)
        .await?
        .ok_or_else(|| ModelError::invalid("提现记录不存在"))?;
    let now = chrono::Utc::now().timestamp();
    sqlx::query("UPDATE user_withdraws SET status = 1, updated_time = ? WHERE id = ?")
        .bind(now)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;

    if mark_balance_withdraw_done(pool, &withdraw).await {
        return Ok(());
    }
    record_finance(pool, &withdraw).await
}

/// 标记打款完成（状态置 2 + 成功时间，既有约定）。
pub async fn set_finished(pool: &MySqlPool, id: u32) -> Result<(), ModelError> {
    let withdraw = get(pool, id)
        .await?
        .ok_or_else(|| ModelError::invalid("提现记录不存在"))?;
    let now = chrono::Utc::now().timestamp();
    sqlx::query("UPDATE user_withdraws SET status = 2, success_time = ?, updated_time = ? WHERE id = ?")
        .bind(now)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;

    if mark_balance_withdraw_done(pool, &withdraw).await {
        return Ok(());
    }
    record_finance(pool, &withdraw).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[tokio::test]
    async fn withdraw_apply_and_status_flow() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let now = chrono::Utc::now().timestamp();
        let user_id: u32 = sqlx::query_scalar("SELECT id FROM users ORDER BY id LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();

        // 隔离该用户既有的待提现佣金（共用真实库，可能已有数据），测完恢复
        let existing: Vec<(u32, i8)> =
            sqlx::query_as("SELECT id, status FROM commissions WHERE user_id = ? AND status = 0")
                .bind(user_id)
                .fetch_all(&pool)
                .await
                .unwrap();
        for (id, _) in &existing {
            sqlx::query("UPDATE commissions SET status = -1 WHERE id = ?")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }

        // 无可提现佣金
        assert!(apply(&pool, user_id).await.is_err());

        // 准备佣金（status=0 待提现，共 300 分）
        let commission_ids: Vec<u32> = vec![
            sqlx::query(
                "INSERT INTO commissions (created_time, updated_time, user_id, order_id, \
                 order_amount, amount, status, withdraw_id, remark) \
                 VALUES (?, ?, ?, ?, 1000, 100, 0, 0, '')",
            )
            .bind(now)
            .bind(now)
            .bind(user_id)
            .bind(format!("rustcms-wd-{now}a"))
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_id() as u32,
            sqlx::query(
                "INSERT INTO commissions (created_time, updated_time, user_id, order_id, \
                 order_amount, amount, status, withdraw_id, remark) \
                 VALUES (?, ?, ?, ?, 1000, 200, 0, 0, '')",
            )
            .bind(now)
            .bind(now)
            .bind(user_id)
            .bind(format!("rustcms-wd-{now}b"))
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_id() as u32,
        ];

        // 低于 2 元（200 分）不可提现
        sqlx::query("UPDATE commissions SET amount = 100 WHERE id = ?")
            .bind(commission_ids[0])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE commissions SET amount = 99 WHERE id = ?")
            .bind(commission_ids[1])
            .execute(&pool)
            .await
            .unwrap();
        assert!(apply(&pool, user_id).await.is_err());

        // 恢复 300 分并申请
        sqlx::query("UPDATE commissions SET amount = 200 WHERE id = ?")
            .bind(commission_ids[1])
            .execute(&pool)
            .await
            .unwrap();
        let withdraw = apply(&pool, user_id).await.unwrap();
        assert_eq!(withdraw.amount, 300);
        assert_eq!(withdraw.status, 0);
        assert_eq!(withdraw.withdraw_way, 1);
        assert_eq!(withdraw.user_name, "", "详情接口 user_name 为空串");

        // 佣金被置为已提现且回填 withdraw_id
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM commissions WHERE user_id = ? AND status = 1 AND withdraw_id = ?")
                .bind(user_id)
                .bind(withdraw.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 2);
        // 佣金清零后再申请应失败
        assert!(apply(&pool, user_id).await.is_err());

        // 列表联表带 user_name
        let (total, rows) = list(&pool, 1, 20).await.unwrap();
        assert!(total >= 1);
        let found = rows.iter().find(|row| row.id == withdraw.id).expect("应找到新记录");
        assert!(!found.user_name.is_empty(), "列表应联表带出用户名");

        // 审核通过：状态 1 + 一条提现流水
        set_approval(&pool, withdraw.id).await.unwrap();
        let got = get(&pool, withdraw.id).await.unwrap().unwrap();
        assert_eq!(got.status, 1);
        let (finance_count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM finances WHERE user_id = ? AND action = 5 AND amount = 300",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(finance_count, 1);

        // 打款完成：状态 2 + success_time
        set_finished(&pool, withdraw.id).await.unwrap();
        let got = get(&pool, withdraw.id).await.unwrap().unwrap();
        assert_eq!(got.status, 2);
        assert!(got.success_time > 0);

        // 不存在的记录
        assert!(set_approval(&pool, 429496729).await.is_err());
        assert!(set_finished(&pool, 429496729).await.is_err());
        assert!(get(&pool, 429496729).await.unwrap().is_none());

        // 清理
        sqlx::query("DELETE FROM commissions WHERE id IN (?, ?)")
            .bind(commission_ids[0])
            .bind(commission_ids[1])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM finances WHERE user_id = ? AND action = 5 AND amount = 300")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM user_withdraws WHERE id = ?")
            .bind(withdraw.id)
            .execute(&pool)
            .await
            .unwrap();
        // 恢复被隔离的既有佣金
        for (id, status) in &existing {
            sqlx::query("UPDATE commissions SET status = ? WHERE id = ?")
                .bind(status)
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
    }
}

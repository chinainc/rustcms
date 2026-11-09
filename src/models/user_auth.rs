//! 会员认证（实名 / 企业认证，表 `user_auths`）。
//!
//! 行为约定：
//! - `status`：`0` 待审核、`1` 已通过、`-1` 已驳回；
//! - 驳回必须填原因（`remark`）；
//! - 审核通过/驳回都记录 `audit_time` 与 `audit_admin_id`。

use serde::{Deserialize, Serialize};
use sqlx::mysql::{MySql, MySqlPool};
use sqlx::{FromRow, QueryBuilder};

use super::ModelError;

/// 认证记录（附带会员用户名）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct UserAuth {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_id: u32,
    /// 1 = 实名认证，2 = 企业认证
    pub kind: i8,
    pub real_name: String,
    pub id_card: String,
    pub id_card_front: String,
    pub id_card_back: String,
    pub id_card_hand: String,
    pub company_name: String,
    pub company_no: String,
    pub company_legal: String,
    pub company_licence: String,
    pub phone: String,
    pub status: i8,
    pub remark: String,
    pub audit_time: i64,
    pub audit_admin_id: u32,
    /// 来自 `users.user_name`（LEFT JOIN）
    pub user_name: String,
}

/// 审核入参。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuditInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub status: i32,
    #[serde(default)]
    pub remark: String,
}

/// 列表筛选。
#[derive(Debug, Clone, Default)]
pub struct UserAuthFilter {
    /// `None` 表示全部
    pub status: Option<i32>,
    pub keyword: String,
}

const SELECT_COLUMNS: &str = "a.id, a.created_time, a.updated_time, a.user_id, \
    a.`type` AS kind, a.real_name, a.id_card, a.id_card_front, a.id_card_back, a.id_card_hand, \
    a.company_name, a.company_no, a.company_legal, a.company_licence, a.phone, a.status, \
    a.remark, a.audit_time, a.audit_admin_id, IFNULL(u.user_name, '') AS user_name";

fn push_filter(builder: &mut QueryBuilder<'_, MySql>, filter: &UserAuthFilter) {
    builder.push(" WHERE 1 = 1");
    if let Some(status) = filter.status {
        builder.push(" AND a.status = ").push_bind(status as i32);
    }
    let keyword = filter.keyword.trim();
    if !keyword.is_empty() {
        let like = format!("%{keyword}%");
        builder
            .push(" AND (a.real_name LIKE ")
            .push_bind(like.clone())
            .push(" OR a.company_name LIKE ")
            .push_bind(like.clone())
            .push(" OR u.user_name LIKE ")
            .push_bind(like)
            .push(")");
    }
}

/// 分页查认证列表，返回 `(总数, 本页记录)`。
pub async fn list(
    pool: &MySqlPool,
    filter: &UserAuthFilter,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<UserAuth>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;

    let mut counter = QueryBuilder::<MySql>::new(
        "SELECT COUNT(*) FROM user_auths a LEFT JOIN users u ON u.id = a.user_id",
    );
    push_filter(&mut counter, filter);
    let total: i64 = counter.build_query_scalar().fetch_one(pool).await?;

    let mut query = QueryBuilder::<MySql>::new(format!(
        "SELECT {SELECT_COLUMNS} FROM user_auths a LEFT JOIN users u ON u.id = a.user_id"
    ));
    push_filter(&mut query, filter);
    query
        .push(" ORDER BY a.id DESC LIMIT ")
        .push_bind(page_size)
        .push(" OFFSET ")
        .push_bind(offset);
    let rows = query.build_query_as::<UserAuth>().fetch_all(pool).await?;

    Ok((total, rows))
}

/// 按 ID 取认证记录。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<UserAuth>> {
    let sql = format!(
        "SELECT {SELECT_COLUMNS} FROM user_auths a LEFT JOIN users u ON u.id = a.user_id \
         WHERE a.id = ?"
    );
    Ok(sqlx::query_as::<_, UserAuth>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// 各状态数量：`(待审核, 已通过, 已驳回)`。
pub async fn stat(pool: &MySqlPool) -> anyhow::Result<(i64, i64, i64)> {
    let rows: Vec<(i8, i64)> =
        sqlx::query_as("SELECT status, COUNT(*) FROM user_auths GROUP BY status")
            .fetch_all(pool)
            .await?;

    let mut pending = 0;
    let mut passed = 0;
    let mut rejected = 0;
    for (status, count) in rows {
        match status {
            1 => passed = count,
            -1 => rejected = count,
            _ => pending += count,
        }
    }
    Ok((pending, passed, rejected))
}

/// 审核：`status = 1` 通过，`-1` 驳回（必须填原因）。
pub async fn audit(
    pool: &MySqlPool,
    input: &AuditInput,
    admin_id: u32,
) -> Result<UserAuth, ModelError> {
    let Some(existing) = get(pool, input.id).await? else {
        return Err(ModelError::invalid("认证记录不存在"));
    };
    if !matches!(input.status, 1 | -1) {
        return Err(ModelError::invalid("审核状态不正确"));
    }
    if input.status == -1 && input.remark.trim().is_empty() {
        return Err(ModelError::invalid("请填写驳回原因"));
    }

    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "UPDATE user_auths SET status = ?, remark = ?, audit_time = ?, audit_admin_id = ?, \
            updated_time = ? WHERE id = ?",
    )
    .bind(input.status as i8)
    .bind(input.remark.trim())
    .bind(now)
    .bind(admin_id)
    .bind(now)
    .bind(existing.id)
    .execute(pool)
    .await?;

    get(pool, existing.id)
        .await?
        .ok_or_else(|| ModelError::invalid("认证记录不存在"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn rejects_bad_audit_status() {
        let input = AuditInput {
            id: 1,
            status: 5,
            remark: String::new(),
        };
        assert!(!matches!(input.status, 1 | -1));
    }

    #[tokio::test]
    async fn reads_real_user_auths() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let (total, rows) = list(&pool, &UserAuthFilter::default(), 1, 5).await.unwrap();
        assert!(total >= 0);
        assert!(rows.len() as i64 <= 5);

        let (pending, passed, rejected) = stat(&pool).await.unwrap();
        let db_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_auths")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(pending + passed + rejected, db_total);

        assert!(get(&pool, u32::MAX).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn audits_temp_record() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        // 造一条临时认证记录（user_id 用不存在的值即可，JOIN 后 user_name 为空串）
        let _ = sqlx::query("DELETE FROM user_auths WHERE real_name = 'rustcms-audit-test'")
            .execute(&pool)
            .await;
        let result = sqlx::query(
            "INSERT INTO user_auths (created_time, updated_time, user_id, `type`, real_name, \
                id_card, id_card_front, id_card_back, id_card_hand, company_name, company_no, \
                company_legal, company_licence, phone, status, remark, audit_time, audit_admin_id) \
             VALUES (UNIX_TIMESTAMP(), UNIX_TIMESTAMP(), 999999, 1, 'rustcms-audit-test', \
                '', '', '', '', '', '', '', '', '', 0, '', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let id = result.last_insert_id() as u32;

        // 驳回必须填原因
        let err = audit(
            &pool,
            &AuditInput {
                id,
                status: -1,
                remark: String::new(),
            },
            1,
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "请填写驳回原因");

        // 非法状态
        let err = audit(
            &pool,
            &AuditInput {
                id,
                status: 2,
                remark: String::new(),
            },
            1,
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "审核状态不正确");

        // 通过
        let passed = audit(
            &pool,
            &AuditInput {
                id,
                status: 1,
                remark: "ok".into(),
            },
            7,
        )
        .await
        .unwrap();
        assert_eq!(passed.status, 1);
        assert_eq!(passed.audit_admin_id, 7);
        assert!(passed.audit_time > 0);

        // 列表按状态过滤能看到
        let (total, rows) = list(
            &pool,
            &UserAuthFilter {
                status: Some(1),
                keyword: "rustcms-audit-test".into(),
            },
            1,
            10,
        )
        .await
        .unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].id, id);

        let _ = sqlx::query("DELETE FROM user_auths WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await;
    }
}

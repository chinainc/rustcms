//! 后台内容的批量操作（状态 / 标记 / 排序 / 分类 / 父级 / 时间 / 回收站恢复）。
//!
//! 对齐 Go 版 `provider/archive.go` 的 `UpdateArchive*` 系列。
//!
//! 内容在两张表之间流转：
//! - `archives`：已发布内容；
//! - `archive_drafts`：草稿 / 回收站（`status = 0` 草稿，`99` 回收站）。
//!
//! 两张表列结构完全一致，所以这里用显式列清单做 `REPLACE ... SELECT` 搬运。

use sqlx::mysql::{MySql, MySqlPool, MySqlConnection};
use sqlx::QueryBuilder;

use super::{archive_write, ModelError};

/// `archives` 的列清单（30 列，**没有 `status`**：正式表只存已发布内容）。
const ARCHIVE_COLUMNS: &str = "id, parent_id, created_time, updated_time, title, seo_title, \
    url_token, keywords, description, module_id, category_id, place_id, views, comment_count, \
    favorite_count, images, template, canonical_url, fixed_link, user_id, price, stock, \
    read_level, password, sort, has_pseudo, keyword_id, origin_url, origin_title, origin_id";

/// `archive_drafts` 的列清单 = 正式表 30 列 + `status`。
const DRAFT_COLUMNS: &str = "id, parent_id, created_time, updated_time, title, seo_title, \
    url_token, keywords, description, module_id, category_id, place_id, views, comment_count, \
    favorite_count, images, template, canonical_url, fixed_link, user_id, price, stock, \
    read_level, password, sort, has_pseudo, keyword_id, origin_url, origin_title, origin_id, \
    status";

fn require_ids(ids: &[i64]) -> Result<(), ModelError> {
    if ids.is_empty() {
        return Err(ModelError::invalid("没有要操作的文档"));
    }
    Ok(())
}

/// 读内容所属分类（搬运前先记下来，搬完要重算计数）。
async fn category_of(pool: &MySqlPool, id: i64, from_draft: bool) -> Option<u32> {
    let table = if from_draft { "archive_drafts" } else { "archives" };
    let sql = format!("SELECT category_id FROM `{table}` WHERE id = ?");
    sqlx::query_scalar::<_, u32>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

/// 草稿 → 正式表（发布 / 恢复）。
async fn publish_draft(conn: &mut MySqlConnection, id: i64, reset_time: bool) -> Result<bool, ModelError> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM archive_drafts WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    if exists.is_none() {
        return Ok(false);
    }

    let sql = format!(
        "REPLACE INTO archives ({ARCHIVE_COLUMNS}) SELECT {ARCHIVE_COLUMNS} \
         FROM archive_drafts WHERE id = ?"
    );
    sqlx::query(&sql).bind(id).execute(&mut *conn).await?;

    if reset_time {
        let now = chrono::Utc::now().timestamp();
        sqlx::query("UPDATE archives SET created_time = ?, updated_time = ? WHERE id = ?")
            .bind(now)
            .bind(now)
            .bind(id)
            .execute(&mut *conn)
            .await?;
    }

    sqlx::query("DELETE FROM archive_drafts WHERE id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(true)
}

/// 正式表 → 草稿表（下架）。
async fn unpublish(conn: &mut MySqlConnection, id: i64) -> Result<bool, ModelError> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM archives WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    if exists.is_none() {
        return Ok(false);
    }

    let sql = format!(
        "REPLACE INTO archive_drafts ({DRAFT_COLUMNS}) SELECT {ARCHIVE_COLUMNS}, 0 \
         FROM archives WHERE id = ?"
    );
    sqlx::query(&sql).bind(id).execute(&mut *conn).await?;
    sqlx::query("DELETE FROM archives WHERE id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(true)
}

async fn recount(pool: &MySqlPool, categories: &[u32]) -> Result<(), ModelError> {
    let unique: std::collections::BTreeSet<u32> = categories.iter().copied().collect();
    for category_id in unique {
        let _ = archive_write::refresh_category_count(pool, category_id).await;
    }
    Ok(())
}

/// 批量改状态：`1` = 发布（草稿 → 正式），`0` = 下架（正式 → 草稿）。
pub async fn set_status(pool: &MySqlPool, ids: &[i64], status: u32) -> Result<usize, ModelError> {
    require_ids(ids)?;
    let mut affected_categories = Vec::new();
    let mut count = 0usize;
    let mut conn = pool.acquire().await?;

    for id in ids {
        if status == 1 {
            if let Some(cid) = category_of(pool, *id, true).await {
                affected_categories.push(cid);
            }
            if publish_draft(&mut conn, *id, true).await? {
                count += 1;
            }
        } else {
            if let Some(cid) = category_of(pool, *id, false).await {
                affected_categories.push(cid);
            }
            if unpublish(&mut conn, *id).await? {
                count += 1;
            }
        }
    }
    drop(conn);
    recount(pool, &affected_categories).await?;
    Ok(count)
}

/// 回收站恢复（草稿 → 正式，保留原时间）。
pub async fn recover(pool: &MySqlPool, id: i64) -> Result<bool, ModelError> {
    let category = category_of(pool, id, true).await;
    let mut conn = pool.acquire().await?;
    let ok = publish_draft(&mut conn, id, false).await?;
    drop(conn);
    if let Some(cid) = category {
        recount(pool, &[cid]).await?;
    }
    Ok(ok)
}

/// 移到回收站（正式表 → 草稿表，`status = 99`）。
pub async fn recycle(pool: &MySqlPool, id: i64) -> Result<bool, ModelError> {
    let category = category_of(pool, id, false).await;
    let mut conn = pool.acquire().await?;
    let ok = unpublish(&mut conn, id).await?;
    if ok {
        sqlx::query("UPDATE archive_drafts SET status = 99 WHERE id = ?")
            .bind(id)
            .execute(&mut *conn)
            .await?;
    }
    drop(conn);
    if let Some(cid) = category {
        recount(pool, &[cid]).await?;
    }
    Ok(ok)
}

/// 永久删除草稿 / 回收站内容。
pub async fn delete_draft(pool: &MySqlPool, id: i64) -> Result<bool, ModelError> {
    let category = category_of(pool, id, true).await;
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM archive_data WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM archive_drafts WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    if let Some(cid) = category {
        recount(pool, &[cid]).await?;
    }
    Ok(true)
}

/// 批量设置标记（`a,b,c` → 逐个写入 `archive_flags`，覆盖旧值）。
pub async fn set_flags(pool: &MySqlPool, ids: &[i64], flag: &str) -> Result<usize, ModelError> {
    require_ids(ids)?;
    let flags: Vec<String> = flag
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.chars().next().unwrap_or_default().to_string())
        .collect();

    let mut tx = pool.begin().await?;
    for id in ids {
        sqlx::query("DELETE FROM archive_flags WHERE archive_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for item in &flags {
            sqlx::query("INSERT INTO archive_flags (archive_id, flag) VALUES (?, ?)")
                .bind(id)
                .bind(item)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(ids.len())
}

/// 设置单个内容的排序值。
pub async fn set_sort(pool: &MySqlPool, id: i64, sort: u32) -> Result<bool, ModelError> {
    let mut changed = false;
    for table in ["archives", "archive_drafts"] {
        let sql = format!("UPDATE `{table}` SET sort = ?, updated_time = ? WHERE id = ?");
        let result = sqlx::query(&sql)
            .bind(sort)
            .bind(chrono::Utc::now().timestamp())
            .bind(id)
            .execute(pool)
            .await?;
        changed |= result.rows_affected() > 0;
    }
    Ok(changed)
}

/// 批量设置父级内容。
pub async fn set_parent(pool: &MySqlPool, ids: &[i64], parent_id: i64) -> Result<usize, ModelError> {
    require_ids(ids)?;
    let mut builder = QueryBuilder::<MySql>::new("UPDATE archives SET parent_id = ");
    builder.push_bind(parent_id);
    builder.push(" WHERE id IN (");
    let mut list = builder.separated(", ");
    for id in ids {
        list.push_bind(*id);
    }
    list.push_unseparated(")");
    builder.build().execute(pool).await?;

    let mut builder = QueryBuilder::<MySql>::new("UPDATE archive_drafts SET parent_id = ");
    builder.push_bind(parent_id);
    builder.push(" WHERE id IN (");
    let mut list = builder.separated(", ");
    for id in ids {
        list.push_bind(*id);
    }
    list.push_unseparated(")");
    builder.build().execute(pool).await?;

    Ok(ids.len())
}

/// 批量改分类（同时更新主分类与多分类关联）。
pub async fn set_category(
    pool: &MySqlPool,
    ids: &[i64],
    category_id: u32,
    category_ids: &[u32],
) -> Result<usize, ModelError> {
    require_ids(ids)?;

    let mut targets: Vec<u32> = category_ids.to_vec();
    if targets.is_empty() && category_id > 0 {
        targets.push(category_id);
    }
    if targets.is_empty() {
        return Err(ModelError::invalid("请选择分类"));
    }

    // 主分类 = 第一个
    let default_category = super::category::get(pool, targets[0])
        .await?
        .ok_or_else(|| ModelError::invalid("分类不存在"))?;
    let mut affected = Vec::new();

    let mut tx = pool.begin().await?;
    for id in ids {
        // 多分类关联
        sqlx::query("DELETE FROM archive_categories WHERE archive_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for cat in &targets {
            sqlx::query("INSERT IGNORE INTO archive_categories (archive_id, category_id) VALUES (?, ?)")
                .bind(id)
                .bind(cat)
                .execute(&mut *tx)
                .await?;
        }
        for table in ["archives", "archive_drafts"] {
            let sql = format!(
                "UPDATE `{table}` SET category_id = ?, module_id = ?, updated_time = ? WHERE id = ?"
            );
            sqlx::query(&sql)
                .bind(default_category.id)
                .bind(default_category.module_id)
                .bind(chrono::Utc::now().timestamp())
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        affected.push(default_category.id);
    }
    tx.commit().await?;

    recount(pool, &affected).await?;
    Ok(ids.len())
}

/// 批量改时间。
///
/// `time`：1 = 选中内容的 `created_time`，2 = 选中内容的 `updated_time`，
/// 3 = 全部内容的 `created_time`，4 = 全部内容的 `updated_time`（与 Go 一致）。
pub async fn set_time(pool: &MySqlPool, ids: &[i64], time_mode: u32) -> Result<usize, ModelError> {
    if ids.is_empty() && time_mode < 3 {
        return Err(ModelError::invalid("没有要操作的文档"));
    }
    let now = chrono::Utc::now().timestamp();
    match time_mode {
        4 => {
            sqlx::query("UPDATE archives SET updated_time = ? WHERE id > 0")
                .bind(now)
                .execute(pool)
                .await?;
        }
        3 => {
            sqlx::query("UPDATE archives SET created_time = ? WHERE id > 0")
                .bind(now)
                .execute(pool)
                .await?;
        }
        mode => {
            let column = if mode == 2 { "updated_time" } else { "created_time" };
            for table in ["archives", "archive_drafts"] {
                let mut builder =
                    QueryBuilder::<MySql>::new(format!("UPDATE `{table}` SET {column} = "));
                builder.push_bind(now);
                builder.push(" WHERE id IN (");
                let mut list = builder.separated(", ");
                for id in ids {
                    list.push_bind(*id);
                }
                list.push_unseparated(")");
                builder.build().execute(pool).await?;
            }
        }
    }
    Ok(ids.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn rejects_empty_ids() {
        assert!(require_ids(&[]).is_err());
        assert!(require_ids(&[1]).is_ok());
    }

    #[tokio::test]
    async fn set_sort_on_missing_id_reports_false() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        assert!(!set_sort(&pool, i64::MAX, 1).await.unwrap());
    }

    #[tokio::test]
    async fn empty_ids_are_rejected_by_batch_ops() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        assert!(set_status(&pool, &[], 1).await.is_err());
        assert!(set_flags(&pool, &[], "a").await.is_err());
        assert!(set_parent(&pool, &[], 0).await.is_err());
        assert!(set_category(&pool, &[], 0, &[]).await.is_err());
        assert!(set_time(&pool, &[], 1).await.is_err());
    }
}

//! 评论（`comments` 表，`pluginComment.go`）。
//!
//! `status`：0 = 待审核，1 = 正常。
//! 后台改内容时会**强制置为正常**并把正文做 HTML 转义（防 XSS），与 Go 一致。

use std::collections::HashMap;

use serde::Serialize;
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::ModelError;

/// 评论。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Comment {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub archive_id: i64,
    pub user_id: u32,
    pub user_name: String,
    pub email: String,
    pub ip: String,
    pub vote_count: i32,
    pub content: Option<String>,
    pub parent_id: u32,
    pub to_uid: u32,
    /// 0 = 待审核，1 = 正常
    pub status: i8,
    pub rating: i8,
}

/// HTML 转义（对应 Go 的 `html.EscapeString`）。
pub fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// 把「孤立的」`&nbsp;` 换成普通空格（连续多个保留原样）。
///
/// 对应 Go 的 `library.ReplaceSingleSpace`。
pub fn replace_single_space(input: &str) -> String {
    const MARK: &str = "&nbsp;";
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(index) = rest.find(MARK) {
        out.push_str(&rest[..index]);
        let mut end = index;
        let mut count = 0usize;
        while rest[end..].starts_with(MARK) {
            end += MARK.len();
            count += 1;
        }
        if count == 1 {
            out.push(' ');
        } else {
            out.push_str(&MARK.repeat(count));
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// 分页查评论，返回 `(总数, 本页记录)`（id 倒序）。
pub async fn list(pool: &MySqlPool, page: i64, page_size: i64) -> anyhow::Result<(i64, Vec<Comment>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM comments")
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as::<_, Comment>("SELECT * FROM comments ORDER BY id DESC LIMIT ? OFFSET ?")
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    Ok((total, rows))
}

/// 按 ID 取评论。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Comment>> {
    Ok(
        sqlx::query_as::<_, Comment>("SELECT * FROM comments WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 批量取内容标题（用于列表展示 `item_title`）。
pub async fn archive_titles(
    pool: &MySqlPool,
    ids: &[i64],
) -> anyhow::Result<HashMap<i64, String>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut builder = sqlx::QueryBuilder::<sqlx::MySql>::new("SELECT id, title FROM archives WHERE id IN (");
    let mut list = builder.separated(", ");
    for id in ids {
        list.push_bind(*id);
    }
    list.push_unseparated(")");
    let rows: Vec<(i64, String)> = builder.build_query_as().fetch_all(pool).await?;
    Ok(rows.into_iter().collect())
}

/// 修改评论内容（转义正文、置为正常、补 IP）。
pub async fn update(
    pool: &MySqlPool,
    id: u32,
    user_name: &str,
    content: &str,
    ip: &str,
) -> Result<Comment, ModelError> {
    let Some(existing) = get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
    else {
        return Err(ModelError::invalid("评论不存在"));
    };

    let content = escape_html(&replace_single_space(content));
    let user_name = user_name.trim();
    let user_name = if user_name.is_empty() {
        existing.user_name.clone()
    } else {
        user_name.to_string()
    };
    let ip = ip.trim();
    let ip = if ip.is_empty() {
        existing.ip.clone()
    } else {
        ip.to_string()
    };

    sqlx::query(
        "UPDATE comments SET updated_time = ?, user_name = ?, content = ?, ip = ?, status = 1 \
         WHERE id = ?",
    )
    .bind(chrono::Utc::now().timestamp())
    .bind(&user_name)
    .bind(&content)
    .bind(&ip)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;

    get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("评论不存在"))
}

/// 删除评论，并同步扣减内容的 `comment_count`（不小于 0）。
pub async fn delete(pool: &MySqlPool, id: u32) -> Result<bool, ModelError> {
    let Some(comment) = get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
    else {
        return Ok(false);
    };

    sqlx::query("DELETE FROM comments WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?;

    let _ = sqlx::query(
        "UPDATE archives SET comment_count = GREATEST(comment_count - 1, 0) WHERE id = ?",
    )
    .bind(comment.archive_id)
    .execute(pool)
    .await;
    Ok(true)
}

/// 批量设置审核状态，返回受影响行数。
pub async fn set_status(pool: &MySqlPool, ids: &[u32], status: i8) -> anyhow::Result<u64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let mut builder = sqlx::QueryBuilder::<sqlx::MySql>::new("UPDATE comments SET status = ");
    builder.push_bind(status);
    builder.push(", updated_time = ");
    builder.push_bind(chrono::Utc::now().timestamp());
    builder.push(" WHERE id IN (");
    let mut list = builder.separated(", ");
    for id in ids {
        list.push_bind(*id);
    }
    list.push_unseparated(")");
    let result = builder.build().execute(pool).await?;
    Ok(result.rows_affected())
}

/// 单条切换审核状态（正常 ↔ 待审核）。
pub async fn toggle(pool: &MySqlPool, id: u32) -> Result<Comment, ModelError> {
    let Some(comment) = get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
    else {
        return Err(ModelError::invalid("评论不存在"));
    };
    let next: i8 = if comment.status == 1 { 0 } else { 1 };
    sqlx::query("UPDATE comments SET status = ?, updated_time = ? WHERE id = ?")
        .bind(next)
        .bind(chrono::Utc::now().timestamp())
        .bind(id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("评论不存在"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn escapes_and_collapses_nbsp() {
        assert_eq!(
            escape_html("<script>alert(\"x\")</script>"),
            "&lt;script&gt;alert(&#34;x&#34;)&lt;/script&gt;"
        );
        assert_eq!(replace_single_space("a&nbsp;b"), "a b");
        // 连续多个保留原样
        assert_eq!(replace_single_space("a&nbsp;&nbsp;b"), "a&nbsp;&nbsp;b");
        assert_eq!(replace_single_space("无标记"), "无标记");
    }

    #[tokio::test]
    async fn comment_crud_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM comments WHERE user_name = 'rustcms-comment-tester'")
            .execute(&pool)
            .await;

        let archive_id: i64 = sqlx::query_scalar("SELECT id FROM archives ORDER BY id LIMIT 1")
            .fetch_optional(&pool)
            .await
            .unwrap()
            .unwrap_or(0);

        let now = chrono::Utc::now().timestamp();
        let id = sqlx::query(
            "INSERT INTO comments (created_time, updated_time, archive_id, user_id, user_name, \
                email, ip, vote_count, content, parent_id, to_uid, status, rating) \
             VALUES (?, ?, ?, 0, 'rustcms-comment-tester', '', '127.0.0.1', 0, '原始内容', 0, 0, 0, 0)",
        )
        .bind(now)
        .bind(now)
        .bind(archive_id)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_id() as u32;

        // 列表
        let (total, rows) = list(&pool, 1, 20).await.unwrap();
        assert!(total >= 1);
        assert!(rows.iter().any(|item| item.id == id));

        // 修改：转义 + 置为正常
        let updated = update(&pool, id, "rustcms-comment-tester", "<b>x</b>&nbsp;y", "1.2.3.4")
            .await
            .unwrap();
        assert_eq!(updated.status, 1);
        assert_eq!(updated.content.as_deref(), Some("&lt;b&gt;x&lt;/b&gt; y"));
        assert_eq!(updated.ip, "1.2.3.4");

        // 切换状态
        let toggled = toggle(&pool, id).await.unwrap();
        assert_eq!(toggled.status, 0);

        // 批量置为正常
        let affected = set_status(&pool, &[id], 1).await.unwrap();
        assert_eq!(affected, 1);

        // 标题映射
        if archive_id > 0 {
            let titles = archive_titles(&pool, &[archive_id]).await.unwrap();
            assert!(titles.contains_key(&archive_id));
        }

        // 删除
        assert!(delete(&pool, id).await.unwrap());
        assert!(get(&pool, id).await.unwrap().is_none());
        assert!(!delete(&pool, id).await.unwrap());
    }
}

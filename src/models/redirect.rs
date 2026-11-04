//! 301 跳转（表 `redirects`，`from_url` 唯一）。
//!
//! 对齐 Go 版 `controller/manageController/pluginRedirect.go`：
//! - 来源/目标地址都会补前导 `/`（以 `http` 开头的保持原样）；
//! - 来源与目标不能相同；
//! - 新增时来源已存在则复用那条记录（相当于更新目标）；
//! - 导入格式：一行一条，逗号分隔 `from_url, to_url`。

use serde::{Deserialize, Serialize};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::ModelError;

/// 跳转记录。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Redirect {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub from_url: String,
    pub to_url: String,
}

/// 保存入参。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RedirectInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub from_url: String,
    #[serde(default)]
    pub to_url: String,
}

/// 补前导 `/`（以 `http` 开头的保持原样）。
fn normalize(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with("http") || raw.starts_with('/') {
        raw.to_string()
    } else {
        format!("/{raw}")
    }
}

fn validate(from_url: &str, to_url: &str) -> Result<(String, String), ModelError> {
    let from_url = normalize(from_url);
    let to_url = normalize(to_url);
    if from_url.is_empty() {
        return Err(ModelError::invalid("请填写来源链接"));
    }
    if to_url.is_empty() {
        return Err(ModelError::invalid("请填写跳转链接"));
    }
    if from_url == to_url {
        return Err(ModelError::invalid("来源链接和跳转链接不能相同"));
    }
    Ok((from_url, to_url))
}

/// 按 ID 取跳转。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Redirect>> {
    Ok(
        sqlx::query_as::<_, Redirect>("SELECT * FROM redirects WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 按来源地址取跳转。
pub async fn get_by_from(pool: &MySqlPool, from_url: &str) -> anyhow::Result<Option<Redirect>> {
    Ok(
        sqlx::query_as::<_, Redirect>("SELECT * FROM redirects WHERE from_url = ? LIMIT 1")
            .bind(from_url)
            .fetch_optional(pool)
            .await?,
    )
}

/// 分页查跳转，返回 `(总数, 本页记录)`。
pub async fn list(
    pool: &MySqlPool,
    from_url: &str,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Redirect>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;
    let keyword = from_url.trim();

    if keyword.is_empty() {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM redirects")
            .fetch_one(pool)
            .await?;
        let rows = sqlx::query_as::<_, Redirect>(
            "SELECT * FROM redirects ORDER BY id DESC LIMIT ? OFFSET ?",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        return Ok((total, rows));
    }

    let like = format!("%{keyword}%");
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM redirects WHERE from_url LIKE ?")
        .bind(&like)
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as::<_, Redirect>(
        "SELECT * FROM redirects WHERE from_url LIKE ? ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(&like)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok((total, rows))
}

/// 新建 / 更新跳转。
pub async fn save(pool: &MySqlPool, input: &RedirectInput) -> Result<Redirect, ModelError> {
    let (from_url, to_url) = validate(&input.from_url, &input.to_url)?;
    let now = chrono::Utc::now().timestamp();

    // 指定了 id：更新同 id 那条，来源冲突时报错
    if input.id > 0 {
        if get(pool, input.id).await?.is_none() {
            return Err(ModelError::invalid("链接不存在"));
        }
        if let Some(exists) = get_by_from(pool, &from_url).await? {
            if exists.id != input.id {
                return Err(ModelError::invalid(format!("链接 {from_url} 已存在")));
            }
        }
        sqlx::query("UPDATE redirects SET updated_time = ?, from_url = ?, to_url = ? WHERE id = ?")
            .bind(now)
            .bind(&from_url)
            .bind(&to_url)
            .bind(input.id)
            .execute(pool)
            .await?;
        return get(pool, input.id)
            .await?
            .ok_or_else(|| ModelError::invalid("链接不存在"));
    }

    // 未指定 id：来源已存在就更新它（与 Go 行为一致）
    if let Some(exists) = get_by_from(pool, &from_url).await? {
        sqlx::query("UPDATE redirects SET updated_time = ?, to_url = ? WHERE id = ?")
            .bind(now)
            .bind(&to_url)
            .bind(exists.id)
            .execute(pool)
            .await?;
        return get(pool, exists.id)
            .await?
            .ok_or_else(|| ModelError::invalid("链接不存在"));
    }

    let result = sqlx::query(
        "INSERT INTO redirects (created_time, updated_time, from_url, to_url) VALUES (?, ?, ?, ?)",
    )
    .bind(now)
    .bind(now)
    .bind(&from_url)
    .bind(&to_url)
    .execute(pool)
    .await?;
    let id = result.last_insert_id() as u32;
    get(pool, id)
        .await?
        .ok_or_else(|| ModelError::invalid("链接创建失败"))
}

/// 删除跳转，返回是否真的删掉。
pub async fn delete(pool: &MySqlPool, id: u32) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM redirects WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// 批量导入：一行 `from_url, to_url`，返回新增条数。
pub async fn import(pool: &MySqlPool, text: &str) -> Result<usize, ModelError> {
    let mut imported = 0usize;
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, ',');
        let from_url = parts.next().unwrap_or_default().trim();
        let to_url = parts.next().unwrap_or_default().trim();
        let (from_url, to_url) = match validate(from_url, to_url) {
            Ok(value) => value,
            // 单行不合法就跳过，不影响整体导入
            Err(_) => continue,
        };
        let now = chrono::Utc::now().timestamp();
        let result = sqlx::query(
            "INSERT INTO redirects (created_time, updated_time, from_url, to_url) VALUES (?, ?, ?, ?) \
             ON DUPLICATE KEY UPDATE to_url = VALUES(to_url), updated_time = VALUES(updated_time)",
        )
        .bind(now)
        .bind(now)
        .bind(&from_url)
        .bind(&to_url)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("导入失败: {err}")))?;
        if result.rows_affected() > 0 {
            imported += 1;
        }
    }
    Ok(imported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn normalizes_and_validates() {
        assert_eq!(normalize("old.html"), "/old.html");
        assert_eq!(normalize("/old.html"), "/old.html");
        assert_eq!(normalize("https://a.com/x"), "https://a.com/x");
        assert!(validate("", "/b").is_err());
        assert!(validate("/a", "/a").is_err());
    }

    #[tokio::test]
    async fn redirect_crud_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM redirects WHERE from_url LIKE '/rustcms%'")
            .execute(&pool)
            .await;

        // 来源=目标 被拒
        let err = save(
            &pool,
            &RedirectInput {
                from_url: "/rustcms-same".into(),
                to_url: "/rustcms-same".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "来源链接和跳转链接不能相同");

        // 新增（自动补前导斜杠）
        let created = save(
            &pool,
            &RedirectInput {
                from_url: "rustcms-old".into(),
                to_url: "rustcms-new".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(created.from_url, "/rustcms-old");
        assert_eq!(created.to_url, "/rustcms-new");

        // 同一来源再保存 → 复用记录、只改目标
        let again = save(
            &pool,
            &RedirectInput {
                from_url: "/rustcms-old".into(),
                to_url: "/rustcms-newer".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(again.id, created.id);
        assert_eq!(again.to_url, "/rustcms-newer");

        // 指定 id 更新到冲突来源 → 报错
        let other = save(
            &pool,
            &RedirectInput {
                from_url: "/rustcms-other".into(),
                to_url: "/rustcms-target".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let err = save(
            &pool,
            &RedirectInput {
                id: other.id,
                from_url: "/rustcms-old".into(),
                to_url: "/rustcms-x".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("已存在"), "{err}");

        // 搜索 + 删除
        let (total, rows) = list(&pool, "rustcms-old", 1, 20).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].id, created.id);
        assert!(delete(&pool, created.id).await.unwrap());
        assert!(get(&pool, created.id).await.unwrap().is_none());

        let _ = sqlx::query("DELETE FROM redirects WHERE from_url LIKE '/rustcms%'")
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    async fn imports_lines() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM redirects WHERE from_url LIKE '/rustcms-imp%'")
            .execute(&pool)
            .await;

        let text = "/rustcms-imp-1, /rustcms-imp-a\n/rustcms-imp-2, /rustcms-imp-b\n\n# 注释行\n坏行没有逗号\n/rustcms-imp-3,/rustcms-imp-c";
        // 注释行、空行、没有逗号的行都应被跳过
        let count = import(&pool, text).await.unwrap();
        assert_eq!(count, 3, "应只导入 3 条合法行");

        let (total, _) = list(&pool, "/rustcms-imp", 1, 20).await.unwrap();
        assert!(total >= 2);

        let _ = sqlx::query("DELETE FROM redirects WHERE from_url LIKE '/rustcms-imp%'")
            .execute(&pool)
            .await;
    }
}

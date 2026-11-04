//! 多站点（表 `websites`）。
//!
//! Go 版每个站点有**独立的 MySQL 连接**（`websites.mysql` 存 JSON 连接串），
//! Rust 侧目前是「单连接池」架构，因此这里只实现站点记录的读取与维护，
//! 真正的多站点数据隔离（切换连接池）尚未实现。
//!
//! 对齐 Go 版 `controller/manageController/website.go` 中的
//! `GetWebsiteList` / `GetWebsiteInfo` / `DeleteWebsite` / `GetCurrentSiteInfo`。

use serde::{Deserialize, Serialize};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::ModelError;

/// 站点记录。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Website {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub root_path: String,
    pub name: String,
    /// JSON 形式的 MySQL 连接串（Go 版用它建独立连接池）
    pub mysql: Option<String>,
    pub token_secret: String,
    pub status: u8,
    pub parent_id: i32,
    pub sync_time: Option<i64>,
    pub language_icon: String,
}

/// 保存入参。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WebsiteInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub root_path: String,
    #[serde(default)]
    pub status: Option<u8>,
    #[serde(default)]
    pub parent_id: Option<i32>,
    /// 直接给 JSON 字符串；不传则沿用原值
    #[serde(default)]
    pub mysql: Option<serde_json::Value>,
}

/// 分页查站点，返回 `(总数, 本页记录)`。
///
/// 注意：`websites` 表**没有 `base_url` 列**（域名存在各站点自己的
/// `settings.system` 里），所以 `base_url` 过滤参数暂不生效。
pub async fn list(
    pool: &MySqlPool,
    name: &str,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Website>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;
    let keyword = name.trim();

    let (total, rows) = if keyword.is_empty() {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM websites")
            .fetch_one(pool)
            .await?;
        let rows = sqlx::query_as::<_, Website>(
            "SELECT * FROM websites ORDER BY id ASC LIMIT ? OFFSET ?",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        (total, rows)
    } else {
        let like = format!("%{keyword}%");
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM websites WHERE name LIKE ?")
            .bind(&like)
            .fetch_one(pool)
            .await?;
        let rows = sqlx::query_as::<_, Website>(
            "SELECT * FROM websites WHERE name LIKE ? ORDER BY id ASC LIMIT ? OFFSET ?",
        )
        .bind(&like)
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        (total, rows)
    };

    Ok((total, rows))
}

/// 按 ID 取站点。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Website>> {
    Ok(
        sqlx::query_as::<_, Website>("SELECT * FROM websites WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 新建 / 更新站点记录。
pub async fn save(pool: &MySqlPool, input: &WebsiteInput) -> Result<Website, ModelError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(ModelError::invalid("请填写站点名称"));
    }
    let now = chrono::Utc::now().timestamp();
    let mysql_json = match input.mysql.as_ref() {
        Some(value) if value.is_object() => Some(serde_json::to_string(value)?),
        Some(value) if value.is_string() => Some(value.as_str().unwrap_or_default().to_string()),
        _ => None,
    };

    if input.id > 0 && get(pool, input.id).await?.is_some() {
        sqlx::query(
            "UPDATE websites SET updated_time = ?, name = ?, root_path = COALESCE(?, root_path), \
                status = COALESCE(?, status), parent_id = COALESCE(?, parent_id), \
                mysql = COALESCE(?, mysql) WHERE id = ?",
        )
        .bind(now)
        .bind(name)
        .bind(if input.root_path.trim().is_empty() {
            None
        } else {
            Some(input.root_path.trim().to_string())
        })
        .bind(input.status)
        .bind(input.parent_id)
        .bind(mysql_json)
        .bind(input.id)
        .execute(pool)
        .await?;
        return get(pool, input.id)
            .await?
            .ok_or_else(|| ModelError::invalid("站点不存在"));
    }

    // 新增：token_secret 复用 32 位随机串
    let secret = crate::auth::generate_secret(32);
    let result = sqlx::query(
        "INSERT INTO websites (created_time, updated_time, root_path, name, mysql, token_secret, \
            status, parent_id, sync_time, language_icon) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, '')",
    )
    .bind(now)
    .bind(now)
    .bind(input.root_path.trim())
    .bind(name)
    .bind(mysql_json)
    .bind(&secret)
    .bind(input.status.unwrap_or(1))
    .bind(input.parent_id.unwrap_or(0))
    .execute(pool)
    .await?;
    let id = result.last_insert_id() as u32;
    get(pool, id)
        .await?
        .ok_or_else(|| ModelError::invalid("站点创建失败"))
}

/// 删除站点记录（默认站点 1 不允许删除）。
pub async fn delete(pool: &MySqlPool, id: u32) -> Result<(), ModelError> {
    if id == 1 {
        return Err(ModelError::invalid("默认站点不能删除"));
    }
    if get(pool, id).await?.is_none() {
        return Err(ModelError::invalid("站点不存在"));
    }
    sqlx::query("DELETE FROM websites WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[tokio::test]
    async fn reads_default_site() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let (total, rows) = list(&pool, "", 1, 20).await.unwrap();
        assert!(total >= 1, "至少应有默认站点");
        assert!(rows.iter().any(|item| item.id == 1));

        let site = get(&pool, 1).await.unwrap().expect("默认站点应存在");
        assert!(!site.token_secret.is_empty());

        // 名称搜索
        let keyword = site.name.chars().take(2).collect::<String>();
        let (found, _) = list(&pool, &keyword, 1, 20).await.unwrap();
        assert!(found >= 1);

        assert!(get(&pool, u32::MAX).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn refuses_to_delete_default_site() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let err = delete(&pool, 1).await.unwrap_err();
        assert_eq!(err.to_string(), "默认站点不能删除");
        assert!(get(&pool, 1).await.unwrap().is_some());
    }
}

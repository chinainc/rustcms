//! 友情链接（表 `links`）。
//!
//! 对齐 Go 版 `controller/manageController/pluginLink.go`：
//! - 列表不分页，按 `sort ASC, id DESC`；
//! - 保存时 `status` **强制归零**（待回链检查），真正置为 1 要等 `PluginLinkCheck` 通过；
//! - 回链检查需要抓取对方页面，本项目暂无 HTTP 客户端，接口返回明确未实现。

use serde::{Deserialize, Serialize};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::ModelError;

/// 友情链接。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Link {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub title: String,
    pub link: String,
    pub back_link: String,
    pub my_title: String,
    pub my_link: String,
    pub contact: String,
    pub remark: String,
    /// 0 = 不添加 nofollow，1 = 添加
    pub nofollow: u8,
    pub sort: u32,
    /// 0 = 待审核，1 = 正常
    pub status: u8,
    pub checked_time: i64,
    pub logo: String,
}

/// 保存入参（对应 Go 的 `request.PluginLink`）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LinkInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub back_link: String,
    #[serde(default)]
    pub my_title: String,
    #[serde(default)]
    pub my_link: String,
    #[serde(default)]
    pub contact: String,
    #[serde(default)]
    pub remark: String,
    #[serde(default)]
    pub nofollow: u8,
    #[serde(default)]
    pub sort: Option<u32>,
    #[serde(default)]
    pub logo: String,
}

/// 全部友情链接。
pub async fn list(pool: &MySqlPool) -> anyhow::Result<Vec<Link>> {
    Ok(sqlx::query_as::<_, Link>(
        "SELECT * FROM links ORDER BY sort ASC, id DESC",
    )
    .fetch_all(pool)
    .await?)
}

/// 按 ID 取链接。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Link>> {
    Ok(sqlx::query_as::<_, Link>("SELECT * FROM links WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// 新建 / 更新友情链接（`status` 一律归零，等待回链检查）。
pub async fn save(pool: &MySqlPool, input: &LinkInput) -> Result<Link, ModelError> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err(ModelError::invalid("请填写链接名称"));
    }
    let link = input.link.trim();
    if link.is_empty() {
        return Err(ModelError::invalid("请填写链接地址"));
    }

    let now = chrono::Utc::now().timestamp();
    let sort = input.sort.unwrap_or(99);

    if input.id > 0 && get(pool, input.id).await?.is_some() {
        sqlx::query(
            "UPDATE links SET updated_time = ?, title = ?, `link` = ?, back_link = ?, \
                my_title = ?, my_link = ?, contact = ?, remark = ?, nofollow = ?, sort = ?, \
                logo = ?, status = 0 WHERE id = ?",
        )
        .bind(now)
        .bind(title)
        .bind(link)
        .bind(input.back_link.trim())
        .bind(&input.my_title)
        .bind(&input.my_link)
        .bind(&input.contact)
        .bind(&input.remark)
        .bind(input.nofollow)
        .bind(sort)
        .bind(&input.logo)
        .bind(input.id)
        .execute(pool)
        .await?;
        return get(pool, input.id)
            .await?
            .ok_or_else(|| ModelError::invalid("链接不存在"));
    }

    let result = sqlx::query(
        "INSERT INTO links (created_time, updated_time, title, `link`, back_link, my_title, \
            my_link, contact, remark, nofollow, sort, status, checked_time, logo) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?)",
    )
    .bind(now)
    .bind(now)
    .bind(title)
    .bind(link)
    .bind(input.back_link.trim())
    .bind(&input.my_title)
    .bind(&input.my_link)
    .bind(&input.contact)
    .bind(&input.remark)
    .bind(input.nofollow)
    .bind(sort)
    .bind(&input.logo)
    .execute(pool)
    .await?;
    let id = result.last_insert_id() as u32;
    get(pool, id)
        .await?
        .ok_or_else(|| ModelError::invalid("链接创建失败"))
}

/// 删除链接，返回是否真的删掉。
pub async fn delete(pool: &MySqlPool, id: u32) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM links WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[tokio::test]
    async fn link_crud_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM links WHERE title = 'rustcms-链接测试'")
            .execute(&pool)
            .await;

        // 必填校验
        assert_eq!(
            save(&pool, &LinkInput::default())
                .await
                .unwrap_err()
                .to_string(),
            "请填写链接名称"
        );

        let created = save(
            &pool,
            &LinkInput {
                title: "rustcms-链接测试".into(),
                link: "https://example.com".into(),
                back_link: "https://example.com/links".into(),
                sort: Some(5),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(created.id > 0);
        // 保存后状态一律归零
        assert_eq!(created.status, 0);
        assert_eq!(created.sort, 5);

        // 列表里能看到
        let list = list(&pool).await.unwrap();
        assert!(list.iter().any(|item| item.id == created.id));

        // 更新
        let updated = save(
            &pool,
            &LinkInput {
                id: created.id,
                title: "rustcms-链接测试（改）".into(),
                link: "https://example.org".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.title, "rustcms-链接测试（改）");
        assert_eq!(updated.link, "https://example.org");

        // 删除
        assert!(delete(&pool, created.id).await.unwrap());
        assert!(get(&pool, created.id).await.unwrap().is_none());
        assert!(!delete(&pool, created.id).await.unwrap());
    }
}

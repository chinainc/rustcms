//! 素材库（`materials` + `material_categories`）。
//!
//! 素材是可复用的富文本片段；`content` 存 `materials.content`，
//! 同时算一份 `md5` 便于去重/校验。

use serde::Serialize;
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::ModelError;

/// 素材。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Material {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub title: String,
    pub category_id: u32,
    pub content: Option<String>,
    pub origin_url: String,
    pub keyword: String,
    pub status: u8,
    pub auto_update: u8,
    pub use_count: u32,
    pub md5: Option<String>,
}

/// 素材分类。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct MaterialCategory {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub title: String,
    pub material_count: u32,
    pub status: u8,
}

/// 新建 / 更新素材的入参。
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct MaterialInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub category_id: u32,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub status: Option<u8>,
    #[serde(default)]
    pub auto_update: Option<u8>,
}

/// 去掉 HTML 标签并把连续空白压成一个空格。
pub fn clean_tags_and_spaces(input: &str) -> String {
    let mut stripped = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => stripped.push(ch),
            _ => {}
        }
    }

    let mut out = String::with_capacity(stripped.len());
    let mut last_space = true; // 开头不保留空格
    for ch in stripped.chars() {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    out.trim().to_string()
}

/// 把上传的文件转成文本。
///
/// 只接受 UTF-8（会剥掉 BOM）；其它编码需要转码库，明确报错。
pub fn convert_text(bytes: &[u8], remove_tag: bool) -> Result<String, ModelError> {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ModelError::invalid("文件不是 UTF-8 编码（暂不支持自动转码）"))?;
    let text = if remove_tag {
        clean_tags_and_spaces(text)
    } else {
        text.to_string()
    };
    Ok(text.trim().to_string())
}

fn md5_of(text: &str) -> String {
    super::attachment::md5_hex(text.as_bytes())
}

/// 分页查素材，返回 `(总数, 本页记录)`。
pub async fn list(
    pool: &MySqlPool,
    category_id: u32,
    keyword: &str,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Material>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;
    let keyword = keyword.trim();

    let mut where_sql = String::from(" WHERE 1 = 1");
    if category_id > 0 {
        where_sql.push_str(" AND category_id = ?");
    }
    if !keyword.is_empty() {
        where_sql.push_str(" AND (title LIKE ? OR keyword LIKE ?)");
    }

    let like = format!("%{keyword}%");
    let count_sql = format!("SELECT COUNT(*) FROM materials{where_sql}");
    let mut counter = sqlx::query_scalar::<_, i64>(&count_sql);
    if category_id > 0 {
        counter = counter.bind(category_id);
    }
    if !keyword.is_empty() {
        counter = counter.bind(&like).bind(&like);
    }
    let total = counter.fetch_one(pool).await?;

    let list_sql = format!("SELECT * FROM materials{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?");
    let mut query = sqlx::query_as::<_, Material>(&list_sql);
    if category_id > 0 {
        query = query.bind(category_id);
    }
    if !keyword.is_empty() {
        query = query.bind(&like).bind(&like);
    }
    let rows = query.bind(page_size).bind(offset).fetch_all(pool).await?;
    Ok((total, rows))
}

/// 按 ID 取素材。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Material>> {
    Ok(
        sqlx::query_as::<_, Material>("SELECT * FROM materials WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 新建 / 更新素材。
pub async fn save(pool: &MySqlPool, input: &MaterialInput) -> Result<Material, ModelError> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err(ModelError::invalid("请填写素材名称"));
    }
    let now = chrono::Utc::now().timestamp();
    let md5 = md5_of(&input.content);
    let status = input.status.unwrap_or(1).min(1);

    if input.id > 0 {
        if get(pool, input.id)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .is_none()
        {
            return Err(ModelError::invalid("素材不存在"));
        }
        sqlx::query(
            "UPDATE materials SET updated_time = ?, title = ?, category_id = ?, content = ?, \
                status = ?, auto_update = ?, md5 = ? WHERE id = ?",
        )
        .bind(now)
        .bind(title)
        .bind(input.category_id)
        .bind(&input.content)
        .bind(status)
        .bind(input.auto_update.unwrap_or(0).min(1))
        .bind(&md5)
        .bind(input.id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
        refresh_category_count(pool, input.category_id).await?;
        return get(pool, input.id)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .ok_or_else(|| ModelError::invalid("素材不存在"));
    }

    let id = sqlx::query(
        "INSERT INTO materials (created_time, updated_time, title, category_id, content, \
            origin_url, keyword, status, auto_update, use_count, md5) \
         VALUES (?, ?, ?, ?, ?, '', '', ?, ?, 0, ?)",
    )
    .bind(now)
    .bind(now)
    .bind(title)
    .bind(input.category_id)
    .bind(&input.content)
    .bind(status)
    .bind(input.auto_update.unwrap_or(0).min(1))
    .bind(&md5)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?
    .last_insert_id() as u32;

    refresh_category_count(pool, input.category_id).await?;
    get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("素材创建失败"))
}

/// 批量导入，返回导入条数。
pub async fn save_many(pool: &MySqlPool, inputs: &[MaterialInput]) -> Result<usize, ModelError> {
    let mut count = 0usize;
    for input in inputs {
        let mut input = input.clone();
        input.id = 0;
        if save(pool, &input).await.is_ok() {
            count += 1;
        }
    }
    Ok(count)
}

/// 删除素材，并重算所属分类计数。
pub async fn delete(pool: &MySqlPool, id: u32) -> Result<bool, ModelError> {
    let Some(item) = get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
    else {
        return Ok(false);
    };
    sqlx::query("DELETE FROM materials WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?;
    refresh_category_count(pool, item.category_id).await?;
    Ok(true)
}

async fn refresh_category_count(pool: &MySqlPool, category_id: u32) -> Result<(), ModelError> {
    if category_id == 0 {
        return Ok(());
    }
    let _ = sqlx::query(
        "UPDATE material_categories SET material_count = \
            (SELECT COUNT(*) FROM materials WHERE category_id = ?) WHERE id = ?",
    )
    .bind(category_id)
    .bind(category_id)
    .execute(pool)
    .await;
    Ok(())
}

// ---------------------------- 分类 -----------------------------------------

/// 全部素材分类。
pub async fn category_list(pool: &MySqlPool) -> anyhow::Result<Vec<MaterialCategory>> {
    Ok(
        sqlx::query_as::<_, MaterialCategory>(
            "SELECT * FROM material_categories ORDER BY id ASC",
        )
        .fetch_all(pool)
        .await?,
    )
}

/// 按 ID 取素材分类。
pub async fn category_get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<MaterialCategory>> {
    Ok(
        sqlx::query_as::<_, MaterialCategory>("SELECT * FROM material_categories WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 新建 / 更新素材分类（名称唯一）。
pub async fn category_save(
    pool: &MySqlPool,
    id: u32,
    title: &str,
) -> Result<MaterialCategory, ModelError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ModelError::invalid("请填写分类名称"));
    }
    let exists: Option<u32> =
        sqlx::query_scalar("SELECT id FROM material_categories WHERE title = ? LIMIT 1")
            .bind(title)
            .fetch_optional(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;
    if let Some(found) = exists {
        if found != id {
            return Err(ModelError::invalid("分类名称已存在"));
        }
    }

    let now = chrono::Utc::now().timestamp();
    let saved_id = if id > 0 {
        sqlx::query("UPDATE material_categories SET updated_time = ?, title = ? WHERE id = ?")
            .bind(now)
            .bind(title)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
        id
    } else {
        sqlx::query(
            "INSERT INTO material_categories (created_time, updated_time, title, material_count, \
                status) VALUES (?, ?, ?, 0, 1)",
        )
        .bind(now)
        .bind(now)
        .bind(title)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?
        .last_insert_id() as u32
    };

    category_get(pool, saved_id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("分类不存在"))
}

/// 删除素材分类（分类下还有素材时拒绝）。
pub async fn category_delete(pool: &MySqlPool, id: u32) -> Result<bool, ModelError> {
    if category_get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .is_none()
    {
        return Ok(false);
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM materials WHERE category_id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;
    if count > 0 {
        return Err(ModelError::invalid(format!(
            "该分类下还有 {count} 条素材，不能删除"
        )));
    }
    sqlx::query("DELETE FROM material_categories WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn cleans_tags_and_spaces() {
        assert_eq!(
            clean_tags_and_spaces("<p>你好   世界</p>\n<p>第二段</p>"),
            "你好 世界 第二段"
        );
        assert_eq!(clean_tags_and_spaces("  <b>a</b>  "), "a");
    }

    #[test]
    fn converts_utf8_text() {
        // BOM 会被剥掉
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"<p>hi</p>");
        assert_eq!(convert_text(&bytes, false).unwrap(), "<p>hi</p>");
        assert_eq!(convert_text(&bytes, true).unwrap(), "hi");

        // 非 UTF-8 明确报错
        let gbk = [0xD6, 0xD0, 0xCE, 0xC4];
        assert!(convert_text(&gbk, false).is_err());
    }

    #[tokio::test]
    async fn material_crud_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM materials WHERE title LIKE 'rustcms-mat%'")
            .execute(&pool)
            .await;

        // 名称必填
        assert_eq!(
            save(&pool, &MaterialInput::default())
                .await
                .unwrap_err()
                .to_string(),
            "请填写素材名称"
        );

        let created = save(
            &pool,
            &MaterialInput {
                title: "rustcms-mat".into(),
                category_id: 0,
                content: "<p>素材正文</p>".into(),
                status: Some(1),
                auto_update: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(created.id > 0);
        assert_eq!(created.status, 1);
        assert!(created.md5.as_deref().unwrap_or_default().len() == 32);

        // 列表 + 关键词
        let (total, rows) = list(&pool, 0, "rustcms-mat", 1, 20).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].id, created.id);

        // 更新
        let updated = save(
            &pool,
            &MaterialInput {
                id: created.id,
                title: "rustcms-mat（改）".into(),
                content: "<p>新正文</p>".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.title, "rustcms-mat（改）");
        assert_ne!(updated.md5, created.md5);

        // 不存在的 id
        assert!(save(
            &pool,
            &MaterialInput {
                id: 999_999,
                title: "x".into(),
                ..Default::default()
            }
        )
        .await
        .is_err());

        // 批量导入
        let count = save_many(
            &pool,
            &[
                MaterialInput {
                    title: "rustcms-mat-import-1".into(),
                    ..Default::default()
                },
                MaterialInput {
                    title: "".into(),
                    ..Default::default()
                },
            ],
        )
        .await
        .unwrap();
        assert_eq!(count, 1, "空标题那条应被跳过");

        assert!(delete(&pool, created.id).await.unwrap());
        assert!(get(&pool, created.id).await.unwrap().is_none());

        let _ = sqlx::query("DELETE FROM materials WHERE title LIKE 'rustcms-mat%'")
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    async fn category_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM material_categories WHERE title LIKE 'rustcms-matcat%'")
            .execute(&pool)
            .await;

        assert_eq!(
            category_save(&pool, 0, "").await.unwrap_err().to_string(),
            "请填写分类名称"
        );

        let created = category_save(&pool, 0, "rustcms-matcat").await.unwrap();
        assert!(created.id > 0);

        let err = category_save(&pool, 0, "rustcms-matcat").await.unwrap_err();
        assert_eq!(err.to_string(), "分类名称已存在");

        // 分类下有素材时不能删
        let material = save(
            &pool,
            &MaterialInput {
                title: "rustcms-mat-owner".into(),
                category_id: created.id,
                content: "x".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let err = category_delete(&pool, created.id).await.unwrap_err();
        assert!(err.to_string().contains("不能删除"), "{err}");

        // 删掉素材后可以删分类，且计数被重算
        delete(&pool, material.id).await.unwrap();
        let counted = category_get(&pool, created.id).await.unwrap().unwrap();
        assert_eq!(counted.material_count, 0);
        assert!(category_delete(&pool, created.id).await.unwrap());
        assert!(!category_delete(&pool, created.id).await.unwrap());
    }
}

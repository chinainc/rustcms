//! 采集配置与关键词（`settings.collector` + `keywords` 表）。
//!
//! 对齐 Go 版 `provider/collector.go`：
//! - 配置整份存在 `settings` 的 `collector` 键里（JSON）；
//! - `focus = true` 时整份覆盖，`false` 时只覆盖请求里出现的字段；
//! - 「内容替换」会真正对 `archive_data.content` / `archives.title` / `description`
//!   执行 SQL `REPLACE`（与 Go 的 `ReplaceArticles` 等价，但不做 HTML 解析）。

use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;

use super::{setting, ModelError};

/// 采集配置在 `settings` 里的键名。
pub const SETTING_KEY: &str = "collector";

/// 读采集配置；没有配置时返回空对象。
pub async fn get_setting(pool: &MySqlPool) -> anyhow::Result<Value> {
    Ok(setting::get_json(pool, SETTING_KEY)
        .await?
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({})))
}

/// 保存采集配置。
///
/// `focus = true`：整份替换（后台「保存配置」）；
/// `focus = false`：只覆盖请求里出现的字段（「保存替换词」这类局部更新）。
pub async fn save_setting(pool: &MySqlPool, value: &Value, focus: bool) -> anyhow::Result<Value> {
    if focus || !value.is_object() {
        setting::save_json(pool, SETTING_KEY, value).await?;
        return Ok(value.clone());
    }
    setting::merge_json(pool, SETTING_KEY, value).await
}

/// 一组「待替换词 → 目标词」。
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ReplaceKeyword {
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
}

/// 批量替换内容里的关键词，返回受影响行数合计。
///
/// 与 Go 的 `ReplaceArticles` 一样做「全表替换」，但不解析 HTML。
pub async fn replace_content(pool: &MySqlPool, pairs: &[ReplaceKeyword]) -> anyhow::Result<usize> {
    let pairs: Vec<&ReplaceKeyword> = pairs
        .iter()
        .filter(|pair| !pair.from.trim().is_empty())
        .collect();
    if pairs.is_empty() {
        return Ok(0);
    }

    let mut total = 0usize;
    for pair in pairs {
        // 正文
        let result = sqlx::query("UPDATE archive_data SET content = REPLACE(content, ?, ?)")
            .bind(&pair.from)
            .bind(&pair.to)
            .execute(pool)
            .await?;
        total += result.rows_affected() as usize;

        // 标题与描述
        let result = sqlx::query(
            "UPDATE archives SET title = REPLACE(title, ?, ?), \
                description = REPLACE(description, ?, ?)",
        )
        .bind(&pair.from)
        .bind(&pair.to)
        .bind(&pair.from)
        .bind(&pair.to)
        .execute(pool)
        .await?;
        total += result.rows_affected() as usize;
    }
    Ok(total)
}

/// 关键词。
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Keyword {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub title: String,
    pub status: u8,
    pub category_id: u32,
    pub level: i32,
    pub article_count: i32,
    pub has_dig: i8,
    pub last_time: i64,
}

/// 按标题取关键词。
pub async fn get_keyword_by_title(
    pool: &MySqlPool,
    title: &str,
) -> anyhow::Result<Option<Keyword>> {
    Ok(
        sqlx::query_as::<_, Keyword>("SELECT * FROM keywords WHERE title = ? LIMIT 1")
            .bind(title)
            .fetch_optional(pool)
            .await?,
    )
}

/// 按 ID 取关键词。
pub async fn get_keyword(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Keyword>> {
    Ok(sqlx::query_as::<_, Keyword>("SELECT * FROM keywords WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// 关键词不存在则创建，返回 ID。
pub async fn ensure_keyword(pool: &MySqlPool, title: &str) -> Result<u32, ModelError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ModelError::invalid("请填写关键词"));
    }
    if let Some(found) = get_keyword_by_title(pool, title).await? {
        return Ok(found.id);
    }
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT IGNORE INTO keywords (created_time, updated_time, title, status, category_id, \
            level, article_count, has_dig, last_time) VALUES (?, ?, ?, 1, 0, 0, 0, 0, 0)",
    )
    .bind(now)
    .bind(now)
    .bind(title)
    .execute(pool)
    .await?;
    get_keyword_by_title(pool, title)
        .await?
        .map(|item| item.id)
        .ok_or_else(|| ModelError::invalid("关键词创建失败"))
}

/// 关键词插件配置在 `settings` 里的键名。
pub const KEYWORD_SETTING: &str = "keyword";

/// 读关键词插件配置。
pub async fn keyword_setting_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = super::site_setting::get_section(pool, KEYWORD_SETTING).await?;
    if let Value::Object(map) = &mut value {
        map.entry("auto_dig").or_insert(json!(false));
        map.entry("language").or_insert(json!("zh"));
        map.entry("max_count").or_insert(json!(100000));
        map.entry("title_exclude").or_insert(json!([]));
        map.entry("title_replace").or_insert(json!([]));
    }
    Ok(value)
}

/// 保存关键词插件配置（字段级 merge）。
pub async fn keyword_setting_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    super::site_setting::save_section(pool, KEYWORD_SETTING, patch).await
}

/// 关键词分页列表，返回 `(总数, 本页记录)`。
pub async fn list_keywords(
    pool: &MySqlPool,
    keyword: &str,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Keyword>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;
    let keyword = keyword.trim();

    if keyword.is_empty() {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM keywords")
            .fetch_one(pool)
            .await?;
        let rows = sqlx::query_as::<_, Keyword>(
            "SELECT * FROM keywords ORDER BY id DESC LIMIT ? OFFSET ?",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        return Ok((total, rows));
    }

    let like = format!("%{keyword}%");
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM keywords WHERE title LIKE ?")
        .bind(&like)
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as::<_, Keyword>(
        "SELECT * FROM keywords WHERE title LIKE ? ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(&like)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok((total, rows))
}

/// 全部关键词（导出用）。
pub async fn all_keywords(pool: &MySqlPool) -> anyhow::Result<Vec<Keyword>> {
    Ok(
        sqlx::query_as::<_, Keyword>("SELECT * FROM keywords ORDER BY id ASC")
            .fetch_all(pool)
            .await?,
    )
}

/// 更新单条关键词（标题重复时报错）。
pub async fn update_keyword(
    pool: &MySqlPool,
    id: u32,
    title: &str,
    category_id: u32,
) -> Result<Keyword, ModelError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ModelError::invalid("请填写关键词"));
    }
    if get_keyword(pool, id).await?.is_none() {
        return Err(ModelError::invalid("关键词不存在"));
    }
    if let Some(exists) = get_keyword_by_title(pool, title).await? {
        if exists.id != id {
            return Err(ModelError::invalid(format!("关键词 {title} 已存在")));
        }
    }

    sqlx::query("UPDATE keywords SET title = ?, category_id = ?, updated_time = ? WHERE id = ?")
        .bind(title)
        .bind(category_id)
        .bind(chrono::Utc::now().timestamp())
        .bind(id)
        .execute(pool)
        .await?;

    get_keyword(pool, id)
        .await?
        .ok_or_else(|| ModelError::invalid("关键词不存在"))
}

/// 批量新增关键词（按换行分隔，自动去重），返回新增成功的记录。
pub async fn save_keywords_batch(
    pool: &MySqlPool,
    text: &str,
    category_id: u32,
) -> Result<Vec<Keyword>, ModelError> {
    let mut saved = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in text.split('\n') {
        let title = line.trim();
        if title.is_empty() || !seen.insert(title.to_string()) {
            continue;
        }
        if get_keyword_by_title(pool, title).await?.is_some() {
            continue;
        }
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT IGNORE INTO keywords (created_time, updated_time, title, status, category_id, \
                level, article_count, has_dig, last_time) VALUES (?, ?, ?, 1, ?, 0, 0, 0, 0)",
        )
        .bind(now)
        .bind(now)
        .bind(title)
        .bind(category_id)
        .execute(pool)
        .await?;
        if let Some(item) = get_keyword_by_title(pool, title).await? {
            saved.push(item);
        }
    }
    Ok(saved)
}

/// 删除单条关键词，返回是否真的删掉。
pub async fn delete_keyword(pool: &MySqlPool, id: u32) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM keywords WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// 批量删除关键词，返回删除条数。
pub async fn delete_keywords(pool: &MySqlPool, ids: &[u32]) -> anyhow::Result<u64> {
    let mut deleted = 0u64;
    for id in ids {
        deleted += sqlx::query("DELETE FROM keywords WHERE id = ?")
            .bind(*id)
            .execute(pool)
            .await?
            .rows_affected();
    }
    Ok(deleted)
}

/// 清空全部关键词，返回删除条数。
pub async fn delete_all_keywords(pool: &MySqlPool) -> anyhow::Result<u64> {
    Ok(sqlx::query("DELETE FROM keywords WHERE id > 0")
        .execute(pool)
        .await?
        .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn deserializes_replace_pairs() {
        let pairs: Vec<ReplaceKeyword> =
            serde_json::from_str(r#"[{"from":"a","to":"b"},{"from":"","to":"x"}]"#).unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].from, "a");
        assert!(pairs[1].from.is_empty());
    }

    #[tokio::test]
    async fn setting_roundtrip_and_restore() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, SETTING_KEY).await.unwrap();

        let saved = save_setting(&pool, &json!({"channels": 3, "save_type": 1}), true)
            .await
            .unwrap();
        assert_eq!(saved["channels"], json!(3));

        // 局部更新：只改 save_type，channels 保留
        let merged = save_setting(&pool, &json!({"save_type": 0}), false)
            .await
            .unwrap();
        assert_eq!(merged["save_type"], json!(0));
        assert_eq!(merged["channels"], json!(3));

        match backup {
            Some(raw) => setting::save_raw(&pool, SETTING_KEY, &raw).await.unwrap(),
            None => setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
    }

    #[tokio::test]
    async fn ensure_keyword_is_idempotent() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let title = "rustcms-测试关键词";
        let first = ensure_keyword(&pool, title).await.unwrap();
        let second = ensure_keyword(&pool, title).await.unwrap();
        assert_eq!(first, second);

        let _ = sqlx::query("DELETE FROM keywords WHERE title = ?")
            .bind(title)
            .execute(&pool)
            .await;
    }
}

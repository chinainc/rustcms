//! 锚文本（表 `anchors`，`title` 唯一）。
//!
//! 对齐 Go 版 `controller/manageController/pluginAnchor.go`：
//! - 列表支持关键词搜索；
//! - `title` 唯一，重复时报「锚文本关键词已存在」；
//! - 保存时把 `link` 的前台域名前缀去掉，只存相对路径；
//! - 配置存 `settings.anchor`（`anchor_density` / `replace_way` / `keyword_way` / `no_strong_tag`），
//!   `anchor_density < 10` 时归一到 100；
//! - 批量替换：把正文里**未被链接**的锚文本包成 `<a>`，密度控制每篇最多几处。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::{category, module, replace, site_setting, url, ModelError};

/// 锚文本配置在 `settings` 里的键名。
pub const SETTING_KEY: &str = "anchor";

/// 锚文本。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Anchor {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub title: String,
    /// 关联内容 ID（从标题导入时写入，普通锚文本为 0）
    pub archive_id: i64,
    pub link: String,
    pub weight: i32,
    pub replace_count: i32,
    pub status: u8,
}

/// 保存入参。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AnchorInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub weight: i32,
}

/// 读锚文本配置（补默认值）。
pub async fn setting_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, SETTING_KEY).await?;
    if let Value::Object(map) = &mut value {
        let density = map
            .get("anchor_density")
            .and_then(Value::as_i64)
            .unwrap_or(100);
        map.insert(
            "anchor_density".into(),
            json!(if density < 10 { 100 } else { density }),
        );
        map.entry("replace_way").or_insert(json!(0));
        map.entry("keyword_way").or_insert(json!(0));
        map.entry("no_strong_tag").or_insert(json!(0));
    }
    Ok(value)
}

/// 保存锚文本配置（`anchor_density < 10` 归一为 100）。
pub async fn setting_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    let mut patch = patch.clone();
    if let Value::Object(map) = &mut patch {
        if let Some(density) = map.get("anchor_density").and_then(Value::as_i64) {
            map.insert(
                "anchor_density".into(),
                json!(if density < 10 { 100 } else { density }),
            );
        }
    }
    site_setting::save_section(pool, SETTING_KEY, &patch).await
}

fn anchor_density(config: &Value) -> i64 {
    config
        .get("anchor_density")
        .and_then(Value::as_i64)
        .filter(|value| *value >= 10)
        .unwrap_or(100)
}

/// 按 ID 取锚文本。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Anchor>> {
    Ok(
        sqlx::query_as::<_, Anchor>("SELECT * FROM anchors WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 按标题取锚文本。
pub async fn get_by_title(pool: &MySqlPool, title: &str) -> anyhow::Result<Option<Anchor>> {
    Ok(
        sqlx::query_as::<_, Anchor>("SELECT * FROM anchors WHERE title = ? LIMIT 1")
            .bind(title.trim())
            .fetch_optional(pool)
            .await?,
    )
}

/// 分页查锚文本，返回 `(总数, 本页记录)`。
pub async fn list(
    pool: &MySqlPool,
    keyword: &str,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Anchor>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;
    let keyword = keyword.trim();

    if keyword.is_empty() {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM anchors")
            .fetch_one(pool)
            .await?;
        let rows = sqlx::query_as::<_, Anchor>(
            "SELECT * FROM anchors ORDER BY id DESC LIMIT ? OFFSET ?",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        return Ok((total, rows));
    }

    let like = format!("%{keyword}%");
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM anchors WHERE title LIKE ? OR `link` LIKE ?",
    )
    .bind(&like)
    .bind(&like)
    .fetch_one(pool)
    .await?;
    let rows = sqlx::query_as::<_, Anchor>(
        "SELECT * FROM anchors WHERE title LIKE ? OR `link` LIKE ? ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(&like)
    .bind(&like)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok((total, rows))
}

/// 全部锚文本（导出用）。
pub async fn all(pool: &MySqlPool) -> anyhow::Result<Vec<Anchor>> {
    Ok(
        sqlx::query_as::<_, Anchor>("SELECT * FROM anchors ORDER BY id ASC")
            .fetch_all(pool)
            .await?,
    )
}

/// 新建 / 更新锚文本。
pub async fn save(pool: &MySqlPool, input: &AnchorInput, front_url: &str) -> Result<Anchor, ModelError> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err(ModelError::invalid("请填写锚文本名称"));
    }
    let mut link = input.link.trim().to_string();
    let front_url = front_url.trim_end_matches('/');
    if !front_url.is_empty() {
        if let Some(stripped) = link.strip_prefix(front_url) {
            link = if stripped.starts_with('/') {
                stripped.to_string()
            } else {
                format!("/{stripped}")
            };
        }
    }
    if link.is_empty() {
        return Err(ModelError::invalid("请填写锚文本链接"));
    }

    let now = chrono::Utc::now().timestamp();
    if input.id > 0 {
        if get(pool, input.id).await?.is_none() {
            return Err(ModelError::invalid("锚文本不存在"));
        }
        if let Some(exists) = get_by_title(pool, title).await? {
            if exists.id != input.id {
                return Err(ModelError::invalid(format!("锚文本关键词 {title} 已存在")));
            }
        }
        sqlx::query(
            "UPDATE anchors SET updated_time = ?, title = ?, `link` = ?, weight = ?, status = 1 \
             WHERE id = ?",
        )
        .bind(now)
        .bind(title)
        .bind(&link)
        .bind(input.weight)
        .bind(input.id)
        .execute(pool)
        .await?;
        return get(pool, input.id)
            .await?
            .ok_or_else(|| ModelError::invalid("锚文本不存在"));
    }

    // 未指定 id：标题已存在就更新它
    if let Some(exists) = get_by_title(pool, title).await? {
        sqlx::query(
            "UPDATE anchors SET updated_time = ?, `link` = ?, weight = ?, status = 1 WHERE id = ?",
        )
        .bind(now)
        .bind(&link)
        .bind(input.weight)
        .bind(exists.id)
        .execute(pool)
        .await?;
        return get(pool, exists.id)
            .await?
            .ok_or_else(|| ModelError::invalid("锚文本不存在"));
    }

    let result = sqlx::query(
        "INSERT INTO anchors (created_time, updated_time, title, archive_id, `link`, weight, \
            replace_count, status) VALUES (?, ?, ?, 0, ?, ?, 0, 1)",
    )
    .bind(now)
    .bind(now)
    .bind(title)
    .bind(&link)
    .bind(input.weight)
    .execute(pool)
    .await?;
    let id = result.last_insert_id() as u32;
    get(pool, id)
        .await?
        .ok_or_else(|| ModelError::invalid("锚文本创建失败"))
}

/// 删除单条锚文本。
pub async fn delete(pool: &MySqlPool, id: u32) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM anchors WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// 批量删除锚文本，返回删除条数。
pub async fn delete_many(pool: &MySqlPool, ids: &[u32]) -> anyhow::Result<u64> {
    let mut deleted = 0u64;
    for id in ids {
        deleted += sqlx::query("DELETE FROM anchors WHERE id = ?")
            .bind(*id)
            .execute(pool)
            .await?
            .rows_affected();
    }
    Ok(deleted)
}

/// 导入：一行 `title, link[, weight]`，返回新增条数。
pub async fn import(pool: &MySqlPool, text: &str, front_url: &str) -> Result<usize, ModelError> {
    let mut imported = 0usize;
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split(',');
        let title = parts.next().unwrap_or_default().trim();
        let link = parts.next().unwrap_or_default().trim();
        let weight = parts
            .next()
            .map(str::trim)
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        if title.is_empty() || link.is_empty() {
            continue;
        }
        match save(
            pool,
            &AnchorInput {
                id: 0,
                title: title.to_string(),
                link: link.to_string(),
                weight,
            },
            front_url,
        )
        .await
        {
            Ok(_) => imported += 1,
            // 单行失败不影响整体
            Err(_) => continue,
        }
    }
    Ok(imported)
}

/// 从分类 / 内容 / 标签的标题批量生成锚文本，返回新增条数。
pub async fn add_from_titles(
    pool: &MySqlPool,
    kind: &str,
    ids: &[u32],
    front_url: &str,
) -> Result<usize, ModelError> {
    if ids.is_empty() {
        return Err(ModelError::invalid("请选择要导入的数据"));
    }

    let mut candidates: Vec<(String, String, i64)> = Vec::new();
    match kind {
        "category" => {
            let list = category::list_all(pool)
                .await
                .map_err(|err| ModelError::invalid(format!("查询分类失败: {err}")))?;
            for item in list {
                if ids.contains(&item.id) && !item.title.trim().is_empty() {
                    candidates.push((
                        item.title.clone(),
                        format!("/{}/", item.url_token),
                        0,
                    ));
                }
            }
        }
        "archive" => {
            let mode = url::load_mode(pool).await.unwrap_or(0);
            let modules = module::list(pool)
                .await
                .map_err(|err| ModelError::invalid(format!("查询模型失败: {err}")))?;
            let categories = category::list_all(pool)
                .await
                .map_err(|err| ModelError::invalid(format!("查询分类失败: {err}")))?;
            let module_tokens: std::collections::HashMap<u32, String> = modules
                .iter()
                .map(|item| (item.id, item.url_token.clone()))
                .collect();

            for id in ids {
                let row: Option<(i64, String, String, u32)> = sqlx::query_as(
                    "SELECT id, title, url_token, category_id FROM archives WHERE id = ?",
                )
                .bind(*id)
                .fetch_optional(pool)
                .await
                .map_err(|err| ModelError::invalid(format!("查询内容失败: {err}")))?;
                let Some((archive_id, title, token, category_id)) = row else {
                    continue;
                };
                let Some(category) = categories.iter().find(|item| item.id == category_id) else {
                    continue;
                };
                let module_token = module_tokens
                    .get(&category.module_id)
                    .cloned()
                    .unwrap_or_default();
                let link = url::archive_url(
                    mode,
                    &category.url_token,
                    &module_token,
                    archive_id,
                    &token,
                    category_id,
                    1,
                );
                candidates.push((title, link, archive_id));
            }
        }
        "tag" => {
            for id in ids {
                let row: Option<(String, String)> =
                    sqlx::query_as("SELECT title, url_token FROM tags WHERE id = ?")
                        .bind(*id)
                        .fetch_optional(pool)
                        .await
                        .map_err(|err| ModelError::invalid(format!("查询标签失败: {err}")))?;
                if let Some((title, token)) = row {
                    candidates.push((title, format!("/tag/{token}"), 0));
                }
            }
        }
        _ => return Err(ModelError::invalid("导入类型不正确")),
    }

    let mut inserted = 0usize;
    for (title, link, archive_id) in candidates {
        let saved = save(
            pool,
            &AnchorInput {
                id: 0,
                title: title.clone(),
                link,
                weight: 0,
            },
            front_url,
        )
        .await;
        if saved.is_err() {
            continue;
        }
        if archive_id > 0 {
            let _ = sqlx::query("UPDATE anchors SET archive_id = ? WHERE title = ?")
                .bind(archive_id)
                .bind(&title)
                .execute(pool)
                .await;
        }
        inserted += 1;
    }
    Ok(inserted)
}

/// 批量替换锚文本：`anchor = None` 时处理全部启用的锚文本，返回受影响的文章数。
///
/// 密度：每篇文章最多 `内容字数 / anchor_density` 处（至少 1 处）。
pub async fn replace_anchor(pool: &MySqlPool, anchor: Option<&Anchor>) -> anyhow::Result<usize> {
    let config = setting_get(pool).await?;
    let density = anchor_density(&config);

    let anchors: Vec<Anchor> = match anchor {
        Some(item) => vec![item.clone()],
        None => {
            sqlx::query_as::<_, Anchor>("SELECT * FROM anchors WHERE status = 1")
                .fetch_all(pool)
                .await?
        }
    };

    let mut touched = 0usize;
    for item in anchors {
        if item.title.trim().is_empty() || item.link.trim().is_empty() {
            continue;
        }
        let like = format!("%{}%", item.title);
        // `archive_data.id` 是 `bigint unsigned`，CAST 成字符串再解析，避免解码失败
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT CAST(id AS CHAR), content FROM archive_data WHERE content LIKE ? \
             AND content NOT LIKE ? LIMIT 5000",
        )
        .bind(&like)
        .bind(format!("%href=\"{}\"%", item.link))
        .fetch_all(pool)
        .await?;

        let mut replaced_total = 0usize;
        for (raw_id, content) in rows {
            let Ok(id) = raw_id.parse::<i64>() else {
                continue;
            };
            let Some(content) = content else { continue };
            let chars = content.chars().count() as i64;
            let max = std::cmp::max(1, chars / density) as usize;
            let (updated, count) = replace::linkify(&content, &item.title, &item.link, max);
            if count == 0 {
                continue;
            }
            sqlx::query("UPDATE archive_data SET content = ? WHERE id = ?")
                .bind(&updated)
                .bind(id)
                .execute(pool)
                .await?;
            replaced_total += count;
            touched += 1;
        }

        if replaced_total > 0 {
            sqlx::query("UPDATE anchors SET replace_count = ? WHERE id = ?")
                .bind(replaced_total as i32)
                .bind(item.id)
                .execute(pool)
                .await?;
        }
    }
    Ok(touched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[tokio::test]
    async fn anchor_crud_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM anchors WHERE title LIKE 'rustcms-anchor%'")
            .execute(&pool)
            .await;

        // 标题必填
        assert_eq!(
            save(&pool, &AnchorInput::default(), "")
                .await
                .unwrap_err()
                .to_string(),
            "请填写锚文本名称"
        );

        let created = save(
            &pool,
            &AnchorInput {
                title: "rustcms-anchor-a".into(),
                link: "https://example.com/about".into(),
                weight: 5,
                ..Default::default()
            },
            "https://example.com",
        )
        .await
        .unwrap();
        assert!(created.id > 0);
        // 前台域名前缀被剥掉
        assert_eq!(created.link, "/about");
        assert_eq!(created.weight, 5);

        // 同标题再保存 → 复用
        let again = save(
            &pool,
            &AnchorInput {
                title: "rustcms-anchor-a".into(),
                link: "/other".into(),
                weight: 1,
                ..Default::default()
            },
            "",
        )
        .await
        .unwrap();
        assert_eq!(again.id, created.id);
        assert_eq!(again.link, "/other");

        // 指定 id 改成别人的标题 → 报错
        let second = save(
            &pool,
            &AnchorInput {
                title: "rustcms-anchor-b".into(),
                link: "/b".into(),
                weight: 0,
                ..Default::default()
            },
            "",
        )
        .await
        .unwrap();
        let err = save(
            &pool,
            &AnchorInput {
                id: second.id,
                title: "rustcms-anchor-a".into(),
                link: "/c".into(),
                weight: 0,
            },
            "",
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("已存在"), "{err}");

        // 列表搜索
        let (total, _) = list(&pool, "rustcms-anchor", 1, 20).await.unwrap();
        assert_eq!(total, 2);

        // 批量删除
        let deleted = delete_many(&pool, &[created.id, second.id]).await.unwrap();
        assert_eq!(deleted, 2);
    }

    #[tokio::test]
    async fn anchor_setting_normalizes_density() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = crate::models::setting::get_raw(&pool, SETTING_KEY).await.unwrap();

        let saved = setting_save(&pool, &json!({"anchor_density": 3, "replace_way": 1}))
            .await
            .unwrap();
        assert_eq!(saved["anchor_density"], json!(100));

        let loaded = setting_get(&pool).await.unwrap();
        assert_eq!(loaded["anchor_density"], json!(100));
        assert_eq!(loaded["replace_way"], json!(1));
        assert!(loaded.get("no_strong_tag").is_some());

        match backup {
            Some(raw) => crate::models::setting::save_raw(&pool, SETTING_KEY, &raw)
                .await
                .unwrap(),
            None => crate::models::setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
    }

    #[tokio::test]
    async fn imports_anchor_lines() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM anchors WHERE title LIKE 'rustcms-imp-anchor%'")
            .execute(&pool)
            .await;

        let text = "rustcms-imp-anchor-1, /a, 3\nrustcms-imp-anchor-2, /b\n\n# 注释\n坏行";
        let count = import(&pool, text, "").await.unwrap();
        assert_eq!(count, 2);

        let anchor = get_by_title(&pool, "rustcms-imp-anchor-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(anchor.link, "/a");
        assert_eq!(anchor.weight, 3);

        let _ = sqlx::query("DELETE FROM anchors WHERE title LIKE 'rustcms-imp-anchor%'")
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    async fn replace_anchor_links_content_once() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };

        let category_id: u32 =
            sqlx::query_scalar("SELECT id FROM categories WHERE `type` = 1 ORDER BY id LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        let module_id: u32 = sqlx::query_scalar("SELECT id FROM modules ORDER BY id LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp();
        let title = format!("rustcms-anchor-content-{now}");
        let id = sqlx::query(
            "INSERT INTO archives (created_time, updated_time, title, module_id, category_id, \
                url_token) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(now)
        .bind(now)
        .bind(&title)
        .bind(module_id)
        .bind(category_id)
        .bind(format!("rustcms-anchor-content-{now}"))
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_id() as i64;

        let body = "<p>rustcms-anchor-link 第一次，rustcms-anchor-link 第二次</p>";
        sqlx::query("INSERT INTO archive_data (id, content) VALUES (?, ?)")
            .bind(id)
            .bind(body)
            .execute(&pool)
            .await
            .unwrap();

        let anchor = save(
            &pool,
            &AnchorInput {
                title: "rustcms-anchor-link".into(),
                link: "/target".into(),
                weight: 0,
                ..Default::default()
            },
            "",
        )
        .await
        .unwrap();

        // 第一次：应处理这篇内容
        let touched = replace_anchor(&pool, Some(&anchor)).await.unwrap();
        assert_eq!(touched, 1);

        let linked: String =
            sqlx::query_scalar("SELECT content FROM archive_data WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            linked.contains(r#"<a href="/target" title="rustcms-anchor-link">rustcms-anchor-link</a>"#),
            "{linked}"
        );

        // 第二次：已经有该链接，不应再处理
        let again = replace_anchor(&pool, Some(&anchor)).await.unwrap();
        assert_eq!(again, 0);

        // 收尾
        let _ = sqlx::query("DELETE FROM archive_data WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM archives WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM anchors WHERE id = ?")
            .bind(anchor.id)
            .execute(&pool)
            .await;
    }
}

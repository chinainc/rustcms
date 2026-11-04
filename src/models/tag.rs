//! 文档标签（`tags` + `tag_contents` + `tag_data`）。
//!
//! 对齐 Go 版 `controller/manageController/pluginTag.go`：
//! - 列表支持标题模糊搜索与分类筛选；
//! - `partial = true` 走 PATCH 语义（只覆盖显式传入的字段），否则全量覆盖；
//! - 正文与自定义字段存在 `tag_contents`（`id` 就是 tag id，一对一）；
//! - 标签字段定义存 `settings.tag_fields`（JSON 数组）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::{setting, site_setting, ModelError};

/// 标签字段定义在 `settings` 里的键名。
pub const TAG_FIELDS: &str = "tag_fields";

/// 标签（只映射表字段）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Tag {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub title: String,
    pub category_id: u32,
    pub seo_title: String,
    pub keywords: String,
    pub url_token: String,
    pub description: String,
    pub first_letter: String,
    pub template: String,
    pub logo: String,
    pub status: u8,
}

/// 保存入参（对应 Go 的 `request.PluginTag`）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TagInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub category_id: Option<u32>,
    #[serde(default)]
    pub url_token: Option<String>,
    #[serde(default)]
    pub seo_title: Option<String>,
    #[serde(default)]
    pub keywords: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub first_letter: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub status: Option<u8>,
    #[serde(default)]
    pub extra: Option<Value>,
    #[serde(default)]
    pub partial: bool,
}

fn apply_str(target: &mut String, input: Option<&str>, update_all: bool) {
    if let Some(value) = input {
        if update_all || !value.is_empty() {
            *target = value.to_string();
        }
    }
}

/// URL 别名清洗：小写，字母数字与 `-`/`_` 保留，其余折叠成 `-`。
fn sanitize_token(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// 首字母：ASCII 字母转大写，其余返回空串。
fn first_letter_of(title: &str) -> String {
    title
        .chars()
        .next()
        .filter(|ch| ch.is_ascii_alphabetic())
        .map(|ch| ch.to_ascii_uppercase().to_string())
        .unwrap_or_default()
}

/// 确保别名唯一（冲突时追加 `-1`、`-2`…）。
async fn unique_token(
    pool: &MySqlPool,
    token: &str,
    exclude_id: u32,
) -> anyhow::Result<String> {
    let token: String = token.chars().take(150).collect();
    let token = if token.is_empty() {
        format!("tag-{}", chrono::Utc::now().timestamp_millis())
    } else {
        token
    };
    let mut index = 0u32;
    loop {
        let candidate = if index == 0 {
            token.clone()
        } else {
            format!("{token}-{index}")
        };
        match get_by_token(pool, &candidate).await? {
            Some(found) if found.id != exclude_id => index += 1,
            _ => return Ok(candidate),
        }
    }
}

/// 按 ID 取标签。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Tag>> {
    Ok(sqlx::query_as::<_, Tag>("SELECT * FROM tags WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// 按别名取标签。
pub async fn get_by_token(pool: &MySqlPool, token: &str) -> anyhow::Result<Option<Tag>> {
    Ok(
        sqlx::query_as::<_, Tag>("SELECT * FROM tags WHERE url_token = ? LIMIT 1")
            .bind(token)
            .fetch_optional(pool)
            .await?,
    )
}

/// 分页查标签，返回 `(总数, 本页记录)`。
pub async fn list(
    pool: &MySqlPool,
    title: &str,
    category_id: u32,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Tag>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;
    let title = title.trim();

    let mut where_sql = String::from(" WHERE 1 = 1");
    if !title.is_empty() {
        where_sql.push_str(" AND title LIKE ?");
    }
    if category_id > 0 {
        where_sql.push_str(" AND category_id = ?");
    }

    let count_sql = format!("SELECT COUNT(*) FROM tags{where_sql}");
    let mut counter = sqlx::query_scalar::<_, i64>(&count_sql);
    if !title.is_empty() {
        counter = counter.bind(format!("%{title}%"));
    }
    if category_id > 0 {
        counter = counter.bind(category_id);
    }
    let total = counter.fetch_one(pool).await?;

    let list_sql = format!("SELECT * FROM tags{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?");
    let mut query = sqlx::query_as::<_, Tag>(&list_sql);
    if !title.is_empty() {
        query = query.bind(format!("%{title}%"));
    }
    if category_id > 0 {
        query = query.bind(category_id);
    }
    let rows = query.bind(page_size).bind(offset).fetch_all(pool).await?;
    Ok((total, rows))
}

/// 读标签正文与自定义字段：`(content, extra)`。
pub async fn content(pool: &MySqlPool, tag_id: u32) -> anyhow::Result<(String, Option<Value>)> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT content, extra FROM tag_contents WHERE id = ?")
            .bind(tag_id)
            .fetch_optional(pool)
            .await?;
    let Some((content, extra)) = row else {
        return Ok((String::new(), None));
    };
    let extra = extra
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    Ok((content.unwrap_or_default(), extra))
}

async fn save_content(
    pool: &MySqlPool,
    tag_id: u32,
    content: Option<&str>,
    extra: Option<&Value>,
) -> anyhow::Result<()> {
    if content.is_none() && extra.is_none() {
        return Ok(());
    }
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tag_contents WHERE id = ?")
        .bind(tag_id)
        .fetch_one(pool)
        .await?;

    if exists == 0 {
        sqlx::query("INSERT INTO tag_contents (id, content, extra) VALUES (?, ?, ?)")
            .bind(tag_id)
            .bind(content.unwrap_or_default())
            .bind(extra.map(|value| value.to_string()))
            .execute(pool)
            .await?;
        return Ok(());
    }

    if let Some(content) = content {
        sqlx::query("UPDATE tag_contents SET content = ? WHERE id = ?")
            .bind(content)
            .bind(tag_id)
            .execute(pool)
            .await?;
    }
    if let Some(extra) = extra {
        sqlx::query("UPDATE tag_contents SET extra = ? WHERE id = ?")
            .bind(extra.to_string())
            .bind(tag_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// 新建 / 更新标签。
pub async fn save(pool: &MySqlPool, input: &TagInput) -> Result<Tag, ModelError> {
    let update_all = !input.partial;
    let is_new = input.id == 0;
    let mut tag = if is_new {
        Tag {
            id: 0,
            created_time: None,
            updated_time: None,
            title: String::new(),
            category_id: 0,
            seo_title: String::new(),
            keywords: String::new(),
            url_token: String::new(),
            description: String::new(),
            first_letter: String::new(),
            template: String::new(),
            logo: String::new(),
            status: 1,
        }
    } else {
        get(pool, input.id)
            .await?
            .ok_or_else(|| ModelError::invalid("标签不存在"))?
    };

    apply_str(&mut tag.title, input.title.as_deref(), update_all);
    apply_str(&mut tag.seo_title, input.seo_title.as_deref(), update_all);
    apply_str(&mut tag.keywords, input.keywords.as_deref(), update_all);
    apply_str(
        &mut tag.description,
        input.description.as_deref(),
        update_all,
    );
    apply_str(&mut tag.template, input.template.as_deref(), update_all);
    apply_str(&mut tag.logo, input.logo.as_deref(), update_all);
    apply_str(
        &mut tag.first_letter,
        input.first_letter.as_deref(),
        update_all,
    );
    if let Some(value) = input.category_id {
        if update_all || value > 0 {
            tag.category_id = value;
        }
    }
    if let Some(value) = input.status {
        if update_all || value > 0 {
            tag.status = value;
        }
    }

    if tag.title.trim().is_empty() {
        return Err(ModelError::invalid("请填写标签名称"));
    }
    tag.title = tag.title.trim().to_string();

    // 别名
    let raw_token = input
        .url_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| sanitize_token(value))
        .unwrap_or_else(|| sanitize_token(&tag.title));
    let need_token = update_all || input.url_token.is_some() || tag.url_token.is_empty();
    if need_token {
        let token = if raw_token.is_empty() {
            format!("tag-{}", chrono::Utc::now().timestamp_millis())
        } else {
            raw_token
        };
        tag.url_token = unique_token(pool, &token, tag.id).await?;
    }

    // 首字母
    if tag.first_letter.trim().is_empty() {
        tag.first_letter = first_letter_of(&tag.title);
    }

    let now = chrono::Utc::now().timestamp();
    tag.updated_time = Some(now);
    if is_new {
        tag.created_time = Some(now);
        let result = sqlx::query(
            "INSERT INTO tags (created_time, updated_time, title, category_id, seo_title, keywords, \
                url_token, description, first_letter, template, logo, status) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(tag.created_time)
        .bind(tag.updated_time)
        .bind(&tag.title)
        .bind(tag.category_id)
        .bind(&tag.seo_title)
        .bind(&tag.keywords)
        .bind(&tag.url_token)
        .bind(&tag.description)
        .bind(&tag.first_letter)
        .bind(&tag.template)
        .bind(&tag.logo)
        .bind(tag.status)
        .execute(pool)
        .await?;
        tag.id = result.last_insert_id() as u32;
    } else {
        sqlx::query(
            "UPDATE tags SET updated_time = ?, title = ?, category_id = ?, seo_title = ?, \
                keywords = ?, url_token = ?, description = ?, first_letter = ?, template = ?, \
                logo = ?, status = ? WHERE id = ?",
        )
        .bind(tag.updated_time)
        .bind(&tag.title)
        .bind(tag.category_id)
        .bind(&tag.seo_title)
        .bind(&tag.keywords)
        .bind(&tag.url_token)
        .bind(&tag.description)
        .bind(&tag.first_letter)
        .bind(&tag.template)
        .bind(&tag.logo)
        .bind(tag.status)
        .bind(tag.id)
        .execute(pool)
        .await?;
    }

    save_content(
        pool,
        tag.id,
        input.content.as_deref(),
        input.extra.as_ref(),
    )
    .await?;
    Ok(tag)
}

/// 删除标签：`tag_contents` / `tag_data` 一起清掉。
pub async fn delete(pool: &MySqlPool, id: u32) -> Result<bool, ModelError> {
    if get(pool, id).await?.is_none() {
        return Ok(false);
    }
    sqlx::query("DELETE FROM tag_contents WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM tag_data WHERE tag_id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM tags WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(true)
}

/// 读标签字段定义（JSON 数组）。
pub async fn fields(pool: &MySqlPool) -> anyhow::Result<Value> {
    Ok(setting::get_json(pool, TAG_FIELDS)
        .await?
        .filter(Value::is_array)
        .unwrap_or_else(|| json!([])))
}

/// 保存标签字段定义（整份覆盖，必须是数组）。
pub async fn save_fields(pool: &MySqlPool, value: &Value) -> Result<(), ModelError> {
    if !value.is_array() {
        return Err(ModelError::invalid("自定义字段必须是数组"));
    }
    setting::save_json(pool, TAG_FIELDS, value).await?;
    Ok(())
}

/// 标签栏目（分类）标题，供列表展示。
pub async fn category_titles(pool: &MySqlPool) -> anyhow::Result<std::collections::HashMap<u32, String>> {
    let rows: Vec<(u32, String)> = sqlx::query_as("SELECT id, title FROM categories")
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().collect())
}

/// 供测试/首次访问确保 `settings.tag_fields` 存在。
pub async fn ensure_fields(pool: &MySqlPool) -> anyhow::Result<()> {
    if setting::get_raw(pool, TAG_FIELDS).await?.is_none() {
        setting::save_json(pool, TAG_FIELDS, &json!([])).await?;
    }
    let _ = site_setting::get_section(pool, TAG_FIELDS).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn sanitizes_and_derives() {
        assert_eq!(sanitize_token("Hello World!"), "hello-world");
        assert_eq!(sanitize_token("  中文  "), "");
        assert_eq!(first_letter_of("apple"), "A");
        assert_eq!(first_letter_of("中文"), "");
    }

    #[tokio::test]
    async fn tag_crud_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM tags WHERE title LIKE 'rustcms-tag%'")
            .execute(&pool)
            .await;

        // 标题必填
        assert_eq!(
            save(&pool, &TagInput::default())
                .await
                .unwrap_err()
                .to_string(),
            "请填写标签名称"
        );

        let created = save(
            &pool,
            &TagInput {
                title: Some("rustcms-tag-alpha".into()),
                content: Some("标签正文".into()),
                extra: Some(json!({"author": "tester"})),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(created.id > 0);
        assert_eq!(created.url_token, "rustcms-tag-alpha");
        assert_eq!(created.first_letter, "R");

        // 正文与自定义字段
        let (content, extra) = content(&pool, created.id).await.unwrap();
        assert_eq!(content, "标签正文");
        assert_eq!(extra.unwrap()["author"], json!("tester"));

        // 别名重复自动加后缀
        let second = save(
            &pool,
            &TagInput {
                title: Some("rustcms-tag-beta".into()),
                url_token: Some("rustcms-tag-alpha".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(second.url_token, "rustcms-tag-alpha-1");

        // PATCH：只改 SEO 标题，标题不动
        let patched = save(
            &pool,
            &TagInput {
                id: created.id,
                seo_title: Some("SEO 改".into()),
                partial: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(patched.seo_title, "SEO 改");
        assert_eq!(patched.title, "rustcms-tag-alpha");

        // 列表 + 搜索
        let (total, rows) = list(&pool, "rustcms-tag", 0, 1, 20).await.unwrap();
        assert_eq!(total, 2);
        assert_eq!(rows.len(), 2);

        // 删除（连带 tag_contents）
        assert!(delete(&pool, created.id).await.unwrap());
        assert!(get(&pool, created.id).await.unwrap().is_none());
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tag_contents WHERE id = ?")
            .bind(created.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0);

        let _ = sqlx::query("DELETE FROM tags WHERE title LIKE 'rustcms-tag%'")
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    async fn tag_fields_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, TAG_FIELDS).await.unwrap();

        assert!(save_fields(&pool, &json!({"x": 1})).await.is_err());
        save_fields(
            &pool,
            &json!([{"field_name": "author", "name": "作者", "type": "text"}]),
        )
        .await
        .unwrap();
        let loaded = fields(&pool).await.unwrap();
        assert_eq!(loaded.as_array().unwrap().len(), 1);

        match backup {
            Some(raw) => setting::save_raw(&pool, TAG_FIELDS, &raw).await.unwrap(),
            None => setting::remove(&pool, TAG_FIELDS).await.unwrap(),
        }
    }
}

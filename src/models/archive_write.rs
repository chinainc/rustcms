//! 内容写入（新增 / 编辑 / 删除）。
//!
//! 行为约定：
//! - 校验顺序与文案（分类必须存在且是内容列表类型、标题必填、正文必填、
//! 同标题不重复）；
//! - url_token：小写化 + 去非法字符，与分类/内容表查重，冲突加 `-N` 后缀；
//! 中文标题在 ASCII 化后为空时留空，前台按 id 出链接（不引入拼音库，
//! 因此不生成拼音别名）；
//! - 分类计数：重算「自身 + 全部下级」的内容数写回 `categories.archive_count`
//! （注意是重算而不是 ±1）；
//! - 删除：先做硬删除（`archives` / `archive_data` / `archive_categories`
//! 三处一起清）；搬进 `archive_drafts` 并置 status=99 的软删除留给后台回收站。

use serde::Deserialize;
use sqlx::mysql::{MySql, MySqlConnection, MySqlPool};
use sqlx::{QueryBuilder, Transaction};

use super::category::{self, Category};
use super::{archive, extra, module, ModelError};

/// 正文长度上限保护（longtext 足够，这里只防误传超大内容）。
const MAX_CONTENT_BYTES: usize = 8 * 1024 * 1024;
/// 标题长度上限（表结构 varchar(190)）。
const MAX_TITLE_CHARS: usize = 190;
/// 摘要长度（250 字符）。
const MAX_DESCRIPTION_CHARS: usize = 250;
/// url_token 长度上限（既有实现 100）。
const MAX_TOKEN_CHARS: usize = 100;

/// 写入用的内容参数。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ArchiveInput {
    pub title: String,
    #[serde(default)]
    pub seo_title: String,
    #[serde(default)]
    pub content: String,
    pub category_id: u32,
    #[serde(default)]
    pub keywords: String,
    /// 留空时自动截取正文生成
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub images: Vec<String>,
    /// 留空时按标题生成；显式传值会做去重
    #[serde(default)]
    pub url_token: Option<String>,
    #[serde(default)]
    pub template: String,
    #[serde(default)]
    pub canonical_url: String,
    #[serde(default)]
    pub fixed_link: String,
    #[serde(default)]
    pub price: i64,
    /// 不传时沿用默认库存（新增）/ 原值（编辑）
    #[serde(default)]
    pub stock: Option<i64>,
    #[serde(default)]
    pub read_level: i32,
    #[serde(default)]
    pub sort: u32,
    /// 不传时新增为 0、编辑保持原值
    #[serde(default)]
    pub views: Option<u32>,
    #[serde(default)]
    pub origin_url: String,
    /// 不传时为当前时间
    #[serde(default)]
    pub created_time: Option<i64>,
    /// 模块自定义字段（键 = 字段标识）；不传表示不改动模块表
    #[serde(default)]
    pub extra: Option<serde_json::Map<String, serde_json::Value>>,
}

/// 写入结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SavedArchive {
    pub id: i64,
    pub url_token: String,
}

fn clip_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

/// 去掉 HTML 标签并把连续空白压成一个空格。
pub fn strip_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    let mut last_space = true;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                // 标签位置补一个空格，避免「</p><p>」把两段粘成一个词
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
            }
            _ if in_tag => {}
            c if c.is_whitespace() => {
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
            }
            c => {
                out.push(c);
                last_space = false;
            }
        }
    }
    out.trim().to_string()
}

/// 从正文生成摘要（剥标签 + 截断）。
pub fn summarize(content: &str, max: usize) -> String {
    clip_chars(&strip_tags(content), max)
}

/// 标题 → url_token：小写、非字母数字转 `-`、压缩连续 `-`。
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for ch in title.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    clip_chars(out.trim_matches('-'), MAX_TOKEN_CHARS)
}

fn normalize_token(token: &str) -> String {
    slugify(token)
}

#[allow(clippy::explicit_auto_deref)] // sqlx 的 Executor 需要显式重借用
async fn token_taken(
    conn: &mut MySqlConnection,
    token: &str,
    exclude_id: i64,
) -> Result<bool, ModelError> {
    let in_category: Option<u32> =
        sqlx::query_scalar("SELECT id FROM categories WHERE url_token = ? LIMIT 1")
            .bind(token)
            .fetch_optional(&mut *conn)
            .await?;
    if in_category.is_some() {
        return Ok(true);
    }

    let in_archive: Option<i64> =
        sqlx::query_scalar("SELECT id FROM archives WHERE url_token = ? AND id <> ? LIMIT 1")
            .bind(token)
            .bind(exclude_id)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(in_archive.is_some())
}

/// 生成不冲突的 url_token。
pub async fn verify_url_token(
    pool: &MySqlPool,
    given: Option<&str>,
    title: &str,
    id: i64,
) -> Result<String, ModelError> {
    let base = match given.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => normalize_token(value),
        None => slugify(title),
    };
    if base.is_empty() {
        // 纯中文标题：不生成 ASCII token，前台回落到 /archive/{id}.html
        return Ok(String::new());
    }

    let mut conn = pool.acquire().await?;
    for index in 0..1000 {
        let candidate = if index == 0 {
            base.clone()
        } else {
            clip_chars(&format!("{base}-{index}"), MAX_TOKEN_CHARS)
        };
        if !token_taken(&mut conn, &candidate, id).await? {
            return Ok(candidate);
        }
    }

    Ok(clip_chars(&format!("{base}-{}", id), MAX_TOKEN_CHARS))
}

async fn find_id_by_title(pool: &MySqlPool, title: &str) -> Result<Option<i64>, ModelError> {
    let id: Option<i64> = sqlx::query_scalar("SELECT id FROM archives WHERE title = ? LIMIT 1")
        .bind(title)
        .fetch_optional(pool)
        .await?;
    Ok(id)
}

/// 校验分类并把模块带出来。
async fn resolve_category(pool: &MySqlPool, category_id: u32) -> Result<Category, ModelError> {
    let category = category::get(pool, category_id)
        .await?
        .ok_or_else(|| ModelError::invalid("请选择文档分类"))?;
    if category.kind != 1 {
        // type=3 是单页，不能放内容
        return Err(ModelError::invalid("请选择文档分类"));
    }
    Ok(category)
}

fn check_title(title: &str) -> Result<String, ModelError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ModelError::invalid("请填写文档标题"));
    }
    Ok(clip_chars(title, MAX_TITLE_CHARS))
}

fn check_content(content: &str) -> Result<(), ModelError> {
    if content.trim().is_empty() {
        return Err(ModelError::invalid("请填写文档内容"));
    }
    if content.len() > MAX_CONTENT_BYTES {
        return Err(ModelError::invalid("文档内容过大"));
    }
    Ok(())
}

fn bind_images(images: &[String]) -> Result<String, ModelError> {
    let images: Vec<&str> = images
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .collect();
    Ok(serde_json::to_string(&images)?)
}

/// 重算某个分类及其全部下级的内容数量。
#[allow(clippy::explicit_auto_deref)] // 同上
async fn recount_category(
    conn: &mut MySqlConnection,
    category_id: u32,
    scope: &[u32],
) -> Result<(), ModelError> {
    let mut builder =
        QueryBuilder::<MySql>::new("SELECT COUNT(*) FROM archives WHERE category_id IN (");
    {
        let mut separated = builder.separated(", ");
        for id in scope {
            separated.push_bind(*id);
        }
        separated.push_unseparated(")");
    }
    let total: i64 = builder.build_query_scalar().fetch_one(&mut *conn).await?;

    sqlx::query("UPDATE categories SET archive_count = ? WHERE id = ?")
        .bind(total)
        .bind(category_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// 分类的统计口径：自身 + 全部下级。
fn count_scope(categories: &[Category], category_id: u32) -> Vec<u32> {
    category::self_and_sub_ids(categories, category_id)
}

/// 分配内容 ID：`archives` 与 `archive_drafts` 的最大 ID + 1（`provider.GetNextArchiveId`）。
///
/// 不能依赖 `archives.id` 的 AUTO_INCREMENT：删除/草稿的内容只留在 `archive_drafts`，
/// 而 `archive_data` 仍占着同一个 ID，用自增值写正文会主键冲突。
async fn next_archive_id(tx: &mut Transaction<'_, MySql>) -> Result<i64, ModelError> {
    // GREATEST(IFNULL(...)) 的结果类型是 DECIMAL，必须 CAST 成有符号整数，
    // 否则 sqlx 按 BIGINT 解码会报 mismatched types。
    let id: i64 = sqlx::query_scalar(
        "SELECT CAST(GREATEST(\
            IFNULL((SELECT MAX(id) FROM archives), 0), \
            IFNULL((SELECT MAX(id) FROM archive_drafts), 0), \
            IFNULL((SELECT MAX(id) FROM archive_data), 0)\
         ) + 1 AS SIGNED)",
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

async fn recount_with_pool(
    pool: &MySqlPool,
    categories: &[Category],
    category_ids: &[u32],
) -> Result<(), ModelError> {
    let mut conn = pool.acquire().await?;
    for category_id in category_ids {
        let scope = count_scope(categories, *category_id);
        recount_category(&mut conn, *category_id, &scope).await?;
    }
    Ok(())
}

fn bind_list(builder: &mut QueryBuilder<'_, MySql>, values: &[String]) {
    let json = match serde_json::to_string(values) {
        Ok(json) => json,
        Err(_) => "[]".to_string(),
    };
    builder.push_bind(json);
}

/// 新增内容，返回新 ID 与最终 url_token。
pub async fn create(pool: &MySqlPool, input: &ArchiveInput) -> Result<SavedArchive, ModelError> {
    let title = check_title(&input.title)?;
    check_content(&input.content)?;
    let category = resolve_category(pool, input.category_id).await?;
    let model = module::get(pool, category.module_id)
        .await?
        .ok_or_else(|| ModelError::invalid("未定义模型"))?;

    // 扩展字段：新增时始终校验（模块配了必填字段就必须传），并补齐缺失的列
    let has_fields = !model.field_defs().is_empty();
    let empty_extra = serde_json::Map::new();
    let extra_values = input.extra.as_ref().unwrap_or(&empty_extra);
    let extra_ready = if has_fields {
        extra::check_required(&model, extra_values)?;
        Some(extra::prepare(pool, &model).await?)
    } else {
        None
    };

    if let Some(exist_id) = find_id_by_title(pool, &title).await? {
        return Err(ModelError::invalid(format!(
            "文档标题已重复（ID:{exist_id}）"
        )));
    }

    let description = if input.description.trim().is_empty() {
        summarize(&input.content, MAX_DESCRIPTION_CHARS)
    } else {
        clip_chars(input.description.trim(), 1000)
    };
    let url_token = verify_url_token(pool, input.url_token.as_deref(), &title, 0).await?;
    let images = bind_images(&input.images)?;

    let now = chrono::Utc::now().timestamp();
    let created_time = input.created_time.unwrap_or(now);
    let stock = input.stock.unwrap_or(9_999_999);

    let mut tx: Transaction<'_, MySql> = pool.begin().await?;

    let id = next_archive_id(&mut tx).await?;

    sqlx::query(
        "INSERT INTO archives (\
            id, parent_id, created_time, updated_time, title, seo_title, url_token, keywords, description, \
            module_id, category_id, place_id, views, comment_count, favorite_count, images, template, \
            canonical_url, fixed_link, user_id, price, stock, read_level, password, sort, has_pseudo, \
            keyword_id, origin_url, origin_title, origin_id\
         ) VALUES (?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, 0, 0, ?, ?, ?, ?, 0, ?, ?, ?, '', ?, 0, 0, ?, '', 0)",
    )
    .bind(id)
    .bind(created_time)
    .bind(now)
    .bind(&title)
    .bind(&input.seo_title)
    .bind(&url_token)
    .bind(&input.keywords)
    .bind(&description)
    .bind(category.module_id)
    .bind(input.category_id)
    .bind(input.views.unwrap_or(0))
    .bind(&images)
    .bind(&input.template)
    .bind(&input.canonical_url)
    .bind(&input.fixed_link)
    .bind(input.price)
    .bind(stock)
    .bind(input.read_level)
    .bind(input.sort)
    .bind(&input.origin_url)
    .execute(&mut *tx)
    .await?;

    sqlx::query("INSERT INTO archive_data (id, content) VALUES (?, ?)")
        .bind(id)
        .bind(&input.content)
        .execute(&mut *tx)
        .await?;

    // 模块扩展字段
    if let Some(columns) = extra_ready.as_ref() {
        extra::save_in_tx(&mut tx, &model, id, extra_values, columns).await?;
    }

    // 分类计数
    let categories = category::list_all(pool).await?;
    let scope = count_scope(&categories, input.category_id);
    recount_category(&mut tx, input.category_id, &scope).await?;

    tx.commit().await?;

    Ok(SavedArchive { id, url_token })
}

/// 编辑内容。
pub async fn update(
    pool: &MySqlPool,
    id: i64,
    input: &ArchiveInput,
) -> Result<SavedArchive, ModelError> {
    let existing = archive::get(pool, id)
        .await?
        .ok_or_else(|| ModelError::invalid("内容不存在"))?;

    let title = check_title(&input.title)?;
    check_content(&input.content)?;
    let category = resolve_category(pool, input.category_id).await?;
    let model = module::get(pool, category.module_id)
        .await?
        .ok_or_else(|| ModelError::invalid("未定义模型"))?;

    let extra_ready = match input.extra.as_ref() {
        Some(values) => {
            extra::check_required(&model, values)?;
            Some(extra::prepare(pool, &model).await?)
        }
        None => None,
    };

    // 同标题只能是它自己
    if let Some(other) = find_id_by_title(pool, &title).await? {
        if other != id {
            return Err(ModelError::invalid(format!("文档标题已重复（ID:{other}）")));
        }
    }

    let description = if input.description.trim().is_empty() {
        summarize(&input.content, MAX_DESCRIPTION_CHARS)
    } else {
        clip_chars(input.description.trim(), 1000)
    };
    let url_token = match input.url_token.as_deref().map(str::trim) {
        Some(value) if !value.is_empty() => verify_url_token(pool, Some(value), &title, id).await?,
        // 未传 token：沿用原值，避免编辑标题把链接改掉
        _ => existing.url_token.clone(),
    };
    let images = bind_images(&input.images)?;

    let now = chrono::Utc::now().timestamp();
    let created_time = input
        .created_time
        .unwrap_or_else(|| existing.created_time.unwrap_or(now));
    let stock = input.stock.unwrap_or(existing.stock);

    let mut tx: Transaction<'_, MySql> = pool.begin().await?;

    sqlx::query(
        "UPDATE archives SET \
            created_time = ?, updated_time = ?, title = ?, seo_title = ?, url_token = ?, keywords = ?, \
            description = ?, module_id = ?, category_id = ?, images = ?, template = ?, canonical_url = ?, \
            fixed_link = ?, price = ?, stock = ?, read_level = ?, sort = ?, origin_url = ? \
         WHERE id = ?",
    )
    .bind(created_time)
    .bind(now)
    .bind(&title)
    .bind(&input.seo_title)
    .bind(&url_token)
    .bind(&input.keywords)
    .bind(&description)
    .bind(category.module_id)
    .bind(input.category_id)
    .bind(&images)
    .bind(&input.template)
    .bind(&input.canonical_url)
    .bind(&input.fixed_link)
    .bind(input.price)
    .bind(stock)
    .bind(input.read_level)
    .bind(input.sort)
    .bind(&input.origin_url)
    .bind(id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO archive_data (id, content) VALUES (?, ?) \
         ON DUPLICATE KEY UPDATE content = VALUES(content)",
    )
    .bind(id)
    .bind(&input.content)
    .execute(&mut *tx)
    .await?;

    if let (Some(values), Some(columns)) = (input.extra.as_ref(), extra_ready.as_ref()) {
        extra::save_in_tx(&mut tx, &model, id, values, columns).await?;
    }

    let categories = category::list_all(pool).await?;
    let mut affected = vec![input.category_id];
    if existing.category_id != input.category_id {
        affected.push(existing.category_id);
    }
    for category_id in affected {
        let scope = count_scope(&categories, category_id);
        recount_category(&mut tx, category_id, &scope).await?;
    }

    tx.commit().await?;
    Ok(SavedArchive { id, url_token })
}

/// 删除内容。
pub async fn delete(pool: &MySqlPool, id: i64) -> Result<bool, ModelError> {
    let Some(existing) = archive::get(pool, id).await? else {
        return Ok(false);
    };

    let mut tx = pool.begin().await?;

    sqlx::query("DELETE FROM archive_data WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    // 多分类关联（本仓库暂未启用多分类，存在则一起清）
    sqlx::query("DELETE FROM archive_categories WHERE archive_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM archives WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    // 模块扩展表里的那一行（既有实现: provider.DeleteArchive 里的 DELETE FROM {module.TableName}）
    if let Ok(Some(model)) = module::get(pool, existing.module_id).await {
        if let Ok(table) = model.safe_table() {
            let sql = format!("DELETE FROM `{table}` WHERE id = ?");
            // 表可能还没建（模块没配扩展字段），失败不影响主流程
            let _ = sqlx::query(&sql).bind(id).execute(&mut *tx).await;
        }
    }

    let categories = category::list_all(pool).await?;
    let scope = count_scope(&categories, existing.category_id);
    recount_category(&mut tx, existing.category_id, &scope).await?;

    tx.commit().await?;
    Ok(true)
}

/// 供后台/命令行手动修数：重算某个分类的计数。
pub async fn refresh_category_count(pool: &MySqlPool, category_id: u32) -> Result<i64, ModelError> {
    let categories = category::list_all(pool).await?;
    recount_with_pool(pool, &categories, &[category_id]).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT CAST(archive_count AS SIGNED) FROM categories WHERE id = ?")
            .bind(category_id)
            .fetch_one(pool)
            .await?;
    Ok(count)
}

/// 测试辅助：按标题前缀清理残留用例数据（含正文表）。
#[cfg(test)]
pub(crate) async fn purge_test_archives(pool: &MySqlPool, prefix: &str) {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM archives WHERE title LIKE ?")
        .bind(format!("{prefix}%"))
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    for id in ids {
        let _ = sqlx::query("DELETE FROM archive_data WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM archives WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::test_pool;

    fn input(category_id: u32, title: &str, content: &str) -> ArchiveInput {
        ArchiveInput {
            title: title.to_string(),
            content: content.to_string(),
            category_id,
            ..Default::default()
        }
    }

    async fn category_count(pool: &MySqlPool, category_id: u32) -> i64 {
        // archive_count 是 INT UNSIGNED，直接按 i64 解码会类型不匹配
        sqlx::query_scalar("SELECT CAST(archive_count AS SIGNED) FROM categories WHERE id = ?")
            .bind(category_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn actual_count(pool: &MySqlPool, category_id: u32) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM archives WHERE category_id = ?")
            .bind(category_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// 分类计数的统计口径：自身 + 全部下级。
    async fn scope_count(pool: &MySqlPool, categories: &[Category], category_id: u32) -> i64 {
        let mut total = 0;
        for id in category::self_and_sub_ids(categories, category_id) {
            total += actual_count(pool, id).await;
        }
        total
    }

    #[test]
    fn slugifies_and_strips() {
        assert_eq!(slugify("GrindMaster 8000"), "grindmaster-8000");
        assert_eq!(slugify("  Hello, World!! "), "hello-world");
        assert_eq!(slugify("纯中文标题"), "");
        assert_eq!(strip_tags("<p>a</p><p>b</p>"), "a b");
        assert_eq!(summarize("<p>你好  世界</p>", 250), "你好 世界");
        assert_eq!(clip_chars("一二三四五", 3), "一二三");
    }

    #[tokio::test]
    async fn create_update_delete_flow() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        purge_test_archives(&pool, "Rust 写入测试文档").await;

        // 找一个内容列表类型的分类
        let categories = category::list_all(&pool).await.unwrap();
        let Some(category) = categories.iter().find(|c| c.kind == 1) else {
            return;
        };
        let category_id = category.id;
        let fallback_category = categories
            .iter()
            .find(|c| c.kind == 1 && c.id != category_id)
            .map(|c| c.id);

        // 新增
        let mut data = input(category_id, "Rust 写入测试文档", "<p>正文内容</p>");
        data.images = vec!["/uploads/demo.webp".to_string()];
        let created = create(&pool, &data).await.unwrap();
        assert!(created.id > 0);
        // 标题里的 ASCII 部分生成 token（可能带 -N 后缀，取决于库里已有数据）
        assert!(
            created.url_token.starts_with("rust"),
            "token={}",
            created.url_token
        );
        assert_eq!(
            category_count(&pool, category_id).await,
            scope_count(&pool, &categories, category_id).await
        );

        let (row, content) = archive::detail(&pool, created.id).await.unwrap().unwrap();
        assert_eq!(row.title, "Rust 写入测试文档");
        assert_eq!(row.module_id, category.module_id);
        assert_eq!(content, "<p>正文内容</p>");
        assert_eq!(row.description, "正文内容");
        assert_eq!(row.image_list(), vec!["/uploads/demo.webp".to_string()]);

        // 同标题拒绝
        let err = create(&pool, &data).await.unwrap_err();
        assert!(err.to_string().contains("文档标题已重复"), "{err}");

        // 同 token 自动加后缀
        let mut second = input(category_id, "Rust 写入测试文档二", "<p>二</p>");
        second.url_token = Some("rust".to_string());
        let created2 = create(&pool, &second).await.unwrap();
        assert_ne!(created2.url_token, created.url_token);
        assert!(created2.url_token.starts_with("rust"));

        // 编辑：改标题 + 换分类
        let mut edit = input(category_id, "Rust 写入测试文档（已改）", "<p>新正文</p>");
        edit.description = String::new();
        if let Some(target) = fallback_category {
            edit.category_id = target;
        }
        let updated = update(&pool, created.id, &edit).await.unwrap();
        assert_eq!(updated.id, created.id);
        // 未传 token 时沿用原值
        assert_eq!(updated.url_token, created.url_token);

        let (row, content) = archive::detail(&pool, created.id).await.unwrap().unwrap();
        assert_eq!(row.title, "Rust 写入测试文档（已改）");
        assert_eq!(content, "<p>新正文</p>");
        assert_eq!(row.category_id, fallback_category.unwrap_or(category_id));
        assert_eq!(
            category_count(&pool, category_id).await,
            scope_count(&pool, &categories, category_id).await
        );
        if let Some(target) = fallback_category {
            assert_eq!(
                category_count(&pool, target).await,
                scope_count(&pool, &categories, target).await
            );
        }

        // 删除：行与正文都没了，计数回到实际值
        assert!(delete(&pool, created.id).await.unwrap());
        assert!(archive::get(&pool, created.id).await.unwrap().is_none());
        assert!(archive::content(&pool, created.id).await.unwrap().is_none());
        assert_eq!(
            category_count(&pool, category_id).await,
            scope_count(&pool, &categories, category_id).await
        );
        // 重复删除返回 false
        assert!(!delete(&pool, created.id).await.unwrap());

        // 收尾
        delete(&pool, created2.id).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_bad_input() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };

        let empty_title = input(13, "   ", "<p>x</p>");
        assert_eq!(
            create(&pool, &empty_title).await.unwrap_err().to_string(),
            "请填写文档标题"
        );

        let empty_content = input(13, "标题", "  ");
        assert_eq!(
            create(&pool, &empty_content).await.unwrap_err().to_string(),
            "请填写文档内容"
        );

        // 单页分类（type=3）不能放内容
        let categories = category::list_all(&pool).await.unwrap();
        if let Some(page_cat) = categories.iter().find(|c| c.kind != 1) {
            let err = create(&pool, &input(page_cat.id, "标题", "<p>x</p>"))
                .await
                .unwrap_err();
            assert_eq!(err.to_string(), "请选择文档分类");
        }

        // 分类不存在
        let err = create(&pool, &input(u32::MAX, "标题", "<p>x</p>"))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "请选择文档分类");

        // 更新不存在的 ID
        let err = update(&pool, i64::MAX, &input(13, "标题", "<p>x</p>"))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "内容不存在");
    }

    #[tokio::test]
    async fn refreshes_category_count_including_children() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let categories = category::list_all(&pool).await.unwrap();
        // 找一个有子分类的父分类
        let Some(parent) = categories
            .iter()
            .find(|c| categories.iter().any(|child| child.parent_id == c.id))
        else {
            return;
        };

        let count = refresh_category_count(&pool, parent.id).await.unwrap();
        let scope = category::self_and_sub_ids(&categories, parent.id);
        let mut total = 0i64;
        for id in &scope {
            total += actual_count(&pool, *id).await;
        }
        assert_eq!(count, total);
    }

    /// 造一套临时模块 + 分类（用于验证扩展字段，不碰真实数据）。
    async fn seed_temp_module(pool: &MySqlPool, fields: &str) -> (u32, u32) {
        const ID: u32 = 990_001;
        const TABLE: &str = "rustcms_test_extra_tbl";

        let _ = sqlx::query("DROP TABLE IF EXISTS `rustcms_test_extra_tbl`")
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM modules WHERE id = ?")
            .bind(ID)
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM categories WHERE id = ?")
            .bind(ID)
            .execute(pool)
            .await;

        sqlx::query(
            "INSERT INTO modules (id, created_time, updated_time, table_name, name, url_token, \
                title, keywords, description, fields, category_fields, is_system, status) \
             VALUES (?, UNIX_TIMESTAMP(), UNIX_TIMESTAMP(), ?, 'Rust 测试模块', 'rustcms-test', \
                '', '', '', ?, 'null', 0, 1)",
        )
        .bind(ID)
        .bind(TABLE)
        .bind(fields)
        .execute(pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO categories (id, created_time, updated_time, title, seo_title, keywords, \
                url_token, description, module_id, parent_id, type, sort, template, detail_template, \
                is_inherit, images, logo, extra, archive_count, status) \
             VALUES (?, UNIX_TIMESTAMP(), UNIX_TIMESTAMP(), 'Rust 测试分类', '', '', \
                'rustcms-test-cate', '', ?, 0, 1, 99, '', '', 0, '{}', '', 'null', 0, 1)",
        )
        .bind(ID)
        .bind(ID)
        .execute(pool)
        .await
        .unwrap();

        (ID, ID)
    }

    async fn drop_temp_module(pool: &MySqlPool) {
        let _ = sqlx::query("DROP TABLE IF EXISTS `rustcms_test_extra_tbl`")
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM modules WHERE id = 990001")
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM categories WHERE id = 990001")
            .execute(pool)
            .await;
    }

    #[tokio::test]
    async fn writes_and_clears_module_extra_fields() {
        use serde_json::{json, Map};

        let _guard = crate::models::db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        purge_test_archives(&pool, "Rust 扩展字段测试").await;

        let fields = r#"[
            {"name":"作者","field_name":"author","type":"text","required":true},
            {"name":"页数","field_name":"pages","type":"number"},
            {"name":"标签","field_name":"tags","type":"checkbox"}
        ]"#;
        let (module_id, category_id) = seed_temp_module(&pool, fields).await;

        // 必填缺失：拒绝
        let mut data = input(category_id, "Rust 扩展字段测试", "<p>x</p>");
        let err = create(&pool, &data).await.unwrap_err();
        assert_eq!(err.to_string(), "作者必填");

        // 带扩展字段创建
        let mut values = Map::new();
        values.insert("author".into(), json!("张三"));
        values.insert("pages".into(), json!(12));
        values.insert("tags".into(), json!(["a", "b"]));
        data.extra = Some(values);
        let saved = create(&pool, &data).await.unwrap();

        let model = module::get(&pool, module_id).await.unwrap().unwrap();
        let loaded = extra::load(&pool, &model, saved.id).await.unwrap();
        assert_eq!(loaded["author"], json!("张三"));
        assert_eq!(loaded["pages"], json!(12));
        // checkbox 落库为逗号串，读回仍是字符串（与既有接口契约一致）
        assert_eq!(loaded["tags"], json!("a,b"));

        let row = archive::get(&pool, saved.id).await.unwrap().unwrap();
        assert_eq!(row.module_id, module_id);

        // 编辑：未提交的字段被清空
        let mut edit = input(category_id, "Rust 扩展字段测试", "<p>x</p>");
        let mut edit_values = Map::new();
        edit_values.insert("author".into(), json!("李四"));
        edit.extra = Some(edit_values);
        update(&pool, saved.id, &edit).await.unwrap();

        let loaded = extra::load(&pool, &model, saved.id).await.unwrap();
        assert_eq!(loaded["author"], json!("李四"));
        assert_eq!(loaded["pages"], json!(0));
        assert_eq!(loaded["tags"], json!(""));

        // 编辑时不传 extra：模块表保持原样（仍是李四）
        let mut plain = input(category_id, "Rust 扩展字段测试", "<p>y</p>");
        plain.url_token = Some(saved.url_token.clone());
        update(&pool, saved.id, &plain).await.unwrap();
        let loaded = extra::load(&pool, &model, saved.id).await.unwrap();
        assert_eq!(loaded["author"], json!("李四"));

        // 删除：内容与模块表行一起清掉
        assert!(delete(&pool, saved.id).await.unwrap());
        let leftover: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM `rustcms_test_extra_tbl` WHERE id = ?")
                .bind(saved.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(leftover, 0);

        drop_temp_module(&pool).await;
    }
}

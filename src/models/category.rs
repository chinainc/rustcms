//! 分类模型（表 `categories`）。
//!
//! 排序固定为 `sort ASC, id ASC`，再按 `parent_id` 组装成树。

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::{json_string_list, ModelError};

/// 内容状态：正式内容。
pub const CONTENT_STATUS_OK: u8 = 1;

/// 分类。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Category {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub title: String,
    pub seo_title: String,
    pub keywords: String,
    pub url_token: String,
    pub description: String,
    pub content: Option<String>,
    pub module_id: u32,
    pub parent_id: u32,
    /// 1 = 内容列表，3 = 单页
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub kind: u32,
    pub sort: u32,
    pub template: String,
    pub detail_template: String,
    pub is_inherit: u8,
    /// 库里是 `{"a","b"}` 形态的文本，序列化时输出成数组。
    #[serde(serialize_with = "super::serialize_images")]
    pub images: Option<String>,
    pub logo: String,
    pub extra: Option<String>,
    pub archive_count: u32,
    pub status: u8,
}

impl Category {
    /// 图片列表。
    pub fn image_list(&self) -> Vec<String> {
        json_string_list(self.images.as_deref())
    }

    /// 封面：优先 logo，其次第一张图；都没有则为空。
    pub fn cover(&self) -> String {
        if !self.logo.is_empty() {
            return self.logo.clone();
        }
        self.image_list().into_iter().next().unwrap_or_default()
    }

    /// 是否单页（type=3）。
    pub fn is_page(&self) -> bool {
        self.kind == 3
    }
}

/// 带下级的分类节点。
#[derive(Debug, Clone, Serialize)]
pub struct CategoryNode {
    #[serde(flatten)]
    pub category: Category,
    pub children: Vec<CategoryNode>,
}

/// 全部分类，按 `sort ASC, id ASC`。
pub async fn list_all(pool: &MySqlPool) -> anyhow::Result<Vec<Category>> {
    let list = sqlx::query_as::<_, Category>("SELECT * FROM categories ORDER BY sort ASC, id ASC")
        .fetch_all(pool)
        .await?;
    Ok(list)
}

/// 某个模块下的分类。
pub async fn list_by_module(pool: &MySqlPool, module_id: u32) -> anyhow::Result<Vec<Category>> {
    let list = sqlx::query_as::<_, Category>(
        "SELECT * FROM categories WHERE module_id = ? ORDER BY sort ASC, id ASC",
    )
    .bind(module_id)
    .fetch_all(pool)
    .await?;
    Ok(list)
}

/// 按 ID 取分类。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Category>> {
    let row = sqlx::query_as::<_, Category>("SELECT * FROM categories WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// 按 url_token 取分类。
pub async fn get_by_token(pool: &MySqlPool, token: &str) -> anyhow::Result<Option<Category>> {
    let row = sqlx::query_as::<_, Category>("SELECT * FROM categories WHERE url_token = ? LIMIT 1")
        .bind(token)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// 按 `parent_id` 分组（组内保持原有顺序）。
pub fn group_by_parent(list: &[Category]) -> HashMap<u32, Vec<Category>> {
    let mut groups: HashMap<u32, Vec<Category>> = HashMap::new();
    for item in list {
        groups.entry(item.parent_id).or_default().push(item.clone());
    }
    groups
}

/// 组装成树（根节点是 `parent_id = 0`）。
pub fn tree(list: &[Category]) -> Vec<CategoryNode> {
    fn build(parent_id: u32, groups: &mut HashMap<u32, Vec<Category>>) -> Vec<CategoryNode> {
        let mut children = groups.remove(&parent_id).unwrap_or_default();
        children.sort_by_key(|c| (c.sort, c.id));
        children
            .into_iter()
            .map(|category| {
                let children = build(category.id, groups);
                CategoryNode { category, children }
            })
            .collect()
    }

    build(0, &mut group_by_parent(list))
}

/// 取某分类的全部下级 ID（不含自身），跳过非正式分类。
pub fn sub_ids(list: &[Category], root_id: u32) -> Vec<u32> {
    fn walk(list: &[Category], parent_id: u32, out: &mut Vec<u32>) {
        for item in list {
            if item.parent_id != parent_id || item.status != CONTENT_STATUS_OK {
                continue;
            }
            out.push(item.id);
            walk(list, item.id, out);
        }
    }

    let mut out = Vec::new();
    walk(list, root_id, &mut out);
    out
}

/// 自身 + 全部下级 ID（用于按分类筛选内容）。
pub fn self_and_sub_ids(list: &[Category], root_id: u32) -> Vec<u32> {
    let mut ids = vec![root_id];
    ids.extend(sub_ids(list, root_id));
    ids
}

/// 分类的新建 / 更新入参（对应既有约定）。
///
/// 全部字段都用 `Option`：`None` 等同于 既有实现的零值，
/// `update_all = !partial` 时才会把「显式为空」也写进去。
#[derive(Debug, Default, Clone, Deserialize)]
pub struct CategoryInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub seo_title: Option<String>,
    #[serde(default)]
    pub keywords: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub module_id: Option<u32>,
    #[serde(default)]
    pub parent_id: Option<u32>,
    #[serde(default)]
    pub sort: Option<u32>,
    #[serde(default)]
    pub status: Option<u8>,
    #[serde(default, rename = "type")]
    pub kind: Option<u32>,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub detail_template: Option<String>,
    #[serde(default)]
    pub url_token: Option<String>,
    /// URL 别名重复时是否强制覆盖（不检查重复）。
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub images: Option<Vec<String>>,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub is_inherit: Option<u8>,
    #[serde(default)]
    pub update_all: bool,
    /// PATCH 语义：只覆盖显式传入的字段。
    #[serde(default)]
    pub partial: bool,
    #[serde(default)]
    pub extra: Option<Value>,
}

/// 字符串字段的赋值规则：`update_all` 时照抄，否则只有非空才覆盖。
fn apply_str(target: &mut String, input: Option<&str>, update_all: bool) {
    if let Some(value) = input {
        if update_all || !value.is_empty() {
            *target = value.to_string();
        }
    }
}

/// 数字字段的赋值规则：`update_all` 时照抄，否则只有大于零才覆盖。
fn apply_num<T: Copy + Default + PartialOrd>(target: &mut T, input: Option<T>, update_all: bool) {
    if let Some(value) = input {
        if update_all || value > T::default() {
            *target = value;
        }
    }
}

/// URL 别名清洗：小写、只保留字母数字与 `-`/`_`，其余折叠成 `-`。
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

/// 确保 URL 别名在分类与内容之间唯一（冲突时追加 `-1`、`-2`…）。
async fn verify_url_token(
    pool: &MySqlPool,
    token: String,
    exclude_id: u32,
) -> anyhow::Result<String> {
    let mut token: String = token.chars().take(150).collect();
    if token.is_empty() {
        token = "category".to_string();
    }
    let mut index = 0u32;
    loop {
        let candidate = if index == 0 {
            token.clone()
        } else {
            format!("{token}-{index}")
        };
        if let Some(found) = get_by_token(pool, &candidate).await? {
            if found.id != exclude_id {
                index += 1;
                continue;
            }
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM archives WHERE url_token = ?")
            .bind(&candidate)
            .fetch_one(pool)
            .await?;
        if count > 0 {
            index += 1;
            continue;
        }
        return Ok(candidate);
    }
}

/// 新建 / 更新分类（遵循既有约定的核心语义）。
pub async fn save(pool: &MySqlPool, input: &CategoryInput) -> Result<Category, ModelError> {
    let update_all = input.update_all || !input.partial;
    let mut is_new = false;

    let mut category = if input.id > 0 {
        match get(pool, input.id).await? {
            Some(found) => found,
            None => {
                is_new = true;
                Category {
                    id: input.id,
                    created_time: None,
                    updated_time: None,
                    title: String::new(),
                    seo_title: String::new(),
                    keywords: String::new(),
                    url_token: String::new(),
                    description: String::new(),
                    content: None,
                    module_id: 0,
                    parent_id: 0,
                    kind: 0,
                    sort: 99,
                    template: String::new(),
                    detail_template: String::new(),
                    is_inherit: 0,
                    images: None,
                    logo: String::new(),
                    extra: None,
                    archive_count: 0,
                    status: 1,
                }
            }
        }
    } else {
        is_new = true;
        Category {
            id: 0,
            created_time: None,
            updated_time: None,
            title: String::new(),
            seo_title: String::new(),
            keywords: String::new(),
            url_token: String::new(),
            description: String::new(),
            content: None,
            module_id: 0,
            parent_id: 0,
            kind: 0,
            sort: 99,
            template: String::new(),
            detail_template: String::new(),
            is_inherit: 0,
            images: None,
            logo: String::new(),
            extra: None,
            archive_count: 0,
            status: 1,
        }
    };

    apply_str(&mut category.title, input.title.as_deref(), update_all);
    apply_str(&mut category.seo_title, input.seo_title.as_deref(), update_all);
    apply_str(&mut category.keywords, input.keywords.as_deref(), update_all);
    apply_str(
        &mut category.description,
        input.description.as_deref(),
        update_all,
    );
    apply_str(&mut category.template, input.template.as_deref(), update_all);
    apply_str(
        &mut category.detail_template,
        input.detail_template.as_deref(),
        update_all,
    );
    apply_str(&mut category.logo, input.logo.as_deref(), update_all);
    apply_num(&mut category.kind, input.kind, update_all);
    apply_num(&mut category.module_id, input.module_id, update_all);
    apply_num(&mut category.parent_id, input.parent_id, update_all);
    apply_num(&mut category.sort, input.sort, update_all);
    apply_num(&mut category.status, input.status, update_all);
    apply_num(&mut category.is_inherit, input.is_inherit, update_all);

    if let Some(content) = input.content.as_deref() {
        if update_all || !content.is_empty() {
            category.content = Some(content.to_string());
        }
    }
    if let Some(images) = input.images.as_ref() {
        if update_all || !images.is_empty() {
            category.images = if images.is_empty() {
                None
            } else {
                Some(serde_json::to_string(images)?)
            };
        }
    }
    if let Some(extra) = input.extra.as_ref() {
        category.extra = Some(serde_json::to_string(extra)?);
    }

    // 下级分类的「类型/模型」强制跟随上级
    if category.parent_id > 0 {
        if let Some(parent) = get(pool, category.parent_id).await? {
            category.kind = parent.kind;
            category.module_id = parent.module_id;
        }
    }

    // 别名
    if update_all || input.url_token.as_deref().is_some_and(|v| !v.is_empty()) {
        let raw = input.url_token.as_deref().unwrap_or_default();
        let mut token = sanitize_token(raw);
        if !input.force && !token.is_empty() {
            if let Some(found) = get_by_token(pool, &token).await? {
                if found.id != category.id {
                    return Err(ModelError::invalid("token duplication"));
                }
            }
        }
        if token.is_empty() {
            token = sanitize_token(&category.title);
        }
        if token.is_empty() {
            // 中文标题无法直接转别名（既有实现走拼音），用时间戳兜底
            token = format!("cat-{}", chrono::Utc::now().timestamp_millis());
        }
        if !input.force {
            token = verify_url_token(pool, token, category.id).await?;
        }
        category.url_token = token;
    }
    if category.url_token.is_empty() {
        let mut token = sanitize_token(&category.title);
        if token.is_empty() {
            token = format!("cat-{}", chrono::Utc::now().timestamp_millis());
        }
        category.url_token = verify_url_token(pool, token, category.id).await?;
    }

    // 文档分类未指定模型时挂第一个模型
    if category.module_id == 0 && category.kind == 1 {
        if let Some(first) = super::module::list(pool).await?.first() {
            category.module_id = first.id;
        }
    }
    if category.kind == 3 {
        category.module_id = 0;
    }

    let now = chrono::Utc::now().timestamp();
    if is_new {
        if category.created_time.is_none() {
            category.created_time = Some(now);
        }
        category.updated_time = Some(now);
        category.id = insert_row(pool, &category).await?;
    } else {
        category.updated_time = Some(now);
        update_row(pool, &category).await?;
    }

    // 隐藏分类时下级一并隐藏；恢复时把上级也恢复
    if category.status == 0 {
        sqlx::query("UPDATE categories SET status = 0 WHERE parent_id = ?")
            .bind(category.id)
            .execute(pool)
            .await?;
    } else if category.status == 1 && category.parent_id > 0 {
        sqlx::query("UPDATE categories SET status = 1 WHERE id = ?")
            .bind(category.parent_id)
            .execute(pool)
            .await?;
    }

    Ok(category)
}

async fn insert_row(pool: &MySqlPool, category: &Category) -> Result<u32, ModelError> {
    let result = if category.id > 0 {
        sqlx::query(
            "INSERT INTO categories (id, created_time, updated_time, title, seo_title, keywords, \
                url_token, description, content, module_id, parent_id, `type`, sort, template, \
                detail_template, is_inherit, images, logo, extra, archive_count, status) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(category.id)
        .bind(category.created_time)
        .bind(category.updated_time)
        .bind(&category.title)
        .bind(&category.seo_title)
        .bind(&category.keywords)
        .bind(&category.url_token)
        .bind(&category.description)
        .bind(&category.content)
        .bind(category.module_id)
        .bind(category.parent_id)
        .bind(category.kind)
        .bind(category.sort)
        .bind(&category.template)
        .bind(&category.detail_template)
        .bind(category.is_inherit)
        .bind(&category.images)
        .bind(&category.logo)
        .bind(&category.extra)
        .bind(category.archive_count)
        .bind(category.status)
        .execute(pool)
        .await?
    } else {
        sqlx::query(
            "INSERT INTO categories (created_time, updated_time, title, seo_title, keywords, \
                url_token, description, content, module_id, parent_id, `type`, sort, template, \
                detail_template, is_inherit, images, logo, extra, archive_count, status) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(category.created_time)
        .bind(category.updated_time)
        .bind(&category.title)
        .bind(&category.seo_title)
        .bind(&category.keywords)
        .bind(&category.url_token)
        .bind(&category.description)
        .bind(&category.content)
        .bind(category.module_id)
        .bind(category.parent_id)
        .bind(category.kind)
        .bind(category.sort)
        .bind(&category.template)
        .bind(&category.detail_template)
        .bind(category.is_inherit)
        .bind(&category.images)
        .bind(&category.logo)
        .bind(&category.extra)
        .bind(category.archive_count)
        .bind(category.status)
        .execute(pool)
        .await?
    };
    Ok(result.last_insert_id() as u32)
}

async fn update_row(pool: &MySqlPool, category: &Category) -> Result<(), ModelError> {
    sqlx::query(
        "UPDATE categories SET updated_time = ?, title = ?, seo_title = ?, keywords = ?, \
            url_token = ?, description = ?, content = ?, module_id = ?, parent_id = ?, \
            `type` = ?, sort = ?, template = ?, detail_template = ?, is_inherit = ?, \
            images = ?, logo = ?, extra = ?, status = ? WHERE id = ?",
    )
    .bind(category.updated_time)
    .bind(&category.title)
    .bind(&category.seo_title)
    .bind(&category.keywords)
    .bind(&category.url_token)
    .bind(&category.description)
    .bind(&category.content)
    .bind(category.module_id)
    .bind(category.parent_id)
    .bind(category.kind)
    .bind(category.sort)
    .bind(&category.template)
    .bind(&category.detail_template)
    .bind(category.is_inherit)
    .bind(&category.images)
    .bind(&category.logo)
    .bind(&category.extra)
    .bind(category.status)
    .bind(category.id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 删除分类：下级分类与内容都上移到被删分类的上级（对齐 `model.Category.Delete`）。
///
/// 返回 `false` 表示分类本来就不存在。
pub async fn delete(pool: &MySqlPool, id: u32) -> anyhow::Result<bool> {
    let Some(category) = get(pool, id).await? else {
        return Ok(false);
    };
    sqlx::query("DELETE FROM categories WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE categories SET parent_id = ? WHERE parent_id = ?")
        .bind(category.parent_id)
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE archives SET category_id = ? WHERE category_id = ?")
        .bind(category.parent_id)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(true)
}

/// 重算每个分类的 `archive_count`（含全部下级分类的内容数）。
///
/// 对齐 `UpdateCategoryArchiveCounts`，但用「一次 GROUP BY + 内存汇总」代替逐条 COUNT。
pub async fn update_archive_counts(pool: &MySqlPool) -> anyhow::Result<usize> {
    let list = list_all(pool).await?;
    if list.is_empty() {
        return Ok(0);
    }

    let rows: Vec<(u32, i64)> =
        sqlx::query_as("SELECT category_id, COUNT(*) FROM archives GROUP BY category_id")
            .fetch_all(pool)
            .await?;
    let direct: HashMap<u32, i64> = rows.into_iter().collect();

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for item in &list {
        children.entry(item.parent_id).or_default().push(item.id);
    }

    fn sum(
        id: u32,
        direct: &HashMap<u32, i64>,
        children: &HashMap<u32, Vec<u32>>,
        seen: &mut HashSet<u32>,
    ) -> i64 {
        if !seen.insert(id) {
            return 0; // 防御环状数据
        }
        let mut total = direct.get(&id).copied().unwrap_or(0);
        if let Some(kids) = children.get(&id) {
            for kid in kids {
                total += sum(*kid, direct, children, seen);
            }
        }
        total
    }

    let mut updated = 0;
    for item in &list {
        let count = sum(item.id, &direct, &children, &mut HashSet::new());
        if count as u64 == item.archive_count as u64 {
            continue;
        }
        sqlx::query("UPDATE categories SET archive_count = ? WHERE id = ?")
            .bind(count)
            .bind(item.id)
            .execute(pool)
            .await?;
        updated += 1;
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::test_pool;

    fn cat(id: u32, parent_id: u32, sort: u32, status: u8) -> Category {
        Category {
            id,
            created_time: None,
            updated_time: None,
            title: format!("分类{id}"),
            seo_title: String::new(),
            keywords: String::new(),
            url_token: format!("c{id}"),
            description: String::new(),
            content: None,
            module_id: 1,
            parent_id,
            kind: 1,
            sort,
            template: String::new(),
            detail_template: String::new(),
            is_inherit: 0,
            images: None,
            logo: String::new(),
            extra: None,
            archive_count: 0,
            status,
        }
    }

    #[test]
    fn builds_tree_with_sort() {
        let list = vec![
            cat(1, 0, 99, 1),
            cat(3, 1, 2, 1),
            cat(2, 1, 1, 1),
            cat(4, 3, 1, 1),
        ];
        let tree = tree(&list);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].category.id, 1);
        // sort 小的在前
        assert_eq!(tree[0].children[0].category.id, 2);
        assert_eq!(tree[0].children[1].category.id, 3);
        assert_eq!(tree[0].children[1].children[0].category.id, 4);
    }

    #[test]
    fn sub_ids_skips_inactive() {
        let list = vec![
            cat(1, 0, 1, 1),
            cat(2, 1, 1, 1),
            cat(3, 1, 1, 0),
            cat(4, 2, 1, 1),
        ];
        assert_eq!(sub_ids(&list, 1), vec![2, 4]);
        assert_eq!(self_and_sub_ids(&list, 1), vec![1, 2, 4]);
        assert!(sub_ids(&list, 99).is_empty());
    }

    #[test]
    fn parses_legacy_images() {
        let raw = vec![
            (None, 0usize),
            (Some(""), 0),
            (Some("{}"), 0),
            (Some(r#"{"a.webp"}"#), 1),
            (Some(r#"{"a.webp","b.webp"}"#), 2),
            (Some(r#"["c.webp"]"#), 1),
        ];
        for (value, expect) in raw {
            assert_eq!(json_string_list(value).len(), expect, "输入：{value:?}");
        }
    }

    #[tokio::test]
    async fn reads_real_categories() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };

        let expect_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM categories")
            .fetch_one(&pool)
            .await
            .unwrap();
        let list = list_all(&pool).await.unwrap();
        assert_eq!(list.len() as i64, expect_total);

        // 排序口径：sort ASC, id ASC
        let mut sorted = list.clone();
        sorted.sort_by_key(|c| (c.sort, c.id));
        assert_eq!(
            list.iter().map(|c| c.id).collect::<Vec<_>>(),
            sorted.iter().map(|c| c.id).collect::<Vec<_>>()
        );

        // 树形：根节点加上所有非根节点应覆盖全部行
        let tree = tree(&list);
        fn count(nodes: &[CategoryNode]) -> usize {
            nodes.iter().map(|n| 1 + count(&n.children)).sum()
        }
        assert_eq!(count(&tree), list.len());

        let first = &list[0];
        let by_id = get(&pool, first.id).await.unwrap().expect("分类应存在");
        assert_eq!(by_id.title, first.title);

        let by_token = get_by_token(&pool, &first.url_token)
            .await
            .unwrap()
            .expect("按 url_token 应能取到");
        assert_eq!(by_token.id, first.id);

        assert!(get(&pool, u32::MAX).await.unwrap().is_none());
    }
}

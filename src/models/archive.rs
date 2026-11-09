//! 内容模型（表 `archives` + 正文表 `archive_data`）。
//!
//! 列表查询采用两段式做法：
//! 先按排序取一页 ID，再按 ID 回捞整行并按原顺序重排，
//! 避免大表 `SELECT *` 参与排序带来的临时表与顺序漂移。

use std::collections::HashMap;

use serde::Serialize;
use sqlx::mysql::{MySql, MySqlPool};
use sqlx::{FromRow, QueryBuilder};

use super::{json_string_list, Page};

/// 内容。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Archive {
    pub id: i64,
    pub parent_id: i64,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub title: String,
    pub seo_title: String,
    pub url_token: String,
    pub keywords: String,
    pub description: String,
    pub module_id: u32,
    pub category_id: u32,
    pub place_id: i32,
    pub views: u32,
    pub comment_count: u32,
    pub favorite_count: u32,
    #[serde(serialize_with = "super::serialize_images")]
    pub images: Option<String>,
    pub template: String,
    pub canonical_url: String,
    pub fixed_link: Option<String>,
    pub user_id: u32,
    pub price: i64,
    pub stock: i64,
    /// 阅读所需会员等级，0 = 公开
    pub read_level: i32,
    pub password: String,
    pub sort: u32,
    pub has_pseudo: i8,
    pub keyword_id: i64,
    pub origin_url: String,
    pub origin_title: String,
    /// 来源：0 手工、1 采集、2 AI 生成
    pub origin_id: u8,
}

impl Archive {
    pub fn image_list(&self) -> Vec<String> {
        json_string_list(self.images.as_deref())
    }

    /// 封面：第一张图。
    pub fn cover(&self) -> String {
        self.image_list().into_iter().next().unwrap_or_default()
    }

    /// 是否需要密码才能查看正文。
    pub fn has_password(&self) -> bool {
        !self.password.is_empty()
    }
}

/// 列表排序（白名单，不允许外部拼 SQL）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    #[default]
    IdDesc,
    IdAsc,
    CreatedDesc,
    CreatedAsc,
    UpdatedDesc,
    ViewsDesc,
    SortDesc,
}

impl Order {
    pub fn sql(self) -> &'static str {
        match self {
            Order::IdDesc => "id DESC",
            Order::IdAsc => "id ASC",
            Order::CreatedDesc => "created_time DESC, id DESC",
            Order::CreatedAsc => "created_time ASC, id ASC",
            Order::UpdatedDesc => "updated_time DESC, id DESC",
            Order::ViewsDesc => "views DESC, id DESC",
            Order::SortDesc => "sort DESC, id DESC",
        }
    }

    /// 解析后台/接口传进来的排序串，非法值回落到默认。
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "id asc" | "id_asc" => Order::IdAsc,
            "created_time desc" | "created_desc" | "time desc" => Order::CreatedDesc,
            "created_time asc" | "created_asc" | "time asc" => Order::CreatedAsc,
            "updated_time desc" | "updated_desc" => Order::UpdatedDesc,
            "views desc" | "views_desc" | "hot" => Order::ViewsDesc,
            "sort desc" | "sort_desc" => Order::SortDesc,
            _ => Order::IdDesc,
        }
    }
}

/// 列表筛选条件。
#[derive(Debug, Clone, Default)]
pub struct ArchiveQuery {
    pub module_id: Option<u32>,
    /// 分类范围（通常是「自身 + 全部下级」）
    pub category_ids: Vec<u32>,
    /// 标题模糊搜索
    pub keyword: Option<String>,
    pub user_id: Option<u32>,
    pub exclude_id: Option<i64>,
    /// 标签筛选（`tag_data` 关联表）
    pub tag_id: Option<u32>,
}

impl ArchiveQuery {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn module(mut self, module_id: u32) -> Self {
        self.module_id = Some(module_id);
        self
    }

    pub fn categories(mut self, ids: Vec<u32>) -> Self {
        self.category_ids = ids;
        self
    }

    pub fn keyword(mut self, keyword: &str) -> Self {
        let keyword = keyword.trim();
        if !keyword.is_empty() {
            self.keyword = Some(keyword.to_string());
        }
        self
    }

    pub fn user(mut self, user_id: u32) -> Self {
        self.user_id = Some(user_id);
        self
    }

    pub fn tag(mut self, tag_id: u32) -> Self {
        self.tag_id = Some(tag_id);
        self
    }
}

fn push_filters(builder: &mut QueryBuilder<'_, MySql>, query: &ArchiveQuery) {
    if let Some(module_id) = query.module_id {
        builder.push(" AND module_id = ").push_bind(module_id);
    }
    if !query.category_ids.is_empty() {
        builder.push(" AND category_id IN (");
        let mut separated = builder.separated(", ");
        for id in &query.category_ids {
            separated.push_bind(*id);
        }
        separated.push_unseparated(")");
    }
    if let Some(keyword) = query.keyword.as_deref() {
        builder
            .push(" AND title LIKE ")
            .push_bind(format!("%{keyword}%"));
    }
    if let Some(user_id) = query.user_id {
        builder.push(" AND user_id = ").push_bind(user_id);
    }
    if let Some(exclude_id) = query.exclude_id {
        builder.push(" AND id <> ").push_bind(exclude_id);
    }
    if let Some(tag_id) = query.tag_id {
        builder
            .push(" AND id IN (SELECT item_id FROM tag_data WHERE tag_id = ")
            .push_bind(tag_id)
            .push(")");
    }
}

/// 分页查询内容列表。
pub async fn list(
    pool: &MySqlPool,
    query: &ArchiveQuery,
    order: Order,
    page: u32,
    page_size: u32,
) -> anyhow::Result<Page<Archive>> {
    let page = page.max(1);
    let page_size = if page_size == 0 {
        20
    } else {
        page_size.clamp(1, 200)
    };
    let offset = (page as i64 - 1) * page_size as i64;

    let mut count_builder = QueryBuilder::<MySql>::new("SELECT COUNT(*) FROM archives WHERE 1 = 1");
    push_filters(&mut count_builder, query);
    let total: i64 = count_builder.build_query_scalar().fetch_one(pool).await?;

    if total == 0 {
        return Ok(Page::empty(page, page_size));
    }

    let mut ids_builder = QueryBuilder::<MySql>::new("SELECT id FROM archives WHERE 1 = 1");
    push_filters(&mut ids_builder, query);
    ids_builder
        .push(" ORDER BY ")
        .push(order.sql())
        .push(" LIMIT ")
        .push_bind(page_size as i64)
        .push(" OFFSET ")
        .push_bind(offset);
    let ids: Vec<i64> = ids_builder.build_query_scalar().fetch_all(pool).await?;

    let rows = fetch_by_ids(pool, &ids).await?;
    Ok(Page::new(rows, total, page, page_size))
}

/// 按给定 ID 批量取行，并保持传入顺序。
pub async fn fetch_by_ids(pool: &MySqlPool, ids: &[i64]) -> anyhow::Result<Vec<Archive>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut builder = QueryBuilder::<MySql>::new("SELECT * FROM archives WHERE id IN (");
    {
        let mut separated = builder.separated(", ");
        for id in ids {
            separated.push_bind(*id);
        }
        separated.push_unseparated(")");
    }
    let rows: Vec<Archive> = builder.build_query_as().fetch_all(pool).await?;

    let mut by_id: HashMap<i64, Archive> = rows.into_iter().map(|row| (row.id, row)).collect();
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

/// 按 ID 取单条内容。
pub async fn get(pool: &MySqlPool, id: i64) -> anyhow::Result<Option<Archive>> {
    let row = sqlx::query_as::<_, Archive>("SELECT * FROM archives WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// 同模块、同栏目下的上一篇 / 下一篇（按 ID 排序）。
pub async fn adjacent(
    pool: &MySqlPool,
    id: i64,
    module_id: u32,
    category_id: u32,
    previous: bool,
) -> anyhow::Result<Option<Archive>> {
    let mut builder = QueryBuilder::<MySql>::new("SELECT * FROM archives WHERE 1 = 1");
    builder.push(" AND module_id = ").push_bind(module_id);
    if category_id > 0 {
        builder.push(" AND category_id = ").push_bind(category_id);
    }
    if previous {
        builder
            .push(" AND id < ")
            .push_bind(id)
            .push(" ORDER BY id DESC LIMIT 1");
    } else {
        builder
            .push(" AND id > ")
            .push_bind(id)
            .push(" ORDER BY id ASC LIMIT 1");
    }
    let row = builder.build_query_as::<Archive>().fetch_optional(pool).await?;
    Ok(row)
}

/// 按 url_token 取单条内容。
pub async fn get_by_token(pool: &MySqlPool, token: &str) -> anyhow::Result<Option<Archive>> {
    let row = sqlx::query_as::<_, Archive>(
        "SELECT * FROM archives WHERE url_token = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(token)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// 取正文（`archive_data`）。
pub async fn content(pool: &MySqlPool, id: i64) -> anyhow::Result<Option<String>> {
    let row: Option<Option<String>> =
        sqlx::query_scalar("SELECT content FROM archive_data WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row.flatten())
}

/// 取详情（内容 + 正文）。
pub async fn detail(pool: &MySqlPool, id: i64) -> anyhow::Result<Option<(Archive, String)>> {
    let Some(archive) = get(pool, id).await? else {
        return Ok(None);
    };
    let content = content(pool, id).await?.unwrap_or_default();
    Ok(Some((archive, content)))
}

/// 浏览量 +1。
pub async fn add_views(pool: &MySqlPool, id: i64) -> anyhow::Result<()> {
    sqlx::query("UPDATE archives SET views = views + 1 WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::category;
    use crate::models::test_pool;

    #[tokio::test]
    async fn paginates_like_go_reference() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };

        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM archives")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(total > 0, "测试库应至少有一条内容");

        let first = list(&pool, &ArchiveQuery::new(), Order::IdDesc, 1, 10)
            .await
            .unwrap();
        assert_eq!(first.total, total);
        assert_eq!(first.page, 1);
        assert_eq!(first.page_size, 10);
        assert_eq!(first.list.len() as i64, total.min(10));
        assert_eq!(first.total_page as i64, (total + 9) / 10);
        // 默认 id 倒序
        let ids: Vec<i64> = first.list.iter().map(|a| a.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(ids, sorted);

        // 倒序首条 = 全表最大 id；升序首条 = 全表最小 id
        let min_id: i64 = sqlx::query_scalar("SELECT MIN(id) FROM archives")
            .fetch_one(&pool)
            .await
            .unwrap();
        let max_id: i64 = sqlx::query_scalar("SELECT MAX(id) FROM archives")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(first.list.first().map(|a| a.id), Some(max_id));

        let asc = list(&pool, &ArchiveQuery::new(), Order::IdAsc, 1, 10)
            .await
            .unwrap();
        assert_eq!(asc.list.first().map(|a| a.id), Some(min_id));

        // 越界页返回空列表但保留 total
        let far = list(&pool, &ArchiveQuery::new(), Order::IdDesc, 9999, 10)
            .await
            .unwrap();
        assert!(far.list.is_empty());
        assert_eq!(far.total, total);
    }

    #[tokio::test]
    async fn filters_by_module_category_and_keyword() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };

        let Some(sample) = get(&pool, 1).await.unwrap() else {
            return;
        };

        // 模块筛选
        let by_module = list(
            &pool,
            &ArchiveQuery::new().module(sample.module_id),
            Order::IdDesc,
            1,
            200,
        )
        .await
        .unwrap();
        assert!(by_module
            .list
            .iter()
            .all(|a| a.module_id == sample.module_id));

        // 分类筛选（自身 + 子级）
        let categories = category::list_all(&pool).await.unwrap();
        let scope = category::self_and_sub_ids(&categories, sample.category_id);
        let by_cate = list(
            &pool,
            &ArchiveQuery::new().categories(scope.clone()),
            Order::IdDesc,
            1,
            200,
        )
        .await
        .unwrap();
        assert!(by_cate.list.iter().all(|a| scope.contains(&a.category_id)));

        // 关键词（取标题前两个字，保证能命中并验证全量包含）
        let keyword: String = sample.title.chars().take(2).collect();
        let by_keyword = list(
            &pool,
            &ArchiveQuery::new().keyword(&keyword),
            Order::IdDesc,
            1,
            200,
        )
        .await
        .unwrap();
        assert!(by_keyword.total > 0, "关键词 {keyword} 应能命中");
        assert!(by_keyword.list.iter().all(|a| a.title.contains(&keyword)));

        // 排除指定 ID
        let excluded = list(
            &pool,
            &ArchiveQuery {
                exclude_id: Some(sample.id),
                ..Default::default()
            },
            Order::IdDesc,
            1,
            200,
        )
        .await
        .unwrap();
        assert!(excluded.list.iter().all(|a| a.id != sample.id));
    }

    #[tokio::test]
    async fn detail_returns_content_and_counts_views() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let Some((archive, content)) = detail(&pool, 1).await.unwrap() else {
            return;
        };

        assert_eq!(archive.id, 1);
        let expect: Option<String> =
            sqlx::query_scalar("SELECT content FROM archive_data WHERE id = 1")
                .fetch_optional(&pool)
                .await
                .unwrap()
                .flatten();
        assert_eq!(content, expect.unwrap_or_default());

        // 浏览量自增（读-写-还原，避免污染测试库）
        let before = get(&pool, 1).await.unwrap().unwrap().views;
        add_views(&pool, 1).await.unwrap();
        let after = get(&pool, 1).await.unwrap().unwrap().views;
        assert_eq!(after, before + 1);
        sqlx::query("UPDATE archives SET views = ? WHERE id = 1")
            .bind(before)
            .execute(&pool)
            .await
            .unwrap();

        assert!(get(&pool, i64::MAX).await.unwrap().is_none());
        assert!(detail(&pool, i64::MAX).await.unwrap().is_none());
    }

    #[test]
    fn parses_order_whitelist() {
        assert_eq!(Order::parse("id asc"), Order::IdAsc);
        assert_eq!(Order::parse("views desc"), Order::ViewsDesc);
        assert_eq!(Order::parse("sort desc"), Order::SortDesc);
        assert_eq!(Order::parse("id desc; drop table archives"), Order::IdDesc);
        assert_eq!(Order::parse(""), Order::IdDesc);
        assert_eq!(Order::IdDesc.sql(), "id DESC");
    }
}

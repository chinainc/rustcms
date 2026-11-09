//! 前台内容数据访问（批次 A）。
//!
//! 覆盖内容 / 栏目 / 标签 / 城市站 / 导航 / 友链的只读查询，供前台接口使用。
//!
//! 只服务**前台只读**接口；写库动作（友链新增/删除）复用 `models::link`，
//! 正文/分类/模块的写入不在本批范围。
//!
//! 路径约定：这些接口亦可挂在 `/api` party 下，
//! 本批按主线要求把路由落在站点根（`/archive/list` 等），
//! 同时在 `api_content::router` 里补了 `/api/...` 别名。

use std::collections::HashMap;

use serde_json::{json, Map, Value};
use sqlx::mysql::{MySql, MySqlPool};
use sqlx::{QueryBuilder, Row};

use super::{
    archive::Archive, category::Category, link::Link, module::Module, nav::NavNode, place::Place,
    site_setting, tag::Tag, url,
};

/// 城市站查询用的列集（`decimal` 列先 `CAST AS CHAR`）。
const PLACE_COLUMNS: &str = "id, created_time, updated_time, title, seo_title, keywords, \
    url_token, description, content, parent_id, sort, template, is_inherit, images, logo, extra, \
    CAST(latitude AS CHAR) AS latitude, CAST(longitude AS CHAR) AS longitude, timezone, status";

// ---------------------------------------------------------------------------
// 排序白名单
// ---------------------------------------------------------------------------

/// 排序串白名单：只接受「列名 [asc|desc]」。
///
/// 按既有约定 `ParseOrderBy`：非法片段整段丢弃，
/// `rand` 是唯一放行的表达式，列名不带点时补表前缀。
pub fn parse_order(raw: &str, prefix: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    for item in raw.split(',') {
        let fields: Vec<&str> = item.split_whitespace().collect();
        if fields.is_empty() || fields.len() > 2 {
            continue;
        }
        let column = fields[0];
        if column.eq_ignore_ascii_case("rand") {
            if fields.len() == 1 {
                parts.push("RAND()".to_string());
            }
            continue;
        }
        if !is_order_column(column) {
            continue;
        }
        let mut column = column.to_string();
        if fields.len() == 2 {
            let dir = fields[1];
            if !(dir.eq_ignore_ascii_case("asc") || dir.eq_ignore_ascii_case("desc")) {
                continue;
            }
            column = format!("{column} {dir}");
        }
        if !prefix.is_empty() && !column.contains('.') {
            column = format!("{prefix}.{column}");
        }
        parts.push(column);
    }
    parts.join(", ")
}

/// 列名是否合法：允许 `列` 或 `表.列`（可带反引号）。
fn is_order_column(column: &str) -> bool {
    let segments: Vec<&str> = column.split('.').collect();
    if segments.is_empty() || segments.len() > 2 {
        return false;
    }
    segments.iter().all(|segment| {
        let segment = segment.trim_matches('`');
        !segment.is_empty()
            && segment
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    })
}

// ---------------------------------------------------------------------------
// 内容（archives）
// ---------------------------------------------------------------------------

/// 前台内容列表的查询条件（对应 `ApiArchiveListRequest`）。
#[derive(Debug, Clone, Default)]
pub struct ArchiveListQuery {
    pub id: Option<i64>,
    pub parent_id: Option<i64>,
    pub place_id: Option<i64>,
    pub module_id: Option<u32>,
    /// 作者（会员）ID。
    pub author_id: Option<u32>,
    pub category_ids: Vec<u32>,
    pub exclude_category_ids: Vec<u32>,
    pub tag_ids: Vec<u32>,
    /// 标记（`archive_flags.flag`）。
    pub flag: Option<String>,
    /// 标题关键字。
    pub q: Option<String>,
    /// 多分类模式（`settings.content.multi_category = 1`）：
    /// 分类筛选走 `archive_categories` 关联表，而不是 `archives.category_id`。
    pub multi_category: bool,
    pub page: u32,
    pub limit: u32,
    pub offset: u32,
    /// 从草稿表 `archive_drafts` 读（`draft` 参数）。
    pub draft: bool,
}

/// 分类筛选列：多分类模式走 `archive_categories` 关联表，否则走主表。
fn category_column(multi_category: bool) -> &'static str {
    if multi_category {
        "archive_categories.category_id"
    } else {
        "archives.`category_id`"
    }
}

/// 估算内容总数（对齐 `GetExplainCount`）。
///
/// 优先走 `EXPLAIN` 读 rows（避免大表全量 count），失败再回退 `COUNT(*)`。
pub async fn estimated_archive_count(pool: &MySqlPool) -> i64 {
    if let Some(value) = explain_rows(pool).await {
        return value;
    }
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM archives")
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

/// 读 `EXPLAIN SELECT id FROM archives` 的 rows 列。
async fn explain_rows(pool: &MySqlPool) -> Option<i64> {
    let rows = sqlx::query("EXPLAIN SELECT id FROM archives")
        .fetch_all(pool)
        .await
        .ok()?;
    for row in rows {
        if let Ok(Some(value)) = row.try_get::<Option<String>, _>("rows") {
            if let Some(parsed) = parse_leading_digits(&value) {
                return Some(parsed);
            }
        } else if let Ok(Some(value)) = row.try_get::<Option<i64>, _>("rows") {
            return Some(value);
        }
    }
    None
}

/// `EXPLAIN` 的 rows 列可能是 `123` 或 `123 (0%)`（分区表），取前导数字。
fn parse_leading_digits(value: &str) -> Option<i64> {
    let digits: String = value.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    digits.parse::<i64>().ok()
}

/// 拼 WHERE 条件（计数与取 ID 两段共用，保证口径一致）。
///
/// - `multi_category` 打开时分类条件走 `archive_categories` 关联表；
/// - `prefix_search` 为真时关键字只做前缀匹配（内容超过 10 万时的口径）。
fn push_archive_filters(
    builder: &mut QueryBuilder<'_, MySql>,
    query: &ArchiveListQuery,
    prefix_search: bool,
) {
    // 草稿表没有 archive_categories 关联，多分类只在正式内容上生效。
    let multi = query.multi_category && !query.draft;
    if let Some(id) = query.id {
        builder.push(" AND archives.id = ").push_bind(id);
    }
    if let Some(parent_id) = query.parent_id {
        builder.push(" AND archives.parent_id = ").push_bind(parent_id);
    }
    if let Some(place_id) = query.place_id {
        builder.push(" AND archives.place_id = ").push_bind(place_id);
    }
    if let Some(module_id) = query.module_id {
        builder.push(" AND archives.module_id = ").push_bind(module_id);
    }
    if let Some(author_id) = query.author_id {
        builder.push(" AND archives.user_id = ").push_bind(author_id);
    }
    if !query.category_ids.is_empty() {
        builder
            .push(" AND ")
            .push(category_column(multi))
            .push(" IN (");
        let mut separated = builder.separated(", ");
        for id in &query.category_ids {
            separated.push_bind(*id);
        }
        separated.push_unseparated(")");
    }
    if !query.exclude_category_ids.is_empty() {
        builder
            .push(" AND ")
            .push(category_column(multi))
            .push(" NOT IN (");
        let mut separated = builder.separated(", ");
        for id in &query.exclude_category_ids {
            separated.push_bind(*id);
        }
        separated.push_unseparated(")");
    }
    if !query.tag_ids.is_empty() {
        builder.push(" AND archives.id IN (SELECT item_id FROM tag_data WHERE tag_id IN (");
        let mut separated = builder.separated(", ");
        for id in &query.tag_ids {
            separated.push_bind(*id);
        }
        separated.push_unseparated("))");
    }
    if let Some(flag) = query.flag.as_deref().map(str::trim) {
        if !flag.is_empty() {
            builder
                .push(" AND archives.id IN (SELECT archive_id FROM archive_flags WHERE `flag` = ")
                .push_bind(flag.to_string())
                .push(")");
        }
    }
    if let Some(q) = query.q.as_deref().map(str::trim) {
        if !q.is_empty() {
            let pattern = if prefix_search {
                format!("{q}%")
            } else {
                format!("%{q}%")
            };
            builder.push(" AND archives.title LIKE ").push_bind(pattern);
        }
    }
}

/// 分页取内容（对齐 `GetArchiveList`：先取 ID 再回捞整行）。
pub async fn archive_list(
    pool: &MySqlPool,
    query: &ArchiveListQuery,
    order: &str,
) -> anyhow::Result<(Vec<Archive>, i64)> {
    let multi = query.multi_category && !query.draft;
    let base_source = if query.draft {
        "archive_drafts AS archives"
    } else {
        "archives"
    };
    // 多分类：分类条件落在 archive_categories 上，需要 join 关联表
    // （对齐 `INNER JOIN archive_categories ON archives.id = archive_categories.archive_id`）。
    let joined = multi && (!query.category_ids.is_empty() || !query.exclude_category_ids.is_empty());
    let source = if joined {
        format!(
            "{base_source} INNER JOIN archive_categories \
             ON archives.id = archive_categories.archive_id"
        )
    } else {
        base_source.to_string()
    };
    let table = if query.draft { "archive_drafts" } else { "archives" };

    // 内容超过 10 万时关键字只匹配开头（对齐 `GetExplainCount` 分支）。
    let keyword = query.q.as_deref().map(str::trim).unwrap_or_default();
    let prefix_search = !keyword.is_empty() && estimated_archive_count(pool).await > 100_000;

    let mut count_builder = QueryBuilder::<MySql>::new(format!(
        "SELECT COUNT(DISTINCT archives.id) FROM {source}"
    ));
    push_archive_filters(&mut count_builder, query, prefix_search);
    let total: i64 = count_builder.build_query_scalar().fetch_one(pool).await?;
    if total == 0 {
        return Ok((Vec::new(), 0));
    }

    let mut ids_builder = QueryBuilder::<MySql>::new(format!("SELECT archives.id FROM {source}"));
    push_archive_filters(&mut ids_builder, query, prefix_search);
    if joined {
        // 一个内容可能命中多个分类，去重（对齐 `Group("archives.id")`）。
        ids_builder.push(" GROUP BY archives.id");
    }
    let order = order.trim();
    if !order.is_empty() {
        ids_builder.push(" ORDER BY ").push(order);
    }
    ids_builder
        .push(" LIMIT ")
        .push_bind(query.limit.max(1) as i64)
        .push(" OFFSET ")
        .push_bind(query.offset as i64);
    let ids: Vec<i64> = ids_builder.build_query_scalar().fetch_all(pool).await?;

    let rows = fetch_archive_rows(pool, table, &ids).await?;
    Ok((rows, total))
}

/// 按 ID 批量取整行并保持传入顺序。
pub async fn fetch_archive_rows(
    pool: &MySqlPool,
    table: &str,
    ids: &[i64],
) -> anyhow::Result<Vec<Archive>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut builder = QueryBuilder::<MySql>::new(format!("SELECT * FROM `{table}` WHERE id IN ("));
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

/// 取内容上的标记（`archive_flags`），返回 `内容ID -> "a,b"`。
pub async fn archive_flags(
    pool: &MySqlPool,
    ids: &[i64],
) -> anyhow::Result<HashMap<i64, String>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut builder = QueryBuilder::<MySql>::new(
        "SELECT CAST(archive_id AS SIGNED) AS archive_id, GROUP_CONCAT(`flag`) AS flags \
         FROM archive_flags WHERE archive_id IN (",
    );
    {
        let mut separated = builder.separated(", ");
        for id in ids {
            separated.push_bind(*id);
        }
        separated.push_unseparated(") GROUP BY archive_id");
    }
    let rows: Vec<(i64, Option<String>)> = builder.build_query_as().fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, flags)| flags.map(|value| (id, value)))
        .collect())
}

/// 取内容上的标签（`tag_data` 关联 `tags`）。
pub async fn archive_tags(
    pool: &MySqlPool,
    ids: &[i64],
) -> anyhow::Result<HashMap<i64, Vec<Tag>>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut builder = QueryBuilder::<MySql>::new(
        "SELECT CAST(item_id AS SIGNED) AS item_id, tag_id FROM tag_data WHERE item_id IN (",
    );
    {
        let mut separated = builder.separated(", ");
        for id in ids {
            separated.push_bind(*id);
        }
        separated.push_unseparated(")");
    }
    let pairs: Vec<(i64, u32)> = builder.build_query_as().fetch_all(pool).await?;
    if pairs.is_empty() {
        return Ok(HashMap::new());
    }

    let mut tag_ids: Vec<u32> = pairs.iter().map(|(_, tag_id)| *tag_id).collect();
    tag_ids.sort_unstable();
    tag_ids.dedup();

    let mut tag_builder = QueryBuilder::<MySql>::new("SELECT * FROM tags WHERE id IN (");
    {
        let mut separated = tag_builder.separated(", ");
        for id in &tag_ids {
            separated.push_bind(*id);
        }
        separated.push_unseparated(")");
    }
    let tags: Vec<Tag> = tag_builder.build_query_as().fetch_all(pool).await?;
    let by_id: HashMap<u32, Tag> = tags.into_iter().map(|tag| (tag.id, tag)).collect();

    let mut result: HashMap<i64, Vec<Tag>> = HashMap::new();
    for (item_id, tag_id) in pairs {
        if let Some(tag) = by_id.get(&tag_id) {
            result.entry(item_id).or_default().push(tag.clone());
        }
    }
    Ok(result)
}

/// 上一篇 / 下一篇（同一栏目内按 ID 前后找）。
pub async fn neighbor_archive(
    pool: &MySqlPool,
    category_id: u32,
    id: i64,
    next: bool,
) -> anyhow::Result<Option<Archive>> {
    let mut builder = QueryBuilder::<MySql>::new("SELECT id FROM archives WHERE 1 = 1");
    if category_id > 0 {
        builder.push(" AND category_id = ").push_bind(category_id);
    }
    if next {
        builder
            .push(" AND id > ")
            .push_bind(id)
            .push(" ORDER BY id ASC LIMIT 1");
    } else {
        builder
            .push(" AND id < ")
            .push_bind(id)
            .push(" ORDER BY id DESC LIMIT 1");
    }
    let found: Option<i64> = builder.build_query_scalar().fetch_optional(pool).await?;
    let Some(found) = found else {
        return Ok(None);
    };
    Ok(fetch_archive_rows(pool, "archives", &[found])
        .await?
        .into_iter()
        .next())
}

/// 相关文档（`archive_relations`）；表不存在时按空处理。
pub async fn related_archives(
    pool: &MySqlPool,
    id: i64,
    limit: u32,
) -> anyhow::Result<Vec<Archive>> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT CAST(relation_id AS SIGNED) FROM archive_relations WHERE archive_id = ? LIMIT ?",
    )
    .bind(id)
    .bind(limit.max(1) as i64)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    fetch_archive_rows(pool, "archives", &ids).await
}

// ---------------------------------------------------------------------------
// 相关文档（`type=related`）
// ---------------------------------------------------------------------------

/// 相关文档的关联方式（`like` 参数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveRelation {
    /// `like=keywords`：按关键字相似度取。
    Keywords,
    /// `like=relation`：按后台手动配置的关联文档取。
    Relation,
    /// `like=tag`：按相同标签取。
    Tag,
    /// `like=id`：当前内容前后各取一半。
    Neighbors,
}

impl ArchiveRelation {
    /// 解析 `like` 参数；空值返回 `None`（走默认逻辑）。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "keywords" => Some(Self::Keywords),
            "relation" => Some(Self::Relation),
            "tag" => Some(Self::Tag),
            "id" => Some(Self::Neighbors),
            _ => None,
        }
    }
}

/// 相关文档查询条件（`type=related` 分支）。
#[derive(Debug, Clone, Default)]
pub struct RelatedQuery {
    /// 基准内容 ID。
    pub archive_id: i64,
    /// 栏目 ID（0 = 不限）。
    pub category_id: u32,
    /// 模型 ID（0 = 不限）。
    pub module_id: u32,
    /// 排除的栏目。
    pub exclude_category_ids: Vec<u32>,
    /// 关键字（`like=keywords` 用）。
    pub keywords: String,
    pub limit: u32,
    pub offset: u32,
    /// 多分类模式（分类条件走 `archive_categories`）。
    pub multi_category: bool,
}

/// 组装相关文档的 FROM 子句。
///
/// 返回 `(source, need_group)`：`need_group` 为真时同一内容可能命中多行，
/// 需要 `GROUP BY archives.id` 去重（对齐 `Group("archives.id")`）。
fn related_source(query: &RelatedQuery, prefix_join: &str) -> (String, bool) {
    let mut source = String::from("archives");
    let mut need_group = false;
    if !prefix_join.is_empty() {
        source.push(' ');
        source.push_str(prefix_join);
        need_group = true;
    }
    if query.multi_category
        && (query.category_id > 0 || !query.exclude_category_ids.is_empty())
    {
        source.push_str(
            " INNER JOIN archive_categories ON archives.id = archive_categories.archive_id",
        );
        need_group = true;
    }
    (source, need_group)
}

/// 追加相关文档的栏目/模型范围条件（三个 where 分支）。
fn push_related_scope(builder: &mut QueryBuilder<'_, MySql>, query: &RelatedQuery) {
    if query.category_id > 0 {
        builder
            .push(" AND ")
            .push(category_column(query.multi_category))
            .push(" = ")
            .push_bind(query.category_id);
    }
    if query.module_id > 0 {
        builder
            .push(" AND archives.module_id = ")
            .push_bind(query.module_id);
    }
    if !query.exclude_category_ids.is_empty() {
        builder
            .push(" AND ")
            .push(category_column(query.multi_category))
            .push(" NOT IN (");
        let mut separated = builder.separated(", ");
        for id in &query.exclude_category_ids {
            separated.push_bind(*id);
        }
        separated.push_unseparated(")");
    }
}

/// 相关文档列表（对齐 `ApiGetArchives` 里 `type == "related"` 分支）。
pub async fn related_list(
    pool: &MySqlPool,
    like: &str,
    query: &RelatedQuery,
) -> anyhow::Result<(Vec<Archive>, i64)> {
    match ArchiveRelation::parse(like) {
        Some(ArchiveRelation::Keywords) => related_by_keywords(pool, query).await,
        Some(ArchiveRelation::Relation) => related_by_relation(pool, query).await,
        Some(ArchiveRelation::Tag) => related_by_tag(pool, query).await,
        Some(ArchiveRelation::Neighbors) => related_by_neighbors(pool, query).await,
        None => {
            // 默认：先取后台配置的关联文档，没有再退回「前后各取一半」。
            let rows = related_archives(pool, query.archive_id, query.limit.max(1)).await?;
            if !rows.is_empty() {
                return Ok((rows, 0));
            }
            related_by_neighbors(pool, query).await
        }
    }
}

/// `like=keywords`：关键字相似。
async fn related_by_keywords(
    pool: &MySqlPool,
    query: &RelatedQuery,
) -> anyhow::Result<(Vec<Archive>, i64)> {
    let (source, need_group) = related_source(query, "");
    let mut ids_builder = QueryBuilder::<MySql>::new(format!("SELECT archives.id FROM {source}"));
    ids_builder.push(" WHERE 1 = 1");
    push_related_scope(&mut ids_builder, query);
    ids_builder
        .push(" AND archives.keywords LIKE ")
        .push_bind(format!("%{}%", query.keywords));
    ids_builder
        .push(" AND archives.id != ")
        .push_bind(query.archive_id);
    if need_group {
        ids_builder.push(" GROUP BY archives.id");
    }
    ids_builder.push(" ORDER BY archives.id DESC");
    ids_builder
        .push(" LIMIT ")
        .push_bind(query.limit.max(1) as i64)
        .push(" OFFSET ")
        .push_bind(query.offset as i64);
    let ids: Vec<i64> = ids_builder.build_query_scalar().fetch_all(pool).await?;
    let rows = fetch_archive_rows(pool, "archives", &ids).await?;
    // keywords 分支不统计总数（total 保持 0）。
    Ok((rows, 0))
}

/// `like=relation`：后台手动配置的关联文档。
async fn related_by_relation(
    pool: &MySqlPool,
    query: &RelatedQuery,
) -> anyhow::Result<(Vec<Archive>, i64)> {
    let has_scope = query.category_id > 0
        || query.module_id > 0
        || !query.exclude_category_ids.is_empty();
    if !has_scope {
        let rows = related_archives(pool, query.archive_id, query.limit.max(1)).await?;
        return Ok((rows, 0));
    }
    let join = format!(
        "INNER JOIN archive_relations t ON archives.id = t.relation_id \
         AND t.archive_id = {} AND archives.id != {}",
        query.archive_id, query.archive_id
    );
    let (source, need_group) = related_source(query, &join);

    let mut count_builder = QueryBuilder::<MySql>::new(format!(
        "SELECT COUNT(DISTINCT archives.id) FROM {source}"
    ));
    count_builder.push(" WHERE 1 = 1");
    push_related_scope(&mut count_builder, query);
    let total: i64 = count_builder.build_query_scalar().fetch_one(pool).await?;

    let mut ids_builder = QueryBuilder::<MySql>::new(format!("SELECT archives.id FROM {source}"));
    ids_builder.push(" WHERE 1 = 1");
    push_related_scope(&mut ids_builder, query);
    if need_group {
        ids_builder.push(" GROUP BY archives.id");
    }
    ids_builder.push(" ORDER BY archives.id DESC");
    ids_builder
        .push(" LIMIT ")
        .push_bind(query.limit.max(1) as i64)
        .push(" OFFSET ")
        .push_bind(query.offset as i64);
    let ids: Vec<i64> = ids_builder.build_query_scalar().fetch_all(pool).await?;
    let rows = fetch_archive_rows(pool, "archives", &ids).await?;
    Ok((rows, total))
}

/// `like=tag`：同标签的内容。
async fn related_by_tag(
    pool: &MySqlPool,
    query: &RelatedQuery,
) -> anyhow::Result<(Vec<Archive>, i64)> {
    let tag_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT CAST(tag_id AS SIGNED) FROM tag_data WHERE item_id = ?",
    )
    .bind(query.archive_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    if tag_ids.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let tag_list = tag_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<String>>()
        .join(",");
    let join = format!(
        "INNER JOIN tag_data t ON archives.id = t.item_id AND t.tag_id IN ({tag_list}) \
         AND archives.id != {}",
        query.archive_id
    );
    let (source, need_group) = related_source(query, &join);

    let mut count_builder = QueryBuilder::<MySql>::new(format!(
        "SELECT COUNT(DISTINCT archives.id) FROM {source}"
    ));
    count_builder.push(" WHERE 1 = 1");
    push_related_scope(&mut count_builder, query);
    let total: i64 = count_builder.build_query_scalar().fetch_one(pool).await?;

    let mut ids_builder = QueryBuilder::<MySql>::new(format!("SELECT archives.id FROM {source}"));
    ids_builder.push(" WHERE 1 = 1");
    push_related_scope(&mut ids_builder, query);
    if need_group {
        ids_builder.push(" GROUP BY archives.id");
    }
    ids_builder.push(" ORDER BY archives.id DESC");
    ids_builder
        .push(" LIMIT ")
        .push_bind(query.limit.max(1) as i64)
        .push(" OFFSET ")
        .push_bind(query.offset as i64);
    let ids: Vec<i64> = ids_builder.build_query_scalar().fetch_all(pool).await?;
    let rows = fetch_archive_rows(pool, "archives", &ids).await?;
    Ok((rows, total))
}

/// `like=id`：当前内容前后各取一半（两侧合并后再按截断规则取用）。
async fn related_by_neighbors(
    pool: &MySqlPool,
    query: &RelatedQuery,
) -> anyhow::Result<(Vec<Archive>, i64)> {
    let mut newer = related_neighbors_side(pool, query, true).await?;
    let mut older = related_neighbors_side(pool, query, false).await?;
    let limit = query.limit.max(1) as usize;
    let half = (limit + 1) / 2;
    if newer.len() + older.len() > limit {
        if newer.len() > half && older.len() > half {
            newer.truncate(half);
            older.truncate(half);
        } else if newer.len() > older.len() {
            newer.truncate(limit.saturating_sub(older.len()));
        } else if older.len() > newer.len() {
            older.truncate(limit.saturating_sub(newer.len()));
        }
    }
    let mut merged = older;
    merged.extend(newer);
    if merged.len() > limit {
        merged.truncate(limit);
    }
    // id 分支同样不统计总数。
    Ok((merged, 0))
}

/// 前后取一半的单边查询（`newer` 为真取 ID 更大的）。
async fn related_neighbors_side(
    pool: &MySqlPool,
    query: &RelatedQuery,
    newer: bool,
) -> anyhow::Result<Vec<Archive>> {
    let mut source = String::from("archives");
    let mut need_group = false;
    if query.multi_category {
        // 多分类时把栏目条件放在 ON 上（对齐 JOIN 写法）。
        source.push_str(&format!(
            " INNER JOIN archive_categories ON archives.id = archive_categories.archive_id \
             AND archive_categories.category_id = {}",
            query.category_id
        ));
        need_group = true;
    }
    let mut builder = QueryBuilder::<MySql>::new(format!("SELECT archives.id FROM {source}"));
    builder.push(" WHERE 1 = 1");
    if !query.multi_category {
        builder
            .push(" AND archives.`category_id` = ")
            .push_bind(query.category_id);
    }
    if query.module_id > 0 {
        builder
            .push(" AND archives.module_id = ")
            .push_bind(query.module_id);
    }
    if !query.exclude_category_ids.is_empty() {
        if query.multi_category {
            builder.push(" AND archive_categories.category_id NOT IN (");
        } else {
            builder.push(" AND archives.`category_id` NOT IN (");
        }
        let mut separated = builder.separated(", ");
        for id in &query.exclude_category_ids {
            separated.push_bind(*id);
        }
        separated.push_unseparated(")");
    }
    if newer {
        builder
            .push(" AND archives.id > ")
            .push_bind(query.archive_id)
            .push(" ORDER BY archives.id ASC");
    } else {
        builder
            .push(" AND archives.id < ")
            .push_bind(query.archive_id)
            .push(" ORDER BY archives.id DESC");
    }
    if need_group {
        builder.push(" GROUP BY archives.id");
    }
    builder
        .push(" LIMIT ")
        .push_bind(query.limit.max(1) as i64)
        .push(" OFFSET ")
        .push_bind(query.offset as i64);
    let ids: Vec<i64> = builder.build_query_scalar().fetch_all(pool).await?;
    fetch_archive_rows(pool, "archives", &ids).await
}

/// 标签下的内容列表（`tag_data` 关联）。
pub async fn tag_archives(
    pool: &MySqlPool,
    tag_id: u32,
    order: &str,
    limit: u32,
    offset: u32,
) -> anyhow::Result<(Vec<Archive>, i64)> {
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM archives INNER JOIN tag_data t ON archives.id = t.item_id \
         AND t.tag_id = ?",
    )
    .bind(tag_id)
    .fetch_one(pool)
    .await?;

    let mut builder = QueryBuilder::<MySql>::new(
        "SELECT archives.id FROM archives INNER JOIN tag_data t ON archives.id = t.item_id \
         AND t.tag_id = ?",
    );
    builder.push_bind(tag_id);
    let order = order.trim();
    if !order.is_empty() {
        builder.push(" ORDER BY ").push(order);
    }
    builder
        .push(" LIMIT ")
        .push_bind(limit.max(1) as i64)
        .push(" OFFSET ")
        .push_bind(offset as i64);
    let ids: Vec<i64> = builder.build_query_scalar().fetch_all(pool).await?;

    let rows = fetch_archive_rows(pool, "archives", &ids).await?;
    Ok((rows, total))
}

/// 内容正文（`archive_data`）。
pub async fn archive_content(pool: &MySqlPool, id: i64) -> anyhow::Result<String> {
    let row: Option<Option<String>> =
        sqlx::query_scalar("SELECT content FROM archive_data WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row.flatten().unwrap_or_default())
}

/// 会员的用户组等级（`users.group_id` → `user_groups.level`）。
///
/// 阅读等级校验：`archive.read_level` 与会员等级比较。
pub async fn member_level(pool: &MySqlPool, user_id: u32) -> Option<i32> {
    let level: Option<i32> = sqlx::query_scalar(
        "SELECT CAST(g.`level` AS SIGNED) FROM users u \
         INNER JOIN user_groups g ON g.id = u.group_id WHERE u.id = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    level
}

// ---------------------------------------------------------------------------
// 分类（categories）
// ---------------------------------------------------------------------------

/// 分类类型：内容列表。
pub const CATEGORY_TYPE_ARCHIVE: u32 = 1;
/// 分类类型：单页。
pub const CATEGORY_TYPE_PAGE: u32 = 3;

/// 前台分类列表（对齐 `GetCategoriesFromCache` + 分页切片）。
///
/// - 只返回 `status = 1` 的正式分类；
/// - `parent_id = 0 && module_id > 0` 时按模块过滤（单页除外）；
/// - `all = true` 时不限父级。
pub async fn api_categories(
    pool: &MySqlPool,
    module_id: u32,
    parent_id: u32,
    all: bool,
    kind: u32,
    offset: usize,
    limit: usize,
) -> anyhow::Result<(Vec<Category>, i64)> {
    let mut builder = QueryBuilder::<MySql>::new(
        "SELECT * FROM categories WHERE status = 1 AND `type` = ",
    );
    builder.push_bind(kind);
    if parent_id == 0 && module_id > 0 && kind != CATEGORY_TYPE_PAGE {
        builder.push(" AND module_id = ").push_bind(module_id);
    }
    if !all {
        builder.push(" AND parent_id = ").push_bind(parent_id);
    }
    builder.push(" ORDER BY sort ASC, id ASC");
    let rows: Vec<Category> = builder.build_query_as().fetch_all(pool).await?;
    let total = rows.len() as i64;
    Ok((slice_page(rows, offset, limit), total))
}

/// 切片：`offset/limit` 语义（`limit = 0` 表示不限）。
fn slice_page<T: Clone>(rows: Vec<T>, offset: usize, limit: usize) -> Vec<T> {
    if offset >= rows.len() {
        return Vec::new();
    }
    let end = if limit > 0 {
        (offset + limit).min(rows.len())
    } else {
        rows.len()
    };
    rows[offset..end].to_vec()
}

// ---------------------------------------------------------------------------
// 城市站（places）
// ---------------------------------------------------------------------------

/// 前台城市站列表（对齐 `GetPlacesFromCache`）。
pub async fn api_places(
    pool: &MySqlPool,
    parent_id: u32,
    all: bool,
    offset: usize,
    limit: usize,
) -> anyhow::Result<(Vec<Place>, i64)> {
    let mut builder =
        QueryBuilder::<MySql>::new(format!("SELECT {PLACE_COLUMNS} FROM places WHERE status = 1"));
    if !all {
        builder.push(" AND parent_id = ").push_bind(parent_id);
    }
    builder.push(" ORDER BY sort ASC, id ASC");
    let rows: Vec<Place> = builder.build_query_as().fetch_all(pool).await?;
    let total = rows.len() as i64;
    Ok((slice_page(rows, offset, limit), total))
}

// ---------------------------------------------------------------------------
// 标签（tags）
// ---------------------------------------------------------------------------

/// 前台标签列表（对齐 `GetTagList`）。
pub async fn api_tags(
    pool: &MySqlPool,
    item_id: i64,
    q: &str,
    category_ids: &[u32],
    letter: &str,
    order: &str,
    limit: u32,
    offset: u32,
) -> anyhow::Result<(Vec<Tag>, i64)> {
    let mut builder = QueryBuilder::<MySql>::new("SELECT * FROM tags WHERE 1 = 1");
    let q = q.trim();
    if !q.is_empty() {
        builder.push(" AND title LIKE ").push_bind(format!("%{q}%"));
    }
    if !category_ids.is_empty() {
        builder.push(" AND category_id IN (");
        let mut separated = builder.separated(", ");
        for id in category_ids {
            separated.push_bind(*id);
        }
        separated.push_unseparated(")");
    }
    let letter = letter.trim();
    if !letter.is_empty() {
        builder.push(" AND first_letter = ").push_bind(letter);
    }
    if item_id > 0 {
        builder
            .push(" AND id IN (SELECT tag_id FROM tag_data WHERE item_id = ")
            .push_bind(item_id)
            .push(")");
    }

    // 计数单独走一遍同样的条件（用子查询避免重复拼装）。
    let mut count = QueryBuilder::<MySql>::new("SELECT COUNT(*) FROM tags WHERE 1 = 1");
    if !q.is_empty() {
        count.push(" AND title LIKE ").push_bind(format!("%{q}%"));
    }
    if !category_ids.is_empty() {
        count.push(" AND category_id IN (");
        let mut separated = count.separated(", ");
        for id in category_ids {
            separated.push_bind(*id);
        }
        separated.push_unseparated(")");
    }
    if !letter.is_empty() {
        count.push(" AND first_letter = ").push_bind(letter);
    }
    if item_id > 0 {
        count
            .push(" AND id IN (SELECT tag_id FROM tag_data WHERE item_id = ")
            .push_bind(item_id)
            .push(")");
    }
    let total: i64 = count.build_query_scalar().fetch_one(pool).await?;

    let order = order.trim();
    if order.is_empty() {
        builder.push(" ORDER BY id DESC");
    } else {
        builder.push(" ORDER BY ").push(order);
    }
    builder
        .push(" LIMIT ")
        .push_bind(limit.max(1) as i64)
        .push(" OFFSET ")
        .push_bind(offset as i64);
    let rows: Vec<Tag> = builder.build_query_as().fetch_all(pool).await?;
    Ok((rows, total))
}

// ---------------------------------------------------------------------------
// 导航（navs）
// ---------------------------------------------------------------------------

/// 前台导航（按分组取整棵树）。
pub async fn api_navs(pool: &MySqlPool, type_id: u32) -> anyhow::Result<Vec<NavNode>> {
    let list = super::nav::list_by_type(pool, type_id).await?;
    Ok(super::nav::tree(&list))
}

/// 前台幻灯片（`settings.banner` 里按分组拍平）。
///
/// 对齐 `ApiGetBanners`：只取 `type` 相同的分组，图片地址补存储域名。
pub async fn api_banners(
    pool: &MySqlPool,
    kind: &str,
    storage_url: &str,
) -> anyhow::Result<Vec<Value>> {
    let groups = site_setting::banner_list(pool).await?;
    let mut out: Vec<Value> = Vec::new();
    let Some(list) = groups.as_array() else {
        return Ok(out);
    };
    for group in list {
        let group_type = group
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("default");
        if group_type != kind {
            continue;
        }
        let Some(items) = group.get("list").and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let mut item = item.clone();
            if let Value::Object(map) = &mut item {
                if let Some(logo) = map.get("logo").and_then(Value::as_str) {
                    map.insert("logo".into(), json!(join_storage_url(storage_url, logo)));
                }
            }
            out.push(item);
        }
    }
    Ok(out)
}

/// 设置内容归属（`archives.user_id`）。
///
/// `models::archive_write::create` 固定写 0，前台投稿需要在建好内容后补写归属。
pub async fn set_archive_owner(
    pool: &MySqlPool,
    id: i64,
    user_id: u32,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE archives SET user_id = ? WHERE id = ?")
        .bind(user_id)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 站点信息 / 链接生成
// ---------------------------------------------------------------------------

/// 站点地址信息。
#[derive(Debug, Clone, Default)]
pub struct SiteUrls {
    pub base_url: String,
    pub front_url: String,
    pub storage_url: String,
}

/// 读站点地址（`settings.system` + `settings.storage`）。
pub async fn site_urls(pool: &MySqlPool) -> anyhow::Result<SiteUrls> {
    let system = site_setting::get_section(pool, site_setting::SYSTEM).await?;
    let storage = site_setting::get_section(pool, "storage").await?;

    let base_url = system
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .trim_end_matches('/')
        .to_string();
    let front_url = system
        .get("front_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| base_url.clone());
    let storage_url = storage
        .get("storage_url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    Ok(SiteUrls {
        base_url,
        front_url,
        storage_url,
    })
}

/// 拼接存储地址（与 `JoinStorageUrl` 同口径）。
pub fn join_storage_url(base: &str, path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return String::new();
    }
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    let base = base.trim();
    if base.is_empty() {
        return path.to_string();
    }
    if path.starts_with('/') {
        format!("{}{}", base.trim_end_matches('/'), path)
    } else {
        format!("{}/{}", base.trim_end_matches('/'), path)
    }
}

/// 链接生成器（伪静态规则 + 站点域名）。
#[derive(Debug, Clone, Default)]
pub struct LinkBuilder {
    pub mode: i32,
    pub front_url: String,
    pub base_url: String,
    pub module_tokens: HashMap<u32, String>,
    pub category_tokens: HashMap<u32, String>,
}

impl LinkBuilder {
    /// 读伪静态模式、站点域名与别名表。
    pub async fn load(pool: &MySqlPool) -> anyhow::Result<Self> {
        let urls = site_urls(pool).await?;
        let mode = url::load_mode(pool).await.unwrap_or(0);
        let modules = super::module::list(pool).await.unwrap_or_default();
        let categories = super::category::list_all(pool).await.unwrap_or_default();
        Ok(Self {
            mode,
            front_url: urls.front_url,
            base_url: urls.base_url,
            module_tokens: modules
                .into_iter()
                .map(|item| (item.id, item.url_token))
                .collect(),
            category_tokens: categories
                .into_iter()
                .map(|item| (item.id, item.url_token))
                .collect(),
        })
    }

    /// 绝对地址。
    pub fn absolute(&self, path: &str) -> String {
        if path.is_empty() {
            return self.front_url.clone();
        }
        if path.starts_with("http://") || path.starts_with("https://") {
            return path.to_string();
        }
        format!("{}{}", self.front_url, path)
    }

    /// 内容详情页地址。
    pub fn archive(&self, archive: &Archive) -> String {
        let catname = self
            .category_tokens
            .get(&archive.category_id)
            .cloned()
            .unwrap_or_default();
        let module_token = self
            .module_tokens
            .get(&archive.module_id)
            .cloned()
            .unwrap_or_default();
        self.absolute(&url::archive_url(
            self.mode,
            &catname,
            &module_token,
            archive.id,
            &archive.url_token,
            archive.category_id,
            1,
        ))
    }

    /// 栏目地址。
    pub fn category(&self, category: &Category) -> String {
        let module_token = self
            .module_tokens
            .get(&category.module_id)
            .cloned()
            .unwrap_or_default();
        self.absolute(&url::category_url(
            self.mode,
            &category.url_token,
            &module_token,
            category.id,
            1,
        ))
    }

    /// 单页地址。
    pub fn page(&self, category: &Category) -> String {
        self.absolute(&url::page_url_for(
            self.mode,
            category.id,
            &category.url_token,
            1,
        ))
    }

    /// 标签地址。
    pub fn tag(&self, tag: &Tag) -> String {
        self.absolute(&url::tag_url(self.mode, tag.id, &tag.url_token, 1))
    }

    /// 模块（内容模型）列表页地址。
    pub fn module(&self, module: &Module) -> String {
        self.absolute(&url::archive_index_url(self.mode, &module.url_token, 1))
    }
}

// ---------------------------------------------------------------------------
// 输出整形
// ---------------------------------------------------------------------------

/// 内容输出结构：`Archive` + 计算字段。
///
/// 直接把内容序列化（表字段 + `category`/`data`/`tags`/`extra` 等计算字段），
/// Rust 侧不能改 `models::archive.rs`，这里在序列化结果上补齐计算字段。
pub fn archive_json(archive: &Archive) -> Value {
    let mut value = serde_json::to_value(archive).unwrap_or_else(|_| json!({}));
    if let Value::Object(map) = &mut value {
        map.insert("category".into(), Value::Null);
        map.insert("module_name".into(), json!(""));
        map.insert("data".into(), Value::Null);
        map.insert("logo".into(), json!(""));
        map.insert("thumb".into(), json!(""));
        map.insert("link".into(), json!(""));
        map.insert("has_ordered".into(), json!(false));
        map.insert("favorable_price".into(), json!(0));
        map.insert("has_password".into(), json!(false));
        map.insert("category_titles".into(), json!([]));
        map.insert("category_ids".into(), json!([]));
        map.insert("flag".into(), json!(""));
        map.insert("type".into(), json!("archive"));
        map.insert("relations".into(), json!([]));
        map.insert("content".into(), json!(""));
        map.insert("is_favorite".into(), json!(false));
        map.insert("extra".into(), json!({}));
    }
    value
}

/// 分类输出结构：`Category` + 计算字段。
pub fn category_json(category: &Category) -> Value {
    let mut value = serde_json::to_value(category).unwrap_or_else(|_| json!({}));
    if let Value::Object(map) = &mut value {
        map.insert("spacer".into(), json!(""));
        map.insert("has_children".into(), json!(false));
        map.insert("link".into(), json!(""));
        map.insert("thumb".into(), json!(""));
        map.insert("is_current".into(), json!(false));
        map.insert("parents".into(), json!([]));
    }
    value
}

/// 标签输出结构：`Tag` + 计算字段。
pub fn tag_json(tag: &Tag) -> Value {
    let mut value = serde_json::to_value(tag).unwrap_or_else(|_| json!({}));
    if let Value::Object(map) = &mut value {
        map.insert("link".into(), json!(""));
        map.insert("thumb".into(), json!(""));
        map.insert("content".into(), json!(""));
        map.insert("extra".into(), json!({}));
    }
    value
}

/// 城市站输出结构：`Place` + 计算字段。
pub fn place_json(place: &Place) -> Value {
    let mut value = serde_json::to_value(place).unwrap_or_else(|_| json!({}));
    if let Value::Object(map) = &mut value {
        map.insert("spacer".into(), json!(""));
        map.insert("has_children".into(), json!(false));
        map.insert("link".into(), json!(""));
        map.insert("thumb".into(), json!(""));
        map.insert("is_current".into(), json!(false));
        map.insert("parents".into(), json!([]));
    }
    value
}

/// 友情链接输出结构（`Link`）。
pub fn link_json(link: &Link) -> Value {
    let mut value = serde_json::to_value(link).unwrap_or_else(|_| json!({}));
    if let Value::Object(map) = &mut value {
        map.insert("logo".into(), json!(""));
        map.insert("thumb".into(), json!(""));
    }
    value
}

// ---------------------------------------------------------------------------
// 站点元信息（metadata）
// ---------------------------------------------------------------------------

/// 生成 `response.WebInfo` 形状的元信息。
///
/// 对齐 `Website.GetMetadata`：`match` 参数决定查什么，
/// 查不到时返回 404 与「404 Not Found」。
pub async fn metadata(
    pool: &MySqlPool,
    params: &HashMap<String, String>,
    links: &LinkBuilder,
) -> anyhow::Result<Value> {
    let current_page = params
        .get("page")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1);

    let mut info = json!({
        "title": "",
        "keywords": "",
        "description": "",
        "image": "",
        "nav_bar": 0,
        "page_id": 0,
        "module_id": 0,
        "page_name": "",
        "canonical_url": "",
        "total_pages": 0,
        "current_page": current_page,
        "status_code": 200,
        "params": params,
        "sep": "",
    });

    let not_found = |info: &mut Value| {
        if let Value::Object(map) = info {
            map.insert("status_code".into(), json!(404));
            map.insert("title".into(), json!("404 Not Found"));
        }
    };

    let match_key = params.get("match").map(String::as_str).unwrap_or("");
    match match_key {
        "notfound" => not_found(&mut info),
        "archive" => {
            let filename = params.get("filename").cloned().unwrap_or_default();
            let id = params
                .get("id")
                .and_then(|value| value.trim().parse::<i64>().ok())
                .unwrap_or(0);
            let found = if !filename.is_empty() {
                super::archive::get_by_token(pool, &filename).await?
            } else if id > 0 {
                super::archive::get(pool, id).await?
            } else {
                None
            };
            match found {
                Some(archive) => {
                    let title = if archive.seo_title.is_empty() {
                        archive.title.clone()
                    } else {
                        archive.seo_title.clone()
                    };
                    let canonical = if archive.canonical_url.is_empty() {
                        links.archive(&archive)
                    } else {
                        archive.canonical_url.clone()
                    };
                    if let Value::Object(map) = &mut info {
                        map.insert("title".into(), json!(title));
                        map.insert("keywords".into(), json!(archive.keywords));
                        map.insert("description".into(), json!(archive.description));
                        map.insert("nav_bar".into(), json!(archive.category_id));
                        map.insert("page_id".into(), json!(archive.id));
                        map.insert("module_id".into(), json!(archive.module_id));
                        map.insert("page_name".into(), json!("archiveDetail"));
                        map.insert("canonical_url".into(), json!(canonical));
                    }
                }
                None => not_found(&mut info),
            }
        }
        "archiveIndex" => {
            let token = params.get("module").cloned().unwrap_or_default();
            match super::module::get_by_token(pool, &token).await? {
                Some(module) => {
                    let canonical = links.module(&module);
                    if let Value::Object(map) = &mut info {
                        map.insert("title".into(), json!(module.title));
                        map.insert("keywords".into(), json!(module.keywords));
                        map.insert("description".into(), json!(module.description));
                        map.insert("nav_bar".into(), json!(module.id));
                        map.insert("page_id".into(), json!(module.id));
                        map.insert("module_id".into(), json!(module.id));
                        map.insert("page_name".into(), json!("archiveIndex"));
                        map.insert("canonical_url".into(), json!(canonical));
                    }
                }
                None => not_found(&mut info),
            }
        }
        "category" | "page" => {
            let filename = params.get("filename").cloned().unwrap_or_default();
            let id = params
                .get("id")
                .and_then(|value| value.trim().parse::<i64>().ok())
                .unwrap_or(0) as u32;
            let found = if !filename.is_empty() {
                super::category::get_by_token(pool, &filename).await?
            } else if id > 0 {
                super::category::get(pool, id).await?
            } else {
                None
            };
            match found {
                Some(category) if category.status == 1 => {
                    let canonical = if match_key == "page" {
                        links.page(&category)
                    } else {
                        links.category(&category)
                    };
                    let title = if category.seo_title.is_empty() {
                        category.title.clone()
                    } else {
                        category.seo_title.clone()
                    };
                    if let Value::Object(map) = &mut info {
                        map.insert("title".into(), json!(title));
                        map.insert("keywords".into(), json!(category.keywords));
                        map.insert("description".into(), json!(category.description));
                        map.insert("nav_bar".into(), json!(category.id));
                        map.insert("page_id".into(), json!(category.id));
                        map.insert("module_id".into(), json!(category.module_id));
                        map.insert(
                            "page_name".into(),
                            json!(if match_key == "page" {
                                "pageDetail"
                            } else {
                                "archiveList"
                            }),
                        );
                        map.insert("canonical_url".into(), json!(canonical));
                    }
                }
                _ => not_found(&mut info),
            }
        }
        "tag" => {
            let id = params
                .get("id")
                .and_then(|value| value.trim().parse::<u32>().ok())
                .unwrap_or(0);
            let filename = params.get("filename").cloned().unwrap_or_default();
            let found = if id > 0 {
                super::tag::get(pool, id).await?
            } else if !filename.is_empty() {
                super::tag::get_by_token(pool, &filename).await?
            } else {
                None
            };
            match found {
                Some(tag) => {
                    let canonical = links.tag(&tag);
                    if let Value::Object(map) = &mut info {
                        map.insert("title".into(), json!(tag.seo_title));
                        map.insert("keywords".into(), json!(tag.keywords));
                        map.insert("description".into(), json!(tag.description));
                        map.insert("nav_bar".into(), json!(tag.category_id));
                        map.insert("page_id".into(), json!(tag.id));
                        map.insert("page_name".into(), json!("tagDetail"));
                        map.insert("canonical_url".into(), json!(canonical));
                    }
                }
                None => not_found(&mut info),
            }
        }
        _ => {}
    }

    Ok(info)
}

// ---------------------------------------------------------------------------
// 友情链接
// ---------------------------------------------------------------------------

/// 按「链接 + 名称」找友情链接（对齐 `GetLinkByLinkAndTitle`）。
pub async fn friendlink_find(
    pool: &MySqlPool,
    link: &str,
    title: &str,
) -> anyhow::Result<Option<Link>> {
    let link = link.trim();
    if link.is_empty() {
        return Ok(None);
    }
    let row = sqlx::query_as::<_, Link>(
        "SELECT * FROM links WHERE `link` = ? AND (`title` = ? OR ? = '') ORDER BY id ASC LIMIT 1",
    )
    .bind(link)
    .bind(title.trim())
    .bind(title.trim())
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// 扩展字段值（`models::extra` 的薄封装，便于 API 层调用）。
pub async fn archive_extra(
    pool: &MySqlPool,
    module: &Module,
    id: i64,
) -> Map<String, Value> {
    super::extra::load(pool, module, id)
        .await
        .unwrap_or_else(|_| Map::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_order_whitelist() {
        assert_eq!(parse_order("id desc", "archives"), "archives.id desc");
        assert_eq!(parse_order("created_time desc", "archives"), "archives.created_time desc");
        assert_eq!(parse_order("archives.id ASC", "archives"), "archives.id ASC");
        assert_eq!(parse_order("id asc, created_time desc", "archives"), "archives.id asc, archives.created_time desc");
        // 非法片段整段丢弃
        assert_eq!(parse_order("id desc; drop table archives", "archives"), "");
        assert_eq!(parse_order("`id` desc", "archives"), "archives.`id` desc");
        // RAND 是唯一放行的表达式
        assert_eq!(parse_order("rand", "archives"), "RAND()");
        assert_eq!(parse_order("rand desc", "archives"), "");
        // 方向词非法则整段丢弃
        assert_eq!(parse_order("id up", "archives"), "");
    }

    #[test]
    fn slices_page_like_go() {
        let rows = vec![1, 2, 3, 4, 5];
        assert_eq!(slice_page(rows.clone(), 0, 2), vec![1, 2]);
        assert_eq!(slice_page(rows.clone(), 2, 0), vec![3, 4, 5]);
        assert_eq!(slice_page(rows.clone(), 9, 2), Vec::<i32>::new());
    }

    #[test]
    fn joins_storage_url() {
        assert_eq!(join_storage_url("https://cdn.example.com/", "/a.png"), "https://cdn.example.com/a.png");
        assert_eq!(join_storage_url("https://cdn.example.com", "a.png"), "https://cdn.example.com/a.png");
        assert_eq!(join_storage_url("", "/a.png"), "/a.png");
        assert_eq!(join_storage_url("https://cdn.example.com", "https://x.com/a.png"), "https://x.com/a.png");
        assert_eq!(join_storage_url("https://cdn.example.com", ""), "");
    }

    #[tokio::test]
    async fn reads_real_content() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };

        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM archives")
            .fetch_one(&pool)
            .await
            .unwrap();
        if total == 0 {
            return;
        }

        let query = ArchiveListQuery {
            limit: 5,
            offset: 0,
            ..Default::default()
        };
        let (rows, count) = archive_list(&pool, &query, "archives.id DESC").await.unwrap();
        assert_eq!(count, total);
        assert_eq!(rows.len() as i64, total.min(5));
        let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(ids, sorted);

        // 上一篇 / 下一篇
        let first = rows.first().unwrap();
        let prev = neighbor_archive(&pool, first.category_id, first.id, false)
            .await
            .unwrap();
        if let Some(prev) = prev {
            assert!(prev.id < first.id);
        }
        let next = neighbor_archive(&pool, first.category_id, first.id, true)
            .await
            .unwrap();
        if let Some(next) = next {
            assert!(next.id > first.id);
        }

        // 分类 / 城市站 / 标签列表都能跑通
        let (_cats, cat_total) = api_categories(&pool, 0, 0, true, CATEGORY_TYPE_ARCHIVE, 0, 10)
            .await
            .unwrap();
        assert!(cat_total >= 0);
        let (_places, _) = api_places(&pool, 0, true, 0, 10).await.unwrap();
        let (_tags, tag_total) = api_tags(&pool, 0, "", &[], "", "", 10, 0).await.unwrap();
        assert!(tag_total >= 0);

        // 标记与标签
        let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
        let _flags = archive_flags(&pool, &ids).await.unwrap();
        let _tags = archive_tags(&pool, &ids).await.unwrap();
    }

    #[tokio::test]
    async fn builds_links_from_real_settings() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };

        let links = LinkBuilder::load(&pool).await.unwrap();
        let urls = site_urls(&pool).await.unwrap();
        assert!(urls.front_url.len() >= urls.base_url.len() || urls.front_url.is_empty());

        if let Some(archive) = crate::models::archive::get(&pool, 1).await.unwrap() {
            let link = links.archive(&archive);
            assert!(link.starts_with(&links.front_url) || links.front_url.is_empty());
        }
    }
}

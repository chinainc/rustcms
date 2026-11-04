//! 互动与第三方数据访问（批次 D）。
//!
//! 对应 Go 版 `provider/comment.go`、`provider/favorite.go`、`provider/guestbook.go`、
//! `provider/weapp.go`、`provider/wechat.go`、`provider/statisticLog.go`。
//!
//! 涉及的表：`comments` / `comment_praises` / `archive_favorites` / `guestbooks` /
//! `weapp_qrcodes` / `wechat_messages` / `wechat_reply_rules` / `user_groups`。
//!
//! 口径（与 Go 版一致）：
//! - 评论 `status`：`0` = 待审核，`1` = 正常（另见 [`super::comment`]）；
//! - 留言 `status`：`0` = 待审核，`1` = 正常，`2` = 垃圾（另见 [`super::guestbook`]）；
//! - 点赞落在 `comment_praises`，同时把 `comments.vote_count` 同步成点赞条数；
//! - 收藏落在 `archive_favorites`，同时把 `archives.favorite_count` 同步成收藏条数；
//! - 评论落库后同步 `archives.comment_count`；
//! - 排序参数走白名单（对应 Go 的 `provider.ParseOrderBy`），杜绝 SQL 注入。

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use serde_json::{json, Map, Value};
use sqlx::mysql::MySqlPool;
use sqlx::{FromRow, MySql, QueryBuilder};

use super::archive::Archive;
use super::comment::Comment;
use super::guestbook;
use super::site_setting;

// ---------------------------------------------------------------------------
// 排序白名单
// ---------------------------------------------------------------------------

/// 排序白名单（对应 Go 的 `provider.ParseOrderBy`）。
///
/// 只放行「列名 + 可选方向」：列名只允许字母/数字/下划线（可带 `表.列`），
/// 方向只允许 `asc` / `desc`；`rand()` 单独放行。
pub fn parse_order_by(order: &str) -> String {
    let order = order.trim();
    if order.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    for chunk in order.split(',') {
        let fields: Vec<&str> = chunk.split_whitespace().collect();
        if fields.is_empty() || fields.len() > 2 {
            continue;
        }
        let column = fields[0].trim_matches('`');
        if column.eq_ignore_ascii_case("rand()") {
            if fields.len() == 1 {
                parts.push("RAND()".to_string());
            }
            continue;
        }
        if !is_order_column(column) {
            continue;
        }
        let quoted = if column.contains('.') {
            let mut split = column.splitn(2, '.');
            let table = split.next().unwrap_or_default().trim_matches('`');
            let field = split.next().unwrap_or_default().trim_matches('`');
            format!("`{table}`.`{field}`")
        } else {
            format!("`{column}`")
        };
        if fields.len() == 2 {
            let dir = fields[1];
            if !dir.eq_ignore_ascii_case("asc") && !dir.eq_ignore_ascii_case("desc") {
                continue;
            }
            parts.push(format!("{quoted} {}", dir.to_ascii_lowercase()));
        } else {
            parts.push(quoted);
        }
    }
    parts.join(", ")
}

fn is_order_column(column: &str) -> bool {
    !column.is_empty()
        && column.split('.').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        })
}

// ---------------------------------------------------------------------------
// 评论
// ---------------------------------------------------------------------------

/// 评论列表（对应 Go 的 `Website.GetCommentList`）。
///
/// - `archive_id > 0` 时按文档过滤；
/// - `user_id > 0` 时按会员过滤；
/// - `page > 1` 时用 `(page - 1) * page_size` 覆盖 `offset`（Go 同款）；
/// - `order` 为空时按 `id DESC` 排序。
pub async fn comment_list(
    pool: &MySqlPool,
    archive_id: i64,
    user_id: u32,
    order: &str,
    page: i64,
    page_size: i64,
    offset: i64,
) -> anyhow::Result<(i64, Vec<Comment>)> {
    let page = page.max(1);
    let page_size = page_size.clamp(1, 1000);
    let offset = if page > 1 {
        (page - 1) * page_size
    } else {
        offset.max(0)
    };

    let mut where_sql = String::from(" WHERE 1 = 1");
    if archive_id > 0 {
        where_sql.push_str(" AND archive_id = ?");
    }
    if user_id > 0 {
        where_sql.push_str(" AND user_id = ?");
    }

    let order_sql = {
        let parsed = parse_order_by(order);
        if parsed.is_empty() {
            "`id` DESC".to_string()
        } else {
            parsed
        }
    };

    let count_sql = format!("SELECT COUNT(*) FROM comments{where_sql}");
    let mut counter = sqlx::query_scalar::<_, i64>(&count_sql);
    if archive_id > 0 {
        counter = counter.bind(archive_id);
    }
    if user_id > 0 {
        counter = counter.bind(user_id);
    }
    let total = counter.fetch_one(pool).await?;

    let list_sql =
        format!("SELECT * FROM comments{where_sql} ORDER BY {order_sql} LIMIT ? OFFSET ?");
    let mut query = sqlx::query_as::<_, Comment>(&list_sql);
    if archive_id > 0 {
        query = query.bind(archive_id);
    }
    if user_id > 0 {
        query = query.bind(user_id);
    }
    let rows = query.bind(page_size).bind(offset).fetch_all(pool).await?;
    Ok((total, rows))
}

/// 批量取父评论（`parent_id > 0` 的那些）。
pub async fn parents(
    pool: &MySqlPool,
    parent_ids: &[u32],
) -> anyhow::Result<HashMap<u32, Comment>> {
    if parent_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut builder = QueryBuilder::<MySql>::new("SELECT * FROM comments WHERE id IN (");
    let mut separated = builder.separated(", ");
    for id in parent_ids {
        separated.push_bind(*id);
    }
    separated.push_unseparated(")");
    let rows: Vec<Comment> = builder.build_query_as().fetch_all(pool).await?;
    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
}

/// 新增评论（对应 Go 的 `Website.SaveComment` 新建分支）。
///
/// 正文走 HTML 转义（防 XSS），落库后同步 `archives.comment_count`。
#[allow(clippy::too_many_arguments)]
pub async fn insert_comment(
    pool: &MySqlPool,
    archive_id: i64,
    user_id: u32,
    user_name: &str,
    email: &str,
    ip: &str,
    content: &str,
    parent_id: u32,
    to_uid: u32,
    status: i8,
) -> anyhow::Result<Comment> {
    let now = chrono::Utc::now().timestamp();
    let content = super::comment::escape_html(content);
    let id = sqlx::query(
        "INSERT INTO comments (created_time, updated_time, archive_id, user_id, user_name, email, ip, \
            vote_count, content, parent_id, to_uid, status, rating) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?, 0)",
    )
    .bind(now)
    .bind(now)
    .bind(archive_id)
    .bind(user_id)
    .bind(user_name)
    .bind(email)
    .bind(ip)
    .bind(&content)
    .bind(parent_id)
    .bind(to_uid)
    .bind(status)
    .execute(pool)
    .await?
    .last_insert_id() as u32;

    refresh_comment_count(pool, archive_id).await?;

    super::comment::get(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("评论保存失败"))
}

/// 同步文档的评论数（对应 Go 的 `Comment.UpdateCommentCount`）。
pub async fn refresh_comment_count(pool: &MySqlPool, archive_id: i64) -> anyhow::Result<()> {
    let _ = sqlx::query(
        "UPDATE archives SET comment_count = \
            (SELECT COUNT(*) FROM comments WHERE archive_id = ?) WHERE id = ?",
    )
    .bind(archive_id)
    .bind(archive_id)
    .execute(pool)
    .await;
    Ok(())
}

/// 评论点赞（对应 Go 的 `Website.AddCommentPraise`）。
///
/// 返回 `true` 表示这次是新增的点赞记录（重复点赞返回 `false`，但计数会重新同步）。
pub async fn add_comment_praise(
    pool: &MySqlPool,
    user_id: u32,
    comment_id: i64,
    archive_id: i64,
) -> anyhow::Result<bool> {
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM comment_praises WHERE user_id = ? AND comment_id = ? LIMIT 1",
    )
    .bind(user_id)
    .bind(comment_id)
    .fetch_optional(pool)
    .await?;

    let created = exists.is_none();
    if created {
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO comment_praises (created_time, archive_id, comment_id, user_id, rate) \
             VALUES (?, ?, ?, ?, 1)",
        )
        .bind(now)
        .bind(archive_id)
        .bind(comment_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    }

    let _ = sqlx::query(
        "UPDATE comments SET vote_count = \
            (SELECT COUNT(*) FROM comment_praises WHERE comment_id = ?) WHERE id = ?",
    )
    .bind(comment_id)
    .bind(comment_id)
    .execute(pool)
    .await;

    Ok(created)
}

/// 评论的 JSON 载荷（字段与 Go 的 `model.Comment` 完全一致）。
///
/// Go 侧 `item_title` / `parent` / `active` 是 `gorm:"-"` 的计算字段，
/// 这里由调用方补齐。
pub fn comment_payload(comment: &Comment, item_title: &str, parent: Option<&Comment>) -> Value {
    let mut map = Map::new();
    map.insert("id".into(), json!(comment.id));
    map.insert("created_time".into(), json!(comment.created_time));
    map.insert("updated_time".into(), json!(comment.updated_time));
    map.insert("archive_id".into(), json!(comment.archive_id));
    map.insert("user_id".into(), json!(comment.user_id));
    map.insert("user_name".into(), json!(comment.user_name));
    map.insert("email".into(), json!(comment.email));
    map.insert("ip".into(), json!(comment.ip));
    map.insert("vote_count".into(), json!(comment.vote_count));
    map.insert(
        "content".into(),
        json!(comment.content.clone().unwrap_or_default()),
    );
    map.insert("parent_id".into(), json!(comment.parent_id));
    map.insert("to_uid".into(), json!(comment.to_uid));
    map.insert("status".into(), json!(comment.status));
    map.insert("rating".into(), json!(comment.rating));
    map.insert("item_title".into(), json!(item_title));
    map.insert(
        "parent".into(),
        match parent {
            Some(item) => comment_payload(item, "", None),
            None => Value::Null,
        },
    );
    map.insert("active".into(), json!(false));
    Value::Object(map)
}

// ---------------------------------------------------------------------------
// 收藏
// ---------------------------------------------------------------------------

/// 收藏记录（`archive_favorites`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ArchiveFavorite {
    pub id: i64,
    pub archive_id: i64,
    pub sku_id: i64,
    pub user_id: i64,
    pub created_time: Option<i64>,
}

/// 收藏列表（对应 Go 的 `Website.GetFavoriteList`）。
///
/// 先按收藏记录倒序取本页，再回捞内容；内容已删除的直接跳过（Go 同款）。
pub async fn favorite_list(
    pool: &MySqlPool,
    user_id: i64,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Archive>)> {
    let page = page.max(1);
    let page_size = page_size.clamp(1, 200);
    let offset = (page - 1) * page_size;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM archive_favorites WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await?;

    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT id, archive_id FROM archive_favorites WHERE user_id = ? \
         ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        return Ok((total, Vec::new()));
    }

    let ids: Vec<i64> = rows.iter().map(|(_, archive_id)| *archive_id).collect();
    let mut builder = QueryBuilder::<MySql>::new("SELECT * FROM archives WHERE id IN (");
    let mut separated = builder.separated(", ");
    for id in &ids {
        separated.push_bind(*id);
    }
    separated.push_unseparated(")");
    let archives: Vec<Archive> = builder.build_query_as().fetch_all(pool).await?;

    let mut map: HashMap<i64, Archive> = archives.into_iter().map(|item| (item.id, item)).collect();
    let mut list = Vec::with_capacity(rows.len());
    for (_, archive_id) in rows {
        if let Some(item) = map.remove(&archive_id) {
            list.push(item);
        }
    }
    Ok((total, list))
}

/// 批量查收藏状态（对应 Go 的 `Website.CheckFavorites`）。
pub async fn check_favorites(
    pool: &MySqlPool,
    user_id: i64,
    archive_ids: &[i64],
) -> anyhow::Result<Vec<ArchiveFavorite>> {
    if archive_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut builder = QueryBuilder::<MySql>::new(
        "SELECT id, archive_id, sku_id, user_id, created_time FROM archive_favorites \
         WHERE user_id = ? AND archive_id IN (",
    );
    builder.push_bind(user_id);
    let mut separated = builder.separated(", ");
    for id in archive_ids {
        separated.push_bind(*id);
    }
    separated.push_unseparated(")");
    Ok(builder.build_query_as().fetch_all(pool).await?)
}

/// 添加 / 取消收藏（对应 Go 的 `Website.AddFavorite`，**是开关语义**）。
///
/// 返回 `true` = 已收藏，`false` = 刚刚取消了收藏。
pub async fn add_favorite(
    pool: &MySqlPool,
    user_id: i64,
    archive_id: i64,
    sku_id: i64,
) -> anyhow::Result<bool> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM archives WHERE id = ? LIMIT 1")
        .bind(archive_id)
        .fetch_optional(pool)
        .await?;
    if exists.is_none() {
        return Err(anyhow::anyhow!("内容不存在"));
    }

    let old: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM archive_favorites WHERE archive_id = ? AND user_id = ? LIMIT 1",
    )
    .bind(archive_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    if let Some(id) = old {
        sqlx::query("DELETE FROM archive_favorites WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;
        refresh_favorite_count(pool, archive_id).await?;
        return Ok(false);
    }

    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO archive_favorites (archive_id, sku_id, user_id, created_time) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(archive_id)
    .bind(sku_id)
    .bind(user_id)
    .bind(now)
    .execute(pool)
    .await?;
    refresh_favorite_count(pool, archive_id).await?;
    Ok(true)
}

/// 取消收藏（对应 Go 的 `Website.DeleteFavorite`）。
pub async fn delete_favorite(
    pool: &MySqlPool,
    user_id: i64,
    archive_id: i64,
) -> anyhow::Result<u64> {
    let affected =
        sqlx::query("DELETE FROM archive_favorites WHERE archive_id = ? AND user_id = ?")
            .bind(archive_id)
            .bind(user_id)
            .execute(pool)
            .await?
            .rows_affected();
    refresh_favorite_count(pool, archive_id).await?;
    Ok(affected)
}

/// 同步内容的收藏数（对应 Go 的 `ArchiveFavorite.AfterCreate`）。
pub async fn refresh_favorite_count(pool: &MySqlPool, archive_id: i64) -> anyhow::Result<()> {
    let _ = sqlx::query(
        "UPDATE archives SET favorite_count = \
            (SELECT COUNT(*) FROM archive_favorites WHERE archive_id = ?) WHERE id = ?",
    )
    .bind(archive_id)
    .bind(archive_id)
    .execute(pool)
    .await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 留言
// ---------------------------------------------------------------------------

/// 留言字段的默认项（对应 Go 的 `Website.GetGuestbookFields` 默认字段）。
fn default_guestbook_fields() -> Vec<Value> {
    vec![
        json!({"name": "用户名", "field_name": "user_name", "type": "text",
               "required": true, "is_system": true}),
        json!({"name": "联系电话", "field_name": "contact", "type": "text",
               "required": false, "is_system": true}),
        json!({"name": "Email", "field_name": "email", "type": "text",
               "required": false, "is_system": false}),
        json!({"name": "QQ", "field_name": "qq", "type": "text",
               "required": false, "is_system": false}),
        json!({"name": "WhatsApp", "field_name": "whatsapp", "type": "text",
               "required": false, "is_system": false}),
        json!({"name": "留言内容", "field_name": "content", "type": "textarea",
               "required": false, "is_system": true}),
    ]
}

fn field_name(field: &Value) -> String {
    field
        .get("field_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

fn field_is_system(field: &Value) -> bool {
    field
        .get("is_system")
        .and_then(Value::as_bool)
        .or_else(|| {
            field
                .get("is_system")
                .and_then(Value::as_i64)
                .map(|v| v != 0)
        })
        .unwrap_or(false)
}

fn field_required(field: &Value) -> bool {
    field
        .get("required")
        .and_then(Value::as_bool)
        .or_else(|| {
            field
                .get("required")
                .and_then(Value::as_i64)
                .map(|v| v != 0)
        })
        .unwrap_or(false)
}

/// 把字段归一成 Go 的 `config.CustomField` 形状。
///
/// Go 的 JSON tag：`name` / `field_name` / `group` / `type` / `value` / `default` /
/// `remark` / `required` / `is_system` / `is_filter` / `follow_level` / `content`，
/// 其中带 `omitempty` 的为空时不出现在 JSON 里。
fn normalize_field(field: &Value) -> Value {
    let mut map = Map::new();
    map.insert(
        "name".into(),
        json!(field
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()),
    );
    let name = field_name(field);
    if !name.is_empty() {
        map.insert("field_name".into(), json!(name));
    }
    map.insert(
        "group".into(),
        json!(field
            .get("group")
            .and_then(Value::as_str)
            .unwrap_or_default()),
    );
    let kind = field
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !kind.is_empty() {
        map.insert("type".into(), json!(kind));
    }
    map.insert(
        "value".into(),
        field.get("value").cloned().unwrap_or(Value::Null),
    );
    map.insert(
        "default".into(),
        field.get("default").cloned().unwrap_or(Value::Null),
    );
    let remark = field
        .get("remark")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !remark.is_empty() {
        map.insert("remark".into(), json!(remark));
    }
    if field_required(field) {
        map.insert("required".into(), json!(true));
    }
    if field_is_system(field) {
        map.insert("is_system".into(), json!(true));
    }
    let is_filter = field
        .get("is_filter")
        .and_then(Value::as_bool)
        .or_else(|| {
            field
                .get("is_filter")
                .and_then(Value::as_i64)
                .map(|v| v != 0)
        })
        .unwrap_or(false);
    if is_filter {
        map.insert("is_filter".into(), json!(true));
    }
    map.insert(
        "follow_level".into(),
        json!(field
            .get("follow_level")
            .and_then(Value::as_bool)
            .or_else(|| {
                field
                    .get("follow_level")
                    .and_then(Value::as_i64)
                    .map(|v| v != 0)
            })
            .unwrap_or(false)),
    );
    let content = field
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !content.is_empty() {
        map.insert("content".into(), json!(content));
    }
    Value::Object(map)
}

/// 留言字段（对应 Go 的 `Website.GetGuestbookFields`）。
///
/// 配置里只要有一个系统字段（或 `user_name`），就直接用配置；否则「默认字段 + 配置字段」。
pub async fn guestbook_fields(pool: &MySqlPool) -> anyhow::Result<Vec<Value>> {
    let config = guestbook::setting_get(pool).await?;
    let configured: Vec<Value> = config
        .get("fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let use_configured_only = configured
        .iter()
        .any(|field| field_is_system(field) || field_name(field) == "user_name");

    let fields = if use_configured_only {
        configured
    } else {
        let mut list = default_guestbook_fields();
        list.extend(configured);
        list
    };

    Ok(fields.iter().map(normalize_field).collect())
}

/// 保存留言（对应 Go 的 `Website` 里 `model.Guestbook` 的落库）。
pub async fn save_guestbook(
    pool: &MySqlPool,
    user_id: u32,
    user_name: &str,
    contact: &str,
    content: &str,
    ip: &str,
    refer: &str,
    extra_data: &Value,
) -> anyhow::Result<u32> {
    let now = chrono::Utc::now().timestamp();
    let extra = if extra_data.is_object() {
        extra_data.to_string()
    } else {
        "{}".to_string()
    };
    let id = sqlx::query(
        "INSERT INTO guestbooks (created_time, updated_time, user_name, contact, content, ip, \
            status, refer, site_id, extra_data, user_id) \
         VALUES (?, ?, ?, ?, ?, ?, 0, ?, 0, ?, ?)",
    )
    .bind(now)
    .bind(now)
    .bind(user_name)
    .bind(contact)
    .bind(content)
    .bind(ip)
    .bind(refer)
    .bind(extra)
    .bind(user_id)
    .execute(pool)
    .await?
    .last_insert_id() as u32;
    Ok(id)
}

/// 留言的提示语（对应 Go 的 `PluginGuestbook.ReturnMessage`，空则用默认文案）。
pub async fn guestbook_return_message(pool: &MySqlPool) -> anyhow::Result<String> {
    let config = guestbook::setting_get(pool).await?;
    Ok(config
        .get("return_message")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string())
}

// ---------------------------------------------------------------------------
// 微信 / 小程序
// ---------------------------------------------------------------------------

/// 小程序二维码（`weapp_qrcodes`）。
///
/// 库里没有记录时需要调微信接口生成（当前项目无 HTTP 客户端），返回 `None`。
pub async fn weapp_qrcode(
    pool: &MySqlPool,
    user_id: u32,
    path: &str,
) -> anyhow::Result<Option<String>> {
    let row: Option<String> = sqlx::query_scalar(
        "SELECT code_url FROM weapp_qrcodes WHERE user_id = ? AND path = ? LIMIT 1",
    )
    .bind(user_id)
    .bind(path)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// 公众号消息落库（对应 Go 的 `model.WechatMessage`）。
pub async fn save_wechat_message(
    pool: &MySqlPool,
    openid: &str,
    content: &str,
    reply: &str,
    reply_time: i64,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO wechat_messages (created_time, updated_time, openid, content, reply, reply_time) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(now)
    .bind(now)
    .bind(openid)
    .bind(content)
    .bind(reply)
    .bind(reply_time)
    .execute(pool)
    .await?;
    Ok(())
}

/// 按关键词取自动回复（对应 Go 的 `GetWechatReplyRuleByKeyword`）。
pub async fn wechat_reply_by_keyword(
    pool: &MySqlPool,
    keyword: &str,
) -> anyhow::Result<Option<String>> {
    let row: Option<String> =
        sqlx::query_scalar("SELECT content FROM wechat_reply_rules WHERE keyword = ? LIMIT 1")
            .bind(keyword)
            .fetch_optional(pool)
            .await?;
    Ok(row)
}

/// 默认自动回复（`is_default = 1`）。
pub async fn wechat_default_reply(pool: &MySqlPool) -> anyhow::Result<Option<String>> {
    let row: Option<String> =
        sqlx::query_scalar("SELECT content FROM wechat_reply_rules WHERE is_default = 1 LIMIT 1")
            .fetch_optional(pool)
            .await?;
    Ok(row)
}

/// 公众号用户是否已绑定（`user_wechats`）。
pub async fn wechat_user_exists(pool: &MySqlPool, openid: &str) -> anyhow::Result<bool> {
    let row: Option<i64> =
        sqlx::query_scalar("SELECT id FROM user_wechats WHERE openid = ? LIMIT 1")
            .bind(openid)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some())
}

/// 公众号配置（`settings.wechat`）。
pub async fn wechat_config(pool: &MySqlPool) -> anyhow::Result<Value> {
    site_setting::get_section(pool, "wechat").await
}

/// 小程序配置（`settings.weapp`）。
pub async fn weapp_config(pool: &MySqlPool) -> anyhow::Result<Value> {
    site_setting::get_section(pool, "weapp").await
}

// ---------------------------------------------------------------------------
// 用户组 / 安全设置
// ---------------------------------------------------------------------------

/// 用户组设置（`user_groups.setting` 里的 JSON）。
pub async fn group_setting(pool: &MySqlPool, group_id: u32) -> anyhow::Result<Value> {
    let raw: Option<String> = sqlx::query_scalar::<_, Option<String>>(
        "SELECT setting FROM user_groups WHERE id = ? LIMIT 1",
    )
    .bind(group_id)
    .fetch_one(pool)
    .await
    .unwrap_or(None);
    Ok(raw
        .and_then(|item| serde_json::from_str::<Value>(&item).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({})))
}

/// 用户组是否免审核（`content_no_verify`）。
pub async fn group_no_verify(pool: &MySqlPool, group_id: u32) -> anyhow::Result<bool> {
    let setting = group_setting(pool, group_id).await?;
    Ok(setting
        .get("content_no_verify")
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

/// 用户组是否免验证码（`content_no_captcha`）。
pub async fn group_no_captcha(pool: &MySqlPool, group_id: u32) -> anyhow::Result<bool> {
    let setting = group_setting(pool, group_id).await?;
    Ok(setting
        .get("content_no_captcha")
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

/// 站点是否开启验证码（`settings.safe.captcha`）。
pub async fn safe_captcha_on(pool: &MySqlPool) -> anyhow::Result<bool> {
    let safe = site_setting::get_section(pool, "safe").await?;
    Ok(safe.get("captcha").and_then(Value::as_i64).unwrap_or(0) == 1)
}

// ---------------------------------------------------------------------------
// 访问日志
// ---------------------------------------------------------------------------

/// 蜘蛛识别（对应 Go 的 `library.GetSpider`）。
pub fn spider_of(user_agent: &str) -> &'static str {
    let ua = user_agent.to_ascii_lowercase();
    const NAMES: &[(&str, &str)] = &[
        ("googlebot", "google"),
        ("bingbot", "bing"),
        ("baiduspider", "baidu"),
        ("360spider", "360"),
        ("yahoo!", "yahoo"),
        ("sogou", "sogou"),
        ("bytespider", "byte"),
        ("yisouspider", "yisou"),
        ("yandexbot", "yandex"),
        ("spider", "other"),
        ("bot", "other"),
    ];
    for (name, value) in NAMES {
        if ua.contains(name) {
            return value;
        }
    }
    ""
}

/// 设备识别（对应 Go 的 `library.GetDevice`）。
pub fn device_of(user_agent: &str) -> &'static str {
    let ua = user_agent.to_ascii_lowercase();
    const NAMES: &[(&str, &str)] = &[
        ("android", "android"),
        ("iphone", "iphone"),
        ("windows", "windows"),
        ("macintosh", "mac"),
        ("linux", "linux"),
        ("mobile", "mobile"),
        ("curl", "curl"),
        ("python", "python"),
        ("client", "client"),
        ("spider", "spider"),
        ("bot", "spider"),
    ];
    for (name, value) in NAMES {
        if ua.contains(name) {
            return value;
        }
    }
    "proxy"
}

/// 追加一条访问日志（`data/statistic/<日期>.log`，每行一个 JSON）。
///
/// 字段与 Go 的 `provider.Statistic` 一致，后台「访问明细」页直接读这个文件。
pub fn append_statistic_log(dir: &Path, record: &Value) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let date = chrono::Local::now().format("%Y%m%d").to_string();
    let path = dir.join(format!("{date}.log"));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(record).unwrap_or_default();
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

/// 内容浏览量 +1（对应 Go 的 `archive.AddViews`）。
pub async fn add_archive_views(pool: &MySqlPool, archive_id: i64) -> anyhow::Result<()> {
    let _ = sqlx::query("UPDATE archives SET views = views + 1 WHERE id = ?")
        .bind(archive_id)
        .execute(pool)
        .await;
    Ok(())
}

// ---------------------------------------------------------------------------
// SHA-1（微信签名校验）
// ---------------------------------------------------------------------------

/// SHA-1 十六进制摘要。
///
/// 微信的服务器校验用的是 SHA-1，仓库里只有 `md-5` 依赖，所以内置一份实现。
pub fn sha1_hex(input: &[u8]) -> String {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (index, word) in chunk.chunks(4).enumerate() {
            w[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..80 {
            w[index] = (w[index - 3] ^ w[index - 8] ^ w[index - 14] ^ w[index - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (index, word) in w.iter().enumerate() {
            let (f, k) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    h.iter().map(|value| format!("{value:08x}")).collect()
}

/// 微信服务器校验签名：`sha1(sort(token, timestamp, nonce))`。
pub fn wechat_signature(token: &str, timestamp: &str, nonce: &str) -> String {
    let mut parts = [token, timestamp, nonce];
    parts.sort_unstable();
    sha1_hex(parts.join("").as_bytes())
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn order_whitelist_rejects_injection() {
        assert_eq!(parse_order_by("id desc"), "`id` desc");
        assert_eq!(parse_order_by("vote_count asc"), "`vote_count` asc");
        assert_eq!(
            parse_order_by("id desc, created_time asc"),
            "`id` desc, `created_time` asc"
        );
        // 非法列名 / 非法方向一律丢弃
        assert_eq!(parse_order_by("id; drop table comments"), "");
        assert_eq!(parse_order_by("id desc; delete from comments"), "");
        assert_eq!(parse_order_by("id up"), "");
        assert_eq!(parse_order_by(""), "");
        assert_eq!(parse_order_by("rand()"), "RAND()");
    }

    #[test]
    fn detects_spider_and_device() {
        assert_eq!(
            spider_of("Mozilla/5.0 (compatible; BaiduSpider/2.0)"),
            "baidu"
        );
        assert_eq!(
            spider_of("Mozilla/5.0 (compatible; Googlebot/2.1)"),
            "google"
        );
        assert_eq!(spider_of("Mozilla/5.0 (Windows NT 10.0) Chrome/120"), "");
        assert_eq!(
            device_of("Mozilla/5.0 (iPhone; CPU iPhone OS 16_0)"),
            "iphone"
        );
        assert_eq!(device_of("Mozilla/5.0 (Windows NT 10.0)"), "windows");
        assert_eq!(device_of("curl/8.0"), "curl");
        assert_eq!(device_of("something-else"), "proxy");
    }

    #[test]
    fn sha1_matches_known_vectors() {
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            sha1_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[tokio::test]
    async fn comment_praise_and_favorite_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM comments WHERE user_name = 'rustcms-interact'")
            .execute(&pool)
            .await;
        let _ = sqlx::query(
            "DELETE FROM comment_praises WHERE comment_id IN \
            (SELECT id FROM comments WHERE user_name = 'rustcms-interact')",
        )
        .execute(&pool)
        .await;

        let archive_id: i64 = sqlx::query_scalar("SELECT id FROM archives ORDER BY id LIMIT 1")
            .fetch_optional(&pool)
            .await
            .unwrap()
            .unwrap_or(0);

        let comment = insert_comment(
            &pool,
            archive_id,
            0,
            "rustcms-interact",
            "a@example.com",
            "127.0.0.1",
            "内容 <b>x</b>",
            0,
            0,
            0,
        )
        .await
        .unwrap();
        assert_eq!(comment.status, 0);
        assert_eq!(
            comment.content.as_deref(),
            Some("内容 &lt;b&gt;x&lt;/b&gt;"),
            "正文应做 HTML 转义"
        );

        // 列表
        let (total, rows) = comment_list(&pool, archive_id, 0, "id desc", 1, 10, 0)
            .await
            .unwrap();
        assert!(total >= 1);
        assert!(rows.iter().any(|item| item.id == comment.id));

        // 点赞（同一用户重复点赞不重复插入）
        assert!(add_comment_praise(&pool, 1, comment.id as i64, archive_id)
            .await
            .unwrap());
        assert!(!add_comment_praise(&pool, 1, comment.id as i64, archive_id)
            .await
            .unwrap());
        let praised = super::super::comment::get(&pool, comment.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(praised.vote_count, 1);

        // 载荷字段与 Go 一致
        let payload = comment_payload(&praised, "标题", None);
        assert_eq!(payload["id"], json!(comment.id));
        assert!(payload.get("item_title").is_some());
        assert!(payload.get("parent").is_some());
        assert!(payload.get("active").is_some());

        // 收藏开关
        assert!(add_favorite(&pool, 2, archive_id, 0).await.unwrap());
        let favorites = check_favorites(&pool, 2, &[archive_id]).await.unwrap();
        assert_eq!(favorites.len(), 1);
        let (total, list) = favorite_list(&pool, 2, 1, 20).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(list.len(), 1);
        assert!(!add_favorite(&pool, 2, archive_id, 0).await.unwrap());
        assert!(delete_favorite(&pool, 2, archive_id).await.unwrap() >= 1);

        // 清理
        sqlx::query("DELETE FROM comment_praises WHERE comment_id = ?")
            .bind(comment.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM comments WHERE id = ?")
            .bind(comment.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM archive_favorites WHERE user_id = ? AND archive_id = ?")
            .bind(2i64)
            .bind(archive_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn guestbook_fields_and_save() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };

        let fields = guestbook_fields(&pool).await.unwrap();
        assert!(!fields.is_empty(), "留言字段不应为空");
        assert!(fields
            .iter()
            .any(|item| item.get("field_name").and_then(Value::as_str) == Some("user_name")));
        // 字段形状与 Go 的 CustomField 对齐
        assert!(fields[0].get("name").is_some());
        assert!(fields[0].get("group").is_some());
        assert!(fields[0].get("value").is_some());
        assert!(fields[0].get("default").is_some());
        assert!(fields[0].get("follow_level").is_some());

        let id = save_guestbook(
            &pool,
            0,
            "rustcms-gb-interact",
            "hi@example.com",
            "留言",
            "127.0.0.1",
            "/contact",
            &json!({"email": "hi@example.com"}),
        )
        .await
        .unwrap();
        assert!(id > 0);

        sqlx::query("DELETE FROM guestbooks WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
}

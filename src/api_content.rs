//! 前台内容 API（批次 A）。
//!
//! 覆盖挂在 `/api` 分组下的**前台只读**接口（内容列表 / 详情 / 相关、
//! 栏目、标签），外加友链接口与前台投稿（`/archive/publish`）。
//!
//! 路径按主线要求**不带 `/api` 前缀**（如 `/archive/list`），
//! 同时注册 `/api/...` 别名，保证既有调用地址同样可用。
//!
//! 响应信封：成功 `{"code":0,"msg":"","data":...}`，
//! 列表接口额外带顶层 `total`，失败 `{"code":1,"msg":"..."}`。

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use axum::Router;
use serde_json::{json, Map, Value};

use crate::auth;
use crate::models::{
    archive, archive_write, category, front_content, link, module, multilang, place, site_setting,
    tag,
};
use crate::routes::{bearer_token, fail, failed, model_error, ok, ok_msg, NO_DB};
use crate::state::AppState;

/// 注册一条路由：站点根路径 + `/api` 别名。
macro_rules! api_route {
    ($router:expr, $path:literal, $method:ident, $handler:expr) => {{
        let handler = $handler;
        $router
            .route($path, axum::routing::$method(handler))
            .route(concat!("/api", $path), axum::routing::$method(handler))
    }};
}

/// 本批路由表。
pub fn router() -> Router<AppState> {
    let mut router = Router::new();
    router = api_route!(router, "/archive/list", get, archive_list);
    router = api_route!(router, "/archive/detail", get, archive_detail);
    router = api_route!(router, "/archive/params", get, archive_params);
    router = api_route!(router, "/archive/filters", get, archive_filters);
    router = api_route!(router, "/archive/next", get, archive_next);
    router = api_route!(router, "/archive/prev", get, archive_prev);
    router = api_route!(router, "/category/list", get, category_list);
    router = api_route!(router, "/category/detail", get, category_detail);
    router = api_route!(router, "/module/list", get, module_list);
    router = api_route!(router, "/module/detail", get, module_detail);
    router = api_route!(router, "/page/list", get, page_list);
    router = api_route!(router, "/page/detail", get, page_detail);
    router = api_route!(router, "/tag/list", get, tag_list);
    router = api_route!(router, "/tag/detail", get, tag_detail);
    router = api_route!(router, "/tag/data/list", get, tag_data_list);
    router = api_route!(router, "/place/list", get, place_list);
    router = api_route!(router, "/place/detail", get, place_detail);
    router = api_route!(router, "/nav/list", get, nav_list);
    router = api_route!(router, "/metadata", get, metadata);
    router = api_route!(router, "/languages", get, languages);
    router = api_route!(router, "/banner/list", get, banner_list);
    router = api_route!(router, "/friendlink/list", get, friendlink_list);
    router = api_route!(router, "/friendlink/create", post, friendlink_create);
    router = api_route!(router, "/friendlink/delete", post, friendlink_delete);
    router = api_route!(router, "/archive/publish", post, archive_publish);
    // 友链校验（既有实现 GET / POST 都支持）
    router = router
        .route("/friendlink/check", axum::routing::get(friendlink_check))
        .route("/friendlink/check", axum::routing::post(friendlink_check))
        .route("/api/friendlink/check", axum::routing::get(friendlink_check))
        .route("/api/friendlink/check", axum::routing::post(friendlink_check));
    router
}

// ---------------------------------------------------------------------------
// 参数解析（取不到就当零值，不报错）
// ---------------------------------------------------------------------------

fn q_str(q: &HashMap<String, String>, key: &str) -> String {
    q.get(key).map(|value| value.trim().to_string()).unwrap_or_default()
}

fn q_i64(q: &HashMap<String, String>, key: &str) -> i64 {
    q.get(key)
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

fn q_u32(q: &HashMap<String, String>, key: &str) -> u32 {
    q.get(key)
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

/// 既有的 `URLParamBool` 语义：`1/t/true/yes/on` 都算真。
fn q_bool(q: &HashMap<String, String>, key: &str, default: bool) -> bool {
    match q.get(key) {
        Some(raw) => parse_bool(raw),
        None => default,
    }
}

fn parse_bool(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "t" | "true" | "yes" | "on"
    )
}

/// 逗号分隔的 ID 列表。
fn q_id_list(q: &HashMap<String, String>, key: &str) -> Vec<u32> {
    q.get(key)
        .map(|raw| {
            raw.split(',')
                .filter_map(|item| item.trim().parse::<u32>().ok())
                .filter(|id| *id > 0)
                .collect()
        })
        .unwrap_or_default()
}

/// `limit` 参数：支持 `offset,limit` 与 `limit` 两种写法（按既有约定）。
///
/// 返回 `(offset, limit)`；参数缺省时 `limit` 取 `default_limit`。
fn q_limit(q: &HashMap<String, String>, max_limit: u32, default_limit: u32) -> (u32, u32) {
    let raw = q_str(q, "limit");
    if raw.is_empty() {
        return (0, default_limit);
    }
    let mut offset = 0u32;
    let mut limit = 0u32;
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() == 2 {
        offset = parts[0].trim().parse().unwrap_or(0);
        limit = parts[1].trim().parse().unwrap_or(0);
    } else if parts.len() == 1 {
        limit = parts[0].trim().parse().unwrap_or(0);
    }
    if limit > max_limit {
        limit = max_limit;
    }
    if limit < 1 {
        limit = 1;
    }
    (offset, limit)
}

/// 列表响应：顶层额外带 `total`（按既有列表接口）。
fn ok_total(total: i64, data: Value) -> Response {
    Json(json!({ "code": 0, "msg": "", "total": total, "data": data })).into_response()
}

/// 读内容相关配置（`settings.content`）。
async fn content_settings(pool: &sqlx::MySqlPool) -> Value {
    site_setting::get_section(pool, site_setting::CONTENT)
        .await
        .unwrap_or_else(|_| json!({}))
}

/// 取会员令牌（Bearer / `token` 头 / `token` cookie）。
fn request_token(headers: &HeaderMap) -> Option<String> {
    bearer_token(headers)
        .or_else(|| {
            headers
                .get("token")
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            headers
                .get("cookie")
                .and_then(|value| value.to_str().ok())
                .and_then(|raw| {
                    raw.split(';').find_map(|item| {
                        let (key, value) = item.trim().split_once('=')?;
                        if key.trim() == "token" {
                            Some(value.trim().to_string())
                        } else {
                            None
                        }
                    })
                })
                .filter(|value| !value.is_empty())
        })
}

/// 解析请求里的会员 ID（令牌缺失或无效时返回 `None`）。
async fn request_user_id(pool: &sqlx::MySqlPool, headers: &HeaderMap) -> Option<u32> {
    let token = request_token(headers)?;
    let secret = auth::token_secret(pool).await.ok()?;
    let claims = auth::verify(auth::TokenKind::User, &secret, &token).ok()?;
    claims.subject(auth::TokenKind::User)
}

/// 百分号编码（按 `url.QueryEscape` 的语义）。
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~') {
            out.push(ch);
        } else if ch == ' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// 拼查询串（按 `url.Values.Encode` 的语义：键名排序、空格转 `+`）。
fn encode_query(params: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut parts: Vec<String> = Vec::new();
    for (key, value) in sorted {
        parts.push(format!("{}={}", percent_encode(key), percent_encode(value)));
    }
    parts.join("&")
}

/// 筛选链接：在当前查询参数上替换某个字段的取值。
fn filter_link(prefix: &str, params: &[(String, String)], field: &str, value: &str) -> String {
    let mut items: Vec<(String, String)> = params
        .iter()
        .filter(|(key, _)| key.as_str() != field)
        .cloned()
        .collect();
    items.push((field.to_string(), value.to_string()));
    format!("{prefix}{}", encode_query(&items))
}

/// 内容最大条数（`settings.content.max_limit`，既有实现默认 100）。
async fn max_limit(pool: &sqlx::MySqlPool) -> u32 {
    content_settings(pool)
        .await
        .get("max_limit")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(100) as u32
}

/// 站点链接生成器。
async fn links(pool: &sqlx::MySqlPool) -> front_content::LinkBuilder {
    front_content::LinkBuilder::load(pool)
        .await
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 内容
// ---------------------------------------------------------------------------

/// `GET /archive/list` —— 内容列表。
async fn archive_list(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let content = content_settings(pool).await;
    let max = content
        .get("max_limit")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(100) as u32;
    let use_sort = content.get("use_sort").and_then(Value::as_i64).unwrap_or(0) == 1;
    // 多分类模式（`settings.content.multi_category = 1`）：分类条件走 archive_categories 关联表。
    let multi_category = match content.get("multi_category") {
        Some(Value::Number(number)) => number.as_i64().unwrap_or(0) == 1,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::String(text)) => text.trim() == "1",
        _ => false,
    };

    let mut category_ids = q_id_list(&q, "categoryId");
    let exclude_category_ids = q_id_list(&q, "excludeCategoryId");
    let mut module_id = q_u32(&q, "moduleId");

    // categoryId → 模型推导（既有实现：仅当 modelId == 0 时取第一个分类的模型）
    if module_id == 0 && !category_ids.is_empty() {
        if let Some(category) = category::get(pool, category_ids[0]).await.unwrap_or(None) {
            module_id = category.module_id;
        }
    }

    // child（默认 true）：把子栏目一起算进来
    let child = q_bool(&q, "child", true);
    if child && !category_ids.is_empty() {
        let all = category::list_all(pool).await.unwrap_or_default();
        let mut expanded: Vec<u32> = Vec::new();
        for cid in &category_ids {
            for item in category::self_and_sub_ids(&all, *cid) {
                if !expanded.contains(&item) {
                    expanded.push(item);
                }
            }
        }
        category_ids = expanded;
    }

    // userId > 0 时覆盖 authorId（既有的写法）
    let user_id = q_u32(&q, "userId");
    let author_id = if user_id > 0 {
        Some(user_id)
    } else {
        let author = q_u32(&q, "authorId");
        if author > 0 {
            Some(author)
        } else {
            None
        }
    };

    let (mut offset, limit) = q_limit(&q, max, 10);
    let page = q_i64(&q, "page").max(1) as u32;
    let list_type = {
        let value = q_str(&q, "type");
        if value.is_empty() {
            "list".to_string()
        } else {
            value
        }
    };
    if list_type == "page" && page > 1 {
        offset = (page - 1) * limit;
    }
    // 相关文档（既有的 `type=related` 分支）走独立逻辑。
    if list_type == "related" {
        return archive_list_related(pool, &q, &content).await;
    }

    let order_raw = q_str(&q, "order");
    let order = if order_raw.is_empty() {
        if use_sort {
            "archives.sort DESC, archives.created_time DESC".to_string()
        } else {
            "archives.created_time DESC".to_string()
        }
    } else {
        front_content::parse_order(&order_raw, "archives")
    };

    let query = front_content::ArchiveListQuery {
        id: {
            let value = q_i64(&q, "id");
            if value > 0 {
                Some(value)
            } else {
                None
            }
        },
        parent_id: {
            let value = q_i64(&q, "parentId");
            if value > 0 {
                Some(value)
            } else {
                None
            }
        },
        place_id: {
            let value = q_i64(&q, "placeId");
            if value > 0 {
                Some(value)
            } else {
                None
            }
        },
        module_id: if module_id > 0 { Some(module_id) } else { None },
        author_id,
        category_ids,
        exclude_category_ids,
        tag_ids: {
            let value = q_i64(&q, "tagId");
            if value > 0 {
                vec![value as u32]
            } else {
                Vec::new()
            }
        },
        flag: {
            let value = q_str(&q, "flag");
            if value.is_empty() {
                None
            } else {
                Some(value)
            }
        },
        q: {
            let value = q_str(&q, "q");
            if value.is_empty() {
                None
            } else {
                Some(value)
            }
        },
        multi_category,
        page,
        limit,
        offset,
        draft: q_bool(&q, "draft", false),
    };

    let (rows, total) = match front_content::archive_list(pool, &query, &order).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };

    let show_flag = q_bool(&q, "showFlag", false);
    let show_tag = q_bool(&q, "showTag", false);
    let show_content = q_bool(&q, "showContent", false);
    let show_extra = q_bool(&q, "showExtra", false);
    let show_category = q_bool(&q, "showCategory", false);

    let builder = links(pool).await;
    let data = archive_list_json(
        pool,
        &rows,
        show_flag,
        show_tag,
        show_content,
        show_extra,
        show_category,
        &builder,
    )
    .await;
    ok_total(total, Value::Array(data))
}

/// `GET /archive/list?type=related` —— 相关文档（按 `ApiGetArchives` 的语义 related 分支）。
async fn archive_list_related(
    pool: &sqlx::MySqlPool,
    q: &HashMap<String, String>,
    content: &Value,
) -> Response {
    let max = content
        .get("max_limit")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(100) as u32;
    let (offset, limit) = q_limit(q, max, 10);
    // 多分类模式（`settings.content.multi_category = 1`）。
    let multi_category = match content.get("multi_category") {
        Some(Value::Number(number)) => number.as_i64().unwrap_or(0) == 1,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::String(text)) => text.trim() == "1",
        _ => false,
    };

    let category_ids = q_id_list(q, "categoryId");
    let exclude_category_ids = q_id_list(q, "excludeCategoryId");
    let mut module_id = q_u32(q, "moduleId");
    let archive_id = q_i64(q, "id");
    let like = q_str(q, "like");
    let mut keywords = q_str(q, "keywords");

    // 只取第一个栏目的模型（既有接口即如此）。
    if module_id == 0 && !category_ids.is_empty() {
        if let Some(item) = category::get(pool, category_ids[0]).await.unwrap_or(None) {
            module_id = item.module_id;
        }
    }
    let mut category_id = category_ids.first().copied().unwrap_or(0);

    // like != relation 时，用基准内容自身的栏目与关键字（按既有约定）。
    if like != "relation" && archive_id > 0 {
        if let Ok(Some(row)) = archive::get(pool, archive_id).await {
            category_id = row.category_id;
            keywords = row
                .keywords
                .split(|ch| ch == ',' || ch == '，')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            if let Some(item) = category::get(pool, category_id).await.unwrap_or(None) {
                module_id = item.module_id;
            }
        }
    }
    if !q_str(q, "keywords").is_empty() {
        keywords = q_str(q, "keywords");
    }

    let related = front_content::RelatedQuery {
        archive_id,
        category_id,
        module_id,
        exclude_category_ids,
        keywords,
        limit,
        offset,
        multi_category,
    };

    let (rows, total) = match front_content::related_list(pool, &like, &related).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let builder = links(pool).await;
    let data = archive_list_json(pool, &rows, false, false, false, false, false, &builder).await;
    ok_total(total, Value::Array(data))
}

/// 把内容行转成既有的 JSON 结构。
async fn archive_list_json(
    pool: &sqlx::MySqlPool,
    rows: &[archive::Archive],
    show_flag: bool,
    show_tag: bool,
    show_content: bool,
    show_extra: bool,
    show_category: bool,
    builder: &front_content::LinkBuilder,
) -> Vec<Value> {
    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    let flags = if show_flag {
        front_content::archive_flags(pool, &ids).await.unwrap_or_default()
    } else {
        HashMap::new()
    };
    let tag_map = if show_tag {
        front_content::archive_tags(pool, &ids).await.unwrap_or_default()
    } else {
        HashMap::new()
    };

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut value = front_content::archive_json(row);
        if let Value::Object(map) = &mut value {
            map.insert("link".into(), json!(builder.archive(row)));
            map.insert("logo".into(), json!(row.cover()));
            map.insert("thumb".into(), json!(row.cover()));
            if show_flag {
                if let Some(flag) = flags.get(&row.id) {
                    map.insert("flag".into(), json!(flag));
                }
            }
            if show_tag {
                if let Some(items) = tag_map.get(&row.id) {
                    let list: Vec<Value> = items.iter().map(front_content::tag_json).collect();
                    map.insert("tags".into(), Value::Array(list));
                }
            }
            if show_content {
                let content = front_content::archive_content(pool, row.id)
                    .await
                    .unwrap_or_default();
                map.insert("content".into(), json!(content));
            }
            if show_category {
                if let Some(item) = category::get(pool, row.category_id).await.unwrap_or(None) {
                    map.insert("category".into(), front_content::category_json(&item));
                }
            }
            if show_extra {
                if let Some(item) = module::get(pool, row.module_id).await.unwrap_or(None) {
                    let extra = front_content::archive_extra(pool, &item, row.id).await;
                    map.insert("extra".into(), Value::Object(extra));
                }
            }
            if !row.password.is_empty() {
                map.insert("password".into(), json!(""));
                map.insert("has_password".into(), json!(true));
            }
        }
        out.push(value);
    }
    out
}

/// `GET /archive/detail` —— 内容详情。
async fn archive_detail(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let id = q_i64(&q, "id");
    let filename = q_str(&q, "filename");
    let url_token = {
        let value = q_str(&q, "url_token");
        if filename.is_empty() {
            value
        } else {
            filename
        }
    };

    let mut row = match archive::get(pool, id).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    if !url_token.is_empty() {
        if url_token == "prev" || url_token == "next" {
            let Some(current) = row.as_ref() else {
                return fail("no archive found");
            };
            let next = url_token == "next";
            match front_content::neighbor_archive(pool, current.category_id, current.id, next)
                .await
            {
                Ok(value) => row = value,
                Err(err) => return failed(err),
            }
        } else {
            match archive::get_by_token(pool, &url_token).await {
                Ok(value) => row = value,
                Err(err) => return failed(err),
            }
        }
    }
    let Some(row) = row else {
        return fail("no archive found");
    };

    let builder = links(pool).await;
    let mut value = front_content::archive_json(&row);
    let content = front_content::archive_content(pool, row.id).await.unwrap_or_default();
    // 密码保护：密码不对时不回传正文（按 `ArchiveData = nil` 的语义）。
    let password = q_str(&q, "password");
    let password_valid = !row.password.is_empty() && password == row.password;
    // 阅读等级：未登录读者看不到付费正文（按既有 ReadLevel 分支）。
    let read_level = row.read_level;
    // 作者本人、等级足够的会员可以看正文（按既有 HasOrdered）。
    let mut has_ordered = row.price == 0 && row.read_level == 0;
    if let Some(uid) = request_user_id(pool, &headers).await {
        if row.user_id == uid {
            has_ordered = true;
        } else if row.read_level > 0 {
            if let Some(level) = front_content::member_level(pool, uid).await {
                if level >= row.read_level {
                    has_ordered = true;
                }
            }
        }
    }
    if let Value::Object(map) = &mut value {
        map.insert("content".into(), json!(content));
        map.insert(
            "data".into(),
            json!({ "id": row.id, "content": content }),
        );
        map.insert("link".into(), json!(builder.archive(&row)));
        map.insert("logo".into(), json!(row.cover()));
        map.insert("thumb".into(), json!(row.cover()));
        map.insert("module_name".into(), json!(""));
        if let Some(item) = module::get(pool, row.module_id).await.unwrap_or(None) {
            map.insert("module_name".into(), json!(item.name));
        }
        if let Some(item) = category::get(pool, row.category_id).await.unwrap_or(None) {
            map.insert("category".into(), front_content::category_json(&item));
        }
        let flags = front_content::archive_flags(pool, &[row.id]).await.unwrap_or_default();
        if let Some(flag) = flags.get(&row.id) {
            map.insert("flag".into(), json!(flag));
        }
        let tags = front_content::archive_tags(pool, &[row.id]).await.unwrap_or_default();
        if let Some(items) = tags.get(&row.id) {
            let list: Vec<Value> = items.iter().map(front_content::tag_json).collect();
            map.insert("tags".into(), Value::Array(list));
        }
        if let Some(item) = module::get(pool, row.module_id).await.unwrap_or(None) {
            let extra = front_content::archive_extra(pool, &item, row.id).await;
            map.insert("extra".into(), Value::Object(extra));
        }
        if !row.password.is_empty() {
            map.insert("password".into(), json!(""));
            map.insert("has_password".into(), json!(true));
            if password_valid {
                map.insert("password_valid".into(), json!(true));
            } else {
                // 密码不正确：正文与正文数据都不下发（按既有约定）。
                map.insert("content".into(), json!(""));
                map.insert("data".into(), Value::Null);
            }
        } else if read_level > 0 && !has_ordered {
            // 需要会员等级才能阅读：只给提示文案（按既有 TplTr 提示）。
            let tip = format!("该内容需要用户等级{read_level}以上才能阅读");
            map.insert("content".into(), json!(tip));
            map.insert("data".into(), json!({ "id": row.id, "content": tip }));
        }
    }
    ok(value)
}

/// `GET /archive/params` —— 内容的自定义字段定义与取值。
async fn archive_params(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let id = q_i64(&q, "id");
    let filename = q_str(&q, "filename");
    let url_token = {
        let value = q_str(&q, "url_token");
        if filename.is_empty() {
            value
        } else {
            filename
        }
    };

    let mut row = match archive::get(pool, id).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    if !url_token.is_empty() && url_token != "prev" && url_token != "next" {
        match archive::get_by_token(pool, &url_token).await {
            Ok(value) => row = value,
            Err(err) => return failed(err),
        }
    }
    let Some(row) = row else {
        return fail("no archive found");
    };

    let Some(item) = module::get(pool, row.module_id).await.unwrap_or(None) else {
        return ok(Value::Array(Vec::new()));
    };
    let extra = front_content::archive_extra(pool, &item, row.id).await;

    let fields: Vec<Value> = item
        .field_defs()
        .iter()
        .map(|field| {
            let mut value = serde_json::to_value(field).unwrap_or_else(|_| json!({}));
            if let Value::Object(map) = &mut value {
                let current = extra.get(&field.field_name).cloned();
                map.insert(
                    "value".into(),
                    current.unwrap_or_else(|| Value::Null),
                );
            }
            value
        })
        .collect();

    if q_bool(&q, "sorted", true) {
        return ok(Value::Array(fields));
    }
    // sorted=false：按字段标识建 map（按既有约定）
    let mut map = Map::new();
    for field in item.field_defs() {
        let mut value = serde_json::to_value(&field).unwrap_or_else(|_| json!({}));
        if let Value::Object(obj) = &mut value {
            obj.insert(
                "value".into(),
                extra.get(&field.field_name).cloned().unwrap_or(Value::Null),
            );
        }
        map.insert(field.field_name.clone(), value);
    }
    ok(Value::Object(map))
}

/// `GET /archive/filters` —— 内容筛选组。
async fn archive_filters(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let module_id = q_u32(&q, "moduleId");
    let Some(item) = module::get(pool, module_id).await.unwrap_or(None) else {
        return fail("module not found");
    };

    let builder = links(pool).await;
    let show_all = q_bool(&q, "showAll", false);
    let all_text = {
        let value = q_str(&q, "allText");
        if value.is_empty() {
            "全部".to_string()
        } else if value == "false" || !show_all {
            String::new()
        } else {
            value
        }
    };

    // 筛选链接的基准地址：栏目页优先，否则用模型列表页（按既有 urlMatch）。
    let current_category = q_u32(&q, "categoryId");
    let base_link = if current_category > 0 {
        match category::get(pool, current_category).await.unwrap_or(None) {
            Some(entry) => builder.category(&entry),
            None => builder.module(&item),
        }
    } else {
        builder.module(&item)
    };
    let link_prefix = if base_link.contains('?') {
        format!("{base_link}&")
    } else {
        format!("{base_link}?")
    };
    // 当前查询参数（去掉分页），用于拼筛选链接。
    let base_params: Vec<(String, String)> = q
        .iter()
        .filter(|(key, _)| key.as_str() != "page")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    let mut groups: Vec<Value> = Vec::new();
    for field in item.field_defs() {
        if !field.is_filter {
            continue;
        }
        let values = field.items();
        if values.is_empty() {
            continue;
        }
        // 当前选中的取值（按 `tmpUrlParam` 的语义）。
        let current_values: Vec<String> = q
            .get(&field.field_name)
            .map(|raw| {
                raw.split(',')
                    .map(|entry| entry.trim().to_string())
                    .filter(|entry| !entry.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let mut entries: Vec<Value> = Vec::new();
        if !all_text.is_empty() {
            entries.push(json!({
                "label": all_text,
                "value": "",
                "link": filter_link(&link_prefix, &base_params, &field.field_name, ""),
                "is_current": current_values.is_empty(),
                "total": 0,
            }));
        }
        for value in values {
            entries.push(json!({
                "label": value,
                "value": value,
                "link": filter_link(&link_prefix, &base_params, &field.field_name, &value),
                "is_current": current_values.contains(&value),
                "total": 0,
            }));
        }
        groups.push(json!({
            "name": field.name,
            "field_name": field.field_name,
            "items": entries,
        }));
    }

    if q_bool(&q, "showPrice", false) {
        let max_price: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(price), 0) FROM archives")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
        let price_value = q_str(&q, "price");
        groups.push(json!({
            "name": "Price",
            "field_name": "price",
            "range": {
                "min": 0,
                "max": (max_price + 99) / 100,
                "value": price_value,
                "link": filter_link(&link_prefix, &base_params, "price", &price_value),
            },
            "items": [],
        }));
    }

    if q_bool(&q, "showCategory", false) {
        let parent_id = q_u32(&q, "parentId");
        let (categories, _) = front_content::api_categories(
            pool,
            module_id,
            parent_id,
            false,
            front_content::CATEGORY_TYPE_ARCHIVE,
            0,
            0,
        )
        .await
        .unwrap_or((Vec::new(), 0));
        let current = q_u32(&q, "categoryId");
        let items: Vec<Value> = categories
            .iter()
            .map(|entry| {
                json!({
                    "label": entry.title,
                    "value": entry.id.to_string(),
                    "link": builder.category(entry),
                    "is_current": entry.id == current,
                    "total": 0,
                })
            })
            .collect();
        groups.push(json!({
            "name": "Category",
            "field_name": "category",
            "items": items,
        }));
    }

    ok(Value::Array(groups))
}

/// `GET /archive/next` —— 下一篇。
async fn archive_next(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    neighbor(state, q, true).await
}

/// `GET /archive/prev` —— 上一篇。
async fn archive_prev(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    neighbor(state, q, false).await
}

async fn neighbor(
    state: AppState,
    q: HashMap<String, String>,
    next: bool,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let id = q_i64(&q, "id");
    let Some(row) = archive::get(pool, id).await.unwrap_or(None) else {
        return fail("文档不存在");
    };
    let found =
        match front_content::neighbor_archive(pool, row.category_id, row.id, next).await {
            Ok(value) => value,
            Err(err) => return failed(err),
        };
    match found {
        Some(item) => {
            let builder = links(pool).await;
            let mut value = front_content::archive_json(&item);
            if let Value::Object(map) = &mut value {
                map.insert("link".into(), json!(builder.archive(&item)));
                map.insert("logo".into(), json!(item.cover()));
                map.insert("thumb".into(), json!(item.cover()));
            }
            ok(value)
        }
        None => ok(Value::Null),
    }
}

// ---------------------------------------------------------------------------
// 分类 / 模型 / 单页
// ---------------------------------------------------------------------------

/// `GET /category/list` —— 分类列表。
async fn category_list(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let module_id = q_u32(&q, "moduleId");
    let parent_id = q_u32(&q, "parentId");
    let all = q_bool(&q, "all", false);
    let max = max_limit(pool).await;
    let (offset, limit) = q_limit(&q, max, 0);

    let (rows, total) = match front_content::api_categories(
        pool,
        module_id,
        parent_id,
        all,
        front_content::CATEGORY_TYPE_ARCHIVE,
        offset as usize,
        limit as usize,
    )
    .await
    {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let builder = links(pool).await;
    let data: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut value = front_content::category_json(row);
            if let Value::Object(map) = &mut value {
                map.insert("link".into(), json!(builder.category(row)));
                map.insert("logo".into(), json!(row.cover()));
                map.insert("thumb".into(), json!(row.cover()));
            }
            value
        })
        .collect();
    ok_total(total, Value::Array(data))
}

/// `GET /category/detail` —— 分类详情。
async fn category_detail(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let id = q_u32(&q, "id");
    let filename = {
        let value = q_str(&q, "filename");
        let catname = q_str(&q, "catname");
        if catname.is_empty() {
            value
        } else {
            catname
        }
    };
    let found = if id > 0 {
        category::get(pool, id).await.unwrap_or(None)
    } else if !filename.is_empty() {
        category::get_by_token(pool, &filename).await.unwrap_or(None)
    } else {
        None
    };
    let Some(row) = found else {
        return fail("no category found");
    };
    let builder = links(pool).await;
    let mut value = front_content::category_json(&row);
    if let Value::Object(map) = &mut value {
        map.insert("link".into(), json!(builder.category(&row)));
        map.insert("logo".into(), json!(row.cover()));
        map.insert("thumb".into(), json!(row.cover()));
    }
    ok(value)
}

/// `GET /module/list` —— 模型列表。
async fn module_list(State(state): State<AppState>) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let rows = match module::list(pool).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let builder = links(pool).await;
    let data: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut value = serde_json::to_value(row).unwrap_or_else(|_| json!({}));
            if let Value::Object(map) = &mut value {
                map.insert("link".into(), json!(builder.module(row)));
            }
            value
        })
        .collect();
    ok(Value::Array(data))
}

/// `GET /module/detail` —— 模型详情。
async fn module_detail(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let id = q_u32(&q, "id");
    let filename = q_str(&q, "filename");
    let found = if id > 0 {
        module::get(pool, id).await.unwrap_or(None)
    } else if !filename.is_empty() {
        module::get_by_token(pool, &filename).await.unwrap_or(None)
    } else {
        None
    };
    let Some(row) = found else {
        return fail("模型不存在");
    };
    let builder = links(pool).await;
    let mut value = serde_json::to_value(&row).unwrap_or_else(|_| json!({}));
    if let Value::Object(map) = &mut value {
        map.insert("link".into(), json!(builder.module(&row)));
    }
    ok(value)
}

/// `GET /page/list` —— 单页列表。
async fn page_list(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let max = max_limit(pool).await;
    let (offset, limit) = q_limit(&q, max, 0);
    let (rows, total) = match front_content::api_categories(
        pool,
        0,
        0,
        true,
        front_content::CATEGORY_TYPE_PAGE,
        offset as usize,
        limit as usize,
    )
    .await
    {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let builder = links(pool).await;
    let data: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut value = front_content::category_json(row);
            if let Value::Object(map) = &mut value {
                map.insert("link".into(), json!(builder.page(row)));
                map.insert("logo".into(), json!(row.cover()));
                map.insert("thumb".into(), json!(row.cover()));
            }
            value
        })
        .collect();
    ok_total(total, Value::Array(data))
}

/// `GET /page/detail` —— 单页详情。
async fn page_detail(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let id = q_u32(&q, "id");
    let filename = q_str(&q, "filename");
    let found = if id > 0 {
        category::get(pool, id).await.unwrap_or(None)
    } else if !filename.is_empty() {
        category::get_by_token(pool, &filename).await.unwrap_or(None)
    } else {
        None
    };
    let Some(row) = found else {
        return fail("not found");
    };
    let builder = links(pool).await;
    let mut value = front_content::category_json(&row);
    if let Value::Object(map) = &mut value {
        map.insert("link".into(), json!(builder.page(&row)));
        map.insert("logo".into(), json!(row.cover()));
        map.insert("thumb".into(), json!(row.cover()));
    }
    ok(value)
}

// ---------------------------------------------------------------------------
// 标签
// ---------------------------------------------------------------------------

/// `GET /tag/list` —— 标签列表。
async fn tag_list(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let max = max_limit(pool).await;
    let (mut offset, limit) = q_limit(&q, max, 10);
    let page = q_i64(&q, "page").max(1) as u32;
    let list_type = {
        let value = q_str(&q, "type");
        if value.is_empty() {
            "list".to_string()
        } else {
            value
        }
    };
    if list_type == "page" && page > 1 {
        offset = (page - 1) * limit;
    }

    let order_raw = q_str(&q, "order");
    let order = if order_raw.is_empty() {
        "id DESC".to_string()
    } else {
        front_content::parse_order(&order_raw, "")
    };

    let (rows, total) = match front_content::api_tags(
        pool,
        q_i64(&q, "itemId"),
        &q_str(&q, "q"),
        &q_id_list(&q, "categoryId"),
        &q_str(&q, "letter"),
        &order,
        limit,
        offset,
    )
    .await
    {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let builder = links(pool).await;
    let data: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut value = front_content::tag_json(row);
            if let Value::Object(map) = &mut value {
                map.insert("link".into(), json!(builder.tag(row)));
            }
            value
        })
        .collect();
    ok_total(total, Value::Array(data))
}

/// `GET /tag/detail` —— 标签详情。
async fn tag_detail(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let id = q_u32(&q, "id");
    let filename = q_str(&q, "filename");
    let found = if id > 0 {
        tag::get(pool, id).await.unwrap_or(None)
    } else if !filename.is_empty() {
        tag::get_by_token(pool, &filename).await.unwrap_or(None)
    } else {
        None
    };
    let Some(row) = found else {
        return fail("no tag found");
    };
    let builder = links(pool).await;
    let mut value = front_content::tag_json(&row);
    let (content, extra) = tag::content(pool, row.id).await.unwrap_or((String::new(), None));
    if let Value::Object(map) = &mut value {
        map.insert("link".into(), json!(builder.tag(&row)));
        map.insert("content".into(), json!(content));
        map.insert(
            "extra".into(),
            extra.unwrap_or_else(|| json!({})),
        );
    }
    ok(value)
}

/// `GET /tag/data/list` —— 标签下的内容。
async fn tag_data_list(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let id = q_u32(&q, "id");
    let filename = q_str(&q, "filename");
    let found = if id > 0 {
        tag::get(pool, id).await.unwrap_or(None)
    } else if !filename.is_empty() {
        tag::get_by_token(pool, &filename).await.unwrap_or(None)
    } else {
        None
    };
    let Some(row) = found else {
        return fail("no tag found");
    };

    let max = max_limit(pool).await;
    let (mut offset, limit) = q_limit(&q, max, 10);
    let page = q_i64(&q, "page").max(1) as u32;
    let list_type = {
        let value = q_str(&q, "type");
        if value.is_empty() {
            "list".to_string()
        } else {
            value
        }
    };
    if list_type == "page" && page > 1 {
        offset = (page - 1) * limit;
    }

    let content = content_settings(pool).await;
    let use_sort = content.get("use_sort").and_then(Value::as_i64).unwrap_or(0) == 1;
    let order_raw = q_str(&q, "order");
    let order = if order_raw.is_empty() {
        if use_sort {
            "archives.sort DESC, archives.created_time DESC".to_string()
        } else {
            "archives.created_time DESC".to_string()
        }
    } else {
        front_content::parse_order(&order_raw, "archives")
    };

    let (rows, total) = match front_content::tag_archives(pool, row.id, &order, limit, offset).await
    {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let builder = links(pool).await;
    let data = archive_list_json(pool, &rows, false, false, false, false, false, &builder).await;
    ok_total(total, Value::Array(data))
}

// ---------------------------------------------------------------------------
// 城市站
// ---------------------------------------------------------------------------

/// 城市站插件是否开启（按 `PluginPlace.Open` 的语义）。
async fn place_open(pool: &sqlx::MySqlPool) -> bool {
    place::setting_get(pool)
        .await
        .ok()
        .and_then(|value| value.get("open").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// `GET /place/list` —— 城市站列表。
async fn place_list(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if !place_open(pool).await {
        return fail("城市站功能未开启");
    }
    let max = max_limit(pool).await;
    let (offset, limit) = q_limit(&q, max, 0);
    let (rows, total) = match front_content::api_places(
        pool,
        q_u32(&q, "parentId"),
        q_bool(&q, "all", false),
        offset as usize,
        limit as usize,
    )
    .await
    {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let data: Vec<Value> = rows.iter().map(front_content::place_json).collect();
    ok_total(total, Value::Array(data))
}

/// `GET /place/detail` —— 城市站详情。
async fn place_detail(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if !place_open(pool).await {
        return fail("城市站功能未开启");
    }
    let id = q_u32(&q, "id");
    let filename = q_str(&q, "filename");
    let found = if id > 0 {
        place::get(pool, id).await.unwrap_or(None)
    } else if !filename.is_empty() {
        place::get_by_token(pool, &filename).await.unwrap_or(None)
    } else {
        None
    };
    let Some(row) = found else {
        return fail("no place found");
    };
    ok(front_content::place_json(&row))
}

// ---------------------------------------------------------------------------
// 站点信息
// ---------------------------------------------------------------------------

/// `GET /nav/list` —— 导航列表。
async fn nav_list(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let type_id = {
        let value = q_u32(&q, "typeId");
        if value == 0 {
            1
        } else {
            value
        }
    };
    match front_content::api_navs(pool, type_id).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `GET /metadata` —— 页面元信息（title / keywords / description …）。
async fn metadata(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let mut params = q.clone();
    // path 形如 "/news/123.html"：拆出 id / filename / module
    let path = q_str(&q, "path");
    if !path.is_empty() && !params.contains_key("match") {
        let segments: Vec<&str> = path
            .trim_matches('/')
            .split('/')
            .filter(|item| !item.is_empty())
            .collect();
        if let Some(last) = segments.last() {
            let last = last.trim_end_matches(".html");
            if last.chars().all(|ch| ch.is_ascii_digit()) && !last.is_empty() {
                params.insert("id".into(), last.to_string());
                params.insert("match".into(), "archive".into());
            } else {
                params.insert("filename".into(), last.to_string());
                params.insert("match".into(), "category".into());
            }
        }
    }
    let builder = links(pool).await;
    match front_content::metadata(pool, &params, &builder).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `GET /languages` —— 多语言子站列表。
async fn languages(State(state): State<AppState>) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = multilang::config_get(pool).await.unwrap_or_else(|_| json!({}));
    let open = config.get("open").and_then(Value::as_bool).unwrap_or(false);
    if !open {
        // 既有实现：多语言未开启时返回 nil（JSON 里是 null）。
        return ok(Value::Null);
    }
    match multilang::sites(pool).await {
        Ok(value) => ok(Value::Array(value)),
        Err(err) => failed(err),
    }
}

/// `GET /banner/list` —— 幻灯片列表。
async fn banner_list(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let kind = {
        let value = q_str(&q, "type");
        if value.is_empty() {
            "default".to_string()
        } else {
            value
        }
    };
    let urls = front_content::site_urls(pool).await.unwrap_or_default();
    match front_content::api_banners(pool, &kind, &urls.storage_url).await {
        Ok(value) => ok(Value::Array(value)),
        Err(err) => failed(err),
    }
}

// ---------------------------------------------------------------------------
// 友情链接
// ---------------------------------------------------------------------------

/// 校验友链接口令牌（按 `VerifyApiLinkToken` 的语义）。
async fn check_link_token(
    pool: &sqlx::MySqlPool,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
) -> Result<(), Response> {
    let expected = crate::models::plugin::import_api_get(pool)
        .await
        .ok()
        .and_then(|value| {
            value
                .get("link_token")
                .and_then(Value::as_str)
                .map(str::trim)
                .map(str::to_string)
        })
        .unwrap_or_default();
    if expected.is_empty() {
        return Err(fail("Token错误"));
    }
    let provided = headers
        .get("didi-token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .map(str::to_string)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| q_str(query, "token"));
    if provided != expected {
        return Err(fail("Token错误"));
    }
    Ok(())
}

/// `GET|POST /friendlink/check` —— 友链接口令牌校验。
async fn friendlink_check(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if let Err(response) = check_link_token(pool, &headers, &q).await {
        return response;
    }
    ok_msg("验证成功")
}

/// `GET /friendlink/list` —— 友链列表。
async fn friendlink_list(State(state): State<AppState>) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match link::list(pool).await {
        Ok(rows) => {
            let data: Vec<Value> = rows.iter().map(front_content::link_json).collect();
            ok(Value::Array(data))
        }
        Err(err) => failed(err),
    }
}

/// `POST /friendlink/create` —— 新增 / 更新友链。
async fn friendlink_create(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if let Err(response) = check_link_token(pool, &headers, &query).await {
        return response;
    }
    let form = parse_body(&headers, &body);
    let get = |key: &str| -> String {
        let value = form.get(key).cloned().unwrap_or_default();
        value.trim().to_string()
    };

    let title = get("title");
    let link_url = {
        let value = get("link");
        let alias = get("url");
        if alias.is_empty() {
            value
        } else {
            alias
        }
    };
    if link_url.is_empty() {
        return fail("link必填");
    }
    if title.is_empty() {
        return fail("请填写链接名称");
    }

    let contact = {
        let value = get("contact");
        let qq = get("qq");
        let email = get("email");
        if !qq.is_empty() {
            qq
        } else if !email.is_empty() {
            email
        } else {
            value
        }
    };

    let input = link::LinkInput {
        id: 0,
        title: title.clone(),
        link: link_url,
        back_link: get("back_link"),
        my_title: get("my_title"),
        my_link: get("my_link"),
        contact,
        remark: get("remark"),
        nofollow: get("nofollow").parse::<u8>().unwrap_or(0),
        sort: None,
        logo: get("logo"),
    };
    match link::save(pool, &input).await {
        Ok(_) => ok_msg("链接已保存"),
        Err(err) => model_error(err),
    }
}

/// `POST /friendlink/delete` —— 删除友链。
async fn friendlink_delete(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if let Err(response) = check_link_token(pool, &headers, &query).await {
        return response;
    }
    let form = parse_body(&headers, &body);
    let link_url = {
        let value = form.get("link").cloned().unwrap_or_default();
        let alias = form.get("url").cloned().unwrap_or_default();
        if alias.trim().is_empty() {
            value.trim().to_string()
        } else {
            alias.trim().to_string()
        }
    };
    if link_url.is_empty() {
        return fail("link必填");
    }
    let title = form.get("title").cloned().unwrap_or_default();
    let found = match front_content::friendlink_find(pool, &link_url, &title).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let Some(row) = found else {
        return fail("链接不存在");
    };
    match link::delete(pool, row.id).await {
        Ok(true) => ok_msg("链接已删除"),
        Ok(false) => fail("链接不存在"),
        Err(err) => failed(err),
    }
}

// ---------------------------------------------------------------------------
// 前台投稿
// ---------------------------------------------------------------------------

/// `POST /archive/publish` —— 前台投稿（需会员令牌）。
async fn archive_publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let form = parse_body(&headers, &body);

    let title = form.get("title").cloned().unwrap_or_default().trim().to_string();
    let content = form
        .get("content")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    let category_id = form
        .get("category_id")
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0);

    if category_id == 0 {
        return fail("请选择一个栏目");
    }
    if title.is_empty() {
        return fail("请填写文章标题");
    }
    if content.is_empty() {
        return fail("请填写文章内容");
    }
    let Some(item) = category::get(pool, category_id).await.unwrap_or(None) else {
        return fail("请选择一个栏目");
    };

    // 会员鉴权：Bearer / token 头 / token cookie
    let Some(token) = request_token(&headers) else {
        return fail("请先登录会员账号后再发布内容");
    };
    let secret = match auth::token_secret(pool).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let Ok(claims) = auth::verify(auth::TokenKind::User, &secret, &token) else {
        return fail("登录已失效，请重新登录");
    };
    let Some(user_id) = claims.subject(auth::TokenKind::User) else {
        return fail("请先登录会员账号后再发布内容");
    };

    // 带 id 时是编辑：只允许改自己的内容（按既有归属校验）。
    let archive_id = form
        .get("id")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0);
    let existing = if archive_id > 0 {
        archive::get(pool, archive_id).await.unwrap_or(None)
    } else {
        None
    };
    if let Some(row) = &existing {
        if row.user_id != user_id {
            return fail("无权操作该内容");
        }
    }

    let images = form
        .get("images")
        .map(|raw| {
            serde_json::from_str::<Vec<String>>(raw).unwrap_or_else(|_| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_string)
                    .collect()
            })
        })
        .unwrap_or_default();

    let input = archive_write::ArchiveInput {
        title: title.clone(),
        seo_title: form.get("seo_title").cloned().unwrap_or_default(),
        content: content.clone(),
        category_id,
        keywords: form.get("keywords").cloned().unwrap_or_default(),
        description: form.get("description").cloned().unwrap_or_default(),
        images,
        url_token: form
            .get("url_token")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        template: form.get("template").cloned().unwrap_or_default(),
        canonical_url: form.get("canonical_url").cloned().unwrap_or_default(),
        fixed_link: form.get("fixed_link").cloned().unwrap_or_default(),
        price: form
            .get("price")
            .and_then(|value| value.trim().parse::<i64>().ok())
            .unwrap_or(0),
        stock: None,
        read_level: form
            .get("read_level")
            .and_then(|value| value.trim().parse::<i32>().ok())
            .unwrap_or(0),
        sort: form
            .get("sort")
            .and_then(|value| value.trim().parse::<u32>().ok())
            .unwrap_or(0),
        views: None,
        origin_url: form.get("origin_url").cloned().unwrap_or_default(),
        created_time: None,
        extra: None,
    };

    let saved = match existing {
        // 编辑：走 archive_write::update（保持原 url_token 等）
        Some(_) => archive_write::update(pool, archive_id, &input).await,
        // 新建
        None => archive_write::create(pool, &input).await,
    };
    let saved = match saved {
        Ok(value) => value,
        Err(err) => return model_error(err),
    };
    if existing.is_none() {
        // 归属到发布者（`archive_write::create` 固定写 0）
        if let Err(err) = front_content::set_archive_owner(pool, saved.id, user_id).await {
            return failed(err);
        }
    }

    let builder = links(pool).await;
    let link = match archive::get(pool, saved.id).await {
        Ok(Some(row)) => builder.archive(&row),
        _ => String::new(),
    };
    Json(json!({
        "code": 0,
        "msg": "发布成功",
        "data": {
            "id": saved.id,
            "url_token": saved.url_token,
            "link": link,
            "module_id": item.module_id,
            "category_id": category_id,
        },
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// 请求体解析
// ---------------------------------------------------------------------------

/// 解析请求体：JSON 或表单（既有的接口两种都收）。
fn parse_body(headers: &HeaderMap, body: &Bytes) -> HashMap<String, String> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if content_type.contains("application/json") {
        if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(body) {
            return map
                .into_iter()
                .filter_map(|(key, value)| {
                    let text = match value {
                        Value::String(text) => text,
                        Value::Number(number) => number.to_string(),
                        Value::Bool(flag) => flag.to_string(),
                        _ => return None,
                    };
                    Some((key, text))
                })
                .collect();
        }
        return HashMap::new();
    }
    parse_form(body)
}

/// 解析 `application/x-www-form-urlencoded` 请求体。
fn parse_form(body: &Bytes) -> HashMap<String, String> {
    let text = String::from_utf8_lossy(body);
    let mut out: HashMap<String, String> = HashMap::new();
    for pair in text.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((key, value)) => (key, value),
            None => (pair, ""),
        };
        let key = percent_decode(key);
        if key.is_empty() {
            continue;
        }
        let value = percent_decode(&value.replace('+', " "));
        out.entry(key).or_insert(value);
    }
    out
}

fn percent_decode(raw: &str) -> String {
    percent_encoding::percent_decode_str(raw)
        .decode_utf8_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_booleans_like_go() {
        assert!(parse_bool("1"));
        assert!(parse_bool("true"));
        assert!(parse_bool("TRUE"));
        assert!(parse_bool("yes"));
        assert!(!parse_bool("0"));
        assert!(!parse_bool("false"));
        assert!(!parse_bool(""));
    }

    #[test]
    fn parses_limit_parameter() {
        let mut q = HashMap::new();
        // 缺省：既有的默认值
        assert_eq!(q_limit(&q, 100, 10), (0, 10));
        // 单值
        q.insert("limit".into(), "20".into());
        assert_eq!(q_limit(&q, 100, 10), (0, 20));
        // 偏移 + 条数
        q.insert("limit".into(), "30,20".into());
        assert_eq!(q_limit(&q, 100, 10), (30, 20));
        // 超过上限被夹住
        q.insert("limit".into(), "500".into());
        assert_eq!(q_limit(&q, 100, 10), (0, 100));
        // 0 被夹到 1
        q.insert("limit".into(), "0".into());
        assert_eq!(q_limit(&q, 100, 10), (0, 1));
    }

    #[test]
    fn parses_id_lists() {
        let mut q = HashMap::new();
        q.insert("categoryId".into(), "1, 2,abc,0,3".into());
        assert_eq!(q_id_list(&q, "categoryId"), vec![1, 2, 3]);
        assert!(q_id_list(&q, "missing").is_empty());
    }

    #[test]
    fn parses_form_and_json_bodies() {
        let form = parse_form(&Bytes::from_static(b"title=%E6%B5%8B%E8%AF%95&content=hello+world"));
        assert_eq!(form.get("title").map(String::as_str), Some("测试"));
        assert_eq!(form.get("content").map(String::as_str), Some("hello world"));

        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        );
        let json_body = parse_body(&headers, &Bytes::from_static(br#"{"title":"x","id":3}"#));
        assert_eq!(json_body.get("title").map(String::as_str), Some("x"));
        assert_eq!(json_body.get("id").map(String::as_str), Some("3"));
    }
}

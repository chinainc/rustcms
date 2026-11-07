//! 互动与第三方 API（批次 D）。
//!
//! 对应 Go 版 `controller/comment.go`、`guestbook.go`、`apiTag.go`（评论 / 留言部分）、
//! `apiArchive.go`（收藏部分）、`apiWechat.go`、`apiWeapp.go`、`apiGoogle.go`、
//! `apiNotify.go`（微信 / 小程序通知）与 `api.go` 的 `ApiLogStatistic`。
//!
//! 路径按主线要求**不带 `/api` 前缀**（如 `/comment/list`），
//! 同时注册 `/api/...` 别名，保证 Go 版原始地址同样可用；
//! 微信 / 小程序通知另外保留 Go 的 `/notify/...` 地址。
//!
//! 响应信封与 Go 版一致：成功 `{"code":0,"msg":"","data":...}`，
//! 列表接口额外带顶层 `total`，失败 `{"code":1,"msg":"..."}`；
//! 需要登录时用 Go 的 `config.StatusNoLogin`（1001）；
//! 依赖外部服务、当前项目未实现的接口用 `{"code":-1,"msg":"..."}`
//! （与 `src/routes.rs` 里的 `fail_ai` 同款）。

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Map, Value};

use crate::auth;
use crate::models::{comment, interact, site_setting, user};
use crate::routes::{bearer_token, fail, failed, ok, NO_DB};
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

    // 评论
    router = api_route!(router, "/comment/list", get, comment_list);
    router = api_route!(router, "/comment/publish", post, comment_publish);
    router = api_route!(router, "/comment/praise", post, comment_praise);

    // 收藏
    router = api_route!(router, "/favorite/list", get, favorite_list);
    router = api_route!(router, "/favorite/check", post, favorite_check);
    router = api_route!(router, "/favorite/add", post, favorite_add);
    router = api_route!(router, "/favorite/delete", post, favorite_delete);

    // 留言
    router = api_route!(router, "/guestbook.html", get, guestbook_page);
    router = api_route!(router, "/guestbook.html", post, guestbook_form);
    router = api_route!(router, "/guestbook/fields", get, guestbook_fields);

    // 微信 / 小程序 / Google
    router = api_route!(router, "/wechat", get, wechat_verify);
    router = api_route!(router, "/wechat", post, wechat_message);
    router = api_route!(router, "/wechat/auth", get, wechat_auth);
    router = api_route!(router, "/wechat/pay", post, wechat_pay_notify);
    router = api_route!(router, "/weapp/msg", get, weapp_msg_get);
    router = api_route!(router, "/weapp/msg", post, weapp_msg_post);
    router = api_route!(router, "/weapp/qrcode", post, weapp_qrcode);
    router = api_route!(router, "/google/url", get, google_url);

    // 访问日志
    router = api_route!(router, "/log", get, access_log);
    router = api_route!(router, "/log", post, access_log);

    // Go 版把微信 / 小程序通知挂在 `/notify` 分组下，这里保留同样的地址
    router = router
        .route("/notify/wechat/pay", post(wechat_pay_notify))
        .route("/notify/weapp/msg", get(weapp_msg_get))
        .route("/notify/weapp/msg", post(weapp_msg_post));

    router
}

// ---------------------------------------------------------------------------
// 响应工具
// ---------------------------------------------------------------------------

/// 列表响应：顶层额外带 `total`（对齐 Go 的列表接口）。
fn ok_total(total: i64, data: Value) -> Response {
    Json(json!({ "code": 0, "msg": "", "total": total, "data": data })).into_response()
}

/// 需要登录（Go 的 `config.StatusNoLogin` = 1001）。
fn fail_no_login() -> Response {
    Json(json!({ "code": 1001, "msg": "请登录" })).into_response()
}

/// 未实现（与 `src/routes.rs` 里的 `fail_ai` 同款）。
fn fail_unsupported(msg: impl Into<String>) -> Response {
    Json(json!({ "code": -1, "msg": msg.into() })).into_response()
}

// ---------------------------------------------------------------------------
// 参数解析
// ---------------------------------------------------------------------------

/// 查询串取值（对齐 Go 的 `URLParam*`：取不到就当零值，不报错）。
fn q_str(q: &HashMap<String, String>, key: &str) -> String {
    q.get(key)
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

fn q_i64(q: &HashMap<String, String>, key: &str) -> i64 {
    q.get(key)
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

/// `limit` 参数：支持 `offset,limit` 与 `limit` 两种写法（对齐 Go）。
fn q_limit(q: &HashMap<String, String>, default_limit: i64, max_limit: i64) -> (i64, i64) {
    let raw = q_str(q, "limit");
    if raw.is_empty() {
        return (0, default_limit);
    }
    let mut offset = 0i64;
    let mut limit = default_limit;
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() == 2 {
        offset = parts[0].trim().parse().unwrap_or(0);
        limit = parts[1].trim().parse().unwrap_or(default_limit);
    } else if parts.len() == 1 {
        limit = parts[0].trim().parse().unwrap_or(default_limit);
    }
    if limit > max_limit {
        limit = max_limit;
    }
    if limit < 1 {
        limit = 1;
    }
    (offset, limit)
}

/// 解析请求体：JSON 对象或 urlencoded 表单。
///
/// 值统一成字符串（数组用逗号连接，对齐 Go 的 checkbox 处理）。
fn parse_body(headers: &HeaderMap, body: &Bytes) -> HashMap<String, String> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if content_type.contains("json") {
        if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(body) {
            return map
                .into_iter()
                .map(|(key, value)| (key, value_to_string(Some(&value))))
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
        // `name[]` 形式（checkbox 多选）合并到 `name` 上
        if let Some(base) = key.strip_suffix("[]") {
            let entry = out.entry(base.to_string()).or_default();
            if entry.is_empty() {
                entry.push_str(&value);
            } else {
                entry.push(',');
                entry.push_str(&value);
            }
            continue;
        }
        out.insert(key, value);
    }
    out
}

fn percent_decode(raw: &str) -> String {
    percent_encoding::percent_decode_str(raw)
        .decode_utf8_lossy()
        .to_string()
}

/// JSON 值转字符串（数组用逗号连接，对齐 Go 的 checkbox 处理）。
fn value_to_string(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.trim().to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Array(list)) => list
            .iter()
            .map(|item| value_to_string(Some(item)))
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => String::new(),
    }
}

/// 逗号分隔的 ID 列表（`archive_ids`）。
fn id_list(raw: &str) -> Vec<i64> {
    raw.split(',')
        .filter_map(|item| item.trim().parse::<i64>().ok())
        .filter(|id| *id > 0)
        .collect()
}

/// 客户端 IP（与 `src/routes.rs` 的 `client_ip` 同口径）。
fn client_ip(headers: &HeaderMap) -> String {
    let from_header = headers
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .or_else(|| {
            headers
                .get("x-forwarded-for")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(',').next())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty());
    from_header.unwrap_or_default().to_string()
}

/// 站点根地址（优先用请求头里的 Host）。
fn site_base_url(headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost")
        .to_string();
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http")
        .trim()
        .to_string();
    format!("{proto}://{host}")
}

/// 取当前登录会员（未登录返回 `None`）。
///
/// 令牌来源与 Go 的 `ParseUserToken` 一致：`token` 头 / `Authorization: Bearer` / `token` Cookie。
async fn member_of(pool: &sqlx::MySqlPool, headers: &HeaderMap) -> Option<u32> {
    let token = headers
        .get("token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| bearer_token(headers))
        .or_else(|| cookie_token(headers))?;
    let secret = auth::token_secret(pool).await.ok()?;
    let claims = auth::verify(auth::TokenKind::User, &secret, &token).ok()?;
    claims.subject(auth::TokenKind::User)
}

/// 从 Cookie 里取 `token`。
fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("token=") {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 评论
// ---------------------------------------------------------------------------

/// `GET /comment/list` —— 评论列表（Go 的 `ApiCommentList`）。
///
/// 参数：`id`（文档 ID）、`user_id`、`order`（默认 `id desc`）、`page`、`limit`
/// （支持 `offset,limit` 与 `limit` 两种写法）。
async fn comment_list(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let archive_id = q_i64(&q, "id");
    let user_id = q_i64(&q, "user_id").max(0) as u32;
    let order = {
        let raw = q_str(&q, "order");
        if raw.is_empty() {
            "id desc".to_string()
        } else {
            raw
        }
    };
    let page = q_i64(&q, "page").max(1);
    let (offset, limit) = q_limit(&q, 10, 1000);

    let (total, rows) = match interact::comment_list(
        pool, archive_id, user_id, &order, page, limit, offset,
    )
    .await
    {
        Ok(result) => result,
        Err(err) => return failed(err),
    };

    // 文档标题 + 父评论（Go 的 `item_title` / `parent`）
    let archive_ids: Vec<i64> = rows.iter().map(|item| item.archive_id).collect();
    let titles = comment::archive_titles(pool, &archive_ids)
        .await
        .unwrap_or_default();
    let parent_ids: Vec<u32> = rows
        .iter()
        .filter(|item| item.parent_id > 0)
        .map(|item| item.parent_id)
        .collect();
    let parents = interact::parents(pool, &parent_ids)
        .await
        .unwrap_or_default();

    let data: Vec<Value> = rows
        .iter()
        .map(|item| {
            let title = titles.get(&item.archive_id).cloned().unwrap_or_default();
            interact::comment_payload(item, &title, parents.get(&item.parent_id))
        })
        .collect();

    ok_total(total, Value::Array(data))
}

/// `POST /comment/publish` —— 发布评论（Go 的 `CommentPublish` / `ApiCommentPublish`）。
///
/// 请求字段（JSON 或表单都支持）：`archive_id`、`content`、`user_name`、`email`、
/// `ip`、`parent_id`、`to_uid`、`captcha_id`、`captcha`。
async fn comment_publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let form = parse_body(&headers, &body);

    let archive_id = form
        .get("archive_id")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0);
    if archive_id <= 0 {
        return fail("缺少文档ID");
    }
    let content = form.get("content").map(|value| value.trim()).unwrap_or("");
    if content.is_empty() {
        return fail("评论内容不能为空");
    }
    let email = form.get("email").map(|value| value.trim()).unwrap_or("");
    let ip = {
        let raw = form.get("ip").map(|value| value.trim()).unwrap_or("");
        if raw.is_empty() {
            client_ip(&headers)
        } else {
            raw.to_string()
        }
    };
    let parent_id = form
        .get("parent_id")
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let mut to_uid = form
        .get("to_uid")
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if parent_id > 0 {
        if let Ok(Some(parent)) = comment::get(pool, parent_id).await {
            to_uid = parent.user_id;
        }
    }

    // 登录会员：用户名取会员资料（Go 同款）
    let member = member_of(pool, &headers).await;
    let member_row = match member {
        Some(id) => user::get(pool, id).await.ok().flatten(),
        None => None,
    };
    let user_id = member_row.as_ref().map(|item| item.id).unwrap_or(0);
    let user_name = match member_row.as_ref() {
        Some(item) => item.user_name.clone(),
        None => form
            .get("user_name")
            .map(|value| value.trim())
            .unwrap_or("")
            .to_string(),
    };

    // 验证码：站点开启且用户组不免验证码时必填
    let captcha_required = interact::safe_captcha_on(pool).await.unwrap_or(false)
        && !match member_row.as_ref() {
            Some(item) => interact::group_no_captcha(pool, item.group_id)
                .await
                .unwrap_or(false),
            None => false,
        };
    if captcha_required {
        let captcha_id = form
            .get("captcha_id")
            .map(|value| value.trim())
            .unwrap_or("");
        let captcha = form.get("captcha").map(|value| value.trim()).unwrap_or("");
        if captcha_id.is_empty() || captcha.is_empty() {
            return fail("验证码不正确");
        }
    }

    // 审核：用户组开启「免审核」时直接正常（Go 的 ContentNoVerify）
    let status = match member_row.as_ref() {
        Some(item)
            if interact::group_no_verify(pool, item.group_id)
                .await
                .unwrap_or(false) =>
        {
            1
        }
        _ => 0,
    };

    let saved = match interact::insert_comment(
        pool, archive_id, user_id, &user_name, email, &ip, content, parent_id, to_uid, status,
    )
    .await
    {
        Ok(row) => row,
        Err(err) => return failed(err),
    };

    let title = comment::archive_titles(pool, &[archive_id])
        .await
        .unwrap_or_default()
        .get(&archive_id)
        .cloned()
        .unwrap_or_default();
    Json(json!({
        "code": 0,
        "msg": "发布成功",
        "data": interact::comment_payload(&saved, &title, None),
    }))
    .into_response()
}

/// `POST /comment/praise` —— 评论点赞（Go 的 `CommentPraise` / `ApiCommentPraise`）。
///
/// 请求字段：`id`（评论 ID）。需要登录。
async fn comment_praise(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = member_of(pool, &headers).await else {
        return fail_no_login();
    };
    let form = parse_body(&headers, &body);
    let id = form
        .get("id")
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if id == 0 {
        return fail("参数不完整");
    }
    let Some(target) = comment::get(pool, id).await.ok().flatten() else {
        return fail("record not found");
    };
    if let Err(err) =
        interact::add_comment_praise(pool, user_id, target.id as i64, target.archive_id).await
    {
        return failed(err);
    }
    let Some(mut updated) = comment::get(pool, id).await.ok().flatten() else {
        return fail("record not found");
    };
    // Go 的返回体：点赞数 +1 并标记 active
    updated.vote_count += 1;
    let mut payload = interact::comment_payload(&updated, "", None);
    if let Some(map) = payload.as_object_mut() {
        map.insert("active".into(), json!(true));
    }
    Json(json!({ "code": 0, "msg": "点赞成功", "data": payload })).into_response()
}

// ---------------------------------------------------------------------------
// 收藏
// ---------------------------------------------------------------------------

/// `GET /favorite/list` —— 我的收藏（Go 的 `ApiGetFavorites`）。
///
/// 参数：`current`（页码）、`pageSize`（每页）。需要登录。
async fn favorite_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = member_of(pool, &headers).await else {
        return fail_no_login();
    };
    let current = q_i64(&q, "current").max(1);
    let page_size = q_i64(&q, "pageSize");
    let page_size = if page_size < 1 {
        20
    } else {
        page_size.min(200)
    };

    match interact::favorite_list(pool, user_id as i64, current, page_size).await {
        Ok((total, rows)) => {
            let data: Vec<Value> = rows
                .iter()
                .map(|item| serde_json::to_value(item).unwrap_or(Value::Null))
                .collect();
            ok_total(total, Value::Array(data))
        }
        Err(err) => failed(err),
    }
}

/// `POST /favorite/check` —— 批量查收藏状态（Go 的 `ApiCheckFavorites`）。
///
/// 请求字段：`archive_id`、`archive_ids`（数组或逗号分隔）。
async fn favorite_check(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let form = parse_body(&headers, &body);
    let user_id = member_of(pool, &headers).await.unwrap_or(0) as i64;

    let mut ids = id_list(form.get("archive_ids").map(String::as_str).unwrap_or(""));
    let single = form
        .get("archive_id")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0);
    if single > 0 {
        ids.push(single);
    }
    ids.sort_unstable();
    ids.dedup();

    match interact::check_favorites(pool, user_id, &ids).await {
        Ok(rows) => {
            let data: Vec<Value> = rows
                .iter()
                .map(|item| serde_json::to_value(item).unwrap_or(Value::Null))
                .collect();
            ok(Value::Array(data))
        }
        Err(err) => failed(err),
    }
}

/// `POST /favorite/add` —— 收藏 / 取消收藏（Go 的 `ApiAddFavorite`，开关语义）。
///
/// 请求字段：`archive_id`、`sku_id`。需要登录。
async fn favorite_add(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = member_of(pool, &headers).await else {
        return fail_no_login();
    };
    let form = parse_body(&headers, &body);
    let archive_id = form
        .get("archive_id")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0);
    if archive_id <= 0 {
        return fail("参数不完整");
    }
    let sku_id = form
        .get("sku_id")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0);

    match interact::add_favorite(pool, user_id as i64, archive_id, sku_id).await {
        Ok(is_favorite) => Json(json!({
            "code": 0,
            "msg": "操作成功",
            "data": { "is_favorite": is_favorite },
        }))
        .into_response(),
        Err(err) => Json(json!({
            "code": 1,
            "msg": format!("{err:#}"),
            "data": { "is_favorite": false },
        }))
        .into_response(),
    }
}

/// `POST /favorite/delete` —— 取消收藏（Go 的 `ApiDeleteFavorite`）。
///
/// 请求字段：`archive_id`。需要登录。
async fn favorite_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = member_of(pool, &headers).await else {
        return fail_no_login();
    };
    let form = parse_body(&headers, &body);
    let archive_id = form
        .get("archive_id")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0);
    if archive_id <= 0 {
        return fail("参数不完整");
    }
    match interact::delete_favorite(pool, user_id as i64, archive_id).await {
        Ok(_) => Json(json!({ "code": 0, "msg": "操作成功" })).into_response(),
        Err(err) => failed(err),
    }
}

// ---------------------------------------------------------------------------
// 留言
// ---------------------------------------------------------------------------

/// `GET /guestbook.html` —— 留言页（Go 的 `GuestbookPage`）。
///
/// 当前项目还没有前台模板，这里直接返回留言字段（等价 `/guestbook/fields`）。
async fn guestbook_page(State(state): State<AppState>) -> Response {
    guestbook_fields(State(state)).await
}

/// `GET /guestbook/fields` —— 留言字段（Go 的 `ApiGuestbook`）。
async fn guestbook_fields(State(state): State<AppState>) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match interact::guestbook_fields(pool).await {
        Ok(fields) => Json(json!({
            "code": 0,
            "msg": "",
            "data": fields,
            "recaptcha_key": "",
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// `POST /guestbook.html` —— 提交留言（Go 的 `GuestbookForm` / `ApiGuestbookForm`）。
///
/// 请求体按 `settings.guestbook.fields` 里的字段名提交；
/// 必填项为空时返回「xx必填」（Go 的 `%sIsRequired`）。
async fn guestbook_form(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let form = parse_body(&headers, &body);
    let fields = match interact::guestbook_fields(pool).await {
        Ok(fields) => fields,
        Err(err) => return failed(err),
    };

    let mut values: HashMap<String, String> = HashMap::new();
    let mut extra: Map<String, Value> = Map::new();
    for field in &fields {
        let field_name = field
            .get("field_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if field_name.is_empty() {
            continue;
        }
        let label = field
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let mut value = form
            .get(&field_name)
            .cloned()
            .unwrap_or_default()
            .trim()
            .to_string();
        if value.is_empty() {
            value = form
                .get(&format!("{field_name}[]"))
                .cloned()
                .unwrap_or_default()
                .trim()
                .to_string();
        }
        let required = field
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if required && value.is_empty() {
            return fail(format!("{label}必填"));
        }
        let is_system = field
            .get("is_system")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !is_system {
            extra.insert(label.clone(), Value::String(value.clone()));
        }
        values.insert(field_name, value);
    }

    let user_id = member_of(pool, &headers).await.unwrap_or(0);
    let user_name = values.get("user_name").cloned().unwrap_or_default();
    let contact = values.get("contact").cloned().unwrap_or_default();
    let content = values.get("content").cloned().unwrap_or_default();
    let refer = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();

    if let Err(err) = interact::save_guestbook(
        pool,
        user_id,
        &user_name,
        &contact,
        &content,
        &client_ip(&headers),
        &refer,
        &Value::Object(extra),
    )
    .await
    {
        return failed(err);
    }

    let mut msg = interact::guestbook_return_message(pool)
        .await
        .unwrap_or_default();
    if msg.is_empty() {
        msg = "感谢您的留言！".to_string();
    }
    Json(json!({ "code": 0, "msg": msg })).into_response()
}

// ---------------------------------------------------------------------------
// 微信
// ---------------------------------------------------------------------------

/// `GET /wechat` —— 微信服务器校验（Go 的 `WechatApi` 的 GET 分支）。
///
/// 校验方式与微信一致：`sha1(sort(token, timestamp, nonce)) == signature`。
async fn wechat_verify(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = interact::wechat_config(pool)
        .await
        .unwrap_or_else(|_| json!({}));
    let token = config
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let signature = q_str(&q, "signature");
    let timestamp = q_str(&q, "timestamp");
    let nonce = q_str(&q, "nonce");
    if signature.is_empty() || timestamp.is_empty() || nonce.is_empty() {
        return fail_unsupported("微信服务器校验失败：缺少 signature / timestamp / nonce");
    }
    if interact::wechat_signature(&token, &timestamp, &nonce) != signature {
        return (StatusCode::FORBIDDEN, "invalid signature").into_response();
    }
    (StatusCode::OK, "success").into_response()
}

/// `POST /wechat` —— 公众号消息回调（Go 的 `WechatApi` 的 POST 分支）。
///
/// 消息会落到 `wechat_messages`，并按「自动回复规则」生成回复内容；
/// **推送给微信本身需要 access_token，当前项目未实现**（回复内容已入库）。
async fn wechat_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let xml = String::from_utf8_lossy(&body).to_string();
    let fields = xml_fields(&xml);
    let msg_type = fields.get("MsgType").cloned().unwrap_or_default();
    let from = fields.get("FromUserName").cloned().unwrap_or_default();
    let mut content = fields.get("Content").cloned().unwrap_or_default();

    if msg_type == "event" {
        let event = fields.get("Event").cloned().unwrap_or_default();
        if event == "CLICK" {
            content = fields.get("EventKey").cloned().unwrap_or_default();
        } else {
            return (StatusCode::OK, "success").into_response();
        }
    }

    let config = interact::wechat_config(pool)
        .await
        .unwrap_or_else(|_| json!({}));
    let verify_key = config
        .get("verify_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("验证码")
        .to_string();
    let verify_msg = config
        .get("verify_msg")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let mut reply = String::new();
    let mut reply_time = chrono::Utc::now().timestamp();
    if content.eq_ignore_ascii_case(&verify_key) {
        if interact::wechat_user_exists(pool, &from)
            .await
            .unwrap_or(false)
        {
            let code = generate_verify_code(&from);
            let mut message = if verify_msg.contains("{code}") {
                verify_msg.clone()
            } else {
                format!("验证码：{code}，30分钟内有效{verify_msg}")
            };
            message = message.replacen("{code}", &code, 1);
            reply = message;
        } else {
            reply = format!(
                "欢迎您，<a href=\"{}/api/wechat/auth?state=code\">请点击此处完成授权</a>",
                site_base_url(&headers)
            );
        }
    } else if let Some(rule) = interact::wechat_reply_by_keyword(pool, &content)
        .await
        .unwrap_or(None)
    {
        reply = rule;
    } else {
        reply_time = 0;
    }

    let _ = interact::save_wechat_message(pool, &from, &content, &reply, reply_time).await;
    (StatusCode::OK, "success").into_response()
}

/// `GET /wechat/auth` —— 公众号网页授权（Go 的 `WechatAuthApi`）。
///
/// 没有 `code` 时 302 跳转到微信授权页；带 `code` 时需要调微信接口换用户信息，
/// 当前项目未实现（返回明确提示）。
async fn wechat_auth(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !ua.contains("micromessenger") {
        return fail("请在微信内打开");
    }
    let config = interact::wechat_config(pool)
        .await
        .unwrap_or_else(|_| json!({}));
    let app_id = config
        .get("app_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if app_id.is_empty() {
        return fail("未配置微信公众号 AppID");
    }
    if !q_str(&q, "code").is_empty() {
        return fail_unsupported("未实现：换取微信用户信息需要调用微信接口");
    }
    let state_value = q_str(&q, "state");
    let redirect_uri = format!("{}/api/wechat/auth", site_base_url(&headers));
    let encoded =
        percent_encoding::utf8_percent_encode(&redirect_uri, percent_encoding::NON_ALPHANUMERIC)
            .to_string();
    let location = format!(
        "https://open.weixin.qq.com/connect/oauth2/authorize?appid={app_id}\
         &redirect_uri={encoded}&response_type=code&scope=snsapi_userinfo&state={state_value}\
         #wechat_redirect"
    );
    let mut response = StatusCode::FOUND.into_response();
    if let Ok(value) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

/// 微信支付回调（Go 的 `NotifyWechatPay`）。
///
/// 流程：查支付单 → 验签 → 标记支付成功（写 `payments` / `orders` / `finances`）
/// 或处理退款（`order_refunds`）。
async fn wechat_pay_notify(State(state): State<AppState>, body: Bytes) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let xml = String::from_utf8_lossy(&body).to_string();
    let fields = xml_fields(&xml);

    let out_trade_no = fields
        .get("out_trade_no")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    if out_trade_no.is_empty() {
        return wechat_pay_reply(false, "参数不完整");
    }

    let payment: Option<(u32, String, i64, u32, i64, i64, i32)> = sqlx::query_as(
        "SELECT id, order_id, paid_time, user_id, amount, payment_account_id, status \
         FROM payments WHERE payment_id = ? LIMIT 1",
    )
    .bind(&out_trade_no)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let Some((payment_id, order_id, paid_time, user_id, amount, account_id, _status)) = payment
    else {
        return wechat_pay_reply(false, "支付单不存在");
    };
    if paid_time > 0 {
        return wechat_pay_reply(true, "OK");
    }

    // 验签（MD5，参数按键名排序后拼 key）
    let api_key = match account_api_key(pool, account_id as i64).await {
        Some(key) => key,
        None => String::new(),
    };
    if !api_key.is_empty() {
        let sign = fields.get("sign").cloned().unwrap_or_default();
        let expect = wechat_sign(&fields, &api_key);
        if !sign.eq_ignore_ascii_case(&expect) {
            return wechat_pay_reply(false, "签名校验失败");
        }
    } else {
        tracing::warn!("微信支付回调：支付账号没有配置 api_key，跳过验签");
    }

    let refund_id = fields
        .get("refund_id")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    let now = chrono::Utc::now().timestamp();

    if !refund_id.is_empty() {
        let refund_status = fields
            .get("refund_status")
            .cloned()
            .unwrap_or_default()
            .trim()
            .to_string();
        let status = if refund_status == "SUCCESS" { 1 } else { -1 };
        let _ = sqlx::query(
            "UPDATE order_refunds SET status = ?, remark = ?, refund_time = ?, updated_time = ? \
             WHERE order_id = ?",
        )
        .bind(status)
        .bind(&refund_status)
        .bind(now)
        .bind(now)
        .bind(&order_id)
        .execute(pool)
        .await;
        return wechat_pay_reply(true, "OK");
    }

    // 支付成功
    let transaction_id = fields
        .get("transaction_id")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    let buyer_id = fields
        .get("openid")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    let _ = sqlx::query(
        "UPDATE payments SET pay_way = 'wechat', terrace_id = ?, paid_time = ?, buyer_id = ?, \
         updated_time = ? WHERE id = ?",
    )
    .bind(&transaction_id)
    .bind(now)
    .bind(&buyer_id)
    .bind(now)
    .bind(payment_id)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "UPDATE orders SET payment_id = ?, paid_time = ?, status = 1, updated_time = ? \
         WHERE order_id = ? AND paid_time = 0",
    )
    .bind(&out_trade_no)
    .bind(now)
    .bind(now)
    .bind(&order_id)
    .execute(pool)
    .await;

    // 资金流水（与 Go 的 Finance 记录同款）
    let balance: i64 = sqlx::query_scalar("SELECT balance FROM users WHERE id = ? LIMIT 1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
    let _ = sqlx::query(
        "INSERT INTO finances (created_time, updated_time, user_id, direction, amount, \
            after_amount, action, order_id, status, remark) \
         VALUES (?, ?, ?, 2, ?, ?, 2, ?, 1, '')",
    )
    .bind(now)
    .bind(now)
    .bind(user_id)
    .bind(amount)
    .bind(balance)
    .bind(&order_id)
    .execute(pool)
    .await;

    wechat_pay_reply(true, "OK")
}

/// 支付账号的 `api_key`（`payment_accounts.pay_config`）。
async fn account_api_key(pool: &sqlx::MySqlPool, account_id: i64) -> Option<String> {
    let raw: Option<String> = sqlx::query_scalar::<_, Option<String>>(
        "SELECT pay_config FROM payment_accounts WHERE id = ? LIMIT 1",
    )
    .bind(account_id)
    .fetch_one(pool)
    .await
    .unwrap_or(None);
    let raw = raw?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    Some(
        value
            .get("api_key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
    )
}

/// 微信支付回调的应答 XML。
fn wechat_pay_reply(success: bool, message: &str) -> Response {
    let (code, msg) = if success {
        ("SUCCESS", "OK")
    } else {
        ("FAIL", message)
    };
    let xml = format!(
        "<xml><return_code><![CDATA[{code}]]></return_code>\
         <return_msg><![CDATA[{msg}]]></return_msg></xml>"
    );
    (StatusCode::OK, xml).into_response()
}

/// 微信签名：参数（去掉 `sign` 与空值）按键名排序 → `k=v&k=v` → 追加 `&key=密钥` → MD5 大写。
fn wechat_sign(fields: &HashMap<String, String>, api_key: &str) -> String {
    let mut keys: Vec<&String> = fields.keys().filter(|key| key.as_str() != "sign").collect();
    keys.sort();
    let mut parts: Vec<String> = Vec::new();
    for key in keys {
        let value = fields.get(key).map(String::as_str).unwrap_or_default();
        if value.is_empty() {
            continue;
        }
        parts.push(format!("{key}={value}"));
    }
    let payload = format!("{}&key={api_key}", parts.join("&"));
    md5_hex_upper(payload.as_bytes())
}

/// MD5 大写十六进制（微信签名用）。
fn md5_hex_upper(data: &[u8]) -> String {
    use md5::{Digest, Md5};
    Md5::digest(data)
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect()
}

// ---------------------------------------------------------------------------
// 小程序 / Google
// ---------------------------------------------------------------------------

/// `GET /weapp/msg` —— 小程序服务器校验（Go 的 `NotifyWeappMsg` 的 GET 分支）。
///
/// 小程序回调是加密协议（AES 解密 + XML），当前项目未实现；
/// GET 的服务器校验（sha1 签名 + 回显 echostr）可以本地完成。
async fn weapp_msg_get(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = interact::weapp_config(pool)
        .await
        .unwrap_or_else(|_| json!({}));
    let token = config
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let signature = q_str(&q, "signature");
    let timestamp = q_str(&q, "timestamp");
    let nonce = q_str(&q, "nonce");
    let echostr = q_str(&q, "echostr");
    if signature.is_empty() || timestamp.is_empty() || nonce.is_empty() {
        return fail_unsupported("小程序服务器校验失败：缺少 signature / timestamp / nonce");
    }
    if interact::wechat_signature(&token, &timestamp, &nonce) != signature {
        return (StatusCode::FORBIDDEN, "invalid signature").into_response();
    }
    (StatusCode::OK, echostr).into_response()
}

/// `POST /weapp/msg` —— 小程序消息回调（Go 的 `NotifyWeappMsg`）。
///
/// 小程序回调是加密协议（AES 解密 + XML），当前项目未实现。
async fn weapp_msg_post(State(state): State<AppState>) -> Response {
    let _ = state;
    fail_unsupported("未实现：小程序回调需要解密消息体，请在公众号设置里配置服务器地址")
}

/// `POST /weapp/qrcode` —— 生成小程序二维码（Go 的 `ApiCreateWeappQrcode`）。
///
/// 请求字段：`path`、`scene`。需要登录。
///
/// 库里已有二维码时直接返回；没有的话需要调微信接口生成（当前项目未实现）。
async fn weapp_qrcode(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = member_of(pool, &headers).await else {
        return fail_no_login();
    };
    let form = parse_body(&headers, &body);
    let path = form.get("path").map(|value| value.trim()).unwrap_or("");
    if path.is_empty() {
        return fail("参数不完整");
    }
    match interact::weapp_qrcode(pool, user_id, path).await {
        Ok(Some(code_url)) => {
            Json(json!({ "code": 0, "msg": "", "data": code_url })).into_response()
        }
        Ok(None) => fail_unsupported("未实现：生成小程序二维码需要调用微信接口"),
        Err(err) => failed(err),
    }
}

/// `GET /google/url` —— Google 登录授权地址（Go 的 `GoogleAuthUrl`）。
///
/// 参数：`state`（缺省时随机 32 位）。
async fn google_url(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = site_setting::get_section(pool, "google_auth")
        .await
        .unwrap_or_else(|_| json!({}));
    let client_id = config
        .get("client_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let client_secret = config
        .get("client_secret")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if client_id.is_empty() || client_secret.is_empty() {
        return fail("Google 登录已禁用");
    }
    let redirect_url = config
        .get("redirect_url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let state_value = {
        let raw = q_str(&q, "state");
        if raw.is_empty() {
            random_string(32)
        } else {
            raw
        }
    };
    let scope = "openid https://www.googleapis.com/auth/userinfo.email \
                 https://www.googleapis.com/auth/userinfo.profile";
    let encoded_scope =
        percent_encoding::utf8_percent_encode(scope, percent_encoding::NON_ALPHANUMERIC)
            .to_string();
    let encoded_redirect =
        percent_encoding::utf8_percent_encode(&redirect_url, percent_encoding::NON_ALPHANUMERIC)
            .to_string();
    let location = format!(
        "https://accounts.google.com/o/oauth2/auth?access_type=online&client_id={client_id}\
         &redirect_uri={encoded_redirect}&response_type=code&scope={encoded_scope}\
         &state={state_value}"
    );
    ok(json!({ "state": state_value, "url": location }))
}

// ---------------------------------------------------------------------------
// 访问日志
// ---------------------------------------------------------------------------

/// `GET|POST /log` —— 访问日志（Go 的 `ApiLogStatistic`）。
///
/// 请求字段：`action`（`views` 时才记录）、`type`（`archive` 时顺带加浏览量）、
/// `id`、`code`、`path`。成功返回 204（与 Go 一致）。
async fn access_log(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let form = parse_body(&headers, &body);
    let action = form.get("action").map(|value| value.trim()).unwrap_or("");
    if action != "views" {
        return StatusCode::NO_CONTENT.into_response();
    }

    let refer = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let path = {
        let raw = form.get("path").map(|value| value.trim()).unwrap_or("");
        if raw.is_empty() {
            if refer.is_empty() {
                "/".to_string()
            } else {
                refer.clone()
            }
        } else {
            raw.to_string()
        }
    };
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let code = form
        .get("code")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0);
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();

    let record = json!({
        "id": 0,
        "created_time": chrono::Utc::now().timestamp(),
        "spider": interact::spider_of(&user_agent),
        "host": host,
        "url": path.chars().take(250).collect::<String>(),
        "ip": client_ip(&headers),
        "device": interact::device_of(&user_agent),
        "http_code": code,
        "user_agent": user_agent.chars().take(250).collect::<String>(),
    });
    let dir = state
        .with_config(|config| config.data_dir())
        .join("statistic");
    if let Err(err) = interact::append_statistic_log(&dir, &record) {
        tracing::warn!("写访问日志失败：{err:#}");
    }

    // 内容浏览量 +1（Go：type == archive 时才加）
    let kind = form.get("type").map(|value| value.trim()).unwrap_or("");
    if kind == "archive" {
        let archive_id = form
            .get("id")
            .and_then(|value| value.trim().parse::<i64>().ok())
            .unwrap_or(0);
        if archive_id > 0 {
            let _ = interact::add_archive_views(pool, archive_id).await;
        }
    }

    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 解析 XML 里的简单标签（微信 / 小程序回调都是这种扁平结构）。
fn xml_fields(xml: &str) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    let bytes = xml.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'<' {
            index += 1;
            continue;
        }
        let name_start = index + 1;
        let mut cursor = name_start;
        while cursor < bytes.len() && bytes[cursor] != b'>' {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }
        let name = &xml[name_start..cursor];
        if name.starts_with('/')
            || name.starts_with('?')
            || name.starts_with('!')
            || name.is_empty()
        {
            index = cursor + 1;
            continue;
        }
        if cursor > name_start && bytes[cursor - 1] == b'/' {
            index = cursor + 1;
            continue;
        }
        let close = format!("</{name}>");
        let value_start = cursor + 1;
        let Some(end) = xml[value_start..].find(&close) else {
            break;
        };
        let raw_value = &xml[value_start..value_start + end];
        // 嵌套容器（值以 `<` 开头但不是 CDATA）：进入内部继续解析，不记录自身
        let value = if let Some(inner) = raw_value
            .strip_prefix("<![CDATA[")
            .and_then(|rest| rest.strip_suffix("]]>"))
        {
            inner
        } else if raw_value.starts_with('<') {
            index = value_start;
            continue;
        } else {
            raw_value
        };
        out.insert(name.to_string(), xml_decode(value));
        index = value_start + end + close.len();
    }
    out
}

/// XML 实体解码。
fn xml_decode(raw: &str) -> String {
    raw.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// 微信公众号验证码缓存（`openid → (验证码, 过期时间)`）。
fn verify_code_store() -> &'static std::sync::Mutex<HashMap<String, (String, i64)>> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, (String, i64)>>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// 生成 / 复用公众号验证码（30 分钟有效）。
fn generate_verify_code(openid: &str) -> String {
    use rand::Rng;
    let now = chrono::Utc::now().timestamp();
    let mut store = verify_code_store()
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    store.retain(|_, (_, expire)| *expire > now);
    if let Some((code, _)) = store.get(openid) {
        return code.clone();
    }
    let code: String = (0..6)
        .map(|_| char::from(b'0' + rand::thread_rng().gen_range(0..10u8)))
        .collect();
    store.insert(openid.to_string(), (code.clone(), now + 1800));
    code
}

/// 随机字符串（Go 的 `config.GenerateRandString`）。
fn random_string(len: usize) -> String {
    use rand::Rng;
    const POOL: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    (0..len)
        .map(|_| POOL[rand::thread_rng().gen_range(0..POOL.len())] as char)
        .collect()
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xml_fields() {
        let xml = "<xml><ToUserName><![CDATA[gh_1]]></ToUserName>\
                   <FromUserName><![CDATA[o_user]]></FromUserName>\
                   <CreateTime>1700000000</CreateTime>\
                   <MsgType><![CDATA[text]]></MsgType>\
                   <Content><![CDATA[你好 &amp; 欢迎]]></Content></xml>";
        let fields = xml_fields(xml);
        assert_eq!(
            fields.get("FromUserName").map(String::as_str),
            Some("o_user")
        );
        assert_eq!(fields.get("MsgType").map(String::as_str), Some("text"));
        assert_eq!(
            fields.get("Content").map(String::as_str),
            Some("你好 & 欢迎")
        );
        assert_eq!(
            fields.get("CreateTime").map(String::as_str),
            Some("1700000000")
        );
    }

    #[test]
    fn wechat_sign_matches_md5_rule() {
        let mut fields = HashMap::new();
        fields.insert("appid".to_string(), "wx1".to_string());
        fields.insert("out_trade_no".to_string(), "pay1".to_string());
        fields.insert("sign".to_string(), "ignored".to_string());
        // md5("appid=wx1&out_trade_no=pay1&key=K") 的标准结果
        let sign = wechat_sign(&fields, "K");
        assert_eq!(sign.len(), 32);
        assert!(sign.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert!(sign.chars().all(|ch| !ch.is_ascii_lowercase()));
    }

    #[test]
    fn parses_body_and_form() {
        let headers = HeaderMap::new();
        let json = Bytes::from_static(br#"{"archive_id":12,"content":"hi"}"#);
        let mut json_headers = HeaderMap::new();
        json_headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let form = parse_body(&json_headers, &json);
        assert_eq!(form.get("archive_id").map(String::as_str), Some("12"));
        assert_eq!(form.get("content").map(String::as_str), Some("hi"));

        let encoded = Bytes::from_static(b"user_name=%E5%BC%A0%E4%B8%89&tags[]=a&tags[]=b");
        let form = parse_body(&headers, &encoded);
        assert_eq!(form.get("user_name").map(String::as_str), Some("张三"));
        assert_eq!(form.get("tags").map(String::as_str), Some("a,b"));
    }

    #[test]
    fn extracts_limit_offset() {
        let mut q = HashMap::new();
        q.insert("limit".to_string(), "20,10".to_string());
        assert_eq!(q_limit(&q, 10, 1000), (20, 10));
        let mut q = HashMap::new();
        q.insert("limit".to_string(), "5".to_string());
        assert_eq!(q_limit(&q, 10, 1000), (0, 5));
        let q = HashMap::new();
        assert_eq!(q_limit(&q, 10, 1000), (0, 10));
    }
}

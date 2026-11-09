//! 互动与第三方 API（批次 D）。
//!
//! 覆盖评论 / 留言、收藏、微信与小程序通知、Google 登录、
//! 访问日志统计这一组接口的既有契约。
//!
//! 路径按主线要求**不带 `/api` 前缀**（如 `/comment/list`），
//! 同时注册 `/api/...` 别名，保证既有调用地址同样可用；
//! 微信 / 小程序通知另外保留 `/notify/...` 地址。
//!
//! 响应信封：成功 `{"code":0,"msg":"","data":...}`，
//! 列表接口额外带顶层 `total`，失败 `{"code":1,"msg":"..."}`；
//! 需要登录时用 `config.StatusNoLogin`（1001）；
//! 微信 / 小程序回调这类「不是 JSON 契约」的地址在参数不完整时也用
//! `{"code":-1,"msg":"..."}`（与 `src/routes.rs` 里的 `fail_ai` 同款）。

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Map, Value};

use crate::auth;
use crate::models::wechat::{self, crypto, wxa};
use crate::models::{comment, front_content, interact, site_setting, trade, user};
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

    // 既有实现把微信 / 小程序通知挂在 `/notify` 分组下，这里保留同样的地址
    router = router
        .route("/notify/wechat/pay", post(wechat_pay_notify))
        .route("/notify/weapp/msg", get(weapp_msg_get))
        .route("/notify/weapp/msg", post(weapp_msg_post));

    router
}

// ---------------------------------------------------------------------------
// 响应工具
// ---------------------------------------------------------------------------

/// 列表响应：顶层额外带 `total`（按既有列表接口）。
fn ok_total(total: i64, data: Value) -> Response {
    Json(json!({ "code": 0, "msg": "", "total": total, "data": data })).into_response()
}

/// 需要登录（既有的 `config.StatusNoLogin` = 1001）。
fn fail_no_login() -> Response {
    Json(json!({ "code": 1001, "msg": "请登录" })).into_response()
}

/// 参数不完整等前置错误（`code:-1`，与 `src/routes.rs` 里的 `fail_ai` 同款）。
///
/// 微信 / 小程序回调的调用方不是前端页面，保持这个信封比 `code:1` 更能区分
/// 「请求本身不合法」与「业务失败」。
fn fail_unsupported(msg: impl Into<String>) -> Response {
    Json(json!({ "code": -1, "msg": msg.into() })).into_response()
}

// ---------------------------------------------------------------------------
// 参数解析
// ---------------------------------------------------------------------------

/// 查询串取值（按 `URLParam*` 的语义：取不到就当零值，不报错）。
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

/// `limit` 参数：支持 `offset,limit` 与 `limit` 两种写法（按既有约定）。
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
/// 值统一成字符串（数组用逗号连接，按既有 checkbox 处理）。
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

/// JSON 值转字符串（数组用逗号连接，按既有 checkbox 处理）。
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
/// 令牌来源与既有的 `ParseUserToken` 一致：`token` 头 / `Authorization: Bearer` / `token` Cookie。
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

/// `GET /comment/list` —— 评论列表（按 `ApiCommentList` 的语义）。
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

    // 文档标题 + 父评论（既有的 `item_title` / `parent`）
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

/// `POST /comment/publish` —— 发布评论（既有的 `CommentPublish` / `ApiCommentPublish`）。
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

    // 登录会员：用户名取会员资料（按既有约定）
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

    // 审核：用户组开启「免审核」时直接正常（既有的 ContentNoVerify）
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

/// `POST /comment/praise` —— 评论点赞（既有的 `CommentPraise` / `ApiCommentPraise`）。
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
    // 既有的返回体：点赞数 +1 并标记 active
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

/// `GET /favorite/list` —— 我的收藏（按 `ApiGetFavorites` 的语义）。
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

/// `POST /favorite/check` —— 批量查收藏状态（按 `ApiCheckFavorites` 的语义）。
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

/// `POST /favorite/add` —— 收藏 / 取消收藏（既有的 `ApiAddFavorite`，开关语义）。
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

/// `POST /favorite/delete` —— 取消收藏（按 `ApiDeleteFavorite` 的语义）。
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

/// `GET /guestbook.html` —— 留言页（按 `GuestbookPage` 的语义）。
///
/// 当前项目还没有前台模板，这里直接返回留言字段（等价 `/guestbook/fields`）。
async fn guestbook_page(State(state): State<AppState>) -> Response {
    guestbook_fields(State(state)).await
}

/// `GET /guestbook/fields` —— 留言字段（按 `ApiGuestbook` 的语义）。
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

/// `POST /guestbook.html` —— 提交留言（既有的 `GuestbookForm` / `ApiGuestbookForm`）。
///
/// 请求体按 `settings.guestbook.fields` 里的字段名提交；
/// 必填项为空时返回「xx必填」（按 `%sIsRequired` 的语义）。
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

/// `GET /wechat` —— 微信服务器校验（既有的 `WechatApi` 的 GET 分支）。
///
/// 校验方式与微信一致：`sha1(sort(token, timestamp, nonce)) == signature`，
/// 通过后必须**原样回显 `echostr`**，否则公众号后台的「服务器配置」保存不了
/// （既有的 `wechatServer.VerifyURL` 就是这么做的）。
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
    let echostr = q_str(&q, "echostr");
    if signature.is_empty() || timestamp.is_empty() || nonce.is_empty() {
        return fail_unsupported("微信服务器校验失败：缺少 signature / timestamp / nonce");
    }
    if interact::wechat_signature(&token, &timestamp, &nonce) != signature {
        return (StatusCode::FORBIDDEN, "invalid signature").into_response();
    }
    if echostr.is_empty() {
        // 没有 echostr（例如浏览器直接访问）时给一句可读结果
        return (StatusCode::OK, "success").into_response();
    }
    (StatusCode::OK, echostr).into_response()
}

/// `POST /wechat` —— 公众号消息回调（既有的 `WechatApi` 的 POST 分支）。
///
/// 公众号开启「安全模式」（配置了 EncodingAESKey）时 body 是加密信封，
/// 先按微信文档验签 + AES-128-CBC 解密，再走同一套分发。
/// 回复用**被动回复**：微信要求 5 秒内回包，
/// 客服消息接口（[`wechat::push_text`]）留给后台手动回复与验证码下发。
async fn wechat_message(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = interact::wechat_config(pool)
        .await
        .unwrap_or_else(|_| json!({}));
    let text_of = |key: &str| {
        config
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let token = text_of("token");
    let aes_key = text_of("encoding_aes_key");
    let app_id = text_of("app_id");

    let raw = String::from_utf8_lossy(&body).to_string();
    // 明文模式：普通 signature 也要校验（安全模式走下面的 msg_signature）
    if wxa::envelope_encrypt(&raw).is_none() && !token.is_empty() {
        let signature = q_str(&q, "signature");
        if !signature.is_empty() {
            let timestamp = q_str(&q, "timestamp");
            let nonce = q_str(&q, "nonce");
            if interact::wechat_signature(&token, &timestamp, &nonce) != signature {
                return (StatusCode::FORBIDDEN, "invalid signature").into_response();
            }
        }
    }

    let mut fields = xml_fields(&raw);
    // 安全模式：信封里带 <Encrypt>，先验签解密再当普通报文处理
    let mut encrypted = false;
    if let Some(envelope) = wxa::envelope_encrypt(&raw) {
        if aes_key.is_empty() {
            return (
                StatusCode::BAD_REQUEST,
                "公众号回调是加密报文，但未配置 EncodingAESKey",
            )
                .into_response();
        }
        let signature = q_str(&q, "msg_signature");
        let timestamp = q_str(&q, "timestamp");
        let nonce = q_str(&q, "nonce");
        match crypto::decrypt_message(
            &token,
            &aes_key,
            &app_id,
            &signature,
            &timestamp,
            &nonce,
            &envelope,
        ) {
            Ok(xml) => {
                fields = xml_fields(&xml);
                encrypted = true;
            }
            Err(err) => {
                tracing::warn!("公众号消息解密失败：{err}");
                return (StatusCode::FORBIDDEN, format!("消息解密失败：{err}")).into_response();
            }
        }
    }

    let msg_type = fields.get("MsgType").cloned().unwrap_or_default();
    let from = fields.get("FromUserName").cloned().unwrap_or_default();
    let to_user = fields.get("ToUserName").cloned().unwrap_or_default();
    let mut content = fields.get("Content").cloned().unwrap_or_default();

    if msg_type == "event" {
        let event = fields.get("Event").cloned().unwrap_or_default();
        if event == "CLICK" {
            content = fields.get("EventKey").cloned().unwrap_or_default();
        } else {
            return (StatusCode::OK, "success").into_response();
        }
    }

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

    if reply.trim().is_empty() {
        // 没有可回复的内容：回 success 表示已收到，微信不会重试
        return (StatusCode::OK, "success").into_response();
    }

    let xml = wechat::text_reply_xml(
        &from,
        &to_user,
        &reply,
        chrono::Utc::now().timestamp(),
    );
    if encrypted {
        return match crypto::encrypt_message(&token, &aes_key, &app_id, &xml) {
            Ok(envelope) => xml_response(envelope),
            Err(err) => {
                // 加密失败时退化成 success（至少不让微信重推）
                tracing::warn!("公众号被动回复加密失败：{err}");
                (StatusCode::OK, "success").into_response()
            }
        };
    }
    xml_response(xml)
}

/// `GET /wechat/auth` —— 公众号网页授权（按 `WechatAuthApi` 的语义）。
///
/// 没有 `code` 时 302 到微信授权页；带 `code` 时换 `sns/oauth2/access_token`
/// → 拉 `sns/userinfo` → upsert `user_wechats` 并绑定会员（`union_id` 相同的视为同一人）。
/// 这是浏览器里打开的页面，所以结果用提示页返回（按 `ShowMessage` 的语义），不是 JSON。
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
        return wechat_page("请在微信内打开");
    }
    let config = interact::wechat_config(pool)
        .await
        .unwrap_or_else(|_| json!({}));
    let text_of = |key: &str| {
        config
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let app_id = text_of("app_id");
    let app_secret = text_of("app_secret");
    let verify_msg = text_of("verify_msg");
    let state_value = q_str(&q, "state");
    let code = q_str(&q, "code");

    if code.is_empty() {
        if app_id.is_empty() {
            return wechat_page("未配置微信公众号 AppID");
        }
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
        return response;
    }

    if app_id.is_empty() || app_secret.is_empty() {
        return wechat_page("请先在公众号配置中填写 AppID 和 AppSecret");
    }
    let (access_token, openid) =
        match wechat::oauth_access_token(&app_id, &app_secret, &code).await {
            Ok(pair) => pair,
            Err(err) => return wechat_page(&err.to_string()),
        };
    let info = match wechat::oauth_user_info(&access_token, &openid).await {
        Ok(value) => value,
        Err(err) => return wechat_page(&err.to_string()),
    };
    let nickname = info.get("nickname").and_then(Value::as_str).unwrap_or("");
    let avatar = info.get("headimgurl").and_then(Value::as_str).unwrap_or("");
    let union_id = info.get("unionid").and_then(Value::as_str).unwrap_or("");
    let gender = info.get("sex").and_then(Value::as_i64).unwrap_or(0) as i32;
    let user_id =
        match interact::save_wechat_user(pool, &openid, union_id, nickname, avatar, gender).await {
            Ok(id) => id,
            Err(err) => {
                tracing::warn!("公众号授权绑定会员失败：{err:#}");
                return wechat_page(&format!("微信授权失败：{err}"));
            }
        };

    if state_value == "code" {
        // `WechatAuthApi`：state=code 时下发验证码并推送到微信
        let verify_code = generate_verify_code(&openid);
        let message = if verify_msg.contains("{code}") {
            verify_msg.replacen("{code}", &verify_code, 1)
        } else {
            format!("验证码：{verify_code}，30分钟内有效{verify_msg}")
        };
        if let Err(err) = wechat::push_text(pool, &openid, &message).await {
            tracing::warn!("推送微信验证码失败：{err}");
        }
        return wechat_page(&message);
    }

    // 既有实现在这个分支不输出内容（浏览器里是白屏），这里给一句提示，便于用户知道能返回了
    wechat_page(&format!(
        "微信授权成功（会员 ID：{user_id}），可以返回继续操作。"
    ))
}

/// 微信支付回调（按 `NotifyWechatPay` 的语义）。
///
/// 真正的实现只有一份：`crate::api_trade::wechat_notify`（V2 验签、金额核对、
/// `settle_gateway_payment` 落库：payments / orders / finances / order_refunds）。
/// 这里只做**薄转发**，让 `/wechat/pay`、`/notify/wechat/pay` 与
/// `/api_trade/wechat/notify` 三条入口的落库口径完全一致。
async fn wechat_pay_notify(State(state): State<AppState>, body: Bytes) -> Response {
    // 统一到 api_trade 的真实实现（V2 验签、金额核对、settle_gateway_payment 落库）
    let body = String::from_utf8_lossy(&body).to_string();
    crate::api_trade::wechat_notify(State(state), body).await
}

// ---------------------------------------------------------------------------
// 小程序 / Google
// ---------------------------------------------------------------------------

/// `GET /weapp/msg` —— 小程序服务器校验（既有的 `NotifyWeappMsg` 的 GET 分支）。
///
/// 校验方式与公众号一致：`sha1(sort(token, timestamp, nonce)) == signature`，
/// 通过后原样回显 `echostr`（小程序后台「服务器配置」点保存时校的就是这一步）。
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

/// `POST /weapp/msg` —— 小程序消息回调（按 `NotifyWeappMsg` 的语义）。
///
/// 小程序后台默认开启**安全模式**：POST body 是 `<Encrypt>` 信封，
/// `msg_signature = sha1(sort(token, timestamp, nonce, 密文))`，先用
/// [`crypto::decrypt_message`] 验签解密，再按事件分发（与公众号同一套协议）。
///
/// 目前处理「订阅消息」两类事件（既有的 `OnSubscribeMsgPopup` / `OnSubscribeMsgChange`）：
/// 同意订阅写 `subscribed_users`，拒绝则删掉，其余事件只回 `success`。
async fn weapp_msg_post(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = interact::weapp_config(pool)
        .await
        .unwrap_or_else(|_| json!({}));
    let text_of = |key: &str| {
        config
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let token = text_of("token");
    let aes_key = text_of("encoding_aes_key");
    let app_id = text_of("app_id");
    if token.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "未配置小程序 Token，无法校验回调合法性",
        )
            .into_response();
    }

    let timestamp = q_str(&q, "timestamp");
    let nonce = q_str(&q, "nonce");
    let raw = String::from_utf8_lossy(&body).to_string();

    let xml = match wxa::envelope_encrypt(&raw) {
        Some(envelope) => {
            if aes_key.is_empty() {
                return (
                    StatusCode::BAD_REQUEST,
                    "小程序回调是加密报文，但未配置 EncodingAESKey",
                )
                    .into_response();
            }
            let signature = q_str(&q, "msg_signature");
            if signature.is_empty() || timestamp.is_empty() || nonce.is_empty() {
                return (
                    StatusCode::BAD_REQUEST,
                    "小程序加密回调缺少 msg_signature / timestamp / nonce",
                )
                    .into_response();
            }
            match crypto::decrypt_message(
                &token,
                &aes_key,
                &app_id,
                &signature,
                &timestamp,
                &nonce,
                &envelope,
            ) {
                Ok(xml) => xml,
                Err(err) => {
                    tracing::warn!("小程序回调解密失败：{err}");
                    return (
                        StatusCode::FORBIDDEN,
                        format!("小程序回调解密失败：{err}"),
                    )
                        .into_response();
                }
            }
        }
        None => {
            // 明文模式：仍然校验普通 signature（微信的「兼容模式/明文模式」）
            let signature = q_str(&q, "signature");
            if !signature.is_empty()
                && interact::wechat_signature(&token, &timestamp, &nonce) != signature
            {
                return (StatusCode::FORBIDDEN, "invalid signature").into_response();
            }
            raw
        }
    };

    let fields = wxa::xml_fields(&xml);
    let openid = fields.get("FromUserName").cloned().unwrap_or_default();
    let event = fields.get("Event").cloned().unwrap_or_default();
    match event.as_str() {
        "subscribe_msg_popup_event" | "subscribe_msg_change_event" => {
            let events = wxa::subscribe_events(&xml);
            if events.is_empty() {
                tracing::warn!("小程序订阅消息事件里没有可识别的模板条目：{xml}");
            }
            for item in events {
                if let Err(err) =
                    interact::save_subscribed(pool, &openid, &item.template_id, item.accepted())
                        .await
                {
                    tracing::warn!(
                        "保存小程序订阅消息状态失败（template={}）：{err:#}",
                        item.template_id
                    );
                }
            }
        }
        _ => {
            tracing::debug!("小程序回调事件 {event}（{openid}）暂不处理");
        }
    }

    (StatusCode::OK, "success").into_response()
}

/// `POST /weapp/qrcode` —— 生成小程序二维码（按 `ApiCreateWeappQrcode` 的语义）。
///
/// 请求字段：`path`、`scene`。需要登录。
///
/// 与既有接口契约一致：同一会员 + path 只生成一次，之后直接回库里的地址；
/// 新生成的码落在 `uploads/qrcode/<md5 前3>/<3..6>/<6..>.png`，同时写 `attachments`
/// 与 `weapp_qrcodes`（库里存相对路径，返回给前端的是可访问的完整地址）。
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
    // 库里的 code_url 是相对路径，对外要拼上存储地址（既有的 JoinStorageUrl）
    let storage = trade::storage_url(pool).await;

    match interact::weapp_qrcode(pool, user_id, path).await {
        Ok(Some(code_url)) => Json(json!({
            "code": 0,
            "msg": "",
            "data": front_content::join_storage_url(&storage, &code_url),
        }))
        .into_response(),
        Ok(None) => {
            let bytes = match wechat::weapp_qrcode_image(pool, path).await {
                Ok(bytes) => bytes,
                Err(err) => return fail(err.to_string()),
            };

            // 与 `CreateWeappQrcode` 一致：uploads/qrcode/<md5 前3>/<3..6>/<6..>.png
            let md5 = crate::models::attachment::md5_hex(&bytes);
            let file_name = format!("{md5}.png");
            let relative = format!(
                "uploads/qrcode/{}/{}/{}",
                &md5[0..3],
                &md5[3..6],
                &md5[6..]
            );
            let public_dir = state.with_config(|config| config.public_dir());
            let file = public_dir.join(&relative);
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(err) = std::fs::write(&file, &bytes) {
                return fail(format!("保存小程序二维码失败：{err}"));
            }
            if let Err(err) = crate::models::attachment::insert(
                pool,
                &file_name,
                &relative,
                bytes.len() as u64,
                &md5,
                0,
                0,
                0,
                0,
                "",
            )
            .await
            {
                // 附件表写失败不影响出码，只记日志
                tracing::warn!("小程序码写 attachments 失败：{err}");
            }
            if let Err(err) = interact::save_weapp_qrcode(pool, user_id, path, &relative).await {
                return failed(err);
            }
            Json(json!({
                "code": 0,
                "msg": "",
                "data": front_content::join_storage_url(&storage, &relative),
            }))
            .into_response()
        }
        Err(err) => failed(err),
    }
}

/// `GET /google/url` —— Google 登录授权地址（按 `GoogleAuthUrl` 的语义）。
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

/// `GET|POST /log` —— 访问日志（按 `ApiLogStatistic` 的语义）。
///
/// 请求字段：`action`（`views` 时才记录）、`type`（`archive` 时顺带加浏览量）、
/// `id`、`code`、`path`。成功返回 204（与既有接口契约一致）。
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

    // 内容浏览量 +1（既有实现：type == archive 时才加）
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

/// XML 响应（微信被动回复）。
fn xml_response(xml: String) -> Response {
    (
        [(header::CONTENT_TYPE, "text/xml; charset=utf-8")],
        xml,
    )
        .into_response()
}

/// 微信内打开的提示页（按 `controller.ShowMessage` 的语义）。
///
/// `GET /wechat/auth` 是浏览器请求，出错时给一个可读页面，比回 JSON 有用；
/// 标记与既有实现保持一致（浅灰背景 + 白卡片 + 返回按钮）。
fn wechat_page(message: &str) -> Response {
    let html = format!(
        "<!DOCTYPE html><html><head><meta charset=utf-8>\
<meta name=\"viewport\" content=\"width=device-width,height=device-height,initial-scale=1.0, \
minimum-scale=1.0, maximum-scale=1.0, user-scalable=no,viewport-fit=cover\">\
<meta http-equiv=X-UA-Compatible content=\"IE=edge,chrome=1\"><title>Info</title>\
<style>a{{text-decoration: none;color: #777;}}</style></head>\
<body style=\"background: #f4f5f7;margin: 0;padding: 20px;\">\
<div style=\"margin-left: auto;margin-right: auto;margin-top: 50px;padding: 20px;\
border: 1px solid #eee;background:#fff;max-width: 640px;\"><div>{message}</div>\
<div style=\"margin-top: 30px;text-align: right;\"><a style=\"display: inline-block;\
border:1px solid #777;padding: 8px 16px;\" href=\"javascript:history.back();\">Back</a>\
</div></div></body></html>"
    );
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

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

/// 随机字符串（按 `config.GenerateRandString` 的语义）。
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

    /// 微信 V2 签名：参数（去掉 `sign` 与空值）按键名排序 → `k=v&k=v` → 追加 `&key=密钥` → MD5 大写。
    ///
    /// 线上的验签已经统一到 `api_trade`（支持 MD5 与 HMAC-SHA256），这里保留一份
    /// 最简实现只为把「字典序拼接 + 大写 MD5」这条规则钉在测试里，因此放在
    /// `#[cfg(test)]` 内，bin 构建不留死代码。
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

    /// MD5 大写十六进制。
    fn md5_hex_upper(data: &[u8]) -> String {
        use md5::{Digest, Md5};
        Md5::digest(data)
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect()
    }

    #[test]
    fn wechat_sign_matches_md5_rule() {
        let mut fields = HashMap::new();
        fields.insert("appid".to_string(), "wx1".to_string());
        fields.insert("out_trade_no".to_string(), "pay1".to_string());
        fields.insert("sign".to_string(), "ignored".to_string());
        fields.insert("attach".to_string(), String::new());
        // md5("appid=wx1&out_trade_no=pay1&key=K")：32 位大写十六进制
        let sign = wechat_sign(&fields, "K");
        assert_eq!(sign.len(), 32);
        assert!(sign
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_lowercase()));

        // 与插入顺序无关；`sign` 自身与空值不参与拼接
        let mut reordered = HashMap::new();
        reordered.insert("out_trade_no".to_string(), "pay1".to_string());
        reordered.insert("sign".to_string(), "another".to_string());
        reordered.insert("appid".to_string(), "wx1".to_string());
        assert_eq!(wechat_sign(&reordered, "K"), sign);

        // 换密钥结果必变
        assert_ne!(wechat_sign(&fields, "K2"), sign);
    }

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

    #[test]
    fn sets_wechat_response_headers() {
        // 被动回复必须是 XML，验证码/错误提示页必须是 HTML
        let response = xml_response("<xml></xml>".to_string());
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/xml; charset=utf-8")
        );
        let response = wechat_page("请在微信内打开");
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/html; charset=utf-8")
        );
    }
}

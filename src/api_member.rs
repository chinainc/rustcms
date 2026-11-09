//! 会员与账户 API（批次 B）。
//!
//! 覆盖账户、会员中心、会员认证、开放接口与订阅这一组接口的既有契约。
//!
//! ## 路由约定
//!
//! 路由使用**绝对路径**（不带 `/api` 前缀），由主线在 `routes.rs` 里
//! `merge` 进来；本文件不改动 `routes.rs`。
//!
//! 既有接口把这些端点挂在 `/api` 分组下（`/api/login`、`/api/user/...`），
//! 本批按主线要求去掉 `/api` 前缀，取的是**分组内的原始路径**。
//!
//! ## 鉴权口径
//!
//! - 令牌来源优先级（见 `ParseUserToken` 的语义）：
//! `token` 头 → `Authorization: Bearer` → `token` cookie；
//! - 会员 JWT 用 `TokenSecret + "-user-token"` 签名，claims 是
//! `{"userId":"<id>","t":"<过期时间戳>"}`（对齐 `src/auth.rs`）；
//! - JWT 不可用时回退到会员开放接口 Key（`users.api_token`）；
//! - 未登录一律回 `code: 1` +「请先登录」（既有契约用 `code: 1001` +
//! 「该操作需要登录，请登录后重试」，本项目统一用 `code: 1`）。

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Multipart, Query, Request, State};
use axum::http::{header, HeaderMap};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Map, Value};

use crate::auth::{self, TokenKind};
use crate::models::attachment;
use crate::models::member::{self, MemberGroup};
use crate::routes::{bearer_token, fail, failed, ok, ok_msg, NO_DB};
use crate::state::AppState;

/// 本批路由表。
pub fn router() -> Router<AppState> {
    Router::new()
        // ------------------------------------------------------------------
        // 账户：登录 / 注册 / 找回密码 / 邮箱验证 / 验证码
        // ------------------------------------------------------------------
        .route("/login", post(login).get(front_page))
        .route("/logout", post(logout).get(logout))
        .route("/register", post(register).get(front_page))
        .route("/forget", post(forget).get(front_page))
        .route("/password/reset", post(password_reset))
        .route("/verify/email", get(verify_email).post(send_verify_email))
        .route("/captcha", get(captcha))
        // ------------------------------------------------------------------
        // 账户页数据（既有接口注册的是页面，这里给 API 客户端用）
        // ------------------------------------------------------------------
        .route("/account/info", get(account_info))
        .route("/account/password/reset", get(account_password_reset))
        // ------------------------------------------------------------------
        // 会员中心
        // ------------------------------------------------------------------
        .route("/member/info", get(member_info))
        .route("/member/nav", get(member_nav))
        .route("/member/archives", get(user_archives))
        .route("/member/finances", get(user_finances))
        .route("/member/points", get(member_points))
        .route("/member/comments", get(user_comments))
        .route("/member/guestbooks", get(user_guestbooks))
        .route("/member/groups", get(user_groups))
        .route("/member/auth", get(user_auth_info).post(user_auth_apply))
        .route("/member/openapi", get(user_openapi_info).post(user_openapi_token))
        .route("/member/sign", get(sign_info).post(sign_do))
        .route("/member/profile", post(user_detail_save))
        .route("/member/password", post(user_password))
        .route("/member/avatar", post(user_avatar))
        .route("/member/upload", post(user_upload))
        // ------------------------------------------------------------------
        // 会员接口（按 `/api/user/*` 的语义）
        // ------------------------------------------------------------------
        .route("/user/detail", get(user_detail).post(user_detail_save))
        .route("/user/archives", get(user_archives))
        .route("/user/archive", get(user_archive))
        .route("/user/comments", get(user_comments))
        .route("/user/comment/delete", post(user_comment_delete))
        .route("/user/comment/reply", post(user_comment_reply))
        .route("/user/comment/status", post(user_comment_status))
        .route("/user/guestbooks", get(user_guestbooks))
        .route("/user/guestbook/delete", post(user_guestbook_delete))
        .route("/user/finances", get(user_finances))
        .route("/user/groups", get(user_groups))
        .route("/user/group/detail", get(user_group_detail))
        .route("/user/avatar", post(user_avatar))
        .route("/user/password", post(user_password))
        .route("/user/upload", post(user_upload))
        .route("/user/auth/apply", post(user_auth_apply))
        .route("/user/auth/info", get(user_auth_info))
        .route("/user/openapi/info", get(user_openapi_info))
        .route("/user/openapi/token", post(user_openapi_token))
        // ------------------------------------------------------------------
        // 签到 / 订阅 / 附件上传
        // ------------------------------------------------------------------
        .route("/sign/info", get(sign_info))
        .route("/sign/do", post(sign_do))
        .route("/subscription", post(subscription))
        .route("/attachment/upload", post(attachment_upload))
}

// ============================================================================
// 通用工具
// ============================================================================

/// 请求体（同时兼容 JSON 与表单提交）。
///
/// 既有实现是「先 `ReadJSON`，失败再读表单」，
/// 这里用同样的顺序解析：JSON 解析失败就当表单处理。
struct Body(Value);

impl Body {
    /// 解析请求体。
    fn parse(bytes: &Bytes) -> Body {
        if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
            if value.is_object() {
                return Body(value);
            }
        }
        let text = String::from_utf8_lossy(bytes);
        let mut map = Map::new();
        for pair in text.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = match pair.split_once('=') {
                Some((key, value)) => (key, value),
                None => (pair, ""),
            };
            map.insert(form_decode(key), Value::String(form_decode(value)));
        }
        Body(Value::Object(map))
    }

    /// 取字符串字段。
    fn str(&self, key: &str) -> String {
        match self.0.get(key) {
            Some(Value::String(value)) => value.clone(),
            Some(Value::Number(value)) => value.to_string(),
            Some(Value::Bool(value)) => value.to_string(),
            _ => String::new(),
        }
    }

    /// 取整数字段（字符串数字也能解）。
    fn int(&self, key: &str) -> i64 {
        match self.0.get(key) {
            Some(Value::Number(value)) => value.as_i64().unwrap_or(0),
            Some(Value::String(value)) => value.trim().parse::<i64>().unwrap_or(0),
            Some(Value::Bool(value)) => i64::from(*value),
            _ => 0,
        }
    }

    /// 取布尔字段。
    fn bool(&self, key: &str) -> bool {
        match self.0.get(key) {
            Some(Value::Bool(value)) => *value,
            Some(Value::Number(value)) => value.as_i64().unwrap_or(0) != 0,
            Some(Value::String(value)) => matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "yes"
            ),
            _ => false,
        }
    }
}

/// 表单值解码（`%xx` 与 `+`）。
fn form_decode(raw: &str) -> String {
    let replaced = raw.replace('+', " ");
    percent_encoding::percent_decode_str(&replaced)
        .decode_utf8_lossy()
        .to_string()
}

/// 查询串参数（手工取值，避免空串导致反序列化失败）。
fn query_value(query: &HashMap<String, String>, key: &str) -> String {
    query.get(key).cloned().unwrap_or_default()
}

/// 查询串里的整数（空串 / 非法值都当 0）。
fn query_int(query: &HashMap<String, String>, key: &str) -> i64 {
    query_value(query, key).trim().parse::<i64>().unwrap_or(0)
}

/// 分页参数（按既有默认分页口径）。
struct Paging {
    page: i64,
    page_size: i64,
}

impl Paging {
    /// `page` 默认 1；`page_size` 认 `page_size` 与 `pageSize` 两种写法。
    fn new(query: &HashMap<String, String>, default_size: i64, max_size: i64) -> Paging {
        let page = query_int(query, "page").max(1);
        let raw = {
            let value = query_value(query, "page_size");
            if value.trim().is_empty() {
                query_value(query, "pageSize")
            } else {
                value
            }
        };
        let size = raw.trim().parse::<i64>().unwrap_or(default_size);
        let page_size = if size < 1 {
            default_size
        } else {
            size.min(max_size)
        };
        Paging { page, page_size }
    }
}

/// 从请求头 / Cookie 里取会员令牌。
fn read_token(headers: &HeaderMap) -> Option<String> {
    let from_header = headers
        .get("token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    if from_header.is_some() {
        return from_header;
    }
    if let Some(token) = bearer_token(headers) {
        return Some(token);
    }
    read_cookie(headers, "token")
}

/// 读 Cookie（只做最基本的分号切分）。
fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(name).and_then(|rest| rest.strip_prefix('=')) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// 取当前登录会员 ID（按 `ParseUserToken` 的语义）。
///
/// JWT 校验失败时回退到会员开放接口 Key（`users.api_token`）。
async fn current_user_id(pool: &sqlx::MySqlPool, headers: &HeaderMap) -> Option<u32> {
    let token = read_token(headers)?;
    if let Ok(secret) = auth::token_secret(pool).await {
        if let Ok(claims) = auth::verify(TokenKind::User, &secret, &token) {
            if let Some(id) = claims.subject(TokenKind::User) {
                return Some(id);
            }
        }
    }
    let id: Option<u32> =
        sqlx::query_scalar("SELECT id FROM users WHERE api_token = ? AND status = 1 LIMIT 1")
            .bind(&token)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    id
}

/// 未登录时统一的错误响应。
fn need_login() -> Response {
    fail("请先登录")
}

/// 令牌失效（能解析出令牌但校验不过）。
#[allow(dead_code)]
fn need_relogin() -> Response {
    fail("登录已失效，请重新登录")
}

/// 会员登录 Cookie（写 `token` 键，值为会员令牌）。
fn token_cookie(token: &str, remember: bool) -> String {
    let max_age = if remember {
        30 * 24 * 3600
    } else {
        24 * 3600
    };
    format!("token={token}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax")
}

/// 给响应挂上登录 Cookie。
fn with_cookie(mut res: Response, token: &str, remember: bool) -> Response {
    if let Ok(value) = header::HeaderValue::from_str(&token_cookie(token, remember)) {
        res.headers_mut().insert(header::SET_COOKIE, value);
    }
    res
}

/// 交给前台管线渲染（既有实现这些路径同时有 GET 页面）。
async fn front_page(State(state): State<AppState>, req: Request) -> Response {
    crate::front::reroute(State(state), req).await
}

/// 站点设置里的某个配置段。
async fn setting_section(pool: &sqlx::MySqlPool, key: &str) -> Value {
    crate::models::site_setting::get_section(pool, key)
        .await
        .unwrap_or_else(|_| json!({}))
}

/// 会员的优惠比例（按 `provider.GetUserDiscount` 的语义）。
///
/// 优先级：会员所在分组的 `setting.discount` > 上级分组的 `setting.share_discount`。
async fn user_discount(pool: &sqlx::MySqlPool, user_id: u32) -> i64 {
    if user_id == 0 {
        return 0;
    }
    let Ok(Some(row)) = member::get(pool, user_id).await else {
        return 0;
    };
    let mut discount = 0i64;
    if let Ok(Some(group)) = member::group(pool, row.group_id).await {
        discount = group.discount();
    }
    if row.parent_id > 0 {
        if let Ok(Some(parent)) = member::get(pool, row.parent_id as u32).await {
            if let Ok(Some(group)) = member::group(pool, parent.group_id).await {
                let share = group
                    .setting_value()
                    .get("share_discount")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                if share > discount {
                    discount = share;
                }
            }
        }
    }
    discount
}

/// 用户组 JSON（带上 `favorable_price`）。
fn group_json(group: &MemberGroup, discount: i64) -> Value {
    let favorable = if group.price > 0 && discount > 0 {
        group.price * discount / 100
    } else {
        0
    };
    group.to_json(favorable)
}

/// 会员所在分组（读不到就返回 `null`）。
async fn member_group_json(pool: &sqlx::MySqlPool, group_id: u32, discount: i64) -> Value {
    match member::group(pool, group_id).await {
        Ok(Some(group)) => group_json(&group, discount),
        _ => Value::Null,
    }
}

/// 站点主地址（取站点配置里的主域名）。
async fn site_base_url(pool: &sqlx::MySqlPool) -> String {
    let system = setting_section(pool, "system").await;
    system
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .trim_end_matches('/')
        .to_string()
}

/// 客户端 IP（只从代理头取）。
fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-real-ip")
        .or_else(|| headers.get("x-forwarded-for"))
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.split(',').next().unwrap_or(value).trim().to_string())
        .unwrap_or_default()
}

// ============================================================================
// 登录 / 注册 / 找回密码
// ============================================================================

/// `POST /login`（按 `ApiLogin` 的语义）。
///
/// 请求字段：`user_name` / `email` / `phone` / `password` / `remember` /
/// `captcha_id` / `captcha` / `platform`。
async fn login(State(state): State<AppState>, body: Bytes) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let body = Body::parse(&body);

    let platform = body.str("platform").trim().to_ascii_lowercase();
    if !platform.is_empty() && !platform.eq_ignore_ascii_case("web") {
        // 微信 / 公众号 / Google 登录需要外部授权，Rust 版尚未接入
        return fail("该登录方式尚未接入");
    }

    // 验证码（既有实现：safe.captcha == 1 时必校）
    let safe = setting_section(pool, "safe").await;
    if safe.get("captcha").and_then(Value::as_i64) == Some(1) {
        let captcha_id = body.str("captcha_id");
        let captcha = body.str("captcha");
        if captcha_id.is_empty() || !captcha::verify(&captcha_id, &captcha) {
            return fail("图形码不正确");
        }
    }

    // 用户名：邮箱 / 手机号优先（按 `ApiLogin` 的语义）
    let mut user_name = body.str("user_name").trim().to_string();
    let email = body.str("email").trim().to_string();
    let phone = body.str("phone").trim().to_string();
    if !email.is_empty() {
        user_name = email;
    } else if !phone.is_empty() {
        user_name = phone;
    }
    if user_name.is_empty() {
        return fail("请输入账号");
    }
    let password = body.str("password").trim().to_string();
    if password.chars().count() < 6 {
        return fail("请输入6位及以上长度的密码");
    }

    // 邮箱 / 手机号 / 用户名三种登录方式（按 `LoginViaPassword` 的语义）
    let found = if member::valid_email(&user_name) {
        member::get_by_email(pool, &user_name).await
    } else if member::valid_phone(&user_name) {
        member::get_by_phone(pool, &user_name).await
    } else {
        member::get_by_name(pool, &user_name).await
    };
    let found = match found {
        Ok(Some(row)) => row,
        Ok(None) => return fail("登录失败"),
        Err(err) => return failed(err),
    };
    if !auth::check_password(&password, &found.password) {
        return fail("登录失败");
    }
    if found.status != 1 {
        // 既有的 `UserStatusActive` 之外一律是这个提示（词条缺失时原样返回键名）
        return fail("UserNotActive");
    }

    let secret = match auth::token_secret(pool).await {
        Ok(secret) => secret,
        Err(err) => return failed(err),
    };
    let remember = body.bool("remember");
    let (token, _expire) = match auth::issue(TokenKind::User, &secret, found.id, remember) {
        Ok(pair) => pair,
        Err(err) => return failed(err),
    };
    if let Err(err) = member::touch_login(pool, found.id).await {
        tracing::warn!("更新会员登录时间失败：{err:#}");
    }

    let mut data = found.to_json_with_token(Some(&token));
    if let Value::Object(map) = &mut data {
        map.insert(
            "group".into(),
            member_group_json(pool, found.group_id, 0).await,
        );
    }
    with_cookie(ok(data), &token, remember)
}

/// `POST /logout`（既有的 `AccountLogout`，`return=json` 分支）。
///
/// 既有实现回 `code: 1001`（已退出），本项目成功一律 `code: 0`。
async fn logout() -> Response {
    let mut res = ok_msg("已退出登录");
    if let Ok(value) = header::HeaderValue::from_str("token=; Path=/; Max-Age=0") {
        res.headers_mut().insert(header::SET_COOKIE, value);
    }
    res
}

/// `POST /register`（按 `ApiRegister` 的语义）。
///
/// 请求字段：`user_name` / `password` / `email` / `phone` / `real_name` /
/// `first_name` / `last_name` / `avatar_url` / `introduce` / `birthday`。
async fn register(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let body = Body::parse(&body);

    let mut user_name = body.str("user_name").trim().to_string();
    let first_name = body.str("first_name").trim().to_string();
    let last_name = body.str("last_name").trim().to_string();
    let email = body.str("email").trim().to_string();
    let phone = body.str("phone").trim().to_string();

    // 既有的用户名兜底：first/last → 邮箱前缀
    if user_name.is_empty() && !first_name.is_empty() {
        user_name = first_name.clone();
        if !last_name.is_empty() {
            user_name.push('_');
            user_name.push_str(&last_name);
        }
    }
    if user_name.is_empty() && !email.is_empty() {
        user_name = email.split('@').next().unwrap_or("").to_string();
    }

    // 邀请人：读同名 cookie `invite`
    let mut parent_id = body.int("invite_id").max(0) as u32;
    if parent_id == 0 {
        if let Some(raw) = read_cookie(&headers, "invite") {
            if let Ok(value) = raw.trim().parse::<u32>() {
                parent_id = value;
            } else if let Ok(Some(row)) = member::get_by_name(pool, raw.trim()).await {
                parent_id = row.id;
            }
        }
    }

    let input = member::RegisterInput {
        user_name,
        password: body.str("password"),
        real_name: body.str("real_name"),
        first_name,
        last_name,
        avatar_url: body.str("avatar_url"),
        introduce: body.str("introduce"),
        email,
        phone,
        birthday: body.int("birthday"),
        parent_id,
    };
    let id = match member::register(pool, &input).await {
        Ok(id) => id,
        Err(err) => return crate::routes::model_error(err),
    };
    let row = match member::get(pool, id).await {
        Ok(Some(row)) => row,
        Ok(None) => return fail("用户不存在"),
        Err(err) => return failed(err),
    };

    let secret = match auth::token_secret(pool).await {
        Ok(secret) => secret,
        Err(err) => return failed(err),
    };
    let (token, _expire) = match auth::issue(TokenKind::User, &secret, id, false) {
        Ok(pair) => pair,
        Err(err) => return failed(err),
    };
    let _ = member::touch_login(pool, id).await;

    let mut data = row.to_json_with_token(Some(&token));
    if let Value::Object(map) = &mut data {
        map.insert(
            "group".into(),
            member_group_json(pool, row.group_id, 0).await,
        );
    }
    with_cookie(ok(data), &token, false)
}

/// `POST /forget`（找回密码：生成重置凭据）。
///
/// 既有实现是 `GET /forget` 页面 + `POST /api/verify/email`（`state=reset`）发信；
/// Rust 版没有邮件通道，重置凭据只在进程内存里保存（不回传给调用方）。
async fn forget(State(state): State<AppState>, body: Bytes) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let body = Body::parse(&body);
    let email = body.str("email").trim().to_string();
    if !member::valid_email(&email) {
        return fail("参数错误");
    }
    let row = match member::get_by_email(pool, &email).await {
        Ok(Some(row)) => row,
        Ok(None) => return fail("用户不存在"),
        Err(err) => return failed(err),
    };

    // 既有的重置令牌：`Md5(email + password)`（改完密码即失效）
    let token = member::md5_reset_token(&email, &row.password);
    let code = captcha::new_code(6);
    verify::put_reset(&email, &token, &code);
    tracing::info!("已生成 {email} 的密码重置凭据（邮件通道未接入，凭据仅存于进程内存）");

    ok_msg("请验证您的邮箱")
}

/// `POST /password/reset`（按 `ApiResetUserPassword` 的语义）。
///
/// 请求字段：`email` / `token` / `code` / `password`。
async fn password_reset(State(state): State<AppState>, body: Bytes) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let body = Body::parse(&body);
    let password = body.str("password").trim().to_string();
    if password.chars().count() < 6 {
        return fail("请填写6位以上的密码");
    }
    let email = body.str("email").trim().to_string();
    let token = body.str("token").trim().to_string();
    if !member::valid_email(&email) || token.is_empty() {
        return fail("参数错误");
    }
    let row = match member::get_by_email(pool, &email).await {
        Ok(Some(row)) => row,
        Ok(None) => return fail("用户不存在"),
        Err(err) => return failed(err),
    };
    if !verify::check_reset(&email, &token) {
        return fail("无效的令牌");
    }

    let hash = match auth::hash_password(&password) {
        Ok(hash) => hash,
        Err(err) => return failed(err),
    };
    if let Err(err) = member::update_password(pool, row.id, &hash).await {
        return failed(err);
    }
    verify::drop_reset(&email);
    ok_msg("密码修改成功")
}

/// `GET /account/password/reset`（既有的 `AccountPasswordResetPage` 的数据部分）。
///
/// 查询参数：`token` / `code` / `email`。
async fn account_password_reset(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let email = query_value(&query, "email").trim().to_string();
    let token = query_value(&query, "token").trim().to_string();
    if !member::valid_email(&email) || token.is_empty() {
        return fail("参数错误");
    }
    let row = match member::get_by_email(pool, &email).await {
        Ok(Some(row)) => row,
        Ok(None) => return fail("用户不存在"),
        Err(err) => return failed(err),
    };
    ok(json!({
        "email": row.email,
        "token": token,
        "code": query_value(&query, "code"),
        "user": { "id": row.id, "user_name": row.user_name },
    }))
}

/// `GET /account/info`（账户概览）。
async fn account_info(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    member_overview(pool, user_id).await
}

// ============================================================================
// 邮箱验证 / 验证码
// ============================================================================

/// `POST /verify/email`（按 `ApiSendVerifyEmail` 的语义）。
///
/// 请求字段：`email` / `state`（`verify` / `reset` / `exist` / `login`）。
async fn send_verify_email(State(state): State<AppState>, body: Bytes) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let body = Body::parse(&body);
    let email = body.str("email").trim().to_string();
    if !member::valid_email(&email) {
        return fail("参数错误");
    }
    let state_kind = body.str("state").trim().to_ascii_lowercase();

    let row = match member::get_by_email(pool, &email).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            if state_kind == "login" {
                // 快速登录：用户尚未注册，先完成注册
                let input = member::RegisterInput {
                    user_name: email.split('@').next().unwrap_or("").to_string(),
                    password: member::random_code(16),
                    email: email.clone(),
                    ..Default::default()
                };
                match member::register(pool, &input).await {
                    Ok(id) => match member::get(pool, id).await {
                        Ok(Some(row)) => row,
                        _ => return fail("用户不存在"),
                    },
                    Err(err) => return crate::routes::model_error(err),
                }
            } else {
                return fail("用户不存在");
            }
        }
        Err(err) => return failed(err),
    };

    if state_kind == "exist" {
        return ok(json!({ "id": row.id, "email": row.email }));
    }

    let code = captcha::new_code(6);
    verify::put_code(&email, &code, &state_kind);
    tracing::info!("已生成 {email} 的邮箱验证码（邮件通道未接入，验证码仅存于进程内存）");
    ok_msg("请验证您的邮箱")
}

/// `GET /verify/email`（按 `ApiVerifyEmail` 的语义）。
///
/// 查询参数：`token` / `code` / `email` / `state`（`verify` / `reset` /
/// `exist` / `login`）。
async fn verify_email(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let email = query_value(&query, "email").trim().to_string();
    let code = query_value(&query, "code").trim().to_string();
    let token = query_value(&query, "token").trim().to_string();
    let state_kind = query_value(&query, "state").trim().to_ascii_lowercase();

    if !member::valid_email(&email) {
        return fail("参数错误");
    }
    let row = member::get_by_email(pool, &email).await.ok().flatten();

    if state_kind == "exist" {
        let Some(row) = row else {
            return fail("用户不存在");
        };
        return ok(json!({ "id": row.id, "email": row.email }));
    }
    if state_kind == "login" && row.is_none() {
        return fail("用户不存在");
    }
    if !verify::check_code(&email, &code, &state_kind) {
        return fail("无效的令牌");
    }
    if state_kind == "reset" {
        return ok(json!({
            "email": email,
            "token": token,
            "code": code,
            "reset_url": format!("/account/password/reset?token={token}&code={code}&email={email}"),
        }));
    }

    let Some(row) = row else {
        return fail("用户不存在");
    };
    if !row.email_verified {
        if let Err(err) = member::set_email_verified(pool, row.id).await {
            return failed(err);
        }
    }
    let secret = match auth::token_secret(pool).await {
        Ok(secret) => secret,
        Err(err) => return failed(err),
    };
    let (token, _expire) = match auth::issue(TokenKind::User, &secret, row.id, true) {
        Ok(pair) => pair,
        Err(err) => return failed(err),
    };
    let _ = member::touch_login(pool, row.id).await;

    let mut data = row.to_json_with_token(Some(&token));
    if let Value::Object(map) = &mut data {
        map.insert(
            "group".into(),
            member_group_json(pool, row.group_id, 0).await,
        );
    }
    let mut res = ok(data);
    if let Ok(value) = header::HeaderValue::from_str(&token_cookie(&token, true)) {
        res.headers_mut().insert(header::SET_COOKIE, value);
    }
    res
}

/// `GET /captcha`（按 `GenerateCaptcha` 的语义）。
///
/// `safe.captcha != 1` 时回 `{ "captcha_off": true }`（与既有接口契约一致）；
/// 开启时返回 `{captcha_id, captcha}`，`captcha` 是 base64 PNG（无 data: 前缀）。
async fn captcha(State(state): State<AppState>) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let safe = setting_section(pool, "safe").await;
    if safe.get("captcha").and_then(Value::as_i64) != Some(1) {
        return ok(json!({ "captcha_off": true }));
    }
    let id = captcha::new_id();
    let code = captcha::new_code(4);
    captcha::put(&id, &code);
    ok(json!({
        "captcha_off": false,
        "captcha_id": id,
        "captcha": captcha::png_base64(&code),
    }))
}

// ============================================================================
// 会员中心
// ============================================================================

/// 会员中心概览（`/member/info`、`/account/info` 共用）。
async fn member_overview(pool: &sqlx::MySqlPool, user_id: u32) -> Response {
    let row = match member::get(pool, user_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return fail("用户不存在"),
        Err(err) => return failed(err),
    };
    let discount = user_discount(pool, user_id).await;
    let (_pending, comment_total) = match member::comment_stat(pool, user_id).await {
        Ok(pair) => pair,
        Err(err) => return failed(err),
    };
    let (archives, _) = match member::published(pool, user_id, 1, 1).await {
        Ok(pair) => pair,
        Err(err) => return failed(err),
    };
    let (guestbooks, _) = match member::guestbooks(pool, user_id, 1, 1).await {
        Ok(pair) => pair,
        Err(err) => return failed(err),
    };
    let favorites: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM archive_favorites WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(pool)
            .await
            .unwrap_or(0);

    let mut user = row.to_json();
    if let Value::Object(map) = &mut user {
        map.insert(
            "group".into(),
            member_group_json(pool, row.group_id, discount).await,
        );
    }
    ok(json!({
        "user": user,
        "group": member_group_json(pool, row.group_id, discount).await,
        "discount": discount,
        "counts": {
            "archives": archives,
            "comments": comment_total,
            "guestbooks": guestbooks,
            "favorites": favorites,
        },
    }))
}

/// `GET /member/info`（会员中心概览）。
async fn member_info(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    member_overview(pool, user_id).await
}

/// `GET /member/nav`（会员中心侧栏菜单，顺序与既有的 `memberNavEntries` 一致）。
async fn member_nav(State(state): State<AppState>) -> Response {
    if state.db().is_none() {
        return fail(NO_DB);
    }
    let entries = [
        ("index", "概览", "fa-dashboard"),
        ("profile", "我的资料", "fa-user-o"),
        ("password", "修改密码", "fa-lock"),
        ("cart", "我的购物车", "fa-shopping-cart"),
        ("favorites", "我的收藏", "fa-star-o"),
        ("orders", "我的订单", "fa-list-alt"),
        ("sales", "已售订单", "fa-truck"),
        ("published", "我的发布", "fa-file-text-o"),
        ("publish", "发布内容", "fa-plus-circle"),
        ("reviews", "收到的评价", "fa-commenting-o"),
        ("verify", "实名认证", "fa-id-card-o"),
        ("openapi", "开放接口", "fa-plug"),
        ("comments", "我的评论", "fa-comments-o"),
        ("messages", "我的留言", "fa-envelope-o"),
        ("finance", "我的资金", "fa-cny"),
        ("recharge", "账户充值", "fa-plus-circle"),
        ("withdraw", "余额提现", "fa-credit-card"),
        ("points", "我的积分", "fa-diamond"),
        ("invite", "推广邀请", "fa-users"),
        ("shop", "我的店铺", "fa-home"),
    ];
    let list: Vec<Value> = entries
        .iter()
        .map(|(route, title, icon)| {
            json!({
                "route": route,
                "title": title,
                "icon": icon,
                "link": if *route == "index" {
                    "/member".to_string()
                } else {
                    format!("/member/{route}")
                },
            })
        })
        .collect();
    ok(list)
}

/// `GET /member/points`（积分流水）。
async fn member_points(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let paging = Paging::new(&query, 20, member::POINT_LOG_LIMIT);
    match member::point_logs(pool, user_id, paging.page, paging.page_size).await {
        Ok((total, list)) => ok(json!({ "list": list, "total": total })),
        Err(err) => failed(err),
    }
}

// ============================================================================
// 会员接口
// ============================================================================

/// `GET /user/detail`（按 `ApiGetUserDetail` 的语义）。
async fn user_detail(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let row = match member::get(pool, user_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return fail("用户不存在"),
        Err(err) => return failed(err),
    };
    let discount = user_discount(pool, user_id).await;
    let mut data = row.to_json();
    if let Value::Object(map) = &mut data {
        map.insert(
            "group".into(),
            member_group_json(pool, row.group_id, discount).await,
        );
    }
    ok(data)
}

/// `POST /user/detail`（按 `ApiUpdateUserDetail` 的语义）。
///
/// 请求字段：`user_name` / `real_name` / `first_name` / `last_name` /
/// `avatar_url` / `introduce` / `phone` / `email` / `birthday`。
async fn user_detail_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let body = Body::parse(&body);

    let input = member::ProfileInput {
        user_name: Some(body.str("user_name")).filter(|v| !v.trim().is_empty()),
        real_name: Some(body.str("real_name")).filter(|v| !v.trim().is_empty()),
        first_name: Some(body.str("first_name")).filter(|v| !v.trim().is_empty()),
        last_name: Some(body.str("last_name")).filter(|v| !v.trim().is_empty()),
        avatar_url: Some(body.str("avatar_url")).filter(|v| !v.trim().is_empty()),
        introduce: Some(body.str("introduce")).filter(|v| !v.trim().is_empty()),
        phone: Some(body.str("phone")).filter(|v| !v.trim().is_empty()),
        email: Some(body.str("email")).filter(|v| !v.trim().is_empty()),
        birthday: if body.int("birthday") > 0 {
            Some(body.int("birthday"))
        } else {
            None
        },
    };
    match member::update_profile(pool, user_id, &input).await {
        Ok(()) => ok_msg("保存成功"),
        Err(err) => crate::routes::model_error(err),
    }
}

/// `GET /user/archives`（既有的 `ApiUserArchives`：我的发布）。
async fn user_archives(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let paging = Paging::new(&query, 20, 50);
    match member::published(pool, user_id, paging.page, paging.page_size).await {
        Ok((total, list)) => ok(json!({ "list": list, "total": total })),
        Err(err) => failed(err),
    }
}

/// `GET /user/archive`（既有的 `ApiUserArchiveDetail`：我的一条内容）。
async fn user_archive(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let id = query_int(&query, "id");
    if id <= 0 {
        return fail("参数错误");
    }
    let owner = match member::archive_owner(pool, id).await {
        Ok(Some(owner)) => owner,
        Ok(None) => return fail("文档不存在"),
        Err(err) => return failed(err),
    };
    if owner != user_id {
        return fail("无权操作该内容");
    }
    match member::archive_detail(pool, id).await {
        Ok(Some(data)) => ok(data),
        Ok(None) => fail("文档不存在"),
        Err(err) => failed(err),
    }
}

/// `GET /user/comments`（按 `ApiUserComments` 的语义）。
async fn user_comments(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let paging = Paging::new(&query, 20, 50);
    let status = match query_value(&query, "status").trim().to_ascii_lowercase().as_str() {
        "pending" => 0,
        "approved" => 1,
        "hidden" => 3,
        _ => -1,
    };
    match member::comments(pool, user_id, status, paging.page, paging.page_size).await {
        Ok((total, list)) => ok(json!({
            "list": list,
            "total": total,
            "page": paging.page,
            "page_size": paging.page_size,
        })),
        Err(err) => failed(err),
    }
}

/// `POST /user/comment/delete`（按 `ApiUserCommentDelete` 的语义）。
async fn user_comment_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let body = Body::parse(&body);
    let id = body.int("id").max(0) as u32;
    if id == 0 {
        return fail("评论不存在");
    }
    let comment = match member::own_comment(pool, user_id, id).await {
        Ok(comment) => comment,
        Err(err) => return crate::routes::model_error(err),
    };
    if let Err(err) = member::delete_comment(pool, comment.id, comment.archive_id).await {
        return failed(err);
    }
    ok(json!({ "id": comment.id }))
}

/// `POST /user/comment/reply`（按 `ApiUserCommentReply` 的语义）。
async fn user_comment_reply(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let body = Body::parse(&body);
    let id = body.int("id").max(0) as u32;
    let content = body.str("content").trim().to_string();
    if content.is_empty() {
        return fail("请输入内容");
    }
    let comment = match member::own_comment(pool, user_id, id).await {
        Ok(comment) => comment,
        Err(err) => return crate::routes::model_error(err),
    };
    let row = match member::get(pool, user_id).await {
        Ok(Some(row)) => row,
        _ => return need_login(),
    };
    let ip = client_ip(&headers);
    match member::reply_comment(pool, user_id, &row.user_name, &comment, &content, &ip).await {
        Ok(reply_id) => ok(json!({ "id": reply_id })),
        Err(err) => crate::routes::model_error(err),
    }
}

/// `POST /user/comment/status`（按 `ApiUserCommentStatus` 的语义）。
async fn user_comment_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let body = Body::parse(&body);
    let id = body.int("id").max(0) as u32;
    // 既有实现：只允许 0 / 1 / 3，其它值按 1 处理
    let status = match body.int("status") {
        0 => 0,
        3 => 3,
        _ => 1,
    };
    let comment = match member::own_comment(pool, user_id, id).await {
        Ok(comment) => comment,
        Err(err) => return crate::routes::model_error(err),
    };
    if let Err(err) = member::set_comment_status(pool, comment.id, status).await {
        return failed(err);
    }
    ok(json!({ "id": comment.id, "status": status }))
}

/// `GET /user/guestbooks`（按 `ApiUserGuestbooks` 的语义）。
async fn user_guestbooks(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let paging = Paging::new(&query, 20, 50);
    match member::guestbooks(pool, user_id, paging.page, paging.page_size).await {
        Ok((total, list)) => ok(json!({
            "list": list,
            "total": total,
            "page": paging.page,
            "page_size": paging.page_size,
        })),
        Err(err) => failed(err),
    }
}

/// `POST /user/guestbook/delete`（按 `ApiUserGuestbookDelete` 的语义）。
async fn user_guestbook_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let body = Body::parse(&body);
    let id = body.int("id").max(0) as u32;
    match member::delete_guestbook(pool, user_id, id).await {
        Ok(()) => ok(json!({ "id": id })),
        Err(err) => crate::routes::model_error(err),
    }
}

/// `GET /user/finances`（既有的 `ApiUserFinances`：资金流水）。
async fn user_finances(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let paging = Paging::new(&query, 20, member::POINT_LOG_LIMIT);
    match member::finances(pool, user_id, paging.page, paging.page_size).await {
        Ok((total, list)) => ok(json!({ "list": list, "total": total })),
        Err(err) => failed(err),
    }
}

/// `GET /user/groups`（按 `ApiGetUserGroups` 的语义）。
async fn user_groups(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let user_id = current_user_id(pool, &headers).await.unwrap_or(0);
    let discount = user_discount(pool, user_id).await;
    match member::groups(pool).await {
        Ok(groups) => {
            let list: Vec<Value> =
                groups.iter().map(|group| group_json(group, discount)).collect();
            ok(list)
        }
        Err(err) => failed(err),
    }
}

/// `GET /user/group/detail`（按 `ApiGetUserGroupDetail` 的语义）。
async fn user_group_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let id = query_int(&query, "id").max(0) as u32;
    let group = match member::group(pool, id).await {
        Ok(Some(group)) => group,
        Ok(None) => return fail("用户组不存在"),
        Err(err) => return failed(err),
    };
    let user_id = current_user_id(pool, &headers).await.unwrap_or(0);
    let discount = user_discount(pool, user_id).await;
    ok(group_json(&group, discount))
}

/// `POST /user/password`（按 `ApiUpdateUserPassword` 的语义）。
///
/// 请求字段：`old_password` / `password`。
async fn user_password(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let body = Body::parse(&body);
    let password = body.str("password").trim().to_string();
    let old_password = body.str("old_password").trim().to_string();
    if password.chars().count() < 6 {
        return fail("请填写6位以上的密码");
    }
    let row = match member::get(pool, user_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return fail("请登录"),
        Err(err) => return failed(err),
    };
    // 初次设置密码时不需要旧密码（既有的 `user.Password != ""` 判断）
    if !row.password.is_empty() && !auth::check_password(&old_password, &row.password) {
        return fail("旧密码错误");
    }
    let hash = match auth::hash_password(&password) {
        Ok(hash) => hash,
        Err(err) => return failed(err),
    };
    if let Err(err) = member::update_password(pool, user_id, &hash).await {
        return failed(err);
    }
    ok_msg("密码修改成功")
}

/// `POST /user/avatar`（既有的 `ApiUpdateUserAvatar`，multipart：`file`）。
async fn user_avatar(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let public_dir = state.with_config(|config| config.public_dir());

    let mut file: Option<(String, Vec<u8>)> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" || name == "file1" {
            let file_name = field.file_name().unwrap_or("avatar").to_string();
            match field.bytes().await {
                Ok(bytes) => file = Some((file_name, bytes.to_vec())),
                Err(err) => return failed(err.into()),
            }
        } else {
            let _ = field.bytes().await;
        }
    }
    let Some((file_name, bytes)) = file else {
        return fail("无法获取上传文件");
    };
    let ext = attachment::extension_of(&file_name);
    if !attachment::is_image_ext(&ext) {
        return fail("只允许上传图片文件");
    }
    if bytes.is_empty() {
        return fail("文件内容为空");
    }

    let saved = match save_upload(&pool, &public_dir, &file_name, &bytes, 0, user_id, "").await {
        Ok(saved) => saved,
        Err(err) => return failed(err),
    };
    // 回写会员头像
    if let Err(err) = sqlx::query("UPDATE users SET avatar_url = ?, updated_time = ? WHERE id = ?")
        .bind(&saved.file_location)
        .bind(chrono::Utc::now().timestamp())
        .bind(user_id)
        .execute(pool)
        .await
    {
        return failed(err.into());
    }
    ok(json!({ "avatar_url": saved.file_location }))
}

/// `POST /user/upload`（既有的 `ApiUserUploadImage`，multipart：`file`）。
///
/// 只允许图片，单文件不超过 20MB。
async fn user_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let public_dir = state.with_config(|config| config.public_dir());

    let mut file: Option<(String, Vec<u8>)> = None;
    let mut category_id = 0u32;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" | "file1" => {
                let file_name = field.file_name().unwrap_or("upload").to_string();
                match field.bytes().await {
                    Ok(bytes) => file = Some((file_name, bytes.to_vec())),
                    Err(err) => return failed(err.into()),
                }
            }
            "category_id" => {
                category_id = field
                    .text()
                    .await
                    .ok()
                    .and_then(|value| value.trim().parse::<u32>().ok())
                    .unwrap_or(0);
            }
            _ => {
                let _ = field.bytes().await;
            }
        }
    }
    let Some((file_name, bytes)) = file else {
        return fail("请选择要上传的图片");
    };
    if bytes.len() > 20 * 1024 * 1024 {
        return fail("图片不能超过 20MB");
    }
    let ext = attachment::extension_of(&file_name);
    if !attachment::is_image_ext(&ext) {
        return fail("只允许上传图片文件");
    }

    let saved = match save_upload(&pool, &public_dir, &file_name, &bytes, category_id, user_id, "")
        .await
    {
        Ok(saved) => saved,
        Err(err) => return failed(err),
    };
    ok(json!({
        "id": saved.id,
        "path": saved.file_location,
        "url": saved.file_location,
        "thumb": saved.file_location,
        "name": saved.file_name,
    }))
}

/// `GET /user/auth/info`（按 `ApiUserAuthInfo` 的语义）。
async fn user_auth_info(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let auth = match member::auth_by_user(pool, user_id).await {
        Ok(auth) => auth,
        Err(err) => return failed(err),
    };
    let Some(auth) = auth else {
        // 既有实现：没有记录时回 status = -2
        return ok(json!({ "status": -2 }));
    };
    ok(json!({
        "status": auth.status,
        "type": auth.kind,
        "status_text": auth_status_text(auth.status),
        "remark": auth.remark,
        "audit_time": auth.audit_time,
        "real_name": auth.real_name.trim(),
        "company_name": auth.company_name.trim(),
        "id": auth.id,
    }))
}

/// `POST /user/auth/apply`（按 `ApiUserAuthApply` 的语义）。
///
/// 请求字段：`type` / `real_name` / `id_card` / `id_card_front` /
/// `id_card_back` / `id_card_hand` / `company_name` / `company_no` /
/// `company_legal` / `company_licence` / `phone`。
async fn user_auth_apply(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let body = Body::parse(&body);
    let input = member::AuthInput {
        kind: body.int("type") as i32,
        real_name: body.str("real_name"),
        id_card: body.str("id_card"),
        id_card_front: body.str("id_card_front"),
        id_card_back: body.str("id_card_back"),
        id_card_hand: body.str("id_card_hand"),
        company_name: body.str("company_name"),
        company_no: body.str("company_no"),
        company_legal: body.str("company_legal"),
        company_licence: body.str("company_licence"),
        phone: body.str("phone"),
    };
    match member::apply_auth(pool, user_id, &input).await {
        Ok(auth) => ok(json!({
            "id": auth.id,
            "user_id": auth.user_id,
            "type": auth.kind,
            "real_name": auth.real_name,
            "company_name": auth.company_name,
            "phone": auth.phone,
            "status": auth.status,
            "remark": auth.remark,
            "audit_time": auth.audit_time,
        })),
        Err(err) => crate::routes::model_error(err),
    }
}

/// 认证状态文案（按 `memberAuthStatusText` 的语义）。
fn auth_status_text(status: i32) -> &'static str {
    match status {
        member::USER_AUTH_STATUS_PASSED => "已认证",
        member::USER_AUTH_STATUS_REJECTED => "已驳回",
        _ => "待审核",
    }
}

/// `GET /user/openapi/info`（按 `ApiMemberOpenApiInfo` 的语义）。
async fn user_openapi_info(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let token = match member::api_token(pool, user_id).await {
        Ok(token) => token,
        Err(err) => return failed(err),
    };
    let base_url = site_base_url(pool).await;
    ok(json!({
        "enabled": !token.is_empty(),
        "token": token,
        "masked": member::mask_api_token(&token),
        "rest_base": format!("{}{}", base_url, member::MEMBER_OPEN_API_BASE_PATH),
        "mcp_url": format!("{}{}", base_url, member::MEMBER_MCP_PATH),
        "cli_package": "rustcms-cli",
    }))
}

/// `POST /user/openapi/token`（按 `ApiMemberOpenApiToken` 的语义）。
///
/// 请求字段：`action`（`reset` / `generate` / `clear` / `disable`）。
async fn user_openapi_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let body = Body::parse(&body);
    let action = body.str("action").trim().to_ascii_lowercase();
    match action.as_str() {
        "clear" | "disable" => {
            if let Err(err) = member::clear_api_token(pool, user_id).await {
                return failed(err);
            }
            ok(json!({ "enabled": false, "token": "", "masked": "" }))
        }
        "reset" | "generate" | "" => match member::reset_api_token(pool, user_id).await {
            Ok(token) => ok(json!({
                "enabled": true,
                "token": token,
                "masked": member::mask_api_token(&token),
            })),
            Err(err) => failed(err),
        },
        _ => fail("操作不合法"),
    }
}

// ============================================================================
// 签到
// ============================================================================

/// `GET /sign/info`（按 `ApiSignInfo` 的语义）。
async fn sign_info(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    match member::sign_info(pool, user_id).await {
        Ok(info) => ok(json!({
            "checked_today": info.checked_today,
            "days": info.days,
            "month_count": info.month_count,
            "points": info.points,
            "base_points": member::SIGN_BASE_POINTS,
        })),
        Err(err) => failed(err),
    }
}

/// `POST /sign/do`（按 `ApiSignDo` 的语义）。
async fn sign_do(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(user_id) = current_user_id(pool, &headers).await else {
        return need_login();
    };
    let sign = match member::sign_today(pool, user_id).await {
        Ok(sign) => sign,
        Err(err) => return crate::routes::model_error(err),
    };
    match member::sign_info(pool, user_id).await {
        Ok(info) => ok(json!({
            "sign": sign,
            "checked_today": info.checked_today,
            "days": info.days,
            "month_count": info.month_count,
            "points": info.points,
        })),
        Err(err) => failed(err),
    }
}

// ============================================================================
// 订阅 / 附件上传
// ============================================================================

/// `POST /subscription`（按 `ApiAddSubscriber` 的语义）。
///
/// 请求字段：`email`。
async fn subscription(State(state): State<AppState>, body: Bytes) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let body = Body::parse(&body);
    let email = body.str("email").trim().to_string();
    if email.is_empty() || !member::valid_email(&email) {
        return fail("请填写邮箱");
    }
    // 已订阅：既有实现直接回成功（不重复登记）
    match crate::models::subscriber::save(pool, 0, &email).await {
        Ok(_) => ok_msg("操作成功"),
        Err(err) => crate::routes::model_error(err),
    }
}

/// `POST /attachment/upload`（既有的 `AttachmentUpload`，需要导入接口令牌）。
///
/// multipart：`file` / `file1` + `category_id`；令牌在表单字段 `token`
/// （与既有的 `VerifyApiToken` 一致）。
async fn attachment_upload(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let public_dir = state.with_config(|config| config.public_dir());

    let mut token = String::new();
    let mut category_id = 0u32;
    let mut replace_id = 0u32;
    let mut file: Option<(String, Vec<u8>)> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "token" => {
                token = field.text().await.unwrap_or_default().trim().to_string();
            }
            "category_id" => {
                category_id = field
                    .text()
                    .await
                    .ok()
                    .and_then(|value| value.trim().parse::<u32>().ok())
                    .unwrap_or(0);
            }
            "id" => {
                replace_id = field
                    .text()
                    .await
                    .ok()
                    .and_then(|value| value.trim().parse::<u32>().ok())
                    .unwrap_or(0);
            }
            "file" | "file1" => {
                let file_name = field.file_name().unwrap_or("upload").to_string();
                match field.bytes().await {
                    Ok(bytes) => file = Some((file_name, bytes.to_vec())),
                    Err(err) => return failed(err.into()),
                }
            }
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    // 导入接口令牌校验（按 `VerifyApiToken` 的语义）
    let import_cfg = setting_section(pool, "import_api").await;
    let expect = import_cfg
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if expect.is_empty() || token != expect {
        return fail("Token错误");
    }
    if replace_id > 0 {
        // 前台用户不允许替换已有附件（按 `UnableToModifyTheImage` 的语义）
        return fail("无法修改图片");
    }
    let Some((file_name, bytes)) = file else {
        return fail("无法获取上传文件");
    };
    let ext = attachment::extension_of(&file_name);
    if !attachment::is_allowed_ext(&ext) {
        return fail("不允许上传该类型文件");
    }

    let saved = match save_upload(&pool, &public_dir, &file_name, &bytes, category_id, 0, "").await {
        Ok(saved) => saved,
        Err(err) => return failed(err),
    };
    ok(json!({
        "id": saved.id,
        "created_time": saved.created_time,
        "updated_time": saved.updated_time,
        "user_id": saved.user_id,
        "file_name": saved.file_name,
        "file_location": saved.file_location,
        "file_size": saved.file_size,
        "file_md5": saved.file_md5,
        "width": saved.width,
        "height": saved.height,
        "category_id": saved.category_id,
        "is_image": saved.is_image,
        "status": saved.status,
        "watermark": saved.watermark,
        "is_remote": saved.is_remote,
        "logo": saved.logo,
    }))
}

/// 上传结果（按 `model.Attachment` 的语义）。
struct SavedUpload {
    id: u32,
    created_time: i64,
    updated_time: i64,
    user_id: u32,
    file_name: String,
    file_location: String,
    file_size: u64,
    file_md5: String,
    width: u32,
    height: u32,
    category_id: u32,
    is_image: i8,
    status: u8,
    watermark: i8,
    is_remote: i8,
    logo: String,
}

/// 落盘 + 登记附件（路径口径与后台的 `manage_attachment_upload` 一致）。
async fn save_upload(
    pool: &sqlx::MySqlPool,
    public_dir: &std::path::Path,
    file_name: &str,
    bytes: &[u8],
    category_id: u32,
    user_id: u32,
    logo: &str,
) -> anyhow::Result<SavedUpload> {
    let ext = attachment::extension_of(file_name);
    let md5 = attachment::md5_hex(bytes);
    let (width, height) = attachment::image_size(bytes);
    let is_image: i8 = if attachment::is_image_ext(&ext) {
        1
    } else if attachment::is_video_ext(&ext) {
        2
    } else {
        0
    };

    // 同一份文件只存一条（`file_md5` 唯一）
    if let Ok(Some(existing)) = attachment::get_by_md5(pool, &md5).await {
        if category_id > 0 && existing.category_id != category_id {
            let _ = attachment::change_category(pool, category_id, &[existing.id]).await;
        }
        return Ok(SavedUpload {
            id: existing.id,
            created_time: existing.created_time.unwrap_or(0),
            updated_time: existing.updated_time.unwrap_or(0),
            user_id: existing.user_id,
            file_name: existing.file_name,
            file_location: existing.file_location,
            file_size: existing.file_size,
            file_md5: existing.file_md5,
            width: existing.width,
            height: existing.height,
            category_id: existing.category_id,
            is_image: existing.is_image,
            status: existing.status,
            watermark: existing.watermark,
            is_remote: existing.is_remote,
            logo: existing.logo,
        });
    }

    let now = chrono::Local::now();
    let subdir = format!("uploads/{}/{}", now.format("%Y%m"), now.format("%d"));
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name);
    let mut base = if stem
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        stem.to_lowercase()
    } else {
        md5.get(8..24).unwrap_or(&md5).to_string()
    };
    if base.chars().count() > 200 {
        base = base.chars().take(200).collect();
    }
    let file_location = format!("{subdir}/{base}{ext}");

    let full = public_dir.join(&file_location);
    if let Some(parent) = full.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&full, bytes).await?;

    let id = attachment::insert(
        pool,
        stem,
        &file_location,
        bytes.len() as u64,
        &md5,
        width,
        height,
        category_id,
        is_image,
        logo,
    )
    .await?;
    if user_id > 0 {
        let _ = sqlx::query("UPDATE attachments SET user_id = ? WHERE id = ?")
            .bind(user_id)
            .bind(id)
            .execute(pool)
            .await;
    }

    Ok(SavedUpload {
        id,
        created_time: now.timestamp(),
        updated_time: now.timestamp(),
        user_id,
        file_name: stem.to_string(),
        file_location,
        file_size: bytes.len() as u64,
        file_md5: md5,
        width,
        height,
        category_id,
        is_image,
        status: 1,
        watermark: 0,
        is_remote: 0,
        logo: logo.to_string(),
    })
}

// ============================================================================
// 验证码（进程内一次性凭据 + 自绘 PNG）
// ============================================================================

/// 图形验证码的一次性凭据（进程内，重启即失效）。
mod captcha {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use rand::Rng;

    /// 凭据有效期。
    const TTL: Duration = Duration::from_secs(5 * 60);

    static STORE: Mutex<Option<HashMap<String, (String, Instant)>>> = Mutex::new(None);

    fn with_store<T>(f: impl FnOnce(&mut HashMap<String, (String, Instant)>) -> T) -> T {
        let mut guard = STORE.lock().unwrap_or_else(|err| err.into_inner());
        let store = guard.get_or_insert_with(HashMap::new);
        let now = Instant::now();
        store.retain(|_, (_, expire)| *expire > now);
        f(store)
    }

    /// 新的凭据 ID。
    pub fn new_id() -> String {
        crate::models::member::random_code(32)
    }

    /// 新的验证码（纯数字）。
    pub fn new_code(len: usize) -> String {
        let mut rng = rand::thread_rng();
        (0..len)
            .map(|_| char::from(b'0' + rng.gen_range(0..10)))
            .collect()
    }

    /// 记录验证码。
    pub fn put(id: &str, code: &str) {
        with_store(|store| {
            store.insert(id.to_string(), (code.to_string(), Instant::now() + TTL));
        });
    }

    /// 校验验证码（一次性：校验通过即作废）。
    pub fn verify(id: &str, code: &str) -> bool {
        with_store(|store| match store.remove(id) {
            Some((expected, _)) => expected.eq_ignore_ascii_case(code.trim()),
            None => false,
        })
    }

    /// 生成 base64 PNG（8 位灰度，无 data: 前缀）。
    pub fn png_base64(code: &str) -> String {
        super::png::encode(code)
    }
}

/// 邮箱验证码 / 密码重置凭据（进程内）。
mod verify {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const TTL: Duration = Duration::from_secs(30 * 60);

    static CODES: Mutex<Option<HashMap<String, (String, String, Instant)>>> = Mutex::new(None);
    static RESETS: Mutex<Option<HashMap<String, (String, String, Instant)>>> = Mutex::new(None);

    /// 记录邮箱验证码。
    pub fn put_code(email: &str, code: &str, state: &str) {
        let mut guard = CODES.lock().unwrap_or_else(|err| err.into_inner());
        let store = guard.get_or_insert_with(HashMap::new);
        let now = Instant::now();
        store.retain(|_, (_, _, expire)| *expire > now);
        store.insert(
            email.to_string(),
            (code.to_string(), state.to_string(), now + TTL),
        );
    }

    /// 校验邮箱验证码（一次性）。
    pub fn check_code(email: &str, code: &str, state: &str) -> bool {
        let mut guard = CODES.lock().unwrap_or_else(|err| err.into_inner());
        let Some(store) = guard.as_mut() else {
            return false;
        };
        let now = Instant::now();
        store.retain(|_, (_, _, expire)| *expire > now);
        match store.remove(email) {
            Some((expected, expected_state, _)) => {
                expected.eq_ignore_ascii_case(code.trim())
                    && (expected_state.is_empty()
                        || expected_state.eq_ignore_ascii_case(state)
                        || state.is_empty())
            }
            None => false,
        }
    }

    /// 记录密码重置凭据。
    pub fn put_reset(email: &str, token: &str, code: &str) {
        let mut guard = RESETS.lock().unwrap_or_else(|err| err.into_inner());
        let store = guard.get_or_insert_with(HashMap::new);
        let now = Instant::now();
        store.retain(|_, (_, _, expire)| *expire > now);
        store.insert(
            email.to_string(),
            (token.to_string(), code.to_string(), now + TTL),
        );
    }

    /// 校验密码重置凭据。
    pub fn check_reset(email: &str, token: &str) -> bool {
        let mut guard = RESETS.lock().unwrap_or_else(|err| err.into_inner());
        let Some(store) = guard.as_mut() else {
            return false;
        };
        let now = Instant::now();
        store.retain(|_, (_, _, expire)| *expire > now);
        match store.get(email) {
            Some((expected, _, _)) => expected == token,
            None => false,
        }
    }

    /// 丢弃密码重置凭据。
    pub fn drop_reset(email: &str) {
        let mut guard = RESETS.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(store) = guard.as_mut() {
            store.remove(email);
        }
    }
}

/// 极简 PNG 编码器（8 位灰度，zlib stored 块）。
///
/// 没有引入图片库，所以自己写一份够用的 PNG：
/// `IHDR` + `IDAT`（zlib 的 stored 块，不压缩）+ `IEND`。
mod png {
    use rand::Rng;

    /// 5x7 点阵字模（数字 0-9，每行 5 位，最低位在左）。
    const GLYPHS: [[u8; 7]; 10] = [
        [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
        [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
    ];

    /// 生成验证码图片的 base64（无 data: 前缀）。
    pub fn encode(code: &str) -> String {
        let digits: Vec<u8> = code.bytes().filter(|b| b.is_ascii_digit()).collect();
        let scale = 4usize;
        let glyph_w = 5 * scale;
        let gap = 6usize;
        let width = digits.len() * (glyph_w + gap) + gap;
        let height = 7 * scale + 16;
        let mut canvas = vec![255u8; width * height];

        // 干扰线
        let mut rng = rand::thread_rng();
        for _ in 0..6 {
            let y = rng.gen_range(0..height);
            let x0 = rng.gen_range(0..width);
            let len = rng.gen_range(8..width.max(9).min(64));
            for x in x0..(x0 + len).min(width) {
                let idx = y * width + x;
                canvas[idx] = 200;
            }
        }

        // 字符
        for (index, digit) in digits.iter().enumerate() {
            let glyph = GLYPHS[(*digit - b'0') as usize % 10];
            let origin_x = gap + index * (glyph_w + gap);
            let origin_y = 8;
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << col) == 0 {
                        continue;
                    }
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let x = origin_x + col * scale + dx;
                            let y = origin_y + row * scale + dy;
                            if x < width && y < height {
                                canvas[y * width + x] = 40;
                            }
                        }
                    }
                }
            }
        }

        let png = build(&canvas, width as u32, height as u32);
        base64(&png)
    }

    /// 组装 PNG 字节流。
    fn build(pixels: &[u8], width: u32, height: u32) -> Vec<u8> {
        // 原始数据：每行一个过滤字节（0 = None）+ 灰度像素
        let mut raw = Vec::with_capacity((width as usize + 1) * height as usize);
        for y in 0..height as usize {
            raw.push(0);
            let start = y * width as usize;
            raw.extend_from_slice(&pixels[start..start + width as usize]);
        }

        let mut out = Vec::new();
        out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.push(8); // 位深
        ihdr.push(0); // 颜色类型：灰度
        ihdr.push(0); // 压缩
        ihdr.push(0); // 过滤
        ihdr.push(0); // 隔行
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &zlib_store(&raw));
        chunk(&mut out, b"IEND", &[]);
        out
    }

    /// 写一个 PNG 块（长度 + 类型 + 数据 + CRC）。
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc_input = Vec::with_capacity(4 + data.len());
        crc_input.extend_from_slice(kind);
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    /// zlib 流（stored 块，不压缩）。
    fn zlib_store(raw: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78u8, 0x01];
        if raw.is_empty() {
            out.push(1);
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&(!0u16).to_le_bytes());
        } else {
            let mut offset = 0usize;
            while offset < raw.len() {
                let len = (raw.len() - offset).min(65535);
                let last = offset + len >= raw.len();
                out.push(if last { 1 } else { 0 });
                out.extend_from_slice(&(len as u16).to_le_bytes());
                out.extend_from_slice(&(!(len as u16)).to_le_bytes());
                out.extend_from_slice(&raw[offset..offset + len]);
                offset += len;
            }
        }
        out.extend_from_slice(&adler32(raw).to_be_bytes());
        out
    }

    /// CRC32（PNG 用）。
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// Adler32（zlib 用）。
    fn adler32(data: &[u8]) -> u32 {
        let mut a = 1u32;
        let mut b = 0u32;
        for byte in data {
            a = (a + u32::from(*byte)) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    /// base64 编码。
    fn base64(data: &[u8]) -> String {
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
        for chunk in data.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = *chunk.get(1).unwrap_or(&0) as u32;
            let b2 = *chunk.get(2).unwrap_or(&0) as u32;
            let bits = (b0 << 16) | (b1 << 8) | b2;
            out.push(TABLE[((bits >> 18) & 63) as usize] as char);
            out.push(TABLE[((bits >> 12) & 63) as usize] as char);
            if chunk.len() > 1 {
                out.push(TABLE[((bits >> 6) & 63) as usize] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(TABLE[(bits & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
        out
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};
    use axum::http::StatusCode;
    use tower::ServiceExt;

    fn test_state(pool: Option<sqlx::MySqlPool>) -> AppState {
        let config = crate::config::ServerJson {
            template_dir: crate::models::test_template_dir()
                .to_string_lossy()
                .to_string(),
            public_dir: std::env::temp_dir()
                .join(format!("rustcms-member-public-{}", std::process::id()))
                .to_string_lossy()
                .to_string(),
            ..Default::default()
        };
        AppState::new(config, pool)
    }

    async fn body_json(res: axum::http::Response<axum::body::Body>) -> Value {
        let bytes = axum::body::to_bytes(res.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    #[test]
    fn parses_json_and_form_bodies() {
        let json = Body::parse(&Bytes::from_static(br#"{"user_name":"abc","age":18,"ok":true}"#));
        assert_eq!(json.str("user_name"), "abc");
        assert_eq!(json.int("age"), 18);
        assert!(json.bool("ok"));

        let form = Body::parse(&Bytes::from_static(
            b"user_name=abc&age=18&ok=true&name=%E4%B8%AD%E6%96%87",
        ));
        assert_eq!(form.str("user_name"), "abc");
        assert_eq!(form.int("age"), 18);
        assert!(form.bool("ok"));
        assert_eq!(form.str("name"), "中文");

        let empty = Body::parse(&Bytes::from_static(b""));
        assert_eq!(empty.str("user_name"), "");
        assert_eq!(empty.int("age"), 0);
        assert!(!empty.bool("ok"));
    }

    #[test]
    fn paging_defaults_match_go() {
        let query = HashMap::new();
        let paging = Paging::new(&query, 20, 50);
        assert_eq!(paging.page, 1);
        assert_eq!(paging.page_size, 20);

        let mut query = HashMap::new();
        query.insert("page".to_string(), "3".to_string());
        query.insert("page_size".to_string(), "10".to_string());
        let paging = Paging::new(&query, 20, 50);
        assert_eq!(paging.page, 3);
        assert_eq!(paging.page_size, 10);

        // 超过上限会被夹住；pageSize 也认
        let mut query = HashMap::new();
        query.insert("pageSize".to_string(), "999".to_string());
        let paging = Paging::new(&query, 20, 50);
        assert_eq!(paging.page_size, 50);
    }

    #[test]
    fn form_decoder_handles_plus_and_percent() {
        assert_eq!(form_decode("a+b"), "a b");
        assert_eq!(form_decode("%E4%B8%AD"), "中");
    }

    #[test]
    fn captcha_store_is_one_time() {
        let id = captcha::new_id();
        captcha::put(&id, "1234");
        assert!(captcha::verify(&id, "1234"));
        // 一次性：校验后即作废
        assert!(!captcha::verify(&id, "1234"));
    }

    #[test]
    fn png_encoder_produces_valid_png() {
        let encoded = png::encode("1234");
        assert!(!encoded.is_empty());
        let bytes = decode_base64(&encoded);
        assert_eq!(&bytes[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("IHDR"));
        assert!(text.contains("IDAT"));
        assert!(text.contains("IEND"));
    }

    /// 简易 base64 解码（测试用）。
    fn decode_base64(text: &str) -> Vec<u8> {
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut table = [255u8; 256];
        for (index, byte) in TABLE.iter().enumerate() {
            table[*byte as usize] = index as u8;
        }
        let mut out = Vec::new();
        let mut buffer = 0u32;
        let mut bits = 0u32;
        for ch in text.bytes() {
            let value = table[ch as usize];
            if value == 255 {
                continue;
            }
            buffer = (buffer << 6) | u32::from(value);
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push(((buffer >> bits) & 0xFF) as u8);
            }
        }
        out
    }

    #[tokio::test]
    async fn requires_login_for_member_endpoints() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = test_state(Some(pool));
        let app = Router::new().merge(router()).with_state(state);

        for uri in [
            "/user/detail",
            "/user/archives",
            "/user/comments",
            "/user/guestbooks",
            "/user/finances",
            "/member/info",
        ] {
            let res = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method("GET")
                        .uri(uri)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK, "{uri}");
            let body = body_json(res).await;
            assert_eq!(body["code"], 1, "{uri} -> {body}");
            assert_eq!(body["msg"], "请先登录", "{uri}");
        }
    }

    #[tokio::test]
    async fn login_validates_input() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = test_state(Some(pool));
        let app = Router::new().merge(router()).with_state(state);

        // 账号为空
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"user_name":"","password":"123456"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1);
        assert_eq!(body["msg"], "请输入账号");

        // 密码太短
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"user_name":"rustcms-none","password":"123"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1);
        assert_eq!(body["msg"], "请输入6位及以上长度的密码");

        // 用户不存在
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"user_name":"rustcms-not-exists","password":"123456"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1);
        assert_eq!(body["msg"], "登录失败");
    }

    #[tokio::test]
    async fn register_then_login_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = test_state(Some(pool.clone()));
        let app = Router::new().merge(router()).with_state(state);
        let _ = sqlx::query("DELETE FROM users WHERE user_name LIKE 'rustcms-api%'")
            .execute(&pool)
            .await;

        // 注册
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"user_name":"rustcms-api","password":"pass-123456","email":"rustcms-api@example.com"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["user_name"], "rustcms-api");
        assert!(body["data"]["token"].as_str().is_some(), "{body}");
        assert!(body["data"].get("password").is_none(), "不能带出密码");

        // 登录
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"user_name":"rustcms-api","password":"pass-123456"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let token = body["data"]["token"].as_str().unwrap().to_string();
        assert!(!token.is_empty());

        // 带上令牌访问会员接口
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/user/detail")
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["user_name"], "rustcms-api");

        // 会员中心概览
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/member/info")
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["user"]["user_name"], "rustcms-api");

        // 开放接口 Key
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/user/openapi/token")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"action":"reset"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["enabled"], true);
        assert_eq!(body["token"].as_str().unwrap().len(), 40);

        // 清理
        let _ = sqlx::query("DELETE FROM users WHERE user_name LIKE 'rustcms-api%'")
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    async fn captcha_is_off_by_default() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = test_state(Some(pool));
        let app = Router::new().merge(router()).with_state(state);

        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/captcha")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].get("captcha_off").is_some(), "{body}");
    }

    #[tokio::test]
    async fn subscription_requires_valid_email() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let state = test_state(Some(pool));
        let app = Router::new().merge(router()).with_state(state);

        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/subscription")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"email":"not-an-email"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1);
        assert_eq!(body["msg"], "请填写邮箱");
    }
}

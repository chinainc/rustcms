//! 路由表。
//!
//! 响应信封与 Go 版一致：`{"code":0,"msg":"","data":...}`，
//! `code` 取 `config.StatusOK(0)` / `config.StatusFailed(1)`。

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Multipart, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth;
use crate::login_guard;
use crate::models::archive_write::{self, ArchiveInput};
use crate::models::{
    admin, anchor, anqi, archive, archive_admin, attachment, backup, category, collector,
    commission, comment, design, diy, extra, guestbook, link, llms, material, menu, module,
    multilang, nav, order, payment, place, plugin, redirect, replace, retailer, sensitive,
    sendmail, setting, site_setting, skill, statistic, subscriber, tag, titleimage, transfer,
    translate, upload_file, user, user_admin, user_auth, watermark, wechat, website,
    withdraw, Category, ModelError, json_string_list,
};
use crate::state::AppState;

/// 组装应用路由。
///
/// `/api`、`/system/api` 的精确路由优先；其余路径全部落入前台管线
/// `front::reroute`（对齐 Go 的 `/{path:path}` 兜底入口）。
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/preview/list/:cid", get(preview_list))
        .nest("/api", api_router())
        // Go 版后台接口的真实前缀是 `/system/api/...`（见 route/manage.go）
        .nest("/system/api", manage_router())
        // 前台交互 API（批次 A/B/C/D）与后台缺口（批次 E），各自独立文件。
        .merge(crate::api_content::router())
        .merge(crate::api_member::router())
        .merge(crate::api_trade::router())
        .merge(crate::api_interact::router())
        .merge(crate::api_admin_gap::router())
        .fallback(crate::front::reroute)
        .with_state(state)
}

/// `GET /preview/list/{cid}`：用真实模板 + 真实数据渲染栏目列表页。
async fn preview_list(State(state): State<AppState>, Path(cid): Path<u32>) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let target = match category::get(pool, cid).await {
        Ok(Some(item)) => item,
        Ok(None) => return fail("分类不存在"),
        Err(err) => return failed(err),
    };
    crate::front::render_category_preview(&state, &target).await
}

fn api_router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/categories", get(category_list))
        .route("/categories/:id", get(category_detail))
        .route("/modules", get(module_list))
        .route("/archives", get(archive_list).post(archive_create))
        .route(
            "/archives/:id",
            get(archive_detail)
                .put(archive_update)
                .delete(archive_delete),
        )
        .route("/admin/login", post(admin_login))
        .route("/admin/logout", post(admin_logout))
        .route("/admin/me", get(admin_me))
        .route("/admin/menus", get(admin_menus))
        .route("/admin/list", get(admin_list))
        .route("/admin/detail", get(admin_detail).post(admin_detail_save))
        .route("/admin/delete", post(admin_delete))
        .route("/admin/group/list", get(admin_group_list))
        .route(
            "/admin/group/detail",
            get(admin_group_detail).post(admin_group_save),
        )
        .route("/admin/group/delete", post(admin_group_delete))
        .route("/admin/logs/login", get(admin_login_logs))
        .route("/admin/logs/action", get(admin_action_logs))
        .route("/user/login", post(user_login))
        .route("/user/me", get(user_me))
        // `/api/*` 未命中时回 JSON 404，而不是落到前台路由
        .fallback(api_not_found)
}

/// `/api/*` 未命中时的 JSON 404。
pub(crate) async fn api_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "code": 1, "msg": "接口不存在" })),
    )
        .into_response()
}

/// 业务错误 → 响应信封。
pub(crate) fn model_error(err: ModelError) -> Response {
    match err {
        ModelError::Invalid(msg) => fail(msg),
        ModelError::Db(err) => failed(err),
    }
}

/// 取出 `Authorization: Bearer <token>`。
pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// 用管理员令牌换出管理员信息（失效返回 `Ok(None)`）。
async fn admin_from_token(
    pool: &sqlx::MySqlPool,
    token: &str,
) -> anyhow::Result<Option<admin::Admin>> {
    let secret = auth::token_secret(pool).await?;
    let Ok(claims) = auth::verify(auth::TokenKind::Admin, &secret, token) else {
        return Ok(None);
    };
    let Some(id) = claims.subject(auth::TokenKind::Admin) else {
        return Ok(None);
    };
    let found = admin::get(pool, id).await?;
    Ok(found.filter(|item| item.status == 1))
}

/// 写入接口鉴权：`api_token` 或管理员登录令牌，二者取其一。
async fn deny_write(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    let expected = state
        .with_config(|c| c.api_token.clone())
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty());

    let provided = headers
        .get("x-api-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");

    if let Some(expected) = expected.as_ref() {
        if provided == expected {
            return None;
        }
    }

    if let Some(token) = bearer_token(headers) {
        let Some(pool) = state.db() else {
            return Some(fail(NO_DB));
        };
        return match admin_from_token(pool, &token).await {
            Ok(Some(_)) => None,
            Ok(None) => Some(fail("登录已失效，请重新登录")),
            Err(err) => Some(failed(err)),
        };
    }

    if expected.is_none() {
        return Some(fail(
            "写入接口未启用：请先登录后台，或在 config.json 配置 api_token",
        ));
    }
    Some(fail("无效的 api_token"))
}

/// 登录请求体。
#[derive(Debug, Deserialize)]
struct LoginRequest {
    user_name: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    remember: bool,
}

/// 账号 / IP 被锁定时的提示（文案取自 Go 版词条 `AdministratorHasBeenTemporarilyLocked`）。
const LOCKED: &str = "管理员已被临时锁定，请稍后重试";

/// 取客户端 IP：优先反向代理头，其次 TCP 对端地址。
fn client_ip(headers: &HeaderMap, peer: SocketAddr) -> String {
    let header_ip = headers
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

    match header_ip {
        Some(ip) => ip.to_string(),
        None => peer.ip().to_string(),
    }
}

/// 管理员对外返回的内容：序列化字段 + 计算出来的 `is_super`。
///
/// `is_super` 对应 Go 版 `Admin.IsSuper`（主站点 + 1 号用户组），本项目单站点，
/// 因此只看 `group_id`。
fn admin_payload(found: &admin::Admin) -> Value {
    let mut value = serde_json::to_value(found).unwrap_or(Value::Null);
    if let Some(map) = value.as_object_mut() {
        map.insert("is_super".to_string(), Value::Bool(admin_is_super(found)));
    }
    value
}

/// 登录日志写失败不影响登录本身，只记一条警告。
async fn log_login(pool: &sqlx::MySqlPool, admin_id: u32, ip: &str, status: u8, user_name: &str) {
    if let Err(err) = admin::record_login(pool, admin_id, ip, status, user_name).await {
        tracing::warn!("写后台登录日志失败：{err:#}");
    }
}

/// `POST /api/admin/login`
///
/// 与 Go 版一致的顺序：先看 IP 是否被锁 → 校验参数 → 查账号（不存在则累计 IP 失败数）
/// → 看账号是否被锁 / 被禁用 → 校验密码（失败同时累计账号与 IP）→ 成功时清空计数。
async fn admin_login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let guard = state.login_guard();
    let ip = client_ip(&headers, peer);
    let ip_key = login_guard::LoginGuard::ip_key(&ip);
    if guard.locked(&ip_key) {
        return fail(LOCKED);
    }

    let user_name = req.user_name.trim();
    let password = req.password.trim();
    if user_name.is_empty() || password.is_empty() {
        return fail("请输入用户名和密码");
    }

    let found = match admin::get_by_name(pool, user_name).await {
        Ok(found) => found,
        Err(err) => return failed(err),
    };
    // 用户不存在与密码错误返回同一句，避免暴露账号是否存在
    let Some(found) = found else {
        guard.fail(&ip_key);
        log_login(pool, 0, &ip, 0, user_name).await;
        return fail("用户名或密码错误");
    };

    let user_key = login_guard::LoginGuard::user_key(&found.user_name);
    if guard.locked(&user_key) {
        return fail(LOCKED);
    }
    if found.status != 1 {
        return fail("账号已被禁用");
    }
    if !auth::check_password(password, &found.password) {
        // 账号与 IP 同时计数，避免轮换用户名绕过
        guard.fail(&user_key);
        guard.fail(&ip_key);
        log_login(pool, found.id, &ip, 0, user_name).await;
        return fail("用户名或密码错误");
    }

    guard.reset(&user_key);
    guard.reset(&ip_key);

    let secret = match auth::token_secret(pool).await {
        Ok(secret) => secret,
        Err(err) => return failed(err),
    };
    let (token, expire) = match auth::issue(auth::TokenKind::Admin, &secret, found.id, req.remember)
    {
        Ok(pair) => pair,
        Err(err) => return failed(err),
    };
    if let Err(err) = admin::touch_login(pool, found.id).await {
        tracing::warn!("更新管理员登录时间失败：{err:#}");
    }
    log_login(pool, found.id, &ip, 1, user_name).await;

    ok(json!({
        "token": token,
        "expire": expire,
        "admin": admin_payload(&found),
    }))
}

/// `POST /api/admin/logout`
///
/// 令牌是无状态的，服务端没有可清理的会话，直接回一句成功（与 Go 版同）。
async fn admin_logout() -> Response {
    ok_msg("已退出登录")
}

/// `GET /api/admin/me`
async fn admin_me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match require_admin(&state, &headers).await {
        Ok(found) => ok(admin_payload(&found)),
        Err(res) => *res,
    }
}

/// 管理员详细资料查询参数：`id` 省略（或 0）表示当前登录的管理员。
#[derive(Debug, Deserialize)]
struct AdminDetailQuery {
    #[serde(default)]
    id: u32,
}

/// 新增/更新管理员信息的请求体，字段名与 Go 版 `request.AdminInfoRequest` 一致。
#[derive(Debug, Deserialize)]
struct AdminDetailRequest {
    /// 0=新建，>0=更新该管理员。
    #[serde(default)]
    id: u32,
    #[serde(default)]
    user_name: String,
    /// 明文新密码；空表示不改密码。
    #[serde(default)]
    password: String,
    /// 改密码时的旧密码（非空才校验，与 Go 版一致）。
    #[serde(default)]
    old_password: String,
    #[serde(default)]
    status: u8,
    #[serde(default)]
    group_id: u32,
}

/// 非超级管理员越权操作其他账号时的提示。
///
/// Go 版靠 `middleware.AdminPermission` 按用户组权限节点拦截；本项目还没有权限表，
/// 先用「1 号用户组 = 超管，其余只能管自己」这条更严格的规则兜底。
const NO_ADMIN_PERMISSION: &str = "无权限操作其他管理员";

fn admin_is_super(item: &admin::Admin) -> bool {
    item.group_id == 1
}

/// 从 `Authorization` 头取出当前管理员；未登录 / 令牌失效时返回现成的失败响应。
///
/// 失败响应装箱，避免 `Result` 的 `Err` 分支把返回值撑到 100+ 字节（clippy 警告）。
pub(crate) async fn require_admin(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<admin::Admin, Box<Response>> {
    let Some(pool) = state.db() else {
        return Err(Box::new(fail(NO_DB)));
    };
    let Some(token) = bearer_token(headers) else {
        return Err(Box::new(fail("请先登录")));
    };
    match admin_from_token(pool, &token).await {
        Ok(Some(found)) => Ok(found),
        Ok(None) => Err(Box::new(fail("登录已失效，请重新登录"))),
        Err(err) => Err(Box::new(failed(err))),
    }
}

/// `GET /api/admin/menus`
///
/// 菜单本身是静态配置（不含任何用户数据），但仍要求登录 —— Go 版这条也挂在
/// `ParseAdminToken` + `AdminPermission` 后面。
async fn admin_menus(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match require_admin(&state, &headers).await {
        Ok(_) => ok(menu::default_menu_groups()),
        Err(res) => *res,
    }
}

/// 管理员列表查询参数（Go 版：`current` / `pageSize` / `id` / `group_id` / `user_name`）。
#[derive(Debug, Deserialize)]
struct AdminListQuery {
    #[serde(default)]
    current: i64,
    #[serde(default, rename = "pageSize")]
    page_size: i64,
    #[serde(default)]
    id: u32,
    #[serde(default)]
    group_id: u32,
    #[serde(default)]
    user_name: String,
}

/// `GET /api/admin/list`
///
/// 信封同 Go 版：`total` 在顶层、`data` 是数组。非超管只能看到自己那条记录，
/// 否则会顺着用户名模糊搜索把别人的账号全列出来。
async fn admin_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminListQuery>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let filter = if admin_is_super(&current) {
        admin::AdminFilter {
            id: (query.id > 0).then_some(query.id),
            group_id: (query.group_id > 0).then_some(query.group_id),
            user_name: Some(query.user_name.clone()),
        }
    } else {
        admin::AdminFilter {
            id: Some(current.id),
            ..Default::default()
        }
    };

    match admin::list_page(pool, &filter, query.current, query.page_size).await {
        Ok((total, rows)) => {
            let list: Vec<Value> = rows.iter().map(admin_payload).collect();
            Json(json!({
                "code": 0,
                "msg": "",
                "total": total,
                "data": list,
            }))
            .into_response()
        }
        Err(err) => failed(err),
    }
}

/// `GET /api/admin/detail?id=`
///
/// 不带 `id` 就取当前登录的管理员；`id` 不存在时与 Go 版一样回「用户不存在」。
async fn admin_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminDetailQuery>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let target_id = if query.id == 0 { current.id } else { query.id };
    if !admin_is_super(&current) && target_id != current.id {
        return fail(NO_ADMIN_PERMISSION);
    }

    match admin::get(pool, target_id).await {
        Ok(Some(found)) => ok(admin_payload(&found)),
        Ok(None) => fail("用户不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /api/admin/detail`
///
/// 与 Go 版 `AdminDetailForm` 相同的顺序：定位目标 → 账号非空 → 改密前校验旧密码 → 落库。
/// 有意差异（安全）：改自己的账号时状态强制为启用（同 Go）、新建账号必须给密码、
/// 改用户名时不允许撞上别人的账号（Go 版不查重）。
async fn admin_detail_save(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<AdminDetailRequest>,
) -> Response {
    let ip = client_ip(&headers, peer);
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let target = if req.id > 0 {
        match admin::get(pool, req.id).await {
            Ok(Some(found)) => {
                if !admin_is_super(&current) && found.id != current.id {
                    return fail(NO_ADMIN_PERMISSION);
                }
                Some(found)
            }
            Ok(None) => return fail("管理员不存在"),
            Err(err) => return failed(err),
        }
    } else {
        if !admin_is_super(&current) {
            return fail(NO_ADMIN_PERMISSION);
        }
        None
    };

    let user_name = req.user_name.trim();
    if user_name.is_empty() {
        return fail("账号不能为空");
    }

    // 用户名变了才查重（新建时等于全库查重）
    let renamed = target
        .as_ref()
        .map(|item| item.user_name != user_name)
        .unwrap_or(true);
    if renamed {
        match admin::get_by_name(pool, user_name).await {
            Ok(Some(_)) => return fail("该账号已存在"),
            Ok(None) => {}
            Err(err) => return failed(err),
        }
    }

    // 自己把自己禁用会让当前令牌立刻失效，Go 版也是直接强制启用
    let status = if target.as_ref().map(|item| item.id) == Some(current.id) {
        1
    } else {
        req.status
    };

    let hash = if req.password.trim().is_empty() {
        None
    } else {
        if !req.old_password.is_empty() {
            let matches = target
                .as_ref()
                .map(|item| auth::check_password(&req.old_password, &item.password))
                .unwrap_or(false);
            if !matches {
                return fail("当前密码不正确");
            }
        }
        match auth::hash_password(req.password.trim()) {
            Ok(hash) => Some(hash),
            Err(err) => return failed(err),
        }
    };

    let Some(target) = target else {
        let Some(hash) = hash else {
            return fail("请输入密码");
        };
        return match admin::create(pool, user_name, &hash, req.status, req.group_id).await {
            Ok(id) => {
                log_action(
                    pool,
                    &current,
                    &ip,
                    format!("更新管理员信息：{id} => {user_name}"),
                )
                .await;
                ok(json!({ "id": id }))
            }
            Err(err) => failed(err),
        };
    };

    match admin::update_profile(
        pool,
        target.id,
        user_name,
        status,
        req.group_id,
        hash.as_deref(),
    )
    .await
    {
        Ok(()) => {
            log_action(
                pool,
                &current,
                &ip,
                format!("更新管理员信息：{} => {user_name}", target.id),
            )
            .await;
            ok_msg("管理员信息已更新")
        }
        Err(err) => failed(err),
    }
}

/// 删除管理员的请求体（Go 版复用 `AdminInfoRequest`，这里只关心 `id`）。
#[derive(Debug, Deserialize)]
struct AdminDeleteRequest {
    #[serde(default)]
    id: u32,
}

/// `POST /api/admin/delete`
///
/// 与 Go 版一样不能删自己；另外 `id = 1`（安装时创建的初始超管）也不允许删。
/// 注意 Go 版把这条写成了 `adminId == 1`（当前登录的是 1 号就谁都不能删），
/// 这里按注释里表达的意图实现。
async fn admin_delete(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<AdminDeleteRequest>,
) -> Response {
    let ip = client_ip(&headers, peer);
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    if !admin_is_super(&current) {
        return fail(NO_ADMIN_PERMISSION);
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    if req.id == 0 {
        return fail("管理员不存在");
    }
    if req.id == current.id || req.id == 1 {
        return fail("该管理员不可删除");
    }

    // 先把用户名捞出来，删完就查不到了，而日志要记它
    let target = match admin::get(pool, req.id).await {
        Ok(Some(found)) => found,
        Ok(None) => return fail("管理员不存在"),
        Err(err) => return failed(err),
    };
    match admin::delete(pool, req.id).await {
        Ok(true) => {
            log_action(
                pool,
                &current,
                &ip,
                format!("删除管理员：{} => {}", target.id, target.user_name),
            )
            .await;
            ok_msg("删除成功")
        }
        Ok(false) => fail("管理员不存在"),
        Err(err) => failed(err),
    }
}

/// 用户组查询参数。
#[derive(Debug, Deserialize)]
struct GroupQuery {
    #[serde(default)]
    id: u32,
}

/// 新增/更新用户组的请求体（Go 版 `request.GroupRequest`）。
#[derive(Debug, Deserialize)]
struct GroupRequest {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    /// 权限列表，缺省即「没有任何权限」。
    #[serde(default)]
    setting: Option<admin::GroupSetting>,
}

/// 删除用户组的请求体。
#[derive(Debug, Deserialize)]
struct GroupDeleteRequest {
    #[serde(default)]
    id: u32,
}

/// `GET /api/admin/group/list`
///
/// 用户组决定谁能管后台，只对超管开放。
async fn admin_group_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    if !admin_is_super(&current) {
        return fail(NO_ADMIN_PERMISSION);
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    match admin::list_groups(pool).await {
        Ok(groups) => ok(groups),
        Err(err) => failed(err),
    }
}

/// `GET /api/admin/group/detail?id=`
async fn admin_group_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<GroupQuery>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    if !admin_is_super(&current) {
        return fail(NO_ADMIN_PERMISSION);
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    // Go 版直接把 gorm 的 `record not found` 吐给前端，这里换成中文
    match admin::get_group(pool, query.id).await {
        Ok(Some(group)) => ok(group),
        Ok(None) => fail("用户组不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /api/admin/group/detail`
///
/// `id > 0` 更新、否则新建；名称不能为空（同 Go 版 `GroupNameCannotBeEmpty`）。
async fn admin_group_save(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<GroupRequest>,
) -> Response {
    let ip = client_ip(&headers, peer);
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    if !admin_is_super(&current) {
        return fail(NO_ADMIN_PERMISSION);
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let title = req.title.trim();
    if title.is_empty() {
        return fail("分组名称不能为空");
    }
    if req.id > 0 {
        match admin::get_group(pool, req.id).await {
            Ok(Some(_)) => {}
            Ok(None) => return fail("用户组不存在"),
            Err(err) => return failed(err),
        }
    }

    let setting = req.setting.unwrap_or_default();
    match admin::save_group(pool, req.id, title, req.description.trim(), &setting).await {
        Ok(id) => {
            log_action(
                pool,
                &current,
                &ip,
                format!("更新管理员组信息：{id} => {title}"),
            )
            .await;
            Json(json!({
                "code": 0,
                "msg": "保存成功",
                "data": { "id": id },
            }))
            .into_response()
        }
        Err(err) => failed(err),
    }
}

/// `POST /api/admin/group/delete`
///
/// 1 号用户组内置为超管，删掉会让所有人失去管理入口；组下还有管理员时也不允许删，
/// 否则那些管理员的 `group_id` 会指向一个不存在的组（Go 版没有这层检查）。
async fn admin_group_delete(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<GroupDeleteRequest>,
) -> Response {
    let ip = client_ip(&headers, peer);
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    if !admin_is_super(&current) {
        return fail(NO_ADMIN_PERMISSION);
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    if req.id == 0 {
        return fail("用户组不存在");
    }
    if req.id == 1 {
        return fail("该用户组不可删除");
    }
    match admin::count_in_group(pool, req.id).await {
        Ok(0) => {}
        Ok(_) => return fail("该用户组下还有管理员，无法删除"),
        Err(err) => return failed(err),
    }

    // 删完就查不到标题了，日志要用
    let title = match admin::get_group(pool, req.id).await {
        Ok(Some(group)) => group.title,
        Ok(None) => return fail("用户组不存在"),
        Err(err) => return failed(err),
    };
    match admin::delete_group(pool, req.id).await {
        Ok(true) => {
            log_action(
                pool,
                &current,
                &ip,
                format!("删除管理员组：{} => {title}", req.id),
            )
            .await;
            ok_msg("删除成功")
        }
        Ok(false) => fail("用户组不存在"),
        Err(err) => failed(err),
    }
}

/// 登录日志分页参数（Go 版用 `current` / `pageSize`）。
#[derive(Debug, Deserialize)]
struct LogPageQuery {
    #[serde(default)]
    current: i64,
    #[serde(default, rename = "pageSize")]
    page_size: i64,
}

/// `GET /api/admin/logs/login`
///
/// 与 Go 版相同的信封差异：`total` 在**顶层**而不是 `data` 里，`data` 直接是数组。
/// 超管看全部，其他管理员只看自己的记录。
async fn admin_login_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<LogPageQuery>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    // 页码 / 每页条数在 model 里兜底（<1 回 1 / 20，上限 100）
    let scope = if admin_is_super(&current) {
        None
    } else {
        Some(current.id)
    };
    match admin::list_login_logs(pool, scope, query.current, query.page_size).await {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// 写一条后台操作日志；失败只记警告，不影响业务响应（与 Go 版 `AddAdminLog` 一致）。
async fn log_action(
    pool: &sqlx::MySqlPool,
    admin: &admin::Admin,
    ip: &str,
    log: impl Into<String>,
) {
    let log = log.into();
    if let Err(err) = admin::record_action(pool, admin.id, &admin.user_name, ip, &log).await {
        tracing::warn!("写后台操作日志失败：{err:#}");
    }
}

/// `GET /api/admin/logs/action`
///
/// 信封与登录日志相同：`total` 在顶层、`data` 是数组。超管看全部，其他管理员只看自己的。
async fn admin_action_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<LogPageQuery>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let scope = if admin_is_super(&current) {
        None
    } else {
        Some(current.id)
    };
    match admin::list_actions(pool, scope, query.current, query.page_size).await {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// `POST /api/user/login`
async fn user_login(State(state): State<AppState>, Json(req): Json<LoginRequest>) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let user_name = req.user_name.trim();
    if user_name.is_empty() || req.password.is_empty() {
        return fail("请输入用户名和密码");
    }

    let found = match user::get_by_name(pool, user_name).await {
        Ok(found) => found,
        Err(err) => return failed(err),
    };
    let Some(found) = found else {
        return fail("用户名或密码错误");
    };
    if !found.status {
        return fail("账号已被禁用");
    }
    if !auth::check_password(&req.password, &found.password) {
        return fail("用户名或密码错误");
    }

    let secret = match auth::token_secret(pool).await {
        Ok(secret) => secret,
        Err(err) => return failed(err),
    };
    let (token, expire) = match auth::issue(auth::TokenKind::User, &secret, found.id, req.remember)
    {
        Ok(pair) => pair,
        Err(err) => return failed(err),
    };
    if let Err(err) = user::touch_login(pool, found.id).await {
        tracing::warn!("更新会员登录时间失败：{err:#}");
    }

    ok(json!({
        "token": token,
        "expire": expire,
        "user": { "id": found.id, "user_name": found.user_name, "avatar_url": found.avatar_url },
    }))
}

/// `GET /api/user/me`
async fn user_me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(token) = bearer_token(&headers) else {
        return fail("请先登录");
    };

    let secret = match auth::token_secret(pool).await {
        Ok(secret) => secret,
        Err(err) => return failed(err),
    };
    let Ok(claims) = auth::verify(auth::TokenKind::User, &secret, &token) else {
        return fail("登录已失效，请重新登录");
    };
    let Some(id) = claims.subject(auth::TokenKind::User) else {
        return fail("登录已失效，请重新登录");
    };
    match user::get(pool, id).await {
        Ok(Some(found)) => ok(found),
        Ok(None) => fail("账号不存在"),
        Err(err) => failed(err),
    }
}

/// 成功响应信封（`code: 0` + `data`）。
pub(crate) fn ok(data: impl Serialize) -> Response {
    Json(json!({ "code": 0, "msg": "", "data": data })).into_response()
}

/// 只有提示语、没有数据的成功响应（如退出登录）。
pub(crate) fn ok_msg(msg: impl Into<String>) -> Response {
    Json(json!({ "code": 0, "msg": msg.into(), "data": Value::Null })).into_response()
}

/// 业务失败（`code: 1`，前端可直接展示 `msg`）。
pub(crate) fn fail(msg: impl Into<String>) -> Response {
    Json(json!({ "code": 1, "msg": msg.into(), "data": Value::Null })).into_response()
}

/// 系统错误（记录日志后按失败返回）。
pub(crate) fn failed(err: anyhow::Error) -> Response {
    tracing::error!("{err:#}");
    fail(format!("{err:#}"))
}

/// 数据库未配置时的统一提示。
pub(crate) const NO_DB: &str = "数据库未配置";

/// `GET /api/health`
async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let env = state.with_config(|c| c.server.env.clone());
    Json(json!({
        "code": 0,
        "msg": "ok",
        "data": {
            "version": state.version(),
            "env": env,
            "uptime": state.uptime_secs(),
            "db": state.has_db(),
        }
    }))
}

#[derive(Debug, Deserialize, Default)]
struct CategoryQuery {
    /// 传 1 返回树形结构
    tree: Option<u8>,
    module_id: Option<u32>,
}

/// `GET /api/categories`
async fn category_list(
    State(state): State<AppState>,
    Query(query): Query<CategoryQuery>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let list = match query.module_id {
        Some(module_id) => category::list_by_module(pool, module_id).await,
        None => category::list_all(pool).await,
    };
    match list {
        Ok(list) if query.tree.unwrap_or(0) == 1 => ok(category::tree(&list)),
        Ok(list) => ok(list),
        Err(err) => failed(err),
    }
}

/// `GET /api/categories/{id}`
async fn category_detail(State(state): State<AppState>, Path(id): Path<u32>) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match category::get(pool, id).await {
        Ok(Some(item)) => ok(item),
        Ok(None) => fail("分类不存在"),
        Err(err) => failed(err),
    }
}

/// `GET /api/modules`
async fn module_list(State(state): State<AppState>) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match module::list(pool).await {
        Ok(list) => ok(list),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct ArchiveListQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    category_id: Option<u32>,
    module_id: Option<u32>,
    keyword: Option<String>,
    /// 排序白名单，见 `archive::Order::parse`
    order: Option<String>,
}

/// `GET /api/archives`
async fn archive_list(
    State(state): State<AppState>,
    Query(params): Query<ArchiveListQuery>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let mut query = archive::ArchiveQuery::new();
    if let Some(module_id) = params.module_id {
        query.module_id = Some(module_id);
    }
    if let Some(category_id) = params.category_id {
        let categories = match category::list_all(pool).await {
            Ok(list) => list,
            Err(err) => return failed(err),
        };
        query.category_ids = category::self_and_sub_ids(&categories, category_id);
    }
    if let Some(keyword) = params.keyword.as_deref() {
        query.keyword = Some(keyword.trim().to_string()).filter(|k| !k.is_empty());
    }

    let page = params.page.unwrap_or(1);
    let page_size = params.page_size.unwrap_or(20);
    let order = archive::Order::parse(params.order.as_deref().unwrap_or_default());

    match archive::list(pool, &query, order, page, page_size).await {
        Ok(result) => ok(result),
        Err(err) => failed(err),
    }
}

/// `GET /api/archives/{id}`
async fn archive_detail(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match archive::detail(pool, id).await {
        Ok(Some((item, content))) => {
            let mut value = match serde_json::to_value(&item) {
                Ok(value) => value,
                Err(err) => return failed(err.into()),
            };
            // 模块扩展字段（模块表里的动态列）
            let mut extra_fields = serde_json::Map::new();
            if let Ok(Some(model)) = module::get(pool, item.module_id).await {
                if let Ok(values) = extra::load(pool, &model, item.id).await {
                    extra_fields = values;
                }
            }
            if let Value::Object(map) = &mut value {
                map.insert("content".to_string(), Value::String(content));
                map.insert("extra".to_string(), Value::Object(extra_fields));
            }
            ok(value)
        }
        Ok(None) => fail("内容不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /api/archives`（需要 `X-Api-Token`）
async fn archive_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ArchiveInput>,
) -> Response {
    if let Some(deny) = deny_write(&state, &headers).await {
        return deny;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match archive_write::create(pool, &input).await {
        Ok(saved) => ok(saved),
        Err(err) => model_error(err),
    }
}

/// `PUT /api/archives/{id}`（需要 `X-Api-Token`）
async fn archive_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<ArchiveInput>,
) -> Response {
    if let Some(deny) = deny_write(&state, &headers).await {
        return deny;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match archive_write::update(pool, id, &input).await {
        Ok(saved) => ok(saved),
        Err(err) => model_error(err),
    }
}

/// `DELETE /api/archives/{id}`（需要 `X-Api-Token`）
async fn archive_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    if let Some(deny) = deny_write(&state, &headers).await {
        return deny;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match archive_write::delete(pool, id).await {
        Ok(true) => ok(json!({ "deleted": true, "id": id })),
        Ok(false) => fail("内容不存在"),
        Err(err) => model_error(err),
    }
}

// ============================================================================
// 后台接口（Go 版 `/system/api/...`，对应 `controller/manageController`）
// ============================================================================

/// 后台接口路由（Part 1：内容体系 / 分类 / 模型 / 版本 / 统计）。
fn manage_router() -> Router<AppState> {
    Router::new()
        // Go 版 `route/manage.go`：`manage.Post("/login")` 与
        // `manage.Party("/admin", middleware.ParseAdminToken, middleware.AdminPermission)`，
        // 即后台管理员端点的规范前缀是 `/system/api/...`。
        // `api_router()` 里那套挂在公开 `/api` 前缀下，这里把 Go 的规范路径补齐
        // （同一批 handler、同一套鉴权与响应格式）。
        .route("/login", post(admin_login))
        .route("/admin/login", post(admin_login))
        .route("/admin/logout", post(admin_logout))
        .route("/admin/me", get(admin_me))
        .route("/admin/menus", get(admin_menus))
        .route("/admin/list", get(admin_list))
        .route("/admin/detail", get(admin_detail).post(admin_detail_save))
        .route("/admin/delete", post(admin_delete))
        .route("/admin/group/list", get(admin_group_list))
        .route(
            "/admin/group/detail",
            get(admin_group_detail).post(admin_group_save),
        )
        .route("/admin/group/delete", post(admin_group_delete))
        .route("/admin/logs/login", get(admin_login_logs))
        .route("/admin/logs/action", get(admin_action_logs))
        .route("/version/info", get(manage_version_info))
        .route("/statistic/summary", get(manage_statistic_summary))
        .route("/statistic/dashboard", get(manage_statistic_dashboard))
        .route("/statistic/spider", get(manage_statistic_spider))
        .route("/statistic/traffic", get(manage_statistic_traffic))
        .route("/statistic/dates", get(manage_statistic_dates))
        .route("/statistic/detail", get(manage_statistic_detail))
        .route("/statistic/include", get(manage_statistic_include))
        .route(
            "/statistic/include/detail",
            get(manage_statistic_include_detail),
        )
        .route("/captcha", get(manage_captcha))
        .route("/module/list", get(manage_module_list))
        .route(
            "/module/detail",
            get(manage_module_detail).post(manage_module_save),
        )
        .route("/module/delete", post(manage_module_delete))
        .route("/module/field/delete", post(manage_module_field_delete))
        .route("/category/list", get(manage_category_list))
        .route(
            "/category/detail",
            get(manage_category_detail).post(manage_category_save),
        )
        .route("/category/delete", post(manage_category_delete))
        .route("/category/count", post(manage_category_count))
        .route("/setting/nav", get(manage_nav_list).post(manage_nav_save))
        .route("/setting/nav/delete", post(manage_nav_delete))
        .route(
            "/setting/nav/type",
            get(manage_nav_type_list).post(manage_nav_type_save),
        )
        .route("/setting/nav/type/delete", post(manage_nav_type_delete))
        .route("/attachment/list", get(manage_attachment_list))
        .route("/attachment/detail", get(manage_attachment_detail))
        .route("/attachment/upload", post(manage_attachment_upload))
        .route("/attachment/delete", post(manage_attachment_delete))
        .route("/attachment/edit", post(manage_attachment_edit))
        .route("/attachment/category", post(manage_attachment_category))
        .route("/attachment/scan", post(manage_attachment_scan))
        .route("/archive/list", get(manage_archive_list))
        .route(
            "/archive/detail",
            get(manage_archive_detail).post(manage_archive_save),
        )
        .route("/archive/delete", post(manage_archive_delete))
        .route("/archive/recover", post(manage_archive_recover))
        .route("/archive/status", post(manage_archive_status))
        .route("/archive/recommend", post(manage_archive_recommend))
        .route("/archive/sort", post(manage_archive_sort))
        .route("/archive/parent", post(manage_archive_parent))
        .route("/archive/category", post(manage_archive_category))
        .route("/archive/time", post(manage_archive_time))
        .route("/diy/types", get(manage_diy_types))
        .route("/diy/page", get(manage_diy_page))
        .route("/diy/save", post(manage_diy_save))
        .route("/diy/reset", post(manage_diy_reset))
        .route(
            "/collector/setting",
            get(manage_collector_setting).post(manage_collector_setting_save),
        )
        .route("/collector/article/replace", post(manage_collector_replace))
        .route("/collector/article/collect", post(manage_collector_collect))
        .route("/collector/article/start", post(manage_collector_start))
        .route(
            "/collector/article/combination/get",
            post(manage_collector_combination),
        )
        .route("/collector/keyword/dig", post(manage_collector_dig))
        .route("/anqi/skill/list", get(manage_skill_list))
        .route("/anqi/skill/detail", get(manage_skill_detail))
        .route("/anqi/skill/edit", post(manage_skill_edit))
        .route("/anqi/skill/delete", post(manage_skill_delete))
        .route("/anqi/skill/reload", post(manage_skill_reload))
        .route("/anqi/info", get(manage_anqi_info))
        .route("/anqi/check", get(manage_anqi_check))
        .route("/anqi/login", post(manage_anqi_login))
        .route("/anqi/upload", post(manage_anqi_upload))
        .route("/anqi/template/share", post(manage_anqi_template_share))
        .route("/anqi/template/download", post(manage_anqi_template_download))
        .route("/anqi/feedback", post(manage_anqi_feedback))
        .route("/anqi/translate", post(manage_anqi_translate))
        .route("/anqi/ai/pseudo", post(manage_anqi_ai_pseudo))
        .route("/anqi/ai/stream", post(manage_anqi_ai_stream))
        .route("/anqi/ai/stream/data", get(manage_anqi_ai_stream_data))
        .route("/anqi/ai/chat", post(manage_ai_chat))
        .route("/anqi/ai/chat/confirm", post(manage_ai_chat_confirm))
        .route("/anqi/ai/upload", post(manage_ai_chat_upload))
        .route(
            "/anqi/ai/settings",
            get(manage_ai_settings).post(manage_ai_settings_save),
        )
        .route("/anqi/ai/history", get(manage_ai_history))
        .route("/anqi/ai/sessions", get(manage_ai_sessions))
        .route("/anqi/ai/agents", get(manage_ai_agents))
        .route("/anqi/ai/agents/:id/logs", get(manage_ai_agent_logs))
        .route("/anqi/ai/agents/:id/chat", post(manage_ai_agent_chat))
        .route("/anqi/ai/health", get(manage_ai_health))
        .route("/anqi/ai/chat/trace", get(manage_ai_chat_trace))
        .route("/anqi/ai/image", post(manage_ai_image))
        .route("/anqi/ai/image/confirm", post(manage_ai_image_confirm))
        .route("/anqi/ai/image/histories", get(manage_ai_image_histories))
        .route("/anqi/extract/keywords", post(manage_anqi_extract_keywords))
        .route("/anqi/extract/description", post(manage_anqi_extract_description))
        .route("/anqi/restart", post(manage_anqi_restart))
        .route(
            "/setting/system",
            get(manage_setting_system).post(manage_setting_system_save),
        )
        .route(
            "/setting/content",
            get(manage_setting_content).post(manage_setting_content_save),
        )
        .route("/setting/thumb/rebuild", post(manage_setting_thumb_rebuild))
        .route(
            "/setting/index",
            get(manage_setting_index).post(manage_setting_index_save),
        )
        .route(
            "/setting/contact",
            get(manage_setting_contact).post(manage_setting_contact_save),
        )
        .route(
            "/setting/cache",
            get(manage_setting_cache).post(manage_setting_cache_form),
        )
        .route(
            "/setting/safe",
            get(manage_setting_safe).post(manage_setting_safe_save),
        )
        .route(
            "/setting/diyfield",
            get(manage_setting_diyfield).post(manage_setting_diyfield_save),
        )
        .route("/setting/favicon", post(manage_setting_favicon_save))
        .route(
            "/setting/favicon/delete",
            post(manage_setting_favicon_delete),
        )
        .route(
            "/setting/banner",
            get(manage_setting_banner).post(manage_setting_banner_save),
        )
        .route(
            "/setting/banner/delete",
            post(manage_setting_banner_delete),
        )
        .route("/setting/migratedb", post(manage_setting_migrate_db))
        .route(
            "/setting/ai",
            get(manage_setting_ai).post(manage_setting_ai_save),
        )
        .route("/plugin/userauth/list", get(manage_user_auth_list))
        .route("/plugin/userauth/detail", get(manage_user_auth_detail))
        .route("/plugin/userauth/audit", post(manage_user_auth_audit))
        .route("/design/list", get(manage_design_list))
        .route("/design/info", get(manage_design_info))
        .route("/design/save", post(manage_design_save))
        .route("/design/use", post(manage_design_use))
        .route("/design/delete", post(manage_design_delete))
        .route("/design/download", post(manage_design_download))
        .route("/design/upload/check", get(manage_design_upload_check))
        .route("/design/upload", post(manage_design_upload))
        .route("/design/data/backup", post(manage_design_data_backup))
        .route("/design/data/restore", post(manage_design_data_restore))
        .route("/design/file/info", get(manage_design_file_info))
        .route("/design/file/histories", get(manage_design_file_histories))
        .route("/design/file/history/info", get(manage_design_file_history_info))
        .route(
            "/design/file/history/delete",
            post(manage_design_file_history_delete),
        )
        .route("/design/file/restore", post(manage_design_file_restore))
        .route("/design/file/save", post(manage_design_file_save))
        .route("/design/file/copy", post(manage_design_file_copy))
        .route("/design/file/delete", post(manage_design_file_delete))
        .route("/design/file/upload", post(manage_design_file_upload))
        .route("/design/file/templates", get(manage_design_file_templates))
        .route("/design/docs", get(manage_design_docs))
        .route("/design/helpers", get(manage_design_helpers))
        .route("/version/env", get(manage_version_env))
        .route("/version/check", get(manage_version_check))
        .route("/version/upgrade", post(manage_version_upgrade))
        .route(
            "/version/upload-upgrade",
            post(manage_version_upload_upgrade),
        )
        .route("/siteinfo", get(manage_current_site_info))
        .route("/website/list", get(manage_website_list))
        .route("/website/info", get(manage_website_info))
        .route("/website/save", post(manage_website_save))
        .route("/website/delete", post(manage_website_delete))
        .route("/website/login", post(manage_website_login))
        .route(
            "/plugin/rewrite",
            get(manage_plugin_rewrite).post(manage_plugin_rewrite_save),
        )
        .route(
            "/plugin/robots",
            get(manage_plugin_robots).post(manage_plugin_robots_save),
        )
        .route(
            "/plugin/sitemap",
            get(manage_plugin_sitemap).post(manage_plugin_sitemap_save),
        )
        .route("/plugin/sitemap/build", post(manage_plugin_sitemap_build))
        .route("/plugin/link/list", get(manage_link_list))
        .route("/plugin/link/detail", post(manage_link_save))
        .route("/plugin/link/delete", post(manage_link_delete))
        .route("/plugin/link/check", post(manage_link_check))
        .route(
            "/plugin/keyword/setting",
            get(manage_keyword_setting).post(manage_keyword_setting_save),
        )
        .route("/plugin/keyword/list", get(manage_keyword_list))
        .route("/plugin/keyword/detail", post(manage_keyword_save))
        .route("/plugin/keyword/delete", post(manage_keyword_delete))
        .route("/plugin/keyword/export", post(manage_keyword_export))
        .route("/plugin/keyword/import", post(manage_keyword_import))
        .route("/plugin/tag/list", get(manage_tag_list))
        .route(
            "/plugin/tag/detail",
            get(manage_tag_detail).post(manage_tag_save),
        )
        .route("/plugin/tag/delete", post(manage_tag_delete))
        .route(
            "/plugin/tag/fields",
            get(manage_tag_fields).post(manage_tag_fields_save),
        )
        .route("/plugin/redirect/list", get(manage_redirect_list))
        .route("/plugin/redirect/detail", post(manage_redirect_save))
        .route("/plugin/redirect/delete", post(manage_redirect_delete))
        .route("/plugin/redirect/import", post(manage_redirect_import))
        .route("/plugin/anchor/list", get(manage_anchor_list))
        .route(
            "/plugin/anchor/detail",
            get(manage_anchor_detail).post(manage_anchor_save),
        )
        .route("/plugin/anchor/delete", post(manage_anchor_delete))
        .route("/plugin/anchor/replace", post(manage_anchor_replace))
        .route("/plugin/anchor/export", post(manage_anchor_export))
        .route("/plugin/anchor/import", post(manage_anchor_import))
        .route(
            "/plugin/anchor/setting",
            get(manage_anchor_setting).post(manage_anchor_setting_save),
        )
        .route(
            "/plugin/anchor/add-from-title",
            post(manage_anchor_add_from_title),
        )
        .route("/plugin/replace/values", post(manage_replace_values))
        .route(
            "/plugin/timefactor/setting",
            get(manage_timefactor_setting).post(manage_timefactor_setting_save),
        )
        .route(
            "/plugin/jsonld/config",
            get(manage_jsonld_config).post(manage_jsonld_config_save),
        )
        .route(
            "/plugin/google/setting",
            get(manage_google_setting).post(manage_google_setting_save),
        )
        .route("/plugin/transfer/task", get(manage_transfer_task))
        .route("/plugin/transfer/download", post(manage_transfer_download))
        .route("/plugin/transfer/create", post(manage_transfer_create))
        .route("/plugin/transfer/modules", post(manage_transfer_modules))
        .route("/plugin/transfer/start", post(manage_transfer_start))
        .route(
            "/plugin/multilang/config",
            get(manage_multilang_config).post(manage_multilang_config_save),
        )
        .route("/plugin/multilang/sites", get(manage_multilang_sites))
        .route(
            "/plugin/multilang/validsites",
            get(manage_multilang_valid_sites),
        )
        .route(
            "/plugin/multilang/site/save",
            post(manage_multilang_site_save),
        )
        .route(
            "/plugin/multilang/site/remove",
            post(manage_multilang_site_remove),
        )
        .route(
            "/plugin/multilang/site/sync",
            post(manage_multilang_site_sync),
        )
        .route(
            "/plugin/multilang/site/sync/status",
            get(manage_multilang_sync_status),
        )
        .route(
            "/plugin/multilang/site/html/logs",
            get(manage_multilang_html_logs),
        )
        .route(
            "/plugin/multilang/site/html/caches",
            get(manage_multilang_html_caches),
        )
        .route(
            "/plugin/multilang/site/html/cache/remove",
            post(manage_multilang_html_cache_remove),
        )
        .route(
            "/plugin/translate/config",
            get(manage_translate_config).post(manage_translate_config_save),
        )
        .route("/plugin/translate/logs", get(manage_translate_logs))
        .route("/plugin/translate/log/texts", get(manage_translate_text_logs))
        .route(
            "/plugin/translate/log/text/remove",
            post(manage_translate_text_remove),
        )
        .route(
            "/plugin/translate/log/text/save",
            post(manage_translate_text_save),
        )
        .route("/plugin/llms/status", get(manage_llms_status))
        .route("/plugin/llms/build", post(manage_llms_build))
        .route(
            "/plugin/llms/setting",
            get(manage_llms_setting).post(manage_llms_setting_save),
        )
        .route(
            "/plugin/titleimage/config",
            get(manage_titleimage_config).post(manage_titleimage_config_save),
        )
        .route("/plugin/titleimage/preview", get(manage_titleimage_preview))
        .route("/plugin/titleimage/upload", post(manage_titleimage_upload))
        .route(
            "/plugin/titleimage/generate",
            post(manage_titleimage_generate),
        )
        .route(
            "/setting/sensitive/words",
            get(manage_sensitive_words).post(manage_sensitive_words_save),
        )
        .route("/setting/sensitive/check", post(manage_sensitive_check))
        .route("/setting/sensitive/sync", post(manage_sensitive_sync))
        .route("/plugin/comment/list", get(manage_comment_list))
        .route(
            "/plugin/comment/detail",
            get(manage_comment_detail).post(manage_comment_save),
        )
        .route("/plugin/comment/delete", post(manage_comment_delete))
        .route("/plugin/comment/check", post(manage_comment_check))
        .route("/plugin/guestbook/list", get(manage_guestbook_list))
        .route("/plugin/guestbook/detail", get(manage_guestbook_detail))
        .route("/plugin/guestbook/status", post(manage_guestbook_status))
        .route("/plugin/guestbook/delete", post(manage_guestbook_delete))
        .route("/plugin/guestbook/export", post(manage_guestbook_export))
        .route(
            "/plugin/guestbook/setting",
            get(manage_guestbook_setting).post(manage_guestbook_setting_save),
        )
        .route("/plugin/subscriber/list", get(manage_subscriber_list))
        .route("/plugin/subscriber/detail", get(manage_subscriber_detail))
        .route("/plugin/subscriber/save", post(manage_subscriber_save))
        .route("/plugin/subscriber/delete", post(manage_subscriber_delete))
        .route("/plugin/subscriber/send", post(manage_subscriber_send))
        .route(
            "/plugin/subscriber/send/status",
            get(manage_subscriber_send_status),
        )
        .route(
            "/plugin/subscriber/category/list",
            get(manage_subscriber_category_list),
        )
        .route(
            "/plugin/subscriber/category/save",
            post(manage_subscriber_category_save),
        )
        .route(
            "/plugin/subscriber/category/delete",
            post(manage_subscriber_category_delete),
        )
        .route("/plugin/fileupload/list", get(manage_fileupload_list))
        .route("/plugin/fileupload/upload", post(manage_fileupload_upload))
        .route("/plugin/fileupload/delete", post(manage_fileupload_delete))
        .route(
            "/plugin/user/fields",
            get(manage_user_fields).post(manage_user_fields_save),
        )
        .route("/plugin/user/field/delete", post(manage_user_field_delete))
        .route("/plugin/user/list", get(manage_user_list))
        .route(
            "/plugin/user/detail",
            get(manage_user_detail).post(manage_user_save),
        )
        .route("/plugin/user/balance", post(manage_user_balance))
        .route("/plugin/user/delete", post(manage_user_delete))
        .route("/plugin/user/group/list", get(manage_user_group_list))
        .route(
            "/plugin/user/group/detail",
            get(manage_user_group_detail).post(manage_user_group_save),
        )
        .route(
            "/plugin/user/group/delete",
            post(manage_user_group_delete),
        )
        .route("/plugin/material/list", get(manage_material_list))
        .route(
            "/plugin/material/detail",
            get(manage_material_detail).post(manage_material_save),
        )
        .route("/plugin/material/import", post(manage_material_import))
        .route("/plugin/material/delete", post(manage_material_delete))
        .route(
            "/plugin/material/category/list",
            get(manage_material_category_list),
        )
        .route(
            "/plugin/material/category/detail",
            post(manage_material_category_save),
        )
        .route(
            "/plugin/material/category/delete",
            post(manage_material_category_delete),
        )
        .route(
            "/plugin/material/convert/file",
            post(manage_material_convert_file),
        )
        .route(
            "/plugin/watermark/config",
            get(manage_watermark_config).post(manage_watermark_config_save),
        )
        .route("/plugin/watermark/preview", get(manage_watermark_preview))
        .route("/plugin/watermark/upload", post(manage_watermark_upload))
        .route("/plugin/watermark/generate", post(manage_watermark_generate))
        .route(
            "/plugin/interference/config",
            get(manage_interference_config).post(manage_interference_config_save),
        )
        .route(
            "/plugin/limiter/setting",
            get(manage_limiter_setting).post(manage_limiter_setting_save),
        )
        .route("/plugin/limiter/blockedips", get(manage_limiter_blocked))
        .route(
            "/plugin/limiter/blockedip/remove",
            post(manage_limiter_unblock),
        )
        .route(
            "/plugin/akismet/setting",
            get(manage_akismet_setting).post(manage_akismet_setting_save),
        )
        .route(
            "/plugin/fulltext/config",
            get(manage_fulltext_config).post(manage_fulltext_config_save),
        )
        .route("/plugin/fulltext/rebuild", post(manage_fulltext_rebuild))
        .route("/plugin/fulltext/status", get(manage_fulltext_status))
        .route("/plugin/import/api", get(manage_import_api))
        .route("/plugin/import/token", post(manage_import_token))
        .route(
            "/plugin/htmlcache/config",
            get(manage_htmlcache_config).post(manage_htmlcache_config_save),
        )
        .route("/plugin/htmlcache/build", post(manage_htmlcache_build))
        .route(
            "/plugin/htmlcache/build/index",
            post(manage_htmlcache_build_part),
        )
        .route(
            "/plugin/htmlcache/build/category",
            post(manage_htmlcache_build_part),
        )
        .route(
            "/plugin/htmlcache/build/archive",
            post(manage_htmlcache_build_part),
        )
        .route(
            "/plugin/htmlcache/build/tag",
            post(manage_htmlcache_build_part),
        )
        .route(
            "/plugin/htmlcache/build/status",
            get(manage_htmlcache_build_status),
        )
        .route("/plugin/htmlcache/clean", post(manage_htmlcache_clean))
        .route(
            "/plugin/htmlcache/upload/file",
            post(manage_htmlcache_upload),
        )
        .route("/plugin/htmlcache/push", post(manage_htmlcache_push))
        .route(
            "/plugin/htmlcache/push/status",
            get(manage_htmlcache_push_status),
        )
        .route(
            "/plugin/htmlcache/push/logs",
            get(manage_htmlcache_push_logs),
        )
        .route(
            "/aigenerate/setting",
            get(manage_aigenerate_setting).post(manage_aigenerate_setting_save),
        )
        .route("/aigenerate/checkapi", get(manage_aigenerate_checkapi))
        .route("/aigenerate/plans", get(manage_aigenerate_plans))
        .route(
            "/aigenerate/article/collect",
            post(manage_aigenerate_article_collect),
        )
        .route(
            "/aigenerate/article/start",
            post(manage_aigenerate_article_start),
        )
        .route(
            "/plugin/place/setting",
            get(manage_place_setting).post(manage_place_setting_save),
        )
        .route("/plugin/place/list", get(manage_place_list))
        .route(
            "/plugin/place/detail",
            get(manage_place_detail).post(manage_place_save),
        )
        .route("/plugin/place/delete", post(manage_place_delete))
        .route("/plugin/push", get(manage_push).post(manage_push_save))
        .route("/plugin/push/push", post(manage_push_urls))
        .route("/plugin/push/logs", get(manage_push_logs))
        .route("/plugin/storage", get(manage_storage).post(manage_storage_save))
        .route("/plugin/storage/upload", post(manage_storage_upload))
        .route(
            "/plugin/weapp/config",
            get(manage_weapp_config).post(manage_weapp_config_save),
        )
        .route(
            "/plugin/wechat/config",
            get(manage_wechat_config).post(manage_wechat_config_save),
        )
        .route(
            "/plugin/wechat/message/list",
            get(manage_wechat_message_list),
        )
        .route(
            "/plugin/wechat/message/delete",
            post(manage_wechat_message_delete),
        )
        .route(
            "/plugin/wechat/message/reply",
            post(manage_wechat_message_reply),
        )
        .route(
            "/plugin/wechat/reply/rule/list",
            get(manage_wechat_rule_list),
        )
        .route(
            "/plugin/wechat/reply/rule/delete",
            post(manage_wechat_rule_delete),
        )
        .route(
            "/plugin/wechat/reply/rule/save",
            post(manage_wechat_rule_save),
        )
        .route("/plugin/wechat/menu/list", get(manage_wechat_menu_list))
        .route("/plugin/wechat/menu/save", post(manage_wechat_menu_save))
        .route("/plugin/wechat/menu/delete", post(manage_wechat_menu_delete))
        .route("/plugin/wechat/menu/sync", post(manage_wechat_menu_sync))
        .route(
            "/plugin/order/config",
            get(manage_order_config).post(manage_order_config_save),
        )
        .route("/plugin/order/list", get(manage_order_list))
        .route("/plugin/order/detail", get(manage_order_detail))
        .route("/plugin/order/pay", post(manage_order_pay))
        .route("/plugin/order/deliver", post(manage_order_deliver))
        .route("/plugin/order/finished", post(manage_order_finished))
        .route("/plugin/order/canceled", post(manage_order_canceled))
        .route("/plugin/order/refund", post(manage_order_refund))
        .route("/plugin/order/refund/apply", post(manage_order_refund_apply))
        .route("/plugin/order/export", post(manage_order_export))
        .route("/plugin/finance/list", get(manage_finance_list))
        .route("/plugin/finance/detail", get(manage_finance_detail))
        .route("/plugin/retailer/list", get(manage_retailer_list))
        .route("/plugin/retailer/detail", get(manage_user_detail))
        .route(
            "/plugin/retailer/config",
            get(manage_retailer_config).post(manage_retailer_config_save),
        )
        .route("/plugin/retailer/realname", post(manage_retailer_realname))
        .route("/plugin/retailer/apply", post(manage_retailer_apply))
        .route("/plugin/commission/list", get(manage_commission_list))
        .route("/plugin/commission/detail", get(manage_commission_detail))
        .route("/plugin/pay/accounts", get(manage_pay_accounts))
        .route("/plugin/pay/detail", get(manage_pay_detail).post(manage_pay_save))
        .route("/plugin/pay/delete", post(manage_pay_delete))
        .route("/plugin/pay/upload", post(manage_pay_upload))
        .route("/plugin/pay/statistic", get(manage_pay_statistic))
        .route("/plugin/withdraw/list", get(manage_withdraw_list))
        .route("/plugin/withdraw/detail", get(manage_withdraw_detail))
        .route("/plugin/withdraw/approval", post(manage_withdraw_approval))
        .route("/plugin/withdraw/finished", post(manage_withdraw_finished))
        .route("/plugin/withdraw/apply", post(manage_withdraw_apply))
        .route("/plugin/sendmail/list", get(manage_sendmail_list))
        .route(
            "/plugin/sendmail/setting",
            get(manage_sendmail_setting).post(manage_sendmail_setting_save),
        )
        .route("/plugin/sendmail/templates", get(manage_sendmail_templates))
        .route(
            "/plugin/sendmail/template",
            get(manage_sendmail_template).post(manage_sendmail_template_save),
        )
        .route(
            "/plugin/sendmail/template/preview",
            post(manage_sendmail_template_preview),
        )
        .route("/plugin/sendmail/test", post(manage_sendmail_test))
        .route("/plugin/backup/list", get(manage_backup_list))
        .route("/plugin/backup/dump", post(manage_backup_dump))
        .route("/plugin/backup/status", get(manage_backup_status))
        .route("/plugin/backup/restore", post(manage_backup_restore))
        .route("/plugin/backup/delete", post(manage_backup_delete))
        .route("/plugin/backup/export", get(manage_backup_export))
        .route("/plugin/backup/import", post(manage_backup_import))
        .route("/plugin/backup/cleanup", post(manage_backup_cleanup))
        .route("/plugin/backup/remark", post(manage_backup_remark))
        .fallback(api_not_found)
}

/// `GET /system/api/version/info`
async fn manage_version_info(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match require_admin(&state, &headers).await {
        Ok(_) => ok(statistic::version_info()),
        Err(res) => *res,
    }
}

#[derive(Debug, Deserialize, Default)]
struct ExactQuery {
    #[serde(default)]
    exact: Option<String>,
}

fn query_bool(value: Option<&str>) -> bool {
    matches!(
        value.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

/// `GET /system/api/statistic/summary?exact=true`
async fn manage_statistic_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ExactQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match statistic::summary(pool, query_bool(query.exact.as_deref())).await {
        Ok(data) => ok(data),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/statistic/dashboard`
async fn manage_statistic_dashboard(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match statistic::dashboard(pool).await {
        Ok(data) => ok(data),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/statistic/spider`
async fn manage_statistic_spider(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match statistic::chart_spider(pool).await {
        Ok(data) => ok(data),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/statistic/traffic`
async fn manage_statistic_traffic(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match statistic::chart_traffic(pool).await {
        Ok(data) => ok(data),
        Err(err) => failed(err),
    }
}

/// 统计日志目录：`<data>/statistic`。
fn statistic_dir(state: &AppState) -> std::path::PathBuf {
    state.with_config(|config| config.data_dir()).join("statistic")
}

/// `GET /system/api/statistic/dates`
async fn manage_statistic_dates(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    ok(statistic::log_dates(&statistic_dir(&state)))
}

#[derive(Debug, Deserialize, Default)]
struct StatisticDetailQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

/// `GET /system/api/statistic/detail`
async fn manage_statistic_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<StatisticDetailQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let dir = statistic_dir(&state);
    let (list, total) = statistic::read_detail(
        &dir,
        query.date.as_deref().unwrap_or(""),
        query.kind.as_deref().unwrap_or(""),
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    );
    Json(json!({
        "code": 0,
        "msg": "",
        "total": total,
        "data": list,
    }))
    .into_response()
}

/// `GET /system/api/statistic/include`
async fn manage_statistic_include(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match statistic::chart_include(pool).await {
        Ok(data) => ok(data),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/statistic/include/detail`
async fn manage_statistic_include_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<StatisticDetailQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match statistic::include_detail(
        pool,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// 验证码内存表：`captcha_id -> (答案, 过期时间戳)`。
fn captcha_store(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, (String, i64)>> {
    static STORE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, (String, i64)>>,
    > = std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// `GET /system/api/captcha`
///
/// 关闭验证码时（`settings.safe.admin_captcha_off = 1`，本站即如此）直接回
/// `{captcha_off: true}`；开启时生成 4 位数字并用 SVG data URI 作为图片。
async fn manage_captcha(State(state): State<AppState>) -> Response {
    let Some(pool) = state.db() else {
        return ok(json!({ "captcha_off": true }));
    };
    let safe = match setting::get_json_or_empty(pool, "safe").await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let off = safe
        .get("admin_captcha_off")
        .and_then(|value| value.as_i64())
        .unwrap_or(0)
        == 1;
    if off {
        return ok(json!({ "captcha_off": true }));
    }

    use rand::Rng;
    let mut rng = rand::thread_rng();
    let code: String = (0..4).map(|_| char::from(b'0' + rng.gen_range(0..10u8))).collect();
    let id = auth::generate_secret(16);
    let now = chrono::Utc::now().timestamp();
    if let Ok(mut store) = captcha_store().lock() {
        // 顺手清掉过期项
        store.retain(|_, (_, expire)| *expire > now);
        store.insert(id.clone(), (code.clone(), now + 600));
    }

    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="40"><rect width="120" height="40" fill="#f4f6f8"/><text x="60" y="27" font-family="monospace" font-size="22" fill="#2b2f36" text-anchor="middle" letter-spacing="3">{code}</text></svg>"##
    );
    let encoded = percent_encoding::utf8_percent_encode(
        &svg,
        percent_encoding::NON_ALPHANUMERIC,
    )
    .to_string();

    ok(json!({
        "captcha_off": false,
        "captcha_id": id,
        "captcha": format!("data:image/svg+xml;charset=utf-8,{encoded}"),
    }))
}

#[derive(Debug, Deserialize, Default)]
struct ManageModuleListQuery {
    #[serde(default)]
    exclude_id: Option<u32>,
}

/// `GET /system/api/module/list`
async fn manage_module_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ManageModuleListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match module::list(pool).await {
        Ok(mut list) => {
            if let Some(exclude) = query.exclude_id.filter(|id| *id > 0) {
                list.retain(|item| item.id != exclude);
            }
            ok(list)
        }
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct IdQuery {
    #[serde(default)]
    id: u32,
}

/// `GET /system/api/module/detail?id=`
async fn manage_module_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match module::get(pool, query.id).await {
        Ok(Some(item)) => ok(item),
        Ok(None) => fail("模型不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/module/detail`
async fn manage_module_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<module::ModuleInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match module::save(pool, &input).await {
        Ok(saved) => Json(json!({ "code": 0, "msg": "保存成功", "data": saved })).into_response(),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/module/delete`
async fn manage_module_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match module::get(pool, body.id).await {
        Ok(Some(item)) if item.is_system == 1 => fail("内置模型不能删除"),
        Ok(Some(_)) => match module::delete(pool, body.id).await {
            Ok(true) => ok_msg("模型已删除"),
            Ok(false) => fail("模型不存在"),
            Err(err) => failed(err),
        },
        Ok(None) => fail("模型不存在"),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct ModuleFieldDeleteBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    field_name: String,
}

/// `POST /system/api/module/field/delete`
async fn manage_module_field_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ModuleFieldDeleteBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match module::delete_field(pool, body.id, &body.field_name).await {
        Ok(true) => ok_msg("字段已删除"),
        Ok(false) => fail("字段不存在"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct ManageCategoryQuery {
    #[serde(default)]
    module_id: Option<u32>,
    #[serde(default, rename = "type")]
    kind: Option<u32>,
    #[serde(default)]
    show_type: Option<u32>,
    #[serde(default)]
    title: Option<String>,
}

/// `GET /system/api/category/list`
async fn manage_category_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ManageCategoryQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let all = match category::list_all(pool).await {
        Ok(list) => list,
        Err(err) => return failed(err),
    };

    let is_page = query.kind == Some(3);
    let keyword = query
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let mut list: Vec<Category> = all
        .iter()
        .filter(|item| {
            if is_page {
                item.kind == 3
            } else {
                item.kind == 1
                    && query
                        .module_id
                        .filter(|id| *id > 0)
                        .map(|id| item.module_id == id)
                        .unwrap_or(true)
            }
        })
        .filter(|item| {
            keyword
                .map(|word| item.title.contains(word))
                .unwrap_or(true)
        })
        .cloned()
        .collect();

    if is_page {
        list.sort_by_key(|item| (item.sort, item.id));
    } else {
        list.sort_by_key(|item| (item.module_id, item.sort, item.id));
    }

    // 搜索时强制列表结构
    let show_type = if keyword.is_some() {
        1
    } else {
        query.show_type.unwrap_or(0)
    };
    if show_type == 0 {
        ok(category::tree(&list))
    } else {
        ok(list)
    }
}

/// `GET /system/api/category/detail?id=`
async fn manage_category_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match category::get(pool, query.id).await {
        Ok(Some(item)) => ok(item),
        Ok(None) => fail("分类不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/category/detail`
async fn manage_category_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<category::CategoryInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match category::save(pool, &input).await {
        Ok(saved) => Json(json!({ "code": 0, "msg": "保存成功", "data": saved })).into_response(),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/category/delete`
async fn manage_category_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match category::delete(pool, body.id).await {
        Ok(true) => ok_msg("分类已删除"),
        Ok(false) => fail("分类不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/category/count`
async fn manage_category_count(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match category::update_archive_counts(pool).await {
        Ok(_) => ok_msg("保存成功"),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct NavListQuery {
    #[serde(default)]
    type_id: Option<u32>,
}

/// `GET /system/api/setting/nav?type_id=`
async fn manage_nav_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<NavListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let type_id = query.type_id.filter(|id| *id > 0).unwrap_or(1);
    match nav::list_by_type(pool, type_id).await {
        Ok(list) => ok(nav::tree(&list)),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/setting/nav`
async fn manage_nav_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<nav::NavInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match nav::save(pool, &input).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "配置已更新", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/setting/nav/delete`
async fn manage_nav_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match nav::delete(pool, body.id).await {
        Ok(true) => ok_msg("导航已删除"),
        Ok(false) => fail("导航不存在"),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/setting/nav/type`
async fn manage_nav_type_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match nav::list_types(pool).await {
        Ok(list) => ok(list),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/setting/nav/type`
async fn manage_nav_type_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<nav::NavTypeInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match nav::save_type(pool, &input).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "配置已更新", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/setting/nav/type/delete`
async fn manage_nav_type_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match nav::delete_type(pool, body.id).await {
        Ok(true) => ok_msg("导航分类已删除"),
        Ok(false) => fail("导航分类不存在"),
        Err(err) => model_error(err),
    }
}

// ---------------------------- 附件 -----------------------------------------

/// 附件响应体：在库字段外补上 `file_path` / `thumb`（Go 版 `Attachment.GetThumb`）。
fn attachment_payload(item: &attachment::Attachment) -> Value {
    let mut value = serde_json::to_value(item).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut value {
        let path = format!("/{}", item.file_location);
        let thumb = if item.logo.trim().is_empty() {
            // 本项目暂不生成缩略图，直接回原图，避免前端拿到破链
            path.clone()
        } else if item.logo.starts_with("http") || item.logo.starts_with('/') {
            item.logo.clone()
        } else {
            format!("/{}", item.logo)
        };
        map.insert("file_path".into(), Value::String(path));
        map.insert("thumb".into(), Value::String(thumb));
    }
    value
}

#[derive(Debug, Deserialize, Default)]
struct AttachmentListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    category_id: Option<u32>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<i32>,
}

/// `GET /system/api/attachment/list`
async fn manage_attachment_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AttachmentListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let filter = attachment::AttachmentFilter {
        category_id: query.category_id.unwrap_or(0),
        keyword: query.q.unwrap_or_default(),
        is_image: query.kind.unwrap_or(0),
    };
    let page = query.current.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(20);

    match attachment::list_page(pool, &filter, page, page_size).await {
        Ok((total, rows)) => {
            let list: Vec<Value> = rows.iter().map(attachment_payload).collect();
            Json(json!({
                "code": 0,
                "msg": "",
                "total": total,
                "limit": page_size,
                "data": list,
            }))
            .into_response()
        }
        Err(err) => failed(err),
    }
}

/// `GET /system/api/attachment/detail?id=`
async fn manage_attachment_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if query.id == 0 {
        return fail("要替换的图片资源不存在");
    }
    match attachment::get(pool, query.id).await {
        Ok(Some(item)) => ok(attachment_payload(&item)),
        Ok(None) => fail("要替换的图片资源不存在"),
        Err(err) => failed(err),
    }
}

/// 是否是「纯英文」文件名（可以原样保留）。
fn is_english_name(stem: &str) -> bool {
    !stem.is_empty()
        && stem
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ' ' | '.'))
}

/// `POST /system/api/attachment/upload`（multipart：`file` / `file1` + `category_id` / `id`）
async fn manage_attachment_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let public_dir = state.with_config(|config| config.public_dir());

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
                // 其余字段（chunks / chunk / md5 …）暂不支持，读掉即可
                let _ = field.bytes().await;
            }
        }
    }

    let Some((file_name, bytes)) = file else {
        return fail("无法获取上传文件");
    };

    let ext = attachment::extension_of(&file_name);
    if !attachment::is_allowed_ext(&ext) {
        return fail("不允许上传该类型文件");
    }
    let md5 = attachment::md5_hex(&bytes);
    let (width, height) = attachment::image_size(&bytes);
    let is_image: i8 = if attachment::is_image_ext(&ext) {
        1
    } else if attachment::is_video_ext(&ext) {
        2
    } else {
        0
    };

    // 替换模式：保持原文件名 / 原目录
    if replace_id > 0 {
        let Some(existing) = (match attachment::get(pool, replace_id).await {
            Ok(value) => value,
            Err(err) => return failed(err),
        }) else {
            return fail("要替换的图片资源不存在");
        };
        if let Ok(Some(other)) = attachment::get_by_md5(pool, &md5).await {
            if other.id != existing.id {
                return fail("替换失败，当前上传的资源已存在");
            }
        }
        let full = public_dir.join(&existing.file_location);
        if let Some(parent) = full.parent() {
            if let Err(err) = tokio::fs::create_dir_all(parent).await {
                return failed(err.into());
            }
        }
        if let Err(err) = tokio::fs::write(&full, &bytes).await {
            return failed(err.into());
        }
        if let Err(err) = attachment::update(
            pool,
            existing.id,
            None,
            None,
            Some(bytes.len() as u64),
            Some(&md5),
            Some(width),
            Some(height),
            None,
        )
        .await
        {
            return model_error(err);
        }
        let updated = match attachment::get(pool, existing.id).await {
            Ok(Some(item)) => item,
            Ok(None) => return fail("要替换的图片资源不存在"),
            Err(err) => return failed(err),
        };
        return ok(attachment_payload(&updated));
    }

    // 去重：同一份文件只存一条
    match attachment::get_by_md5(pool, &md5).await {
        Ok(Some(existing)) => {
            if category_id > 0 && existing.category_id != category_id {
                let _ = attachment::change_category(pool, category_id, &[existing.id]).await;
            }
            if let Ok(Some(item)) = attachment::get(pool, existing.id).await {
                return ok(attachment_payload(&item));
            }
            return ok(attachment_payload(&existing));
        }
        Ok(None) => {}
        Err(err) => return failed(err),
    }

    // 新文件：uploads/YYYYMM/DD/<name><ext>
    let now = chrono::Local::now();
    let subdir = format!("uploads/{}/{}", now.format("%Y%m"), now.format("%d"));
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name.as_str());
    let mut base = if is_english_name(stem) {
        stem.to_lowercase().replace("%20", "-").replace(' ', "-")
    } else {
        md5.get(8..24).unwrap_or(&md5).to_string()
    };
    if base.chars().count() > 200 {
        base = base.chars().take(200).collect();
    }
    let file_location = format!("{subdir}/{base}{ext}");

    let full = public_dir.join(&file_location);
    if let Some(parent) = full.parent() {
        if let Err(err) = tokio::fs::create_dir_all(parent).await {
            return failed(err.into());
        }
    }
    if let Err(err) = tokio::fs::write(&full, &bytes).await {
        return failed(err.into());
    }

    match attachment::insert(
        pool,
        stem,
        &file_location,
        bytes.len() as u64,
        &md5,
        width,
        height,
        category_id,
        is_image,
        "",
    )
    .await
    {
        Ok(id) => match attachment::get(pool, id).await {
            Ok(Some(item)) => ok(attachment_payload(&item)),
            _ => ok(json!({ "id": id, "file_location": file_location })),
        },
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/attachment/delete`
async fn manage_attachment_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let item = match attachment::get(pool, body.id).await {
        Ok(Some(item)) => item,
        Ok(None) => return fail("附件不存在"),
        Err(err) => return failed(err),
    };
    if let Err(err) = attachment::delete(pool, body.id).await {
        return failed(err);
    }

    let public_dir = state.with_config(|config| config.public_dir());
    let _ = tokio::fs::remove_file(public_dir.join(&item.file_location)).await;
    if !item.logo.trim().is_empty() && item.logo != item.file_location {
        let _ = tokio::fs::remove_file(public_dir.join(item.logo.trim_start_matches('/'))).await;
    }

    ok_msg("图片已删除")
}

#[derive(Debug, Deserialize, Default)]
struct AttachmentEditBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    file_name: String,
    #[serde(default)]
    file_location: String,
}

/// `POST /system/api/attachment/edit`
async fn manage_attachment_edit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AttachmentEditBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(item) = (match attachment::get(pool, body.id).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    }) else {
        return fail("附件不存在");
    };

    if !body.file_name.is_empty() {
        if let Err(err) = attachment::update(pool, item.id, Some(&body.file_name), None, None, None, None, None, None).await {
            return model_error(err);
        }
    }

    let new_location = body.file_location.trim();
    if !new_location.is_empty() && new_location != item.file_location {
        let old_ext = attachment::extension_of(&item.file_location);
        if attachment::extension_of(new_location) != old_ext {
            return fail("文件不正确");
        }
        if !attachment::is_allowed_ext(&old_ext) {
            return fail("文件不正确");
        }
        let sanitized = new_location
            .replace("%20", "-")
            .replace(' ', "-")
            .to_lowercase();
        if sanitized.chars().count() > 220 || sanitized.contains("..") {
            return fail("文件不正确");
        }

        let public_dir = state.with_config(|config| config.public_dir());
        let from = public_dir.join(&item.file_location);
        let to = public_dir.join(&sanitized);
        if let Some(parent) = to.parent() {
            if let Err(err) = tokio::fs::create_dir_all(parent).await {
                return failed(err.into());
            }
        }
        if let Err(err) = tokio::fs::rename(&from, &to).await {
            return failed(err.into());
        }

        let logo = if item.is_image == 2 { Some(item.logo.as_str()) } else { Some("") };
        if let Err(err) = attachment::update(pool, item.id, None, Some(&sanitized), None, None, None, None, logo).await {
            return model_error(err);
        }
    }

    match attachment::get(pool, item.id).await {
        Ok(Some(updated)) => ok(attachment_payload(&updated)),
        Ok(None) => fail("附件不存在"),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct AttachmentCategoryBody {
    #[serde(default)]
    category_id: u32,
    #[serde(default)]
    ids: Vec<u32>,
}

/// `POST /system/api/attachment/category`
async fn manage_attachment_category(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AttachmentCategoryBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match attachment::change_category(pool, body.category_id, &body.ids).await {
        Ok(_) => ok_msg("分类已更新"),
        Err(err) => failed(err),
    }
}

/// 扫描 `public/uploads` 把未登记的文件补录进附件库（后台异步执行）。
async fn scan_uploads(pool: &sqlx::MySqlPool, public_dir: &std::path::Path, limit: usize) -> anyhow::Result<usize> {
    let mut stack = vec![public_dir.join("uploads")];
    let mut inserted = 0usize;
    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let Ok(meta) = entry.metadata().await else { continue };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if inserted >= limit || meta.len() > 200 * 1024 * 1024 {
                continue;
            }
            let Ok(rel) = path.strip_prefix(public_dir) else { continue };
            let rel = rel.to_string_lossy().replace('\\', "/");
            let ext = attachment::extension_of(&rel);
            if !attachment::is_allowed_ext(&ext) {
                continue;
            }
            let Ok(bytes) = tokio::fs::read(&path).await else { continue };
            let md5 = attachment::md5_hex(&bytes);
            if attachment::get_by_md5(pool, &md5).await?.is_some() {
                continue;
            }
            let (width, height) = attachment::image_size(&bytes);
            let is_image = if attachment::is_image_ext(&ext) {
                1
            } else if attachment::is_video_ext(&ext) {
                2
            } else {
                0
            };
            let name = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default();
            attachment::insert(pool, &name, &rel, meta.len(), &md5, width, height, 0, is_image, "").await?;
            inserted += 1;
        }
    }
    Ok(inserted)
}

/// `POST /system/api/attachment/scan`
async fn manage_attachment_scan(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let pool = pool.clone();
    let public_dir = state.with_config(|config| config.public_dir());
    tokio::spawn(async move {
        if let Err(err) = scan_uploads(&pool, &public_dir, 2000).await {
            tracing::warn!("附件扫描失败：{err:#}");
        }
    });
    ok_msg("已提交后台处理")
}

// ---------------------------- 内容（后台） ---------------------------------

/// 组装内容详情响应（库字段 + 正文 + 模块扩展字段）。
async fn archive_detail_payload(
    pool: &sqlx::MySqlPool,
    id: i64,
) -> anyhow::Result<Option<Value>> {
    let Some((item, content)) = archive::detail(pool, id).await? else {
        return Ok(None);
    };
    let mut value = serde_json::to_value(&item)?;
    let mut extra_fields = serde_json::Map::new();
    if let Ok(Some(model)) = module::get(pool, item.module_id).await {
        if let Ok(values) = extra::load(pool, &model, item.id).await {
            extra_fields = values;
        }
    }
    if let Value::Object(map) = &mut value {
        map.insert("content".into(), Value::String(content));
        map.insert("extra".into(), Value::Object(extra_fields));
    }
    Ok(Some(value))
}

#[derive(Debug, Deserialize, Default)]
struct ManageArchiveQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    module_id: Option<u32>,
    #[serde(default)]
    category_id: Option<u32>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    order: Option<String>,
}

/// `GET /system/api/archive/list`
async fn manage_archive_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<ManageArchiveQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let mut query = archive::ArchiveQuery::new();
    if let Some(module_id) = params.module_id.filter(|id| *id > 0) {
        query.module_id = Some(module_id);
    }
    if let Some(category_id) = params.category_id.filter(|id| *id > 0) {
        let categories = match category::list_all(pool).await {
            Ok(list) => list,
            Err(err) => return failed(err),
        };
        query.category_ids = category::self_and_sub_ids(&categories, category_id);
    }
    if let Some(keyword) = params
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        query.keyword = Some(keyword.to_string());
    }

    let page = params.current.unwrap_or(1).max(1) as u32;
    let page_size = params.page_size.unwrap_or(20).max(1) as u32;
    let order = archive::Order::parse(params.order.as_deref().unwrap_or_default());

    match archive::list(pool, &query, order, page, page_size).await {
        Ok(result) => Json(json!({
            "code": 0,
            "msg": "",
            "total": result.total,
            "limit": page_size,
            "data": result.list,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/archive/detail?id=`
async fn manage_archive_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match archive_detail_payload(pool, query.id as i64).await {
        Ok(Some(value)) => ok(value),
        Ok(None) => fail("内容不存在"),
        Err(err) => failed(err),
    }
}

/// 内容保存入参：`id` 在外层，其余字段展开成 `ArchiveInput`。
#[derive(Debug, Deserialize)]
struct ArchiveSaveBody {
    #[serde(default)]
    id: i64,
    #[serde(flatten)]
    input: archive_write::ArchiveInput,
}

/// `POST /system/api/archive/detail`
async fn manage_archive_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ArchiveSaveBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    if body.id > 0 {
        match archive_write::update(pool, body.id, &body.input).await {
            Ok(saved) => {
                Json(json!({ "code": 0, "msg": "编辑完成", "data": saved })).into_response()
            }
            Err(err) => model_error(err),
        }
    } else {
        match archive_write::create(pool, &body.input).await {
            Ok(saved) => {
                Json(json!({ "code": 0, "msg": "发布完成", "data": saved })).into_response()
            }
            Err(err) => model_error(err),
        }
    }
}

/// `POST /system/api/archive/delete`
///
/// 正式内容先移到回收站（`archive_drafts.status = 99`）；已经在回收站里的直接永久删除。
async fn manage_archive_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let id = body.id as i64;

    match archive::get(pool, id).await {
        Ok(Some(_)) => match archive_admin::recycle(pool, id).await {
            Ok(true) => ok_msg("文章已删除"),
            Ok(false) => fail("文档不存在"),
            Err(err) => model_error(err),
        },
        Ok(None) => match archive_admin::delete_draft(pool, id).await {
            Ok(true) => ok_msg("文章已删除"),
            Ok(false) => fail("文档不存在"),
            Err(err) => model_error(err),
        },
        Err(err) => failed(err),
    }
}

/// `POST /system/api/archive/recover`
async fn manage_archive_recover(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match archive_admin::recover(pool, body.id as i64).await {
        Ok(true) => ok_msg("文章已恢复"),
        Ok(false) => fail("文档不存在"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct ArchiveStatusBody {
    #[serde(default)]
    ids: Vec<i64>,
    #[serde(default)]
    status: u32,
}

/// `POST /system/api/archive/status`
async fn manage_archive_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ArchiveStatusBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match archive_admin::set_status(pool, &body.ids, body.status).await {
        Ok(_) => ok_msg("文章已更新"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct ArchiveFlagBody {
    #[serde(default)]
    ids: Vec<i64>,
    #[serde(default)]
    flag: String,
}

/// `POST /system/api/archive/recommend`
async fn manage_archive_recommend(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ArchiveFlagBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match archive_admin::set_flags(pool, &body.ids, &body.flag).await {
        Ok(_) => ok_msg("文章已更新"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct ArchiveSortBody {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    sort: u32,
}

/// `POST /system/api/archive/sort`
async fn manage_archive_sort(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ArchiveSortBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match archive_admin::set_sort(pool, body.id, body.sort).await {
        Ok(true) => ok_msg("文章已更新"),
        Ok(false) => fail("文档不存在"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct ArchiveParentBody {
    #[serde(default)]
    ids: Vec<i64>,
    #[serde(default)]
    parent_id: i64,
}

/// `POST /system/api/archive/parent`
async fn manage_archive_parent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ArchiveParentBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match archive_admin::set_parent(pool, &body.ids, body.parent_id).await {
        Ok(_) => ok_msg("文章已更新"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct ArchiveCategoryBody {
    #[serde(default)]
    ids: Vec<i64>,
    #[serde(default)]
    category_id: u32,
    #[serde(default)]
    category_ids: Vec<u32>,
}

/// `POST /system/api/archive/category`
async fn manage_archive_category(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ArchiveCategoryBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match archive_admin::set_category(pool, &body.ids, body.category_id, &body.category_ids).await {
        Ok(_) => ok_msg("文章已更新"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct ArchiveTimeBody {
    #[serde(default)]
    ids: Vec<i64>,
    #[serde(default)]
    time: u32,
}

/// `POST /system/api/archive/time`
async fn manage_archive_time(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ArchiveTimeBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match archive_admin::set_time(pool, &body.ids, body.time).await {
        Ok(_) => ok_msg("文章已更新"),
        Err(err) => model_error(err),
    }
}

// ---------------------------- 可视化装修 -----------------------------------

/// `GET /system/api/diy/types`
async fn manage_diy_types(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    ok(json!({
        "types": diy::module_types(),
        "pages": diy::page_keys(),
    }))
}

#[derive(Debug, Deserialize, Default)]
struct DiyKeyQuery {
    #[serde(default)]
    key: Option<String>,
}

/// `GET /system/api/diy/page?key=home`
async fn manage_diy_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DiyKeyQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let key = query
        .key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("home")
        .to_string();

    let page = match diy::get_page(pool, &key).await {
        Ok(page) => page,
        Err(err) => return failed(err),
    };
    let modules = match diy::modules(pool, &key).await {
        Ok(list) => list,
        Err(err) => return failed(err),
    };
    let (title, updated) = match page {
        Some(page) => (page.title, page.updated_time.unwrap_or(0)),
        None => (String::new(), 0),
    };
    ok(json!({
        "key": key,
        "title": title,
        "updated_time": updated,
        "modules": modules,
    }))
}

#[derive(Debug, Deserialize, Default)]
struct DiySaveBody {
    #[serde(default)]
    key: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    modules: Vec<diy::DiyModule>,
}

/// `POST /system/api/diy/save`
async fn manage_diy_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DiySaveBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let key = if body.key.trim().is_empty() {
        "home".to_string()
    } else {
        body.key.clone()
    };

    match diy::save_page(pool, &key, &body.title, &body.modules).await {
        Ok(count) => {
            let bytes = serde_json::to_string(&body.modules)
                .map(|raw| raw.len())
                .unwrap_or(0);
            Json(json!({
                "code": 0,
                "msg": "保存成功",
                "data": { "key": key, "count": count, "bytes": bytes },
            }))
            .into_response()
        }
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct DiyKeyBody {
    #[serde(default)]
    key: String,
}

/// `POST /system/api/diy/reset`
async fn manage_diy_reset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DiyKeyBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let key = if body.key.trim().is_empty() {
        "home".to_string()
    } else {
        body.key.clone()
    };
    match diy::reset_page(pool, &key).await {
        Ok(()) => ok_msg("保存成功"),
        Err(err) => model_error(err),
    }
}

// ---------------------------- 采集 -----------------------------------------

/// `GET /system/api/collector/setting`
async fn manage_collector_setting(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match collector::get_setting(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/collector/setting`（整份覆盖）
async fn manage_collector_setting_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match collector::save_setting(pool, &body, true).await {
        Ok(_) => ok_msg("保存成功"),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct CollectReplaceBody {
    #[serde(default)]
    replace: bool,
    #[serde(default)]
    content_replace: Vec<collector::ReplaceKeyword>,
}

/// `POST /system/api/collector/article/replace`
async fn manage_collector_replace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CollectReplaceBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if body.content_replace.is_empty() {
        return fail("替换关键词不能为空");
    }

    let pairs: Vec<Value> = body
        .content_replace
        .iter()
        .map(|pair| json!({ "from": pair.from, "to": pair.to }))
        .collect();
    if let Err(err) = collector::save_setting(pool, &json!({ "content_replace": pairs }), false).await
    {
        return failed(err);
    }

    if body.replace {
        match collector::replace_content(pool, &body.content_replace).await {
            Ok(_) => ok_msg("替换任务已触发"),
            Err(err) => failed(err),
        }
    } else {
        ok_msg("关键词已保存")
    }
}

#[derive(Debug, Deserialize, Default)]
struct KeywordBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    title: String,
}

/// 在后台执行单个关键词的采集，并把进度写入任务记录。
fn spawn_collect_keyword(pool: sqlx::MySqlPool, keyword: collector::Keyword) {
    tokio::spawn(async move {
        let job = match collector::task_create(&pool, collector::COLLECTOR_TASK_KEY, "collect").await
        {
            Ok(job) => job,
            Err(err) => {
                tracing::warn!("创建采集任务失败：{err:#}");
                return;
            }
        };
        let config = collector::get_setting(&pool).await.unwrap_or_else(|_| json!({}));
        let result = collector::collect_keyword_articles(&pool, &keyword, &config).await;
        let (status, done, message) = match &result {
            Ok(count) => ("finished", *count as i64, format!("采集完成，新增 {count} 篇")),
            Err(err) => ("failed", 0, format!("{err:#}")),
        };
        let total = if result.is_ok() { done } else { 0 };
        if let Err(err) = collector::task_update(
            &pool,
            collector::COLLECTOR_TASK_KEY,
            &job.id,
            status,
            done,
            total,
            &message,
        )
        .await
        {
            tracing::warn!("更新采集任务失败：{err:#}");
        }
    });
}

/// `POST /system/api/collector/article/collect`
///
/// 按关键词立即触发一次采集；`title` 为空时用 `id` 定位关键词。
async fn manage_collector_collect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<KeywordBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let keyword = if body.title.trim().is_empty() {
        match collector::get_keyword(pool, body.id).await {
            Ok(Some(item)) => item,
            Ok(None) => return fail("关键词不存在"),
            Err(err) => return failed(err),
        }
    } else {
        let title = body.title.trim();
        match collector::ensure_keyword(pool, title).await {
            Ok(id) => match collector::get_keyword(pool, id).await {
                Ok(Some(item)) => item,
                Ok(None) => return fail("关键词不存在"),
                Err(err) => return failed(err),
            },
            Err(err) => return model_error(err),
        }
    };

    spawn_collect_keyword(pool.clone(), keyword);
    ok_msg("采集任务已触发")
}

/// `POST /system/api/collector/article/start`
///
/// 立即执行一次待采集关键词的采集任务，后台异步执行。
async fn manage_collector_start(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let pool = pool.clone();
    tokio::spawn(async move {
        let job = match collector::task_create(&pool, collector::COLLECTOR_TASK_KEY, "start").await {
            Ok(job) => job,
            Err(err) => {
                tracing::warn!("创建采集任务失败：{err:#}");
                return;
            }
        };
        let config = collector::get_setting(&pool).await.unwrap_or_else(|_| json!({}));
        let result = collector::collect_pending_articles(&pool, &config).await;
        let (status, done, total, message) = match &result {
            Ok((keywords, articles)) => (
                "finished",
                *keywords as i64,
                *keywords as i64,
                format!("处理 {keywords} 个关键词，新增 {articles} 篇"),
            ),
            Err(err) => ("failed", 0, 0, format!("{err:#}")),
        };
        if let Err(err) = collector::task_update(
            &pool,
            collector::COLLECTOR_TASK_KEY,
            &job.id,
            status,
            done,
            total,
            &message,
        )
        .await
        {
            tracing::warn!("更新采集任务失败：{err:#}");
        }
    });
    ok_msg("采集任务已触发")
}

/// `POST /system/api/collector/article/combination/get`
///
/// 用素材库里的同关键词素材组装一篇问答组合文章，结果不直接入库。
async fn manage_collector_combination(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<KeywordBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let keyword = if body.title.trim().is_empty() {
        match collector::get_keyword(pool, body.id).await {
            Ok(Some(item)) => item.title,
            Ok(None) => return fail("关键词不存在"),
            Err(err) => return failed(err),
        }
    } else {
        body.title.trim().to_string()
    };

    match collector::combination_article(pool, &keyword).await {
        Ok(article) => ok(article),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/collector/keyword/dig`
///
/// 立即触发一轮关键词挖掘（联想词扩展），后台异步执行。
async fn manage_collector_dig(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let pool = pool.clone();
    tokio::spawn(async move {
        let job = match collector::task_create(&pool, collector::COLLECTOR_TASK_KEY, "dig").await {
            Ok(job) => job,
            Err(err) => {
                tracing::warn!("创建关键词挖掘任务失败：{err:#}");
                return;
            }
        };
        let setting = collector::keyword_setting_get(&pool)
            .await
            .unwrap_or_else(|_| json!({}));
        let language = collector::cfg_str(&setting, "language", "zh");
        let max_count = collector::cfg_i64(&setting, "max_count", 100000);
        let result = collector::dig_keywords(&pool, 20, &language, max_count).await;
        let (status, done, total, message) = match &result {
            Ok((processed, added)) => (
                "finished",
                *processed as i64,
                *processed as i64,
                format!("处理 {processed} 个关键词，新增 {added} 个"),
            ),
            Err(err) => ("failed", 0, 0, format!("{err:#}")),
        };
        if let Err(err) = collector::task_update(
            &pool,
            collector::COLLECTOR_TASK_KEY,
            &job.id,
            status,
            done,
            total,
            &message,
        )
        .await
        {
            tracing::warn!("更新关键词挖掘任务失败：{err:#}");
        }
    });
    ok_msg("关键词拓展任务已触发")
}

// ---------------------------- AI 技能 --------------------------------------

fn skills_dir(state: &AppState) -> std::path::PathBuf {
    state.with_config(|config| config.data_dir())
}

/// `GET /system/api/anqi/skill/list`
async fn manage_skill_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let dir = skills_dir(&state);
    // Go 版这里用的是 code=-1 表示失败、0 表示成功（AI 系列接口的特例）
    Json(json!({
        "code": 0,
        "msg": "success",
        "data": skill::list(&dir),
    }))
    .into_response()
}

#[derive(Debug, Deserialize, Default)]
struct SkillNameQuery {
    #[serde(default)]
    name: Option<String>,
}

/// `GET /system/api/anqi/skill/detail?name=`
async fn manage_skill_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SkillNameQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let name = query.name.unwrap_or_default();
    if name.trim().is_empty() {
        return Json(json!({ "code": -1, "msg": "name is required" })).into_response();
    }
    let dir = skills_dir(&state);
    match skill::get(&dir, name.trim()) {
        Ok(Some(found)) => Json(json!({ "code": 0, "msg": "success", "data": found })).into_response(),
        Ok(None) => Json(json!({ "code": -1, "msg": "技能不存在" })).into_response(),
        Err(err) => Json(json!({ "code": -1, "msg": err.to_string() })).into_response(),
    }
}

/// `POST /system/api/anqi/skill/edit`
async fn manage_skill_edit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<skill::SkillInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let dir = skills_dir(&state);
    match skill::save(&dir, &input) {
        Ok(()) => Json(json!({ "code": 0, "msg": "保存成功" })).into_response(),
        Err(err) => Json(json!({ "code": -1, "msg": err.to_string() })).into_response(),
    }
}

/// `POST /system/api/anqi/skill/delete`
async fn manage_skill_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SkillNameQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let name = body.name.unwrap_or_default();
    if name.trim().is_empty() {
        return Json(json!({ "code": -1, "msg": "name is required" })).into_response();
    }
    let dir = skills_dir(&state);
    match skill::delete(&dir, name.trim()) {
        Ok(true) => Json(json!({ "code": 0, "msg": "删除成功" })).into_response(),
        Ok(false) => Json(json!({ "code": -1, "msg": "技能不存在" })).into_response(),
        Err(err) => Json(json!({ "code": -1, "msg": err.to_string() })).into_response(),
    }
}

/// `POST /system/api/anqi/skill/reload`
async fn manage_skill_reload(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let dir = skills_dir(&state);
    let count = skill::list(&dir).len();
    Json(json!({
        "code": 0,
        "msg": "成功",
        "data": {
            "count": count,
            "time": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        },
    }))
    .into_response()
}

// ---------------------------- 云端服务账号 / AI 对话 ---------------------------
//
// 口径（对齐 coverage-plan 第七节第 7 条）：
// - 这一组接口成功/失败用 `code:0` / `code:-1`（Go 版 StatusOK/StatusFailed），
//   与普通插件的 0/1 不同；`fail_ai` 统一产出 -1 信封；
// - 需要外呼云端服务的动作（翻译 / 伪原创 / 生图 / 提取 / AI 对话 / 流式生成）
//   统一走 `anqi_cloud_post`（`ext::http`）真实调用，凭据从 `settings.anqi` 读取；
// - 站内可得的（授权信息读取、AI 接口配置、会话历史、智能体列表与日志、
//   对话附件上传）直接从数据库实现。

/// AI 组专用失败信封：`code:-1`（Go `config.StatusFailed`）。
fn fail_ai(msg: impl Into<String>) -> Response {
    Json(json!({ "code": -1, "msg": msg.into() })).into_response()
}

/// `GET /system/api/anqi/info`
///
/// 返回站内已存的云端服务登录态并现算 `valid`（Go `GetAnqiInfo`；
/// 云端异步校验部分未实现）。
async fn manage_anqi_info(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match anqi::auth_info_get(pool).await {
        Ok(mut value) => {
            let bound = value
                .get("token")
                .and_then(Value::as_str)
                .map(|token| !token.trim().is_empty())
                .unwrap_or(false);
            if let Value::Object(map) = &mut value {
                map.insert("bound".into(), json!(bound));
                map.insert(
                    "tip".into(),
                    json!(if bound {
                        ""
                    } else {
                        "请先绑定安企账号后再使用 AI 助手"
                    }),
                );
            }
            ok(value)
        }
        Err(err) => failed(err),
    }
}

/// `GET /system/api/anqi/check`
///
/// Go 版会同步外呼云端服务校验登录态；这里返回站内已存登录态
/// （校验外呼未实现，`valid` 同样现算）。
async fn manage_anqi_check(State(state): State<AppState>, headers: HeaderMap) -> Response {
    manage_anqi_info(State(state), headers).await
}

#[derive(Debug, Deserialize, Default)]
struct AnqiLoginBody {
    #[serde(default)]
    user_name: String,
    #[serde(default)]
    password: String,
}

// ---------------------------- 云端服务客户端 ---------------------------------
//
// 对齐 Go `provider/anqi.go`：请求全部打到 `AnqiApi`，统一带
// `token`（`settings.anqi.token`）、`domain`（站点 `settings.system.base_url`）、
// `User-Agent: anqicms/<version>` 三个头，响应信封 `{code,msg,data}`，`code != 0` 即失败。
// 出网走 `rustcms_ext::http`（curl 后端支持 https），zip 走 `rustcms_ext::packer`。

/// 云端服务 API 根地址（Go `provider.AnqiApi`）。
const ANQI_API: &str = "https://auth.anqicms.com/auth";

/// 云端调用失败提示（直接作为 `code:1` 的 `msg` 回给前端）。
type CloudResult<T> = Result<T, String>;

/// 云端请求头：token / domain / User-Agent。
async fn anqi_cloud_headers(pool: &sqlx::MySqlPool) -> Vec<(String, String)> {
    let token = setting::get_json(pool, anqi::SETTING_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|value| {
            value
                .get("token")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    vec![
        ("token".to_string(), token),
        ("domain".to_string(), site_base_url(pool).await),
        (
            "User-Agent".to_string(),
            format!("anqicms/{}", env!("CARGO_PKG_VERSION")),
        ),
    ]
}

/// 云端账号绑定校验：`ai/chat` 与 AI 写作/流式入口共用，文案保持一致。
async fn ensure_anqi_bound(pool: &sqlx::MySqlPool) -> Result<(), String> {
    let info = anqi::auth_info_get(pool)
        .await
        .map_err(|err| format!("读取云端服务登录态失败：{err}"))?;
    let token = info
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if token.is_empty() {
        return Err("请先绑定安企账号后再使用 AI 助手".to_string());
    }
    Ok(())
}

/// 截断云端返回的异常正文，避免把整段响应塞进错误提示。
fn anqi_short_text(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// 解析云端信封：`code != 0` 视为失败。
fn anqi_cloud_envelope(response: &rustcms_ext::http::Response) -> CloudResult<Value> {
    let text = response.text();
    let value: Value = serde_json::from_str(&text)
        .map_err(|_| format!("云端服务返回异常：{}", anqi_short_text(&text, 200)))?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let msg = value
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        return Err(if msg.is_empty() {
            format!("云端服务返回失败（code={code}）")
        } else {
            msg
        });
    }
    Ok(value)
}

/// POST JSON 到云端服务，返回完整信封。
async fn anqi_cloud_post(
    pool: &sqlx::MySqlPool,
    path: &str,
    payload: &Value,
) -> CloudResult<Value> {
    let mut request = rustcms_ext::http::Request::post(format!("{ANQI_API}{path}"))
        .with_json(payload.clone())
        .with_timeout(std::time::Duration::from_secs(300));
    for (key, value) in anqi_cloud_headers(pool).await {
        request = request.with_header(key, value);
    }
    let response = rustcms_ext::http::send(&request)
        .await
        .map_err(|err| format!("云端服务请求失败：{err}"))?;
    anqi_cloud_envelope(&response)
}

/// 轮询云端 AI 计划结果（`/ai/syncplan`，每 10 秒一次、最多 30 次）。
///
/// `pending` 是 `/ai/generate`、`/ai/pseudo` 首次返回的 `data`；已完成则直接返回。
async fn anqi_ai_wait_plan(
    pool: &sqlx::MySqlPool,
    req_id: i64,
    mut pending: Value,
) -> CloudResult<Value> {
    if pending.get("status").and_then(Value::as_i64).unwrap_or(0) == 2 {
        return Ok(pending);
    }
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        pending = anqi_cloud_post(pool, "/ai/syncplan", &json!({ "req_id": req_id }))
            .await?
            .get("data")
            .cloned()
            .unwrap_or(Value::Null);
        match pending.get("status").and_then(Value::as_i64).unwrap_or(0) {
            // 0 等待、1 进行中：继续轮询
            0 | 1 => {}
            2 => return Ok(pending),
            _ => return Err("AI 任务失败，请稍后重试".to_string()),
        }
    }
    Err("AI 任务超时（5 分钟内未完成），请稍后重试".to_string())
}

/// multipart 上传到云端（字段名 `attach`，对齐 Go `SendFile(data, name, "attach")`）。
///
/// `ext::http` 没有 multipart 体，这里按 RFC 7578 手工拼包，用 `Body::Bytes` 发送。
async fn anqi_cloud_upload(
    pool: &sqlx::MySqlPool,
    file_name: &str,
    data: &[u8],
) -> CloudResult<Value> {
    let boundary = format!(
        "----rustcms{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    );
    let safe_name = file_name.replace(['"', '\r', '\n'], "");
    let mut body = Vec::with_capacity(data.len() + 256);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"attach\"; filename=\"{safe_name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let mut request = rustcms_ext::http::Request::post(format!("{ANQI_API}/template/upload"))
        .with_body(rustcms_ext::http::Body::Bytes(body))
        .with_header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .with_timeout(std::time::Duration::from_secs(300));
    for (key, value) in anqi_cloud_headers(pool).await {
        request = request.with_header(key, value);
    }
    let response = rustcms_ext::http::send(&request)
        .await
        .map_err(|err| format!("云端服务请求失败：{err}"))?;
    anqi_cloud_envelope(&response)
}

/// 云端账号登录态校验（Go `AuthId == 0` → 提示先登录）。
async fn anqi_require_login(pool: &sqlx::MySqlPool) -> CloudResult<()> {
    let auth_id = setting::get_json(pool, anqi::SETTING_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|value| value.get("auth_id").and_then(Value::as_i64))
        .unwrap_or(0);
    if auth_id == 0 {
        return Err("请先登录 AnqiCMS 云端账号".to_string());
    }
    Ok(())
}

/// 递归收集目录下所有文件。
fn anqi_collect_files(
    dir: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            anqi_collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// 打模板分享包（Go `CreateDesignZip`）：条目统一挂在 `template/` 与 `static/` 两个前缀下。
fn anqi_design_zip(store: &design::DesignStore, package: &str) -> CloudResult<Vec<u8>> {
    let template_dir = store.template_root.join(package);
    let mut files = Vec::new();
    anqi_collect_files(&template_dir, &mut files)
        .map_err(|err| format!("读取模板目录失败：{err}"))?;
    let static_dir = store.static_root.join(package);
    if static_dir.is_dir() {
        anqi_collect_files(&static_dir, &mut files)
            .map_err(|err| format!("读取静态资源目录失败：{err}"))?;
    }

    let mut writer = rustcms_ext::packer::ZipWriter::new();
    for file in &files {
        let (root, prefix) = if static_dir.is_dir() && file.starts_with(&static_dir) {
            (&static_dir, "static")
        } else {
            (&template_dir, "template")
        };
        let rel = file
            .strip_prefix(root)
            .map_err(|err| format!("读取模板文件路径失败：{err}"))?;
        let name = format!("{prefix}/{}", rel.to_string_lossy().replace('\\', "/"));
        let data = std::fs::read(file).map_err(|err| format!("读取 {} 失败：{err}", file.display()))?;
        writer
            .add_file(&name, &data)
            .map_err(|err| format!("打包 {} 失败：{err}", name))?;
    }
    Ok(writer.finish())
}

/// 回写模板配置（Go `writeDesignInfo`）。
fn anqi_write_design_info(
    store: &design::DesignStore,
    package: &str,
    info: &design::DesignPackage,
) -> CloudResult<()> {
    let dir = store.template_root.join(package);
    std::fs::create_dir_all(&dir).map_err(|err| format!("创建模板目录失败：{err}"))?;
    let raw = serde_json::to_string_pretty(info)
        .map_err(|err| format!("序列化模板配置失败：{err}"))?;
    std::fs::write(dir.join("config.json"), raw)
        .map_err(|err| format!("写入模板配置失败：{err}"))
}

/// 解包云端模板（Go `UploadDesignZip`）。
///
/// 包名优先取 `template/config.json` 的 `package`，否则用下载文件名；
/// `template/` 下只收 `.html` / `.json` / `.db`，`.php` 一律跳过；`static/` 全收。
fn anqi_extract_design_zip(
    store: &design::DesignStore,
    data: &[u8],
    download_url: &str,
) -> CloudResult<String> {
    let archive = rustcms_ext::packer::ZipArchive::open(data.to_vec())
        .map_err(|err| format!("模板包解析失败：{err}"))?;

    let mut info = Value::Object(Default::default());
    if let Ok(raw) = archive.read_by_name("template/config.json") {
        if let Ok(value) = serde_json::from_slice::<Value>(&raw) {
            if value.is_object() {
                info = value;
            }
        }
    }
    let mut package = info
        .get("package")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if package.is_empty() {
        package = download_url
            .rsplit(['/', '?', '#'])
            .next()
            .unwrap_or("")
            .trim_end_matches(".zip")
            .to_string();
    }
    if !design::valid_package(&package) {
        return Err("模板包名不合法".to_string());
    }

    // 同名模板已存在时追加序号（对齐 Go 的处理方式）
    let mut final_package = package.clone();
    let mut index = 1;
    while store.template_root.join(&final_package).exists() {
        if index > 100 {
            return Err("同名模板过多，无法落盘".to_string());
        }
        final_package = format!("{package}{index}");
        index += 1;
    }

    for entry in archive.entries() {
        if entry.is_dir {
            continue;
        }
        let name = entry.name.replace('\\', "/");
        if name.split('/').any(|segment| segment == "..") {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".php") {
            continue;
        }
        let dest = if let Some(rel) = name.strip_prefix("template/") {
            let rel_lower = rel.to_ascii_lowercase();
            if !(rel_lower.ends_with(".html")
                || rel_lower.ends_with(".json")
                || rel_lower.ends_with(".db"))
            {
                continue;
            }
            store.template_root.join(&final_package).join(rel)
        } else if let Some(rel) = name.strip_prefix("static/") {
            store.static_root.join(&final_package).join(rel)
        } else {
            continue;
        };
        if let Ok(bytes) = archive.read_entry(entry) {
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&dest, bytes);
        }
    }

    if let Value::Object(map) = &mut info {
        map.insert("package".to_string(), json!(final_package));
        if !map.contains_key("created") {
            map.insert(
                "created".to_string(),
                json!(chrono::Utc::now()
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()),
            );
        }
    }
    let dir = store.template_root.join(&final_package);
    std::fs::create_dir_all(&dir).map_err(|err| format!("创建模板目录失败：{err}"))?;
    std::fs::write(
        dir.join("config.json"),
        serde_json::to_string_pretty(&info).map_err(|err| format!("序列化模板配置失败：{err}"))?,
    )
    .map_err(|err| format!("写入模板配置失败：{err}"))?;
    Ok(final_package)
}

/// 记录翻译日志（Go `AddTranslateLog`，`md5` 冲突则更新）。
///
/// 写库失败只记警告：日志本身不影响翻译结果。
async fn anqi_add_translate_log(
    pool: &sqlx::MySqlPool,
    language: &str,
    to_language: &str,
    texts: &[String],
) {
    use md5::{Digest, Md5};

    let title: String = texts
        .first()
        .map(|text| text.chars().take(190).collect())
        .unwrap_or_default();
    // Go: md5(fmt.Sprint(text) + "-" + toLanguage)，fmt.Sprint([]string) 形如 `[a b c]`
    let joined = format!("[{}]", texts.join(" "));
    let digest = format!(
        "{:x}",
        Md5::digest(format!("{joined}-{to_language}").as_bytes())
    );
    if let Err(err) = sqlx::query(
        "INSERT INTO translate_logs (created_time, `md5`, origin_title, language, to_language) \
         VALUES (?, ?, ?, ?, ?) ON DUPLICATE KEY UPDATE origin_title = VALUES(origin_title), \
         language = VALUES(language), to_language = VALUES(to_language)",
    )
    .bind(chrono::Utc::now().timestamp())
    .bind(digest)
    .bind(title)
    .bind(language)
    .bind(to_language)
    .execute(pool)
    .await
    {
        tracing::warn!("记录翻译日志失败：{err}");
    }
}

/// `POST /system/api/anqi/login`
async fn manage_anqi_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AnqiLoginBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    // 参数校验与 Go 一致（ReadJSON 失败才报错；这里补上空参提示）
    if body.user_name.trim().is_empty() || body.password.is_empty() {
        return fail_ai("请输入云端服务账号和密码");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    // Go `AnqiLogin`：先清空本地登录态，再请求云端
    if let Err(err) = setting::save_json(pool, anqi::SETTING_KEY, &json!({})).await {
        return failed(err);
    }
    let payload = json!({
        "user_name": body.user_name.trim(),
        "password": body.password,
    });
    let value = match anqi_cloud_post(pool, "/login", &payload).await {
        Ok(value) => value,
        Err(msg) => return fail_ai(msg),
    };
    // 落盘云端返回的账号信息 + 登录 / 校验时间
    let data = value.get("data").cloned().unwrap_or_else(|| json!({}));
    let now = chrono::Utc::now().timestamp();
    let mut stored = data;
    if let Value::Object(map) = &mut stored {
        map.insert("login_time".to_string(), json!(now));
        map.insert("check_time".to_string(), json!(now));
    }
    if let Err(err) = setting::save_json(pool, anqi::SETTING_KEY, &stored).await {
        return failed(err);
    }
    match anqi::auth_info_get(pool).await {
        Ok(info) => ok(info),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/anqi/upload`
async fn manage_anqi_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    // 先收文件（Go `FormFile("file")`），再校验登录态后上传云端
    let mut file_name = String::new();
    let mut file_bytes: Option<Vec<u8>> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                if field.name() == Some("file") {
                    let name = field.file_name().unwrap_or("upload.bin").to_string();
                    match field.bytes().await {
                        Ok(bytes) => {
                            file_name = name;
                            file_bytes = Some(bytes.to_vec());
                        }
                        Err(err) => return failed(err.into()),
                    }
                }
            }
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        }
    }
    let Some(data) = file_bytes else {
        return fail_ai("请选择要上传的文件");
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if let Err(msg) = anqi_require_login(pool).await {
        return fail_ai(msg);
    }
    match anqi_cloud_upload(pool, &file_name, &data).await {
        Ok(value) => Json(json!({
            "code": 0,
            "msg": "上传成功",
            "data": value.get("data").cloned().unwrap_or(Value::Null),
        }))
        .into_response(),
        Err(msg) => fail_ai(msg),
    }
}

/// `POST /system/api/anqi/template/share`
async fn manage_anqi_template_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail_ai("请求格式错误");
    }
    let package = body
        .get("package")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if package.is_empty() {
        return fail_ai("请提供模板包名 package");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if let Err(msg) = anqi_require_login(pool).await {
        return fail_ai(msg);
    }
    let store = design_store(&state);
    // 先确认模板存在并读出现有配置（Go `GetDesignInfo(package)`）
    let info = match store.get(&package, true) {
        Ok(info) => info,
        Err(err) => return fail_ai(err.to_string()),
    };
    // Go 在 `auto_backup` 打开时先备份演示数据；本站没有对应的演示数据备份子能力，
    // 这里跳过，分享流程本身不依赖备份结果。
    let zip_data = match anqi_design_zip(&store, &package) {
        Ok(data) => data,
        Err(msg) => return fail_ai(msg),
    };
    // 上传模板包，拿到云端文件地址
    let attachment = match anqi_cloud_upload(pool, &format!("{package}.zip"), &zip_data).await {
        Ok(value) => value.get("data").cloned().unwrap_or(Value::Null),
        Err(msg) => return fail_ai(msg),
    };
    let file_location = attachment
        .get("file_location")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if file_location.is_empty() {
        return fail_ai("云端模板包上传失败");
    }
    // 请求体原样透传，补上模板路径 / 类型 / 云端 ID（Go 写法）
    let mut payload = body.clone();
    if let Value::Object(map) = &mut payload {
        map.insert("template_path".to_string(), json!(file_location));
        map.insert("template_type".to_string(), json!(info.template_type));
        map.insert("template_id".to_string(), json!(info.template_id));
    }
    let value = match anqi_cloud_post(pool, "/template/share", &payload).await {
        Ok(value) => value,
        Err(msg) => return fail_ai(msg),
    };
    // 回写云端返回的模板信息（Go `writeDesignInfo`）
    if let Some(data) = value.get("data").filter(|data| data.is_object()) {
        let mut updated = info.clone();
        if let Some(v) = data.get("template_id").and_then(Value::as_i64) {
            updated.template_id = v;
        }
        if let Some(v) = data.get("auth_id").and_then(Value::as_i64) {
            updated.auth_id = v;
        }
        if let Some(v) = data.get("name").and_then(Value::as_str) {
            updated.name = v.to_string();
        }
        if let Some(v) = data.get("version").and_then(Value::as_str) {
            updated.version = v.to_string();
        }
        if let Some(v) = data.get("description").and_then(Value::as_str) {
            updated.description = v.to_string();
        }
        if let Some(v) = data.get("author").and_then(Value::as_str) {
            updated.author = v.to_string();
        }
        if let Some(v) = data.get("homepage").and_then(Value::as_str) {
            updated.homepage = v.to_string();
        }
        if let Err(msg) = anqi_write_design_info(&store, &package, &updated) {
            return fail_ai(msg);
        }
    }
    ok_msg("提交成功")
}

#[derive(Debug, Deserialize, Default)]
struct AnqiDownloadTemplateBody {
    #[serde(default)]
    template_id: u32,
}

/// `POST /system/api/anqi/template/download`
async fn manage_anqi_template_download(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AnqiDownloadTemplateBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if body.template_id == 0 {
        return fail_ai("请提供要下载的模板 ID template_id");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    // 1) 向云端要下载地址
    let value = match anqi_cloud_post(
        pool,
        "/template/download",
        &json!({ "template_id": body.template_id }),
    )
    .await
    {
        Ok(value) => value,
        Err(msg) => return fail_ai(msg),
    };
    let url = value
        .get("data")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if url.is_empty() {
        return fail_ai("读取模板下载地址失败");
    }
    // 2) 下载模板包
    let request = rustcms_ext::http::Request::get(url.clone())
        .with_timeout(std::time::Duration::from_secs(300));
    let response = match rustcms_ext::http::send(&request).await {
        Ok(response) => response,
        Err(err) => return fail_ai(format!("下载模板包失败：{err}")),
    };
    if !response.is_success() {
        return fail_ai(format!("下载模板包失败（HTTP {}）", response.status));
    }
    // 3) 解包落盘（Go `UploadDesignZip`）
    let store = design_store(&state);
    match anqi_extract_design_zip(&store, &response.body, &url) {
        Ok(_package) => ok_msg("下载成功"),
        Err(msg) => fail_ai(msg),
    }
}

/// `POST /system/api/anqi/feedback`
async fn manage_anqi_feedback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail_ai("请求格式错误");
    }
    if body.get("title").and_then(Value::as_str).unwrap_or("").is_empty() {
        return fail_ai("请填写反馈标题");
    }
    if body.get("content").and_then(Value::as_str).unwrap_or("").is_empty() {
        return fail_ai("请填写反馈内容");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if let Err(msg) = anqi_require_login(pool).await {
        return fail_ai(msg);
    }
    // Go `AnqiSendFeedback`：补上版本 / 平台 / 域名后透传
    let mut payload = body.clone();
    if let Value::Object(map) = &mut payload {
        map.insert("version".to_string(), json!(env!("CARGO_PKG_VERSION")));
        map.insert("platform".to_string(), json!(std::env::consts::OS));
        map.insert("domain".to_string(), json!(site_base_url(pool).await));
    }
    match anqi_cloud_post(pool, "/feedback", &payload).await {
        Ok(_) => ok_msg("提交成功"),
        Err(msg) => fail_ai(msg),
    }
}

/// `POST /system/api/anqi/translate`
///
/// 云端翻译标题 / 摘要 / 关键词 / 正文，并回写文档或草稿。
async fn manage_anqi_translate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let id = body.get("id").and_then(Value::as_i64).unwrap_or(0);
    if id <= 0 {
        return fail_ai("请提供要翻译的文档 ID");
    }
    let to_language = body
        .get("to_language")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if to_language.is_empty() {
        return fail_ai("请提供目标语言 to_language");
    }
    // 取待翻译字段：先正式文档，再草稿（Go `GetArchiveByID` / `GetArchiveDraft`）
    let archive_row: Option<(Option<String>, Option<String>, Option<String>)> =
        match sqlx::query_as("SELECT title, description, keywords FROM archives WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await
        {
            Ok(row) => row,
            Err(err) => return failed(err.into()),
        };
    let use_draft = archive_row.is_none();
    let row = match archive_row {
        Some(row) => row,
        None => {
            match sqlx::query_as(
                "SELECT title, description, keywords FROM archive_drafts WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(pool)
            .await
            {
                Ok(Some(row)) => row,
                Ok(None) => return fail_ai("文档不存在"),
                Err(err) => return failed(err.into()),
            }
        }
    };
    let table = if use_draft { "archive_drafts" } else { "archives" };
    let (title, description, keywords) = (
        row.0.unwrap_or_default(),
        row.1.unwrap_or_default(),
        row.2.unwrap_or_default(),
    );
    let content = match archive::content(pool, id).await {
        Ok(Some(content)) => content,
        Ok(None) => return fail_ai("文档正文不存在"),
        Err(err) => return failed(err),
    };
    // Go `AnqiTranslateString`：源语言取站点语言
    let language = site_setting::get_section(pool, site_setting::SYSTEM)
        .await
        .ok()
        .and_then(|value| {
            value
                .get("language")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "zh-CN".to_string());
    let texts = vec![
        title.clone(),
        description.clone(),
        keywords.clone(),
        content.clone(),
    ];
    let payload = json!({
        "text": texts,
        "language": language,
        "to_language": to_language,
    });
    let value = match anqi_cloud_post(pool, "/translate", &payload).await {
        Ok(value) => value,
        Err(msg) => return fail_ai(msg),
    };
    let translated: Vec<String> = value
        .get("data")
        .and_then(|data| data.get("text"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if translated.len() < 4 {
        return fail_ai("翻译结果数量不足");
    }
    // 回写标题 / 摘要 / 关键词与正文（Go `AnqiTranslate` 的两处 Save）
    if let Err(err) = sqlx::query(&format!(
        "UPDATE {table} SET title = ?, description = ?, keywords = ? WHERE id = ?"
    ))
    .bind(&translated[0])
    .bind(&translated[1])
    .bind(&translated[2])
    .bind(id)
    .execute(pool)
    .await
    {
        return failed(err.into());
    }
    if let Err(err) = sqlx::query("UPDATE archive_data SET content = ? WHERE id = ?")
        .bind(&translated[3])
        .bind(id)
        .execute(pool)
        .await
    {
        return failed(err.into());
    }
    anqi_add_translate_log(pool, &language, &to_language, &translated).await;
    ok_msg("翻译完成")
}

/// 把伪原创结果写回文档（先试 `archives`，没有命中再试 `archive_drafts`）。
async fn apply_pseudo_result(
    pool: &sqlx::MySqlPool,
    id: i64,
    data: &Value,
) -> anyhow::Result<()> {
    let content = data.get("content").and_then(Value::as_str).unwrap_or("");
    if content.trim().is_empty() {
        return Ok(());
    }
    let title = data
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let description = data
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| archive_write::summarize(content, 250));
    let keywords = data
        .get("keywords")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    for table in ["archives", "archive_drafts"] {
        let affected = if title.is_empty() {
            sqlx::query(&format!(
                "UPDATE {table} SET description = ?, keywords = ?, has_pseudo = 1 WHERE id = ?"
            ))
            .bind(&description)
            .bind(&keywords)
            .bind(id)
            .execute(pool)
            .await?
            .rows_affected()
        } else {
            sqlx::query(&format!(
                "UPDATE {table} SET title = ?, description = ?, keywords = ?, has_pseudo = 1 WHERE id = ?"
            ))
            .bind(&title)
            .bind(&description)
            .bind(&keywords)
            .bind(id)
            .execute(pool)
            .await?
            .rows_affected()
        };
        if affected > 0 {
            sqlx::query("UPDATE archive_data SET content = ? WHERE id = ?")
                .bind(content)
                .bind(id)
                .execute(pool)
                .await?;
            return Ok(());
        }
    }
    Ok(())
}

/// `POST /system/api/anqi/ai/pseudo`
///
/// 取文档标题与正文提交云端 `/ai/pseudo`，随后后台轮询 `/ai/syncplan`，
/// 完成后把改写结果写回文档（对齐 Go `AnqiAiPseudoArticle` 的云端分支）。
async fn manage_anqi_ai_pseudo(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let id = body.get("id").and_then(Value::as_i64).unwrap_or(0);
    if id <= 0 {
        return fail_ai("请提供要伪原创的文档 ID");
    }
    let language = body
        .get("language")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("zh")
        .to_string();

    let title = match sqlx::query_scalar::<_, String>("SELECT title FROM archives WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
    {
        Ok(Some(title)) => title,
        Ok(None) => {
            match sqlx::query_scalar::<_, String>(
                "SELECT title FROM archive_drafts WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(pool)
            .await
            {
                Ok(Some(title)) => title,
                Ok(None) => return fail_ai("文档不存在"),
                Err(err) => return failed(err.into()),
            }
        }
        Err(err) => return failed(err.into()),
    };
    let content = match archive::content(pool, id).await {
        Ok(Some(content)) => content,
        Ok(None) => return fail_ai("文档内容不存在"),
        Err(err) => return failed(err),
    };

    let payload = json!({
        "title": title,
        "content": content,
        "language": language,
        "async": true,
    });
    let data = match anqi_cloud_post(pool, "/ai/pseudo", &payload).await {
        Ok(value) => value.get("data").cloned().unwrap_or(Value::Null),
        Err(msg) => return fail_ai(msg),
    };
    let req_id = data.get("req_id").and_then(Value::as_i64).unwrap_or(0);
    if req_id <= 0 {
        return fail_ai("云端未返回任务编号（req_id）");
    }

    let pool = pool.clone();
    tokio::spawn(async move {
        match anqi_ai_wait_plan(&pool, req_id, data).await {
            Ok(result) => {
                if let Err(err) = apply_pseudo_result(&pool, id, &result).await {
                    tracing::warn!("AI 伪原创结果写回失败（文档 {id}）：{err:#}");
                }
            }
            Err(err) => tracing::warn!("AI 伪原创失败（文档 {id}）：{err}"),
        }
    });
    ok_msg("AI伪原创已加入生成计划")
}

/// 流式生成任务的内存态（等价 Go `AiStreamResults`，进程内、不落库）。
#[derive(Debug, Default)]
struct AnqiStreamEntry {
    /// 尚未下发给前端的增量内容。
    pending: String,
    err: String,
    finished: bool,
}

fn anqi_stream_store(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, AnqiStreamEntry>> {
    static STORE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, AnqiStreamEntry>>,
    > = std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// `POST /system/api/anqi/ai/stream`
///
/// 提交云端 `/ai/stream` 并在后台读取 NDJSON 结果，立即返回 `stream_id`，
/// 前端用 `/ai/stream/data` 轮询。注意：`ext::http` 会缓冲整个响应，
/// 因此内容在上游完成后一次性可读，而不是逐 token 到达。
async fn manage_anqi_ai_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail_ai("请求格式错误");
    }
    let title = body
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if title.is_empty() {
        return fail_ai("请提供关键词标题 title");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if let Err(msg) = ensure_anqi_bound(pool).await {
        return fail_ai(msg);
    }
    let demand = body
        .get("demand")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let language = body
        .get("language")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("zh")
        .to_string();

    let stream_id = format!("{}", chrono::Utc::now().timestamp_millis());
    anqi_stream_store()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .insert(stream_id.clone(), AnqiStreamEntry::default());

    let task_id = stream_id.clone();
    let pool = pool.clone();
    tokio::spawn(async move {
        let payload = json!({
            "keyword": title,
            "demand": demand,
            "language": language,
        });
        let mut request =
            rustcms_ext::http::Request::post(format!("{ANQI_API}/ai/stream"))
                .with_json(payload)
                .with_timeout(std::time::Duration::from_secs(300));
        for (key, value) in anqi_cloud_headers(&pool).await {
            request = request.with_header(key, value);
        }

        let response = match rustcms_ext::http::send(&request).await {
            Ok(response) => response,
            Err(err) => {
                let mut store = anqi_stream_store().lock().unwrap_or_else(|err| err.into_inner());
                if let Some(entry) = store.get_mut(&task_id) {
                    entry.err = format!("AI 流式生成请求失败：{err}");
                    entry.finished = true;
                }
                return;
            }
        };

        let mut content = String::new();
        let mut error = String::new();
        if !response.is_success() {
            error = format!("AI 流式生成失败（HTTP {}）", response.status);
        } else {
            for line in response.text().lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let value: Value = match serde_json::from_str(line) {
                    Ok(value) => value,
                    Err(_) => {
                        error = "AI 流式生成返回异常".to_string();
                        break;
                    }
                };
                let code = value.get("code").and_then(Value::as_i64).unwrap_or(0);
                if code != 0 {
                    error = value
                        .get("msg")
                        .and_then(Value::as_str)
                        .unwrap_or("AI 流式生成失败")
                        .to_string();
                    break;
                }
                content.push_str(value.get("data").and_then(Value::as_str).unwrap_or(""));
            }
        }

        let mut store = anqi_stream_store().lock().unwrap_or_else(|err| err.into_inner());
        if let Some(entry) = store.get_mut(&task_id) {
            entry.pending = content;
            entry.err = error;
            entry.finished = true;
        }
    });
    ok(stream_id)
}

/// `GET /system/api/anqi/ai/stream/data?stream_id=`
///
/// 返回尚未下发的增量内容；任务结束后再查一次即得到 `msg="finished"`。
async fn manage_anqi_ai_stream_data(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AnqiStreamDataQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let stream_id = query.stream_id.trim().to_string();
    let mut store = anqi_stream_store().lock().unwrap_or_else(|err| err.into_inner());
    let (content, error, finished) = match store.get_mut(&stream_id) {
        Some(entry) => {
            let content = std::mem::take(&mut entry.pending);
            let error = std::mem::take(&mut entry.err);
            (content, error, entry.finished)
        }
        // 未知 / 已清理的任务：与 Go 版一致，直接回 finished
        None => (String::new(), String::new(), true),
    };
    if finished {
        store.remove(&stream_id);
    }
    drop(store);

    if !error.is_empty() {
        return fail_ai(error);
    }
    let msg = if finished { "finished" } else { "" };
    Json(json!({ "code": 0, "msg": msg, "data": content })).into_response()
}

#[derive(Debug, Deserialize, Default)]
struct AnqiStreamDataQuery {
    #[serde(default)]
    stream_id: String,
}

// ── AI 对话（aiChat.go）──

/// AI 对话请求体（对齐 Go 的 `AiChatRequest`）。
#[derive(Debug, Deserialize, Default)]
struct AiChatBody {
    #[serde(default)]
    message: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    session_id: String,
}

/// 取配置项字符串（兼容多个字段别名）。
fn config_string(config: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| config.get(*key).and_then(Value::as_str))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 解析本次对话使用的接口：`custom:<下标>` 取站内配置，否则走官方云端。
async fn resolve_ai_provider(
    pool: &sqlx::MySqlPool,
    model: &str,
) -> Result<(String, String, String, u64, f64, u64), String> {
    let model = model.trim();
    if let Some(index) = model
        .strip_prefix("custom:")
        .and_then(|value| value.trim().parse::<usize>().ok())
    {
        let ai = anqi::ai_settings_get(pool)
            .await
            .map_err(|err| format!("读取 AI 配置失败：{err}"))?;
        let config = ai
            .get("configs")
            .and_then(Value::as_array)
            .and_then(|list| list.get(index))
            .ok_or_else(|| "自定义 AI 接口配置不存在".to_string())?;
        let base_url = config_string(config, &["base_url", "url"])
            .ok_or_else(|| "自定义 AI 接口缺少 base_url".to_string())?;
        let api_key = config_string(config, &["api_key", "token"])
            .ok_or_else(|| "自定义 AI 接口缺少 api_key".to_string())?;
        let model_name = config_string(config, &["model", "name"])
            .ok_or_else(|| "自定义 AI 接口缺少 model".to_string())?;
        let max_tokens = config
            .get("max_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(4096);
        let temperature = config
            .get("temperature")
            .and_then(Value::as_f64)
            .unwrap_or(0.7);
        let timeout = config
            .get("timeout_seconds")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
            .unwrap_or(120);
        return Ok((base_url, api_key, model_name, max_tokens, temperature, timeout));
    }

    let official = if model.is_empty() {
        "anqi-flash"
    } else if matches!(model, "anqi-flash" | "anqi-pro") {
        model
    } else {
        return Err("官方模型只支持 anqi-flash / anqi-pro".to_string());
    };
    let info = anqi::auth_info_get(pool)
        .await
        .map_err(|err| format!("读取云端服务登录态失败：{err}"))?;
    let token = info
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    ensure_anqi_bound(pool).await?;
    Ok((
        "https://auth.anqicms.com/auth/v1".to_string(),
        token,
        official.to_string(),
        8192,
        0.7,
        300,
    ))
}

/// 调用 OpenAI 兼容的 `/chat/completions`（非流式），返回助手回复文本。
///
/// 官方云端与 `custom:<下标>` 配置共用；`system_prompt` 供智能体注入策略。
async fn ai_chat_complete(
    pool: &sqlx::MySqlPool,
    message: &str,
    model: &str,
    system_prompt: Option<&str>,
) -> CloudResult<String> {
    let (base_url, api_key, model, max_tokens, temperature, timeout) =
        resolve_ai_provider(pool, model).await?;
    let mut messages = Vec::new();
    if let Some(prompt) = system_prompt.map(str::trim).filter(|value| !value.is_empty()) {
        messages.push(json!({ "role": "system", "content": prompt }));
    }
    messages.push(json!({ "role": "user", "content": message }));

    let link = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let payload = json!({
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": temperature,
        "stream": false,
    });
    let request = rustcms_ext::http::Request::post(link)
        .with_header("authorization", format!("Bearer {api_key}"))
        .with_header("content-type", "application/json")
        .with_json(payload)
        .with_timeout(std::time::Duration::from_secs(timeout));
    let response = rustcms_ext::http::send(&request)
        .await
        .map_err(|err| {
            tracing::warn!("AI 对话请求失败：{err:#}");
            format!("AI 服务请求失败：{err}")
        })?;
    let value: Value = serde_json::from_str(&response.text()).unwrap_or(Value::Null);
    if !response.is_success() {
        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .or_else(|| value.get("msg").and_then(Value::as_str))
            .unwrap_or("AI 服务返回错误");
        return Err(message.to_string());
    }
    let content = value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if content.is_empty() {
        return Err("AI 服务返回内容为空".to_string());
    }
    Ok(content)
}

/// 按项目现有 SSE 约定输出单块回复：`session` → `message` → `end`。
fn ai_chat_sse(session_id: &str, content: &str) -> Response {
    let mut body = String::new();
    body.push_str(&format!("event: session\ndata: {session_id}\n\n"));
    body.push_str(&format!(
        "event: message\ndata: {}\n\n",
        json!({ "v": content, "timestamp": chrono::Utc::now().timestamp() })
    ));
    body.push_str("event: end\ndata: [DONE]\n\n");
    (
        [
            (header::CONTENT_TYPE, "text/event-stream; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

/// `POST /system/api/anqi/ai/chat`
///
/// 调用 OpenAI 兼容的 `/chat/completions`（官方云端或 `custom:<下标>` 配置），
/// 返回与 Go 版一致的最小 SSE：`session` → 单个 `message` 块 → `end`。
async fn manage_ai_chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AiChatBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let message = if body.message.trim().is_empty() {
        body.prompt.trim().to_string()
    } else {
        body.message.trim().to_string()
    };
    if message.is_empty() {
        return fail_ai("message cannot be empty");
    }
    let Some(pool) = state.db() else {
        return fail_ai("数据库未配置");
    };
    let session_id = if body.session_id.trim().is_empty() {
        format!("session_{}", chrono::Utc::now().timestamp_millis())
    } else {
        body.session_id.trim().to_string()
    };
    let content = match ai_chat_complete(pool, &message, &body.model, None).await {
        Ok(value) => value,
        Err(err) => return fail_ai(err),
    };
    ai_chat_sse(&session_id, &content)
}

/// `POST /system/api/anqi/ai/chat/confirm`
///
/// 审批确认依赖进行中的对话（工具审批队列），没有对话就永远没有待审批项；
/// 参数校验与 Go 完全一致。
async fn manage_ai_chat_confirm(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let tool_call_id = body
        .get("tool_call_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let decision = body.get("decision").and_then(Value::as_str).unwrap_or("");
    if tool_call_id.is_empty() || decision.is_empty() {
        return fail_ai("tool_call_id and decision are required");
    }
    match decision {
        "allow" | "deny" | "once_allow" | "full_control" => {}
        _ => return fail_ai("decision must be allow/deny/once_allow/full_control"),
    }
    fail_ai("no pending approval for this tool_call_id")
}

/// `POST /system/api/anqi/ai/upload`
///
/// **真实实现**：附件落到 `<cache>/ai/upload/<session_id>/<时间戳>_<文件名>`。
async fn manage_ai_chat_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let mut file_name = String::new();
    let mut bytes: Option<Vec<u8>> = None;
    let mut session_id = String::new();
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        match field.name().unwrap_or_default() {
            "session_id" => session_id = field.text().await.unwrap_or_default(),
            "file" => {
                file_name = field.file_name().unwrap_or_default().to_string();
                bytes = Some(field.bytes().await.unwrap_or_default().to_vec());
            }
            _ => {
                let _ = field.bytes().await;
            }
        }
    }
    let Some(bytes) = bytes else {
        return fail("请选择要上传的文件");
    };
    if session_id.trim().is_empty() {
        session_id = "common".to_string();
    }
    // 会话 ID 只允许安全字符，防止路径穿越
    let session_id: String = session_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();

    let ext = std::path::Path::new(&file_name)
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    let base_name = file_name
        .strip_suffix(&ext)
        .unwrap_or(&file_name)
        .to_string();
    let save_name = format!(
        "{}_{}{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        base_name,
        ext
    );

    let cache_dir = state.with_config(|config| config.cache_dir());
    let upload_dir = cache_dir.join("ai").join("upload").join(&session_id);
    if let Err(err) = std::fs::create_dir_all(&upload_dir) {
        return fail_ai(format!("目录创建失败: {err}"));
    }
    if let Err(err) = std::fs::write(upload_dir.join(&save_name), &bytes) {
        return fail_ai(format!("文件保存失败: {err}"));
    }

    ok(json!({
        "file_name": file_name,
        "file_size": bytes.len(),
        "file_path": format!("/cache/ai/upload/{session_id}/{save_name}"),
        "session_id": session_id,
        "file_ext": ext,
    }))
}

/// `GET /system/api/anqi/ai/settings`
async fn manage_ai_settings(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match anqi::ai_settings_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/anqi/ai/settings`
///
/// 请求体是 `eino.Config` 对象数组，`configs` 整份替换、其余保留。
async fn manage_ai_settings_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Value>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(configs) = body.as_array() else {
        return fail_ai("invalid request");
    };
    for item in configs {
        if !item.is_object() {
            return fail_ai("invalid request");
        }
    }
    match anqi::ai_settings_save(pool, configs).await {
        Ok(saved) => {
            log_action(pool, &current, &client_ip(&headers, peer), "更新AI接口配置").await;
            ok(saved)
        }
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/anqi/ai/history?session_id=`
async fn manage_ai_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AnqiHistoryQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if query.session_id.trim().is_empty() {
        return fail_ai("session_id is required");
    }
    match anqi::chat_messages(pool, query.session_id.trim()).await {
        Ok(messages) => ok(messages),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct AnqiHistoryQuery {
    #[serde(default)]
    session_id: String,
}

/// `GET /system/api/anqi/ai/sessions`
async fn manage_ai_sessions(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match anqi::chat_sessions(pool).await {
        Ok(sessions) => ok(sessions),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/anqi/ai/agents`
async fn manage_ai_agents(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match anqi::agent_list(pool).await {
        Ok(agents) => ok(agents),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct AnqiAgentLogQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
}

/// `GET /system/api/anqi/ai/agents/:id/logs`
async fn manage_ai_agent_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<u32>,
    Query(query): Query<AnqiAgentLogQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if agent_id == 0 {
        return fail_ai("ID无效");
    }
    match anqi::agent_logs(pool, agent_id, query.current.unwrap_or(1), query.page_size.unwrap_or(20))
        .await
    {
        Ok(logs) => ok(logs),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/anqi/ai/agents/:id/chat`
///
/// 与普通 AI 对话共用 `/chat/completions`，额外把智能体的策略与最近摘要
/// 注入 system 提示，并用智能体自己的 `session_id` 回给前端。
async fn manage_ai_agent_chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<u32>,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if agent_id == 0 {
        return fail_ai("ID无效");
    }
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| body.get("prompt").and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string();
    if message.is_empty() {
        return fail_ai("消息不能为空");
    }
    let agent = match anqi::agent_get(pool, agent_id).await {
        Ok(Some(agent)) => agent,
        Ok(None) => return fail_ai("智能体不存在"),
        Err(err) => return failed(err),
    };
    let session_id = agent
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("agent_{agent_id}_{}", chrono::Utc::now().timestamp_millis()));
    let strategy = agent
        .get("strategy")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let summary = agent
        .get("last_summary")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let mut prompt = String::from("你是 AnQiCMS 的 AI 智能体，请按下面的执行策略完成用户任务。");
    if !strategy.is_empty() {
        prompt.push_str(&format!("\n执行策略：{strategy}"));
    }
    if !summary.is_empty() {
        prompt.push_str(&format!("\n最近一次执行摘要：{summary}"));
    }

    let model = body.get("model").and_then(Value::as_str).unwrap_or("");
    let content = match ai_chat_complete(pool, &message, model, Some(&prompt)).await {
        Ok(value) => value,
        Err(err) => return fail_ai(err),
    };
    ai_chat_sse(&session_id, &content)
}

/// `GET /system/api/anqi/ai/health`
///
/// Rust 版未内置 AI 服务，等价于 Go 的 `AiSrv == nil` 分支。
async fn manage_ai_health(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    fail_ai("ai service not available")
}

/// `GET /system/api/anqi/ai/chat/trace`
async fn manage_ai_chat_trace(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    fail_ai("ai service not available")
}

#[derive(Debug, Deserialize, Default)]
struct AnqiImageBody {
    #[serde(default)]
    image: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    size: String,
    #[serde(default, rename = "type")]
    kind: i32,
}

/// 标准 Base64 编码（图生图需要把图片转成 base64 再提交云端）。
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(((b1 & 0x0F) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(b2 & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// `POST /system/api/anqi/ai/image`
///
/// 文生图 / 图生图：`image` 支持远程 URL（会先下载再转 base64），也支持直接传
/// base64；最终统一提交云端 `/ai/image`，返回云端生图记录。
async fn manage_ai_image(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AnqiImageBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if body.prompt.trim().is_empty() {
        return fail_ai("提示词不能为空");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let mut image = body.image.trim().to_string();
    if image.starts_with("http://") || image.starts_with("https://") || image.starts_with("//") {
        let url = if image.starts_with("//") {
            format!("https:{image}")
        } else {
            image.clone()
        };
        let request = rustcms_ext::http::Request::get(url)
            .with_timeout(std::time::Duration::from_secs(30));
        let response = match rustcms_ext::http::send(&request).await {
            Ok(response) => response,
            Err(err) => return fail_ai(format!("下载参考图片失败：{err}")),
        };
        if !response.is_success() {
            return fail_ai(format!("下载参考图片失败（HTTP {}）", response.status));
        }
        image = base64_encode(&response.body);
    }

    let payload = json!({
        "image": image,
        "prompt": body.prompt.trim(),
        "size": body.size.trim(),
        "type": body.kind,
    });
    match anqi_cloud_post(pool, "/ai/image", &payload).await {
        Ok(value) => ok(value.get("data").cloned().unwrap_or(Value::Null)),
        Err(msg) => fail_ai(msg),
    }
}

#[derive(Debug, Deserialize, Default)]
struct AnqiImageConfirmBody {
    #[serde(default)]
    action: i32,
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    replace: bool,
    #[serde(default, rename = "replace_id")]
    replace_id: u32,
}

/// 保存下载到的图片：`replace_id > 0` 时覆盖原附件，否则按 md5 去重后落盘入库。
async fn store_image_bytes(
    pool: &sqlx::MySqlPool,
    public_dir: &std::path::Path,
    stem: &str,
    ext: &str,
    bytes: &[u8],
    replace_id: u32,
) -> CloudResult<()> {
    let md5 = attachment::md5_hex(bytes);
    let (width, height) = attachment::image_size(bytes);

    if replace_id > 0 {
        let existing = attachment::get(pool, replace_id)
            .await
            .map_err(|err| format!("读取原图片失败：{err}"))?
            .ok_or_else(|| "要替换的图片资源不存在".to_string())?;
        if let Ok(Some(other)) = attachment::get_by_md5(pool, &md5).await {
            if other.id != existing.id {
                return Err("替换失败，当前图片资源已存在".to_string());
            }
        }
        let full = public_dir.join(&existing.file_location);
        if let Some(parent) = full.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| format!("创建目录失败：{err}"))?;
        }
        tokio::fs::write(&full, bytes)
            .await
            .map_err(|err| format!("保存图片失败：{err}"))?;
        attachment::update(
            pool,
            existing.id,
            None,
            None,
            Some(bytes.len() as u64),
            Some(&md5),
            Some(width),
            Some(height),
            None,
        )
        .await
        .map_err(|err| err.to_string())?;
        return Ok(());
    }

    // 同一份图片只存一条
    if let Ok(Some(_)) = attachment::get_by_md5(pool, &md5).await {
        return Ok(());
    }

    let now = chrono::Local::now();
    let subdir = format!("uploads/{}/{}", now.format("%Y%m"), now.format("%d"));
    let mut base = if is_english_name(stem) {
        stem.to_lowercase().replace("%20", "-").replace(' ', "-")
    } else {
        md5.get(8..24).unwrap_or(&md5).to_string()
    };
    if base.chars().count() > 200 {
        base = base.chars().take(200).collect();
    }
    let file_location = format!("{subdir}/{base}{ext}");
    let full = public_dir.join(&file_location);
    if let Some(parent) = full.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|err| format!("创建目录失败：{err}"))?;
    }
    tokio::fs::write(&full, bytes)
        .await
        .map_err(|err| format!("保存图片失败：{err}"))?;
    attachment::insert(
        pool,
        stem,
        &file_location,
        bytes.len() as u64,
        &md5,
        width,
        height,
        0,
        1,
        "",
    )
    .await
    .map_err(|err| err.to_string())?;
    Ok(())
}

/// `POST /system/api/anqi/ai/image/confirm`
///
/// `action=1` 时下载远端图片并保存为附件（`replace` + `replace_id` 覆盖原图）。
async fn manage_ai_image_confirm(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AnqiImageConfirmBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if body.action != 1 {
        return ok_msg("");
    }
    let url = body.url.trim();
    if url.is_empty() {
        return fail_ai("请提供要保存的图片地址 url");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let mut title = body.title.trim().to_string();
    let mut replace_id = body.replace_id;
    if body.replace && replace_id > 0 {
        match attachment::get(pool, replace_id).await {
            Ok(Some(existing)) => {
                if title.is_empty() {
                    title = existing.file_name.clone();
                }
            }
            Ok(None) => return fail_ai("要替换的图片资源不存在"),
            Err(err) => return failed(err),
        }
    } else {
        replace_id = 0;
    }
    if title.is_empty() {
        let path = url.split(|ch| ch == '?' || ch == '#').next().unwrap_or("");
        let name = path.rsplit('/').next().unwrap_or("image");
        title = name
            .rsplit_once('.')
            .map(|(stem, _)| stem)
            .unwrap_or(name)
            .to_string();
    }
    if title.trim().is_empty() {
        title = "image".to_string();
    }

    let request = rustcms_ext::http::Request::get(url)
        .with_header("referer", url)
        .with_timeout(std::time::Duration::from_secs(15));
    let response = match rustcms_ext::http::send(&request).await {
        Ok(response) => response,
        Err(err) => return fail_ai(format!("下载图片失败：{err}")),
    };
    if !response.is_success() {
        return fail_ai(format!("下载图片失败（HTTP {}）", response.status));
    }
    let content_type = response
        .header("content-type")
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let ext = match content_type.as_str() {
        "image/jpeg" | "image/jpg" => ".jpg",
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        _ => return fail_ai("不支持的图片格式（仅支持 jpg / png / gif / webp）"),
    };
    if response.body.is_empty() {
        return fail_ai("下载的图片内容为空");
    }

    let public_dir = state.with_config(|config| config.public_dir());
    match store_image_bytes(
        pool,
        &public_dir,
        title.trim(),
        ext,
        &response.body,
        replace_id,
    )
    .await
    {
        Ok(()) => ok_msg(""),
        Err(msg) => fail_ai(msg),
    }
}

/// `GET /system/api/anqi/ai/image/histories`
///
/// 生图历史存在云端服务侧（Go 版拉取失败时返回空），这里照实返回空列表。
async fn manage_ai_image_histories(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AnqiAgentLogQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let _ = query;
    Json(json!({ "code": 0, "msg": "", "total": 0, "data": [] })).into_response()
}

/// `POST /system/api/anqi/extract/keywords`
async fn manage_anqi_extract_keywords(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if body.get("text").and_then(Value::as_str).unwrap_or("").is_empty() {
        return fail_ai("请提供要提取关键词的文本 text");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let payload = json!({
        "text": body.get("text").cloned().unwrap_or(Value::Null),
        "num": body.get("num").and_then(Value::as_i64).unwrap_or(0),
    });
    let value = match anqi_cloud_post(pool, "/extract/keywords", &payload).await {
        Ok(value) => value,
        Err(msg) => return fail_ai(msg),
    };
    Json(json!({
        "code": 0,
        "msg": "提交成功",
        "data": value.get("data").cloned().unwrap_or(Value::Null),
    }))
    .into_response()
}

/// `POST /system/api/anqi/extract/description`
async fn manage_anqi_extract_description(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if body.get("text").and_then(Value::as_str).unwrap_or("").is_empty() {
        return fail_ai("请提供要提取摘要的文本 text");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let payload = json!({
        "text": body.get("text").cloned().unwrap_or(Value::Null),
    });
    let value = match anqi_cloud_post(pool, "/extract/summarize", &payload).await {
        Ok(value) => value,
        Err(msg) => return fail_ai(msg),
    };
    // Go 把云端返回的 []string 直接拼接成一段摘要
    let summary = match value.get("data") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    };
    Json(json!({ "code": 0, "msg": "提交成功", "data": summary })).into_response()
}

/// `POST /system/api/anqi/restart`
async fn manage_anqi_restart(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    // 方案 A：拉起同一可执行文件的新进程后退出（依赖外部守护 / 进程管理器接管）。
    // 先让本次响应写完，再换进程，避免端口被旧进程占住。
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(err) => return fail_ai(format!("读取可执行文件路径失败：{err}")),
    };
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        match std::process::Command::new(&exe).args(&args).spawn() {
            Ok(_) => tracing::info!("重启服务：已拉起新进程 {}", exe.display()),
            Err(err) => {
                tracing::error!("重启服务：拉起新进程失败：{err}");
                return;
            }
        }
        std::process::exit(0);
    });
    ok_msg("重启成功")
}

// ---------------------------- 站点设置 -------------------------------------

async fn read_setting_section(state: &AppState, headers: &HeaderMap, key: &str) -> Response {
    if let Err(res) = require_admin(state, headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match site_setting::get_section(pool, key).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

async fn write_setting_section(
    state: &AppState,
    headers: &HeaderMap,
    key: &str,
    body: &Value,
) -> Response {
    if let Err(res) = require_admin(state, headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match site_setting::save_section(pool, key, body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/setting/system`
async fn manage_setting_system(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let mut system = match site_setting::get_section(pool, site_setting::SYSTEM).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };

    // favicon 以文件是否存在为准，不信任库里残留的值
    let public_dir = state.with_config(|config| config.public_dir());
    let favicon = if public_dir.join("favicon.ico").is_file() {
        let base = system
            .get("base_url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim_end_matches('/');
        format!("{base}/favicon.ico")
    } else {
        String::new()
    };
    if let Value::Object(map) = &mut system {
        map.insert("favicon".into(), Value::String(favicon));
    }

    // 可用语言：<root>/locales 下的目录名
    let locales_dir = state
        .with_config(|config| config.data_dir())
        .parent()
        .map(|root| root.join("locales"));
    let mut languages = Vec::new();
    if let Some(dir) = locales_dir {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                    if let Some(name) = entry.file_name().to_str() {
                        languages.push(name.to_string());
                    }
                }
            }
        }
    }
    languages.sort();

    ok(json!({ "system": system, "languages": languages }))
}

/// `POST /system/api/setting/system`
async fn manage_setting_system_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    if let Some(admin_url) = body.get("admin_url").and_then(Value::as_str) {
        let trimmed = admin_url.trim();
        if !trimmed.is_empty() && !trimmed.starts_with("http") {
            return fail("请填写正确的后台域名");
        }
    }
    write_setting_section(&state, &headers, site_setting::SYSTEM, &body).await
}

/// `GET /system/api/setting/content`
async fn manage_setting_content(State(state): State<AppState>, headers: HeaderMap) -> Response {
    read_setting_section(&state, &headers, site_setting::CONTENT).await
}

/// `POST /system/api/setting/content`
async fn manage_setting_content_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    write_setting_section(&state, &headers, site_setting::CONTENT, &body).await
}

/// 缩略图文件路径（与原图同目录，文件名前加 `thumb_`）。
fn thumb_path_of(location: &str) -> String {
    match location.rsplit_once('/') {
        Some((dir, name)) => format!("{dir}/thumb_{name}"),
        None => format!("thumb_{location}"),
    }
}

/// 后台批量重建附件缩略图（当前图片引擎支持 PNG/BMP）。
async fn rebuild_all_thumbs(pool: sqlx::mysql::MySqlPool, public_dir: std::path::PathBuf) {
    let content = site_setting::get_section(&pool, site_setting::CONTENT)
        .await
        .ok()
        .unwrap_or_else(|| json!({}));
    let thumb_width = content
        .get("thumb_width")
        .and_then(Value::as_u64)
        .unwrap_or(250)
        .clamp(1, 4096) as u32;
    let thumb_height = content
        .get("thumb_height")
        .and_then(Value::as_u64)
        .unwrap_or(250)
        .clamp(1, 4096) as u32;
    let crop = content
        .get("thumb_crop")
        .and_then(Value::as_i64)
        .unwrap_or(0);

    let mut last_id = 0u32;
    loop {
        let rows: Vec<(u32, String)> = match sqlx::query_as(
            "SELECT id, file_location FROM attachments WHERE is_image = 1 AND `status` = 1 AND id > ? ORDER BY id ASC LIMIT 500",
        )
        .bind(last_id)
        .fetch_all(&pool)
        .await
        {
            Ok(rows) => rows,
            Err(_) => break,
        };
        if rows.is_empty() {
            break;
        }
        last_id = rows.iter().map(|row| row.0).max().unwrap_or(last_id);

        for (id, location) in rows {
            let ext = file_ext(&location);
            if !crate::models::titleimage::image_ops::supports_extension(&ext) {
                continue;
            }
            let Ok(bytes) = tokio::fs::read(public_dir.join(&location)).await else {
                continue;
            };
            let Ok(image) = crate::models::titleimage::image_ops::decode(&bytes) else {
                continue;
            };
            let thumb = match crop {
                0 => crate::models::titleimage::image_ops::resize_fit(
                    &image,
                    thumb_width,
                    thumb_height,
                ),
                1 => crate::models::titleimage::image_ops::resize_contain(
                    &image,
                    thumb_width,
                    thumb_height,
                    [255, 255, 255, 255],
                ),
                _ => crate::models::titleimage::image_ops::resize_cover(
                    &image,
                    thumb_width,
                    thumb_height,
                ),
            };
            let Ok(encoded) = crate::models::titleimage::image_ops::encode(&thumb, &ext) else {
                continue;
            };
            let thumb_path = thumb_path_of(&location);
            let path = public_dir.join(&thumb_path);
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            if tokio::fs::write(&path, encoded).await.is_err() {
                continue;
            }
            let _ = sqlx::query("UPDATE attachments SET logo = ? WHERE id = ?")
                .bind(&thumb_path)
                .bind(id)
                .execute(&pool)
                .await;
        }
    }
}

/// `POST /system/api/setting/thumb/rebuild`
async fn manage_setting_thumb_rebuild(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let public_dir = state.with_config(|config| config.public_dir());
    let pool = pool.clone();
    tokio::spawn(async move {
        rebuild_all_thumbs(pool, public_dir).await;
    });
    ok_msg("已提交，缩略图将在后台重建（当前引擎支持 PNG/BMP）")
}

/// `GET /system/api/setting/index`
async fn manage_setting_index(State(state): State<AppState>, headers: HeaderMap) -> Response {
    read_setting_section(&state, &headers, site_setting::INDEX).await
}

/// `POST /system/api/setting/index`
async fn manage_setting_index_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    write_setting_section(&state, &headers, site_setting::INDEX, &body).await
}

/// `GET /system/api/setting/contact`
async fn manage_setting_contact(State(state): State<AppState>, headers: HeaderMap) -> Response {
    read_setting_section(&state, &headers, site_setting::CONTACT).await
}

/// `POST /system/api/setting/contact`
async fn manage_setting_contact_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    write_setting_section(&state, &headers, site_setting::CONTACT, &body).await
}

/// `GET /system/api/setting/safe`
async fn manage_setting_safe(State(state): State<AppState>, headers: HeaderMap) -> Response {
    read_setting_section(&state, &headers, site_setting::SAFE).await
}

/// `POST /system/api/setting/safe`
async fn manage_setting_safe_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    write_setting_section(&state, &headers, site_setting::SAFE, &body).await
}

/// 缓存清理时间戳（`<cache>/cache_clear.log` 的 mtime）。
fn cache_clear_time(cache_dir: &std::path::Path) -> i64 {
    std::fs::metadata(cache_dir.join("cache_clear.log"))
        .and_then(|meta| meta.modified())
        .map(|time| {
            let datetime: chrono::DateTime<chrono::Local> = time.into();
            datetime.timestamp()
        })
        .unwrap_or(0)
}

/// `GET /system/api/setting/cache`
async fn manage_setting_cache(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let cache_dir = state.with_config(|config| config.cache_dir());
    let cache_type = match site_setting::get_cache_type(pool).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    ok(json!({
        "last_update": cache_clear_time(&cache_dir),
        "cache_type": cache_type,
    }))
}

#[derive(Debug, Deserialize, Default)]
struct CacheFormBody {
    #[serde(default)]
    cache_type: String,
    #[serde(default)]
    update: bool,
}

/// `POST /system/api/setting/cache`
///
/// `update = true` 切换缓存类型；否则执行「清理缓存」（这里只记录清理时间）。
async fn manage_setting_cache_form(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CacheFormBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    if body.update {
        match site_setting::save_cache_type(pool, &body.cache_type).await {
            Ok(_) => return ok_msg("缓存类型已更新"),
            Err(err) => return failed(err),
        }
    }

    let cache_dir = state.with_config(|config| config.cache_dir());
    if let Err(err) = tokio::fs::create_dir_all(&cache_dir).await {
        return failed(err.into());
    }
    let mark = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    if let Err(err) = tokio::fs::write(cache_dir.join("cache_clear.log"), mark).await {
        return failed(err.into());
    }
    ok_msg("缓存已更新")
}

/// `GET /system/api/setting/diyfield`
async fn manage_setting_diyfield(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match site_setting::get_array(pool, site_setting::DIY_FIELDS).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/setting/diyfield`（整份覆盖，必须是数组）
async fn manage_setting_diyfield_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_array() {
        return fail("自定义字段必须是数组");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match site_setting::save_value(pool, site_setting::DIY_FIELDS, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/setting/favicon`（multipart：`file`）
async fn manage_setting_favicon_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let mut bytes: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" || name == "file1" {
            match field.bytes().await {
                Ok(value) => bytes = Some(value.to_vec()),
                Err(err) => return failed(err.into()),
            }
        } else {
            let _ = field.bytes().await;
        }
    }

    let Some(bytes) = bytes else {
        return fail("请选择要上传的图标");
    };

    let public_dir = state.with_config(|config| config.public_dir());
    if let Err(err) = tokio::fs::create_dir_all(&public_dir).await {
        return failed(err.into());
    }
    if let Err(err) = tokio::fs::write(public_dir.join("favicon.ico"), &bytes).await {
        return failed(err.into());
    }

    let system = match site_setting::get_section(pool, site_setting::SYSTEM).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let base = system
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim_end_matches('/');

    Json(json!({
        "code": 0,
        "msg": "文件上传完成",
        "data": { "favicon": format!("{base}/favicon.ico") },
    }))
    .into_response()
}

/// `POST /system/api/setting/favicon/delete`
async fn manage_setting_favicon_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let public_dir = state.with_config(|config| config.public_dir());
    let path = public_dir.join("favicon.ico");
    if path.is_file() {
        if let Err(err) = tokio::fs::remove_file(&path).await {
            return failed(err.into());
        }
    }
    ok_msg("Ico 图标已删除")
}

/// `GET /system/api/setting/banner`
async fn manage_setting_banner(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match site_setting::banner_list(pool).await {
        Ok(list) => ok(list),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/setting/banner`（新增 / 更新一条）
async fn manage_setting_banner_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match site_setting::banner_save(pool, &body, "").await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "配置已更新", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct BannerDeleteBody {
    #[serde(default)]
    id: i64,
    #[serde(default, rename = "type")]
    kind: String,
}

/// `POST /system/api/setting/banner/delete`
async fn manage_setting_banner_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BannerDeleteBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match site_setting::banner_delete(pool, body.id, &body.kind).await {
        Ok(true) => ok_msg("配置已更新"),
        Ok(false) => fail("Banner 不存在"),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/setting/migratedb`
///
/// 对齐 Go `SettingMigrateDB`：先整库备份，再幂等补齐表结构。
async fn manage_setting_migrate_db(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let dir = backup_dir(&state);
    match setting::migrate_db(pool, &dir).await {
        Ok(report) => Json(json!({
            "code": 0,
            "msg": "数据库表结构已更新",
            "data": report,
        }))
        .into_response(),
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/setting/ai`
async fn manage_setting_ai(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let ai_generate = match site_setting::get_section(pool, "ai_generate").await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let ai_chat = match site_setting::get_section(pool, site_setting::AI).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    ok(json!({
        "ai_generate": ai_generate,
        "ai_chat": ai_chat,
        "tools": [],
    }))
}

/// `POST /system/api/setting/ai`
///
/// 兼容两种提交：`{ai_generate:{...}, ai_chat:{...}}` 或直接提交一份 AI 配置。
async fn manage_setting_ai_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let ai_generate = body.get("ai_generate").filter(|value| value.is_object());
    let ai_chat = body.get("ai_chat").filter(|value| value.is_object());

    if ai_generate.is_none() && ai_chat.is_none() {
        return match site_setting::save_section(pool, site_setting::AI, &body).await {
            Ok(_) => ok_msg("配置已更新"),
            Err(err) => failed(err),
        };
    }

    if let Some(section) = ai_generate {
        if let Err(err) = site_setting::save_section(pool, "ai_generate", section).await {
            return failed(err);
        }
    }
    if let Some(section) = ai_chat {
        if let Err(err) = site_setting::save_section(pool, site_setting::AI, section).await {
            return failed(err);
        }
    }
    ok_msg("配置已更新")
}

// ---------------------------- 会员认证 -------------------------------------

#[derive(Debug, Deserialize, Default)]
struct UserAuthListQuery {
    #[serde(default)]
    page: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    keyword: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

/// `GET /system/api/plugin/userauth/list`
async fn manage_user_auth_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UserAuthListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let status = match query.status.as_deref().map(str::trim) {
        None | Some("") | Some("all") => None,
        Some(raw) => raw.parse::<i32>().ok(),
    };
    let filter = user_auth::UserAuthFilter {
        status,
        keyword: query.keyword.unwrap_or_default(),
    };
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).max(1);

    let (rows, total, stat) = match tokio::try_join!(
        user_auth::list(pool, &filter, page, page_size),
        user_auth::stat(pool)
    ) {
        Ok(((total, rows), stat)) => (rows, total, stat),
        Err(err) => return failed(err),
    };

    let (pending, passed, rejected) = stat;
    Json(json!({
        "code": 0,
        "msg": "",
        "data": {
            "list": rows,
            "total": total,
            "page": page,
            "page_size": page_size,
            "pending": pending,
            "passed": passed,
            "rejected": rejected,
        },
    }))
    .into_response()
}

/// `GET /system/api/plugin/userauth/detail?id=`
async fn manage_user_auth_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match user_auth::get(pool, query.id).await {
        Ok(Some(item)) => ok(item),
        Ok(None) => fail("认证记录不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/userauth/audit`
async fn manage_user_auth_audit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<user_auth::AuditInput>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match user_auth::audit(pool, &body, admin.id).await {
        Ok(item) => {
            Json(json!({ "code": 0, "msg": "审核完成", "data": item })).into_response()
        }
        Err(err) => model_error(err),
    }
}

// ---------------------------- 模板设计器 -----------------------------------

fn design_store(state: &AppState) -> design::DesignStore {
    let root = state
        .with_config(|config| config.data_dir())
        .parent()
        .map(|path| path.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    design::DesignStore::new(&root)
}

/// 当前站点使用的模板包名（`settings.system.template_name`，缺省 `default`）。
async fn current_template_name(pool: &sqlx::MySqlPool) -> String {
    site_setting::get_section(pool, site_setting::SYSTEM)
        .await
        .ok()
        .and_then(|value| {
            value
                .get("template_name")
                .and_then(Value::as_str)
                .map(|name| name.trim().to_string())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "default".to_string())
}

/// `GET /system/api/design/list`
async fn manage_design_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    ok(design_store(&state).list())
}

#[derive(Debug, Deserialize, Default)]
struct DesignPackageQuery {
    #[serde(default)]
    package: Option<String>,
}

/// `GET /system/api/design/info?package=`
async fn manage_design_info(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DesignPackageQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let package = match query.package.filter(|value| !value.trim().is_empty()) {
        Some(value) => value,
        None => current_template_name(pool).await,
    };
    match design_store(&state).get(package.trim(), true) {
        Ok(info) => ok(info),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct DesignSaveBody {
    #[serde(default)]
    package: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    template_type: i32,
}

/// `POST /system/api/design/save`
async fn manage_design_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DesignSaveBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let store = design_store(&state);
    match store.save_info(body.package.trim(), &body.name, body.template_type) {
        Ok(info) => {
            Json(json!({ "code": 0, "msg": "修改成功", "data": info })).into_response()
        }
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/design/use`
async fn manage_design_use(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DesignPackageQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let package = body.package.unwrap_or_default().trim().to_string();
    let info = match design_store(&state).get(&package, false) {
        Ok(info) => info,
        Err(err) => return model_error(err),
    };

    let mut system = match site_setting::get_section(pool, site_setting::SYSTEM).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    if let Value::Object(map) = &mut system {
        map.insert("template_name".into(), Value::String(info.package.clone()));
        map.insert("template_type".into(), json!(info.template_type));
    }
    match site_setting::save_section(pool, site_setting::SYSTEM, &system).await {
        Ok(_) => ok_msg("切换成功"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/design/delete`
async fn manage_design_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DesignPackageQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let package = body.package.unwrap_or_default();
    match design_store(&state).delete(package.trim()) {
        Ok(()) => ok_msg("删除成功"),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/design/download`
///
/// 把 `template/<package>` 与 `public/static/<package>` 打包为 zip，以附件返回。
async fn manage_design_download(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DesignDataBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let package = body.package.unwrap_or_default();
    let package = package.trim();
    let data = match design::market::package_zip(&design_store(&state), package) {
        Ok(data) => data,
        Err(err) => return model_error(err),
    };
    (
        [
            (
                header::CONTENT_DISPOSITION,
                format!("attachment;filename={package}.zip"),
            ),
            (header::CONTENT_TYPE, "application/zip".to_string()),
        ],
        data,
    )
        .into_response()
}

/// `GET /system/api/design/upload/check?package=`
///
/// 已存在返回 `code: 1`（与 Go 一致），不存在返回 `code: 0`。
async fn manage_design_upload_check(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DesignPackageQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let package = query.package.unwrap_or_default();
    let package = package.trim();
    if !design::valid_package(package) {
        return fail("模板包名不合法");
    }
    let store = design_store(&state);
    if store.template_root.join(package).exists() {
        Json(json!({ "code": 1, "msg": "" })).into_response()
    } else {
        Json(json!({ "code": 0, "msg": "" })).into_response()
    }
}

/// `POST /system/api/design/upload`（multipart：`file` + `cover`）
///
/// `cover = "cover"` 时覆盖同名模板，否则自动追加序号；解压时拒绝模板包内的
/// 绝对路径与 `..`（见 `design::market::install_zip`）。
async fn manage_design_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let mut file_name = String::new();
    let mut cover = String::new();
    let mut bytes: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" => {
                file_name = field.file_name().unwrap_or_default().to_string();
                match field.bytes().await {
                    Ok(value) => bytes = Some(value.to_vec()),
                    Err(err) => return failed(err.into()),
                }
            }
            "cover" => cover = field.text().await.unwrap_or_default(),
            _ => {
                let _ = field.bytes().await;
            }
        }
    }
    let Some(bytes) = bytes else {
        return fail("请选择要上传的模板压缩包");
    };
    if file_name.is_empty() {
        return fail("请选择要上传的模板压缩包");
    }
    match design::market::install_zip(&design_store(&state), &bytes, &file_name, cover.trim()) {
        Ok(package) => Json(json!({
            "code": 0,
            "msg": "上传成功",
            "data": { "package": package },
        }))
        .into_response(),
        Err(err) => model_error(err),
    }
}

/// 模板演示数据备份请求体。
#[derive(Debug, Deserialize, Default)]
struct DesignDataBody {
    #[serde(default)]
    package: Option<String>,
}

/// `POST /system/api/design/data/backup`
async fn manage_design_data_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DesignDataBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let package = body.package.unwrap_or_default();
    let package = package.trim();
    if !design::valid_package(package) {
        return fail("模板包名不合法");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let store = design_store(&state);
    let public_dir = state.with_config(|config| config.public_dir());
    match design::market::backup_data(&store, pool, package, &public_dir).await {
        Ok(()) => ok_msg("数据备份成功"),
        Err(err) => model_error(err),
    }
}

/// 模板演示数据恢复请求体。
///
/// 对齐 Go `request.DesignDataRequest`：`auto_cleanup` 为真时先自动备份整站数据，
/// 再清空现有数据；`auto_backup` 单独为真时只备份。
#[derive(Debug, Deserialize, Default)]
struct DesignDataRestoreBody {
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    auto_backup: bool,
    #[serde(default)]
    auto_cleanup: bool,
}

/// `POST /system/api/design/data/restore`
async fn manage_design_data_restore(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DesignDataRestoreBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let package = body.package.unwrap_or_default();
    let package = package.trim();
    if !design::valid_package(package) {
        return fail("模板包名不合法");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let store = design_store(&state);
    let public_dir = state.with_config(|config| config.public_dir());

    if body.auto_backup || body.auto_cleanup {
        if let Err(err) = backup::new_task("backup") {
            return model_error(err);
        }
        if let Err(err) = backup::dump(pool, &backup_dir(&state)).await {
            backup::finish_status("backup", format!("{err}"));
            return model_error(err);
        }
        backup::finish_status("backup", "备份完成");
    }
    if body.auto_cleanup {
        if let Err(err) = backup::cleanup(pool, &public_dir, false).await {
            return model_error(err);
        }
    }
    match design::market::restore_data(&store, pool, package, &public_dir).await {
        Ok(()) => ok_msg("数据初始化成功"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct DesignFileQuery {
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    hash: Option<String>,
}

/// `GET /system/api/design/file/info`
async fn manage_design_file_info(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DesignFileQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let store = design_store(&state);
    let package = query.package.unwrap_or_default();
    let path = query.path.unwrap_or_default();
    let kind = query.kind.unwrap_or_else(|| "template".into());
    match store.file_detail(package.trim(), &path, kind.trim()) {
        Ok(detail) => ok(detail),
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/design/file/histories`
async fn manage_design_file_histories(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DesignFileQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let store = design_store(&state);
    let package = query.package.unwrap_or_default();
    let path = query.path.unwrap_or_default();
    ok(store.histories(package.trim(), &path))
}

/// `GET /system/api/design/file/history/info`
async fn manage_design_file_history_info(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DesignFileQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let store = design_store(&state);
    let package = query.package.unwrap_or_default();
    let path = query.path.unwrap_or_default();
    let hash = query.hash.unwrap_or_default();
    match store.history_content(package.trim(), &path, &hash) {
        Ok(detail) => ok(detail),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct DesignFileSaveBody {
    #[serde(default)]
    package: String,
    #[serde(default)]
    path: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    rename_path: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    remark: String,
    #[serde(default)]
    update_content: bool,
}

/// `POST /system/api/design/file/save`
///
/// `update_content = true` 写正文；否则用 `rename_path` 重命名。
async fn manage_design_file_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DesignFileSaveBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let store = design_store(&state);
    let package = body.package.trim();
    let kind = if body.kind.trim().is_empty() { "template" } else { body.kind.trim() };
    let _ = &body.remark;

    if body.update_content {
        return match store.save_file(package, &body.path, kind, &body.content) {
            Ok(()) => ok_msg("修改成功"),
            Err(err) => model_error(err),
        };
    }

    let rename_path = body.rename_path.trim();
    if rename_path.is_empty() || rename_path == body.path.trim() {
        return ok_msg("修改成功");
    }
    match store.rename_file(package, &body.path, rename_path, kind) {
        Ok(()) => ok_msg("修改成功"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct DesignFileRestoreBody {
    #[serde(default)]
    package: String,
    #[serde(default)]
    path: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    hash: String,
}

/// `POST /system/api/design/file/restore`
async fn manage_design_file_restore(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DesignFileRestoreBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let store = design_store(&state);
    let kind = if body.kind.trim().is_empty() { "template" } else { body.kind.trim() };
    match store.restore_history(body.package.trim(), &body.path, &body.hash, kind) {
        Ok(()) => ok_msg("替换成功"),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/design/file/history/delete`
async fn manage_design_file_history_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DesignFileRestoreBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let store = design_store(&state);
    match store.delete_histories(body.package.trim(), &body.path, &body.hash) {
        Ok(_) => ok_msg("删除成功"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct DesignFileCopyBody {
    #[serde(default)]
    package: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    new_path: String,
    #[serde(default, rename = "type")]
    kind: String,
}

/// `POST /system/api/design/file/copy`
async fn manage_design_file_copy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DesignFileCopyBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let store = design_store(&state);
    let kind = if body.kind.trim().is_empty() { "template" } else { body.kind.trim() };
    let new_path = body.new_path.trim();
    if new_path.is_empty() {
        return fail("请填写新文件路径");
    }
    match store.copy_file(body.package.trim(), &body.path, new_path, kind) {
        Ok(()) => ok_msg("复制成功"),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/design/file/delete`
async fn manage_design_file_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DesignFileRestoreBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let store = design_store(&state);
    let kind = if body.kind.trim().is_empty() { "template" } else { body.kind.trim() };
    match store.delete_file(body.package.trim(), &body.path, kind) {
        Ok(()) => ok_msg("删除成功"),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/design/file/upload`（multipart：`file` + `package` / `path` / `name` / `type`）
async fn manage_design_file_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }

    let mut package = String::new();
    let mut dir = String::new();
    let mut name = String::new();
    let mut kind = String::new();
    let mut bytes: Option<Vec<u8>> = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let field_name = field.name().unwrap_or_default().to_string();
        match field_name.as_str() {
            "package" | "path" | "name" | "type" => {
                let value = field.text().await.unwrap_or_default();
                match field_name.as_str() {
                    "package" => package = value,
                    "path" => dir = value,
                    "name" => name = value,
                    _ => kind = value,
                }
            }
            "file" | "file1" => {
                if name.trim().is_empty() {
                    name = field.file_name().unwrap_or_default().to_string();
                }
                match field.bytes().await {
                    Ok(value) => bytes = Some(value.to_vec()),
                    Err(err) => return failed(err.into()),
                }
            }
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    let Some(bytes) = bytes else {
        return fail("无法获取上传文件");
    };

    let store = design_store(&state);
    let kind = if kind.trim().is_empty() { "template" } else { kind.trim() };
    match store.upload_file(package.trim(), dir.trim(), name.trim(), kind, &bytes) {
        Ok(path) => ok(json!({ "path": path, "size": bytes.len() })),
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/design/file/templates`
async fn manage_design_file_templates(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let package = current_template_name(pool).await;
    match design_store(&state).get(&package, true) {
        Ok(info) => ok(info.tpl_files),
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/design/docs`
///
/// Go 版的模板文档来自内置 docs 资源，Rust 侧暂未内置。
async fn manage_design_docs(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    ok(json!([]))
}

/// `GET /system/api/design/helpers`
async fn manage_design_helpers(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    ok(json!([]))
}

// ---------------------------- 版本 / 升级 ----------------------------------

/// 单个升级包上限（512MB，与 Go 一致）。
const UPGRADE_UPLOAD_MAX_SIZE: u64 = 512 * 1024 * 1024;

/// `GET /system/api/version/env`
async fn manage_version_env(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let exe = std::env::current_exe().ok();
    let exec = exe
        .as_ref()
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().to_string()))
        .unwrap_or_default();
    let exec_path = exe
        .as_ref()
        .and_then(|path| path.parent().map(|dir| dir.to_string_lossy().to_string()))
        .unwrap_or_default();

    ok(json!({
        "version": statistic::VERSION,
        "trial": statistic::TRIAL,
        "goos": std::env::consts::OS,
        "goarch": std::env::consts::ARCH,
        "exec": exec,
        "exec_path": exec_path,
        "max_size": UPGRADE_UPLOAD_MAX_SIZE,
        "skip_rules": "template/、public/、data/、uploads/、logs/、config.json、*.sh、*.bat 不会被覆盖",
    }))
}

/// 按 `.` 分段比较版本号（对齐 Go `library.VersionCompare`）。
fn version_compare(left: &str, right: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let segments = |text: &str| -> Vec<i64> {
        text.trim()
            .trim_start_matches(['v', 'V'])
            .split(|ch: char| !ch.is_ascii_digit())
            .map(|item| item.parse::<i64>().unwrap_or(0))
            .collect()
    };
    let left = segments(left);
    let right = segments(right);
    for index in 0..left.len().max(right.len()) {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        match a.cmp(&b) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

/// `GET /system/api/version/check`
///
/// 对齐 Go 版 `CheckVersion`：请求官方 `version.json`，比对当前版本；
/// 外呼失败时按「已是最新版本」返回（前端不会因此报错）。
async fn manage_version_check(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }

    // 与 Go 的 runtime.GOOS / runtime.GOARCH 口径保持一致
    let goos = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let goarch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        other => other,
    };
    let version_type = if statistic::TRIAL { "trial" } else { "" };
    let url = format!(
        "https://www.anqicms.com/downloads/version.json?goos={goos}&goarch={goarch}&type={version_type}"
    );
    let request = rustcms_ext::http::Request::get(url)
        .with_timeout(std::time::Duration::from_secs(10));
    let response = match rustcms_ext::http::send(&request).await {
        Ok(response) if response.is_success() => response,
        _ => return ok_msg("当前已是最新版本"),
    };
    let remote: Value = match serde_json::from_str(&response.text()) {
        Ok(value) => value,
        Err(_) => return ok_msg("当前已是最新版本"),
    };

    let remote_version = remote.get("version").and_then(Value::as_str).unwrap_or("");
    if version_compare(remote_version, statistic::VERSION).is_gt() {
        return Json(json!({
            "code": 0,
            "msg": "发现新版本",
            "data": remote,
        }))
        .into_response();
    }

    let trial_version = remote
        .get("trial_version")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !trial_version.is_empty() && version_compare(trial_version, statistic::VERSION).is_gt() {
        let mut data = remote.clone();
        if let Value::Object(map) = &mut data {
            map.insert("trial".into(), json!(true));
            map.insert("version".into(), json!(trial_version));
            map.insert(
                "description".into(),
                remote.get("trial_description").cloned().unwrap_or(json!("")),
            );
        }
        return Json(json!({
            "code": 0,
            "msg": "发现新试用版本",
            "data": data,
        }))
        .into_response();
    }

    ok_msg("当前已是最新版本")
}

/// 在线升级请求体（对齐 Go 的 `request.VersionData`）。
#[derive(Debug, Deserialize, Default)]
struct VersionUpgradeBody {
    #[serde(default)]
    version: String,
    #[serde(default)]
    trial: bool,
    #[serde(default)]
    trial_version: String,
}

/// 解压升级包并覆盖程序目录。
///
/// 与 Go 一致：跳过模板 / 静态资源 / 脚本 / favicon，主程序先写 `.new`，
/// 再把正在运行的程序改名备份后替换；包内路径拒绝 `..` 与绝对路径。
fn install_upgrade_zip(body: &[u8], exec_dir: &std::path::Path) -> Result<(), ModelError> {
    let archive = rustcms_ext::packer::ZipArchive::open(body.to_vec())
        .map_err(|err| ModelError::invalid(format!("升级包解压失败：{err}")))?;
    let exe = std::env::current_exe()
        .map_err(|err| ModelError::invalid(format!("取程序路径失败：{err}")))?;
    let exe_name = exe
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "rustcms".to_string());
    let exe_path = exec_dir.join(&exe_name);
    let new_exe_path = exec_dir.join(format!("{exe_name}.new"));

    for entry in archive.entries() {
        if entry.is_dir {
            continue;
        }
        let name = entry.name.replace('\\', "/");
        if name.contains("template/")
            || name.contains("static/")
            || name.contains(".sh")
            || name.contains(".bat")
            || name.contains("favicon.ico")
        {
            continue;
        }
        // 防路径穿越：拒绝绝对路径、盘符与上级目录
        if name.starts_with('/')
            || name.starts_with('.')
            || name.contains(':')
            || name.split('/').any(|segment| segment == "..")
        {
            continue;
        }
        let target = if name == exe_name {
            new_exe_path.clone()
        } else {
            exec_dir.join(&name)
        };
        let Ok(bytes) = archive.read_entry(entry) else {
            continue;
        };
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&target, bytes);
        if name == exe_name {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
            }
        }
    }

    // 替换主程序：先备份旧程序，失败时回滚
    let old_exe_path = exec_dir.join(format!("{exe_name}.old"));
    let _ = std::fs::remove_file(&old_exe_path);
    if new_exe_path.exists() && std::fs::rename(&exe_path, &old_exe_path).is_ok() {
        if std::fs::rename(&new_exe_path, &exe_path).is_ok() {
            let _ = std::fs::remove_file(&old_exe_path);
        } else {
            let _ = std::fs::rename(&old_exe_path, &exe_path);
        }
    }
    Ok(())
}

/// `POST /system/api/version/upgrade`
///
/// 从官方下载站按平台 / 架构拼接安装包地址，下载 zip 后覆盖程序目录。
async fn manage_version_upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<VersionUpgradeBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let version = if body.trial {
        body.trial_version.trim()
    } else {
        body.version.trim()
    };
    if version.is_empty() {
        return fail("缺少目标版本号");
    }

    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        other => other,
    };
    let link = if statistic::TRIAL {
        format!("https://www.anqicms.com/downloads/anqicms-trial-{os}-{arch}-v{version}.zip")
    } else {
        format!("https://www.anqicms.com/downloads/anqicms-{os}-{arch}-v{version}.zip")
    };

    let request = rustcms_ext::http::Request::get(link)
        .with_timeout(std::time::Duration::from_secs(600));
    let response = match rustcms_ext::http::send(&request).await {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!("下载升级包失败：{err:#}");
            return fail("版本更新失败");
        }
    };
    if !response.is_success() || response.body.is_empty() {
        return fail("版本更新失败");
    }

    let exec_dir = match std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.to_path_buf()))
    {
        Some(dir) => dir,
        None => return fail("版本更新失败"),
    };
    match install_upgrade_zip(&response.body, &exec_dir) {
        Ok(()) => ok_msg("系统版本已升级"),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/version/upload-upgrade`
async fn manage_version_upload_upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    fail("升级包上传解压尚未实现（需 zip/tar 库）")
}

// ---------------------------- 多站点 ---------------------------------------

#[derive(Debug, Deserialize, Default)]
struct WebsiteListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
}

/// `GET /system/api/website/list`
async fn manage_website_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WebsiteListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let name = query.name.unwrap_or_default();
    let page = query.current.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(20);
    // `websites` 表没有 base_url 列，域名存在各站点自己的 settings.system 里
    let _ = query.base_url;

    match website::list(pool, &name, page, page_size).await {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// 把 MySQL 配置里的密码抹掉（`request.SafeMysql`）。
fn safe_mysql(mut value: Value) -> Value {
    if let Value::Object(map) = &mut value {
        map.insert("password".into(), Value::String(String::new()));
    }
    value
}

/// `GET /system/api/website/info?id=`
async fn manage_website_info(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let id = if query.id == 0 { 1 } else { query.id };
    let Some(site) = (match website::get(pool, id).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    }) else {
        return Json(json!({ "code": 1, "msg": "", "data": Value::Null })).into_response();
    };

    // 站点 1 的连接信息以 config.json 为准，其它站点读表里的 JSON
    let mysql = if id == 1 {
        let config = state.with_config(|config| config.mysql.clone());
        safe_mysql(json!({
            "database": config.database,
            "user": config.user,
            "password": config.password,
            "host": config.host,
            "port": config.port,
        }))
    } else {
        site.mysql
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .map(safe_mysql)
            .unwrap_or_else(|| json!({}))
    };

    let admin_user: Option<String> =
        sqlx::query_scalar("SELECT user_name FROM admins WHERE id = 1 LIMIT 1")
            .fetch_optional(pool)
            .await
            .unwrap_or_default();
    let system = match site_setting::get_section(pool, site_setting::SYSTEM).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let initialed = state
        .with_config(|config| config.data_dir())
        .join("installed.lock")
        .is_file();

    Json(json!({
        "code": 0,
        "msg": "",
        "data": {
            "id": site.id,
            "root_path": site.root_path,
            "name": site.name,
            "status": site.status,
            "mysql": mysql,
            "admin_user": admin_user.unwrap_or_default(),
            "base_url": system.get("base_url").cloned().unwrap_or_else(|| json!("")),
            "front_url": system.get("front_url").cloned().unwrap_or_else(|| json!("")),
            "initialed": initialed,
        },
    }))
    .into_response()
}

/// `POST /system/api/website/save`
async fn manage_website_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<website::WebsiteInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match website::save(pool, &body).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "保存成功", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct WebsiteDeleteBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    remove_file: bool,
}

/// `POST /system/api/website/delete`
async fn manage_website_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<WebsiteDeleteBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    // 只删数据库记录；站点文件暂不删除（与 Go 的 removeFile 语义对齐但更保守）
    let _ = body.remove_file;
    match website::delete(pool, body.id).await {
        Ok(()) => ok_msg("删除成功"),
        Err(err) => model_error(err),
    }
}

/// 多站点切换登录请求体。
#[derive(Debug, Deserialize, Default)]
struct WebsiteLoginBody {
    #[serde(default)]
    site_id: u32,
}

/// `POST /system/api/website/login`
///
/// 生成带一次性令牌的跳转链接：默认站点直接用当前连接池，子站点用其自身数据库。
async fn manage_website_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<WebsiteLoginBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if body.site_id == 0 {
        return fail("站点不存在");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = state.with_config(|config| config.clone());
    match website::login_sub_site(pool, &config, body.site_id).await {
        Ok(link) => ok(link),
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/siteinfo`
async fn manage_current_site_info(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let system = match site_setting::get_section(pool, site_setting::SYSTEM).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let name = match website::get(pool, 1).await {
        Ok(Some(site)) => site.name,
        Ok(None) => String::new(),
        Err(err) => return failed(err),
    };
    ok(json!({
        "id": 1,
        "base_url": system.get("base_url").cloned().unwrap_or_else(|| json!("")),
        "name": name,
    }))
}

// ---------------------------- SEO 插件 -------------------------------------

/// `GET /system/api/plugin/rewrite`
async fn manage_plugin_rewrite(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::rewrite_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/rewrite`
async fn manage_plugin_rewrite_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::rewrite_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/robots`
async fn manage_plugin_robots(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let public_dir = state.with_config(|config| config.public_dir());
    ok(json!({ "robots": plugin::robots_get(&public_dir) }))
}

#[derive(Debug, Deserialize, Default)]
struct RobotsBody {
    #[serde(default)]
    robots: String,
}

/// `POST /system/api/plugin/robots`
async fn manage_plugin_robots_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RobotsBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let public_dir = state.with_config(|config| config.public_dir());
    match plugin::robots_save(&public_dir, &body.robots) {
        Ok(()) => ok_msg("配置已更新"),
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/plugin/sitemap`
async fn manage_plugin_sitemap(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let cache_dir = state.with_config(|config| config.cache_dir());
    match plugin::sitemap_get(pool, &cache_dir).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/sitemap`
async fn manage_plugin_sitemap_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let cache_dir = state.with_config(|config| config.cache_dir());
    let public_dir = state.with_config(|config| config.public_dir());

    let old_type = plugin::sitemap_get(pool, &cache_dir)
        .await
        .ok()
        .and_then(|value| value.get("type").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "txt".to_string());

    let saved = match plugin::sitemap_save(pool, &body).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };

    // 格式切换后清理另一份旧文件
    let new_type = saved
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("txt")
        .to_string();
    if old_type != new_type {
        let _ = tokio::fs::remove_file(public_dir.join(format!("sitemap.{old_type}"))).await;
    }

    ok_msg("配置已更新")
}

/// `POST /system/api/plugin/sitemap/build`
async fn manage_plugin_sitemap_build(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let public_dir = state.with_config(|config| config.public_dir());
    let cache_dir = state.with_config(|config| config.cache_dir());

    match plugin::sitemap_build(pool, &public_dir, &cache_dir).await {
        Ok(count) => {
            let payload = plugin::sitemap_payload(pool, &cache_dir)
                .await
                .unwrap_or_else(|_| json!({}));
            Json(json!({
                "code": 0,
                "msg": "Sitemap 已更新",
                "count": count,
                "data": payload,
            }))
            .into_response()
        }
        Err(err) => model_error(err),
    }
}

// ---------------------------- 友情链接 -------------------------------------

/// `GET /system/api/plugin/link/list`
async fn manage_link_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match link::list(pool).await {
        Ok(list) => ok(list),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/link/detail`
async fn manage_link_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<link::LinkInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match link::save(pool, &body).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "链接已更新", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/link/delete`
async fn manage_link_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match link::delete(pool, body.id).await {
        Ok(true) => ok_msg("链接已删除"),
        Ok(false) => fail("链接不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/link/check`
///
/// 抓取对方页面，检查是否回链本站（对齐 Go `PluginLinkCheck`）。
async fn manage_link_check(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match link::check(pool, body.id).await {
        Ok(item) => ok(item),
        Err(err) => model_error(err),
    }
}

// ---------------------------- 关键词 ---------------------------------------

/// `GET /system/api/plugin/keyword/setting`
async fn manage_keyword_setting(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match collector::keyword_setting_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/keyword/setting`
async fn manage_keyword_setting_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match collector::keyword_setting_save(pool, &body).await {
        Ok(_) => ok_msg("保存成功"),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct KeywordListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    title: Option<String>,
}

/// `GET /system/api/plugin/keyword/list`
async fn manage_keyword_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<KeywordListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let keyword = query.title.unwrap_or_default();
    match collector::list_keywords(
        pool,
        &keyword,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct KeywordSaveBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    title: String,
    #[serde(default)]
    category_id: u32,
}

/// `POST /system/api/plugin/keyword/detail`
///
/// `id > 0` 更新单条；否则按换行批量新增（自动去重）。
async fn manage_keyword_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<KeywordSaveBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let saved = if body.id > 0 {
        match collector::update_keyword(pool, body.id, &body.title, body.category_id).await {
            Ok(item) => vec![item],
            Err(err) => return model_error(err),
        }
    } else {
        match collector::save_keywords_batch(pool, &body.title, body.category_id).await {
            Ok(list) => list,
            Err(err) => return model_error(err),
        }
    };

    Json(json!({ "code": 0, "msg": "关键词已更新", "data": saved })).into_response()
}

#[derive(Debug, Deserialize, Default)]
struct KeywordDeleteBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    ids: Vec<u32>,
    #[serde(default)]
    all: bool,
}

/// `POST /system/api/plugin/keyword/delete`
async fn manage_keyword_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<KeywordDeleteBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let result = if body.id > 0 {
        collector::delete_keyword(pool, body.id).await.map(|_| ())
    } else if !body.ids.is_empty() {
        collector::delete_keywords(pool, &body.ids).await.map(|_| ())
    } else if body.all {
        collector::delete_all_keywords(pool).await.map(|_| ())
    } else {
        Ok(())
    };

    match result {
        Ok(()) => ok_msg("删除操作已执行"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/keyword/export`
async fn manage_keyword_export(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match collector::all_keywords(pool).await {
        Ok(list) => {
            let content: Vec<Value> = list
                .iter()
                .map(|item| json!([item.title, item.category_id]))
                .collect();
            ok(json!({
                "header": ["title", "category_id"],
                "content": content,
            }))
        }
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/keyword/import`（multipart：`file`）
///
/// 每行一个关键词，也支持 `title, category_id` 两列。
async fn manage_keyword_import(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let mut bytes: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" || name == "file1" {
            match field.bytes().await {
                Ok(value) => bytes = Some(value.to_vec()),
                Err(err) => return failed(err.into()),
            }
        } else {
            let _ = field.bytes().await;
        }
    }

    let Some(bytes) = bytes else {
        return fail("请选择要导入的文件");
    };
    let text = String::from_utf8_lossy(&bytes).to_string();

    let mut imported = 0usize;
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, ',');
        let title = parts.next().unwrap_or_default().trim();
        if title.is_empty() {
            continue;
        }
        let category_id = parts
            .next()
            .map(str::trim)
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        match collector::save_keywords_batch(pool, title, category_id).await {
            Ok(list) => imported += list.len(),
            Err(err) => return model_error(err),
        }
    }

    ok(json!({ "count": imported }))
}

// ---------------------------- 标签 -----------------------------------------

/// 标签响应体：库字段 + 正文 / 自定义字段 / 分类标题 / 前台链接。
fn tag_payload(
    item: &tag::Tag,
    content: &str,
    extra: Option<Value>,
    category_title: &str,
) -> Value {
    let mut value = serde_json::to_value(item).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut value {
        map.insert("content".into(), Value::String(content.to_string()));
        map.insert("extra".into(), extra.unwrap_or_else(|| json!({})));
        map.insert(
            "category_title".into(),
            Value::String(category_title.to_string()),
        );
        map.insert(
            "link".into(),
            Value::String(format!("/tag/{}", item.url_token)),
        );
    }
    value
}

#[derive(Debug, Deserialize, Default)]
struct TagListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    category_id: Option<u32>,
}

/// `GET /system/api/plugin/tag/list`
async fn manage_tag_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TagListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let title = query.title.unwrap_or_default();
    let category_id = query.category_id.unwrap_or(0);
    let page = query.current.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(20);

    let (total, rows) =
        match tag::list(pool, &title, category_id, page, page_size).await {
            Ok(value) => value,
            Err(err) => return failed(err),
        };
    let titles = tag::category_titles(pool).await.unwrap_or_default();

    let list: Vec<Value> = rows
        .iter()
        .map(|item| {
            let category_title = titles
                .get(&item.category_id)
                .cloned()
                .unwrap_or_default();
            tag_payload(item, "", None, &category_title)
        })
        .collect();

    Json(json!({
        "code": 0,
        "msg": "",
        "total": total,
        "data": list,
    }))
    .into_response()
}

/// `GET /system/api/plugin/tag/detail?id=`
async fn manage_tag_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(item) = (match tag::get(pool, query.id).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    }) else {
        return fail("标签不存在");
    };
    let (content, extra) = tag::content(pool, item.id).await.unwrap_or_default();
    let category_title = tag::category_titles(pool)
        .await
        .ok()
        .and_then(|map| map.get(&item.category_id).cloned())
        .unwrap_or_default();
    ok(tag_payload(&item, &content, extra, &category_title))
}

/// `POST /system/api/plugin/tag/detail`
async fn manage_tag_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<tag::TagInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match tag::save(pool, &input).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "保存成功", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/tag/delete`
async fn manage_tag_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match tag::delete(pool, body.id).await {
        Ok(true) => ok_msg("标签已删除"),
        Ok(false) => fail("标签不存在"),
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/plugin/tag/fields`
async fn manage_tag_fields(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match tag::fields(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/tag/fields`
async fn manage_tag_fields_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match tag::save_fields(pool, &body).await {
        Ok(()) => ok_msg("保存成功"),
        Err(err) => model_error(err),
    }
}

// ---------------------------- 301 跳转 -------------------------------------

#[derive(Debug, Deserialize, Default)]
struct RedirectListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    from_url: Option<String>,
}

/// `GET /system/api/plugin/redirect/list`
async fn manage_redirect_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RedirectListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let from_url = query.from_url.unwrap_or_default();
    match redirect::list(
        pool,
        &from_url,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/redirect/detail`
async fn manage_redirect_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<redirect::RedirectInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match redirect::save(pool, &input).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "链接已更新", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/redirect/delete`
async fn manage_redirect_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match redirect::delete(pool, body.id).await {
        Ok(true) => ok_msg("删除操作已执行"),
        Ok(false) => fail("链接不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/redirect/import`（multipart：`file`）
async fn manage_redirect_import(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let mut bytes: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" || name == "file1" {
            match field.bytes().await {
                Ok(value) => bytes = Some(value.to_vec()),
                Err(err) => return failed(err.into()),
            }
        } else {
            let _ = field.bytes().await;
        }
    }

    let Some(bytes) = bytes else {
        return fail("请选择要导入的文件");
    };
    let text = String::from_utf8_lossy(&bytes).to_string();
    match redirect::import(pool, &text).await {
        Ok(count) => ok(json!({ "count": count })),
        Err(err) => model_error(err),
    }
}

// ---------------------------- 锚文本 ---------------------------------------

/// 站点前台地址（`front_url` 优先，回退 `base_url`，去掉尾部 `/`）。
async fn site_front_url(pool: &sqlx::MySqlPool) -> String {
    let system = site_setting::get_section(pool, site_setting::SYSTEM)
        .await
        .unwrap_or_else(|_| json!({}));
    system
        .get("front_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| system.get("base_url").and_then(Value::as_str))
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string()
}

#[derive(Debug, Deserialize, Default)]
struct AnchorListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    keyword: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

/// `GET /system/api/plugin/anchor/list`
async fn manage_anchor_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AnchorListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    // `title` 是兼容参数，非空时覆盖 keyword
    let keyword = query
        .title
        .filter(|value| !value.trim().is_empty())
        .or(query.keyword)
        .unwrap_or_default();

    match anchor::list(
        pool,
        &keyword,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/anchor/detail?id=`
async fn manage_anchor_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match anchor::get(pool, query.id).await {
        Ok(Some(item)) => ok(item),
        Ok(None) => fail("锚文本不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/anchor/detail`
async fn manage_anchor_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<anchor::AnchorInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let front = site_front_url(pool).await;
    match anchor::save(pool, &input, &front).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "链接已更新", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct AnchorDeleteBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    ids: Vec<u32>,
}

/// `POST /system/api/plugin/anchor/delete`
async fn manage_anchor_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AnchorDeleteBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let result = if body.id > 0 {
        anchor::delete(pool, body.id).await.map(|_| ())
    } else if !body.ids.is_empty() {
        anchor::delete_many(pool, &body.ids).await.map(|_| ())
    } else {
        Ok(())
    };
    match result {
        Ok(()) => ok_msg("删除操作已执行"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/anchor/replace`
///
/// `id > 0` 只替换该条，否则替换全部启用的锚文本。
async fn manage_anchor_replace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let single = if body.id > 0 {
        match anchor::get(pool, body.id).await {
            Ok(Some(item)) => Some(item),
            Ok(None) => return fail("锚文本不存在"),
            Err(err) => return failed(err),
        }
    } else {
        None
    };

    match anchor::replace_anchor(pool, single.as_ref()).await {
        Ok(count) => Json(json!({
            "code": 0,
            "msg": "锚文本替换任务已执行",
            "data": { "articles": count },
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/anchor/export`
async fn manage_anchor_export(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match anchor::all(pool).await {
        Ok(list) => {
            let content: Vec<Value> = list
                .iter()
                .map(|item| json!([item.title, item.link, item.weight]))
                .collect();
            ok(json!({
                "header": ["title", "link", "weight"],
                "content": content,
            }))
        }
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/anchor/import`（multipart：`file`）
async fn manage_anchor_import(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let mut bytes: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" || name == "file1" {
            match field.bytes().await {
                Ok(value) => bytes = Some(value.to_vec()),
                Err(err) => return failed(err.into()),
            }
        } else {
            let _ = field.bytes().await;
        }
    }

    let Some(bytes) = bytes else {
        return fail("请选择要导入的文件");
    };
    let text = String::from_utf8_lossy(&bytes).to_string();
    let front = site_front_url(pool).await;
    match anchor::import(pool, &text, &front).await {
        Ok(count) => ok(json!({ "count": count })),
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/plugin/anchor/setting`
async fn manage_anchor_setting(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match anchor::setting_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/anchor/setting`
async fn manage_anchor_setting_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match anchor::setting_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct AnchorFromTitleBody {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    ids: Vec<u32>,
}

/// `POST /system/api/plugin/anchor/add-from-title`
async fn manage_anchor_add_from_title(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AnchorFromTitleBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let front = site_front_url(pool).await;
    match anchor::add_from_titles(pool, body.kind.trim(), &body.ids, &front).await {
        Ok(count) => ok(json!({ "count": count })),
        Err(err) => model_error(err),
    }
}

// ---------------------------- 全站替换 -------------------------------------

#[derive(Debug, Deserialize, Default)]
struct ReplacePairBody {
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: String,
}

#[derive(Debug, Deserialize, Default)]
struct ReplaceValuesBody {
    #[serde(default)]
    replace_tag: bool,
    #[serde(default)]
    places: Vec<String>,
    #[serde(default)]
    keywords: Vec<ReplacePairBody>,
}

/// `POST /system/api/plugin/replace/values`
async fn manage_replace_values(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ReplaceValuesBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if body.places.is_empty() {
        return fail("请选择要替换的位置");
    }
    if body.keywords.is_empty() {
        return fail("替换关键词不能为空");
    }

    let pairs: Vec<(String, String)> = body
        .keywords
        .iter()
        .filter(|pair| !pair.from.trim().is_empty())
        .map(|pair| (pair.from.clone(), pair.to.clone()))
        .collect();
    if pairs.is_empty() {
        return fail("替换关键词不能为空");
    }

    match replace::replace_values(pool, &body.places, &pairs, body.replace_tag).await {
        Ok(total) => Json(json!({
            "code": 0,
            "msg": "替换完成",
            "data": total,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

// ---------------------------- 纯配置类插件 ---------------------------------

/// `GET /system/api/plugin/timefactor/setting`
async fn manage_timefactor_setting(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::timefactor_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/timefactor/setting`
async fn manage_timefactor_setting_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::timefactor_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/jsonld/config`
async fn manage_jsonld_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::jsonld_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/jsonld/config`
async fn manage_jsonld_config_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::jsonld_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/google/setting`
async fn manage_google_setting(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::google_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/google/setting`
async fn manage_google_setting_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::google_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

// ---------------------------- 数据迁移 -------------------------------------

/// 站点根目录（`config.json` 所在目录），`clientFiles/` 在这里。
fn site_root(state: &AppState) -> std::path::PathBuf {
    state
        .with_config(|config| config.data_dir())
        .parent()
        .map(|path| path.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// `GET /system/api/plugin/transfer/task`
///
/// Go 版的迁移任务只存在内存里（重启即丢），这里没有持久化，恒返回 `null`。
async fn manage_transfer_task(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    ok(Value::Null)
}

#[derive(Debug, Deserialize, Default)]
struct TransferProviderBody {
    #[serde(default)]
    provider: String,
}

/// `POST /system/api/plugin/transfer/download`
///
/// 把内置的 `clientFiles/<provider>2rustcms.php`（`train` 是 `.wpm`）作为附件发给用户。
async fn manage_transfer_download(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TransferProviderBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let provider = body.provider.trim();
    if provider.is_empty() {
        return fail("provider is required");
    }
    if !transfer::valid_provider(provider) {
        return fail("invalid provider name");
    }

    let root = site_root(&state);
    let Some(path) = transfer::client_file(&root, provider) else {
        return fail("客户端文件不存在");
    };
    let name = transfer::client_file_name(provider);
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(err) => return failed(err.into()),
    };

    let headers = [
        (
            header::CONTENT_TYPE,
            "application/octet-stream".to_string(),
        ),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        ),
    ];
    (headers, bytes).into_response()
}

/// `POST /system/api/plugin/transfer/create`
///
/// 校验迁移参数并请求旧站 `a=config` 确认客户端文件可用（对齐 Go `CreateTransferTask`）。
async fn manage_transfer_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<transfer::TransferTask>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    match transfer::create(body).await {
        Ok(task) => ok(task),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/transfer/modules`
async fn manage_transfer_modules(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    fail("无可用迁移任务")
}

/// `POST /system/api/plugin/transfer/start`
async fn manage_transfer_start(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    fail("无可用迁移任务")
}

// ---------------------------- 多语言站点 -----------------------------------

/// `GET /system/api/plugin/multilang/config`
async fn manage_multilang_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match multilang::config_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/multilang/config`
async fn manage_multilang_config_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match multilang::config_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct MultiLangSitesQuery {
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

/// `GET /system/api/plugin/multilang/sites`
async fn manage_multilang_sites(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<MultiLangSitesQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    // `type=multi` 且配置是 single 时不返回数据（与 Go 一致）
    if query.kind.as_deref() == Some("multi") {
        let config = match multilang::config_get(pool).await {
            Ok(value) => value,
            Err(err) => return failed(err),
        };
        if config.get("site_type").and_then(Value::as_str) == Some("single") {
            return Json(json!({ "code": 0, "msg": "" })).into_response();
        }
    }

    match multilang::sites(pool).await {
        Ok(list) => ok(list),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/multilang/validsites`
async fn manage_multilang_valid_sites(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match multilang::valid_sites(pool).await {
        Ok(list) => ok(list),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/multilang/site/save`
async fn manage_multilang_site_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<multilang::SubSiteInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match multilang::site_save(pool, &input).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "保存成功", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct MultiLangSiteRemoveBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    language: String,
}

/// `POST /system/api/plugin/multilang/site/remove`
async fn manage_multilang_site_remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<MultiLangSiteRemoveBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match multilang::site_remove(pool, body.id, &body.language).await {
        Ok(true) => ok_msg("删除成功"),
        Ok(false) => fail("子站点不存在"),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/multilang/site/sync`
async fn manage_multilang_site_sync(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<multilang::SiteSyncInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let mysql = state.with_config(|config| config.mysql.clone());
    match multilang::site_sync(pool, &mysql, input.id, input.focus).await {
        Ok(report) => Json(json!({ "code": 0, "msg": "同步完成", "data": report })).into_response(),
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/plugin/multilang/site/sync/status`
async fn manage_multilang_sync_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    Json(json!({
        "code": 0,
        "msg": "没有正在进行的任务",
        "data": Value::Null,
    }))
    .into_response()
}

#[derive(Debug, Deserialize, Default)]
struct TranslateCacheQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    lang: Option<String>,
    #[serde(default)]
    uri: Option<String>,
}

/// `GET /system/api/plugin/multilang/site/html/logs`
async fn manage_multilang_html_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TranslateCacheQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let cache_dir = state.with_config(|config| config.cache_dir());
    let (items, total) = multilang::html_logs(
        &cache_dir,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    );
    Json(json!({
        "code": 0,
        "msg": "",
        "total": total,
        "data": items,
    }))
    .into_response()
}

/// `GET /system/api/plugin/multilang/site/html/caches`
async fn manage_multilang_html_caches(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TranslateCacheQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let cache_dir = state.with_config(|config| config.cache_dir());
    let (items, total) = multilang::html_caches(
        &cache_dir,
        query.lang.as_deref().unwrap_or(""),
        query.uri.as_deref().unwrap_or(""),
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    );
    Json(json!({
        "code": 0,
        "msg": "",
        "total": total,
        "data": items,
    }))
    .into_response()
}

#[derive(Debug, Deserialize, Default)]
struct TranslateCacheRemoveBody {
    #[serde(default)]
    uris: Vec<String>,
    #[serde(default)]
    all: bool,
}

/// `POST /system/api/plugin/multilang/site/html/cache/remove`
async fn manage_multilang_html_cache_remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TranslateCacheRemoveBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let cache_dir = state.with_config(|config| config.cache_dir());
    let removed = multilang::html_cache_remove(&cache_dir, &body.uris, body.all);
    ok(json!({ "removed": removed }))
}

// ---------------------------- 翻译插件 ---------------------------------

/// `GET /system/api/plugin/translate/config`
async fn manage_translate_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match translate::config_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/translate/config`
///
/// 字段级 merge：只覆盖请求体里出现的字段（Go `MergeJSONPayload`）。
async fn manage_translate_config_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Value>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match translate::config_save(pool, &body).await {
        Ok(_) => {
            log_action(pool, &current, &client_ip(&headers, peer), "更新翻译配置").await;
            ok_msg("配置已更新")
        }
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/translate/logs`
async fn manage_translate_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WithdrawListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match translate::log_list(pool, query.current.unwrap_or(1), query.page_size.unwrap_or(20)).await
    {
        Ok((total, items)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": items,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct TranslateTextLogQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    text: String,
    #[serde(default)]
    translated: String,
}

/// `GET /system/api/plugin/translate/log/texts`
///
/// 支持按原文 / 译文前缀筛选（Go `PluginGetTranslateTextLog`）。
async fn manage_translate_text_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TranslateTextLogQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match translate::text_log_list(
        pool,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
        &query.text,
        &query.translated,
    )
    .await
    {
        Ok((total, items)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": items,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct TranslateTextRemoveBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    all: bool,
}

/// `POST /system/api/plugin/translate/log/text/remove`
async fn manage_translate_text_remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<TranslateTextRemoveBody>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match translate::text_log_remove(pool, body.id, body.all).await {
        Ok(()) => {
            log_action(pool, &current, &client_ip(&headers, peer), "删除翻译记录").await;
            ok_msg("删除成功")
        }
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/translate/log/text/save`
///
/// 修正自动翻译：更新文本翻译日志，并把 HTML 缓存里的旧译文一并替换。
async fn manage_translate_text_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(input): Json<translate::TextLogInput>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let cache_dir = state.with_config(|config| config.cache_dir());
    match translate::text_log_save(pool, &input, &cache_dir).await {
        Ok(saved) => {
            log_action(pool, &current, &client_ip(&headers, peer), "保存文本翻译").await;
            Json(json!({ "code": 0, "msg": "保存成功", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

// ---------------------------- llms.txt 插件 ----------------------------

/// `GET /system/api/plugin/llms/status`
///
/// 无任务 = 已完成（Go 原实现把「已完成」回成失败，这里是修正后的语义）。
async fn manage_llms_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    match llms::build_status() {
        None => Json(json!({
            "code": 0,
            "msg": "Finished",
            "data": { "finished": true, "building": false },
        }))
        .into_response(),
        Some(status) => ok(status),
    }
}

/// `POST /system/api/plugin/llms/build`
async fn manage_llms_build(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let public_dir = state.with_config(|config| config.public_dir());
    match llms::build(pool, &public_dir).await {
        Ok(_) => {
            log_action(pool, &current, &client_ip(&headers, peer), "生成 llms.txt").await;
            ok_msg("llms.txt 正在生成，请稍后")
        }
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/plugin/llms/setting`
async fn manage_llms_setting(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let public_dir = state.with_config(|config| config.public_dir());
    match llms::setting_get(pool, &public_dir).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/llms/setting`
async fn manage_llms_setting_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Value>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let public_dir = state.with_config(|config| config.public_dir());
    match llms::setting_save(pool, &body, &public_dir).await {
        Ok(_) => {
            log_action(pool, &current, &client_ip(&headers, peer), "更新 LLMs 配置").await;
            ok_msg("配置已更新")
        }
        Err(err) => failed(err),
    }
}

// ---------------------------- 图片预览公共工具 -----------------------------

/// 读 `public` 下的静态资源；路径含 `..` 或读取失败时返回 `None`。
async fn read_public_asset(public_dir: &std::path::Path, relative: &str) -> Option<Vec<u8>> {
    let relative = relative.trim().trim_start_matches('/');
    if relative.is_empty() || relative.contains("..") {
        return None;
    }
    tokio::fs::read(public_dir.join(relative)).await.ok()
}

/// 取小写扩展名（不含点）。
fn file_ext(relative: &str) -> String {
    relative
        .rsplit_once('.')
        .map(|(_, ext)| ext.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

/// 按扩展名推断图片 MIME。
fn image_mime_of(relative: &str) -> &'static str {
    match file_ext(relative).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// 基础 base64 编码（不引入新依赖）。
fn image_base64(data: &[u8]) -> String {
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

/// SVG 文本转义。
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// 校验并规范化 `#rrggbb` 颜色；非法时回落到默认值。
fn hex_color_or(raw: &str, default: &str) -> String {
    let value = raw.trim();
    let valid = value.len() == 7
        && value.starts_with('#')
        && value[1..].chars().all(|ch| ch.is_ascii_hexdigit());
    if valid {
        value.to_ascii_lowercase()
    } else {
        default.to_string()
    }
}

/// 按估算字宽折行（中文按 1 个字号，ASCII 按 0.5 个字号）。
fn wrap_title_text(text: &str, size: usize, max_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            lines.push(std::mem::take(&mut current));
            used = 0;
            continue;
        }
        let advance = if ch.is_ascii() {
            (size / 2).max(1)
        } else {
            size
        };
        if used + advance > max_width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(ch);
        used += advance;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// 标题的 32 位数字摘要（对齐 Go `sumTitle`）。
fn titleimage_title_sum(title: &str) -> String {
    attachment::md5_hex(title.as_bytes())
        .bytes()
        .map(|byte| char::from(if byte > 57 { byte - 49 } else { byte }))
        .collect()
}

/// 无背景图时按标题生成深色背景（对齐 Go `RandColor` / `RandDeepColor`）。
fn titleimage_bg_color(title: &str) -> String {
    let sum = titleimage_title_sum(title);
    let head: i64 = sum.get(0..9).and_then(|part| part.parse().ok()).unwrap_or(0);
    let tail: i64 = sum
        .get(22..31)
        .and_then(|part| part.parse().ok())
        .unwrap_or(0);
    let red = head % 255;
    let green = (head / 1000) % 255;
    let blue = if red + green > 400 {
        0
    } else {
        (400 - red - green).min(255)
    };
    let increase = 30 + tail % 255;
    let red = (red - increase).abs().min(255);
    let green = (green - increase).abs().min(255);
    let blue = (blue - increase).abs().min(255);
    format!("#{red:02x}{green:02x}{blue:02x}")
}

/// 背景噪声（对应 Go 的 `Noise` 开关）。
fn titleimage_noise(title: &str, width: u32, height: u32) -> String {
    let sum = titleimage_title_sum(title);
    let bytes = sum.as_bytes();
    let mut out = String::new();
    for index in 0..24usize {
        let a = bytes[(index * 5) % bytes.len()] as u32;
        let b = bytes[(index * 7 + 3) % bytes.len()] as u32;
        let c = bytes[(index * 11 + 5) % bytes.len()] as u32;
        let x = if width > 8 { a % (width - 8) } else { 0 };
        let y = if height > 8 { b % (height - 8) } else { 0 };
        out.push_str(&format!(
            r##"<rect x="{x}" y="{y}" width="8" height="8" fill="#{:02x}{:02x}{:02x}" opacity="0.35"/>"##,
            c * 8 % 256,
            a * 8 % 256,
            b * 8 % 256
        ));
    }
    out
}

/// 水印素材等比缩放到不超过 `max_width × max_height`。
fn fit_mark(source_width: u32, source_height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if source_width == 0 || source_height == 0 {
        return (max_width, max_height);
    }
    let scale = (max_width as f64 / source_width as f64)
        .min(max_height as f64 / source_height as f64);
    (
        ((source_width as f64 * scale).round() as u32).max(1),
        ((source_height as f64 * scale).round() as u32).max(1),
    )
}

/// 水印位置：5 居中、1 左上、3 右上、7 左下，其它按右下。
fn watermark_position(
    position: i64,
    canvas_width: u32,
    canvas_height: u32,
    width: u32,
    height: u32,
    edge: u32,
) -> (i64, i64) {
    let right = canvas_width as i64 - edge as i64 - width as i64;
    let bottom = canvas_height as i64 - edge as i64 - height as i64;
    match position {
        5 => (
            (canvas_width.saturating_sub(width) / 2) as i64,
            (canvas_height.saturating_sub(height) / 2) as i64,
        ),
        1 => (edge as i64, edge as i64),
        3 => (right, edge as i64),
        7 => (edge as i64, bottom),
        _ => (right, bottom),
    }
}

// ---------------------------- 标题生成图片 ---------------------------------

/// `GET /system/api/plugin/titleimage/config`
async fn manage_titleimage_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match titleimage::config_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/titleimage/config`
async fn manage_titleimage_config_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match titleimage::config_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// 标题图片预览的查询参数。
#[derive(Debug, Deserialize, Default)]
struct TitleImagePreviewQuery {
    /// 预览文字；缺省时使用欢迎语。
    #[serde(default)]
    text: Option<String>,
}

/// 渲染标题图片 SVG（预览与批量生成共用）。
fn render_titleimage_svg(
    config: &Value,
    text: &str,
    font_data: Option<&Vec<u8>>,
    background: Option<(&str, &[u8])>,
) -> String {
    let width = config
        .get("width")
        .and_then(Value::as_u64)
        .unwrap_or(800)
        .clamp(1, 4096) as u32;
    let height = config
        .get("height")
        .and_then(Value::as_u64)
        .unwrap_or(600)
        .clamp(1, 4096) as u32;
    let font_size = config
        .get("font_size")
        .and_then(Value::as_u64)
        .unwrap_or(32)
        .clamp(8, 200) as usize;
    let font_color = hex_color_or(
        config.get("font_color").and_then(Value::as_str).unwrap_or(""),
        "#ffffff",
    );
    let font_bg_color = hex_color_or(
        config
            .get("font_bg_color")
            .and_then(Value::as_str)
            .unwrap_or(""),
        "",
    );
    let noise = config.get("noise").and_then(Value::as_bool).unwrap_or(false);

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">"#
    );
    if let Some(bytes) = font_data {
        svg.push_str(&format!(
            r#"<defs><style>@font-face{{font-family:'TitleImageFont';src:url(data:font/ttf;base64,{}) format('truetype');}}</style></defs>"#,
            image_base64(bytes)
        ));
    }
    match background {
        Some((relative, bytes)) => {
            svg.push_str(&format!(
                r#"<image x="0" y="0" width="{width}" height="{height}" preserveAspectRatio="xMidYMid slice" href="data:{};base64,{}"/>"#,
                image_mime_of(relative),
                image_base64(bytes)
            ));
        }
        None => {
            let bg_color = titleimage_bg_color(text);
            svg.push_str(&format!(
                r#"<rect x="0" y="0" width="{width}" height="{height}" fill="{bg_color}"/>"#
            ));
            if noise {
                svg.push_str(&titleimage_noise(text, width, height));
            }
        }
    }

    // 与 Go `drawTitle` 一致：按文字数量自适应字号，超宽自动折行并居中。
    let max_text_width = (width.saturating_sub(100) as usize).max(1);
    let real_size = (max_text_width / text.chars().count().max(1))
        .clamp(font_size.min(100).max(1), 100);
    let lines = wrap_title_text(text, real_size, max_text_width);
    let line_count = lines.len() as i64;
    let start_y = height as i64 / 2
        - ((real_size as f64 / 1.5) as i64
            + ((line_count - 1) as f64 * real_size as f64 * 1.6) as i64 / 2)
        + real_size as i64;

    if !font_bg_color.is_empty() {
        let bg_y = start_y - real_size as i64 - real_size as i64 / 2;
        let bg_y1 = start_y
            + (real_size as f64 * 1.6 * (line_count - 1) as f64) as i64
            + (real_size as f64 * 1.6 / 2.0) as i64;
        let bg_height = (bg_y1 - bg_y).max(1);
        svg.push_str(&format!(
            r#"<rect x="0" y="{bg_y}" width="{width}" height="{bg_height}" fill="{font_bg_color}"/>"#
        ));
    }

    let font_family = if font_data.is_some() {
        "TitleImageFont, sans-serif"
    } else {
        "sans-serif"
    };
    for (index, line) in lines.iter().enumerate() {
        let y = start_y + (index as f64 * 1.6 * real_size as f64) as i64;
        svg.push_str(&format!(
            r#"<text x="{}" y="{y}" fill="{font_color}" font-size="{real_size}" font-family="{font_family}" text-anchor="middle">{}</text>"#,
            width / 2,
            xml_escape(line)
        ));
    }
    svg.push_str("</svg>");
    svg
}

/// `GET /system/api/plugin/titleimage/preview`
///
/// 对齐 Go 版 `PluginTitleImagePreview`：按当前配置生成一张可直接查看的
/// `data:image/svg+xml;base64,` 图片（内嵌字体、背景图、噪声）。
async fn manage_titleimage_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TitleImagePreviewQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = match titleimage::config_get(pool).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let public_dir = state.with_config(|config| config.public_dir());
    let text = query
        .text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("欢迎使用安企CMS")
        .to_string();

    // 字体资源（可选）；存在时以 @font-face 内嵌，保证预览与生成效果一致。
    let font_path = config.get("font_path").and_then(Value::as_str).unwrap_or("");
    let font_data = read_public_asset(&public_dir, font_path).await;

    // 背景图（可选）；与 Go 版本一致，多张时随机取一张。
    let mut background = None;
    if let Some(items) = config.get("bg_images").and_then(Value::as_array) {
        if !items.is_empty() {
            use rand::Rng;
            let index = rand::thread_rng().gen_range(0..items.len());
            let relative = items[index].as_str().unwrap_or("");
            background = read_public_asset(&public_dir, relative)
                .await
                .map(|bytes| (relative.to_string(), bytes));
        }
    }

    let svg = render_titleimage_svg(
        &config,
        &text,
        font_data.as_ref(),
        background
            .as_ref()
            .map(|(relative, bytes)| (relative.as_str(), bytes.as_slice())),
    );

    ok(json!(format!(
        "data:image/svg+xml;base64,{}",
        image_base64(svg.as_bytes())
    )))
}

/// `POST /system/api/plugin/titleimage/upload`（multipart：`name` + `file`）
async fn manage_titleimage_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let mut kind = String::new();
    let mut file_name = String::new();
    let mut bytes: Option<Vec<u8>> = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "name" => kind = field.text().await.unwrap_or_default(),
            "file" | "file1" => {
                file_name = field.file_name().unwrap_or_default().to_string();
                match field.bytes().await {
                    Ok(value) => bytes = Some(value.to_vec()),
                    Err(err) => return failed(err.into()),
                }
            }
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    if kind != "font_path" && kind != "bg_image" {
        return fail("文件名无效");
    }
    let Some(bytes) = bytes else {
        return fail("请选择要上传的文件");
    };

    let public_dir = state.with_config(|config| config.public_dir());
    let path = match titleimage::upload(&public_dir, kind.trim(), &file_name, &bytes) {
        Ok(path) => path,
        Err(err) => return model_error(err),
    };

    // 字体路径需要写回配置
    if kind == "font_path" {
        if let Err(err) = titleimage::set_font_path(pool, &path).await {
            return failed(err);
        }
    }

    ok(json!(path))
}

/// 后台批量生成标题图片：为无图文档生成 SVG 并写回 `images`。
async fn generate_all_title_images(pool: sqlx::mysql::MySqlPool, public_dir: std::path::PathBuf) {
    let Ok(config) = titleimage::config_get(&pool).await else {
        return;
    };
    let font_path = config.get("font_path").and_then(Value::as_str).unwrap_or("");
    let font_data = read_public_asset(&public_dir, font_path).await;
    let bg_images: Vec<String> = config
        .get("bg_images")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let mut last_id = 0i64;
    loop {
        let rows: Vec<(i64, String, Option<String>)> = match sqlx::query_as(
            "SELECT id, title, images FROM archives WHERE id > ? ORDER BY id ASC LIMIT 500",
        )
        .bind(last_id)
        .fetch_all(&pool)
        .await
        {
            Ok(rows) => rows,
            Err(_) => break,
        };
        if rows.is_empty() {
            break;
        }
        last_id = rows.iter().map(|row| row.0).max().unwrap_or(last_id);

        for (id, title, images) in rows {
            if !json_string_list(images.as_deref()).is_empty() || title.trim().is_empty() {
                continue;
            }
            let background = if bg_images.is_empty() {
                None
            } else {
                use rand::Rng;
                let index = rand::thread_rng().gen_range(0..bg_images.len());
                read_public_asset(&public_dir, &bg_images[index])
                    .await
                    .map(|bytes| (bg_images[index].clone(), bytes))
            };
            let svg = render_titleimage_svg(
                &config,
                &title,
                font_data.as_ref(),
                background
                    .as_ref()
                    .map(|(relative, bytes)| (relative.as_str(), bytes.as_slice())),
            );
            let relative = format!(
                "uploads/titleimage/generated/{}.svg",
                attachment::md5_hex(title.as_bytes())
            );
            let path = public_dir.join(&relative);
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            if tokio::fs::write(&path, svg.as_bytes()).await.is_err() {
                continue;
            }

            let mut list = json_string_list(images.as_deref());
            if !list.contains(&relative) {
                list.push(relative);
            }
            let images_json = serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string());
            let _ = sqlx::query("UPDATE archives SET images = ? WHERE id = ?")
                .bind(images_json)
                .bind(id)
                .execute(&pool)
                .await;
        }
    }
}

/// `POST /system/api/plugin/titleimage/generate`
async fn manage_titleimage_generate(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let public_dir = state.with_config(|config| config.public_dir());
    let pool = pool.clone();
    tokio::spawn(async move {
        generate_all_title_images(pool, public_dir).await;
    });
    ok_msg("已提交，标题图片将在后台生成")
}

// ---------------------------- 敏感词 ---------------------------------------

/// `GET /system/api/setting/sensitive/words`
async fn manage_sensitive_words(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match sensitive::get(pool).await {
        Ok(words) => ok(words),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/setting/sensitive/words`（请求体就是字符串数组）
async fn manage_sensitive_words_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(words): Json<Vec<String>>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match sensitive::save(pool, &words).await {
        Ok(()) => ok_msg("配置已更新"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct SensitiveCheckBody {
    #[serde(default)]
    title: String,
    #[serde(default)]
    content: String,
}

/// `POST /system/api/setting/sensitive/check`
async fn manage_sensitive_check(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SensitiveCheckBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let words = match sensitive::get(pool).await {
        Ok(words) => words,
        Err(err) => return failed(err),
    };
    let rules = sensitive::parse(&words);

    let mut found = sensitive::hits(&body.content, &rules);
    for hit in sensitive::hits(&body.title, &rules) {
        if !found.contains(&hit) {
            found.push(hit);
        }
    }
    ok(found)
}

/// `POST /system/api/setting/sensitive/sync`
///
/// 从云端同步敏感词，成功且带回非空列表时整份覆盖本地配置。
async fn manage_sensitive_sync(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match sensitive::sync_from_cloud(pool).await {
        Ok(count) => ok_msg(format!("同步完成，共 {count} 条敏感词")),
        Err(err) => model_error(err),
    }
}

// ---------------------------- 评论 -----------------------------------------

#[derive(Debug, Deserialize, Default)]
struct CommentListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
}

/// `GET /system/api/plugin/comment/list`
async fn manage_comment_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CommentListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let (total, rows) =
        match comment::list(pool, query.current.unwrap_or(1), query.page_size.unwrap_or(20)).await {
            Ok(value) => value,
            Err(err) => return failed(err),
        };

    let archive_ids: Vec<i64> = rows.iter().map(|item| item.archive_id).collect();
    let titles = comment::archive_titles(pool, &archive_ids)
        .await
        .unwrap_or_default();

    let list: Vec<Value> = rows
        .iter()
        .map(|item| {
            let mut value = serde_json::to_value(item).unwrap_or(Value::Null);
            if let Value::Object(map) = &mut value {
                map.insert(
                    "item_title".into(),
                    Value::String(
                        titles
                            .get(&item.archive_id)
                            .cloned()
                            .unwrap_or_default(),
                    ),
                );
            }
            value
        })
        .collect();

    Json(json!({
        "code": 0,
        "msg": "",
        "total": total,
        "data": list,
    }))
    .into_response()
}

/// `GET /system/api/plugin/comment/detail?id=`
async fn manage_comment_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match comment::get(pool, query.id).await {
        Ok(Some(item)) => ok(item),
        Ok(None) => fail("评论不存在"),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct CommentSaveBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    user_name: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    ip: String,
}

/// `POST /system/api/plugin/comment/detail`
async fn manage_comment_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CommentSaveBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match comment::update(pool, body.id, &body.user_name, &body.content, &body.ip).await {
        Ok(_) => ok_msg("评论已更新"),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/comment/delete`
async fn manage_comment_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match comment::delete(pool, body.id).await {
        Ok(true) => ok_msg("评论已删除"),
        Ok(false) => fail("评论不存在"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct CommentCheckBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    ids: Vec<u32>,
    #[serde(default)]
    status: i8,
}

/// `POST /system/api/plugin/comment/check`
///
/// 传 `ids` 批量置为指定状态；只传 `id` 则在「正常 / 待审核」之间切换。
async fn manage_comment_check(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CommentCheckBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    if !body.ids.is_empty() {
        return match comment::set_status(pool, &body.ids, body.status).await {
            Ok(_) => ok_msg("评论已更新"),
            Err(err) => failed(err),
        };
    }
    if body.id == 0 {
        return fail("请选择要审核的评论");
    }
    match comment::toggle(pool, body.id).await {
        Ok(_) => ok_msg("评论已更新"),
        Err(err) => model_error(err),
    }
}

// ---------------------------- 留言 -----------------------------------------

#[derive(Debug, Deserialize, Default)]
struct GuestbookListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    keyword: Option<String>,
    /// `default` / `ok` / `spam`，其它值视为不筛选
    #[serde(default)]
    status: Option<String>,
}

fn guestbook_status(raw: Option<&str>) -> Option<i8> {
    match raw.map(str::trim) {
        Some("default") => Some(0),
        Some("ok") => Some(1),
        Some("spam") => Some(2),
        _ => None,
    }
}

/// `GET /system/api/plugin/guestbook/list`
async fn manage_guestbook_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<GuestbookListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let filter = guestbook::GuestbookFilter {
        keyword: query.keyword.unwrap_or_default(),
        status: guestbook_status(query.status.as_deref()),
    };
    match guestbook::list(
        pool,
        &filter,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/guestbook/detail?id=`
async fn manage_guestbook_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match guestbook::get(pool, query.id).await {
        Ok(Some(item)) => ok(item),
        Ok(None) => fail("留言不存在"),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct GuestbookStatusBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    ids: Vec<u32>,
    #[serde(default)]
    status: i8,
}

/// `POST /system/api/plugin/guestbook/status`
async fn manage_guestbook_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GuestbookStatusBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if !guestbook::valid_status(body.status) {
        return fail("状态不正确");
    }

    let ids: Vec<u32> = if body.id > 0 {
        vec![body.id]
    } else {
        body.ids.clone()
    };
    if ids.is_empty() {
        return fail("请选择要更新的留言");
    }

    match guestbook::set_status(pool, &ids, body.status).await {
        Ok(_) => ok_msg("删除操作已执行"),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct GuestbookDeleteBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    ids: Vec<u32>,
}

/// `POST /system/api/plugin/guestbook/delete`
async fn manage_guestbook_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GuestbookDeleteBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let result = if body.id > 0 {
        guestbook::delete(pool, body.id).await.map(|_| ())
    } else if !body.ids.is_empty() {
        guestbook::delete_many(pool, &body.ids).await.map(|_| ())
    } else {
        return fail("请选择要删除的留言");
    };

    match result {
        Ok(()) => ok_msg("删除操作已执行"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/guestbook/export`
async fn manage_guestbook_export(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match guestbook::export(pool).await {
        Ok((header, content)) => ok(json!({ "header": header, "content": content })),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/guestbook/setting`
async fn manage_guestbook_setting(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match guestbook::setting_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/guestbook/setting`
async fn manage_guestbook_setting_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match guestbook::setting_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

// ---------------------------- 邮件订阅 -------------------------------------

#[derive(Debug, Deserialize, Default)]
struct SubscriberListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    category_id: Option<i64>,
}

/// `GET /system/api/plugin/subscriber/list`
async fn manage_subscriber_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SubscriberListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match subscriber::list(
        pool,
        query.category_id.unwrap_or(0),
        query.email.as_deref().unwrap_or(""),
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct SubscriberIdQuery {
    #[serde(default)]
    id: Option<i64>,
}

/// `GET /system/api/plugin/subscriber/detail?id=`
async fn manage_subscriber_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SubscriberIdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let id = query.id.unwrap_or(0) as i32;
    if id <= 0 {
        return fail("订阅用户不存在");
    }
    match subscriber::get(pool, id).await {
        Ok(Some(item)) => ok(item),
        Ok(None) => fail("订阅用户不存在"),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct SubscriberSaveBody {
    #[serde(default)]
    id: i32,
    #[serde(default)]
    email: String,
}

/// `POST /system/api/plugin/subscriber/save`
async fn manage_subscriber_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubscriberSaveBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match subscriber::save(pool, body.id, &body.email).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "操作成功", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct SubscriberDeleteBody {
    #[serde(default)]
    id: i32,
}

/// `POST /system/api/plugin/subscriber/delete`
async fn manage_subscriber_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubscriberDeleteBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match subscriber::delete(pool, body.id).await {
        Ok(true) => ok_msg("操作成功"),
        Ok(false) => fail("订阅用户不存在"),
        Err(err) => failed(err),
    }
}

/// 订阅群发任务状态（后台按 job_id 查询）。
#[derive(Debug, Clone, Serialize)]
struct SubscriberSendJob {
    job_id: String,
    finished: bool,
    total: i64,
    sent: i64,
    failed: i64,
    percent: i32,
    message: String,
    created_time: i64,
}

/// 进程内任务表（对齐备份任务的做法：进度只保存在进程内存里）。
fn subscriber_send_jobs(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, SubscriberSendJob>> {
    static JOBS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, SubscriberSendJob>>,
    > = std::sync::OnceLock::new();
    JOBS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 新建任务；任务表超过 50 条时清理最早结束的一条。
fn subscriber_send_job_create(job: SubscriberSendJob) {
    let mut jobs = subscriber_send_jobs()
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    if jobs.len() >= 50 {
        let oldest = jobs
            .iter()
            .filter(|(_, item)| item.finished)
            .min_by_key(|(_, item)| item.created_time)
            .map(|(key, _)| key.clone());
        if let Some(key) = oldest {
            jobs.remove(&key);
        }
    }
    jobs.insert(job.job_id.clone(), job);
}

/// 更新任务进度（任务已被清理时忽略）。
fn subscriber_send_job_update(job_id: &str, update: impl FnOnce(&mut SubscriberSendJob)) {
    let mut jobs = subscriber_send_jobs()
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    if let Some(job) = jobs.get_mut(job_id) {
        update(job);
    }
}

/// 读取任务状态。
fn subscriber_send_job_get(job_id: &str) -> Option<SubscriberSendJob> {
    let jobs = subscriber_send_jobs()
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    jobs.get(job_id).cloned()
}

#[derive(Debug, Deserialize, Default)]
struct SubscriberSendBody {
    #[serde(default, alias = "categoryId")]
    category_id: i64,
    #[serde(default, alias = "title")]
    subject: String,
    #[serde(default)]
    content: String,
}

/// `POST /system/api/plugin/subscriber/send`
///
/// 按分类（category_id=0 表示全部已确认订阅用户）异步群发，立即返回 job_id，
/// 进度通过 `GET /plugin/subscriber/send/status?job_id=` 查询。
async fn manage_subscriber_send(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubscriberSendBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let subject = body.subject.trim().to_string();
    let content = body.content.trim().to_string();
    if subject.is_empty() {
        return fail("请填写邮件标题");
    }
    if content.is_empty() {
        return fail("请填写邮件内容");
    }
    let config = match sendmail::config_get(pool).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let Some(account) = sendmail_smtp_account(&config) else {
        return fail("请先设置邮件发送账户");
    };

    // 只群发给已确认（status=1）的订阅用户。
    let sql = if body.category_id > 0 {
        "SELECT email FROM subscribers WHERE status = 1 AND category_id = ? ORDER BY id ASC"
    } else {
        "SELECT email FROM subscribers WHERE status = 1 ORDER BY id ASC"
    };
    let mut query = sqlx::query_scalar::<_, String>(sql);
    if body.category_id > 0 {
        query = query.bind(body.category_id);
    }
    let emails = match query.fetch_all(pool).await {
        Ok(list) => list,
        Err(err) => return failed(err.into()),
    };
    if emails.is_empty() {
        return fail("没有可发送的订阅用户");
    }

    let job_id = format!(
        "{:x}{:04x}",
        chrono::Utc::now().timestamp_millis(),
        rand::random::<u16>()
    );
    let total = emails.len() as i64;
    subscriber_send_job_create(SubscriberSendJob {
        job_id: job_id.clone(),
        finished: false,
        total,
        sent: 0,
        failed: 0,
        percent: 0,
        message: format!("正在发送 (0/{total})"),
        created_time: chrono::Utc::now().timestamp(),
    });

    let site_name = site_setting::get_section(pool, site_setting::SYSTEM)
        .await
        .ok()
        .and_then(|value| value["site_name"].as_str().map(str::to_string))
        .unwrap_or_default();
    let cache_dir = state.with_config(|config| config.cache_dir());

    let pool_bg = pool.clone();
    let job_id_bg = job_id.clone();
    tokio::spawn(async move {
        for email in emails {
            let data = json!({
                "system": {"SiteName": site_name},
                "user": {"UserName": email, "Email": email},
            });
            let mail_subject =
                sendmail::render_template(&subject, &data).unwrap_or_else(|_| subject.clone());
            let mail_html =
                sendmail::render_template(&content, &data).unwrap_or_else(|_| content.clone());
            let account_one = account.clone();
            let recipient = email.clone();
            let mail_subject_bg = mail_subject.clone();
            let result = tokio::task::spawn_blocking(move || {
                ext_smtp_send(&account_one, &recipient, &mail_subject_bg, &mail_html)
            })
            .await;
            let success = matches!(result, Ok(Ok(())));

            let now = chrono::Utc::now().timestamp() as i32;
            let update_sql = if success {
                "UPDATE subscribers SET send_times = COALESCE(send_times, 0) + 1, \
                 updated_time = ? WHERE email = ?"
            } else {
                "UPDATE subscribers SET error_times = COALESCE(error_times, 0) + 1, \
                 updated_time = ? WHERE email = ?"
            };
            let _ = sqlx::query(update_sql)
                .bind(now)
                .bind(&email)
                .execute(&pool_bg)
                .await;
            append_mail_log(
                &cache_dir,
                &mail_subject,
                if success { "success" } else { "failed" },
                &email,
            );
            subscriber_send_job_update(&job_id_bg, |job| {
                if success {
                    job.sent += 1;
                } else {
                    job.failed += 1;
                }
                let done = job.sent + job.failed;
                let total = job.total;
                job.percent = if total > 0 {
                    (done * 100 / total) as i32
                } else {
                    100
                };
                job.message = format!("正在发送 ({done}/{total})");
            });
        }
        subscriber_send_job_update(&job_id_bg, |job| {
            job.finished = true;
            job.percent = 100;
            job.message = format!("发送完成：成功 {} 条，失败 {} 条", job.sent, job.failed);
        });
    });

    ok(json!({ "job_id": job_id, "total": total }))
}

#[derive(Debug, Deserialize, Default)]
struct SendStatusQuery {
    #[serde(default)]
    job_id: Option<String>,
}

/// `GET /system/api/plugin/subscriber/send/status?job_id=`
async fn manage_subscriber_send_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SendStatusQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let job_id = query
        .job_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(job_id) = job_id else {
        // 错误信息要能指向下一步：告诉调用方 job_id 从哪来
        return fail(
            "缺少 job_id 参数。请先调用 POST /plugin/subscriber/send 发起群发，\
             从返回结果里取 job_id，再带上它来查进度（如 ?job_id=xxx）。",
        );
    };
    match subscriber_send_job_get(job_id) {
        Some(job) => ok(job),
        None => fail(format!("未找到 job_id={job_id} 的群发任务")),
    }
}

/// `GET /system/api/plugin/subscriber/category/list`
async fn manage_subscriber_category_list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match subscriber::category_list(pool).await {
        Ok(list) => Json(json!({
            "code": 0,
            "msg": "",
            "total": list.len(),
            "data": list,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct SubscriberCategoryBody {
    #[serde(default)]
    id: i32,
    #[serde(default)]
    title: String,
}

/// `POST /system/api/plugin/subscriber/category/save`
async fn manage_subscriber_category_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubscriberCategoryBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match subscriber::category_save(pool, body.id, &body.title).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "操作成功", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/subscriber/category/delete`
async fn manage_subscriber_category_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubscriberCategoryBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match subscriber::category_delete(pool, body.id).await {
        Ok(true) => ok_msg("删除成功"),
        Ok(false) => fail("订阅分类不存在"),
        Err(err) => failed(err),
    }
}

// ---------------------------- 验证文件上传 ---------------------------------

/// `GET /system/api/plugin/fileupload/list`
async fn manage_fileupload_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let base = site_front_url(pool).await;
    match upload_file::list(pool, &base).await {
        Ok(list) => ok(list),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/fileupload/upload`（multipart：`file`）
async fn manage_fileupload_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let mut file_name = String::new();
    let mut bytes: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" || name == "file1" {
            file_name = field.file_name().unwrap_or_default().to_string();
            match field.bytes().await {
                Ok(value) => bytes = Some(value.to_vec()),
                Err(err) => return failed(err.into()),
            }
        } else {
            let _ = field.bytes().await;
        }
    }

    let Some(bytes) = bytes else {
        return fail("请选择要上传的文件");
    };
    let public_dir = state.with_config(|config| config.public_dir());
    match upload_file::add(pool, &public_dir, &file_name, &bytes).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "文件上传完成", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct UploadFileDeleteBody {
    #[serde(default)]
    hash: String,
}

/// `POST /system/api/plugin/fileupload/delete`
async fn manage_fileupload_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<UploadFileDeleteBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let public_dir = state.with_config(|config| config.public_dir());
    match upload_file::remove(pool, &public_dir, &body.hash).await {
        Ok(true) => ok_msg("删除成功"),
        Ok(false) => fail("文件不正确"),
        Err(err) => model_error(err),
    }
}

// ---------------------------- 会员管理 -------------------------------------

/// `GET /system/api/plugin/user/fields`
async fn manage_user_fields(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match user_admin::setting_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/user/fields`
async fn manage_user_fields_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match user_admin::setting_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct UserFieldDeleteBody {
    #[serde(default)]
    field_name: String,
}

/// `POST /system/api/plugin/user/field/delete`
async fn manage_user_field_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<UserFieldDeleteBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match user_admin::field_delete(pool, &body.field_name).await {
        Ok(true) => ok_msg("字段已删除"),
        Ok(false) => fail("字段不存在"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct UserListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    id: Option<u32>,
    #[serde(default)]
    group_id: Option<u32>,
    #[serde(default)]
    user_name: Option<String>,
    #[serde(default)]
    real_name: Option<String>,
    #[serde(default)]
    phone: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    user_type: Option<String>,
}

/// `GET /system/api/plugin/user/list`
async fn manage_user_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UserListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let filter = user_admin::UserFilter {
        id: query.id.unwrap_or(0),
        group_id: query.group_id.unwrap_or(0),
        user_name: query.user_name.unwrap_or_default(),
        real_name: query.real_name.unwrap_or_default(),
        phone: query.phone.unwrap_or_default(),
        q: query.q.unwrap_or_default(),
        status: query.status.unwrap_or_default(),
        user_type: query.user_type.unwrap_or_default(),
        is_retailer: None,
    };
    match user_admin::list(
        pool,
        &filter,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/user/detail?id=`
async fn manage_user_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match user_admin::get(pool, query.id).await {
        Ok(Some(item)) => ok(item),
        Ok(None) => fail("会员不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/user/detail`
async fn manage_user_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<user_admin::UserInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match user_admin::save(pool, &input).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "保存成功", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct UserBalanceBody {
    #[serde(default)]
    user_id: u32,
    #[serde(default)]
    amount: i64,
    #[serde(default)]
    remark: String,
}

/// `POST /system/api/plugin/user/balance`
async fn manage_user_balance(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<UserBalanceBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match user_admin::change_balance(pool, body.user_id, body.amount, &body.remark).await {
        Ok(balance) => ok(json!({ "balance": balance })),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/user/delete`
async fn manage_user_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match user_admin::delete(pool, body.id).await {
        Ok(true) => ok_msg("删除成功"),
        Ok(false) => fail("会员不存在"),
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/plugin/user/group/list`
async fn manage_user_group_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match user_admin::group_list(pool).await {
        Ok(list) => ok(list),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/user/group/detail?id=`
async fn manage_user_group_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match user_admin::group_get(pool, query.id).await {
        Ok(Some(item)) => ok(item),
        Ok(None) => fail("用户组不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/user/group/detail`
async fn manage_user_group_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<user_admin::UserGroupInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match user_admin::group_save(pool, &input).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "保存成功", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/user/group/delete`
async fn manage_user_group_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match user_admin::group_delete(pool, body.id).await {
        Ok(true) => ok_msg("删除成功"),
        Ok(false) => fail("用户组不存在"),
        Err(err) => model_error(err),
    }
}

// ---------------------------- 素材库 ---------------------------------------

#[derive(Debug, Deserialize, Default)]
struct MaterialListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    keyword: Option<String>,
    #[serde(default)]
    category_id: Option<u32>,
}

/// `GET /system/api/plugin/material/list`
async fn manage_material_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<MaterialListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match material::list(
        pool,
        query.category_id.unwrap_or(0),
        query.keyword.as_deref().unwrap_or(""),
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/material/detail?id=`
async fn manage_material_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match material::get(pool, query.id).await {
        Ok(Some(item)) => ok(item),
        Ok(None) => fail("素材不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/material/detail`
async fn manage_material_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<material::MaterialInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match material::save(pool, &input).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "素材已更新", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct MaterialImportBody {
    #[serde(default)]
    materials: Vec<material::MaterialInput>,
}

/// `POST /system/api/plugin/material/import`
async fn manage_material_import(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<MaterialImportBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if body.materials.is_empty() {
        return fail("请提供要导入的素材");
    }
    match material::save_many(pool, &body.materials).await {
        Ok(count) => ok(json!({ "count": count })),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/material/delete`
async fn manage_material_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match material::delete(pool, body.id).await {
        Ok(true) => ok_msg("素材已删除"),
        Ok(false) => fail("素材不存在"),
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/plugin/material/category/list`
async fn manage_material_category_list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match material::category_list(pool).await {
        Ok(list) => ok(list),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct MaterialCategoryBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    title: String,
}

/// `POST /system/api/plugin/material/category/detail`
async fn manage_material_category_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<MaterialCategoryBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match material::category_save(pool, body.id, &body.title).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "分类已更新", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/material/category/delete`
async fn manage_material_category_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match material::category_delete(pool, body.id).await {
        Ok(true) => ok_msg("分类已删除"),
        Ok(false) => fail("分类不存在"),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/material/convert/file`（multipart：`file` + `remove_tag`）
async fn manage_material_convert_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }

    let mut remove_tag = false;
    let mut bytes: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "remove_tag" => {
                let value = field.text().await.unwrap_or_default();
                remove_tag = matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                );
            }
            "file" | "file1" => match field.bytes().await {
                Ok(value) => bytes = Some(value.to_vec()),
                Err(err) => return failed(err.into()),
            },
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    let Some(bytes) = bytes else {
        return fail("请选择要转码的文件");
    };
    match material::convert_text(&bytes, remove_tag) {
        Ok(content) => ok(json!(content)),
        Err(err) => model_error(err),
    }
}

// ---------------------------- 水印 / 干扰码 / 限流 ---------------------------

/// `GET /system/api/plugin/watermark/config`
async fn manage_watermark_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match watermark::config_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/watermark/config`
async fn manage_watermark_config_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match watermark::config_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/watermark/preview`
///
/// 对齐 Go 版 `PluginWatermarkPreview`：用当前配置在 800×600 纯色底图上
/// 生成水印效果，返回可直接查看的 `data:image/svg+xml;base64,` 图片。
async fn manage_watermark_preview(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = match watermark::config_get(pool).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let public_dir = state.with_config(|config| config.public_dir());

    const PREVIEW_WIDTH: u32 = 800;
    const PREVIEW_HEIGHT: u32 = 600;
    let size = config
        .get("size")
        .and_then(Value::as_i64)
        .unwrap_or(20)
        .clamp(1, 200) as u32;
    let position = config
        .get("position")
        .and_then(Value::as_i64)
        .unwrap_or(9);
    let opacity = config
        .get("opacity")
        .and_then(Value::as_i64)
        .unwrap_or(100)
        .clamp(1, 100);
    let watermark_type = config.get("type").and_then(Value::as_i64).unwrap_or(0);

    let max_width = (PREVIEW_WIDTH * size / 100).max(1);
    let max_height = (PREVIEW_HEIGHT * size / 100).max(1);
    let edge = (PREVIEW_WIDTH / 15).min(50);

    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{PREVIEW_WIDTH}" height="{PREVIEW_HEIGHT}" viewBox="0 0 {PREVIEW_WIDTH} {PREVIEW_HEIGHT}"><rect x="0" y="0" width="{PREVIEW_WIDTH}" height="{PREVIEW_HEIGHT}" fill="#607d8b"/>"##
    );

    if watermark_type == 0 {
        // 图片水印
        let image_path = config
            .get("image_path")
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(bytes) = read_public_asset(&public_dir, image_path).await else {
            return fail("水印图片未配置或无法读取");
        };
        let (source_width, source_height) = attachment::image_size(&bytes);
        let (mark_width, mark_height) = fit_mark(source_width, source_height, max_width, max_height);
        let (x, y) = watermark_position(
            position,
            PREVIEW_WIDTH,
            PREVIEW_HEIGHT,
            mark_width,
            mark_height,
            edge,
        );
        svg.push_str(&format!(
            r#"<image x="{x}" y="{y}" width="{mark_width}" height="{mark_height}" preserveAspectRatio="xMidYMid meet" opacity="{}" href="data:{};base64,{}"/>"#,
            opacity as f64 / 100.0,
            image_mime_of(image_path),
            image_base64(&bytes)
        ));
    } else {
        // 文字水印
        let text = config
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if text.is_empty() {
            return fail("水印文字未配置");
        }
        let font_path = config.get("font_path").and_then(Value::as_str).unwrap_or("");
        let font_data = read_public_asset(&public_dir, font_path).await;
        let color = hex_color_or(
            config.get("color").and_then(Value::as_str).unwrap_or(""),
            "#ffffff",
        );

        // 与 Go 一致：中文按 1 个字号、英文按 0.5 个字号估算文字宽度
        let text_width: u32 = text
            .chars()
            .map(|ch| if ch.is_ascii() { (size / 2).max(1) } else { size })
            .sum();
        let mark_width = text_width + text_width / 10;
        let mark_height = size + size / 10;
        let (x, y) = watermark_position(
            position,
            PREVIEW_WIDTH,
            PREVIEW_HEIGHT,
            mark_width,
            mark_height,
            edge,
        );

        if let Some(bytes) = &font_data {
            svg.push_str(&format!(
                r#"<defs><style>@font-face{{font-family:'WatermarkFont';src:url(data:font/ttf;base64,{}) format('truetype');}}</style></defs>"#,
                image_base64(bytes)
            ));
        }
        let font_family = if font_data.is_some() {
            "WatermarkFont, sans-serif"
        } else {
            "sans-serif"
        };
        svg.push_str(&format!(
            r#"<text x="{x}" y="{}" fill="{color}" font-size="{size}" font-family="{font_family}" opacity="{}">{}</text>"#,
            y + size as i64,
            opacity as f64 / 100.0,
            xml_escape(&text)
        ));
    }
    svg.push_str("</svg>");

    ok(json!(format!(
        "data:image/svg+xml;base64,{}",
        image_base64(svg.as_bytes())
    )))
}

/// `POST /system/api/plugin/watermark/upload`（multipart：`name` + `file`）
async fn manage_watermark_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let mut kind = String::new();
    let mut file_name = String::new();
    let mut bytes: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "name" => kind = field.text().await.unwrap_or_default(),
            "file" | "file1" => {
                file_name = field.file_name().unwrap_or_default().to_string();
                match field.bytes().await {
                    Ok(value) => bytes = Some(value.to_vec()),
                    Err(err) => return failed(err.into()),
                }
            }
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    let kind = kind.trim();
    if kind != "font_path" && kind != "image_path" {
        return fail("文件名无效");
    }
    let Some(bytes) = bytes else {
        return fail("请选择要上传的文件");
    };

    let public_dir = state.with_config(|config| config.public_dir());
    let path = match watermark::upload(&public_dir, kind, &file_name, &bytes) {
        Ok(path) => path,
        Err(err) => return model_error(err),
    };
    if let Err(err) = watermark::set_path(pool, kind, &path).await {
        return failed(err);
    }
    ok(json!(path))
}

/// 后台批量添加图片水印（当前图片引擎支持 PNG/BMP）。
async fn generate_all_watermarks(pool: sqlx::mysql::MySqlPool, public_dir: std::path::PathBuf) {
    let Ok(config) = watermark::config_get(&pool).await else {
        return;
    };
    if config.get("type").and_then(Value::as_i64).unwrap_or(0) != 0 {
        return;
    }
    let size = config
        .get("size")
        .and_then(Value::as_i64)
        .unwrap_or(20)
        .clamp(1, 200) as u32;
    let position = config
        .get("position")
        .and_then(Value::as_i64)
        .unwrap_or(9);
    let opacity = config
        .get("opacity")
        .and_then(Value::as_i64)
        .unwrap_or(100)
        .clamp(0, 100) as u8;
    let min_size = config
        .get("min_size")
        .and_then(Value::as_i64)
        .unwrap_or(400)
        .max(1) as u32;

    let image_path = config
        .get("image_path")
        .and_then(Value::as_str)
        .unwrap_or("");
    let Ok(mark_bytes) = tokio::fs::read(public_dir.join(image_path.trim_start_matches('/'))).await
    else {
        return;
    };
    let Ok(mark_source) = crate::models::titleimage::image_ops::decode(&mark_bytes) else {
        return;
    };

    let mut last_id = 0u32;
    loop {
        let rows: Vec<(u32, String)> = match sqlx::query_as(
            "SELECT id, file_location FROM attachments WHERE is_image = 1 AND watermark = 0 AND `status` = 1 AND id > ? ORDER BY id ASC LIMIT 500",
        )
        .bind(last_id)
        .fetch_all(&pool)
        .await
        {
            Ok(rows) => rows,
            Err(_) => break,
        };
        if rows.is_empty() {
            break;
        }
        last_id = rows.iter().map(|row| row.0).max().unwrap_or(last_id);

        for (id, location) in rows {
            let ext = file_ext(&location);
            if !crate::models::titleimage::image_ops::supports_extension(&ext) {
                continue;
            }
            let Ok(bytes) = tokio::fs::read(public_dir.join(&location)).await else {
                continue;
            };
            let Ok(mut image) = crate::models::titleimage::image_ops::decode(&bytes) else {
                continue;
            };
            if image.width < min_size && image.height < min_size {
                continue;
            }

            let max_width = (image.width * size / 100).max(1);
            let max_height = (image.height * size / 100).max(1);
            let mark = crate::models::titleimage::image_ops::resize_fit(
                &mark_source, max_width, max_height,
            );
            let edge = (image.width / 15).min(50);
            let (x, y) = watermark_position(
                position,
                image.width,
                image.height,
                mark.width,
                mark.height,
                edge,
            );
            image.composite(&mark, x, y, opacity);
            let Ok(encoded) = crate::models::titleimage::image_ops::encode(&image, &ext) else {
                continue;
            };
            if tokio::fs::write(public_dir.join(&location), encoded)
                .await
                .is_err()
            {
                continue;
            }
            let _ = sqlx::query("UPDATE attachments SET watermark = 1 WHERE id = ?")
                .bind(id)
                .execute(&pool)
                .await;
        }
    }
}

/// `POST /system/api/plugin/watermark/generate`
async fn manage_watermark_generate(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = match watermark::config_get(pool).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    if !config.get("open").and_then(Value::as_bool).unwrap_or(false) {
        return ok_msg("水印插件未开启");
    }
    if config.get("type").and_then(Value::as_i64).unwrap_or(0) != 0 {
        return fail("当前图片引擎暂只支持图片水印（PNG/BMP）批量处理");
    }
    let image_path = config
        .get("image_path")
        .and_then(Value::as_str)
        .unwrap_or("");
    let public_dir = state.with_config(|config| config.public_dir());
    let Ok(mark_bytes) = tokio::fs::read(public_dir.join(image_path.trim_start_matches('/'))).await
    else {
        return fail("水印图片未配置或无法读取");
    };
    if crate::models::titleimage::image_ops::decode(&mark_bytes).is_err() {
        return fail("水印图片格式不受支持（当前引擎支持 PNG/BMP）");
    }
    let pool = pool.clone();
    tokio::spawn(async move {
        generate_all_watermarks(pool, public_dir).await;
    });
    ok_msg("已提交，水印将在后台生成")
}

/// `GET /system/api/plugin/interference/config`
async fn manage_interference_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::interference_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/interference/config`
async fn manage_interference_config_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::interference_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/limiter/setting`
async fn manage_limiter_setting(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::limiter_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/limiter/setting`
async fn manage_limiter_setting_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::limiter_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/limiter/blockedips`
///
/// 限流器本身未实现，因此屏蔽列表恒为空。
async fn manage_limiter_blocked(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    ok(json!([]))
}

/// 解除 IP 封禁请求体。
#[derive(Debug, Deserialize, Default)]
struct LimiterIpBody {
    #[serde(default)]
    ip: String,
}

/// `POST /system/api/plugin/limiter/blockedip/remove`
async fn manage_limiter_unblock(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LimiterIpBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    plugin::limiter_unblock(&body.ip);
    ok_msg("删除成功")
}

// ---------------------------- Akismet / 全文索引 / 导入 API ------------------

/// `GET /system/api/plugin/akismet/setting`
async fn manage_akismet_setting(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::akismet_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/akismet/setting`
async fn manage_akismet_setting_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::akismet_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/fulltext/config`
async fn manage_fulltext_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::fulltext_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/fulltext/config`
async fn manage_fulltext_config_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::fulltext_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// 全文索引文件目录：`<cache>/fulltext`。
fn fulltext_index_dir(cache_dir: &std::path::Path) -> std::path::PathBuf {
    cache_dir.join("fulltext")
}

/// 去掉 HTML 标签并压缩空白，供索引正文使用。
fn fulltext_plain_text(input: &str) -> String {
    let mut text = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
            text.push(' ');
        } else if !in_tag {
            text.push(ch);
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 按配置的 `contain_length` 截断索引正文（0 表示不截断）。
fn fulltext_trim(text: &str, limit: usize) -> String {
    if limit == 0 {
        text.to_string()
    } else {
        text.chars().take(limit).collect()
    }
}

/// 读取插件配置里的布尔开关。
fn config_flag(config: &Value, key: &str) -> bool {
    config.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// 读取全文索引配置里的模型 ID 列表。
fn fulltext_modules(config: &Value) -> Vec<u32> {
    config
        .get("modules")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| item.as_u64().map(|id| id as u32))
                .collect()
        })
        .unwrap_or_default()
}

/// 按配置为内容、栏目与标签建立本地全文索引，写入 `<cache>/fulltext/index.json`。
async fn build_fulltext_index(
    pool: &sqlx::mysql::MySqlPool,
    cache_dir: &std::path::Path,
    modules: &[u32],
    use_content: bool,
    use_category: bool,
    use_tag: bool,
    contain_length: usize,
) -> anyhow::Result<usize> {
    let placeholders = vec!["?"; modules.len()].join(",");
    let mut docs: Vec<Value> = Vec::new();

    // 内容
    let mut archive_sql = String::from(
        "SELECT a.id, a.title, a.keywords, a.description, a.module_id, a.category_id",
    );
    if use_content {
        archive_sql.push_str(", d.content");
        archive_sql.push_str(" FROM archives a LEFT JOIN archive_data d ON a.id = d.id");
    } else {
        archive_sql.push_str(" FROM archives a");
    }
    archive_sql.push_str(&format!(
        " WHERE a.module_id IN ({placeholders}) ORDER BY a.id ASC"
    ));

    if use_content {
        let mut query =
            sqlx::query_as::<_, (i64, String, String, String, u32, u32, Option<String>)>(
                &archive_sql,
            );
        for id in modules {
            query = query.bind(*id);
        }
        for (id, title, keywords, description, module_id, category_id, content) in
            query.fetch_all(pool).await?
        {
            let body = fulltext_trim(
                &fulltext_plain_text(content.as_deref().unwrap_or_default()),
                contain_length,
            );
            docs.push(json!({
                "id": id,
                "type": "archive",
                "title": title,
                "keywords": keywords,
                "description": description,
                "module_id": module_id,
                "category_id": category_id,
                "content": body,
            }));
        }
    } else {
        let mut query =
            sqlx::query_as::<_, (i64, String, String, String, u32, u32)>(&archive_sql);
        for id in modules {
            query = query.bind(*id);
        }
        for (id, title, keywords, description, module_id, category_id) in
            query.fetch_all(pool).await?
        {
            docs.push(json!({
                "id": id,
                "type": "archive",
                "title": title,
                "keywords": keywords,
                "description": description,
                "module_id": module_id,
                "category_id": category_id,
                "content": "",
            }));
        }
    }

    // 栏目
    if use_category {
        let sql = format!(
            "SELECT id, title, keywords, description, module_id, content FROM categories \
             WHERE module_id IN ({placeholders}) ORDER BY id ASC"
        );
        let mut query =
            sqlx::query_as::<_, (u32, String, String, String, u32, Option<String>)>(&sql);
        for id in modules {
            query = query.bind(*id);
        }
        for (id, title, keywords, description, module_id, content) in
            query.fetch_all(pool).await?
        {
            let body = fulltext_trim(
                &fulltext_plain_text(content.as_deref().unwrap_or_default()),
                contain_length,
            );
            docs.push(json!({
                "id": id,
                "type": "category",
                "title": title,
                "keywords": keywords,
                "description": description,
                "module_id": module_id,
                "category_id": 0,
                "content": body,
            }));
        }
    }

    // 标签
    if use_tag {
        let rows: Vec<(u32, String, String, String)> =
            sqlx::query_as("SELECT id, title, keywords, description FROM tags ORDER BY id ASC")
                .fetch_all(pool)
                .await?;
        for (id, title, keywords, description) in rows {
            docs.push(json!({
                "id": id,
                "type": "tag",
                "title": title,
                "keywords": keywords,
                "description": description,
                "module_id": 0,
                "category_id": 0,
                "content": "",
            }));
        }
    }

    let total = docs.len();
    let dir = fulltext_index_dir(cache_dir);
    std::fs::create_dir_all(&dir)?;
    let payload = json!({
        "built_at": chrono::Local::now().timestamp(),
        "total": total,
        "docs": docs,
    });
    std::fs::write(dir.join("index.json"), serde_json::to_string(&payload)?)?;
    Ok(total)
}

/// `POST /system/api/plugin/fulltext/rebuild`
async fn manage_fulltext_rebuild(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = match plugin::fulltext_get(pool).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    if !config_flag(&config, "open") {
        return fail("全文索引未开启，请先在配置中开启");
    }
    let modules = fulltext_modules(&config);
    if modules.is_empty() {
        return fail("请先选择需要建立索引的模型");
    }
    let contain_length = config
        .get("contain_length")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let cache_dir = state.with_config(|config| config.cache_dir());
    match build_fulltext_index(
        pool,
        &cache_dir,
        &modules,
        config_flag(&config, "use_content"),
        config_flag(&config, "use_category"),
        config_flag(&config, "use_tag"),
        contain_length,
    )
    .await
    {
        Ok(total) => {
            let _ = plugin::fulltext_save(pool, &json!({ "initialed": true })).await;
            ok(json!({ "total": total, "initialed": true }))
        }
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/fulltext/status`
async fn manage_fulltext_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = plugin::fulltext_get(pool).await.unwrap_or_else(|_| json!({}));
    ok(json!({
        "initialed": config.get("initialed").cloned().unwrap_or(json!(false)),
        "running": false,
        "total": 0,
        "done": 0,
        "error_msg": "",
    }))
}

/// `GET /system/api/plugin/import/api`
async fn manage_import_api(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::import_api_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/import/token`
///
/// 重新生成内容导入与友情链接两个 token。
async fn manage_import_token(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let payload = json!({
        "token": auth::generate_secret(32),
        "link_token": auth::generate_secret(32),
    });
    match plugin::import_api_save(pool, &payload).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "操作成功", "data": saved })).into_response()
        }
        Err(err) => failed(err),
    }
}

// ---------------------------- 静态缓存 -------------------------------------

/// `GET /system/api/plugin/htmlcache/config`
async fn manage_htmlcache_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::html_cache_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/htmlcache/config`
async fn manage_htmlcache_config_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::html_cache_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// 静态缓存输出目录：`<cache>/html`（与 clean 接口保持一致）。
fn html_cache_root(state: &AppState) -> std::path::PathBuf {
    state
        .with_config(|config| config.cache_dir())
        .join("html")
}

/// 把站点内链接转成缓存文件相对路径，等价 Go 的 `transToLocalPath`。
fn html_cache_local_path(link: &str) -> String {
    let path = link.split(['?', '#']).next().unwrap_or("");
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    if path.ends_with('/') {
        return format!("{path}index.html");
    }
    let base = path.rsplit('/').next().unwrap_or("");
    if base.contains('.') {
        path
    } else {
        format!("{path}/index.html")
    }
}

/// 写入静态缓存文件。
fn html_cache_write(root: &std::path::Path, link: &str, body: &str) -> Result<(), String> {
    let local = html_cache_local_path(link);
    let rel = local.trim_start_matches('/');
    if rel.is_empty() || rel.split('/').any(|segment| segment == ".." || segment.contains('\\')) {
        return Err(format!("非法缓存路径：{link}"));
    }
    let file = root.join(rel);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建缓存目录失败：{err}"))?;
    }
    std::fs::write(&file, body).map_err(|err| format!("写入缓存失败：{err}"))
}

/// 静态缓存页面作用域（与前台管线首屏一致）。
fn html_cache_scope(page: u32) -> crate::template::Scope {
    let mut scope = crate::template::Scope::new();
    scope.insert("currentPage".to_string(), Value::from(page));
    scope.insert(
        "version".to_string(),
        Value::from(env!("CARGO_PKG_VERSION")),
    );
    scope.insert("isMobile".to_string(), Value::Bool(false));
    scope.insert("urlParams".to_string(), json!({}));
    scope
}

/// 从候选模板名里挑第一个实际存在的模板。
fn html_cache_template(state: &AppState, candidates: &[String]) -> Option<String> {
    let dir = state.templates().dir().to_path_buf();
    candidates
        .iter()
        .filter(|name| !name.trim().is_empty())
        .find(|name| dir.join(name).is_file())
        .cloned()
}

async fn render_html_cache_index(
    state: &AppState,
    pool: &sqlx::mysql::MySqlPool,
) -> Result<String, String> {
    let template = html_cache_template(
        state,
        &["index/index.html".to_string(), "index.html".to_string()],
    )
    .ok_or_else(|| "缺少首页模板 index/index.html".to_string())?;
    let provider = crate::template::site_provider::SiteProvider::new(pool.clone(), 0)
        .await
        .map_err(|err| format!("{err:#}"))?;
    state
        .templates()
        .render(&template, &html_cache_scope(1), &provider)
        .await
        .map_err(|err| format!("{err:#}"))
}

async fn render_html_cache_module(
    state: &AppState,
    pool: &sqlx::mysql::MySqlPool,
    item: &crate::models::Module,
) -> Result<String, String> {
    let template = html_cache_template(
        state,
        &[
            format!("{}/index.html", item.table_name),
            format!("{}_index.html", item.table_name),
        ],
    )
    .ok_or_else(|| format!("缺少模型 {} 的列表模板", item.table_name))?;
    let provider = crate::template::site_provider::SiteProvider::for_module(pool.clone(), item.id)
        .await
        .map_err(|err| format!("{err:#}"))?
        .with_page(1);
    let mut scope = html_cache_scope(1);
    scope.insert(
        "module".to_string(),
        serde_json::to_value(item).map_err(|err| format!("{err:#}"))?,
    );
    state
        .templates()
        .render(&template, &scope, &provider)
        .await
        .map_err(|err| format!("{err:#}"))
}

async fn render_html_cache_category(
    state: &AppState,
    pool: &sqlx::mysql::MySqlPool,
    item: &Category,
    modules: &[crate::models::Module],
) -> Result<String, String> {
    let table = modules
        .iter()
        .find(|module| module.id == item.module_id)
        .map(|module| module.table_name.clone())
        .unwrap_or_else(|| "article".to_string());
    let mut custom = item.template.trim().to_string();
    if !custom.is_empty() && !custom.ends_with(".html") {
        custom.push_str(".html");
    }
    let template = html_cache_template(
        state,
        &[
            custom,
            format!("{table}/{}.html", item.url_token),
            format!("{table}/list-{}.html", item.id),
            format!("{table}/list.html"),
            format!("{table}_list.html"),
        ],
    )
    .ok_or_else(|| format!("缺少栏目 {} 的列表模板", item.title))?;
    let provider = crate::template::site_provider::SiteProvider::new(pool.clone(), item.id)
        .await
        .map_err(|err| format!("{err:#}"))?
        .with_page(1);
    let mut scope = html_cache_scope(1);
    scope.insert(
        "category".to_string(),
        serde_json::to_value(item).map_err(|err| format!("{err:#}"))?,
    );
    if let Some(module) = modules.iter().find(|module| module.id == item.module_id) {
        scope.insert(
            "module".to_string(),
            serde_json::to_value(module).map_err(|err| format!("{err:#}"))?,
        );
    }
    state
        .templates()
        .render(&template, &scope, &provider)
        .await
        .map_err(|err| format!("{err:#}"))
}

async fn render_html_cache_archive(
    state: &AppState,
    pool: &sqlx::mysql::MySqlPool,
    id: i64,
    item: &crate::models::archive::Archive,
    current: Option<&Category>,
    modules: &[crate::models::Module],
) -> Result<String, String> {
    let table = current
        .and_then(|category| modules.iter().find(|module| module.id == category.module_id))
        .map(|module| module.table_name.clone())
        .unwrap_or_else(|| "article".to_string());
    let mut custom = item.template.trim().to_string();
    if custom.is_empty() {
        custom = current
            .map(|category| category.detail_template.trim().to_string())
            .unwrap_or_default();
    }
    if !custom.is_empty() && !custom.ends_with(".html") {
        custom.push_str(".html");
    }
    let template = html_cache_template(
        state,
        &[
            custom,
            format!("{table}/detail-{}.html", item.id),
            format!("{table}/detail.html"),
            format!("{table}_detail.html"),
        ],
    )
    .ok_or_else(|| format!("缺少内容 {} 的详情模板", item.title))?;
    let provider = crate::template::site_provider::SiteProvider::for_archive(pool.clone(), id)
        .await
        .map_err(|err| format!("{err:#}"))?
        .ok_or_else(|| "内容不存在".to_string())?;
    let mut scope = html_cache_scope(1);
    scope.insert(
        "archive".to_string(),
        serde_json::to_value(item).map_err(|err| format!("{err:#}"))?,
    );
    if let Some(category) = current {
        scope.insert(
            "category".to_string(),
            serde_json::to_value(category).map_err(|err| format!("{err:#}"))?,
        );
    }
    state
        .templates()
        .render(&template, &scope, &provider)
        .await
        .map_err(|err| format!("{err:#}"))
}

async fn render_html_cache_tag_index(
    state: &AppState,
    pool: &sqlx::mysql::MySqlPool,
) -> Result<String, String> {
    let template = html_cache_template(
        state,
        &["tag/index.html".to_string(), "tag_index.html".to_string()],
    )
    .ok_or_else(|| "缺少标签首页模板 tag/index.html".to_string())?;
    let provider = crate::template::site_provider::SiteProvider::new(pool.clone(), 0)
        .await
        .map_err(|err| format!("{err:#}"))?
        .as_tag_index()
        .with_page(1);
    state
        .templates()
        .render(&template, &html_cache_scope(1), &provider)
        .await
        .map_err(|err| format!("{err:#}"))
}

async fn render_html_cache_tag(
    state: &AppState,
    pool: &sqlx::mysql::MySqlPool,
    item: &tag::Tag,
) -> Result<String, String> {
    let mut custom = item.template.trim().to_string();
    if !custom.is_empty() && !custom.ends_with(".html") {
        custom.push_str(".html");
    }
    let template = html_cache_template(
        state,
        &[
            custom,
            format!("tag/list-{}.html", item.id),
            "tag/list.html".to_string(),
            "tag_list.html".to_string(),
        ],
    )
    .ok_or_else(|| format!("缺少标签 {} 的列表模板", item.title))?;
    let provider = crate::template::site_provider::SiteProvider::new(pool.clone(), item.category_id)
        .await
        .map_err(|err| format!("{err:#}"))?
        .with_tag(item.clone())
        .with_page(1);
    let mut scope = html_cache_scope(1);
    scope.insert(
        "tag".to_string(),
        serde_json::to_value(item).map_err(|err| format!("{err:#}"))?,
    );
    state
        .templates()
        .render(&template, &scope, &provider)
        .await
        .map_err(|err| format!("{err:#}"))
}

/// 静态缓存生成统计。
#[derive(Default)]
struct HtmlCacheReport {
    index: usize,
    category: usize,
    archive: usize,
    tag: usize,
    failed: usize,
}

impl HtmlCacheReport {
    fn total(&self) -> usize {
        self.index + self.category + self.archive + self.tag
    }

    fn data(&self) -> Value {
        json!({
            "index": self.index,
            "category": self.category,
            "archive": self.archive,
            "tag": self.tag,
            "failed": self.failed,
            "total": self.total(),
        })
    }
}

/// 渲染并写入一个缓存页，按类型累计统计。
fn html_cache_store(
    report: &mut HtmlCacheReport,
    kind: &str,
    root: &std::path::Path,
    link: &str,
    page: Result<String, String>,
) {
    match page {
        Ok(html) => match html_cache_write(root, link, &html) {
            Ok(()) => match kind {
                "index" => report.index += 1,
                "category" => report.category += 1,
                "archive" => report.archive += 1,
                _ => report.tag += 1,
            },
            Err(err) => {
                report.failed += 1;
                tracing::warn!("{err}");
            }
        },
        Err(err) => {
            report.failed += 1;
            tracing::warn!("静态缓存渲染失败（{link}）：{err}");
        }
    }
}

async fn build_html_cache_index(
    state: &AppState,
    pool: &sqlx::mysql::MySqlPool,
    root: &std::path::Path,
    report: &mut HtmlCacheReport,
) {
    let page = render_html_cache_index(state, pool).await;
    html_cache_store(report, "index", root, "/", page);
}

async fn build_html_cache_categories(
    state: &AppState,
    pool: &sqlx::mysql::MySqlPool,
    root: &std::path::Path,
    report: &mut HtmlCacheReport,
) {
    let mode = crate::models::url::load_mode(pool).await.unwrap_or(0);
    let modules = match module::list(pool).await {
        Ok(list) => list,
        Err(err) => {
            report.failed += 1;
            tracing::warn!("读取模型列表失败：{err:#}");
            return;
        }
    };
    for item in modules.iter().filter(|module| module.status == 1) {
        let link = crate::models::url::archive_index_url(mode, &item.url_token, 1);
        let page = render_html_cache_module(state, pool, item).await;
        html_cache_store(report, "category", root, &link, page);
    }
    let categories = match category::list_all(pool).await {
        Ok(list) => list,
        Err(err) => {
            report.failed += 1;
            tracing::warn!("读取栏目列表失败：{err:#}");
            return;
        }
    };
    for item in categories
        .iter()
        .filter(|category| category.status == 1 && category.kind == 1)
    {
        let module_token = modules
            .iter()
            .find(|module| module.id == item.module_id)
            .map(|module| module.url_token.as_str())
            .unwrap_or("");
        let link =
            crate::models::url::category_url(mode, &item.url_token, module_token, item.id, 1);
        let page = render_html_cache_category(state, pool, item, &modules).await;
        html_cache_store(report, "category", root, &link, page);
    }
}

async fn build_html_cache_archives(
    state: &AppState,
    pool: &sqlx::mysql::MySqlPool,
    root: &std::path::Path,
    report: &mut HtmlCacheReport,
) {
    let mode = crate::models::url::load_mode(pool).await.unwrap_or(0);
    let modules = match module::list(pool).await {
        Ok(list) => list,
        Err(err) => {
            report.failed += 1;
            tracing::warn!("读取模型列表失败：{err:#}");
            return;
        }
    };
    let categories = match category::list_all(pool).await {
        Ok(list) => list,
        Err(err) => {
            report.failed += 1;
            tracing::warn!("读取栏目列表失败：{err:#}");
            return;
        }
    };
    let rows: Vec<(i64, u32, String)> =
        match sqlx::query_as("SELECT id, category_id, url_token FROM archives ORDER BY id ASC")
            .fetch_all(pool)
            .await
        {
            Ok(list) => list,
            Err(err) => {
                report.failed += 1;
                tracing::warn!("读取内容列表失败：{err:#}");
                return;
            }
        };
    for (id, category_id, token) in rows {
        let Some(current) = categories
            .iter()
            .find(|category| category.id == category_id)
        else {
            continue;
        };
        if current.status != 1 {
            continue;
        }
        let item = match archive::get(pool, id).await {
            Ok(Some(item)) => item,
            Ok(None) => continue,
            Err(err) => {
                report.failed += 1;
                tracing::warn!("读取内容 {id} 失败：{err:#}");
                continue;
            }
        };
        let module_token = modules
            .iter()
            .find(|module| module.id == item.module_id)
            .map(|module| module.url_token.as_str())
            .unwrap_or("");
        let link = item
            .fixed_link
            .as_deref()
            .map(str::trim)
            .filter(|link| !link.is_empty())
            .map(|link| link.to_string())
            .unwrap_or_else(|| {
                crate::models::url::archive_url(
                    mode,
                    &current.url_token,
                    module_token,
                    id,
                    &token,
                    category_id,
                    1,
                )
            });
        let page = render_html_cache_archive(state, pool, id, &item, Some(current), &modules).await;
        html_cache_store(report, "archive", root, &link, page);
    }
}

async fn build_html_cache_tags(
    state: &AppState,
    pool: &sqlx::mysql::MySqlPool,
    root: &std::path::Path,
    report: &mut HtmlCacheReport,
) {
    let mode = crate::models::url::load_mode(pool).await.unwrap_or(0);
    let link = crate::models::url::tag_index_url(mode, 1);
    let page = render_html_cache_tag_index(state, pool).await;
    html_cache_store(report, "tag", root, &link, page);

    let tags: Vec<tag::Tag> = match sqlx::query_as("SELECT * FROM tags ORDER BY id ASC")
        .fetch_all(pool)
        .await
    {
        Ok(list) => list,
        Err(err) => {
            report.failed += 1;
            tracing::warn!("读取标签列表失败：{err:#}");
            return;
        }
    };
    for item in tags.iter().filter(|tag| tag.status == 1) {
        let link = crate::models::url::tag_url(mode, item.id, &item.url_token, 1);
        let page = render_html_cache_tag(state, pool, item).await;
        html_cache_store(report, "tag", root, &link, page);
    }
}

async fn build_html_cache_all(
    state: &AppState,
    pool: &sqlx::mysql::MySqlPool,
) -> HtmlCacheReport {
    let root = html_cache_root(state);
    let mut report = HtmlCacheReport::default();
    if let Err(err) = std::fs::create_dir_all(&root) {
        report.failed += 1;
        tracing::warn!("创建静态缓存目录失败：{err}");
        return report;
    }
    build_html_cache_index(state, pool, &root, &mut report).await;
    build_html_cache_categories(state, pool, &root, &mut report).await;
    build_html_cache_archives(state, pool, &root, &mut report).await;
    build_html_cache_tags(state, pool, &root, &mut report).await;
    report
}

/// `POST /system/api/plugin/htmlcache/build`
async fn manage_htmlcache_build(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = match plugin::html_cache_get(pool).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    if !config_flag(&config, "open") {
        return fail("静态缓存未开启，请先在配置中开启");
    }
    let report = build_html_cache_all(&state, pool).await;
    let now = chrono::Local::now().timestamp();
    let _ = plugin::html_cache_save(pool, &json!({ "last_build_time": now })).await;
    let mut data = report.data();
    if let Some(object) = data.as_object_mut() {
        object.insert("last_build_time".to_string(), json!(now));
    }
    ok(data)
}

/// `POST /system/api/plugin/htmlcache/build/{index,category,archive,tag}`
async fn manage_htmlcache_build_part(
    State(state): State<AppState>,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = match plugin::html_cache_get(pool).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    if !config_flag(&config, "open") {
        return fail("静态缓存未开启，请先在配置中开启");
    }
    let kind = uri
        .path()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    let root = html_cache_root(&state);
    if let Err(err) = std::fs::create_dir_all(&root) {
        return failed(err.into());
    }
    let mut report = HtmlCacheReport::default();
    match kind {
        "index" => build_html_cache_index(&state, pool, &root, &mut report).await,
        "category" => build_html_cache_categories(&state, pool, &root, &mut report).await,
        "archive" => build_html_cache_archives(&state, pool, &root, &mut report).await,
        "tag" => build_html_cache_tags(&state, pool, &root, &mut report).await,
        _ => return fail("未知的静态缓存构建类型"),
    }
    let now = chrono::Local::now().timestamp();
    let _ = plugin::html_cache_save(pool, &json!({ "last_build_time": now })).await;
    let mut data = report.data();
    if let Some(object) = data.as_object_mut() {
        object.insert("type".to_string(), json!(kind));
        object.insert("last_build_time".to_string(), json!(now));
    }
    ok(data)
}

/// `GET /system/api/plugin/htmlcache/build/status`
async fn manage_htmlcache_build_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    Json(json!({ "code": 0, "msg": "", "data": Value::Null })).into_response()
}

/// 统计并清空 `<cache>/html` 下的文件，返回删除数量。
fn clean_html_dir(cache_dir: &std::path::Path) -> usize {
    let root = cache_dir.join("html");
    let mut stack = vec![root.clone()];
    let mut removed = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    let _ = std::fs::remove_dir_all(&root);
    removed
}

/// `POST /system/api/plugin/htmlcache/clean`
async fn manage_htmlcache_clean(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let cache_dir = state.with_config(|config| config.cache_dir());
    let removed = clean_html_dir(&cache_dir);
    ok(json!({ "removed": removed }))
}

/// `POST /system/api/plugin/htmlcache/upload/file`（multipart：`file`）
///
/// 保存 SSH 私钥到 `<data>/cert/htmlcache_ssh_private_key.key`。
async fn manage_htmlcache_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }

    let mut bytes: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" || name == "file1" {
            match field.bytes().await {
                Ok(value) => bytes = Some(value.to_vec()),
                Err(err) => return failed(err.into()),
            }
        } else {
            let _ = field.bytes().await;
        }
    }

    let Some(bytes) = bytes else {
        return fail("请选择要上传的文件");
    };
    if bytes.is_empty() {
        return fail("文件内容为空");
    }

    let cert_dir = state.with_config(|config| config.data_dir()).join("cert");
    if let Err(err) = tokio::fs::create_dir_all(&cert_dir).await {
        return failed(err.into());
    }
    let file_name = "htmlcache_ssh_private_key.key";
    if let Err(err) = tokio::fs::write(cert_dir.join(file_name), &bytes).await {
        return failed(err.into());
    }
    ok(json!(file_name))
}

/// 递归列出目录下的普通文件。
fn html_cache_walk_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    files
}

/// 把本地文件映射成远端相对路径。
fn html_cache_push_entry(
    file: &std::path::Path,
    public_dir: &std::path::Path,
    cache_root: &std::path::Path,
) -> Option<(std::path::PathBuf, String)> {
    if !file.is_file() {
        return None;
    }
    if let Ok(rel) = file.strip_prefix(public_dir) {
        return Some((file.to_path_buf(), rel.to_string_lossy().replace('\\', "/")));
    }
    if let Ok(rel) = file.strip_prefix(cache_root) {
        return Some((file.to_path_buf(), rel.to_string_lossy().replace('\\', "/")));
    }
    let name = file.file_name()?.to_string_lossy().to_string();
    Some((file.to_path_buf(), name))
}

/// 解析请求里传入的推送路径（兼容绝对路径、public 相对路径与 cache/html 相对路径）。
fn html_cache_resolve_push_path(
    raw: &str,
    public_dir: &std::path::Path,
    cache_root: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let direct = std::path::PathBuf::from(raw);
    if direct.is_file() {
        return Some(direct);
    }
    let rel = raw.trim_start_matches('/').replace('\\', "/");
    let under_public = public_dir.join(&rel);
    if under_public.is_file() {
        return Some(under_public);
    }
    let under_cache = cache_root.join(&rel);
    if under_cache.is_file() {
        return Some(under_cache);
    }
    None
}

/// 收集本次需要推送的文件；`paths` 为空时全量收集 public 与 cache/html。
fn collect_html_cache_push_files(
    state: &AppState,
    paths: &[String],
) -> Vec<(std::path::PathBuf, String)> {
    let public_dir = state.with_config(|config| config.public_dir());
    let cache_root = html_cache_root(state);
    let mut entries = Vec::new();
    if paths.is_empty() {
        for file in html_cache_walk_files(&public_dir) {
            if let Some(entry) = html_cache_push_entry(&file, &public_dir, &cache_root) {
                entries.push(entry);
            }
        }
        for file in html_cache_walk_files(&cache_root) {
            if let Some(entry) = html_cache_push_entry(&file, &public_dir, &cache_root) {
                entries.push(entry);
            }
        }
        return entries;
    }
    for raw in paths {
        let Some(file) = html_cache_resolve_push_path(raw, &public_dir, &cache_root) else {
            continue;
        };
        if let Some(entry) = html_cache_push_entry(&file, &public_dir, &cache_root) {
            entries.push(entry);
        }
    }
    entries
}

/// FTP 推送参数。
struct FtpTarget {
    host: String,
    port: u16,
    username: String,
    password: String,
    webroot: String,
}

/// 读取一条 FTP 响应（含多行响应）。
fn ftp_read_reply(
    reader: &mut std::io::BufReader<std::net::TcpStream>,
) -> Result<(u16, String), String> {
    use std::io::BufRead;

    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|err| format!("读取 FTP 响应失败：{err}"))?;
    if line.trim().is_empty() {
        return Err("FTP 连接已关闭".to_string());
    }
    let code = line
        .get(0..3)
        .and_then(|text| text.parse::<u16>().ok())
        .ok_or_else(|| format!("FTP 响应格式错误：{line}"))?;
    let first = line.trim().to_string();
    if line.as_bytes().get(3) == Some(&b'-') {
        loop {
            let mut next = String::new();
            reader
                .read_line(&mut next)
                .map_err(|err| format!("读取 FTP 响应失败：{err}"))?;
            if next.trim().is_empty() {
                return Err("FTP 连接已关闭".to_string());
            }
            if next.as_bytes().get(3) == Some(&b' ') && next.starts_with(&code.to_string()) {
                return Ok((code, next.trim().to_string()));
            }
        }
    }
    Ok((code, first))
}

/// 发送一条 FTP 命令并读取响应。
fn ftp_send(
    reader: &mut std::io::BufReader<std::net::TcpStream>,
    command: &str,
) -> Result<(u16, String), String> {
    use std::io::Write;

    {
        let stream = reader.get_mut();
        stream
            .write_all(format!("{command}\r\n").as_bytes())
            .map_err(|err| format!("发送 FTP 命令失败：{err}"))?;
        stream
            .flush()
            .map_err(|err| format!("发送 FTP 命令失败：{err}"))?;
    }
    ftp_read_reply(reader)
}

/// 解析 PASV 响应中的数据通道地址。
fn ftp_parse_pasv(text: &str) -> Result<std::net::SocketAddr, String> {
    let start = text
        .find('(')
        .ok_or_else(|| format!("FTP PASV 响应无效：{text}"))?;
    let end = text[start..]
        .find(')')
        .map(|offset| start + offset)
        .ok_or_else(|| format!("FTP PASV 响应无效：{text}"))?;
    let parts: Vec<&str> = text[start + 1..end].split(',').collect();
    if parts.len() < 6 {
        return Err(format!("FTP PASV 响应无效：{text}"));
    }
    let mut nums = [0u8; 6];
    for (index, part) in parts.iter().take(6).enumerate() {
        nums[index] = part
            .trim()
            .parse::<u8>()
            .map_err(|_| format!("FTP PASV 响应无效：{text}"))?;
    }
    let host = format!("{}.{}.{}.{}", nums[0], nums[1], nums[2], nums[3]);
    let port = (nums[4] as u16) * 256 + nums[5] as u16;
    format!("{host}:{port}")
        .parse()
        .map_err(|_| "FTP PASV 地址无效".to_string())
}

/// 通过 FTP 上传静态缓存文件（被动模式）。
fn ftp_upload_files(
    target: &FtpTarget,
    files: &[(std::path::PathBuf, String)],
) -> Result<usize, String> {
    use std::io::Write;
    use std::net::TcpStream;
    use std::time::Duration;

    let addr = format!("{}:{}", target.host, target.port);
    let socket_addr: std::net::SocketAddr = addr
        .parse()
        .map_err(|_| format!("FTP 地址无效：{addr}"))?;
    let stream = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(10))
        .map_err(|err| format!("连接 FTP 失败：{err}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(20)));
    let mut reader = std::io::BufReader::new(stream);

    let (code, _) = ftp_read_reply(&mut reader)?;
    if code / 100 != 2 {
        return Err(format!("FTP 握手失败：{code}"));
    }
    let (user_code, _) = ftp_send(&mut reader, &format!("USER {}", target.username))?;
    if user_code == 331 || user_code == 332 {
        let (pass_code, pass_text) = ftp_send(&mut reader, &format!("PASS {}", target.password))?;
        if pass_code / 100 != 2 {
            return Err(format!("FTP 登录失败：{pass_code} {pass_text}"));
        }
    } else if user_code / 100 != 2 {
        return Err(format!("FTP 登录失败：{user_code}"));
    }
    let (type_code, type_text) = ftp_send(&mut reader, "TYPE I")?;
    if type_code / 100 != 2 {
        return Err(format!("FTP 切换二进制模式失败：{type_code} {type_text}"));
    }
    if !target.webroot.trim().is_empty() {
        let webroot = target.webroot.trim_end_matches('/');
        let (cwd_code, cwd_text) = ftp_send(&mut reader, &format!("CWD {webroot}"))?;
        if cwd_code / 100 != 2 {
            return Err(format!("FTP 进入目录 {webroot} 失败：{cwd_code} {cwd_text}"));
        }
    }

    let mut uploaded = 0usize;
    for (local, remote) in files {
        let bytes =
            std::fs::read(local).map_err(|err| format!("读取文件 {} 失败：{err}", local.display()))?;
        let remote = remote.trim_start_matches('/').replace('\\', "/");
        let mut current = String::new();
        if let Some((dirs, _)) = remote.rsplit_once('/') {
            for part in dirs.split('/').filter(|part| !part.is_empty()) {
                current.push_str(part);
                let _ = ftp_send(&mut reader, &format!("MKD {current}"));
                current.push('/');
            }
        }

        let (pasv_code, pasv_text) = ftp_send(&mut reader, "PASV")?;
        if pasv_code != 227 {
            return Err(format!("FTP PASV 失败：{pasv_code} {pasv_text}"));
        }
        let data_addr = ftp_parse_pasv(&pasv_text)?;
        let mut data = TcpStream::connect_timeout(&data_addr, Duration::from_secs(10))
            .map_err(|err| format!("连接 FTP 数据通道失败：{err}"))?;
        let _ = data.set_read_timeout(Some(Duration::from_secs(20)));
        let _ = data.set_write_timeout(Some(Duration::from_secs(20)));

        let (stor_code, stor_text) = ftp_send(&mut reader, &format!("STOR {remote}"))?;
        if stor_code != 150 && stor_code != 125 {
            return Err(format!("FTP 上传 {remote} 失败：{stor_code} {stor_text}"));
        }
        data.write_all(&bytes)
            .map_err(|err| format!("写入 FTP 数据通道失败：{err}"))?;
        let _ = data.flush();
        drop(data);

        let (done_code, done_text) = ftp_read_reply(&mut reader)?;
        if done_code / 100 != 2 {
            return Err(format!(
                "FTP 上传 {remote} 完成确认失败：{done_code} {done_text}"
            ));
        }
        uploaded += 1;
    }
    let _ = ftp_send(&mut reader, "QUIT");
    Ok(uploaded)
}

/// `POST /system/api/plugin/htmlcache/push`
///
/// 请求体可选：`{"paths": ["..."], "all": true}`；`paths` 为空时全量推送。
/// 本地存储只做统计；FTP 走被动模式上传；其余远程存储类型需要额外的存储客户端。
async fn manage_htmlcache_push(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let payload: Value = if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&body).unwrap_or_else(|_| json!({}))
    };
    let paths: Vec<String> = payload
        .get("paths")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let config = match plugin::html_cache_get(pool).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let files = collect_html_cache_push_files(&state, &paths);
    if files.is_empty() {
        return fail("没有找到需要推送的静态缓存文件");
    }
    let storage_type = config
        .get("storage_type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let storage_type = if storage_type.is_empty() {
        "local"
    } else {
        storage_type
    };
    match storage_type {
        "local" => {
            let now = chrono::Local::now().timestamp();
            let _ = plugin::html_cache_save(pool, &json!({ "last_push_time": now })).await;
            let total = files.len();
            ok(json!({
                "total": total,
                "pushed": total,
                "failed": 0,
                "storage_type": "local",
                "last_push_time": now,
            }))
        }
        "ftp" => {
            let target = FtpTarget {
                host: config
                    .get("ftp_host")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                port: config
                    .get("ftp_port")
                    .and_then(Value::as_u64)
                    .unwrap_or(21) as u16,
                username: config
                    .get("ftp_username")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                password: config
                    .get("ftp_password")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                webroot: config
                    .get("ftp_webroot")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string(),
            };
            if target.host.is_empty() || target.username.is_empty() {
                return fail("请先配置 FTP 主机与用户名");
            }
            let total = files.len();
            let result = tokio::task::spawn_blocking(move || ftp_upload_files(&target, &files)).await;
            match result {
                Ok(Ok(uploaded)) => {
                    let now = chrono::Local::now().timestamp();
                    let _ = plugin::html_cache_save(pool, &json!({ "last_push_time": now })).await;
                    ok(json!({
                        "total": total,
                        "pushed": uploaded,
                        "failed": total - uploaded,
                        "storage_type": "ftp",
                        "last_push_time": now,
                    }))
                }
                Ok(Err(err)) => fail(err),
                Err(err) => failed(err.into()),
            }
        }
        other => fail(format!(
            "静态缓存推送暂只支持 local 与 ftp，当前存储类型为 {other}"
        )),
    }
}

/// `GET /system/api/plugin/htmlcache/push/status`
async fn manage_htmlcache_push_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    Json(json!({ "code": 0, "msg": "", "data": Value::Null })).into_response()
}

/// `GET /system/api/plugin/htmlcache/push/logs`
async fn manage_htmlcache_push_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    ok(json!([]))
}

// ---------------------------- AI 内容生成 -----------------------------------

/// `GET /system/api/aigenerate/setting`
async fn manage_aigenerate_setting(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match site_setting::get_section(pool, "ai_generate").await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/aigenerate/setting`
async fn manage_aigenerate_setting_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match site_setting::save_section(pool, "ai_generate", &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/aigenerate/checkapi`
///
/// 对齐 Go `CheckOpenAIAPIValid`：请求配置的 OpenAI 接口地址，
/// 只要能拿到 HTTP 响应就算可达（401 等鉴权失败也算网络通）。
/// 未配置接口地址时直接提示去配置，不再默认请求 OpenAI。
async fn manage_aigenerate_checkapi(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = site_setting::get_section(pool, "ai_generate")
        .await
        .ok()
        .unwrap_or(Value::Null);
    let Some(link) = config_string(&config, &["open_ai_api", "openai_api", "api_url"]) else {
        return fail("未配置 AI 接口，请先在 AI 写作设置中填写 OpenAI 接口地址");
    };
    let request =
        rustcms_ext::http::Request::get(&link).with_timeout(std::time::Duration::from_secs(10));
    match rustcms_ext::http::send(&request).await {
        Ok(_) => ok_msg("服务器可以正常访问 AI 接口"),
        Err(err) => {
            tracing::warn!("检测 AI 接口失败（{link}）：{err:#}");
            let detail = anqi_short_text(&format!("{err:#}"), 120);
            fail(format!("已配置 AI 接口但无法访问：{detail}"))
        }
    }
}

/// `GET /system/api/aigenerate/plans`
async fn manage_aigenerate_plans(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    ok(json!([]))
}

/// AI 写作请求体（对齐 Go `request.KeywordRequest` 的 `id` / `title`）。
#[derive(Debug, Deserialize, Default)]
struct AiGenerateKeywordBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    title: String,
}

/// 读取 AI 写作配置（缺省时返回 `null`，各取值处再兜默认）。
async fn ai_generate_config(pool: &sqlx::MySqlPool) -> Value {
    site_setting::get_section(pool, "ai_generate")
        .await
        .ok()
        .unwrap_or(Value::Null)
}

/// 解析该关键词要写入的分类：关键词自带分类 → 配置的 `category_id` →
/// 配置的 `category_ids` 首个 → 第一个内容列表分类（对齐 Go 兜底逻辑）。
async fn ai_generate_category(
    pool: &sqlx::MySqlPool,
    config: &Value,
    keyword: &collector::Keyword,
) -> Result<u32, String> {
    if keyword.category_id > 0 {
        return Ok(keyword.category_id);
    }
    if let Some(id) = config.get("category_id").and_then(Value::as_u64) {
        if id > 0 {
            return Ok(id as u32);
        }
    }
    if let Some(id) = config
        .get("category_ids")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(Value::as_u64)
    {
        if id > 0 {
            return Ok(id as u32);
        }
    }
    let fallback: Option<u32> =
        sqlx::query_scalar("SELECT id FROM categories WHERE type = 1 ORDER BY sort ASC, id ASC LIMIT 1")
            .fetch_optional(pool)
            .await
            .map_err(|err| format!("查询分类失败：{err}"))?;
    fallback.ok_or_else(|| "请先在 AI 写作配置里选择文档分类".to_string())
}

/// 生成单个关键词的文章并落库（对齐 Go `AnqiAiGenerateArticle` +
/// `AnqiSyncAiPlanResult`）：云端 `/ai/generate` 异步提交，之后每 10 秒
/// `/ai/syncplan` 轮询一次，最多 30 次（5 分钟）。
///
/// 返回 `Ok(None)` 表示同名文档已存在（视为已生成过，跳过）。
async fn ai_generate_article_for_keyword(
    pool: &sqlx::MySqlPool,
    keyword: &collector::Keyword,
) -> Result<Option<i64>, String> {
    let config = ai_generate_config(pool).await;
    let language = config_string(&config, &["language"]).unwrap_or_else(|| "zh".to_string());
    let demand = config_string(&config, &["demand"]).unwrap_or_default();
    let category_id = ai_generate_category(pool, &config, keyword).await?;

    let payload = json!({
        "keyword": keyword.title,
        "language": language,
        "demand": demand,
        "async": true,
    });
    let result = anqi_cloud_post(pool, "/ai/generate", &payload)
        .await?
        .get("data")
        .cloned()
        .unwrap_or(Value::Null);
    let req_id = result.get("req_id").and_then(Value::as_i64).unwrap_or(0);
    if req_id <= 0 {
        return Err("云端未返回任务编号（req_id）".to_string());
    }
    let result = anqi_ai_wait_plan(pool, req_id, result).await?;

    let title = result
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(keyword.title.as_str())
        .to_string();
    let content = result
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if content.trim().is_empty() {
        return Err("云端未返回文章内容".to_string());
    }

    let input = ArchiveInput {
        title,
        content,
        category_id,
        keywords: keyword.title.clone(),
        origin_url: keyword.title.clone(),
        ..Default::default()
    };
    match archive_write::create(pool, &input).await {
        Ok(saved) => Ok(Some(saved.id)),
        // 文档标题已重复 = 该关键词已经生成过
        Err(err) if err.is_invalid() && err.to_string().contains("文档标题已重复") => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}

/// `POST /system/api/aigenerate/article/collect`
///
/// 按指定关键词触发一次 AI 写作（关键词可传 `title` 自动建档，或传 `id`）。
async fn manage_aigenerate_article_collect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AiGenerateKeywordBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if let Err(msg) = ensure_anqi_bound(pool).await {
        return fail(msg);
    }
    let keyword = if !body.title.trim().is_empty() {
        let title = body.title.trim();
        match collector::ensure_keyword(pool, title).await {
            Ok(id) => match collector::get_keyword(pool, id).await {
                Ok(Some(item)) => item,
                Ok(None) => return fail("关键词不存在"),
                Err(err) => return failed(err),
            },
            Err(err) => return failed(err.into()),
        }
    } else {
        match collector::get_keyword(pool, body.id).await {
            Ok(Some(item)) => item,
            Ok(None) => return fail("关键词不存在"),
            Err(err) => return failed(err),
        }
    };

    let pool = pool.clone();
    tokio::spawn(async move {
        match ai_generate_article_for_keyword(&pool, &keyword).await {
            Ok(Some(_)) => {
                if let Err(err) = collector::touch_keyword(&pool, keyword.id, 1).await {
                    tracing::warn!("更新关键词状态失败：{err:#}");
                }
            }
            Ok(None) => {
                if let Err(err) = collector::touch_keyword(&pool, keyword.id, 0).await {
                    tracing::warn!("更新关键词状态失败：{err:#}");
                }
            }
            Err(err) => tracing::warn!("AI 写作失败（{}）：{err}", keyword.title),
        }
    });
    ok_msg("AI生成任务已触发")
}

/// `POST /system/api/aigenerate/article/start`
///
/// 批量触发生成：后台按序处理最多 5 个「从未生成过」的启用关键词。
async fn manage_aigenerate_article_start(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if let Err(msg) = ensure_anqi_bound(pool).await {
        return fail(msg);
    }
    let pool = pool.clone();
    tokio::spawn(async move {
        let keywords = match collector::pending_keywords(&pool, 5).await {
            Ok(list) => list,
            Err(err) => {
                tracing::warn!("读取待生成关键词失败：{err:#}");
                return;
            }
        };
        for keyword in keywords {
            match ai_generate_article_for_keyword(&pool, &keyword).await {
                Ok(_) => {
                    if let Err(err) = collector::touch_keyword(&pool, keyword.id, 1).await {
                        tracing::warn!("更新关键词状态失败：{err:#}");
                    }
                }
                Err(err) => {
                    tracing::warn!("AI 写作失败（{}）：{err}", keyword.title);
                    // 云端/网络异常时直接结束本轮，避免把剩余关键词全部耗在失败上
                    break;
                }
            }
        }
    });
    ok_msg("AI生成任务已触发")
}


// ---------------------------- 城市分站 -------------------------------------

/// 城市站响应体：补 `link` 与默认模板提示。
fn place_payload(item: &place::Place) -> Value {
    let mut value = serde_json::to_value(item).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut value {
        map.insert(
            "link".into(),
            Value::String(format!("/place/{}", item.url_token)),
        );
        if item.template.trim().is_empty() {
            map.insert(
                "template".into(),
                Value::String("(默认)place/detail.html".into()),
            );
        }
    }
    value
}

/// `GET /system/api/plugin/place/setting`
async fn manage_place_setting(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match place::setting_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/place/setting`
async fn manage_place_setting_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match place::setting_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct PlaceListQuery {
    #[serde(default)]
    parent_id: Option<u32>,
    /// 0 = 树形，1 = 单层，2 = 平铺列表
    #[serde(default)]
    show_type: Option<i32>,
    #[serde(default)]
    title: Option<String>,
}

/// `GET /system/api/plugin/place/list`
async fn manage_place_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PlaceListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let (tree, rows) = match place::list(
        pool,
        query.parent_id.unwrap_or(0),
        query.title.as_deref().unwrap_or(""),
        query.show_type.unwrap_or(0),
    )
    .await
    {
        Ok(value) => value,
        Err(err) => return failed(err),
    };

    let data = if tree {
        place::build_tree(&rows)
    } else {
        rows.iter().map(place_payload).collect()
    };
    Json(json!({ "code": 0, "msg": "", "data": data })).into_response()
}

/// `GET /system/api/plugin/place/detail?id=`
async fn manage_place_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match place::get(pool, query.id).await {
        Ok(Some(item)) => ok(place_payload(&item)),
        Ok(None) => fail("城市站不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/place/detail`
async fn manage_place_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<place::PlaceInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match place::save(pool, &input).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "保存成功", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/place/delete`
async fn manage_place_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match place::delete(pool, body.id).await {
        Ok(0) => fail("城市站不存在"),
        Ok(count) => ok(json!({ "count": count })),
        Err(err) => model_error(err),
    }
}

// ---------------------------- 搜索引擎推送 ---------------------------------

/// `GET /system/api/plugin/push`
async fn manage_push(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::push_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/push`
async fn manage_push_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::push_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/push/logs`
async fn manage_push_logs(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::push_logs(pool).await {
        Ok(list) => ok(list),
        Err(err) => failed(err),
    }
}

/// 查询串百分号编码（只保留 RFC 3986 未保留字符）。
fn query_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 向已配置的搜索引擎推送链接，返回（是否至少有一项成功，汇总信息）。
async fn push_to_search_engines(
    site: &str,
    baidu_token: &str,
    bing_key: &str,
    google_json: &str,
    urls: &[String],
) -> (bool, String) {
    let mut success_any = false;
    let mut parts: Vec<String> = Vec::new();

    if !baidu_token.is_empty() {
        // 百度普通收录：POST 纯文本换行分隔的 URL 列表。
        let endpoint = format!(
            "http://data.zz.baidu.com/urls?site={}&token={}",
            query_encode(site),
            query_encode(baidu_token)
        );
        let request = rustcms_ext::http::Request::post(endpoint)
            .with_header("Content-Type", "text/plain; charset=utf-8")
            .with_body(rustcms_ext::http::Body::Bytes(urls.join("\n").into_bytes()));
        match rustcms_ext::http::send(&request).await {
            Ok(response) if response.status == 200 && !response.text().contains("\"error\"") => {
                success_any = true;
                parts.push(format!("百度推送 {} 条成功", urls.len()));
            }
            Ok(response) => parts.push(format!("百度推送失败：{}", response.text().trim())),
            Err(err) => parts.push(format!("百度推送失败：{err}")),
        }
    }

    if !bing_key.is_empty() {
        // Bing URL 提交接口：JSON 里带 siteUrl + urlList。
        let endpoint = format!(
            "https://ssl.bing.com/webmaster/api.svc/json/SubmitUrlbatch?apikey={}",
            query_encode(bing_key)
        );
        let request = rustcms_ext::http::Request::post(endpoint).with_json(json!({
            "siteUrl": site,
            "urlList": urls,
        }));
        match rustcms_ext::http::send(&request).await {
            Ok(response) if response.is_success() => {
                success_any = true;
                parts.push(format!("Bing 推送 {} 条成功", urls.len()));
            }
            Ok(response) => parts.push(format!(
                "Bing 推送失败（HTTP {}）：{}",
                response.status,
                response.text().trim()
            )),
            Err(err) => parts.push(format!("Bing 推送失败：{err}")),
        }
    }

    if !google_json.trim().is_empty() {
        parts.push("Google 推送需要服务账号 OAuth 签名，本包暂未实现".to_string());
    }

    if parts.is_empty() {
        parts.push("未配置可用的搜索引擎推送账号".to_string());
    }
    (success_any, parts.join("；"))
}

#[derive(Debug, Deserialize, Default)]
struct PushUrlsBody {
    #[serde(default)]
    urls: Vec<String>,
}

/// `POST /system/api/plugin/push/push`
///
/// 按 `settings.push` 里配置的百度 / Bing 账号推送链接，并记录一条推送日志。
async fn manage_push_urls(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PushUrlsBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let urls: Vec<String> = body
        .urls
        .iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();
    if urls.is_empty() {
        return fail("请提供要推送的链接");
    }
    let config = match plugin::push_get(pool).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let baidu_token = config["baidu_api"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    let bing_key = config["bing_api"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    let google_json = config["google_json"].as_str().unwrap_or_default().to_string();
    if baidu_token.is_empty() && bing_key.is_empty() && google_json.trim().is_empty() {
        return fail("请先设置搜索引擎推送账号");
    }
    let site = site_base_url(pool).await;
    if (!baidu_token.is_empty() || !bing_key.is_empty()) && site.is_empty() {
        return fail("请先设置站点主域名");
    }

    let count = urls.len();
    let (success, message) =
        push_to_search_engines(&site, &baidu_token, &bing_key, &google_json, &urls).await;

    let entry = json!({
        "time": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        "count": count,
        "status": if success { 1 } else { 0 },
        "msg": message.clone(),
    });
    let _ = plugin::push_log_add(pool, entry).await;

    if success {
        ok(json!({ "count": count, "message": message }))
    } else {
        fail(message)
    }
}

// ---------------------------- 存储配置 -------------------------------------

/// `GET /system/api/plugin/storage`
async fn manage_storage(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::storage_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/storage`
async fn manage_storage_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match plugin::storage_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/storage/upload`（multipart：`file`）
///
/// SSH 私钥落 `<data>/cert/ssh_private_key.key`，并把文件名写回配置。
async fn manage_storage_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let mut bytes: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" || name == "file1" {
            match field.bytes().await {
                Ok(value) => bytes = Some(value.to_vec()),
                Err(err) => return failed(err.into()),
            }
        } else {
            let _ = field.bytes().await;
        }
    }

    let Some(bytes) = bytes else {
        return fail("请选择要上传的文件");
    };
    if bytes.is_empty() {
        return fail("文件内容为空");
    }

    let cert_dir = state.with_config(|config| config.data_dir()).join("cert");
    if let Err(err) = tokio::fs::create_dir_all(&cert_dir).await {
        return failed(err.into());
    }
    let file_name = "ssh_private_key.key";
    if let Err(err) = tokio::fs::write(cert_dir.join(file_name), &bytes).await {
        return failed(err.into());
    }
    if let Err(err) = plugin::storage_save(pool, &json!({ "ssh_private_key": file_name })).await {
        return failed(err);
    }
    ok(json!(file_name))
}

// ---------------------------- 微信 -----------------------------------------

/// 站点主域名（`settings.system.base_url`，去尾部 `/`）。
async fn site_base_url(pool: &sqlx::MySqlPool) -> String {
    site_setting::get_section(pool, site_setting::SYSTEM)
        .await
        .ok()
        .and_then(|value| {
            value
                .get("base_url")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string()
}

/// `GET /system/api/plugin/weapp/config`
async fn manage_weapp_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let base = site_base_url(pool).await;
    match wechat::weapp_get(pool, &base).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/weapp/config`
async fn manage_weapp_config_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match wechat::weapp_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/wechat/config`
async fn manage_wechat_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let base = site_base_url(pool).await;
    match wechat::wechat_get(pool, &base).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/wechat/config`
async fn manage_wechat_config_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match wechat::wechat_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct WechatListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
}

/// `GET /system/api/plugin/wechat/message/list`
async fn manage_wechat_message_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WechatListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match wechat::message_list(
        pool,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/wechat/message/delete`
async fn manage_wechat_message_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match wechat::message_delete(pool, body.id).await {
        Ok(true) => ok_msg("删除成功"),
        Ok(false) => fail("消息不存在"),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct WechatReplyBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    reply: String,
}

/// `POST /system/api/plugin/wechat/message/reply`
///
/// 回复落库；公众号已配置 AppID / AppSecret 时同时通过客服接口推送给用户。
async fn manage_wechat_message_reply(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<WechatReplyBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match wechat::message_reply(pool, body.id, &body.reply).await {
        Ok(saved) => Json(json!({
            "code": 0,
            "msg": "回复成功",
            "data": saved,
        }))
        .into_response(),
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/plugin/wechat/reply/rule/list`
async fn manage_wechat_rule_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WechatListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match wechat::rule_list(
        pool,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/wechat/reply/rule/delete`
async fn manage_wechat_rule_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match wechat::rule_delete(pool, body.id).await {
        Ok(true) => ok_msg("删除成功"),
        Ok(false) => fail("规则不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/wechat/reply/rule/save`
async fn manage_wechat_rule_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<wechat::WechatRuleInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match wechat::rule_save(pool, &input).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "操作成功", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/plugin/wechat/menu/list`
async fn manage_wechat_menu_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match wechat::menu_list(pool).await {
        Ok(list) => ok(list),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/wechat/menu/save`
async fn manage_wechat_menu_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<wechat::WechatMenuInput>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match wechat::menu_save(pool, &input).await {
        Ok(saved) => {
            Json(json!({ "code": 0, "msg": "操作成功", "data": saved })).into_response()
        }
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/wechat/menu/delete`
async fn manage_wechat_menu_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match wechat::menu_delete(pool, body.id).await {
        Ok(0) => fail("菜单不存在"),
        Ok(count) => ok(json!({ "count": count })),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/wechat/menu/sync`
async fn manage_wechat_menu_sync(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match wechat::menu_sync(pool).await {
        Ok(count) => Json(json!({
            "code": 0,
            "msg": "同步成功",
            "data": { "count": count },
        }))
        .into_response(),
        Err(err) => model_error(err),
    }
}

// ---------------------------- 订单 / 财务 ----------------------------------

/// `GET /system/api/plugin/order/config`
async fn manage_order_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match order::config_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/order/config`
async fn manage_order_config_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match order::config_save(pool, &body).await {
        Ok(_) => ok_msg("配置已更新"),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct OrderListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    user_name: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

/// `GET /system/api/plugin/order/list`
async fn manage_order_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<OrderListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let filter = order::OrderFilter {
        order_id: query.order_id.unwrap_or_default(),
        user_name: query.user_name.unwrap_or_default(),
        status: query.status.unwrap_or_default(),
        kind: query.kind.unwrap_or_default(),
    };
    match order::list(
        pool,
        &filter,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct OrderIdQuery {
    #[serde(default)]
    order_id: Option<String>,
}

/// `GET /system/api/plugin/order/detail?order_id=`
async fn manage_order_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<OrderIdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let order_id = query.order_id.unwrap_or_default();
    if order_id.trim().is_empty() {
        return fail("订单号不能为空");
    }
    match order::detail(pool, order_id.trim()).await {
        Ok(value) => ok(value),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct OrderPayBody {
    #[serde(default)]
    order_id: String,
    #[serde(default)]
    pay_way: String,
}

/// `POST /system/api/plugin/order/pay`
async fn manage_order_pay(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<OrderPayBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if body.order_id.trim().is_empty() {
        return fail("订单号不能为空");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match order::set_pay(pool, body.order_id.trim(), &body.pay_way).await {
        Ok(()) => ok_msg("支付成功"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct OrderDeliverBody {
    #[serde(default)]
    order_id: String,
    #[serde(default)]
    express_company: String,
    #[serde(default)]
    tracking_number: String,
}

/// `POST /system/api/plugin/order/deliver`
async fn manage_order_deliver(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<OrderDeliverBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match order::set_deliver(
        pool,
        body.order_id.trim(),
        &body.express_company,
        &body.tracking_number,
    )
    .await
    {
        Ok(()) => ok_msg("设置成功"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct OrderIdBody {
    #[serde(default)]
    order_id: String,
}

/// `POST /system/api/plugin/order/finished`
async fn manage_order_finished(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<OrderIdBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match order::set_finished(pool, body.order_id.trim()).await {
        Ok(()) => ok_msg("设置成功"),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/order/canceled`
async fn manage_order_canceled(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<OrderIdBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match order::set_canceled(pool, body.order_id.trim()).await {
        Ok(()) => ok_msg("设置成功"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct OrderRefundBody {
    #[serde(default)]
    order_id: String,
    #[serde(default)]
    status: i8,
}

/// `POST /system/api/plugin/order/refund`
async fn manage_order_refund(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<OrderRefundBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match order::set_refund(pool, body.order_id.trim(), body.status, false).await {
        Ok(()) => ok_msg("设置成功"),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/order/refund/apply`
async fn manage_order_refund_apply(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<OrderIdBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match order::set_refund(pool, body.order_id.trim(), 0, true).await {
        Ok(()) => ok_msg("申请成功"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct OrderExportBody {
    #[serde(default)]
    status: Option<i8>,
    #[serde(default)]
    start: Option<i64>,
    #[serde(default)]
    end: Option<i64>,
}

/// `POST /system/api/plugin/order/export`
async fn manage_order_export(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<OrderExportBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match order::export(
        pool,
        body.status.unwrap_or(i8::MAX),
        body.start.unwrap_or(0),
        body.end.unwrap_or(0),
    )
    .await
    {
        Ok((header, content)) => ok(json!({ "header": header, "content": content })),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct FinanceListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
}

/// `GET /system/api/plugin/finance/list`
async fn manage_finance_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FinanceListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match order::finance_list(
        pool,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/finance/detail?id=`
async fn manage_finance_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match order::finance_get(pool, query.id).await {
        Ok(Some(item)) => ok(item),
        Ok(None) => fail("财务记录不存在"),
        Err(err) => failed(err),
    }
}

// ---------------------------- 分销员 / 佣金 ---------------------------------

#[derive(Debug, Deserialize, Default)]
struct RetailerListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    id: Option<u32>,
    #[serde(default)]
    user_name: Option<String>,
}

/// `GET /system/api/plugin/retailer/list`
///
/// Go 版 `PluginGetRetailers`：`become_retailer = 1`（自动成为分销员）时只列
/// `is_retailer = 1` 的会员；注意 Go 版 real_name 筛选误读了 `user_name`
/// 参数（`realName := ctx.URLParam("user_name")`），这里保持一致。
async fn manage_retailer_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RetailerListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let become_retailer = retailer::config_get(pool)
        .await
        .ok()
        .and_then(|value| {
            value
                .get("become_retailer")
                .and_then(Value::as_i64)
                .map(|raw| raw == 1)
        })
        .unwrap_or(false);

    let user_name = query.user_name.unwrap_or_default();
    let filter = user_admin::UserFilter {
        id: query.id.unwrap_or(0),
        user_name: user_name.clone(),
        real_name: user_name,
        is_retailer: become_retailer.then_some(true),
        ..Default::default()
    };
    match user_admin::list(
        pool,
        &filter,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/retailer/config`
async fn manage_retailer_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match retailer::config_get(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/retailer/config`
async fn manage_retailer_config_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Value>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match retailer::config_save(pool, &body).await {
        Ok(_) => {
            log_action(pool, &current, &client_ip(&headers, peer), "更新分销员设置").await;
            ok_msg("配置已更新")
        }
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct RetailerRealNameBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    real_name: String,
}

/// `POST /system/api/plugin/retailer/realname`
async fn manage_retailer_realname(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RetailerRealNameBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match retailer::set_real_name(pool, body.id, &body.real_name).await {
        Ok(()) => ok_msg("设置成功"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct RetailerApplyBody {
    #[serde(default)]
    id: u32,
    #[serde(default)]
    is_retailer: i8,
}

/// `POST /system/api/plugin/retailer/apply`
async fn manage_retailer_apply(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RetailerApplyBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match retailer::set_retailer(pool, body.id, body.is_retailer == 1).await {
        Ok(()) => ok_msg("设置成功"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct CommissionListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    user_name: Option<String>,
    #[serde(default)]
    status: Option<i8>,
}

/// `GET /system/api/plugin/commission/list`
async fn manage_commission_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CommissionListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let filter = commission::CommissionFilter {
        user_name: query.user_name.unwrap_or_default(),
        status: query.status,
    };
    match commission::list(
        pool,
        &filter,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/commission/detail?id=`
async fn manage_commission_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match commission::get(pool, query.id).await {
        Ok(Some(item)) => ok(item),
        Ok(None) => fail("佣金记录不存在"),
        Err(err) => model_error(err),
    }
}

// ---------------------------- 支付账户 ---------------------------------------

/// `GET /system/api/plugin/pay/accounts`
///
/// Go 版 `PluginGetPaymentAccounts`：全部支付账户，`id desc`。
async fn manage_pay_accounts(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match payment::accounts(pool).await {
        Ok(rows) => ok(rows),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/pay/detail?id=`
async fn manage_pay_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if query.id == 0 {
        return fail("Invalid ID");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match payment::get(pool, query.id as i64).await {
        Ok(Some(item)) => ok(item),
        Ok(None) => fail("支付账户不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/pay/detail`
///
/// Go 版 `PluginSavePaymentAccount`：新增或更新支付账户（pay_config 整体覆盖）。
async fn manage_pay_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<payment::PaymentAccountSave>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match payment::save(pool, &body).await {
        Ok(account) => {
            log_action(pool, &current, &client_ip(&headers, peer), "更新支付配置").await;
            Json(json!({
                "code": 0,
                "msg": "配置已更新",
                "data": account,
            }))
            .into_response()
        }
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct PayDeleteBody {
    #[serde(default)]
    id: i64,
}

/// `POST /system/api/plugin/pay/delete`
async fn manage_pay_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<PayDeleteBody>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match payment::delete(pool, body.id).await {
        Ok(()) => {
            log_action(pool, &current, &client_ip(&headers, peer), "更新支付配置").await;
            ok_msg("配置已更新")
        }
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/pay/upload`（multipart：`name` + `file`）
///
/// Go 版 `PluginPayUploadFile`：支付证书落 `<data>/cert/<md5>.pem`。
async fn manage_pay_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut multipart: Multipart,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let mut name = String::new();
    let mut bytes: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let field_name = field.name().unwrap_or_default().to_string();
        match field_name.as_str() {
            "name" => name = field.text().await.unwrap_or_default(),
            "file" | "file1" => match field.bytes().await {
                Ok(value) => bytes = Some(value.to_vec()),
                Err(err) => return failed(err.into()),
            },
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".pem") && !lower.ends_with(".crt") && !lower.ends_with(".key") {
        return fail("文件名无效");
    }
    let Some(bytes) = bytes else {
        return fail("请选择要上传的文件");
    };

    let cert_dir = state.with_config(|config| config.data_dir()).join("cert");
    if let Err(err) = tokio::fs::create_dir_all(&cert_dir).await {
        return failed(err.into());
    }
    // 与 Go 一致：按内容 md5 命名，后缀固定 .pem
    let file_name = format!("{}.pem", attachment::md5_hex(&bytes));
    if let Err(err) = tokio::fs::write(cert_dir.join(&file_name), &bytes).await {
        return failed(err.into());
    }

    log_action(
        pool,
        &current,
        &client_ip(&headers, peer),
        format!("上传支付证书 {name}"),
    )
    .await;
    ok(json!(file_name))
}

#[derive(Debug, Deserialize, Default)]
struct PayStatisticQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
    #[serde(default)]
    account_id: i64,
}

/// `GET /system/api/plugin/pay/statistic`
async fn manage_pay_statistic(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PayStatisticQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match payment::statistic(
        pool,
        query.account_id,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

// ---------------------------- 提现 -------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct WithdrawListQuery {
    #[serde(default)]
    current: Option<i64>,
    #[serde(default, rename = "pageSize")]
    page_size: Option<i64>,
}

/// `GET /system/api/plugin/withdraw/list`
async fn manage_withdraw_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WithdrawListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match withdraw::list(
        pool,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    )
    .await
    {
        Ok((total, rows)) => Json(json!({
            "code": 0,
            "msg": "",
            "total": total,
            "data": rows,
        }))
        .into_response(),
        Err(err) => failed(err),
    }
}

/// `GET /system/api/plugin/withdraw/detail?id=`
async fn manage_withdraw_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IdQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match withdraw::get(pool, query.id).await {
        Ok(Some(item)) => ok(item),
        Ok(None) => fail("提现记录不存在"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct WithdrawIdBody {
    #[serde(default)]
    id: u32,
}

/// `POST /system/api/plugin/withdraw/approval`
async fn manage_withdraw_approval(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<WithdrawIdBody>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match withdraw::set_approval(pool, body.id).await {
        Ok(()) => {
            log_action(pool, &current, &client_ip(&headers, peer), "提现审核").await;
            ok_msg("设置成功")
        }
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/withdraw/finished`
async fn manage_withdraw_finished(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<WithdrawIdBody>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match withdraw::set_finished(pool, body.id).await {
        Ok(()) => {
            log_action(pool, &current, &client_ip(&headers, peer), "提现完成").await;
            ok_msg("设置成功")
        }
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct WithdrawApplyBody {
    #[serde(default)]
    user_id: u32,
}

/// `POST /system/api/plugin/withdraw/apply`
///
/// Go 版 `PluginWithdrawSetApply`：为分销员申请提现（汇总待提现佣金）。
async fn manage_withdraw_apply(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<WithdrawApplyBody>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match withdraw::apply(pool, body.user_id).await {
        Ok(item) => {
            log_action(
                pool,
                &current,
                &client_ip(&headers, peer),
                format!("分销员提现申请 user_id={}", body.user_id),
            )
            .await;
            Json(json!({
                "code": 0,
                "msg": "申请成功",
                "data": item,
            }))
            .into_response()
        }
        Err(err) => model_error(err),
    }
}

// ---------------------------- 邮件发送 ---------------------------------------

/// `GET /system/api/plugin/sendmail/list`
///
/// Go 版 `PluginSendmailList`：`<cache>/mail.log` 的 JSON 行，倒序分页。
async fn manage_sendmail_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WithdrawListQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let cache_dir = state.with_config(|config| config.cache_dir());
    let (list, total) = sendmail::mail_log_list(
        &cache_dir,
        query.current.unwrap_or(1),
        query.page_size.unwrap_or(20),
    );
    Json(json!({
        "code": 0,
        "msg": "",
        "data": list,
        "total": total,
    }))
    .into_response()
}

/// `GET /system/api/plugin/sendmail/setting`
///
/// 授权码非空时以 `********` 掩码返回（Go `SendmailPasswordMask`）。
async fn manage_sendmail_setting(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match sendmail::config_get_masked(pool).await {
        Ok(value) => ok(value),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/sendmail/setting`
///
/// 逐字段覆盖；授权码等于掩码时沿用原值（Go `PluginSendmailSettingForm`）。
async fn manage_sendmail_setting_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Value>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    if !body.is_object() {
        return fail("配置格式错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match sendmail::config_save(pool, &body).await {
        Ok(_) => {
            log_action(pool, &current, &client_ip(&headers, peer), "更新邮件发送配置").await;
            ok_msg("配置已更新")
        }
        Err(err) => model_error(err),
    }
}

/// `GET /system/api/plugin/sendmail/templates`
async fn manage_sendmail_templates(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match sendmail::templates_list(pool).await {
        Ok(list) => ok(list),
        Err(err) => failed(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct SendmailKeyQuery {
    #[serde(default)]
    key: String,
}

/// `GET /system/api/plugin/sendmail/template?key=`
async fn manage_sendmail_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SendmailKeyQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if query.key.is_empty() {
        return fail("Missing key parameter");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match sendmail::template_get(pool, query.key.trim()).await {
        Ok(Some(template)) => ok(template),
        Ok(None) => fail("Template not found"),
        Err(err) => failed(err),
    }
}

/// `POST /system/api/plugin/sendmail/template`
async fn manage_sendmail_template_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Value>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match sendmail::template_save(pool, &body).await {
        Ok(template) => {
            log_action(pool, &current, &client_ip(&headers, peer), "更新邮件发送配置").await;
            ok(template)
        }
        Err(err) => model_error(err),
    }
}

/// 组装模板预览的渲染数据（Go `PluginSendmailTemplatePreview` 的 data 常量）。
async fn preview_data(pool: &sqlx::MySqlPool) -> Value {
    let site_name = site_setting::get_section(pool, "system")
        .await
        .ok()
        .and_then(|system| system["site_name"].as_str().map(str::to_string))
        .unwrap_or_default();
    let now = chrono::Utc::now().timestamp();
    json!({
        "system": {"SiteName": site_name},
        "user": {"UserName": "TestUser", "Email": "test@example.com"},
        "url": "https://example.com",
        "order": {
            "Id": 123456,
            "OrderId": "ORD20240915001",
            "Amount": 1999,
            "CreatedTime": now,
            "PaidTime": now,
        },
        "message": {
            "CommOrderId": "ORD20240915001",
            "MessageType": "admin",
            "SenderName": "TestUser",
            "Content": "This is a message content for testing purposes.",
        },
        "guestbook": {
            "Id": 123456,
            "UserName": "TestUser",
            "Email": "test@example.com",
            "Content": "This is a guestbook content for testing purposes.",
            "ExtraData": {"Extra1": "Extra1Value", "Extra2": "Extra2Value"},
            "Contact": "1234567890",
            "Referer": "https://example.com",
            "Ip": "192.168.1.1",
            "CreatedTime": now,
        },
    })
}

/// `POST /system/api/plugin/sendmail/template/preview`
///
/// 只传 key、没传 content 时，用库里的模板补全后再渲染（对齐 Go 版）。
async fn manage_sendmail_template_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let mut subject = body["subject"].as_str().unwrap_or_default().to_string();
    let mut content = body["content"].as_str().unwrap_or_default().to_string();
    let key = body["key"].as_str().unwrap_or_default();
    if content.is_empty() && !key.is_empty() {
        if let Ok(Some(template)) = sendmail::template_get(pool, key).await {
            if subject.is_empty() {
                subject = template["subject"].as_str().unwrap_or_default().to_string();
            }
            content = template["content"].as_str().unwrap_or_default().to_string();
        }
    }
    let data = preview_data(pool).await;
    ok(sendmail::render_preview(&subject, &content, &data))
}

/// 把邮件配置转换成 ext 的发信账号；未配置服务器或账号时返回 None。
fn sendmail_smtp_account(config: &Value) -> Option<rustcms_ext::smtp::SmtpAccount> {
    let server = config["server"].as_str().unwrap_or_default().trim();
    let account = config["account"].as_str().unwrap_or_default().trim();
    if server.is_empty() || account.is_empty() {
        return None;
    }
    let use_ssl = match &config["use_ssl"] {
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_i64().unwrap_or(0) != 0,
        Value::String(text) => matches!(text.trim(), "1" | "true" | "True" | "yes"),
        _ => false,
    };
    let port = config["port"]
        .as_i64()
        .filter(|value| (1..=65535).contains(value))
        .map(|value| value as u16)
        .unwrap_or(if use_ssl { 465 } else { 25 });
    let from_email = if account.contains('@') {
        account.to_string()
    } else {
        format!("{account}@{server}")
    };
    let mut smtp = rustcms_ext::smtp::SmtpAccount::new(
        server.to_string(),
        port,
        account.to_string(),
        config["password"].as_str().unwrap_or_default().to_string(),
    )
    .with_sender(from_email, String::new());
    if use_ssl {
        smtp = smtp.with_tls(rustcms_ext::smtp::TlsMode::Implicit);
    }
    Some(smtp)
}

/// 用 ext 的 SMTP 客户端发一封信（阻塞调用，需放在 spawn_blocking 里）。
fn ext_smtp_send(
    account: &rustcms_ext::smtp::SmtpAccount,
    to: &str,
    subject: &str,
    html: &str,
) -> anyhow::Result<()> {
    let config = rustcms_ext::smtp::SmtpConfig::single(account.clone());
    rustcms_ext::smtp::send(&config, &rustcms_ext::smtp::Mail::new(to, subject, html)).map(|_| ())
}

/// 追加一条发送记录到 `<cache>/mail.log`（与 `sendmail::mail_log_list` 的格式一致）。
fn append_mail_log(cache_dir: &std::path::Path, subject: &str, status: &str, address: &str) {
    use std::io::Write;
    let _ = std::fs::create_dir_all(cache_dir);
    let line = json!({
        "created_time": chrono::Utc::now().timestamp(),
        "subject": subject,
        "status": status,
        "address": address,
    })
    .to_string();
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(cache_dir.join(sendmail::MAIL_LOG_FILE))
    {
        let _ = writeln!(file, "{line}");
    }
}

/// `POST /system/api/plugin/sendmail/test`
///
/// 发送一封测试邮件；收件人优先取请求体里的 recipient/to/email，缺省用配置里的 recipient。
async fn manage_sendmail_test(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let config = match sendmail::config_get(pool).await {
        Ok(value) => value,
        Err(err) => return failed(err),
    };
    let Some(account) = sendmail_smtp_account(&config) else {
        return fail("请先设置邮件发送账户");
    };
    let recipient = ["recipient", "to", "email"]
        .iter()
        .find_map(|key| {
            body.get(*key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            config
                .get("recipient")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        });
    let Some(recipient) = recipient else {
        return fail("请填写收件邮箱地址");
    };

    let cache_dir = state.with_config(|config| config.cache_dir());
    let subject = "测试邮件";
    let html = "<p>这是一封测试邮件，收到说明邮件发送配置正常。</p>";
    let to = recipient.clone();
    let result = tokio::task::spawn_blocking(move || {
        ext_smtp_send(&account, &to, subject, html)
    })
    .await;
    match result {
        Ok(Ok(())) => {
            append_mail_log(&cache_dir, subject, "success", &recipient);
            ok_msg("发送成功")
        }
        Ok(Err(err)) => {
            append_mail_log(&cache_dir, subject, "failed", &recipient);
            fail(format!("发送失败：{err}"))
        }
        Err(err) => failed(anyhow::anyhow!("发送任务异常：{err}")),
    }
}

// ---------------------------- 网站备份 ---------------------------------------

fn backup_dir(state: &AppState) -> std::path::PathBuf {
    state.with_config(|config| config.data_dir()).join("backup")
}

/// `GET /system/api/plugin/backup/list`
async fn manage_backup_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    ok(backup::list(&backup_dir(&state)))
}

/// `POST /system/api/plugin/backup/dump`
///
/// 异步执行备份（无 zip 依赖，产出纯 SQL 文件），立即返回（Go 版同样异步）。
async fn manage_backup_dump(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let pool = pool.clone();
    if let Err(err) = backup::new_task("backup") {
        return model_error(err);
    }
    let dir = backup_dir(&state);
    let admin = current.clone();
    let ip = client_ip(&headers, peer);
    tokio::spawn(async move {
        if let Err(err) = backup::dump(&pool, &dir).await {
            backup::finish_status("backup", format!("{err}"));
        }
        log_action(&pool, &admin, &ip, "备份数据").await;
    });
    ok_msg("备份已开始")
}

/// `GET /system/api/plugin/backup/status`
async fn manage_backup_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    match backup::get_status() {
        Some(status) => ok(status),
        None => Json(json!({
            "code": 0,
            "msg": "当前没有正在执行的任务",
            "data": Value::Null,
        }))
        .into_response(),
    }
}

/// `POST /system/api/plugin/backup/restore`
///
/// 异步执行恢复；`.zip` 备份明确未实现。Go 版恢复后会重载配置并重启，
/// 本项目为单进程，不自动重启（模板引擎需重启进程后生效）。
async fn manage_backup_restore(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Value>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let pool = pool.clone();
    let name = body["name"].as_str().unwrap_or_default().to_string();
    if let Err(err) = backup::new_task("restore") {
        return model_error(err);
    }
    let dir = backup_dir(&state);
    let admin = current.clone();
    let ip = client_ip(&headers, peer);
    let restore_name = name.clone();
    tokio::spawn(async move {
        if let Err(err) = backup::restore(&pool, &dir, &restore_name).await {
            backup::finish_status("restore", format!("{err}"));
        }
        log_action(&pool, &admin, &ip, "从备份恢复数据").await;
    });
    ok_msg("恢复已开始")
}

/// `POST /system/api/plugin/backup/delete`
async fn manage_backup_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Value>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let name = body["name"].as_str().unwrap_or_default().to_string();
    match backup::delete(&backup_dir(&state), name.trim()) {
        Ok(()) => {
            let Some(pool) = state.db() else {
                return fail(NO_DB);
            };
            log_action(pool, &current, &client_ip(&headers, peer), "删除备份文件").await;
            ok_msg("处理成功")
        }
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct BackupNameQuery {
    #[serde(default)]
    name: String,
}

/// `GET /system/api/plugin/backup/export?name=`
///
/// 把备份文件作为附件返回（浏览器下载；Go 版允许 query 传 token，
/// 这里仍走 `require_admin`）。
async fn manage_backup_export(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<BackupNameQuery>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    if query.name.is_empty() {
        return fail("备份文件不存在");
    }
    let path = match backup::sanitize_path(&backup_dir(&state), query.name.trim()) {
        Ok(path) => path,
        Err(err) => return model_error(err),
    };
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(_) => return fail("备份文件不存在"),
    };
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("site")
        .to_string();
    let file_name = format!(
        "{}-{}",
        host.split(':').next().unwrap_or("site"),
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default()
    );
    (
        [
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{file_name}\""),
            ),
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
        ],
        bytes,
    )
        .into_response()
}

/// `POST /system/api/plugin/backup/import`（multipart：`file`）
///
/// 只收单文件上传；Go 版的分片上传（chunks/chunk/md5）字段读掉但不支持。
async fn manage_backup_import(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut multipart: Multipart,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let mut file_name = String::new();
    let mut bytes: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" => {
                file_name = field
                    .file_name()
                    .unwrap_or_default()
                    .to_string();
                match field.bytes().await {
                    Ok(value) => bytes = Some(value.to_vec()),
                    Err(err) => return failed(err.into()),
                }
            }
            _ => {
                // chunks / chunk / file_name / md5 等分片字段：读掉即可
                let _ = field.bytes().await;
            }
        }
    }
    let Some(bytes) = bytes else {
        return fail("请选择要上传的文件");
    };
    if file_name.is_empty() {
        return fail("导入的备份文件格式不正确");
    }
    match backup::import(&backup_dir(&state), &file_name, &bytes) {
        Ok(()) => {
            let Some(pool) = state.db() else {
                return fail(NO_DB);
            };
            log_action(
                pool,
                &current,
                &client_ip(&headers, peer),
                format!("导入备份文件 {file_name}"),
            )
            .await;
            Json(json!({
                "code": 0,
                "msg": "备份文件导入完成",
                "data": {"status": "success", "file": file_name},
            }))
            .into_response()
        }
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct BackupCleanupBody {
    #[serde(default)]
    clean_uploads: bool,
}

/// `POST /system/api/plugin/backup/cleanup`
///
/// 一键清空整站数据（TRUNCATE 除管理员/设置外的全部表，可选删 uploads）。
async fn manage_backup_cleanup(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<BackupCleanupBody>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let public_dir = state.with_config(|config| config.public_dir());
    match backup::cleanup(pool, &public_dir, body.clean_uploads).await {
        Ok(_) => {
            log_action(
                pool,
                &current,
                &client_ip(&headers, peer),
                "一键清空网站数据",
            )
            .await;
            ok_msg("清理完成")
        }
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/backup/remark`
async fn manage_backup_remark(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Value>,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };
    let name = body["name"].as_str().unwrap_or_default().to_string();
    if name.is_empty() {
        return fail("备份文件不存在");
    }
    let remark = body["remark"].as_str().unwrap_or_default().to_string();
    match backup::set_remark(&backup_dir(&state), name.trim(), &remark) {
        Ok(()) => {
            let Some(pool) = state.db() else {
                return fail(NO_DB);
            };
            log_action(
                pool,
                &current,
                &client_ip(&headers, peer),
                format!("更新备份备注 {name}"),
            )
            .await;
            ok_msg("处理成功")
        }
        Err(err) => model_error(err),
    }
}

/// 便于测试：数据库可用时把连接池塞进状态。
#[cfg(test)]
async fn test_app() -> Option<Router> {
    test_app_with_token(None).await
}

#[cfg(test)]
async fn test_app_with_token(token: Option<&str>) -> Option<Router> {
    let pool = crate::models::test_pool().await;
    // 测试进程的 CWD 不一定是 crate 根，模板目录用 manifest 相对路径固定下来
    let template_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../template/default")
        .to_string_lossy()
        .to_string();
    // 上传类用例把文件写到临时目录，绝不碰仓库里的 public/uploads
    let public_dir = std::env::temp_dir()
        .join(format!("rustcms-public-{}", std::process::id()))
        .to_string_lossy()
        .to_string();
    // 让 data_dir 指向临时目录（技能用例会写文件，不能写进仓库 data/）
    let config_path = std::env::temp_dir()
        .join(format!("rustcms-cfg-{}", std::process::id()))
        .join("config.json");
    let config = crate::config::ServerJson {
        api_token: token.map(str::to_string),
        template_dir,
        public_dir,
        path: Some(config_path),
        ..Default::default()
    };
    Some(router(AppState::new(config, pool)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use crate::config::ServerJson;

    fn app() -> Router {
        router(AppState::new(ServerJson::default(), None))
    }

    async fn body_text(res: Response) -> String {
        let bytes = axum::body::to_bytes(res.into_body(), 64 * 1024)
            .await
            .expect("读取响应体失败");
        String::from_utf8_lossy(&bytes).to_string()
    }

    async fn body_json(res: Response) -> Value {
        serde_json::from_str(&body_text(res).await).expect("响应不是合法 JSON")
    }

    fn request(method: &str, uri: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn health_reports_ok() {
        let res = app().oneshot(request("GET", "/api/health")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_text(res).await;
        assert!(body.contains("\"code\":0"), "响应体：{body}");
        assert!(body.contains("\"db\":false"), "响应体：{body}");
    }

    #[tokio::test]
    async fn unknown_api_returns_json_404() {
        let res = app().oneshot(request("GET", "/api/nope")).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let body = body_text(res).await;
        assert!(body.contains("接口不存在"), "响应体：{body}");
    }

    #[tokio::test]
    async fn unknown_page_returns_plain_404() {
        let res = app().oneshot(request("GET", "/nope.html")).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_text(res).await, "404 page not found");
    }

    #[tokio::test]
    async fn root_shows_version() {
        let res = app().oneshot(request("GET", "/")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(body_text(res).await.contains(env!("CARGO_PKG_VERSION")));
    }

    #[tokio::test]
    async fn api_without_db_reports_code_1() {
        let res = app()
            .oneshot(request("GET", "/api/categories"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_json(res).await;
        assert_eq!(body["code"], 1);
        assert_eq!(body["msg"], NO_DB);
    }

    #[tokio::test]
    async fn serves_categories_and_archives_from_db() {
        let _guard = crate::models::db_lock().await;
        let Some(app) = test_app().await else {
            return;
        };

        // 分类列表
        let res = app
            .clone()
            .oneshot(request("GET", "/api/categories"))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0);
        let flat_len = body["data"].as_array().unwrap().len();
        assert!(flat_len > 0, "库里应至少有一个分类");

        // 树形
        let res = app
            .clone()
            .oneshot(request("GET", "/api/categories?tree=1"))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0);
        let roots = body["data"].as_array().unwrap();
        assert!(roots.iter().all(|node| node["parent_id"] == 0));

        // 分类详情 + 不存在
        let res = app
            .clone()
            .oneshot(request("GET", "/api/categories/1"))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["id"], 1);

        let res = app
            .clone()
            .oneshot(request("GET", "/api/categories/4294967295"))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 内容列表分页
        let res = app
            .clone()
            .oneshot(request("GET", "/api/archives?page=1&page_size=5"))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["page_size"], 5);
        let list = body["data"]["list"].as_array().unwrap();
        assert!(list.len() <= 5 && !list.is_empty());
        assert!(body["data"]["total"].as_i64().unwrap() >= list.len() as i64);

        // 内容详情：带正文
        let id = list[0]["id"].as_i64().unwrap();
        let res = app
            .clone()
            .oneshot(request("GET", &format!("/api/archives/{id}")))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["id"], id);
        assert!(body["data"]["content"].is_string());

        // 不存在的内容
        let res = app
            .oneshot(request("GET", "/api/archives/99999999"))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1);
        assert_eq!(body["msg"], "内容不存在");
    }

    /// 不带客户端地址的 JSON 请求（走默认的 127.0.0.1）。
    fn json_request(method: &str, uri: &str, token: Option<&str>, body: Value) -> Request<Body> {
        json_request_from("127.0.0.1", method, uri, token, body)
    }

    /// 指定客户端 IP 的 JSON 请求。
    ///
    /// 后台登录要读 TCP 对端地址（`ConnectInfo`），`oneshot` 调用没有真实连接，
    /// 所以这里手工塞一个 extension。
    fn json_request_from(
        ip: &str,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Value,
    ) -> Request<Body> {
        let peer: SocketAddr = format!("{ip}:50000").parse().expect("客户端地址格式错误");
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .extension(ConnectInfo(peer));
        if let Some(token) = token {
            builder = builder.header("x-api-token", token);
        }
        builder.body(Body::from(body.to_string())).unwrap()
    }

    /// 带 Bearer 令牌的请求（客户端地址固定 127.0.0.1）。
    fn bearer_request(method: &str, uri: &str, token: &str, body: Value) -> Request<Body> {
        bearer_request_from("127.0.0.1", method, uri, token, body)
    }

    /// 带 Bearer 令牌、指定客户端 IP 的请求。
    ///
    /// 后台写接口会读 `ConnectInfo` 记操作日志的 IP，`oneshot` 没有真实连接，得手工塞 extension。
    fn bearer_request_from(
        ip: &str,
        method: &str,
        uri: &str,
        token: &str,
        body: Value,
    ) -> Request<Body> {
        let peer: SocketAddr = format!("{ip}:50000").parse().expect("客户端地址格式错误");
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .extension(ConnectInfo(peer))
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    /// 造一个临时管理员（不动真实账号），默认放进 1 号用户组（= 超管）。
    async fn seed_temp_admin(pool: &sqlx::MySqlPool, id: u32, user_name: &str, password: &str) {
        seed_temp_admin_in_group(pool, id, user_name, password, 1).await;
    }

    /// 造一个指定用户组的临时管理员。
    async fn seed_temp_admin_in_group(
        pool: &sqlx::MySqlPool,
        id: u32,
        user_name: &str,
        password: &str,
        group_id: u32,
    ) {
        let hash = crate::auth::hash_password(password).unwrap();
        let _ = sqlx::query("DELETE FROM admins WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await;
        sqlx::query(
            "INSERT INTO admins (id, created_time, updated_time, user_name, password, status, \
                login_time, group_id) \
             VALUES (?, UNIX_TIMESTAMP(), UNIX_TIMESTAMP(), ?, ?, 1, 0, ?)",
        )
        .bind(id)
        .bind(user_name)
        .bind(hash)
        .bind(group_id)
        .execute(pool)
        .await
        .unwrap();
    }

    /// 清掉测试造的管理员与它们产生的登录日志。
    async fn drop_temp_admin(pool: &sqlx::MySqlPool) {
        for id in [990002u32, 990003, 990004, 990005, 990006] {
            let _ = sqlx::query("DELETE FROM admins WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await;
            let _ = sqlx::query("DELETE FROM admin_login_logs WHERE admin_id = ?")
                .bind(id)
                .execute(pool)
                .await;
        }
        // 接口新建的账号自增 ID 不固定，按用户名前缀兜底（真实账号不会这么起名）
        let _ = sqlx::query("DELETE FROM admins WHERE user_name LIKE 'rustcms_%'")
            .execute(pool)
            .await;
        // 用户名不存在的失败日志里 admin_id 是 0，只能按用户名清
        let _ = sqlx::query("DELETE FROM admin_login_logs WHERE user_name LIKE 'rustcms_%'")
            .execute(pool)
            .await;
        // 测试新建的用户组（真实组名不会这么起）
        let _ = sqlx::query("DELETE FROM admin_groups WHERE title LIKE 'rustcms\\_%'")
            .execute(pool)
            .await;
        // 操作日志同样按测试账号用户名清
        let _ = sqlx::query("DELETE FROM admin_logs WHERE user_name LIKE 'rustcms\\_%'")
            .execute(pool)
            .await;
    }

    #[tokio::test]
    async fn admin_login_and_jwt_write_flow() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let Some(app) = test_app().await else {
            return;
        };
        crate::models::archive_write::purge_test_archives(&pool, "Rust 鉴权测试").await;
        seed_temp_admin(&pool, 990002, "rustcms_test_admin", "test-password-123").await;

        // 密码错误
        let res = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/admin/login",
                None,
                json!({"user_name": "rustcms_test_admin", "password": "wrong"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1);
        assert_eq!(body["msg"], "用户名或密码错误");

        // 登录成功
        let res = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/admin/login",
                None,
                json!({"user_name": "rustcms_test_admin", "password": "test-password-123", "remember": true}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let token = body["data"]["token"].as_str().unwrap().to_string();
        assert!(!token.is_empty());
        assert_eq!(body["data"]["admin"]["user_name"], "rustcms_test_admin");

        // me
        let res = app
            .clone()
            .oneshot(bearer_request("GET", "/api/admin/me", &token, Value::Null))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["user_name"], "rustcms_test_admin");
        // 密码哈希不应该被序列化出去
        assert!(body["data"].get("password").is_none(), "{body}");

        // 用 JWT 写内容（测试环境没有配置 api_token）
        let categories = crate::models::category::list_all(&pool).await.unwrap();
        let Some(target) = categories.iter().find(|item| item.kind == 1) else {
            drop_temp_admin(&pool).await;
            return;
        };
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/archives",
                &token,
                json!({
                    "title": "Rust 鉴权测试文档",
                    "content": "<p>jwt 写入</p>",
                    "category_id": target.id,
                }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let id = body["data"]["id"].as_i64().unwrap();

        // 坏令牌被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "DELETE",
                &format!("/api/archives/{id}"),
                "bad-token",
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1);
        assert!(
            body["msg"].as_str().unwrap().contains("登录已失效"),
            "{body}"
        );

        // 正常删除（也走 JWT）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "DELETE",
                &format!("/api/archives/{id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");

        // 会员登录：库里有真实会员，但密码未知，这里只验证错误分支
        let res = app
            .oneshot(json_request(
                "POST",
                "/api/user/login",
                None,
                json!({"user_name": "member_test", "password": "definitely-wrong"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1);

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn admin_login_locks_after_repeated_failures() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };

        let user_name = "rustcms_lock_admin";
        let password = "right-password-123";
        let ip = "10.9.9.9";
        seed_temp_admin(&pool, 990003, user_name, password).await;
        let _ = sqlx::query("DELETE FROM admin_login_logs WHERE admin_id = 990003")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM admin_login_logs WHERE user_name = ?")
            .bind(user_name)
            .execute(&pool)
            .await;

        let Some(app) = test_app().await else {
            return;
        };

        // 连续 5 次密码错误
        for _ in 0..login_guard::MAX_TIMES {
            let res = app
                .clone()
                .oneshot(json_request_from(
                    ip,
                    "POST",
                    "/api/admin/login",
                    None,
                    json!({"user_name": user_name, "password": "wrong"}),
                ))
                .await
                .unwrap();
            let body = body_json(res).await;
            assert_eq!(body["code"], 1, "{body}");
            assert_eq!(body["msg"], "用户名或密码错误");
        }

        // 第 6 次：密码正确也进不来（账号已被锁）
        let res = app
            .clone()
            .oneshot(json_request_from(
                ip,
                "POST",
                "/api/admin/login",
                None,
                json!({"user_name": user_name, "password": password}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], LOCKED);

        // 换个 IP 也一样：账号级计数不受 IP 影响
        let res = app
            .clone()
            .oneshot(json_request_from(
                "10.9.9.10",
                "POST",
                "/api/admin/login",
                None,
                json!({"user_name": user_name, "password": password}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], LOCKED, "{body}");

        // 失败日志已落库
        let failed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM admin_login_logs WHERE admin_id = 990003 AND status = 0",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            failed >= login_guard::MAX_TIMES as i64,
            "失败日志条数={failed}"
        );

        // 用户名不存在时也要累计 IP 失败数，避免轮换用户名枚举
        let Some(app_ip) = test_app().await else {
            return;
        };
        let bad_ip = "10.9.9.11";
        for _ in 0..login_guard::MAX_TIMES {
            let res = app_ip
                .clone()
                .oneshot(json_request_from(
                    bad_ip,
                    "POST",
                    "/api/admin/login",
                    None,
                    json!({"user_name": "rustcms_no_such_admin", "password": "x"}),
                ))
                .await
                .unwrap();
            let body = body_json(res).await;
            assert_eq!(body["msg"], "用户名或密码错误", "{body}");
        }
        let res = app_ip
            .oneshot(json_request_from(
                bad_ip,
                "POST",
                "/api/admin/login",
                None,
                json!({"user_name": user_name, "password": password}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], LOCKED, "IP 级计数没生效：{body}");

        // 进程状态清空（等价于 10 分钟锁定窗口过去）后能正常登录
        let Some(fresh) = test_app().await else {
            return;
        };
        let res = fresh
            .clone()
            .oneshot(json_request_from(
                ip,
                "POST",
                "/api/admin/login",
                None,
                json!({"user_name": user_name, "password": password}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["admin"]["user_name"], user_name);
        // 1 号用户组即超管（Go 版 IsSuper）
        assert_eq!(body["data"]["admin"]["is_super"], true, "{body}");

        let ok_logs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM admin_login_logs WHERE admin_id = 990003 AND status = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(ok_logs, 1, "成功日志应恰好一条");

        // 退出登录：令牌是无状态的，只回提示
        let res = fresh
            .oneshot(json_request("POST", "/api/admin/logout", None, Value::Null))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["msg"], "已退出登录");

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn admin_detail_updates_profile_and_password() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };

        let admin_name = "rustcms_profile_admin";
        let staff_name = "rustcms_profile_staff";
        seed_temp_admin(&pool, 990004, admin_name, "profile-pass-123").await;
        seed_temp_admin_in_group(&pool, 990005, staff_name, "staff-pass-123", 2).await;
        let _ = sqlx::query("DELETE FROM admin_login_logs WHERE user_name LIKE 'rustcms_%'")
            .execute(&pool)
            .await;

        let Some(app) = test_app().await else {
            return;
        };

        // 未登录不能读资料
        let res = app
            .clone()
            .oneshot(json_request("GET", "/api/admin/detail", None, Value::Null))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "请先登录");

        // 超管登录
        let token = login_admin(&app, "10.8.8.1", admin_name, "profile-pass-123").await;

        // 不带 id 取自己
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/detail",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["user_name"], admin_name);
        // 密码哈希不参与序列化
        assert!(body["data"].get("password").is_none(), "{body}");
        assert_eq!(body["data"]["is_super"], true, "{body}");

        // 超管能看别人
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/detail?id=990005",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["user_name"], staff_name);
        assert_eq!(body["data"]["group_id"], 2, "{body}");
        assert_eq!(body["data"]["is_super"], false, "{body}");

        // 不存在的管理员
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/detail?id=9999999",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "用户不存在");

        // 新建时撞上已有账号
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/detail",
                &token,
                json!({"user_name": admin_name, "password": "whatever", "group_id": 1}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], "该账号已存在", "{body}");

        // 账号不能为空
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/detail",
                &token,
                json!({"id": 990004, "user_name": "  ", "group_id": 1}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], "账号不能为空", "{body}");

        // 不带旧密码也能改密（Go 版就是这样：后台已登录即可重置）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/detail",
                &token,
                json!({"id": 990004, "user_name": admin_name, "password": "new-pass-456", "group_id": 1}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["msg"], "管理员信息已更新");

        // 旧密码失效、新密码可用
        let res = app
            .clone()
            .oneshot(json_request_from(
                "10.8.8.2",
                "POST",
                "/api/admin/login",
                None,
                json!({"user_name": admin_name, "password": "profile-pass-123"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["msg"], "用户名或密码错误");
        let token = login_admin(&app, "10.8.8.2", admin_name, "new-pass-456").await;

        // 旧密码不对时拒绝改密，且密码保持不变
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/detail",
                &token,
                json!({"id": 990004, "user_name": admin_name, "password": "ignored-999", "old_password": "wrong-old", "group_id": 1}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "当前密码不正确");
        login_admin(&app, "10.8.8.3", admin_name, "new-pass-456").await;

        // 旧密码正确才改
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/detail",
                &token,
                json!({"id": 990004, "user_name": admin_name, "password": "final-pass-789", "old_password": "new-pass-456", "group_id": 1}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let token = login_admin(&app, "10.8.8.4", admin_name, "final-pass-789").await;

        // 改自己的状态会被强制回启用，避免把自己锁在门外
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/detail",
                &token,
                json!({"id": 990004, "user_name": admin_name, "status": 0, "group_id": 1}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let status: u8 = sqlx::query_scalar("SELECT status FROM admins WHERE id = 990004")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, 1);

        // 改名撞上别人的账号同样被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/detail",
                &token,
                json!({"id": 990004, "user_name": staff_name, "group_id": 1}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], "该账号已存在", "{body}");

        // 非超管只能看/改自己
        let staff_token = login_admin(&app, "10.8.8.5", staff_name, "staff-pass-123").await;
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/detail?id=990004",
                &staff_token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], NO_ADMIN_PERMISSION, "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/detail",
                &staff_token,
                json!({"id": 990004, "user_name": "rustcms_hijack", "group_id": 1}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], NO_ADMIN_PERMISSION, "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/detail",
                &staff_token,
                json!({"user_name": "rustcms_created_by_staff", "password": "x", "group_id": 2}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], NO_ADMIN_PERMISSION, "{body}");

        // 自己看自己没问题
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/detail",
                &staff_token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["is_super"], false, "{body}");

        // 超管新建账号：必须给密码
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/detail",
                &token,
                json!({"user_name": "rustcms_profile_nopass", "group_id": 2}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], "请输入密码", "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/detail",
                &token,
                json!({"user_name": "rustcms_profile_new", "password": "brand-new-pass", "group_id": 2, "status": 1}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let new_id = body["data"]["id"].as_u64().unwrap();
        assert!(new_id > 0, "{body}");
        // 新账号能登录，且落在 2 号组（非超管）
        let res = app
            .clone()
            .oneshot(json_request_from(
                "10.8.8.6",
                "POST",
                "/api/admin/login",
                None,
                json!({"user_name": "rustcms_profile_new", "password": "brand-new-pass"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["admin"]["is_super"], false, "{body}");

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn admin_login_logs_are_paginated_and_scoped() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };

        let admin_name = "rustcms_log_admin";
        let staff_name = "rustcms_log_staff";
        seed_temp_admin(&pool, 990004, admin_name, "log-pass-123").await;
        seed_temp_admin_in_group(&pool, 990005, staff_name, "log-pass-456", 2).await;
        let _ = sqlx::query("DELETE FROM admin_login_logs WHERE user_name LIKE 'rustcms_%'")
            .execute(&pool)
            .await;

        let Some(app) = test_app().await else {
            return;
        };

        // 未登录不能看日志
        let res = app
            .clone()
            .oneshot(json_request(
                "GET",
                "/api/admin/logs/login",
                None,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 造两条成功日志（超管 + 普通管理员各一条）
        let token = login_admin(&app, "10.7.7.1", admin_name, "log-pass-123").await;
        let staff_token = login_admin(&app, "10.7.7.2", staff_name, "log-pass-456").await;

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/logs/login?pageSize=5",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        // 信封与其它接口不同：total 在顶层
        assert!(body["total"].as_i64().unwrap() >= 2, "{body}");
        let list = body["data"].as_array().unwrap();
        assert!(list.len() <= 5 && !list.is_empty(), "{body}");
        // 倒序：id 递减
        let ids: Vec<i64> = list
            .iter()
            .map(|item| item["id"].as_i64().unwrap())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(ids, sorted, "应按 id 倒序：{body}");
        // 不返回密码列
        assert!(list[0].get("password").is_none(), "{body}");

        // 分页：每页 1 条，第 2 页应是与第 1 页不同的记录
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/logs/login?current=1&pageSize=1",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let first = body_json(res).await;
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/logs/login?current=2&pageSize=1",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let second = body_json(res).await;
        assert_eq!(first["data"].as_array().unwrap().len(), 1, "{first}");
        assert_eq!(second["data"].as_array().unwrap().len(), 1, "{second}");
        assert_ne!(
            first["data"][0]["id"], second["data"][0]["id"],
            "{first}{second}"
        );
        assert_eq!(first["total"], second["total"], "总数与页码无关");

        // 非法页码回落默认值，不会报错
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/logs/login?current=0&pageSize=0",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        // pageSize=0 应回落到默认 20 条（而不是被夹成 1 条）
        assert!(
            body["data"].as_array().unwrap().len() >= 2,
            "默认每页条数被夹成 1 了：{body}"
        );

        // 非超管只能看自己的记录
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/logs/login?pageSize=50",
                &staff_token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let list = body["data"].as_array().unwrap();
        assert!(!list.is_empty(), "{body}");
        assert!(
            list.iter().all(|item| item["admin_id"] == 990005),
            "普通管理员不应看到别人的日志：{body}"
        );
        assert_eq!(body["total"].as_i64().unwrap(), list.len() as i64, "{body}");

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn admin_list_filters_and_scopes_by_role() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };

        let admin_name = "rustcms_list_admin";
        let staff_name = "rustcms_list_staff";
        seed_temp_admin(&pool, 990004, admin_name, "list-pass-123").await;
        seed_temp_admin_in_group(&pool, 990005, staff_name, "list-pass-456", 2).await;
        let _ = sqlx::query("DELETE FROM admin_login_logs WHERE user_name LIKE 'rustcms_%'")
            .execute(&pool)
            .await;

        let Some(app) = test_app().await else {
            return;
        };

        // 未登录被拒
        let res = app
            .clone()
            .oneshot(json_request("GET", "/api/admin/list", None, Value::Null))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        let token = login_admin(&app, "10.6.6.1", admin_name, "list-pass-123").await;
        let staff_token = login_admin(&app, "10.6.6.2", staff_name, "list-pass-456").await;

        // 用户名模糊搜索
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/list?user_name=rustcms_list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].as_i64().unwrap() >= 2, "{body}");
        let list = body["data"].as_array().unwrap();
        assert!(
            list.iter()
                .all(|item| item["user_name"].as_str().unwrap().contains("rustcms_list")),
            "{body}"
        );
        // id 倒序：ID 大的排前面
        assert_eq!(list[0]["id"], 990005, "{body}");
        // 密码哈希不返回
        assert!(list[0].get("password").is_none(), "{body}");
        // is_super 按用户组计算
        assert_eq!(list[0]["is_super"], false, "{body}");
        assert_eq!(list[1]["is_super"], true, "{body}");

        // 按用户组过滤
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/list?group_id=2&user_name=rustcms_list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["total"], 1, "{body}");
        assert_eq!(body["data"][0]["id"], 990005, "{body}");

        // 按 ID 过滤
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/list?id=990004",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["total"], 1, "{body}");
        assert_eq!(body["data"][0]["user_name"], admin_name, "{body}");

        // 分页：每页 1 条，两页不同记录、total 不变
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/list?current=1&pageSize=1&user_name=rustcms_list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let first = body_json(res).await;
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/list?current=2&pageSize=1&user_name=rustcms_list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let second = body_json(res).await;
        assert_eq!(first["data"].as_array().unwrap().len(), 1, "{first}");
        assert_eq!(second["data"].as_array().unwrap().len(), 1, "{second}");
        assert_ne!(
            first["data"][0]["id"], second["data"][0]["id"],
            "{first}{second}"
        );
        assert_eq!(first["total"], second["total"]);

        // 非超管：无论怎么传过滤条件都只拿到自己
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/list?group_id=1&pageSize=50",
                &staff_token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["total"], 1, "{body}");
        assert_eq!(body["data"][0]["id"], 990005, "{body}");

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn admin_delete_refuses_self_root_and_non_super() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };

        let admin_name = "rustcms_del_admin";
        let victim_name = "rustcms_del_victim";
        let staff_name = "rustcms_del_staff";
        seed_temp_admin(&pool, 990004, admin_name, "del-pass-123").await;
        seed_temp_admin_in_group(&pool, 990005, victim_name, "del-pass-456", 2).await;
        seed_temp_admin_in_group(&pool, 990006, staff_name, "del-pass-789", 2).await;

        let Some(app) = test_app().await else {
            return;
        };

        // 未登录被拒
        let res = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/admin/delete",
                None,
                json!({ "id": 990005 }),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        let token = login_admin(&app, "10.5.5.1", admin_name, "del-pass-123").await;
        let staff_token = login_admin(&app, "10.5.5.2", staff_name, "del-pass-789").await;

        // 不能删自己
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/delete",
                &token,
                json!({ "id": 990004 }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], "该管理员不可删除", "{body}");

        // 不能删初始超管（1 号）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/delete",
                &token,
                json!({ "id": 1 }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], "该管理员不可删除", "{body}");

        // id=0 / 不存在
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/delete",
                &token,
                json!({ "id": 0 }),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["msg"], "管理员不存在");

        // 非超管不能删别人，且目标必须还在
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/delete",
                &staff_token,
                json!({ "id": 990005 }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], NO_ADMIN_PERMISSION, "{body}");
        assert!(admin::get(&pool, 990005).await.unwrap().is_some());

        // 超管删除成功
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/delete",
                &token,
                json!({ "id": 990005 }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["msg"], "删除成功", "{body}");
        assert!(admin::get(&pool, 990005).await.unwrap().is_none());

        // 重复删除
        let res = app
            .oneshot(bearer_request(
                "POST",
                "/api/admin/delete",
                &token,
                json!({ "id": 990005 }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], "管理员不存在", "{body}");

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn admin_group_crud_requires_super() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };

        let admin_name = "rustcms_grp_admin";
        let staff_name = "rustcms_grp_staff";
        seed_temp_admin(&pool, 990004, admin_name, "grp-pass-123").await;
        seed_temp_admin_in_group(&pool, 990006, staff_name, "grp-pass-456", 2).await;
        // 清掉上次中断留下的测试组
        let _ = sqlx::query("DELETE FROM admin_groups WHERE title LIKE 'rustcms\\_%'")
            .execute(&pool)
            .await;

        let Some(app) = test_app().await else {
            return;
        };

        // 未登录被拒
        let res = app
            .clone()
            .oneshot(json_request(
                "GET",
                "/api/admin/group/list",
                None,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        let token = login_admin(&app, "10.4.4.1", admin_name, "grp-pass-123").await;
        let staff_token = login_admin(&app, "10.4.4.2", staff_name, "grp-pass-456").await;

        // 列表：至少有内置的 1 号超管组
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/group/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let groups = body["data"].as_array().unwrap();
        assert!(!groups.is_empty(), "{body}");
        let ids: Vec<u64> = groups
            .iter()
            .map(|item| item["id"].as_u64().unwrap())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted, "用户组应按 ID 升序：{body}");

        // 详情：setting 是对象、permissions 是数组
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/group/detail?id=1",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["id"], 1, "{body}");
        assert!(body["data"]["setting"].is_object(), "{body}");
        assert!(body["data"]["setting"]["permissions"].is_array(), "{body}");

        // 不存在的组
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/group/detail?id=999999",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["msg"], "用户组不存在");

        // 新建
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/group/detail",
                &token,
                json!({
                    "title": "rustcms_测试组",
                    "description": "  只读组  ",
                    "setting": { "permissions": ["archive/list", "archive/detail"] },
                }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["msg"], "保存成功", "{body}");
        let group_id = body["data"]["id"].as_u64().unwrap();
        assert!(group_id > 0, "{body}");

        // 新建的就该能查到，描述被 trim，权限原样落库
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/api/admin/group/detail?id={group_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["title"], "rustcms_测试组", "{body}");
        assert_eq!(body["data"]["description"], "只读组", "{body}");
        assert_eq!(
            body["data"]["setting"]["permissions"],
            json!(["archive/list", "archive/detail"]),
            "{body}"
        );
        // 保存时状态恒为启用（与 Go 版一致）
        assert_eq!(body["data"]["status"], 1, "{body}");

        // 更新
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/group/detail",
                &token,
                json!({
                    "id": group_id,
                    "title": "rustcms_测试组2",
                    "description": "",
                    "setting": { "permissions": [] },
                }),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/api/admin/group/detail?id={group_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["title"], "rustcms_测试组2", "{body}");
        assert_eq!(body["data"]["setting"]["permissions"], json!([]), "{body}");

        // 校验：名称不能为空 / 更新的组必须存在
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/group/detail",
                &token,
                json!({ "title": "   " }),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["msg"], "分组名称不能为空");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/group/detail",
                &token,
                json!({ "id": 999999, "title": "rustcms_不存在" }),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["msg"], "用户组不存在");

        // 非超管一律拒绝
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/group/list",
                &staff_token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["msg"], NO_ADMIN_PERMISSION);
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/group/delete",
                &staff_token,
                json!({ "id": group_id }),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["msg"], NO_ADMIN_PERMISSION);

        // 1 号内置组不能删
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/group/delete",
                &token,
                json!({ "id": 1 }),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["msg"], "该用户组不可删除");

        // 组下还有管理员的不能删（990006 在 2 号组）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/group/delete",
                &token,
                json!({ "id": 2 }),
            ))
            .await
            .unwrap();
        assert_eq!(
            body_json(res).await["msg"],
            "该用户组下还有管理员，无法删除"
        );

        // 空组可以删；真实库里万一有管理员的 group_id 正好等于这个自增 ID，
        // 就应该被拦下（这也是正确行为），按实际情况选分支断言。
        let in_group = admin::count_in_group(&pool, group_id as u32).await.unwrap();
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/api/admin/group/delete",
                &token,
                json!({ "id": group_id }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        if in_group == 0 {
            assert_eq!(body["code"], 0, "{body}");
            assert_eq!(body["msg"], "删除成功", "{body}");
            let res = app
                .oneshot(bearer_request(
                    "GET",
                    &format!("/api/admin/group/detail?id={group_id}"),
                    &token,
                    Value::Null,
                ))
                .await
                .unwrap();
            assert_eq!(body_json(res).await["msg"], "用户组不存在");
        } else {
            assert_eq!(
                body["msg"], "该用户组下还有管理员，无法删除",
                "组下有 {in_group} 个管理员时不该允许删除：{body}"
            );
            assert!(admin::get_group(&pool, group_id as u32)
                .await
                .unwrap()
                .is_some());
        }

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn admin_action_logs_record_writes() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };

        let admin_name = "rustcms_alog_admin";
        let victim_name = "rustcms_alog_victim";
        let staff_name = "rustcms_alog_staff";
        seed_temp_admin(&pool, 990004, admin_name, "alog-pass-123").await;
        seed_temp_admin_in_group(&pool, 990005, victim_name, "alog-pass-000", 2).await;
        seed_temp_admin_in_group(&pool, 990006, staff_name, "alog-pass-456", 2).await;
        let _ = sqlx::query("DELETE FROM admin_logs WHERE user_name LIKE 'rustcms\\_%'")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM admin_groups WHERE title LIKE 'rustcms\\_%'")
            .execute(&pool)
            .await;

        let Some(app) = test_app().await else {
            return;
        };

        // 未登录被拒
        let res = app
            .clone()
            .oneshot(json_request(
                "GET",
                "/api/admin/logs/action",
                None,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        let token = login_admin(&app, "10.3.3.1", admin_name, "alog-pass-123").await;
        let staff_token = login_admin(&app, "10.3.3.2", staff_name, "alog-pass-456").await;

        // 三次写操作：改资料 → 建组 → 删组
        let res = app
            .clone()
            .oneshot(bearer_request_from(
                "10.3.3.1",
                "POST",
                "/api/admin/detail",
                &token,
                json!({ "id": 990004, "user_name": admin_name, "group_id": 1 }),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request_from(
                "10.3.3.1",
                "POST",
                "/api/admin/group/detail",
                &token,
                json!({ "title": "rustcms_日志组", "setting": { "permissions": [] } }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let group_id = body["data"]["id"].as_u64().unwrap();

        // 新建组的自增 ID 可能正好等于某个真实管理员的 group_id（库里存在这种孤儿值），
        // 那种情况删除会被正确拦下，两种结果都算正常。
        let in_group = admin::count_in_group(&pool, group_id as u32).await.unwrap();
        let res = app
            .clone()
            .oneshot(bearer_request_from(
                "10.3.3.1",
                "POST",
                "/api/admin/group/delete",
                &token,
                json!({ "id": group_id }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        if in_group == 0 {
            assert_eq!(body["code"], 0, "{body}");
        } else {
            assert_eq!(body["msg"], "该用户组下还有管理员，无法删除", "{body}");
        }

        // 第四次写操作：删掉一个临时管理员（保证一定成功，作为「最新一条」）
        let res = app
            .clone()
            .oneshot(bearer_request_from(
                "10.3.3.1",
                "POST",
                "/api/admin/delete",
                &token,
                json!({ "id": 990005 }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");

        // 日志列表：倒序，最新一条是「删除管理员」
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/logs/action?pageSize=50",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let list = body["data"].as_array().unwrap();
        assert!(body["total"].as_i64().unwrap() >= 4, "{body}");
        let logs: Vec<&str> = list
            .iter()
            .map(|item| item["log"].as_str().unwrap())
            .collect();
        assert!(
            logs.iter().any(|log| log.contains("更新管理员信息")),
            "缺少改资料日志：{body}"
        );
        assert!(
            logs.iter().any(|log| log.contains("更新管理员组信息")),
            "缺少建组日志：{body}"
        );
        assert!(
            logs.iter().any(|log| log.contains("删除管理员：990005")),
            "缺少删管理员日志：{body}"
        );
        if in_group == 0 {
            assert!(
                logs.iter().any(|log| log.contains("删除管理员组")),
                "缺少删组日志：{body}"
            );
        }
        // 最新一条排在最前，IP 与操作人取自当次请求
        assert_eq!(
            list[0]["log"],
            format!("删除管理员：990005 => {victim_name}")
        );
        assert_eq!(list[0]["ip"], "10.3.3.1", "{body}");
        assert_eq!(list[0]["user_name"], admin_name, "{body}");
        assert_eq!(list[0]["admin_id"], 990004, "{body}");

        // 非超管只看自己的
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/api/admin/logs/action?pageSize=50",
                &staff_token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let list = body["data"].as_array().unwrap();
        assert!(
            list.iter().all(|item| item["admin_id"] == 990006),
            "普通管理员不该看到别人的操作日志：{body}"
        );
        assert_eq!(body["total"].as_i64().unwrap(), list.len() as i64, "{body}");

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn admin_menus_require_login() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };

        let admin_name = "rustcms_menu_admin";
        let staff_name = "rustcms_menu_staff";
        seed_temp_admin(&pool, 990004, admin_name, "menu-pass-123").await;
        seed_temp_admin_in_group(&pool, 990006, staff_name, "menu-pass-456", 2).await;

        let Some(app) = test_app().await else {
            return;
        };

        // 未登录拿不到菜单
        let res = app
            .clone()
            .oneshot(json_request("GET", "/api/admin/menus", None, Value::Null))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        let token = login_admin(&app, "10.2.2.1", admin_name, "menu-pass-123").await;
        let res = app
            .clone()
            .oneshot(bearer_request("GET", "/api/admin/menus", &token, Value::Null))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let groups = body["data"].as_array().unwrap();
        assert!(groups.len() >= 8, "菜单分组太少：{body}");
        assert!(
            groups.iter().any(|group| group["key"] == "setting"),
            "缺少 setting 分组：{body}"
        );
        for group in groups {
            assert!(group["name"].as_str().is_some_and(|name| !name.is_empty()));
            let menus = group["menus"].as_array().unwrap();
            assert!(!menus.is_empty(), "分组没有菜单项：{group}");
            for menu in menus {
                // Go 数据里「多站点」分组的菜单项 group_key 就是 `tool`，不校验两者一致
                assert!(
                    menu["group_key"].as_str().is_some_and(|value| !value.is_empty()),
                    "{menu}"
                );
                assert!(menu["path"].as_str().is_some_and(|path| !path.is_empty()));
            }
        }

        // 普通管理员也能拿到菜单（前端按权限过滤，接口不分级）
        let staff_token = login_admin(&app, "10.2.2.2", staff_name, "menu-pass-456").await;
        let res = app
            .oneshot(bearer_request(
                "GET",
                "/api/admin/menus",
                &staff_token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"].as_array().unwrap().len(), groups.len());

        drop_temp_admin(&pool).await;
    }

    /// 用指定 IP 登录并取出令牌（失败即 panic，方便复用）。
    async fn login_admin(app: &Router, ip: &str, user_name: &str, password: &str) -> String {
        let res = app
            .clone()
            .oneshot(json_request_from(
                ip,
                "POST",
                "/api/admin/login",
                None,
                json!({"user_name": user_name, "password": password}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "登录失败：{body}");
        body["data"]["token"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn serves_front_pages_by_rewrite_rules() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let Some(app) = test_app().await else {
            return;
        };

        // 找一个有内容的栏目
        let categories = crate::models::category::list_all(&pool).await.unwrap();
        let Some(target) = categories
            .iter()
            .find(|item| item.kind == 1 && item.archive_count > 0)
        else {
            return;
        };

        // 栏目页：/{catname}/
        let res = app
            .clone()
            .oneshot(request("GET", &format!("/{}/", target.url_token)))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let html = body_text(res).await;
        // 分类有 SEO 标题时优先输出 SEO 标题
        let title_hit = html.contains(&target.title)
            || (!target.seo_title.is_empty() && html.contains(&target.seo_title));
        assert!(
            title_hit,
            "栏目页没渲染分类标题：{}",
            &html[..html.len().min(400)]
        );
        assert!(!html.contains("{%"), "栏目页残留模板标签");

        // 栏目分页：/{catname}/2
        let res = app
            .clone()
            .oneshot(request("GET", &format!("/{}/2", target.url_token)))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 详情页：/{catname}/{id}.html（内容必须属于该栏目，catname 才对得上）
        let cat_ids = crate::models::category::self_and_sub_ids(&categories, target.id);
        let list = crate::models::archive::list(
            &pool,
            &crate::models::archive::ArchiveQuery::new().categories(cat_ids),
            crate::models::archive::Order::IdDesc,
            1,
            1,
        )
        .await
        .unwrap();
        let Some(item) = list.list.first() else {
            return;
        };
        let res = app
            .clone()
            .oneshot(request(
                "GET",
                &format!("/{}/{}.html", target.url_token, item.id),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let html = body_text(res).await;
        assert!(html.contains(&item.title), "详情页没渲染标题");
        assert!(!html.contains("{{"), "详情页残留变量占位");

        // 单页：/about.html
        if let Some(page_cat) = categories.iter().find(|item| item.kind == 3) {
            let res = app
                .clone()
                .oneshot(request("GET", &format!("/{}.html", page_cat.url_token)))
                .await
                .unwrap();
            assert_eq!(
                res.status(),
                StatusCode::OK,
                "单页 {:?}",
                page_cat.url_token
            );
        }

        // 不存在的栏目
        let res = app
            .oneshot(request("GET", "/not-exists-page/"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn renders_home_and_module_index_with_pagination() {
        let _guard = crate::models::db_lock().await;
        let Some(app) = test_app().await else {
            return;
        };

        // 首页
        let res = app.clone().oneshot(request("GET", "/")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let html = body_text(res).await;
        assert!(html.contains("<!DOCTYPE html>"), "首页没走模板");
        assert!(!html.contains("{%"), "首页残留模板标签");
        assert!(!html.contains("{{"), "首页残留变量占位");
        assert!(html.contains("ind-pro"), "首页区块缺失");

        // 模块列表 /news/
        let res = app.clone().oneshot(request("GET", "/news/")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let html = body_text(res).await;
        assert!(html.contains("<!DOCTYPE html>"), "/news/ 没走模板");
        assert!(!html.contains("{%"));

        // 分页：栏目第 1 页应有分页条，并指向第 2 页
        let categories = match crate::models::test_pool().await {
            Some(pool) => crate::models::category::list_all(&pool).await.unwrap(),
            None => return,
        };
        let Some(target) = categories
            .iter()
            .find(|item| item.kind == 1 && item.archive_count > 10)
        else {
            return;
        };
        let res = app
            .oneshot(request("GET", &format!("/{}/", target.url_token)))
            .await
            .unwrap();
        let html = body_text(res).await;
        assert!(html.contains("pagnavi"), "分页条没渲染");
        assert!(
            html.contains(&format!("/{}/2", target.url_token)),
            "缺少第 2 页链接"
        );
    }

    #[tokio::test]
    async fn write_api_requires_token() {
        let Some(app) = test_app_with_token(None).await else {
            return;
        };

        // 没配 api_token：写入整体禁用
        let res = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/archives",
                None,
                json!({"title": "x", "content": "y", "category_id": 13}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1);
        assert!(body["msg"].as_str().unwrap().contains("写入接口未启用"));

        // 配了 token 但传错：拒绝
        let Some(app) = test_app_with_token(Some("secret-token")).await else {
            return;
        };
        let res = app
            .oneshot(json_request(
                "POST",
                "/api/archives",
                Some("wrong"),
                json!({"title": "x", "content": "y", "category_id": 13}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1);
        assert_eq!(body["msg"], "无效的 api_token");
    }

    #[tokio::test]
    async fn write_api_crud_roundtrip() {
        const TOKEN: &str = "test-token";
        let _guard = crate::models::db_lock().await;
        let Some(app) = test_app_with_token(Some(TOKEN)).await else {
            return;
        };
        if let Some(pool) = crate::models::test_pool().await {
            crate::models::archive_write::purge_test_archives(&pool, "Rust 接口写入测试").await;
        }

        // 找一个内容列表类型的分类
        let res = app
            .clone()
            .oneshot(request("GET", "/api/categories"))
            .await
            .unwrap();
        let body = body_json(res).await;
        let category = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == 1)
            .cloned()
            .expect("库里应有内容列表分类");
        let category_id = category["id"].as_u64().unwrap();

        // 新增
        let res = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/archives",
                Some(TOKEN),
                json!({
                    "title": "Rust 接口写入测试",
                    "content": "<p>接口写入正文</p>",
                    "category_id": category_id,
                    "images": ["/uploads/api-test.webp"],
                }),
            ))
            .await
            .unwrap();
        let created = body_json(res).await;
        assert_eq!(created["code"], 0, "{created}");
        let id = created["data"]["id"].as_i64().unwrap();
        assert!(id > 0);

        // 读回
        let res = app
            .clone()
            .oneshot(request("GET", &format!("/api/archives/{id}")))
            .await
            .unwrap();
        let detail = body_json(res).await;
        assert_eq!(detail["code"], 0);
        assert_eq!(detail["data"]["title"], "Rust 接口写入测试");
        assert_eq!(detail["data"]["content"], "<p>接口写入正文</p>");
        assert_eq!(detail["data"]["images"][0], "/uploads/api-test.webp");

        // 同标题重复：拒绝
        let res = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/archives",
                Some(TOKEN),
                json!({"title": "Rust 接口写入测试", "content": "x", "category_id": category_id}),
            ))
            .await
            .unwrap();
        let dup = body_json(res).await;
        assert_eq!(dup["code"], 1);
        assert!(dup["msg"].as_str().unwrap().contains("文档标题已重复"));

        // 编辑
        let res = app
            .clone()
            .oneshot(json_request(
                "PUT",
                &format!("/api/archives/{id}"),
                Some(TOKEN),
                json!({
                    "title": "Rust 接口写入测试（已改）",
                    "content": "<p>改过的正文</p>",
                    "category_id": category_id,
                }),
            ))
            .await
            .unwrap();
        let updated = body_json(res).await;
        assert_eq!(updated["code"], 0, "{updated}");

        let res = app
            .clone()
            .oneshot(request("GET", &format!("/api/archives/{id}")))
            .await
            .unwrap();
        let detail = body_json(res).await;
        assert_eq!(detail["data"]["title"], "Rust 接口写入测试（已改）");
        assert_eq!(detail["data"]["content"], "<p>改过的正文</p>");

        // 删除
        let res = app
            .clone()
            .oneshot(json_request(
                "DELETE",
                &format!("/api/archives/{id}"),
                Some(TOKEN),
                Value::Null,
            ))
            .await
            .unwrap();
        let deleted = body_json(res).await;
        assert_eq!(deleted["code"], 0, "{deleted}");
        assert_eq!(deleted["data"]["deleted"], true);

        let res = app
            .oneshot(request("GET", &format!("/api/archives/{id}")))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);
    }

    // ------------------------------------------------------------------
    // 后台 `/system/api/...`（Part 1：版本 / 统计 / 模型 / 分类）
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn manage_api_requires_admin_token() {
        let Some(app) = test_app().await else {
            return;
        };
        for uri in [
            "/system/api/version/info",
            "/system/api/statistic/summary",
            "/system/api/module/list",
            "/system/api/category/list",
        ] {
            let res = app.clone().oneshot(request("GET", uri)).await.unwrap();
            let body = body_json(res).await;
            assert_eq!(body["code"], 1, "{uri}：{body}");
            assert_eq!(body["msg"], "请先登录", "{uri}");
        }
        // 验证码不需要登录
        let res = app.oneshot(request("GET", "/system/api/captcha")).await.unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
    }

    #[tokio::test]
    async fn manage_part1_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_manage_admin";
        seed_temp_admin(&pool, 990006, name, "manage-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.1", name, "manage-pass-123").await;

        // 版本
        let res = app
            .clone()
            .oneshot(bearer_request("GET", "/system/api/version/info", &token, Value::Null))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["version"], statistic::VERSION);

        // 统计摘要
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/statistic/summary?exact=true",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["archive_count"]["total"].is_number(), "{body}");
        assert!(body["data"]["archive_counts"].is_array(), "{body}");

        // 仪表盘
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/statistic/dashboard",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["version"].is_string(), "{body}");

        // 模型列表
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/module/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let modules = body["data"].as_array().unwrap();
        assert!(!modules.is_empty(), "库里应有模型");
        let module_id = modules[0]["id"].as_u64().unwrap();

        // 分类列表（扁平）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/category/list?type=1&show_type=1",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let list = body["data"].as_array().unwrap();
        assert!(!list.is_empty(), "库里应有内容分类");
        assert!(list.iter().all(|item| item["type"] == 1), "{body}");

        // 模型详情
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/module/detail?id={module_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["id"], module_id);

        // 分类数量重算
        let res = app
            .oneshot(bearer_request(
                "POST",
                "/system/api/category/count",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_category_crud_roundtrip() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_cat_admin";
        seed_temp_admin(&pool, 990006, name, "cat-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.2", name, "cat-pass-123").await;

        let module_id: u32 = sqlx::query_scalar("SELECT id FROM modules ORDER BY id LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let stamp = chrono::Utc::now().timestamp_millis();
        let title = format!("Rust 测试分类 {stamp}");
        let token_name = format!("rustcms-test-cat-{stamp}");

        // 新建
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/category/detail",
                &token,
                json!({
                    "title": title,
                    "url_token": token_name,
                    "type": 1,
                    "module_id": module_id,
                    "status": 1,
                    "sort": 88,
                    "description": "测试分类",
                }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let id = body["data"]["id"].as_u64().unwrap() as u32;
        assert!(id > 0);
        assert_eq!(body["data"]["title"], title);

        // 详情
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/category/detail?id={id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["url_token"], token_name);

        // PATCH：只改标题，其它字段保持
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/category/detail",
                &token,
                json!({"id": id, "partial": true, "title": format!("{title}（改）")}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["title"], format!("{title}（改）"));
        assert_eq!(body["data"]["url_token"], token_name, "PATCH 不该改别名");

        // 别名重复（不 force）应被拒绝
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/category/detail",
                &token,
                json!({
                    "title": format!("{title} 冲突"),
                    "url_token": token_name,
                    "type": 1,
                    "module_id": module_id,
                }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "token duplication");

        // 删除
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/category/delete",
                &token,
                json!({"id": id}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");

        // 确认真的没了（不能只看响应）
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "分类应已删除");

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_nav_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_nav_admin";
        seed_temp_admin(&pool, 990006, name, "nav-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.3", name, "nav-pass-123").await;

        // 默认分组与导航树
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/setting/nav?type_id=1",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].is_array(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/setting/nav/type",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(!body["data"].as_array().unwrap().is_empty());

        // 新建分组
        let stamp = chrono::Utc::now().timestamp_millis();
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/nav/type",
                &token,
                json!({"title": format!("Rust 导航组 {stamp}")}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let type_id = body["data"]["id"].as_u64().unwrap() as u32;

        // 新建导航
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/nav",
                &token,
                json!({
                    "title": "Rust 导航项",
                    "type_id": type_id,
                    "nav_type": 2,
                    "link": "https://example.com",
                    "sort": 5,
                    "status": 1,
                }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let nav_id = body["data"]["id"].as_u64().unwrap() as u32;

        // 列表里能看到
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/setting/nav?type_id={type_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"][0]["id"], nav_id);

        // 删除导航后分组也删掉（连带清空组内导航）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/nav/delete",
                &token,
                json!({"id": nav_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/nav/type/delete",
                &token,
                json!({"id": type_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 默认分组不允许删除
        let res = app
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/nav/type/delete",
                &token,
                json!({"id": 1}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "不允许删除默认分类");

        drop_temp_admin(&pool).await;
    }

    /// 组一个 multipart 请求（一个文本字段 + 一个文件字段）。
    fn multipart_upload_request(
        uri: &str,
        token: &str,
        boundary: &str,
        file_name: &str,
        bytes: &[u8],
    ) -> Request<Body> {
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"category_id\"\r\n\r\n0\r\n",
        );
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n\
                 Content-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let peer: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .header("authorization", format!("Bearer {token}"))
            .extension(ConnectInfo(peer))
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn manage_attachment_upload_and_delete() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_attach_admin";
        seed_temp_admin(&pool, 990006, name, "attach-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.4", name, "attach-pass-123").await;

        // 1x1 PNG 的头部（解析宽高够用）
        let png: Vec<u8> = vec![
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
            0, 0, 0, 1, 0, 0, 0, 1,
        ];
        // 先清掉可能残留的同 MD5 记录，保证走「新建」分支
        let md5 = crate::models::attachment::md5_hex(&png);
        let _ = sqlx::query("DELETE FROM attachments WHERE file_md5 = ?")
            .bind(&md5)
            .execute(&pool)
            .await;

        // 上传
        let res = app
            .clone()
            .oneshot(multipart_upload_request(
                "/system/api/attachment/upload",
                &token,
                "rustcmsBoundary",
                "rustcms-upload-test.png",
                &png,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let id = body["data"]["id"].as_u64().unwrap() as u32;
        let location = body["data"]["file_location"].as_str().unwrap().to_string();
        assert!(location.starts_with("uploads/"), "{location}");
        assert_eq!(body["data"]["width"], 1, "{body}");
        assert_eq!(body["data"]["height"], 1, "{body}");
        assert_eq!(body["data"]["is_image"], 1, "{body}");

        // 文件真的落盘了
        let public_dir = std::env::temp_dir().join(format!("rustcms-public-{}", std::process::id()));
        assert!(public_dir.join(&location).is_file(), "文件未落盘：{location}");

        // 列表能搜到
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/attachment/list?q=rustcms-upload-test",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].as_i64().unwrap() >= 1, "{body}");
        assert!(body["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == id));

        // 改名
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/attachment/edit",
                &token,
                json!({"id": id, "file_name": "改名后的图片"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["file_name"], "改名后的图片");

        // 批量改分类
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/attachment/category",
                &token,
                json!({"category_id": 7, "ids": [id]}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let category_id: u32 = sqlx::query_scalar("SELECT category_id FROM attachments WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(category_id, 7);

        // 删除：库记录与磁盘文件都要消失
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/attachment/delete",
                &token,
                json!({"id": id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "附件记录应已删除");
        assert!(!public_dir.join(&location).exists(), "磁盘文件应已删除");

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_archive_batch_actions() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_archive_admin";
        seed_temp_admin(&pool, 990006, name, "archive-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.5", name, "archive-pass-123").await;

        let categories = category::list_all(&pool).await.unwrap();
        let Some(category) = categories.iter().find(|item| item.kind == 1) else {
            return;
        };
        let category_id = category.id;
        let other_category = categories
            .iter()
            .find(|item| item.kind == 1 && item.id != category_id)
            .map(|item| item.id);

        let stamp = chrono::Utc::now().timestamp_millis();
        let title = format!("Rust 后台内容测试 {stamp}");

        // 新建
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/archive/detail",
                &token,
                json!({
                    "title": title,
                    "content": "<p>后台写入正文</p>",
                    "category_id": category_id,
                }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let id = body["data"]["id"].as_i64().unwrap();

        // 列表（用标题里的数字戳做关键词，避免 URL 里出现中文/空格）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/archive/list?q={stamp}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].as_i64().unwrap() >= 1, "{body}");

        // 详情
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/archive/detail?id={id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["content"], "<p>后台写入正文</p>");

        // 排序
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/archive/sort",
                &token,
                json!({"id": id, "sort": 42}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let sort: u32 = sqlx::query_scalar("SELECT sort FROM archives WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sort, 42);

        // 改分类
        if let Some(target) = other_category {
            let res = app
                .clone()
                .oneshot(bearer_request(
                    "POST",
                    "/system/api/archive/category",
                    &token,
                    json!({"ids": [id], "category_id": target}),
                ))
                .await
                .unwrap();
            assert_eq!(body_json(res).await["code"], 0);
            let cid: u32 = sqlx::query_scalar("SELECT category_id FROM archives WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(cid, target);
        }

        // 下架：移到草稿表
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/archive/status",
                &token,
                json!({"ids": [id], "status": 0}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "下架失败：{body}");
        let in_archives: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM archives WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let draft_status: Option<u8> =
            sqlx::query_scalar("SELECT status FROM archive_drafts WHERE id = ?")
                .bind(id)
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert_eq!(in_archives, 0, "下架后不应还在正式表");
        assert_eq!(draft_status, Some(0), "应在草稿表且状态为 0");

        // 恢复
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/archive/recover",
                &token,
                json!({"id": id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let back: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM archives WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(back, 1, "恢复后应回到正式表");

        // 删除 → 进回收站（status = 99）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/archive/delete",
                &token,
                json!({"id": id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let recycle_status: Option<u8> =
            sqlx::query_scalar("SELECT status FROM archive_drafts WHERE id = ?")
                .bind(id)
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert_eq!(recycle_status, Some(99), "应进回收站");

        // 再删一次 → 永久删除
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/archive/delete",
                &token,
                json!({"id": id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        for table in ["archive_drafts", "archive_data"] {
            let sql = format!("SELECT COUNT(*) FROM `{table}` WHERE id = ?");
            let count: i64 = sqlx::query_scalar(&sql)
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(count, 0, "{table} 应已清空");
        }

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_diy_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_diy_admin";
        seed_temp_admin(&pool, 990006, name, "diy-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.6", name, "diy-pass-123").await;

        // 备份真实数据，用例结束还原
        let backup: Option<(String, Option<String>, i8)> =
            sqlx::query_as("SELECT title, modules, status FROM diy_pages WHERE `key` = 'home'")
                .fetch_optional(&pool)
                .await
                .unwrap();

        // 区块类型 + 页面
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/diy/types",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["types"].as_array().unwrap().len(), 8);
        assert_eq!(body["data"]["pages"][0]["value"], "home");

        // 保存
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/diy/save",
                &token,
                json!({
                    "key": "home",
                    "title": "首页",
                    "modules": [
                        {"type": "notice", "settings": {"text": "hi"}},
                        {"type": "divider", "settings": {}}
                    ],
                }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["count"], 2);

        // 读回
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/diy/page?key=home",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["modules"].as_array().unwrap().len(), 2);
        assert_eq!(body["data"]["title"], "首页");

        // 非法 key
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/diy/save",
                &token,
                json!({"key": "admin", "modules": []}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 重置
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/diy/reset",
                &token,
                json!({"key": "home"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/diy/page?key=home",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(
            body_json(res).await["data"]["modules"]
                .as_array()
                .unwrap()
                .len(),
            0
        );

        // 还原
        match backup {
            Some((title, modules, status)) => {
                sqlx::query(
                    "UPDATE diy_pages SET title = ?, modules = ?, status = ? WHERE `key` = 'home'",
                )
                .bind(title)
                .bind(modules)
                .bind(status)
                .execute(&pool)
                .await
                .unwrap();
            }
            None => {
                let _ = sqlx::query("DELETE FROM diy_pages WHERE `key` = 'home'")
                    .execute(&pool)
                    .await;
            }
        }

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_skill_crud() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_skill_admin";
        seed_temp_admin(&pool, 990006, name, "skill-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.7", name, "skill-pass-123").await;
        let skill_name = "rustcms-demo-skill";

        // 新建
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/skill/edit",
                &token,
                json!({
                    "name": skill_name,
                    "description": "测试技能",
                    "category": "测试",
                    "version": "1.0",
                    "author": "tester",
                    "tags": ["a", "b"],
                    "content": "# 正文\nhello",
                }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");

        // 列表
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/anqi/skill/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["name"] == skill_name));

        // 详情
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/anqi/skill/detail?name={skill_name}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["author"], "tester");
        assert_eq!(body["data"]["tags"][0], "a");

        // 非法名字
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/anqi/skill/detail?name=..%2Fetc",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], -1);

        // 重载
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/skill/reload",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["count"].as_u64().unwrap() >= 1);

        // 删除
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/skill/delete",
                &token,
                json!({"name": skill_name}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .oneshot(bearer_request(
                "GET",
                "/system/api/anqi/skill/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert!(!body["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["name"] == skill_name));

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_statistic_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_stat_admin";
        seed_temp_admin(&pool, 990006, name, "stat-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.8", name, "stat-pass-123").await;

        for uri in [
            "/system/api/statistic/spider",
            "/system/api/statistic/traffic",
            "/system/api/statistic/dates",
            "/system/api/statistic/include",
        ] {
            let res = app
                .clone()
                .oneshot(bearer_request("GET", uri, &token, Value::Null))
                .await
                .unwrap();
            let body = body_json(res).await;
            assert_eq!(body["code"], 0, "{uri}：{body}");
            assert!(body["data"].is_array(), "{uri}：{body}");
        }

        // 明细（可能没有日志文件，只校验信封）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/statistic/detail?type=traffic",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].is_number(), "{body}");
        assert!(body["data"].is_array(), "{body}");

        // 收录明细
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/statistic/include/detail",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].is_number(), "{body}");

        // 未登录一律拒绝
        let res = app
            .oneshot(request("GET", "/system/api/statistic/spider"))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_setting_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_setting_admin";
        seed_temp_admin(&pool, 990006, name, "setting-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.9", name, "setting-pass-123").await;

        // 备份所有会被改动的配置键
        let keys = ["system", "index", "contact", "safe", "content", "diy_fields", "banner", "cache_type"];
        let mut backup = Vec::new();
        for key in keys {
            let raw = crate::models::setting::get_raw(&pool, key).await.unwrap();
            backup.push((key, raw));
        }

        // 系统设置：读 → 字段级写 → 读回
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/setting/system",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["system"].is_object(), "{body}");
        assert!(body["data"]["languages"].is_array(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/system",
                &token,
                json!({"site_name": "Rust 测试站点"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/setting/system",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["system"]["site_name"], "Rust 测试站点", "{body}");
        // 老字段仍在（merge 而不是整块覆盖）
        assert!(body["data"]["system"]["base_url"].is_string(), "{body}");

        // 后台域名不合法被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/system",
                &token,
                json!({"admin_url": "admin.example.com"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "请填写正确的后台域名");

        // 首页 SEO
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/index",
                &token,
                json!({"seo_title": "Rust 首页标题"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/setting/index",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["data"]["seo_title"], "Rust 首页标题");

        // 缓存信息 + 清理
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/setting/cache",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].get("cache_type").is_some(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/cache",
                &token,
                json!({"update": false}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 自定义字段（数组）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/diyfield",
                &token,
                json!([{"name": "作者", "field_name": "author", "type": "text"}]),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/setting/diyfield",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"][0]["field_name"], "author", "{body}");

        // 非数组被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/diyfield",
                &token,
                json!({"name": "x"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // Banner：新增 → 列表 → 删除
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/banner",
                &token,
                json!({"logo": "/uploads/test-banner.webp", "type": "rustcms-test"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let banner_id = body["data"]["id"].as_i64().unwrap();

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/setting/banner",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert!(body["data"].as_array().unwrap().iter().any(|group| {
            group["type"] == json!("rustcms-test")
        }), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/banner/delete",
                &token,
                json!({"id": banner_id, "type": "rustcms-test"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // AI 设置
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/setting/ai",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["ai_generate"].is_object(), "{body}");
        assert!(body["data"]["ai_chat"].is_object(), "{body}");

        // 未实现的能力明确报错
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/migratedb",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/thumb/rebuild",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 还原
        for (key, raw) in backup {
            match raw {
                Some(raw) => crate::models::setting::save_raw(&pool, key, &raw)
                    .await
                    .unwrap(),
                None => crate::models::setting::remove(&pool, key).await.unwrap(),
            }
        }

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_user_auth_flow() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_userauth_admin";
        seed_temp_admin(&pool, 990006, name, "userauth-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.10", name, "userauth-pass-123").await;

        // 造一条待审核记录
        let _ = sqlx::query("DELETE FROM user_auths WHERE real_name = 'rustcms-route-auth'")
            .execute(&pool)
            .await;
        let inserted = sqlx::query(
            "INSERT INTO user_auths (created_time, updated_time, user_id, `type`, real_name, \
                id_card, id_card_front, id_card_back, id_card_hand, company_name, company_no, \
                company_legal, company_licence, phone, status, remark, audit_time, audit_admin_id) \
             VALUES (UNIX_TIMESTAMP(), UNIX_TIMESTAMP(), 999999, 1, 'rustcms-route-auth', \
                '', '', '', '', '', '', '', '', '', 0, '', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let id = inserted.last_insert_id() as u32;

        // 列表
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/userauth/list?status=all&pageSize=5",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["list"].is_array(), "{body}");
        assert!(body["data"]["total"].is_number(), "{body}");
        assert!(body["data"]["pending"].is_number(), "{body}");

        // 详情
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/plugin/userauth/detail?id={id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["real_name"], "rustcms-route-auth");

        // 驳回必须填原因
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/userauth/audit",
                &token,
                json!({"id": id, "status": -1, "remark": ""}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "请填写驳回原因");

        // 通过
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/userauth/audit",
                &token,
                json!({"id": id, "status": 1, "remark": "资料齐全"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["status"], 1);

        let stored: (i8, String) =
            sqlx::query_as("SELECT status, remark FROM user_auths WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored.0, 1);
        assert_eq!(stored.1, "资料齐全");

        let _ = sqlx::query("DELETE FROM user_auths WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await;

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_design_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_design_admin";
        seed_temp_admin(&pool, 990006, name, "design-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.11", name, "design-pass-123").await;

        // 造一个临时模板包（test_app 的根目录是 <temp>/rustcms-cfg-<pid>）
        let root = std::env::temp_dir().join(format!("rustcms-cfg-{}", std::process::id()));
        let package = "rustcms-design-demo";
        let tpl_dir = root.join("template").join(package);
        let static_dir = root.join("public").join("static").join(package);
        let _ = std::fs::remove_dir_all(&tpl_dir);
        let _ = std::fs::remove_dir_all(&static_dir);
        std::fs::create_dir_all(&tpl_dir).unwrap();
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(
            tpl_dir.join("config.json"),
            format!(r#"{{"name":"临时模板","package":"{package}","version":"1.0","template_type":0}}"#),
        )
        .unwrap();
        std::fs::write(tpl_dir.join("index.html"), "<h1>v1</h1>").unwrap();
        std::fs::write(static_dir.join("a.css"), "body{}").unwrap();

        // 列表
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/design/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["package"] == json!(package)), "{body}");

        // 详情（扫盘）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/design/info?package={package}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["tpl_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == json!("index.html")), "{body}");
        assert!(!body["data"]["tpl_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == json!("config.json")), "{body}");

        // 文件详情
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/design/file/info?package={package}&path=index.html"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["content"], "<h1>v1</h1>");

        // 保存正文 → 产生历史版本
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/design/file/save",
                &token,
                json!({
                    "package": package,
                    "path": "index.html",
                    "type": "template",
                    "content": "<h1>v2</h1>",
                    "update_content": true,
                }),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/design/file/histories?package={package}&path=index.html"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let histories = body["data"].as_array().unwrap();
        assert_eq!(histories.len(), 1, "{body}");
        let hash = histories[0]["hash"].as_str().unwrap().to_string();

        // 恢复历史版本
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/design/file/restore",
                &token,
                json!({
                    "package": package,
                    "path": "index.html",
                    "type": "template",
                    "hash": hash,
                }),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/design/file/info?package={package}&path=index.html"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["data"]["content"], "<h1>v1</h1>");

        // 复制 / 删除
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/design/file/copy",
                &token,
                json!({
                    "package": package,
                    "path": "index.html",
                    "new_path": "index-copy.html",
                    "type": "template",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/design/file/delete",
                &token,
                json!({
                    "package": package,
                    "path": "index-copy.html",
                    "type": "template",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        assert!(!tpl_dir.join("index-copy.html").exists());

        // 路径穿越被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!(
                    "/system/api/design/file/info?package={package}&path=..%2F..%2Fetc%2Fpasswd"
                ),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 保存模板信息
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/design/save",
                &token,
                json!({"package": package, "name": "临时模板改名", "template_type": 1}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["name"], "临时模板改名");

        // 上传重名检查（已存在 → code 1）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/design/upload/check?package={package}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 打包下载：附件形式返回 zip
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/design/download",
                &token,
                json!({"package": package}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/zip")
        );
        assert!(
            res.headers()
                .get(header::CONTENT_DISPOSITION)
                .and_then(|value| value.to_str().ok())
                .map(|value| value.contains("attachment;"))
                .unwrap_or(false),
            "应带附件头"
        );

        // 上传非法压缩包：解压失败
        let boundary = "designUploadBoundary";
        let mut mp_body = Vec::new();
        mp_body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        mp_body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"file\"; filename=\"bad.zip\"\r\n\
              Content-Type: application/octet-stream\r\n\r\n",
        );
        mp_body.extend_from_slice(b"not a zip");
        mp_body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let req = Request::builder()
            .method("POST")
            .uri("/system/api/design/upload")
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(mp_body))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 演示数据恢复：没有 data.db 时明确报错
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/design/data/restore",
                &token,
                json!({"package": package}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "没有可用的演示数据", "{body}");

        // 删除模板（移到 cache/.history）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/design/delete",
                &token,
                json!({"package": package}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        assert!(!tpl_dir.exists(), "模板目录应已移走");

        // 收尾
        let _ = std::fs::remove_dir_all(root.join("cache").join(".history").join(package));
        let _ = std::fs::remove_dir_all(&static_dir);
        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_website_and_version_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_website_admin";
        seed_temp_admin(&pool, 990006, name, "website-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.12", name, "website-pass-123").await;

        // 升级环境信息
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/version/env",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["version"], statistic::VERSION);
        assert!(body["data"]["max_size"].as_u64().unwrap() > 0, "{body}");
        assert!(!body["data"]["skip_rules"].as_str().unwrap().is_empty());

        // 在线版本检查：外呼失败也会按「已是最新」返回成功
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/version/check",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0, "version/check 应返回成功");

        // 在线升级：缺少版本号时明确报错
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/version/upgrade",
                &token,
                json!({}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "缺少目标版本号", "{body}");

        // 上传升级包：暂未实现
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/version/upload-upgrade",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 当前站点信息
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/siteinfo",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["id"], 1);
        assert!(!body["data"]["name"].as_str().unwrap().is_empty(), "{body}");

        // 站点列表
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/website/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].as_i64().unwrap() >= 1, "{body}");

        // 站点详情：密码必须是空串
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/website/info?id=1",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["id"], 1);
        assert_eq!(body["data"]["mysql"]["password"], "", "{body}");
        assert!(body["data"]["initialed"].is_boolean(), "{body}");

        // 新建站点
        let _ = sqlx::query("DELETE FROM websites WHERE name LIKE 'rustcms%'")
            .execute(&pool)
            .await;
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/website/save",
                &token,
                json!({"name": "rustcms_test_site", "root_path": "/tmp/rustcms-test-site"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let site_id = body["data"]["id"].as_u64().unwrap() as u32;
        assert!(site_id > 1);
        assert!(!body["data"]["token_secret"].as_str().unwrap().is_empty());

        // 按名称搜索能查到
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/website/list?name=rustcms_test_site",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["total"], 1, "{body}");

        // 删除
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/website/delete",
                &token,
                json!({"id": site_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        assert!(website::get(&pool, site_id).await.unwrap().is_none());

        // 默认站点不能删
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/website/delete",
                &token,
                json!({"id": 1}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "默认站点不能删除");

        // 切换登录：站点已删除时明确报错
        let res = app
            .oneshot(bearer_request(
                "POST",
                "/system/api/website/login",
                &token,
                json!({"site_id": site_id}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "站点不存在", "{body}");

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_seo_plugin_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_seo_admin";
        seed_temp_admin(&pool, 990006, name, "seo-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.13", name, "seo-pass-123").await;

        // 备份
        let rewrite_backup = crate::models::setting::get_raw(&pool, "rewrite").await.unwrap();
        let sitemap_backup = crate::models::setting::get_raw(&pool, "sitemap").await.unwrap();
        // 与 test_app_with_token 里设置的 public_dir 保持一致
        let public_dir = std::env::temp_dir().join(format!("rustcms-public-{}", std::process::id()));
        let robots_path = public_dir.join("robots.txt");
        let robots_backup = std::fs::read_to_string(&robots_path).ok();

        // 伪静态
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/rewrite",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["mode"].is_number(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/rewrite",
                &token,
                json!({"mode": 4}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/rewrite",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["data"]["mode"], 4);

        // robots：默认内容 → 保存 → 读回
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/robots",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(!body["data"]["robots"].as_str().unwrap().is_empty(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/robots",
                &token,
                json!({"robots": "User-agent: *\nDisallow: /rustcms-private\n"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        assert!(std::fs::read_to_string(&robots_path)
            .unwrap()
            .contains("Disallow: /rustcms-private"));

        // sitemap：非法 type 归一
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/sitemap",
                &token,
                json!({"type": "rss", "auto_build": 1}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/sitemap",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["type"], "txt", "{body}");
        assert!(body["data"]["sitemap_url"]
            .as_str()
            .unwrap()
            .ends_with("/sitemap.txt"), "{body}");

        // 生成
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/sitemap/build",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["count"].as_u64().unwrap() >= 1, "{body}");
        assert!(public_dir.join("sitemap.txt").is_file(), "sitemap.txt 未生成");

        // xml 模式
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/sitemap",
                &token,
                json!({"type": "xml"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/sitemap/build",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let xml = std::fs::read_to_string(public_dir.join("sitemap.xml")).unwrap();
        assert!(xml.contains("<urlset"), "{xml}");
        // 切换格式后旧的 txt 被清理
        assert!(!public_dir.join("sitemap.txt").exists());

        // 还原
        match rewrite_backup {
            Some(raw) => crate::models::setting::save_raw(&pool, "rewrite", &raw).await.unwrap(),
            None => crate::models::setting::remove(&pool, "rewrite").await.unwrap(),
        }
        match sitemap_backup {
            Some(raw) => crate::models::setting::save_raw(&pool, "sitemap", &raw).await.unwrap(),
            None => crate::models::setting::remove(&pool, "sitemap").await.unwrap(),
        }
        match robots_backup {
            Some(raw) => {
                let _ = std::fs::write(&robots_path, raw);
            }
            None => {
                let _ = std::fs::remove_file(&robots_path);
            }
        }
        let _ = std::fs::remove_file(public_dir.join("sitemap.xml"));
        let _ = std::fs::remove_file(public_dir.join("sitemap.txt"));

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_link_and_keyword_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_link_admin";
        seed_temp_admin(&pool, 990006, name, "link-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.14", name, "link-pass-123").await;

        let _ = sqlx::query("DELETE FROM links WHERE title LIKE 'rustcms%'")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM keywords WHERE title LIKE 'rustcms%'")
            .execute(&pool)
            .await;
        let keyword_backup = crate::models::setting::get_raw(&pool, "keyword").await.unwrap();

        // ---- 友情链接 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/link/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].is_array(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/link/detail",
                &token,
                json!({"title": "rustcms-route-link", "link": "https://example.com"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let link_id = body["data"]["id"].as_u64().unwrap() as u32;
        // 保存后状态归零（待回链检查）
        assert_eq!(body["data"]["status"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/link/detail",
                &token,
                json!({"title": "rustcms-route-link", "link": ""}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "请填写链接地址");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/link/check",
                &token,
                json!({"id": link_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/link/delete",
                &token,
                json!({"id": link_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        assert!(link::get(&pool, link_id).await.unwrap().is_none());

        // ---- 关键词配置 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/keyword/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].get("auto_dig").is_some(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/keyword/setting",
                &token,
                json!({"auto_dig": true, "max_count": 500}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/keyword/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["auto_dig"], true, "{body}");
        assert_eq!(body["data"]["max_count"], 500, "{body}");

        // ---- 关键词批量新增 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/keyword/detail",
                &token,
                json!({"title": "rustcms-kw-a\nrustcms-kw-b\n\nrustcms-kw-a", "category_id": 3}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let saved = body["data"].as_array().unwrap();
        assert_eq!(saved.len(), 2, "重复行应被去掉：{body}");
        let first_id = saved[0]["id"].as_u64().unwrap() as u32;
        assert_eq!(saved[0]["category_id"], 3);

        // 再提交一次：全部已存在，不再新增
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/keyword/detail",
                &token,
                json!({"title": "rustcms-kw-a"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["data"].as_array().unwrap().len(), 0);

        // 列表 + 搜索
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/keyword/list?title=rustcms-kw-",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["total"], 2, "{body}");

        // 更新：改成已存在的标题应被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/keyword/detail",
                &token,
                json!({"id": first_id, "title": "rustcms-kw-b"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");

        // 导出
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/keyword/export",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["header"][0], "title");
        assert!(body["data"]["content"].as_array().unwrap().len() >= 2);

        // 批量删除
        let ids: Vec<u32> = saved
            .iter()
            .map(|item| item["id"].as_u64().unwrap() as u32)
            .collect();
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/keyword/delete",
                &token,
                json!({"ids": ids}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM keywords WHERE title LIKE 'rustcms%'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0);

        // 还原配置
        match keyword_backup {
            Some(raw) => crate::models::setting::save_raw(&pool, "keyword", &raw)
                .await
                .unwrap(),
            None => crate::models::setting::remove(&pool, "keyword").await.unwrap(),
        }

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_tag_and_redirect_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_tag_admin";
        seed_temp_admin(&pool, 990006, name, "tag-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.15", name, "tag-pass-123").await;

        let _ = sqlx::query("DELETE FROM tags WHERE title LIKE 'rustcms-tag-route%'")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM redirects WHERE from_url LIKE '/rustcms-redir%'")
            .execute(&pool)
            .await;
        let fields_backup = crate::models::setting::get_raw(&pool, "tag_fields").await.unwrap();

        // ---- 标签 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/tag/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].is_number(), "{body}");
        assert!(body["data"].is_array(), "{body}");

        // 字段定义：非数组被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/tag/fields",
                &token,
                json!({"x": 1}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/tag/fields",
                &token,
                json!([{"name": "作者", "field_name": "author", "type": "text"}]),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/tag/fields",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"][0]["field_name"], "author");

        // 新建标签（带正文与自定义字段）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/tag/detail",
                &token,
                json!({
                    "title": "rustcms-tag-route",
                    "content": "标签正文",
                    "extra": {"author": "tester"},
                    "status": 1,
                }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let tag_id = body["data"]["id"].as_u64().unwrap() as u32;
        assert_eq!(body["data"]["url_token"], "rustcms-tag-route");

        // 详情：正文 / 自定义字段 / 链接
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/plugin/tag/detail?id={tag_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["content"], "标签正文");
        assert_eq!(body["data"]["extra"]["author"], "tester");
        assert_eq!(body["data"]["link"], "/tag/rustcms-tag-route");

        // 搜索命中
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/tag/list?title=rustcms-tag-route",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert!(body["total"].as_i64().unwrap() >= 1, "{body}");

        // 删除
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/tag/delete",
                &token,
                json!({"id": tag_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        assert!(tag::get(&pool, tag_id).await.unwrap().is_none());

        // ---- 301 跳转 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/redirect/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].is_number(), "{body}");

        // 来源=目标 被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/redirect/detail",
                &token,
                json!({"from_url": "/rustcms-redir-same", "to_url": "/rustcms-redir-same"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "来源链接和跳转链接不能相同");

        // 新建（自动补前导斜杠）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/redirect/detail",
                &token,
                json!({"from_url": "rustcms-redir-old", "to_url": "rustcms-redir-new"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let redirect_id = body["data"]["id"].as_u64().unwrap() as u32;
        assert_eq!(body["data"]["from_url"], "/rustcms-redir-old");

        // 搜索
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/redirect/list?from_url=rustcms-redir-old",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["total"], 1, "{body}");

        // 删除
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/redirect/delete",
                &token,
                json!({"id": redirect_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        assert!(redirect::get(&pool, redirect_id).await.unwrap().is_none());

        // 还原
        match fields_backup {
            Some(raw) => crate::models::setting::save_raw(&pool, "tag_fields", &raw)
                .await
                .unwrap(),
            None => crate::models::setting::remove(&pool, "tag_fields").await.unwrap(),
        }

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_anchor_and_replace_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_anchor_admin";
        seed_temp_admin(&pool, 990006, name, "anchor-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.16", name, "anchor-pass-123").await;

        let _ = sqlx::query("DELETE FROM anchors WHERE title LIKE 'rustcms-anchor-route%'")
            .execute(&pool)
            .await;
        let setting_backup = crate::models::setting::get_raw(&pool, "anchor").await.unwrap();

        // ---- 锚文本 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/anchor/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].is_number(), "{body}");

        // 配置：密度 < 10 归一为 100
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/anchor/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["anchor_density"].as_i64().unwrap() >= 10, "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/anchor/setting",
                &token,
                json!({"anchor_density": 3, "replace_way": 1}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/anchor/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["anchor_density"], 100, "{body}");
        assert_eq!(body["data"]["replace_way"], 1, "{body}");

        // 新建
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/anchor/detail",
                &token,
                json!({"title": "rustcms-anchor-route", "link": "/anchor-target", "weight": 2}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let anchor_id = body["data"]["id"].as_u64().unwrap() as u32;
        assert_eq!(body["data"]["link"], "/anchor-target");

        // 详情
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/plugin/anchor/detail?id={anchor_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["title"], "rustcms-anchor-route");

        // 导出
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/anchor/export",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["header"][0], "title");

        // 替换（单条 + 全部）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/anchor/replace",
                &token,
                json!({"id": anchor_id}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["articles"].is_number(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/anchor/replace",
                &token,
                json!({"id": 0}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 从标题导入（类型非法被拒）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/anchor/add-from-title",
                &token,
                json!({"type": "unknown", "ids": [1]}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 删除
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/anchor/delete",
                &token,
                json!({"id": anchor_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        assert!(anchor::get(&pool, anchor_id).await.unwrap().is_none());

        // ---- 全站替换 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/replace/values",
                &token,
                json!({"places": [], "keywords": [{"from": "a", "to": "b"}]}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "请选择要替换的位置");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/replace/values",
                &token,
                json!({"places": ["keyword"], "keywords": []}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "替换关键词不能为空");

        // 正常执行（不存在的词，受影响 0 行也算成功）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/replace/values",
                &token,
                json!({
                    "places": ["keyword"],
                    "keywords": [{"from": "rustcms-not-exist-xyz", "to": "x"}],
                    "replace_tag": false,
                }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].is_number(), "{body}");

        // 还原
        match setting_backup {
            Some(raw) => crate::models::setting::save_raw(&pool, "anchor", &raw)
                .await
                .unwrap(),
            None => crate::models::setting::remove(&pool, "anchor").await.unwrap(),
        }

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_config_plugin_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_cfg_admin";
        seed_temp_admin(&pool, 990006, name, "cfg-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.17", name, "cfg-pass-123").await;

        let keys = ["time_factor", "json_ld", "google_auth"];
        let mut backups = Vec::new();
        for key in keys {
            backups.push((key, crate::models::setting::get_raw(&pool, key).await.unwrap()));
        }

        // 三组配置：读 → 非对象被拒 → 保存 → 读回
        let cases = [
            (
                "/system/api/plugin/timefactor/setting",
                json!({"open": true, "daily_update": 9}),
                "open",
                json!(true),
            ),
            (
                "/system/api/plugin/jsonld/config",
                json!({"open": true, "organization_name": "Rust 组织"}),
                "organization_name",
                json!("Rust 组织"),
            ),
            (
                "/system/api/plugin/google/setting",
                json!({"client_id": "rustcms-client"}),
                "client_id",
                json!("rustcms-client"),
            ),
        ];

        for (uri, payload, field, expected) in cases {
            let res = app
                .clone()
                .oneshot(bearer_request("GET", uri, &token, Value::Null))
                .await
                .unwrap();
            let body = body_json(res).await;
            assert_eq!(body["code"], 0, "{uri} GET：{body}");
            assert!(body["data"].is_object(), "{uri} GET：{body}");

            let res = app
                .clone()
                .oneshot(bearer_request("POST", uri, &token, json!([1, 2])))
                .await
                .unwrap();
            let body = body_json(res).await;
            assert_eq!(body["code"], 1, "{uri} 非对象应被拒：{body}");

            let res = app
                .clone()
                .oneshot(bearer_request("POST", uri, &token, payload))
                .await
                .unwrap();
            assert_eq!(body_json(res).await["code"], 0, "{uri} POST");

            let res = app
                .clone()
                .oneshot(bearer_request("GET", uri, &token, Value::Null))
                .await
                .unwrap();
            let body = body_json(res).await;
            assert_eq!(body["data"][field], expected, "{uri} 读回：{body}");
        }

        // 时间因子的运行期计数不被覆盖
        crate::models::setting::save_json(
            &pool,
            "time_factor",
            &json!({"today_count": 11, "open": false}),
        )
        .await
        .unwrap();
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/timefactor/setting",
                &token,
                json!({"daily_limit": 3}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/timefactor/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["today_count"], 11, "{body}");
        assert_eq!(body["data"]["daily_limit"], 3, "{body}");

        // 还原
        for (key, raw) in backups {
            match raw {
                Some(raw) => crate::models::setting::save_raw(&pool, key, &raw)
                    .await
                    .unwrap(),
                None => crate::models::setting::remove(&pool, key).await.unwrap(),
            }
        }

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_transfer_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_transfer_admin";
        seed_temp_admin(&pool, 990006, name, "transfer-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.18", name, "transfer-pass-123").await;

        // 任务：恒为空
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/transfer/task",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].is_null(), "{body}");

        // 三个需要 HTTP 的动作明确报错
        for uri in [
            "/system/api/plugin/transfer/create",
            "/system/api/plugin/transfer/modules",
            "/system/api/plugin/transfer/start",
        ] {
            let res = app
                .clone()
                .oneshot(bearer_request("POST", uri, &token, json!({})))
                .await
                .unwrap();
            let body = body_json(res).await;
            assert_eq!(body["code"], 1, "{uri}：{body}");
        }

        // 下载：参数校验
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/transfer/download",
                &token,
                json!({}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "provider is required");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/transfer/download",
                &token,
                json!({"provider": "../etc"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "invalid provider name");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/transfer/download",
                &token,
                json!({"provider": "notexist"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "客户端文件不存在");

        // 造一个临时客户端文件，验证真能下载到
        let root = std::env::temp_dir().join(format!("rustcms-cfg-{}", std::process::id()));
        let client_dir = root.join("clientFiles");
        std::fs::create_dir_all(&client_dir).unwrap();
        let payload = b"<?php // rustcms test client file";
        std::fs::write(client_dir.join("dedecms2rustcms.php"), payload).unwrap();

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/transfer/download",
                &token,
                json!({"provider": "dedecms"}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let disposition = res
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            disposition.contains("dedecms2rustcms.php"),
            "{disposition}"
        );
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), payload);

        // 收尾
        let _ = std::fs::remove_file(client_dir.join("dedecms2rustcms.php"));
        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_multilang_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_ml_admin";
        seed_temp_admin(&pool, 990006, name, "ml-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.19", name, "ml-pass-123").await;

        let ml_backup = crate::models::setting::get_raw(&pool, "multi_lang").await.unwrap();
        let system_backup = crate::models::setting::get_raw(&pool, "system").await.unwrap();

        // 配置：读 → 保存（site_type 空值归一）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/multilang/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["sub_sites"].is_array(), "{body}");
        assert!(body["data"]["default_language"].is_string(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/multilang/config",
                &token,
                json!({"open": true, "site_type": ""}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/multilang/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["open"], true, "{body}");
        assert_eq!(body["data"]["site_type"], "multi", "{body}");

        // 非对象被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/multilang/config",
                &token,
                json!([]),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 子站点：先清空
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/multilang/config",
                &token,
                json!({"sub_sites": []}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/multilang/sites",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"].as_array().unwrap().len(), 0, "{body}");

        // 保存子站点
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/multilang/site/save",
                &token,
                json!({"id": 2, "language": "en", "language_icon": "/uploads/en.png"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["language"], "en");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/multilang/sites",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"].as_array().unwrap().len(), 1, "{body}");

        // 缺 id → 被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/multilang/site/save",
                &token,
                json!({"language": "en"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 可用站点
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/multilang/validsites",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].is_array(), "{body}");
        assert!(!body["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|site| site["id"] == 2), "已占用站点不该出现");

        // site_type=single 时 sites?type=multi 不返回数据
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/multilang/config",
                &token,
                json!({"site_type": "single"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/multilang/sites?type=multi",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body.get("data").is_none(), "{body}");

        // 同步：未实现 / 无任务
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/multilang/site/sync",
                &token,
                json!({"id": 2}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/multilang/site/sync/status",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].is_null(), "{body}");

        // 翻译缓存：造两个文件后列 / 删
        let cache_root = std::env::temp_dir()
            .join(format!("rustcms-cfg-{}", std::process::id()))
            .join("cache")
            .join("multiLang");
        std::fs::create_dir_all(cache_root.join("en")).unwrap();
        std::fs::write(cache_root.join("en").join("index.html"), "en").unwrap();

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/multilang/site/html/caches?lang=en",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].as_i64().unwrap() >= 1, "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/multilang/site/html/logs",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].is_number(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/multilang/site/html/cache/remove",
                &token,
                json!({"all": true}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["removed"].as_u64().unwrap() >= 1, "{body}");
        assert!(!cache_root.exists());

        // 移除子站点
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/multilang/site/remove",
                &token,
                json!({"id": 2}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 还原
        for (key, raw) in [("multi_lang", ml_backup), ("system", system_backup)] {
            match raw {
                Some(raw) => crate::models::setting::save_raw(&pool, key, &raw)
                    .await
                    .unwrap(),
                None => crate::models::setting::remove(&pool, key).await.unwrap(),
            }
        }

        drop_temp_admin(&pool).await;
    }

    /// 组一个 multipart 请求：一个文本字段 + 一个文件字段。
    fn multipart_named_request(
        uri: &str,
        token: &str,
        boundary: &str,
        field_name: &str,
        field_value: &str,
        file_name: &str,
        bytes: &[u8],
    ) -> Request<Body> {
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{field_name}\"\r\n\r\n{field_value}\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n\
                 Content-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let peer: SocketAddr = "127.0.0.1:50001".parse().unwrap();
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .header("authorization", format!("Bearer {token}"))
            .extension(ConnectInfo(peer))
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn manage_titleimage_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_ti_admin";
        seed_temp_admin(&pool, 990006, name, "ti-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.20", name, "ti-pass-123").await;

        let backup = crate::models::setting::get_raw(&pool, "title_image")
            .await
            .unwrap();

        // 配置：默认值
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/titleimage/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["width"], 800, "{body}");
        assert!(body["data"]["bg_images"].is_array(), "{body}");

        // 归零尺寸回落
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/titleimage/config",
                &token,
                json!({"open": true, "width": 0, "height": 0}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/titleimage/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["width"], 800, "{body}");
        assert_eq!(body["data"]["height"], 600, "{body}");
        assert_eq!(body["data"]["open"], true, "{body}");

        // 预览：返回可直接查看的 SVG data URI
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/titleimage/preview?text=hi",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(
            body["data"]
                .as_str()
                .unwrap_or("")
                .starts_with("data:image/svg+xml;base64,"),
            "{body}"
        );

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/titleimage/generate",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 上传：字体只收 ttf
        let res = app
            .clone()
            .oneshot(multipart_named_request(
                "/system/api/plugin/titleimage/upload",
                &token,
                "tiBoundary1",
                "name",
                "font_path",
                "font.otf",
                b"not-a-font",
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "仅支持 ttf 格式字体");

        let res = app
            .clone()
            .oneshot(multipart_named_request(
                "/system/api/plugin/titleimage/upload",
                &token,
                "tiBoundary2",
                "name",
                "font_path",
                "font.ttf",
                b"ttf-bytes",
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"], "uploads/titleimage/title_font.ttf", "{body}");

        let public_dir =
            std::env::temp_dir().join(format!("rustcms-public-{}", std::process::id()));
        assert!(public_dir
            .join("uploads/titleimage/title_font.ttf")
            .is_file());

        // 字体路径写回了配置
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/titleimage/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(
            body["data"]["font_path"], "uploads/titleimage/title_font.ttf",
            "{body}"
        );

        // 背景图：按 md5 命名
        let res = app
            .clone()
            .oneshot(multipart_named_request(
                "/system/api/plugin/titleimage/upload",
                &token,
                "tiBoundary3",
                "name",
                "bg_image",
                "bg.PNG",
                b"image-bytes",
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let bg = body["data"].as_str().unwrap().to_string();
        assert!(bg.starts_with("uploads/titleimage/"), "{bg}");
        assert!(bg.ends_with(".png"), "{bg}");
        assert!(public_dir.join(&bg).is_file());

        // 非法 name
        let res = app
            .clone()
            .oneshot(multipart_named_request(
                "/system/api/plugin/titleimage/upload",
                &token,
                "tiBoundary4",
                "name",
                "not_allowed",
                "a.png",
                b"x",
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "文件名无效");

        // 收尾
        let _ = std::fs::remove_file(public_dir.join("uploads/titleimage/title_font.ttf"));
        let _ = std::fs::remove_file(public_dir.join(&bg));
        match backup {
            Some(raw) => crate::models::setting::save_raw(&pool, "title_image", &raw)
                .await
                .unwrap(),
            None => crate::models::setting::remove(&pool, "title_image")
                .await
                .unwrap(),
        }

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_sensitive_and_comment_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_cmt_admin";
        seed_temp_admin(&pool, 990006, name, "cmt-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.21", name, "cmt-pass-123").await;

        let backup = crate::models::setting::get_raw(&pool, "sensitive_words")
            .await
            .unwrap();

        // ---- 敏感词 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/setting/sensitive/words",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].is_array(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/sensitive/words",
                &token,
                json!(["第一", "第二|很好", r"{\d{6,12}}"]),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/setting/sensitive/words",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"].as_array().unwrap().len(), 3, "{body}");

        // 检测：正文命中「第一」+ 正则，标题命中「第二」
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/sensitive/check",
                &token,
                json!({"content": "第一 和 123456789", "title": "第二"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let hits: Vec<&str> = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item.as_str())
            .collect();
        assert_eq!(hits.len(), 3, "{body}");
        assert!(hits.contains(&"第一"), "{hits:?}");
        assert!(hits.contains(&"第二"), "{hits:?}");

        // 云端同步未实现
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/setting/sensitive/sync",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // ---- 评论 ----
        let _ = sqlx::query("DELETE FROM comments WHERE user_name = 'rustcms-route-cmt'")
            .execute(&pool)
            .await;
        let archive_id: i64 = sqlx::query_scalar("SELECT id FROM archives ORDER BY id LIMIT 1")
            .fetch_optional(&pool)
            .await
            .unwrap()
            .unwrap_or(0);
        let now = chrono::Utc::now().timestamp();
        let cmt_id = sqlx::query(
            "INSERT INTO comments (created_time, updated_time, archive_id, user_id, user_name, \
                email, ip, vote_count, content, parent_id, to_uid, status, rating) \
             VALUES (?, ?, ?, 0, 'rustcms-route-cmt', '', '127.0.0.1', 0, '原始内容', 0, 0, 0, 0)",
        )
        .bind(now)
        .bind(now)
        .bind(archive_id)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_id() as u32;

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/comment/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].as_i64().unwrap() >= 1, "{body}");
        let row = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == json!(cmt_id))
            .cloned()
            .expect("列表里应有刚插入的评论");
        assert!(row.get("item_title").is_some(), "{row}");

        // 详情
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/plugin/comment/detail?id={cmt_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["content"], "原始内容");

        // 改内容：转义 + 置为正常
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/comment/detail",
                &token,
                json!({"id": cmt_id, "user_name": "rustcms-route-cmt", "content": "<b>x</b>&nbsp;y", "ip": "1.2.3.4"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let stored: (i8, String, String) =
            sqlx::query_as("SELECT status, content, ip FROM comments WHERE id = ?")
                .bind(cmt_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored.0, 1);
        assert_eq!(stored.1, "&lt;b&gt;x&lt;/b&gt; y");
        assert_eq!(stored.2, "1.2.3.4");

        // 单条切换 → 待审核
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/comment/check",
                &token,
                json!({"id": cmt_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let status: i8 = sqlx::query_scalar("SELECT status FROM comments WHERE id = ?")
            .bind(cmt_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, 0);

        // 批量置为正常
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/comment/check",
                &token,
                json!({"ids": [cmt_id], "status": 1}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 既没 id 也没 ids
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/comment/check",
                &token,
                json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 删除
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/comment/delete",
                &token,
                json!({"id": cmt_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        assert!(comment::get(&pool, cmt_id).await.unwrap().is_none());

        // 还原
        match backup {
            Some(raw) => crate::models::setting::save_raw(&pool, "sensitive_words", &raw)
                .await
                .unwrap(),
            None => crate::models::setting::remove(&pool, "sensitive_words")
                .await
                .unwrap(),
        }

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_guestbook_and_subscriber_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_gb_admin";
        seed_temp_admin(&pool, 990006, name, "gb-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.22", name, "gb-pass-123").await;

        let gb_backup = crate::models::setting::get_raw(&pool, "guestbook").await.unwrap();

        // ---- 留言 ----
        let _ = sqlx::query("DELETE FROM guestbooks WHERE user_name = 'rustcms-gb-route'")
            .execute(&pool)
            .await;
        let now = chrono::Utc::now().timestamp();
        let gb_id = sqlx::query(
            "INSERT INTO guestbooks (created_time, updated_time, user_name, contact, content, ip, \
                status, refer, site_id, extra_data, user_id) \
             VALUES (?, ?, 'rustcms-gb-route', 'hi@example.com', '留言内容', '127.0.0.1', 0, \
                '/contact', 0, '{}', 0)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_id() as u32;

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/guestbook/list?keyword=rustcms-gb-route&status=default",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["total"], 1, "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/plugin/guestbook/detail?id={gb_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["content"], "留言内容");

        // 单条置垃圾
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/guestbook/status",
                &token,
                json!({"id": gb_id, "status": 2}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let status: i8 = sqlx::query_scalar("SELECT status FROM guestbooks WHERE id = ?")
            .bind(gb_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, 2);

        // 非法状态
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/guestbook/status",
                &token,
                json!({"id": gb_id, "status": 9}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "状态不正确");

        // 批量置正常
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/guestbook/status",
                &token,
                json!({"ids": [gb_id], "status": 1}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 导出
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/guestbook/export",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let header = body["data"]["header"].as_array().unwrap();
        assert_eq!(header[header.len() - 1], "Spam", "{body}");
        assert!(body["data"]["content"].as_array().unwrap().len() >= 1);

        // 设置：字段去重
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/guestbook/setting",
                &token,
                json!({
                    "return_message": "已收到",
                    "fields": [
                        {"name": "姓名", "field_name": "user_name", "is_system": true},
                        {"name": "微信", "field_name": "wechat"},
                        {"name": "重复", "field_name": "wechat"}
                    ]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/guestbook/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["fields"].as_array().unwrap().len(), 2, "{body}");
        assert_eq!(body["data"]["return_message"], "已收到", "{body}");

        // 删除
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/guestbook/delete",
                &token,
                json!({"id": gb_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        assert!(guestbook::get(&pool, gb_id).await.unwrap().is_none());

        // ---- 订阅 ----
        let _ = sqlx::query("DELETE FROM subscriber_categories WHERE title LIKE 'rustcms-route-sub%'")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM subscribers WHERE email LIKE 'rustcms-route-sub%'")
            .execute(&pool)
            .await;

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/subscriber/category/save",
                &token,
                json!({"title": "rustcms-route-sub-cat"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let cat_id = body["data"]["id"].as_i64().unwrap();

        // 同名冲突
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/subscriber/category/save",
                &token,
                json!({"title": "rustcms-route-sub-cat"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "分类名称已存在");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/subscriber/category/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].as_array().unwrap().iter().any(|item| {
            item["id"] == json!(cat_id)
        }));

        // 缺邮箱被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/subscriber/save",
                &token,
                json!({}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "请填写邮箱地址");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/subscriber/save",
                &token,
                json!({"email": "rustcms-route-sub@example.com"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let sub_id = body["data"]["id"].as_i64().unwrap();
        assert!(sub_id > 0);

        // 列表 + 详情
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/subscriber/list?email=rustcms-route-sub",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["total"], 1, "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/plugin/subscriber/detail?id={sub_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["email"], "rustcms-route-sub@example.com");

        // 群发未实现；status 的错误信息要能指出下一步
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/subscriber/send",
                &token,
                json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/subscriber/send/status",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert!(body["msg"].as_str().unwrap().contains("job_id"), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/subscriber/send/status?job_id=abc",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 删除订阅者与分类
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/subscriber/delete",
                &token,
                json!({"id": sub_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        assert!(subscriber::get(&pool, sub_id as i32).await.unwrap().is_none());

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/subscriber/category/delete",
                &token,
                json!({"id": cat_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 还原
        match gb_backup {
            Some(raw) => crate::models::setting::save_raw(&pool, "guestbook", &raw)
                .await
                .unwrap(),
            None => crate::models::setting::remove(&pool, "guestbook")
                .await
                .unwrap(),
        }

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_user_and_fileupload_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_userm_admin";
        seed_temp_admin(&pool, 990006, name, "userm-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.23", name, "userm-pass-123").await;

        let user_backup = crate::models::setting::get_raw(&pool, "user").await.unwrap();
        let upload_backup = crate::models::setting::get_raw(&pool, "upload_file")
            .await
            .unwrap();

        // ---- 验证文件 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/fileupload/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].is_array(), "{body}");

        // 非法扩展名
        let res = app
            .clone()
            .oneshot(multipart_named_request(
                "/system/api/plugin/fileupload/upload",
                &token,
                "fuBoundary1",
                "name",
                "file",
                "shell.php",
                b"<?php ?>",
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");

        // 合法上传
        let res = app
            .clone()
            .oneshot(multipart_named_request(
                "/system/api/plugin/fileupload/upload",
                &token,
                "fuBoundary2",
                "name",
                "file",
                "rustcms-verify.xml",
                b"<xml/>",
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let hash = body["data"]["hash"].as_str().unwrap().to_string();

        let public_dir =
            std::env::temp_dir().join(format!("rustcms-public-{}", std::process::id()));
        assert!(public_dir.join("rustcms-verify.xml").is_file());

        // 列表带 link
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/fileupload/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert!(body["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["link"].as_str().unwrap_or_default().ends_with("/rustcms-verify.xml")));

        // 删除
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/fileupload/delete",
                &token,
                json!({"hash": hash}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        assert!(!public_dir.join("rustcms-verify.xml").exists());

        // 再删 → 文件不正确
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/fileupload/delete",
                &token,
                json!({"hash": hash}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");

        // ---- 会员 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/user/fields",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["fields"].is_array(), "{body}");

        // 非法字段名
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/user/fields",
                &token,
                json!({"fields": [{"name": "作者", "field_name": "Author"}]}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");

        // 合法 + 去重
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/user/fields",
                &token,
                json!({"fields": [
                    {"name": "作者", "field_name": "author"},
                    {"name": "作者2", "field_name": "author"},
                    {"name": "微信", "field_name": "wx_openid"}
                ]}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/user/fields",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["fields"].as_array().unwrap().len(), 2, "{body}");

        // 删不存在的字段
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/user/field/delete",
                &token,
                json!({"field_name": "not_exists"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "字段不存在");

        // 建会员
        let _ = sqlx::query("DELETE FROM users WHERE user_name = 'rustcms-user-route'")
            .execute(&pool)
            .await;
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/user/detail",
                &token,
                json!({
                    "user_name": "rustcms-user-route",
                    "real_name": "路由测试",
                    "email": "rustcms-user-route@example.com",
                    "password": "pass-123456",
                    "status": 1,
                }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let user_id = body["data"]["id"].as_u64().unwrap() as u32;

        // 列表
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/user/list?q=rustcms-user-route",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["total"], 1, "{body}");

        // PATCH 只改手机号
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/user/detail",
                &token,
                json!({"id": user_id, "phone": "13900000000", "partial": true}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["user_name"], "rustcms-user-route", "{body}");
        assert_eq!(body["data"]["phone"], "13900000000", "{body}");

        // 余额
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/user/balance",
                &token,
                json!({"user_id": user_id, "amount": 0, "remark": "x"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "请填写金额");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/user/balance",
                &token,
                json!({"user_id": user_id, "amount": 250, "remark": "充值"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["balance"], 250, "{body}");

        // 用户组
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/user/group/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");

        let _ = sqlx::query("DELETE FROM user_groups WHERE title = 'rustcms-route-ug'")
            .execute(&pool)
            .await;
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/user/group/detail",
                &token,
                json!({"title": "rustcms-route-ug", "level": 3, "status": 1}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let group_id = body["data"]["id"].as_u64().unwrap() as u32;

        // 重名
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/user/group/detail",
                &token,
                json!({"title": "rustcms-route-ug"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/user/group/delete",
                &token,
                json!({"id": group_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 删会员
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/user/delete",
                &token,
                json!({"id": user_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        assert!(user_admin::get(&pool, user_id).await.unwrap().is_none());

        // 还原
        for (key, raw) in [("user", user_backup), ("upload_file", upload_backup)] {
            match raw {
                Some(raw) => crate::models::setting::save_raw(&pool, key, &raw)
                    .await
                    .unwrap(),
                None => crate::models::setting::remove(&pool, key).await.unwrap(),
            }
        }

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_material_and_config_plugins_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_mat_admin";
        seed_temp_admin(&pool, 990006, name, "mat-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.24", name, "mat-pass-123").await;

        let mut backups = Vec::new();
        for key in ["watermark", "interference", "limiter"] {
            backups.push((key, crate::models::setting::get_raw(&pool, key).await.unwrap()));
        }

        // ---- 素材 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/material/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].is_array(), "{body}");

        let _ = sqlx::query("DELETE FROM materials WHERE title LIKE 'rustcms-mat%'")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM material_categories WHERE title = 'rustcms-matcat-route'")
            .execute(&pool)
            .await;

        // 分类
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/material/category/detail",
                &token,
                json!({"title": "rustcms-matcat-route"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let cat_id = body["data"]["id"].as_u64().unwrap() as u32;

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/material/category/detail",
                &token,
                json!({"title": "rustcms-matcat-route"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");

        // 素材：标题必填
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/material/detail",
                &token,
                json!({"title": ""}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "请填写素材名称");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/material/detail",
                &token,
                json!({"title": "rustcms-mat-route", "category_id": cat_id, "content": "<p>正文</p>"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let mat_id = body["data"]["id"].as_u64().unwrap() as u32;

        // 详情 + 搜索
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/plugin/material/detail?id={mat_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["content"], "<p>正文</p>");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/material/list?keyword=rustcms-mat-route",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["total"], 1);

        // 分类下还有素材 → 不能删
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/material/category/delete",
                &token,
                json!({"id": cat_id}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert!(body["msg"].as_str().unwrap().contains("不能删除"), "{body}");

        // 批量导入（空标题被跳过）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/material/import",
                &token,
                json!({"materials": [{"title": "rustcms-mat-import"}, {"title": ""}]}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["count"], 1, "{body}");

        // 转码：UTF-8 + 去标签
        let res = app
            .clone()
            .oneshot(multipart_named_request(
                "/system/api/plugin/material/convert/file",
                &token,
                "cvBoundary1",
                "remove_tag",
                "true",
                "a.html",
                b"<p>hi</p>",
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"], "hi", "{body}");

        // 非 UTF-8 明确报错
        let res = app
            .clone()
            .oneshot(multipart_named_request(
                "/system/api/plugin/material/convert/file",
                &token,
                "cvBoundary2",
                "remove_tag",
                "false",
                "gbk.html",
                &[0xD6, 0xD0, 0xCE, 0xC4],
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");

        // 删除素材后分类可删
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/material/delete",
                &token,
                json!({"id": mat_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/material/category/delete",
                &token,
                json!({"id": cat_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 收尾导入的素材
        let _ = sqlx::query("DELETE FROM materials WHERE title LIKE 'rustcms-mat%'")
            .execute(&pool)
            .await;

        // ---- 水印 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/watermark/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["size"], 20, "{body}");
        assert_eq!(body["data"]["position"], 9, "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/watermark/config",
                &token,
                json!({"open": true, "size": 0, "opacity": 0}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/watermark/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["size"], 20, "{body}");
        assert_eq!(body["data"]["opacity"], 100, "{body}");
        assert_eq!(body["data"]["open"], true, "{body}");

        // 未配置水印图片时预览应给出明确错误
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/watermark/preview",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 上传水印图
        let res = app
            .clone()
            .oneshot(multipart_named_request(
                "/system/api/plugin/watermark/upload",
                &token,
                "wmBoundary1",
                "name",
                "image_path",
                "logo.png",
                b"png-bytes",
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"], "uploads/watermark/watermark_image.png");

        // 字体只收 ttf
        let res = app
            .clone()
            .oneshot(multipart_named_request(
                "/system/api/plugin/watermark/upload",
                &token,
                "wmBoundary2",
                "name",
                "font_path",
                "f.otf",
                b"x",
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");

        // ---- 干扰码 / 限流 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/interference/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["mode"], 0, "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/interference/config",
                &token,
                json!({"open": true, "mode": 1, "disable_copy": true}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/limiter/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["white_ips"].is_array(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/limiter/setting",
                &token,
                json!({"open": true, "max_requests": 30}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/limiter/blockedips",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].as_array().unwrap().is_empty(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/limiter/blockedip/remove",
                &token,
                json!({"ip": "1.2.3.4"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 还原
        for (key, raw) in backups {
            match raw {
                Some(raw) => crate::models::setting::save_raw(&pool, key, &raw)
                    .await
                    .unwrap(),
                None => crate::models::setting::remove(&pool, key).await.unwrap(),
            }
        }

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_remaining_plugin_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_rest_admin";
        seed_temp_admin(&pool, 990006, name, "rest-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.25", name, "rest-pass-123").await;

        let mut backups = Vec::new();
        for key in ["akismet", "fulltext", "import_api", "html_cache", "ai_generate"] {
            backups.push((key, crate::models::setting::get_raw(&pool, key).await.unwrap()));
        }

        // ---- Akismet ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/akismet/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["check_type"].is_array(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/akismet/setting",
                &token,
                json!({"open": true, "api_key": "rustcms-key"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/akismet/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["open"], true, "{body}");
        assert_eq!(body["data"]["api_key"], "rustcms-key", "{body}");

        // ---- 全文索引 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/fulltext/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["engine"], "default", "{body}");
        assert!(body["data"]["modules"].is_array(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/fulltext/config",
                &token,
                json!({"open": true, "engine": "meilisearch", "ranking_score": 30}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/fulltext/status",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["running"], false, "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/fulltext/rebuild",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // ---- 导入 API token ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/import/token",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let new_token = body["data"]["token"].as_str().unwrap().to_string();
        assert_eq!(new_token.len(), 32, "{body}");
        assert_eq!(
            body["data"]["link_token"].as_str().unwrap().len(),
            32,
            "{body}"
        );

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/import/api",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["token"], new_token, "{body}");

        // ---- 静态缓存 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/htmlcache/config",
                &token,
                json!({"open": true, "storage_url": "https://cdn.example.com", "keep_local": true}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/htmlcache/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(
            body["data"]["storage_url"], "https://cdn.example.com/",
            "storage_url 应补尾部斜杠：{body}"
        );
        assert_eq!(body["data"]["keep_local"], false, "{body}");
        assert_eq!(body["data"]["open"], true, "{body}");

        // 生成/推送未实现
        for uri in [
            "/system/api/plugin/htmlcache/build",
            "/system/api/plugin/htmlcache/build/index",
            "/system/api/plugin/htmlcache/push",
        ] {
            let res = app
                .clone()
                .oneshot(bearer_request("POST", uri, &token, Value::Null))
                .await
                .unwrap();
            assert_eq!(body_json(res).await["code"], 1, "{uri}");
        }

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/htmlcache/build/status",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].is_null(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/htmlcache/push/logs",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert!(body_json(res).await["data"].as_array().unwrap().is_empty());

        // 上传 SSH 私钥
        let res = app
            .clone()
            .oneshot(multipart_named_request(
                "/system/api/plugin/htmlcache/upload/file",
                &token,
                "hcBoundary1",
                "name",
                "file",
                "id_rsa",
                b"-----BEGIN KEY-----",
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let key_path = std::env::temp_dir()
            .join(format!("rustcms-cfg-{}", std::process::id()))
            .join("data")
            .join("cert")
            .join("htmlcache_ssh_private_key.key");
        assert!(key_path.is_file(), "SSH 私钥应已写入 {key_path:?}");

        // 清理静态缓存
        let cache_html = std::env::temp_dir()
            .join(format!("rustcms-cfg-{}", std::process::id()))
            .join("cache")
            .join("html");
        std::fs::create_dir_all(cache_html.join("news")).unwrap();
        std::fs::write(cache_html.join("news").join("index.html"), "x").unwrap();
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/htmlcache/clean",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["removed"], 1, "{body}");
        assert!(!cache_html.exists());

        // ---- AI 生成 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/aigenerate/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].is_object(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/aigenerate/setting",
                &token,
                json!({"open": true, "count": 3}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/aigenerate/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["count"], 3, "{body}");

        for uri in [
            "/system/api/aigenerate/checkapi",
            "/system/api/aigenerate/plans",
        ] {
            let res = app
                .clone()
                .oneshot(bearer_request("GET", uri, &token, Value::Null))
                .await
                .unwrap();
            let body = body_json(res).await;
            // plans 返回空数组；checkapi 会外呼探测，通与不通都应给出明确提示
            if uri.ends_with("plans") {
                assert_eq!(body["code"], 0, "{body}");
                assert!(body["data"].as_array().unwrap().is_empty(), "{body}");
            } else {
                assert!(
                    body["code"] == 0 || body["code"] == 1,
                    "checkapi 返回异常: {body}"
                );
                assert!(
                    body["msg"].as_str().unwrap_or_default().contains("OpenAI"),
                    "{body}"
                );
            }
        }

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/aigenerate/article/start",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 还原
        for (key, raw) in backups {
            match raw {
                Some(raw) => crate::models::setting::save_raw(&pool, key, &raw)
                    .await
                    .unwrap(),
                None => crate::models::setting::remove(&pool, key).await.unwrap(),
            }
        }

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_place_and_push_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_place_admin";
        seed_temp_admin(&pool, 990006, name, "place-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.26", name, "place-pass-123").await;

        let mut backups = Vec::new();
        for key in ["place", "push"] {
            backups.push((key, crate::models::setting::get_raw(&pool, key).await.unwrap()));
        }
        let _ = sqlx::query("DELETE FROM places WHERE title LIKE 'rustcms-place-route%'")
            .execute(&pool)
            .await;

        // ---- 城市站配置 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/place/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["url_type"], "directory", "{body}");
        assert!(body["data"]["fields"].is_array(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/place/setting",
                &token,
                json!({"open": true, "url_type": "weird"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/place/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["open"], true, "{body}");
        assert_eq!(body["data"]["url_type"], "directory", "{body}");

        // ---- 城市站 CRUD ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/place/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].is_array(), "{body}");

        // 名称必填
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/place/detail",
                &token,
                json!({"title": ""}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "请填写城市站名称");

        // 新建父级
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/place/detail",
                &token,
                json!({"title": "rustcms-place-route-root", "status": 1}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let root_id = body["data"]["id"].as_u64().unwrap() as u32;
        assert_eq!(body["data"]["url_token"], "rustcms-place-route-root", "{body}");

        // 详情补 link 与默认模板
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/plugin/place/detail?id={root_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["link"], "/place/rustcms-place-route-root", "{body}");
        assert!(body["data"]["template"].as_str().unwrap().contains("place/detail.html"), "{body}");

        // 新建子级
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/place/detail",
                &token,
                json!({"title": "rustcms-place-route-child", "parent_id": root_id, "sort": 1}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let child_id = body["data"]["id"].as_u64().unwrap() as u32;

        // 上级不能是自己的下级
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/place/detail",
                &token,
                json!({"id": root_id, "title": "rustcms-place-route-root", "parent_id": child_id}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");

        // 搜索：平铺
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/place/list?title=rustcms-place-route",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"].as_array().unwrap().len(), 2, "{body}");

        // 树形：根节点带 children
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/place/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        let root = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == json!(root_id))
            .cloned()
            .expect("树里应有父级");
        assert_eq!(
            root["children"].as_array().unwrap().len(),
            1,
            "父级应有一个子级：{root}"
        );

        // 删除父级连带子级
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/place/delete",
                &token,
                json!({"id": root_id}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["count"].as_u64().unwrap() >= 2, "{body}");
        assert!(place::get(&pool, child_id).await.unwrap().is_none());

        // ---- 搜索引擎推送 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/push",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["baidu_api"], "", "{body}");
        assert!(body["data"]["js_codes"].is_array(), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/push",
                &token,
                json!({"baidu_api": "https://zz.baidu.com/urls?site=example.com", "logs": [{"fake": true}]}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/push",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert!(body["data"]["baidu_api"]
            .as_str()
            .unwrap()
            .contains("zz.baidu.com"), "{body}");
        // 前端传的 logs 不应被写进配置
        assert!(
            body["data"]["logs"].as_array().unwrap().is_empty(),
            "logs 不该被前端覆盖：{body}"
        );

        // 推送：缺 urls 被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/push/push",
                &token,
                json!({}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "请提供要推送的链接");

        // 推送：明确未实现，但留一条记录
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/push/push",
                &token,
                json!({"urls": ["/a.html"]}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/push/logs",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let logs = body["data"].as_array().unwrap();
        assert_eq!(logs.len(), 1, "{body}");
        assert_eq!(logs[0]["status"], 0, "{body}");
        assert_eq!(logs[0]["count"], 1, "{body}");

        // 还原
        for (key, raw) in backups {
            match raw {
                Some(raw) => crate::models::setting::save_raw(&pool, key, &raw)
                    .await
                    .unwrap(),
                None => crate::models::setting::remove(&pool, key).await.unwrap(),
            }
        }
        let _ = sqlx::query("DELETE FROM places WHERE title LIKE 'rustcms-place-route%'")
            .execute(&pool)
            .await;

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_storage_and_wechat_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_wx_admin";
        seed_temp_admin(&pool, 990006, name, "wx-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.27", name, "wx-pass-123").await;

        let mut backups = Vec::new();
        for key in ["storage", "wechat", "weapp"] {
            backups.push((key, crate::models::setting::get_raw(&pool, key).await.unwrap()));
        }
        let _ = sqlx::query("DELETE FROM wechat_messages WHERE openid = 'rustcms-route-openid'")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM wechat_reply_rules WHERE keyword LIKE 'rustcms-rt%'")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM wechat_menus WHERE name LIKE 'rustcms-rt%'")
            .execute(&pool)
            .await;

        // ---- 存储配置 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/storage",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["storage_type"], "local", "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/storage",
                &token,
                json!({
                    "storage_url": "https://cdn.example.com",
                    "keep_local": true,
                    "ftp_webroot": "/var/www/",
                    "ssh_webroot": "/data\\"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/storage",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["storage_url"], "https://cdn.example.com/", "{body}");
        assert_eq!(body["data"]["keep_local"], true, "keep_local 不该被强制改：{body}");
        assert_eq!(body["data"]["ftp_webroot"], "/var/www", "{body}");
        assert_eq!(body["data"]["ssh_webroot"], "/data", "{body}");

        // SSH 私钥上传
        let res = app
            .clone()
            .oneshot(multipart_named_request(
                "/system/api/plugin/storage/upload",
                &token,
                "stBoundary1",
                "name",
                "file",
                "id_rsa",
                b"-----BEGIN KEY-----",
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"], "ssh_private_key.key");
        let key_path = std::env::temp_dir()
            .join(format!("rustcms-cfg-{}", std::process::id()))
            .join("data")
            .join("cert")
            .join("ssh_private_key.key");
        assert!(key_path.is_file(), "私钥应已写入 {key_path:?}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/storage",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["ssh_private_key"], "ssh_private_key.key", "{body}");

        // ---- 小程序配置 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/weapp/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["server_url"]
            .as_str()
            .unwrap()
            .ends_with("/api/wechat"), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/weapp/config",
                &token,
                json!({"app_id": "wxapp123", "server_url": "https://fake/hack"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/weapp/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["app_id"], "wxapp123", "{body}");
        assert!(
            !body["data"]["server_url"].as_str().unwrap().contains("fake"),
            "server_url 是派生字段：{body}"
        );

        // ---- 公众号配置 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/wechat/config",
                &token,
                json!({"app_id": "wxmp456", "verify_key": "rustcms"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/wechat/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["app_id"], "wxmp456", "{body}");
        assert_eq!(body["data"]["verify_key"], "rustcms", "{body}");

        // ---- 消息 ----
        let now = chrono::Utc::now().timestamp();
        let msg_id = sqlx::query(
            "INSERT INTO wechat_messages (created_time, updated_time, openid, content, reply, \
                reply_time) VALUES (?, ?, 'rustcms-route-openid', '你好', '', 0)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_id() as u32;

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/wechat/message/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].as_i64().unwrap() >= 1, "{body}");

        // 空回复被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/wechat/message/reply",
                &token,
                json!({"id": msg_id, "reply": "  "}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "请填写回复内容");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/wechat/message/reply",
                &token,
                json!({"id": msg_id, "reply": "收到"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["reply"], "收到", "{body}");

        // ---- 自动回复规则 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/wechat/reply/rule/save",
                &token,
                json!({}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "请填写关键词");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/wechat/reply/rule/save",
                &token,
                json!({"keyword": "rustcms-rt-kw-1", "content": "回复1", "is_default": true}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let rule1 = body["data"]["id"].as_u64().unwrap() as u32;

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/wechat/reply/rule/save",
                &token,
                json!({"keyword": "rustcms-rt-kw-2", "content": "回复2", "is_default": true}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 默认规则唯一：第一条应被取消
        let first = wechat::rule_get(&pool, rule1).await.unwrap().unwrap();
        assert!(!first.is_default, "默认规则应唯一");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/wechat/reply/rule/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].as_i64().unwrap() >= 2, "{body}");

        // ---- 菜单 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/wechat/menu/save",
                &token,
                json!({}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "请填写菜单名称");

        // 超长名称在入库前就被拦住（列是 varchar(20)）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/wechat/menu/save",
                &token,
                json!({"name": "rustcms-menu-name-too-long", "type": "view", "value": "/"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "菜单名称不能超过 20 个字符");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/wechat/menu/save",
                &token,
                json!({"name": "rustcms-rt-menu", "type": "view", "value": "https://example.com"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let parent = body["data"]["id"].as_u64().unwrap() as u32;
        assert_eq!(body["data"]["type"], "view", "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/wechat/menu/save",
                &token,
                json!({"name": "rustcms-rt-sub", "parent_id": parent, "type": "click", "value": "KEY"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let child = body["data"]["id"].as_u64().unwrap() as u32;

        // 上级不存在
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/wechat/menu/save",
                &token,
                json!({"name": "rustcms-rt-bad", "parent_id": 999999}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/wechat/menu/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == json!(child)));

        // 同步菜单未实现
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/wechat/menu/sync",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 删除父级连带子级
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/wechat/menu/delete",
                &token,
                json!({"id": parent}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["count"], 2, "{body}");
        assert!(wechat::menu_get(&pool, child).await.unwrap().is_none());

        // 收尾
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/wechat/message/delete",
                &token,
                json!({"id": msg_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/wechat/reply/rule/delete",
                &token,
                json!({"id": rule1}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        for (key, raw) in backups {
            match raw {
                Some(raw) => crate::models::setting::save_raw(&pool, key, &raw)
                    .await
                    .unwrap(),
                None => crate::models::setting::remove(&pool, key).await.unwrap(),
            }
        }
        let _ = sqlx::query("DELETE FROM wechat_reply_rules WHERE keyword LIKE 'rustcms-rt%'")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM wechat_menus WHERE name LIKE 'rustcms-rt%'")
            .execute(&pool)
            .await;

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_order_and_finance_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let name = "rustcms_order_admin";
        seed_temp_admin(&pool, 990006, name, "order-pass-123").await;
        let Some(app) = test_app().await else {
            return;
        };
        let token = login_admin(&app, "10.9.9.28", name, "order-pass-123").await;

        let backup = crate::models::setting::get_raw(&pool, "order").await.unwrap();
        let user_id: u32 = sqlx::query_scalar("SELECT id FROM users ORDER BY id LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp();
        let order_id = format!("rustcms-order-route-{now}");
        let order2_id = format!("rustcms-order2-route-{now}");
        let _ = sqlx::query("DELETE FROM orders WHERE order_id IN (?, ?)")
            .bind(&order_id)
            .bind(&order2_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM payments WHERE order_id IN (?, ?)")
            .bind(&order_id)
            .bind(&order2_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM finances WHERE order_id IN (?, ?)")
            .bind(&order_id)
            .bind(&order2_id)
            .execute(&pool)
            .await;

        // 配置
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/order/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["no_process"], false, "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/order/config",
                &token,
                json!({"no_process": true, "seller_percent": 25}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/order/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["no_process"], true, "{body}");
        assert_eq!(body["data"]["seller_percent"], 25, "{body}");

        // 造订单
        sqlx::query(
            "INSERT INTO orders (created_time, updated_time, order_id, payment_id, user_id, \
                `type`, status, amount) VALUES (?, ?, ?, '', ?, 'goods', 0, 1000)",
        )
        .bind(now)
        .bind(now)
        .bind(&order_id)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

        // 列表筛选
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/plugin/order/list?order_id={order_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["total"], 1, "{body}");

        // 详情
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/plugin/order/detail?order_id={order_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["user"].is_object(), "{body}");

        // 支付
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/order/pay",
                &token,
                json!({"order_id": order_id, "pay_way": "offline"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // no_process=true → 支付即完成（status 3）
        let st: i8 = sqlx::query_scalar("SELECT status FROM orders WHERE order_id = ?")
            .bind(&order_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(st, 3, "no_process 配置下付款应直接完成");

        // 财务流水
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/finance/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].as_i64().unwrap() >= 1, "{body}");

        // 导出
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/order/export",
                &token,
                json!({}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["header"][0], "order_id", "{body}");
        assert!(body["data"]["content"].as_array().unwrap().len() >= 1);

        // 第二单：取消 → 退款
        sqlx::query(
            "INSERT INTO orders (created_time, updated_time, order_id, payment_id, user_id, \
                `type`, status, amount) VALUES (?, ?, ?, '', ?, 'goods', 1, 100)",
        )
        .bind(now)
        .bind(now)
        .bind(&order2_id)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/order/canceled",
                &token,
                json!({"order_id": order2_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let st: i8 = sqlx::query_scalar("SELECT status FROM orders WHERE order_id = ?")
            .bind(&order2_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(st, -1);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/order/refund/apply",
                &token,
                json!({"order_id": order2_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let st: i8 = sqlx::query_scalar("SELECT status FROM orders WHERE order_id = ?")
            .bind(&order2_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(st, 8);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/order/refund",
                &token,
                json!({"order_id": order2_id, "status": 1}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let st: i8 = sqlx::query_scalar("SELECT status FROM orders WHERE order_id = ?")
            .bind(&order2_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(st, 9);

        // 不存在的订单
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/order/pay",
                &token,
                json!({"order_id": "not-exist"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "订单不存在");

        // 收尾
        let _ = sqlx::query("DELETE FROM payments WHERE order_id IN (?, ?)")
            .bind(&order_id)
            .bind(&order2_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM finances WHERE order_id IN (?, ?)")
            .bind(&order_id)
            .bind(&order2_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM orders WHERE order_id IN (?, ?)")
            .bind(&order_id)
            .bind(&order2_id)
            .execute(&pool)
            .await;

        match backup {
            Some(raw) => crate::models::setting::save_raw(&pool, "order", &raw)
                .await
                .unwrap(),
            None => crate::models::setting::remove(&pool, "order").await.unwrap(),
        }

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_pay_and_withdraw_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let Some(app) = test_app().await else {
            return;
        };
        seed_temp_admin(&pool, 990006, "rustcms_pay_admin", "pay-pass-123").await;
        let token = login_admin(&app, "10.12.12.1", "rustcms_pay_admin", "pay-pass-123").await;

        // ---- 支付账户 ----
        // 未登录
        let res = app
            .clone()
            .oneshot(request("GET", "/system/api/plugin/pay/accounts"))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 新增
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/pay/detail",
                &token,
                json!({
                    "pay_way": "alipay",
                    "account_name": "路由测试账户",
                    "status": 1,
                    "pay_config": {"app_id": "ali123"},
                    "min_amount": 100,
                    "is_default": false
                }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["msg"], "配置已更新");
        let account_id = body["data"]["id"].as_i64().unwrap();
        assert!(account_id > 0);

        // 列表 + 详情（account_name 来自 name 列）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/pay/accounts",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["account_name"] == "路由测试账户"));

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/plugin/pay/detail?id={account_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["pay_config"]["app_id"], "ali123");
        assert_eq!(
            body["data"]["pay_config"]["api_key"],
            "",
            "pay_config 缺字段补默认值"
        );

        // id=0
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/pay/detail?id=0",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "Invalid ID");

        // 证书上传：后缀白名单 + md5 命名固定 .pem
        let res = app
            .clone()
            .oneshot(multipart_named_request(
                "/system/api/plugin/pay/upload",
                &token,
                "payBoundary1",
                "name",
                "apiclient_cert.crt",
                "apiclient_cert.crt",
                b"-----BEGIN CERT-----",
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let cert_name = body["data"].as_str().unwrap().to_string();
        assert!(cert_name.ends_with(".pem"), "{body}");

        let res = app
            .clone()
            .oneshot(multipart_named_request(
                "/system/api/plugin/pay/upload",
                &token,
                "payBoundary2",
                "name",
                "evil.exe",
                "evil.exe",
                b"MZ",
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "文件名无效");

        // 统计（插入一条再查）
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO payment_statistics (account_id, stat_time, daily_amount, \
             monthly_amount, daily_count) VALUES (?, ?, 500, 800, 2)",
        )
        .bind(account_id)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/plugin/pay/statistic?account_id={account_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].as_i64().unwrap() >= 1, "{body}");
        assert_eq!(body["data"][0]["daily_amount"], 500);

        // 删除
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/pay/delete",
                &token,
                json!({"id": account_id}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/plugin/pay/detail?id={account_id}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);
        let _ = sqlx::query("DELETE FROM payment_statistics WHERE account_id = ?")
            .bind(account_id)
            .execute(&pool)
            .await;

        // ---- 提现 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/withdraw/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].is_i64(), "{body}");

        // 佣金不足的申请被拒（找一个没有待提现佣金的用户；找不到就跳过）
        let user_id: Option<u32> = sqlx::query_scalar(
            "SELECT id FROM users u WHERE NOT EXISTS \
             (SELECT 1 FROM commissions c WHERE c.user_id = u.id AND c.status = 0) \
             ORDER BY id LIMIT 1",
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        if let Some(user_id) = user_id {
            let res = app
                .clone()
                .oneshot(bearer_request(
                    "POST",
                    "/system/api/plugin/withdraw/apply",
                    &token,
                    json!({"user_id": user_id}),
                ))
                .await
                .unwrap();
            let body = body_json(res).await;
            assert_eq!(body["code"], 1, "{body}");
        }

        // 不存在的提现记录
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/withdraw/approval",
                &token,
                json!({"id": 429496729}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "提现记录不存在");

        drop_temp_admin(&pool).await;
    }

    #[tokio::test]
    async fn manage_sendmail_and_backup_endpoints_work() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let Some(app) = test_app().await else {
            return;
        };
        seed_temp_admin(&pool, 990002, "rustcms_mail_admin", "mail-pass-123").await;
        let token = login_admin(&app, "10.13.13.1", "rustcms_mail_admin", "mail-pass-123").await;

        // ---- 邮件配置 ----
        let sendmail_backup = crate::models::setting::get_raw(&pool, "sendmail").await.unwrap();
        // 清掉可能的残留配置，保证「默认值」断言成立，结尾再恢复
        crate::models::setting::remove(&pool, "sendmail").await.unwrap();

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/sendmail/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["server"], "", "默认配置补齐：{body}");

        // 保存：授权码设置真实值
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/sendmail/setting",
                &token,
                json!({
                    "server": "smtp.example.com",
                    "port": 465,
                    "use_ssl": 1,
                    "account": "bot@example.com",
                    "password": "real-secret-16char",
                    "recipient": "admin@example.com"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 读取：授权码被掩码
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/sendmail/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["password"], "********", "{body}");
        assert_eq!(body["data"]["server"], "smtp.example.com");

        // 掩码回传表示「未修改」，不覆盖真实授权码
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/sendmail/setting",
                &token,
                json!({"password": "********", "port": 587}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let raw: (String,) = sqlx::query_as("SELECT value FROM settings WHERE `key` = 'sendmail'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(raw.0.contains("real-secret-16char"), "掩码不该覆盖真实授权码：{}", raw.0);
        assert!(raw.0.contains("587"), "port 应已更新：{}", raw.0);

        // ---- 邮件模板 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/sendmail/templates",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let list = body["data"].as_array().unwrap();
        assert_eq!(list.len(), 8, "默认模板 8 个：{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/sendmail/template?key=guestbook",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"]["subject"]
            .as_str()
            .unwrap()
            .contains("SiteName"), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/sendmail/template?key=nope",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "Template not found");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/sendmail/template",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], "Missing key parameter");

        // 保存模板（覆盖 guestbook 的标题/内容并启用）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/sendmail/template",
                &token,
                json!({
                    "key": "guestbook",
                    "open": true,
                    "subject": "自定义标题",
                    "content": "自定义内容",
                    "delay": 30
                }),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["subject"], "自定义标题");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/sendmail/template?key=guestbook",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["subject"], "自定义标题", "应读到覆盖后的标题：{body}");
        assert_eq!(body["data"]["open"], true);

        // 只读模板 report：subject/content 覆盖无效
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/sendmail/template",
                &token,
                json!({"key": "report", "open": true, "subject": "hack", "content": "hack"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/sendmail/template?key=report",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["subject"], "", "只读模板不应被覆盖：{body}");
        assert_eq!(body["data"]["open"], true, "open 可以覆盖");

        // 预览：只传 key 也能补全渲染
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/sendmail/template/preview",
                &token,
                json!({"key": "new_order"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(
            body["data"]["subject"]
                .as_str()
                .unwrap()
                .contains("ORD20240915001"),
            "预览应渲染出订单号：{body}"
        );
        assert!(!body["data"]["subject"]
            .as_str()
            .unwrap()
            .contains("{{"), "{body}");
        assert!(
            body["data"]["content"]
                .as_str()
                .unwrap()
                .starts_with("<p>"),
            "纯文本应转 HTML：{body}"
        );

        // 测试发信：账户已设置 → 未实现提示
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/sendmail/test",
                &token,
                json!({"recipient": "a@b.c", "subject": "S", "message": "M"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert_eq!(body["msg"], "发送测试邮件需要 SMTP 客户端，尚未实现");

        // 发送记录列表（文件可能不存在，接口应返回空而不是报错）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/sendmail/list?current=1&pageSize=10",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].is_i64(), "{body}");

        match sendmail_backup {
            Some(raw) => crate::models::setting::save_raw(&pool, "sendmail", &raw)
                .await
                .unwrap(),
            None => crate::models::setting::remove(&pool, "sendmail").await.unwrap(),
        }

        // ---- 备份 ----
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/backup/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // dump：异步任务，轮询状态直到完成
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/backup/dump",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["msg"], "备份已开始");

        let mut backup_name = String::new();
        for _ in 0..60 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let res = app
                .clone()
                .oneshot(bearer_request(
                    "GET",
                    "/system/api/plugin/backup/status",
                    &token,
                    Value::Null,
                ))
                .await
                .unwrap();
            let body = body_json(res).await;
            if body["data"].is_null() {
                continue;
            }
            if body["data"]["finished"].as_bool().unwrap_or(false) {
                let res = app
                    .clone()
                    .oneshot(bearer_request(
                        "GET",
                        "/system/api/plugin/backup/list",
                        &token,
                        Value::Null,
                    ))
                    .await
                    .unwrap();
                let body = body_json(res).await;
                let list = body["data"].as_array().unwrap();
                assert!(!list.is_empty(), "备份完成后列表应有文件：{body}");
                backup_name = list[0]["name"].as_str().unwrap().to_string();
                break;
            }
        }
        assert!(!backup_name.is_empty(), "备份任务应在 30 秒内完成");

        // 备注
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/backup/remark",
                &token,
                json!({"name": backup_name, "remark": "测试备注"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["msg"], "处理成功");
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/backup/list",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"][0]["remark"], "测试备注", "{body}");

        // 导出（附件）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                &format!("/system/api/plugin/backup/export?name={backup_name}"),
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(
            res.headers()
                .get("content-disposition")
                .and_then(|value| value.to_str().ok())
                .map(|value| value.contains("attachment;"))
                .unwrap_or(false),
            "应是附件响应"
        );

        // 导入（.sql 直接落备份目录）
        let boundary = "bkBoundary1";
        let mut mp_body = Vec::new();
        mp_body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        mp_body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"file\"; filename=\"import-test.sql\"\r\n\
              Content-Type: application/octet-stream\r\n\r\n",
        );
        mp_body.extend_from_slice(b"SELECT 1;\r\n");
        mp_body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let peer: SocketAddr = "127.0.0.1:50002".parse().unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/system/api/plugin/backup/import")
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .header("authorization", format!("Bearer {token}"))
            .extension(ConnectInfo(peer))
            .body(Body::from(mp_body))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["file"], "import-test.sql");

        // 删除
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/backup/delete",
                &token,
                json!({"name": backup_name}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/backup/delete",
                &token,
                json!({"name": "import-test.sql"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 防穿越
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/backup/delete",
                &token,
                json!({"name": "../config.json"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");

        // 未登录
        let res = app
            .clone()
            .oneshot(request("GET", "/system/api/plugin/backup/list"))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        drop_temp_admin(&pool).await;
    }

    // ---------------------------- 翻译插件 ----------------------------

    #[tokio::test]
    async fn translate_config_logs_and_text_logs_flow() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let Some(app) = test_app().await else {
            return;
        };
        seed_temp_admin(&pool, 990002, "rustcms_tl_admin", "tl-pass-123").await;
        let token = login_admin(&app, "10.13.13.1", "rustcms_tl_admin", "tl-pass-123").await;

        // 未登录
        let res = app
            .clone()
            .oneshot(request("GET", "/system/api/plugin/translate/config"))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 配置默认值 + 字段级 merge
        let translate_backup = crate::models::setting::get_raw(&pool, "translate").await.unwrap();
        crate::models::setting::remove(&pool, "translate").await.unwrap();

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/translate/config",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["engine"], "", "默认配置补齐：{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/translate/config",
                &token,
                json!({"engine": "baidu", "baidu_app_id": "aid"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/translate/config",
                &token,
                json!({"engine": "deepl"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let raw: (String,) =
            sqlx::query_as("SELECT value FROM settings WHERE `key` = 'translate'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(raw.0.contains("deepl"), "engine 应已更新：{}", raw.0);
        assert!(raw.0.contains("aid"), "未传字段应保留：{}", raw.0);

        // 文本翻译日志：保存 → 列表 → 删除
        sqlx::query("DELETE FROM translate_text_logs WHERE `text` LIKE '__rustcms_route_tl_%'")
            .execute(&pool)
            .await
            .unwrap();

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/translate/log/text/save",
                &token,
                json!({"to_language": "en", "text": "__rustcms_route_tl_正文", "translated": "hello"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        let saved_id = body["data"]["id"].as_u64().unwrap();
        assert_eq!(body["data"]["md5"].as_str().unwrap().len(), 32, "{body}");

        // 空 text 被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/translate/log/text/save",
                &token,
                json!({"translated": "x"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert!(body["msg"].as_str().unwrap().contains("请填写内容"));

        // 列表（前缀筛选）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/translate/log/texts?text=__rustcms_route_tl_&pageSize=10",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].as_i64().unwrap() >= 1, "{body}");

        // 删除：id 不存在 / 缺参数 / 正常删除
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/translate/log/text/remove",
                &token,
                json!({"id": 9999999}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert!(body["msg"].as_str().unwrap().contains("不存在"), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/translate/log/text/remove",
                &token,
                json!({}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert!(body["msg"].as_str().unwrap().contains("请提供"), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/translate/log/text/remove",
                &token,
                json!({"id": saved_id}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        // 翻译日志列表（无数据也应回 total + data）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/translate/logs?current=1&pageSize=5",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["total"].is_i64(), "{body}");
        assert!(body["data"].is_array(), "{body}");

        // 操作日志已记录
        let (log_count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM admin_logs WHERE user_name = 'rustcms_tl_admin' \
             AND `log` = '更新翻译配置'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(log_count >= 2, "应记录两条配置更新日志：{log_count}");

        crate::models::setting::remove(&pool, "translate").await.unwrap();
        match translate_backup {
            Some(raw) => crate::models::setting::save_raw(&pool, "translate", &raw)
                .await
                .unwrap(),
            None => {}
        }
        drop_temp_admin(&pool).await;
    }

    // ---------------------------- llms.txt 插件 ----------------------------

    #[tokio::test]
    async fn llms_setting_build_and_status_flow() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let Some(app) = test_app().await else {
            return;
        };
        // 构建状态是进程级单例，先清掉别的用例留下的记录
        crate::models::llms::build_status_reset();
        seed_temp_admin(&pool, 990002, "rustcms_llms_admin", "llms-pass-123").await;
        let token = login_admin(&app, "10.13.13.1", "rustcms_llms_admin", "llms-pass-123").await;

        let llms_backup = crate::models::setting::get_raw(&pool, "llms").await.unwrap();
        crate::models::setting::remove(&pool, "llms").await.unwrap();

        // 未登录
        let res = app
            .clone()
            .oneshot(request("GET", "/system/api/plugin/llms/setting"))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // 默认配置
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/llms/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["open"], false, "默认配置：{body}");
        assert_eq!(body["data"]["file_status"], false);

        // 未开启时 build 被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/llms/build",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 1, "{body}");
        assert!(body["msg"].as_str().unwrap().contains("未开启"), "{body}");

        // status：无任务 = 已完成
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/llms/status",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["finished"], true, "{body}");
        assert_eq!(body["data"]["building"], false);

        // 开启并构建
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/llms/setting",
                &token,
                json!({"open": true, "include_category": true}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/llms/build",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "构建失败：{body}");

        // 构建产物落 public 目录（测试 app 的 public_dir 是临时目录）
        let public_dir = std::env::temp_dir()
            .join(format!("rustcms-public-{}", std::process::id()));
        let txt = std::fs::read_to_string(public_dir.join("llms.txt"))
            .expect("llms.txt 应已生成");
        assert!(txt.starts_with("# "), "llms.txt 应以标题开头：{txt}");
        assert!(txt.contains("## Full Content Export"));
        assert!(public_dir.join("llms-full.txt").is_file());

        // 构建后配置里的 last_update / file_status 已更新
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/llms/setting",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["file_status"], true, "{body}");
        assert!(body["data"]["last_update"].as_i64().unwrap() > 0, "{body}");

        // 构建刚结束：status 还挂着「已完成」状态（与 Go 内存状态一致）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/plugin/llms/status",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["status"], 2, "{body}");
        assert_eq!(body["data"]["percent"], 100);

        // 非对象配置被拒
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/plugin/llms/setting",
                &token,
                json!([1, 2]),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        match llms_backup {
            Some(raw) => crate::models::setting::save_raw(&pool, "llms", &raw).await.unwrap(),
            None => crate::models::setting::remove(&pool, "llms").await.unwrap(),
        }
        drop_temp_admin(&pool).await;
    }

    // ---------------------------- 云端服务账号 ----------------------------

    #[tokio::test]
    async fn anqi_info_unimplemented_writes_and_validations() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let Some(app) = test_app().await else {
            return;
        };
        seed_temp_admin(&pool, 990002, "rustcms_anqi_admin", "anqi-pass-123").await;
        let token = login_admin(&app, "10.13.13.1", "rustcms_anqi_admin", "anqi-pass-123").await;

        let anqi_backup = crate::models::setting::get_raw(&pool, "anqi").await.unwrap();
        crate::models::setting::remove(&pool, "anqi").await.unwrap();

        // 未登录
        let res = app
            .clone()
            .oneshot(request("GET", "/system/api/anqi/info"))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // info：默认配置 + valid 现算
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/anqi/info",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["user_name"], "");
        assert_eq!(body["data"]["valid"], false);

        // 已过期的登录态 → valid=false
        crate::models::setting::save_json(
            &pool,
            "anqi",
            &json!({"user_name": "tester", "expire_time": 1}),
        )
        .await
        .unwrap();
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/anqi/check",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["user_name"], "tester");
        assert_eq!(body["data"]["valid"], false);

        // 登录：空参 / 正常参数都明确未实现（外呼）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/login",
                &token,
                json!({"user_name": "", "password": ""}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], -1, "{body}");
        assert!(body["msg"].as_str().unwrap().contains("请输入"), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/login",
                &token,
                json!({"user_name": "u", "password": "p"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], -1, "{body}");
        assert!(body["msg"].as_str().unwrap().contains("尚未实现"), "{body}");

        // restart / extract：明确未实现
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/restart",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], -1);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/extract/keywords",
                &token,
                json!({}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], -1, "{body}");
        assert!(body["msg"].as_str().unwrap().contains("请提供"), "{body}");

        // translate：文档不存在是真实站内校验
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/translate",
                &token,
                json!({"id": 99999999, "to_language": "en"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], -1, "{body}");
        assert!(body["msg"].as_str().unwrap().contains("文档不存在"), "{body}");

        // translate：缺目标语言
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/translate",
                &token,
                json!({"id": 1}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], -1, "{body}");
        assert!(body["msg"].as_str().unwrap().contains("to_language"), "{body}");

        // stream/data：无任务时与 Go 未知 stream_id 行为一致
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/anqi/ai/stream/data?stream_id=nope",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["msg"], "finished");
        assert_eq!(body["data"], "");

        match anqi_backup {
            Some(raw) => crate::models::setting::save_raw(&pool, "anqi", &raw).await.unwrap(),
            None => crate::models::setting::remove(&pool, "anqi").await.unwrap(),
        }
        drop_temp_admin(&pool).await;
    }

    // ---------------------------- AI 对话 ----------------------------

    #[tokio::test]
    async fn ai_settings_history_sessions_upload_flow() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let Some(app) = test_app().await else {
            return;
        };
        seed_temp_admin(&pool, 990002, "rustcms_ai_admin", "ai-pass-123").await;
        let token = login_admin(&app, "10.13.13.1", "rustcms_ai_admin", "ai-pass-123").await;

        // ai_agents / ai_chat_messages 表由 Go 建，不存在时相关断言降级
        async fn table_exists(pool: &sqlx::MySqlPool, name: &str) -> bool {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM information_schema.tables \
                 WHERE table_schema = DATABASE() AND table_name = ?",
            )
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap_or(0);
            count > 0
        }
        let has_agents = table_exists(&pool, "ai_agents").await;
        let has_messages = table_exists(&pool, "ai_chat_messages").await;

        // 未登录
        let res = app
            .clone()
            .oneshot(request("GET", "/system/api/anqi/ai/settings"))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 1);

        // AI 配置：默认值 → 保存数组 → 非数组被拒 → 读回
        let ai_backup = crate::models::setting::get_raw(&pool, "ai_setting").await.unwrap();
        crate::models::setting::remove(&pool, "ai_setting").await.unwrap();

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/anqi/ai/settings",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["configs"], json!([]));
        assert!(body["data"]["mcp"].is_object(), "mcp 默认值：{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/ai/settings",
                &token,
                json!([{"api_key": "sk-rs", "model": "deepseek-chat", "name": "测试"}]),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["configs"][0]["api_key"], json!("sk-rs"));

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/ai/settings",
                &token,
                json!({"api_key": "not-array"}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], -1);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/anqi/ai/settings",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["data"]["configs"][0]["model"], json!("deepseek-chat"));

        // history：缺 session_id → -1；带 session_id → code 0
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/anqi/ai/history",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], -1, "{body}");
        assert_eq!(body["msg"], "session_id is required");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/anqi/ai/history?session_id=__rustcms_none__",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"], json!([]));

        // sessions
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/anqi/ai/sessions",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert!(body["data"].is_array());

        // agents（表存在时必须是 0 + 数组）
        if has_agents {
            let res = app
                .clone()
                .oneshot(bearer_request(
                    "GET",
                    "/system/api/anqi/ai/agents",
                    &token,
                    Value::Null,
                ))
                .await
                .unwrap();
            let body = body_json(res).await;
            assert_eq!(body["code"], 0, "{body}");
            assert!(body["data"].is_array());

            // agent chat：消息为空 / 智能体不存在是真实校验
            let res = app
                .clone()
                .oneshot(bearer_request(
                    "POST",
                    "/system/api/anqi/ai/agents/1/chat",
                    &token,
                    json!({"message": ""}),
                ))
                .await
                .unwrap();
            let body = body_json(res).await;
            assert_eq!(body["code"], -1, "{body}");
            assert_eq!(body["msg"], "消息不能为空");

            let res = app
                .clone()
                .oneshot(bearer_request(
                    "POST",
                    "/system/api/anqi/ai/agents/99999999/chat",
                    &token,
                    json!({"message": "你好"}),
                ))
                .await
                .unwrap();
            let body = body_json(res).await;
            assert_eq!(body["code"], -1, "{body}");
            assert_eq!(body["msg"], "智能体不存在");
        }

        // chat / confirm / health / trace / chat：未实现走 -1
        for (method, uri, payload) in [
            ("POST", "/system/api/anqi/ai/chat", json!({"message": "hi"})),
            ("GET", "/system/api/anqi/ai/health", Value::Null),
            ("GET", "/system/api/anqi/ai/chat/trace", Value::Null),
        ] {
            let res = app
                .clone()
                .oneshot(bearer_request(method, uri, &token, payload))
                .await
                .unwrap();
            let body = body_json(res).await;
            assert_eq!(body["code"], -1, "{uri}：{body}");
        }

        // confirm：参数校验与 Go 一致
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/ai/chat/confirm",
                &token,
                json!({"tool_call_id": "x"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], -1, "{body}");
        assert_eq!(body["msg"], "tool_call_id and decision are required");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/ai/chat/confirm",
                &token,
                json!({"tool_call_id": "x", "decision": "maybe"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], "decision must be allow/deny/once_allow/full_control");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/ai/chat/confirm",
                &token,
                json!({"tool_call_id": "x", "decision": "allow"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["msg"], "no pending approval for this tool_call_id");

        // image：空提示词 / 参考图下载失败（用不可达地址避免测试外呼云端）
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/ai/image",
                &token,
                json!({"prompt": ""}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], -1, "{body}");
        assert!(body["msg"].as_str().unwrap().contains("提示词"), "{body}");

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/ai/image",
                &token,
                json!({"prompt": "一只猫", "image": "http://127.0.0.1:1/a.png"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], -1, "{body}");
        assert!(body["msg"].as_str().unwrap().contains("下载参考图片失败"), "{body}");

        // image/confirm：舍弃（action=0）真实可用
        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/ai/image/confirm",
                &token,
                json!({"action": 0}),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let res = app
            .clone()
            .oneshot(bearer_request(
                "POST",
                "/system/api/anqi/ai/image/confirm",
                &token,
                json!({"action": 1, "url": "http://127.0.0.1:1/a.png"}),
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], -1, "{body}");
        assert!(body["msg"].as_str().unwrap().contains("下载图片失败"), "{body}");

        // image/histories：历史在云端服务侧，恒空
        let res = app
            .clone()
            .oneshot(bearer_request(
                "GET",
                "/system/api/anqi/ai/image/histories",
                &token,
                Value::Null,
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["total"], 0);
        assert_eq!(body["data"], json!([]));

        // upload：真实落盘
        let res = app
            .clone()
            .oneshot(multipart_named_request(
                "/system/api/anqi/ai/upload",
                &token,
                "rustcms-aiboundary",
                "session_id",
                "common",
                "hello.txt",
                b"chat attachment",
            ))
            .await
            .unwrap();
        let body = body_json(res).await;
        assert_eq!(body["code"], 0, "{body}");
        assert_eq!(body["data"]["file_name"], "hello.txt");
        assert_eq!(body["data"]["session_id"], "common");
        assert_eq!(body["data"]["file_ext"], ".txt");
        let saved_path = body["data"]["file_path"].as_str().unwrap();
        assert!(saved_path.starts_with("/cache/ai/upload/common/"), "{saved_path}");
        if has_messages {
            // 文件在测试 app 的 cache_dir 里（<config 目录>/cache）
            let cfg_dir = std::env::temp_dir()
                .join(format!("rustcms-cfg-{}", std::process::id()));
            let full = cfg_dir.join("cache/ai/upload/common");
            assert!(full.is_dir(), "上传目录应存在：{}", full.display());
            let count = std::fs::read_dir(&full).unwrap().count();
            assert!(count >= 1);
            // 清理
            let _ = std::fs::remove_dir_all(cfg_dir.join("cache/ai"));
        }

        match ai_backup {
            Some(raw) => crate::models::setting::save_raw(&pool, "ai_setting", &raw)
                .await
                .unwrap(),
            None => crate::models::setting::remove(&pool, "ai_setting").await.unwrap(),
        }
        drop_temp_admin(&pool).await;
        let _ = has_messages;
    }
}

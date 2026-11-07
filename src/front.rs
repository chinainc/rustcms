//! 前台路由核心管线（对齐 Go 版 `route/base.go` 的 `/{path:path}` 兜底链路）。
//!
//! 处理顺序与 Go 一致：
//! 1. [`check_301`]（`middleware.Check301`）：查 `redirects` 表做 301 跳转；
//! 2. [`parse_user_token`]（`middleware.ParseUserToken`）：解析会员 JWT /
//!    API Key，账号被禁用时直接回 JSON（与 Go 一致）；
//! 3. [`FrontCtx::serve_file`]（`controller.FileServe`）：public/ 静态文件、
//!    默认 robots.txt、favicon 缺省 400；
//! 4. [`FrontCtx::check_close_site`]（`controller.CheckCloseSite`）：闭站 /
//!    禁蜘蛛 / UA、IP 黑名单；
//! 5. [`parse_route`]（`controller.ParseRoute`）：伪静态规则按 Go 的顺序
//!    （search → archiveIndex → people → tag → page → category → archive）
//!    逐一匹配并做存在性校验；
//! 6. 按匹配结果渲染对应页面（模板选择优先级与 Go 各 controller 一致），
//!    全部落空则 [`not_found`]。
//!
//! 整个解析渲染包在 30s 超时里（对齐 `middleware.HandlerTimeout`）。
//!
//! 与 Go 的已知差异（都有注释）：
//! - 多语言子站、城市分站（place）、HTML 缓存、蜘蛛访问日志未移植；
//! - mode 4（自定义正则）在 `settings.rewrite.patten` 为空时回落到
//!   字符串模式 2 的规则（`models::url` 的既有约定）；patten 非空时按行解析；
//! - 会员主页 /people、/peoples 规则能匹配但明确提示未移植（会员中心未到批次）。

use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Map, Value};
use sqlx::mysql::MySqlPool;

use crate::auth::{self, TokenKind};
use crate::models::{archive, category, module, redirect, site_setting, tag, url, user, Category};
use crate::state::AppState;
use crate::template::site_provider::SiteProvider;
use crate::template::Scope;

/// 对齐 Go 版 `currentSite.Content.MaxPage`。
const MAX_PAGE: u32 = 1000;
/// 对齐 Go 版 `middleware.HandlerTimeout` 的 30 秒。
const HANDLER_TIMEOUT: Duration = Duration::from_secs(30);

/// 前台兜底入口（对齐 `app.Get("/{path:path}", middleware.Check301,
/// middleware.ParseUserToken, middleware.HandlerTimeout, controller.ReRouteContext)`）。
pub async fn reroute(State(state): State<AppState>, req: Request) -> Response {
    // 客户端 IP 只从代理头取（直连场景拿不到 SocketAddr 时跳过 IP 黑名单检查）
    let headers = req.headers().clone();
    let ip = client_ip(&headers);

    // controller.UseLimiter：限流 / 防刷（后台与静态资源在判定内部跳过）
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let referer = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if let Some(block) =
        crate::models::plugin::limiter_check(ip.as_deref(), user_agent, referer, req.uri().path())
    {
        let status = StatusCode::from_u16(block.status).unwrap_or(StatusCode::FORBIDDEN);
        return (status, block.body).into_response();
    }

    match tokio::time::timeout(HANDLER_TIMEOUT, pipeline(state, req, ip)).await {
        Ok(res) => res,
        Err(_) => (StatusCode::REQUEST_TIMEOUT, "Request Timeout").into_response(),
    }
}

async fn pipeline(state: AppState, req: Request, ip: Option<String>) -> Response {
    let uri = req.uri().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let headers = req.headers().clone();

    // controller.FileServe：静态文件不依赖数据库，放在 DB 检查之前
    if let Some(res) = FrontCtx::serve_file(&state, &path).await {
        return res;
    }

    // 页面请求没配数据库时：首页保留探活文本，其余 404
    let Some(pool) = state.db().cloned() else {
        if path == "/" {
            return html_response(format!(
                "rustcms {} 正在运行\n探活接口：/api/health\n",
                env!("CARGO_PKG_VERSION")
            ));
        }
        return (StatusCode::NOT_FOUND, "404 page not found").into_response();
    };

    // middleware.Check301
    if let Some(res) = check_301(&pool, &uri).await {
        return res;
    }

    // middleware.ParseUserToken（账号被禁用时 Go 直接回 JSON 并中断链路）
    let (front_user, renew_token) = match parse_user_token(&pool, &headers).await {
        Ok(value) => value,
        Err(res) => return res,
    };

    let ctx = FrontCtx {
        state,
        pool,
        path,
        query,
        headers,
        front_user,
        ip,
    };

    // controller.CheckCloseSite
    if let Some(res) = ctx.check_close_site().await {
        return with_renew(res, &renew_token);
    }

    // controller.ReRouteContext：解析 + 渲染
    let res = ctx.dispatch().await;
    with_renew(res, &renew_token)
}

/// 登录过期前 1 小时内下发续签令牌（对齐 Go 的 `update-token` 响应头）。
fn with_renew(mut res: Response, renew_token: &Option<String>) -> Response {
    if let Some(token) = renew_token {
        if let Ok(value) = header::HeaderValue::from_str(token) {
            res.headers_mut().insert("update-token", value);
        }
    }
    res
}

// ---------------------------------------------------------------------------
// 中间件对应实现
// ---------------------------------------------------------------------------

/// `middleware.Check301`：`redirects` 表命中且目标不是自身时 301。
async fn check_301(pool: &MySqlPool, uri: &str) -> Option<Response> {
    let Some(item) = redirect::get_by_from(pool, uri).await.ok().flatten() else {
        return None;
    };
    if item.to_url == uri {
        return None;
    }
    Some((
        StatusCode::MOVED_PERMANENTLY,
        [(header::LOCATION, item.to_url)],
    )
        .into_response())
}

/// 已登录的会员信息（模板里的 `userId` / `userInfo` / `userGroup`）。
pub struct FrontUser {
    pub id: u32,
    pub info: Value,
    /// 会员分组（对齐 Go 的 `ctx.ViewData("userGroup", userGroup)`）
    pub group: Value,
}

/// `middleware.ParseUserToken`。
///
/// 返回 `(会员信息, 续签令牌)`；会员被禁用时返回 `Err(JSON 响应)`
/// （与 Go 的 `{"code":1001,"msg":"ThisAccountHasBeenDisabled"}` 一致）。
async fn parse_user_token(
    pool: &MySqlPool,
    headers: &HeaderMap,
) -> Result<(Option<FrontUser>, Option<String>), Response> {
    // token 来源优先级：header "token" > Authorization: Bearer > cookie "token"
    let mut token = headers
        .get("token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if token.is_none() {
        token = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
    }
    if token.is_none() {
        token = read_cookie(headers, "token");
    }
    let Some(token) = token else {
        return Ok((None, None));
    };

    let secret = match auth::token_secret(pool).await {
        Ok(secret) => secret,
        Err(err) => {
            tracing::warn!("读取 token_secret 失败，按匿名处理：{err:#}");
            return Ok((None, None));
        }
    };

    let mut renew = None;
    if let Ok(claims) = auth::verify(TokenKind::User, &secret, &token) {
        if let Some(id) = claims.subject(TokenKind::User) {
            // 登录过期时间在 1 小时内则续签（对齐 Go）
            if let Ok(expire) = claims.expire.trim().parse::<i64>() {
                if expire < chrono::Utc::now().timestamp() + 3600 {
                    if let Ok((new_token, _)) = auth::issue(TokenKind::User, &secret, id, false) {
                        renew = Some(new_token);
                    }
                }
            }
            if let Ok(Some(info)) = user::get(pool, id).await {
                // 账号被禁用：Go 直接回 JSON 并中断
                if !info.status {
                    return Err((
                        StatusCode::OK,
                        axum::Json(json!({
                            "code": 1001,
                            "msg": "ThisAccountHasBeenDisabled",
                        })),
                    )
                        .into_response());
                }
                let member = FrontUser {
                    id: info.id,
                    info: serde_json::to_value(&info).unwrap_or(Value::Null),
                    group: user_group_value(pool, info.group_id).await,
                };
                return Ok((Some(member), renew));
            }
        }
    }

    // JWT 无效/未携带时回退到会员 API Key（对齐 Go 的 GetUserByApiToken）
    let api_token: Option<u32> =
        sqlx::query_scalar::<_, u32>("SELECT id FROM users WHERE api_token = ? AND status = 1 LIMIT 1")
            .bind(&token)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    if let Some(id) = api_token {
        if let Ok(Some(info)) = user::get(pool, id).await {
            let member = FrontUser {
                id: info.id,
                info: serde_json::to_value(&info).unwrap_or(Value::Null),
                group: user_group_value(pool, info.group_id).await,
            };
            return Ok((Some(member), None));
        }
    }

    Ok((None, renew))
}

/// 读会员分组（读失败时为 null，不影响登录态）。
async fn user_group_value(pool: &MySqlPool, group_id: u32) -> Value {
    match crate::models::user_admin::group_get(pool, group_id).await {
        Ok(Some(group)) => serde_json::to_value(&group).unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// 从代理头取客户端 IP（无代理头时返回 None）。
fn client_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-real-ip")
        .or_else(|| headers.get("x-forwarded-for"))
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.split(',').next().unwrap_or(value).trim().to_string())
}

/// 读 cookie 值（只做最基本的分号切分）。
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

// ---------------------------------------------------------------------------
// 管线上下文
// ---------------------------------------------------------------------------

struct FrontCtx {
    state: AppState,
    pool: MySqlPool,
    path: String,
    query: String,
    headers: HeaderMap,
    front_user: Option<FrontUser>,
    ip: Option<String>,
}

impl FrontCtx {
    /// `controller.FileServe`：public/ 下的静态文件直接回；未命中继续路由。
    async fn serve_file(state: &AppState, path: &str) -> Option<Response> {
        // 目录类请求交给路由（Go：uri 以 / 结尾不 serve 文件）
        if path == "/" || path.ends_with('/') {
            return None;
        }
        let rel = path.trim_start_matches('/');
        // 防路径穿越：逐段校验（Go 用 filepath.Clean + 前缀校验）
        if rel.is_empty()
            || rel.split('/').any(|seg| {
                seg.is_empty() || seg == ".." || seg.contains('\\') || seg.starts_with('.')
            })
        {
            return None;
        }

        let base = state.with_config(|c| c.public_dir());
        let file = base.join(rel);
        if let Ok(meta) = std::fs::metadata(&file) {
            if meta.is_file() {
                let bytes = std::fs::read(&file).ok()?;
                return Some(
                    ([(
                        header::CONTENT_TYPE,
                        header::HeaderValue::from_static(content_type(path)),
                    )], bytes)
                        .into_response(),
                );
            }
        }

        // 避开 favicon.ico（对齐 Go：文件不存在时 400）
        if path.ends_with("favicon.ico") {
            return Some(StatusCode::BAD_REQUEST.into_response());
        }
        // robots.txt 缺省内容（对齐 Go 的自动生成；Sitemap 行依赖 base_url，略）
        if path == "/robots.txt" {
            return Some((
                [(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static("text/plain; charset=utf-8"),
                )],
                "User-agent: *\nDisallow: /system\nDisallow: /static\n",
            )
                .into_response());
        }
        // sitemap.xml / sitemap.txt：文件存在时已被上面的逻辑服务；
        // 自动生成未移植（后台 plugin/sitemap/build 可手动生成），此处直接落空。

        None
    }

    /// `controller.CheckCloseSite`：闭站 / 禁蜘蛛 / UA、IP 黑名单。
    async fn check_close_site(&self) -> Option<Response> {
        let system = site_setting::get_section(&self.pool, "system").await.ok()?;
        let ua = self
            .headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let spider = is_spider(&ua);

        // 闭站：1 = 全部关闭，2 = 仅蜘蛛可见
        let site_close = match system.get("site_close").and_then(|v| v.as_i64()) {
            Some(1) => true,
            Some(2) => !spider,
            _ => false,
        };
        if site_close {
            let tips = system
                .get("site_close_tips")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Some(self.close_page(&tips).await);
        }
        // 禁止蜘蛛
        if system.get("ban_spider").and_then(|v| v.as_i64()) == Some(1) && spider {
            return Some((
                StatusCode::FORBIDDEN,
                html_response(show_message("您已被禁止访问")),
            )
                .into_response());
        }
        // UA 黑名单（safe.ua_forbidden，一行一个关键词）
        if let Ok(safe) = site_setting::get_section(&self.pool, "safe").await {
            let ua_forbidden = safe
                .get("ua_forbidden")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            for keyword in ua_forbidden.lines() {
                let keyword = keyword.trim();
                if !keyword.is_empty() && ua.contains(&keyword.to_ascii_lowercase()) {
                    return Some((
                        StatusCode::FORBIDDEN,
                        html_response(show_message("您已被禁止访问")),
                    )
                        .into_response());
                }
            }
            // IP 黑名单（safe.ip_forbidden，前缀匹配；127.0.0.1 放行）
            if let Some(ip) = &self.ip {
                if ip != "127.0.0.1" {
                    let ip_forbidden = safe
                        .get("ip_forbidden")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    for line in ip_forbidden.lines() {
                        let mut rule = line.trim();
                        if rule.is_empty() {
                            continue;
                        }
                        // 移除子网掩码与末尾 .0（对齐 Go）
                        rule = rule.split('/').next().unwrap_or(rule);
                        while rule.ends_with(".0") {
                            rule = rule.trim_end_matches(".0");
                        }
                        if ip.starts_with(rule) {
                            return Some((
                                StatusCode::FORBIDDEN,
                                html_response(show_message("您已被禁止访问")),
                            )
                                .into_response());
                        }
                    }
                }
            }
        }

        None
    }

    /// 闭站提示页：errors/close.html，缺模板时退回纯提示。
    async fn close_page(&self, tips: &str) -> Response {
        let scope = self.base_scope(1);
        let mut scope = scope;
        scope.insert("closeTips".into(), Value::String(tips.to_string()));
        if let Ok(provider) = SiteProvider::new(self.pool.clone(), 0).await {
            if let Ok(html) = self
                .state
                .templates()
                .render("errors/close.html", &scope, &provider)
                .await
            {
                return (StatusCode::FORBIDDEN, html_response(html)).into_response();
            }
        }
        (StatusCode::FORBIDDEN, html_response(show_message(tips))).into_response()
    }

    /// `controller.ReRouteContext`：解析路由并渲染。
    async fn dispatch(&self) -> Response {
        let mode = match url::load_mode(&self.pool).await {
            Ok(mode) => mode,
            Err(err) => return internal_error(&self.state, &self.pool, &format!("{err:#}")).await,
        };

        let page = query_page(&self.query);
        if page > MAX_PAGE {
            return self.not_found().await;
        }

        let matched = self.parse_route(mode).await;
        match matched {
            RouteMatch::Index => self.render_index().await,
            RouteMatch::ArchiveIndex { module, page } => {
                self.render_module_page(&module, page).await
            }
            RouteMatch::TagIndex { page } => self.render_tag_index(page).await,
            RouteMatch::Tag { id, token, page } => self.render_tag_page(id, token, page).await,
            RouteMatch::Page { token } => self.render_single_page(&token).await,
            RouteMatch::Category { token, page } => self.render_category_page(&token, page).await,
            RouteMatch::Archive {
                id,
                token,
                module: module_token,
                catname,
            } => self.render_archive_page(id, token, module_token, catname).await,
            RouteMatch::Search { module } => self.render_search(module, page).await,
            RouteMatch::NotImplemented(what) => (
                StatusCode::NOT_FOUND,
                html_response(show_message(&format!("{what} 尚未移植"))),
            )
                .into_response(),
            RouteMatch::NotFound => self.not_found().await,
        }
    }

    // -- 路由解析（对齐 controller.ParseRoute 的规则顺序） ------------------

    async fn parse_route(&self, mode: i32) -> RouteMatch {
        // url::match_path 的规则串带前导斜杠，因此用完整 path 匹配
        let param = &self.path;
        if self.path.trim_start_matches('/').is_empty() {
            return RouteMatch::Index;
        }
        // 静态资源目录没命中文件时直接 404（对齐 Go）
        if param.starts_with("/uploads/")
            || param.starts_with("/static/")
            || param.starts_with("/system/")
        {
            return RouteMatch::NotFound;
        }
        // 固化链接（Go 的 GetFixedLinkFromCache）
        if let Ok(Some(id)) =
            sqlx::query_scalar::<_, i64>("SELECT id FROM archives WHERE fixed_link = ? LIMIT 1")
                .bind(&self.path)
                .fetch_optional(&self.pool)
                .await
        {
            return RouteMatch::Archive {
                id: Some(id),
                token: None,
                module: None,
                catname: None,
            };
        }

        let page = |values: &std::collections::HashMap<String, String>| {
            values
                .get("page")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(1)
        };

        // 规则顺序对齐 Go：search → archiveIndex → peopleIndex → people →
        // tagIndex → tag → page → category → archive
        if let Some(values) = url::match_path(mode, url::Kind::Search, param) {
            return RouteMatch::Search {
                module: values.get("module").cloned().filter(|v| !v.is_empty()),
            };
        }
        if let Some(values) = url::match_path(mode, url::Kind::ArchiveIndex, param) {
            if let Some(token) = values.get("module") {
                // 模块必须真实存在（按 url_token 匹配，对齐 Go），否则继续
                if let Ok(Some(item)) = module::get_by_token(&self.pool, token).await {
                    let _ = item;
                    return RouteMatch::ArchiveIndex {
                        module: token.clone(),
                        page: page(&values),
                    };
                }
            }
        }
        if url::match_path(mode, url::Kind::PeopleIndex, param).is_some()
            || url::match_path(mode, url::Kind::People, param).is_some()
        {
            return RouteMatch::NotImplemented("会员主页");
        }
        if let Some(values) = url::match_path(mode, url::Kind::TagIndex, param) {
            return RouteMatch::TagIndex { page: page(&values) };
        }
        if let Some(values) = url::match_path(mode, url::Kind::Tag, param) {
            let id = values.get("id").and_then(|v| v.parse::<u32>().ok());
            let token = values.get("filename").cloned().filter(|v| !v.is_empty());
            return RouteMatch::Tag {
                id,
                token,
                page: page(&values),
            };
        }
        if let Some(values) = url::match_path(mode, url::Kind::Page, param) {
            if let Some(token) = values.get("filename") {
                // 必须是单页类型分类（对齐 Go 的校验），否则继续
                if let Ok(Some(item)) = category::get_by_token(&self.pool, token).await {
                    if item.is_page() && item.status == 1 {
                        return RouteMatch::Page {
                            token: token.clone(),
                        };
                    }
                }
            }
        }
        if let Some(values) = url::match_path(mode, url::Kind::Category, param) {
            if let Some(token) = values.get("catname") {
                if let Ok(Some(item)) = category::get_by_token(&self.pool, token).await {
                    if !item.is_page() && item.status == 1 {
                        return RouteMatch::Category {
                            token: token.clone(),
                            page: page(&values),
                        };
                    }
                }
            }
        }
        if let Some(values) = url::match_path(mode, url::Kind::Archive, param) {
            let id = values.get("id").and_then(|v| v.parse::<i64>().ok());
            let token = values.get("filename").cloned().filter(|v| !v.is_empty());
            return RouteMatch::Archive {
                id,
                token,
                module: values.get("module").cloned().filter(|v| !v.is_empty()),
                catname: values.get("catname").cloned().filter(|v| !v.is_empty()),
            };
        }

        RouteMatch::NotFound
    }

    // -- 各页面的模板选择与渲染 ---------------------------------------------

    /// 模板候选列表里第一个真实存在的文件。
    fn template_exist(&self, candidates: &[&str]) -> Option<String> {
        let dir = self.state.templates().dir().to_path_buf();
        candidates
            .iter()
            .find(|name| dir.join(name).is_file())
            .map(|name| name.to_string())
    }

    /// 公共模板上下文（对齐 Go 的 Common 中间件注入的变量）。
    fn base_scope(&self, page: u32) -> Scope {
        let mut scope = Scope::new();
        scope.insert("currentPage".into(), Value::from(page));
        scope.insert("version".into(), Value::from(env!("CARGO_PKG_VERSION")));
        // 手机端模板切换未移植，固定 false；UA 简单判断用于变量本身
        let ua = self
            .headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        scope.insert("isMobile".into(), Value::Bool(ua.contains("mobile")));
        scope.insert("urlParams".into(), query_params(&self.query));
        if let Some(member) = &self.front_user {
            scope.insert("userId".into(), Value::from(member.id));
            scope.insert("userInfo".into(), member.info.clone());
            scope.insert("userGroup".into(), member.group.clone());
        }
        scope
    }

    async fn render_with(&self, template: &str, scope: &Scope, provider: &SiteProvider) -> Response {
        match self.state.templates().render(template, scope, provider).await {
            Ok(html) => html_response(html),
            // 渲染失败 → 500 页（对齐 Go 的 OnErrorCode(500)）
            Err(err) => internal_error(&self.state, &self.pool, &format!("{err:#}")).await,
        }
    }

    /// 首页（`controller.IndexPage`）。
    async fn render_index(&self) -> Response {
        let Some(template) = self.template_exist(&["index/index.html", "index.html"]) else {
            return self.not_found().await;
        };
        let Ok(provider) = SiteProvider::new(self.pool.clone(), 0).await else {
            return self.not_found().await;
        };
        let scope = self.base_scope(1);
        self.render_with(&template, &scope, &provider).await
    }

    /// 模块列表页（`controller.ArchiveIndex`）。
    async fn render_module_page(&self, module_token: &str, page: u32) -> Response {
        let Some(item) = module::get_by_token(&self.pool, module_token)
            .await
            .ok()
            .flatten()
        else {
            return self.not_found().await;
        };
        let template = match self.template_exist(&[
            &format!("{}/index.html", item.table_name),
            &format!("{}_index.html", item.table_name),
        ]) {
            Some(template) => template,
            None => return self.not_found().await,
        };
        let provider = match SiteProvider::for_module(self.pool.clone(), item.id).await {
            Ok(provider) => provider.with_page(page),
            Err(_) => return self.not_found().await,
        };
        let mut scope = self.base_scope(page);
        if let Ok(value) = serde_json::to_value(&item) {
            scope.insert("module".into(), value);
        }
        self.render_with(&template, &scope, &provider).await
    }

    /// 栏目列表页（`controller.CategoryPage`）。
    pub async fn render_category_page(&self, token: &str, page: u32) -> Response {
        let Some(item) = category::get_by_token(&self.pool, token).await.ok().flatten()
        else {
            return self.not_found().await;
        };
        self.render_category(&item, page).await
    }

    /// 栏目渲染（供 `/preview/list/:cid` 复用）：模板优先级对齐 Go
    /// `CategoryPage`：设置的 template → {模块}/{url_token}.html →
    /// {模块}/list-{id}.html → {模块}/list.html → {模块}_list.html。
    pub async fn render_category(&self, item: &Category, page: u32) -> Response {
        if page > MAX_PAGE {
            return self.not_found().await;
        }
        let modules = match module::list(&self.pool).await {
            Ok(list) => list,
            Err(err) => {
                return internal_error(&self.state, &self.pool, &format!("{err:#}")).await
            }
        };
        let table = modules
            .iter()
            .find(|m| m.id == item.module_id)
            .map(|m| m.table_name.clone())
            .unwrap_or_else(|| "article".to_string());

        let mut tpl = item.template.trim().to_string();
        if !tpl.is_empty() && !tpl.ends_with(".html") {
            tpl.push_str(".html");
        }
        let template = match self.template_exist(&[
            &tpl,
            &format!("{}/{}.html", table, item.url_token),
            &format!("{}/list-{}.html", table, item.id),
            &format!("{}/list.html", table),
            &format!("{}_list.html", table),
        ]) {
            Some(template) => template,
            None => return self.not_found().await,
        };

        let provider = match SiteProvider::new(self.pool.clone(), item.id).await {
            Ok(provider) => provider.with_page(page),
            Err(_) => return self.not_found().await,
        };
        let mut scope = self.base_scope(page);
        if let Ok(value) = serde_json::to_value(item) {
            scope.insert("category".into(), value);
        }
        if let Some(target) = modules.iter().find(|m| m.id == item.module_id) {
            if let Ok(value) = serde_json::to_value(target) {
                scope.insert("module".into(), value);
            }
        }
        self.render_with(&template, &scope, &provider).await
    }

    /// 单页（`controller.PagePage`）。
    async fn render_single_page(&self, token: &str) -> Response {
        let Some(item) = category::get_by_token(&self.pool, token).await.ok().flatten()
        else {
            return self.not_found().await;
        };
        if !item.is_page() || item.status != 1 {
            return self.not_found().await;
        }

        let mut tpl = item.template.trim().to_string();
        if !tpl.is_empty() && !tpl.ends_with(".html") {
            tpl.push_str(".html");
        }
        let template = match self.template_exist(&[
            &tpl,
            &format!("page/{}.html", item.url_token),
            &format!("page/detail-{}.html", item.id),
            &format!("page-{}.html", item.id),
            "page/detail.html",
            "page_detail.html",
        ]) {
            Some(template) => template,
            None => return self.not_found().await,
        };

        let provider = match SiteProvider::new(self.pool.clone(), item.id).await {
            Ok(provider) => provider,
            Err(_) => return self.not_found().await,
        };
        let mut scope = self.base_scope(1);
        if let Ok(value) = serde_json::to_value(&item) {
            scope.insert("page".into(), value);
        }
        self.render_with(&template, &scope, &provider).await
    }

    /// 内容详情页（`controller.ArchiveDetail`）。
    async fn render_archive_page(
        &self,
        id: Option<i64>,
        token: Option<String>,
        module_token: Option<String>,
        catname: Option<String>,
    ) -> Response {
        // 定位内容
        let found = match (id, token.as_deref()) {
            (Some(id), _) => archive::get(&self.pool, id).await,
            (None, Some(token)) => archive::get_by_token(&self.pool, token).await,
            _ => Ok(None),
        };
        let Ok(Some(item)) = found else {
            return self.not_found().await;
        };
        // module / catname 校验（对齐 Go：对不上直接 404）
        if let Some(token) = &module_token {
            match module::get_by_token(&self.pool, token).await {
                Ok(Some(m)) if m.id == item.module_id => {}
                _ => return self.not_found().await,
            }
        }
        if let Some(token) = &catname {
            match category::get_by_token(&self.pool, token).await {
                Ok(Some(c)) if c.id == item.category_id => {}
                _ => return self.not_found().await,
            }
        }

        let modules = match module::list(&self.pool).await {
            Ok(list) => list,
            Err(err) => {
                return internal_error(&self.state, &self.pool, &format!("{err:#}")).await
            }
        };
        let current = category::get(&self.pool, item.category_id)
            .await
            .ok()
            .flatten();
        let table = current
            .as_ref()
            .and_then(|c| {
                modules
                    .iter()
                    .find(|m| m.id == c.module_id)
                    .map(|m| m.table_name.clone())
            })
            .unwrap_or_else(|| "article".to_string());

        // 模板优先级对齐 Go `ArchiveDetail`：内容设置的 template →
        // 栏目的 detail_template → {模块}/detail-{id}.html → {模块}/detail.html
        // → {模块}_detail.html
        let mut tpl = item.template.trim().to_string();
        if tpl.is_empty() {
            tpl = current
                .as_ref()
                .map(|c| c.detail_template.trim().to_string())
                .unwrap_or_default();
        }
        if !tpl.is_empty() && !tpl.ends_with(".html") {
            tpl.push_str(".html");
        }
        let template = match self.template_exist(&[
            &tpl,
            &format!("{}/detail-{}.html", table, item.id),
            &format!("{}/detail.html", table),
            &format!("{}_detail.html", table),
        ]) {
            Some(template) => template,
            None => return self.not_found().await,
        };

        let provider = match SiteProvider::for_archive(self.pool.clone(), item.id).await {
            Ok(Some(provider)) => provider,
            _ => return self.not_found().await,
        };
        let mut scope = self.base_scope(1);
        if let Ok(value) = serde_json::to_value(&item) {
            scope.insert("archive".into(), value);
        }
        if let Some(target) = &current {
            if let Ok(value) = serde_json::to_value(target) {
                scope.insert("category".into(), value);
            }
        }
        self.render_with(&template, &scope, &provider).await
    }

    /// 标签列表页（`controller.TagIndexPage`）。
    async fn render_tag_index(&self, page: u32) -> Response {
        let Some(template) = self.template_exist(&["tag/index.html", "tag_index.html"]) else {
            return self.not_found().await;
        };
        let Ok(provider) = SiteProvider::new(self.pool.clone(), 0).await else {
            return self.not_found().await;
        };
        let provider = provider.as_tag_index().with_page(page);
        let scope = self.base_scope(page);
        self.render_with(&template, &scope, &provider).await
    }

    /// 标签详情页（`controller.TagPage`）。
    async fn render_tag_page(&self, id: Option<u32>, token: Option<String>, page: u32) -> Response {
        // 优先 url_token（对齐 Go）
        let found = match (token.as_deref(), id) {
            (Some(token), _) if !token.is_empty() => tag::get_by_token(&self.pool, token).await,
            (_, Some(id)) => tag::get(&self.pool, id).await,
            _ => Ok(None),
        };
        let Ok(Some(item)) = found else {
            return self.not_found().await;
        };

        let template = match self.template_exist(&[
            &format!("tag/list-{}.html", item.id),
            "tag/list.html",
            "tag_list.html",
        ]) {
            Some(template) => template,
            None => return self.not_found().await,
        };

        let Ok(provider) = SiteProvider::new(self.pool.clone(), 0).await else {
            return self.not_found().await;
        };
        let provider = provider.with_tag(item.clone()).with_page(page);
        let mut scope = self.base_scope(page);
        if let Ok(value) = serde_json::to_value(&item) {
            scope.insert("tag".into(), value);
        }
        self.render_with(&template, &scope, &provider).await
    }

    /// 搜索页（`controller.SearchPage`）。
    async fn render_search(&self, module_token: Option<String>, page: u32) -> Response {
        let mut module_row = None;
        if let Some(token) = &module_token {
            match module::get_by_token(&self.pool, token).await {
                Ok(Some(item)) => module_row = Some(item),
                // 模块不存在（对齐 Go：UndefinedModel 提示）
                _ => {
                    return (
                        StatusCode::NOT_FOUND,
                        html_response(show_message("未定义的模型")),
                    )
                        .into_response()
                }
            }
        }
        let q = query_params(&self.query)
            .get("q")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        let template = match &module_row {
            Some(item) => match self.template_exist(&[
                &format!("search/{}.html", item.url_token),
                "search/index.html",
                "search.html",
            ]) {
                Some(template) => template,
                None => return self.not_found().await,
            },
            None => match self.template_exist(&["search/index.html", "search.html"]) {
                Some(template) => template,
                None => return self.not_found().await,
            },
        };

        let base = match &module_row {
            Some(item) => SiteProvider::for_module(self.pool.clone(), item.id).await,
            None => SiteProvider::new(self.pool.clone(), 0).await,
        };
        let Ok(provider) = base else {
            return self.not_found().await;
        };
        let provider = provider.with_keyword(&q).with_page(page);
        let mut scope = self.base_scope(page);
        scope.insert("q".into(), Value::String(q));
        if let Some(item) = &module_row {
            if let Ok(value) = serde_json::to_value(item) {
                scope.insert("module".into(), value);
            }
        }
        self.render_with(&template, &scope, &provider).await
    }

    /// `controller.NotFound`：errors/404.html，缺模板退回纯提示。
    async fn not_found(&self) -> Response {
        let scope = self.base_scope(1);
        if let Ok(provider) = SiteProvider::new(self.pool.clone(), 0).await {
            if let Ok(html) = self
                .state
                .templates()
                .render("errors/404.html", &scope, &provider)
                .await
            {
                return (StatusCode::NOT_FOUND, html_response(html)).into_response();
            }
        }
        (
            StatusCode::NOT_FOUND,
            html_response(show_message("404 Not Found")),
        )
            .into_response()
    }
}

// ---------------------------------------------------------------------------
// 错误页
// ---------------------------------------------------------------------------

/// 供 `/preview/list/:cid` 复用：按栏目直接渲染列表页。
pub async fn render_category_preview(state: &AppState, item: &Category) -> Response {
    let Some(pool) = state.db().cloned() else {
        return (StatusCode::NOT_FOUND, "404 page not found").into_response();
    };
    let ctx = FrontCtx {
        state: state.clone(),
        pool,
        path: String::new(),
        query: String::new(),
        headers: HeaderMap::new(),
        front_user: None,
        ip: None,
    };
    ctx.render_category(item, 1).await
}

/// `controller.InternalServerError`：errors/500.html，缺模板退回纯提示。
async fn internal_error(state: &AppState, pool: &MySqlPool, message: &str) -> Response {
    tracing::error!("前台渲染失败：{message}");
    if let Ok(provider) = SiteProvider::new(pool.clone(), 0).await {
        let mut scope = Scope::new();
        scope.insert("errMessage".into(), Value::String(message.to_string()));
        if let Ok(html) = state
            .templates()
            .render("errors/500.html", &scope, &provider)
            .await
        {
            return (StatusCode::INTERNAL_SERVER_ERROR, html_response(html))
                .into_response();
        }
    }
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        html_response(show_message(message)),
    )
        .into_response()
}

/// `controller.ShowMessage`：内联样式的提示页（Go 版逐字对齐，含返回按钮）。
fn show_message(message: &str) -> String {
    format!(
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
</div></body></html>"
    )
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

fn html_response(html: String) -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

/// 蜘蛛判断（Go `library.GetSpider` 的宽松版：含 spider/bot 即视为蜘蛛）。
fn is_spider(ua_lower: &str) -> bool {
    ua_lower.contains("spider") || ua_lower.contains("bot")
}

/// query string 里的 page 参数。
fn query_page(query: &str) -> u32 {
    query_params(query)
        .get("page")
        .and_then(|value| value.as_str())
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(1)
}

/// query string 解析成对象（值做 percent 解码）。
fn query_params(query: &str) -> Value {
    let mut map = Map::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((key, value)) => (key, value),
            None => (pair, ""),
        };
        let key = percent_encoding::percent_decode_str(key).decode_utf8_lossy().to_string();
        let value = percent_encoding::percent_decode_str(value)
            .decode_utf8_lossy()
            .to_string();
        map.insert(key, Value::String(value));
    }
    Value::Object(map)
}

/// 按扩展名猜 Content-Type（覆盖静态站点常见类型）。
fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "eot" => "application/vnd.ms-fontobject",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "map" => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// 路由解析结果（对齐 Go ParseRoute 的 matchMap["match"]）。
#[derive(Debug, Clone, PartialEq)]
enum RouteMatch {
    Index,
    Archive {
        id: Option<i64>,
        token: Option<String>,
        module: Option<String>,
        catname: Option<String>,
    },
    ArchiveIndex {
        module: String,
        page: u32,
    },
    Category {
        token: String,
        page: u32,
    },
    Page {
        token: String,
    },
    Search {
        module: Option<String>,
    },
    TagIndex {
        page: u32,
    },
    Tag {
        id: Option<u32>,
        token: Option<String>,
        page: u32,
    },
    /// 规则能匹配但功能未移植（会员主页等），明确提示而不是静默 404
    NotImplemented(&'static str),
    NotFound,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::Router;
    use tower::ServiceExt;

    use crate::config::ServerJson;

    fn front_app(state: AppState) -> Router {
        Router::new().fallback(reroute).with_state(state)
    }

    fn test_state(pool: Option<MySqlPool>) -> AppState {
        let template_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../template/default")
            .to_string_lossy()
            .to_string();
        let public_dir = std::env::temp_dir()
            .join(format!("rustcms-front-public-{}", std::process::id()))
            .to_string_lossy()
            .to_string();
        let config = ServerJson {
            template_dir,
            public_dir,
            ..Default::default()
        };
        AppState::new(config, pool)
    }

    async fn get(
        app: Router,
        uri: &str,
    ) -> Result<axum::http::Response<Body>, std::convert::Infallible> {
        app.oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    async fn body_text(res: axum::http::Response<Body>) -> String {
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    }

    #[test]
    fn content_type_by_extension() {
        assert_eq!(content_type("/a/b.png"), "image/png");
        assert_eq!(content_type("/a/b.WEBP"), "image/webp");
        assert_eq!(content_type("/a/b.txt"), "text/plain; charset=utf-8");
        assert_eq!(content_type("/a/unknown"), "application/octet-stream");
    }

    #[test]
    fn parses_query_params() {
        let params = query_params("q=%E6%9C%BA%E6%A2%B0&page=2&flag");
        assert_eq!(params["q"], "机械");
        assert_eq!(params["page"], "2");
        assert_eq!(params["flag"], "");
        assert_eq!(query_page("q=x&page=3"), 3);
        assert_eq!(query_page(""), 1);
    }

    #[test]
    fn detects_spiders() {
        assert!(is_spider("mozilla/5.0 (compatible; baiduspider/2.0)"));
        assert!(is_spider("googlebot"));
        assert!(!is_spider("mozilla/5.0 (windows nt 10.0)"));
    }

    #[tokio::test]
    async fn serves_static_files_safely() {
        let state = test_state(None);
        let base = state.with_config(|c| c.public_dir());
        std::fs::create_dir_all(base.join("css")).unwrap();
        std::fs::write(base.join("css/app.css"), "body{}").unwrap();

        let app = front_app(state.clone());

        // 真实文件
        let res = get(app.clone(), "/css/app.css").await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["content-type"], "text/css; charset=utf-8");
        assert_eq!(body_text(res).await, "body{}");

        // 路径穿越不放行（也不会 panic）
        let res = get(app.clone(), "/css/../../secret.txt").await.unwrap();
        assert_ne!(res.status(), StatusCode::OK);

        // favicon.ico 缺省 400
        let res = get(app.clone(), "/favicon.ico").await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        // robots.txt 缺省内容
        let res = get(app.clone(), "/robots.txt").await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let text = body_text(res).await;
        assert!(text.contains("User-agent: *"), "{text}");
        assert!(text.contains("Disallow: /system"), "{text}");

        // 目录请求不 serve 文件
        let res = get(app, "/css/").await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn shows_message_page() {
        let html = show_message("测试提示");
        assert!(html.contains("测试提示"));
        assert!(html.contains("history.back"));
    }

    #[tokio::test]
    async fn home_and_pages_render_through_pipeline() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let state = test_state(Some(pool.clone()));
        let app = front_app(state);

        // 首页
        let res = get(app.clone(), "/").await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let html = body_text(res).await;
        assert!(html.contains("<!DOCTYPE html>"), "首页没走模板");

        // 栏目页
        let categories = category::list_all(&pool).await.unwrap();
        if let Some(target) = categories
            .iter()
            .find(|item| !item.is_page() && item.status == 1)
        {
            let res = get(app.clone(), &format!("/{}/", target.url_token))
                .await
                .unwrap();
            assert_eq!(
                res.status(),
                StatusCode::OK,
                "栏目页 {} 渲染失败",
                target.url_token
            );
        }

        // 不存在的页面 → 404 且走 404 模板
        let res = get(app.clone(), "/rustcms-not-exists/").await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let html = body_text(res).await;
        assert!(html.contains("404"), "应渲染 404 页：{html}");
    }

    #[tokio::test]
    async fn archive_and_page_routes_render() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let state = test_state(Some(pool.clone()));
        let app = front_app(state);

        // 内容详情：/{catname}/{id}.html（mode 4 空 patten 回落字符串模式 2）
        let row: Option<(i64, u32)> =
            sqlx::query_as("SELECT id, category_id FROM archives ORDER BY id LIMIT 1")
                .fetch_optional(&pool)
                .await
                .unwrap();
        if let Some((id, category_id)) = row {
            if let Some(cat) = category::get(&pool, category_id).await.unwrap() {
                let uri = format!("/{}/{}.html", cat.url_token, id);
                let res = get(app.clone(), &uri).await.unwrap();
                assert_eq!(res.status(), StatusCode::OK, "详情页 {uri} 渲染失败");
            }
            // catname 对不上 → 404
            let uri = format!("/rustcms-wrong-cat/{}.html", id);
            let res = get(app.clone(), &uri).await.unwrap();
            assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri} 应 404");
        }

        // 单页
        let categories = category::list_all(&pool).await.unwrap();
        if let Some(page_cat) = categories.iter().find(|item| item.is_page()) {
            let res = get(app.clone(), &format!("/{}.html", page_cat.url_token))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK, "单页渲染失败");
        }
    }

    #[tokio::test]
    async fn tag_and_search_pages_render() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let state = test_state(Some(pool.clone()));
        let app = front_app(state);

        // 标签列表页
        let res = get(app.clone(), "/tags/").await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "标签列表页渲染失败");

        // 标签详情页（URL 形态取决于伪静态模式：字符串模式用 url_token）
        let Some(item) = tag::get(&pool, 1).await.unwrap() else {
            return;
        };
        let mode = url::load_mode(&pool).await.unwrap();
        let string_mode = matches!(mode, url::MODE_STRING1 | url::MODE_STRING3);
        let uri = if string_mode && !item.url_token.is_empty() {
            format!("/tag/{}", item.url_token)
        } else {
            format!("/tag/{}", item.id)
        };
        let res = get(app.clone(), &uri).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "标签页 {uri} 渲染失败");

        // 搜索页（带关键词）
        let res = get(app.clone(), "/search/?q=%E6%B5%8B%E8%AF%95")
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "搜索页渲染失败");
    }

    #[tokio::test]
    async fn check301_redirects_from_table() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM redirects WHERE from_url LIKE '/rustcms-front%'")
            .execute(&pool)
            .await;
        sqlx::query(
            "INSERT INTO redirects (created_time, updated_time, from_url, to_url) \
             VALUES (UNIX_TIMESTAMP(), UNIX_TIMESTAMP(), '/rustcms-front-old', '/rustcms-front-new')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let state = test_state(Some(pool.clone()));
        let app = front_app(state);

        let res = get(app.clone(), "/rustcms-front-old").await.unwrap();
        assert_eq!(res.status(), StatusCode::MOVED_PERMANENTLY);
        assert_eq!(res.headers()["location"], "/rustcms-front-new");

        // 清理自己的残留数据
        let _ = sqlx::query("DELETE FROM redirects WHERE from_url LIKE '/rustcms-front%'")
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    async fn user_token_resolves_member() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let Some(secret) = auth::token_secret(&pool).await.ok() else {
            return;
        };
        let Some(id): Option<u32> =
            sqlx::query_scalar("SELECT id FROM users WHERE status = 1 ORDER BY id LIMIT 1")
                .fetch_optional(&pool)
                .await
                .unwrap()
        else {
            return;
        };
        let (token, _) = auth::issue(TokenKind::User, &secret, id, false).unwrap();

        let mut headers = HeaderMap::new();
        headers.insert("token", header::HeaderValue::from_str(&token).unwrap());
        let (member, renew) = parse_user_token(&pool, &headers).await.unwrap();
        assert_eq!(member.map(|m| m.id), Some(id));
        assert!(renew.is_none(), "新签发的令牌不应续签");

        // 无 token → 匿名
        let (member, _) = parse_user_token(&pool, &HeaderMap::new()).await.unwrap();
        assert!(member.is_none());
    }
}

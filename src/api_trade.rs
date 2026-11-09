//! 交易 API（批次 C）：购物车 / 订单 / 店铺 / 支付 / 分销。
//!
//! 覆盖购物车、订单、分销、会员资金与支付网关下单 / 回调的既有契约，
//! 路由挂在 `/api` 分组下。
//!
//! # 路由前缀
//!
//! 既有接口的真实路径是 `/api/cart/add`、`/api/order/create` …。
//! 本文件的 `router` 同时注册**相对路径**（`/cart/add`）与
//! **`/api` 前缀路径**（`/api/cart/add`）两套，
//! 这样无论主线是 `.nest("/api", api_trade::router)` 还是
//! `.merge(api_trade::router)`（顶层），都能命中 `/api/...` 路径。
//! 只需要一套时删掉另一套即可，不影响其它批次。
//!
//! # 鉴权
//!
//! 会员接口的登录态沿用 `ParseUserToken` 的令牌口径：
//! 请求头 `token` > `Authorization: Bearer` > Cookie `token`；
//! JWT 失效时回退到会员 API Key（`users.api_token`）。
//!
//! # 响应信封
//!
//! 统一 `{"code":0,"msg":"","data":…}`；分页类接口额外带顶层 `total`。

use std::collections::{BTreeMap, HashMap};

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth;
use crate::models::payment;
use crate::models::trade::{self, CreateOrderInput, DepositInput, OrderAddressInput, PaymentInput,
    RechargeInput, ShopInput, WithdrawInput};
use crate::routes::{fail, failed, ok, ok_msg, NO_DB};
use crate::state::AppState;

/// 本批路由表（见文件头「路由前缀」说明）。
pub fn router() -> Router<AppState> {
    trade_routes("").merge(trade_routes("/api"))
}

/// 生成一批带前缀的路由。
fn trade_routes(prefix: &str) -> Router<AppState> {
    Router::new()
        // 购物车
        .route(&format!("{prefix}/cart/list"), get(cart_list))
        .route(&format!("{prefix}/cart/add"), post(cart_add))
        .route(&format!("{prefix}/cart/update"), post(cart_update))
        .route(&format!("{prefix}/cart/delete"), post(cart_delete))
        .route(&format!("{prefix}/cart/clear"), post(cart_clear))
        .route(&format!("{prefix}/cart/checkout"), post(cart_checkout))
        // 订单
        .route(&format!("{prefix}/order/create"), post(order_create))
        .route(&format!("{prefix}/order/detail"), get(order_detail))
        .route(&format!("{prefix}/order/cancel"), post(order_cancel))
        .route(&format!("{prefix}/order/finish"), post(order_finish))
        .route(&format!("{prefix}/order/payment"), post(order_payment))
        .route(&format!("{prefix}/order/refund"), post(order_refund))
        .route(&format!("{prefix}/order/address"), get(order_address).post(order_address_save))
        .route(&format!("{prefix}/order/addresses"), get(order_addresses))
        .route(&format!("{prefix}/orders"), get(order_list))
        // 支付 / 积分
        .route(&format!("{prefix}/payment/check"), get(payment_check))
        .route(&format!("{prefix}/point/logs"), get(point_logs))
        // 店铺
        .route(&format!("{prefix}/shop/info"), get(shop_info))
        .route(&format!("{prefix}/shop/save"), post(shop_save))
        .route(&format!("{prefix}/shop/reviews"), get(shop_reviews))
        .route(&format!("{prefix}/shop/sales"), get(shop_sales))
        .route(&format!("{prefix}/shop/deposit/logs"), get(shop_deposit_logs))
        .route(&format!("{prefix}/shop/deposit/pay"), post(shop_deposit_pay))
        .route(&format!("{prefix}/shop/deposit/refund"), post(shop_deposit_refund))
        // 分销
        .route(&format!("{prefix}/retailer/info"), get(retailer_info))
        .route(&format!("{prefix}/retailer/orders"), get(retailer_orders))
        .route(&format!("{prefix}/retailer/members"), get(retailer_members))
        .route(&format!("{prefix}/retailer/commissions"), get(retailer_commissions))
        .route(&format!("{prefix}/retailer/statistics"), get(retailer_statistics))
        .route(&format!("{prefix}/retailer/update"), post(retailer_update))
        .route(
            &format!("{prefix}/retailer/withdraw"),
            get(retailer_withdraws).post(retailer_withdraw),
        )
        // 会员资金
        .route(&format!("{prefix}/user/recharge"), post(user_recharge))
        .route(&format!("{prefix}/user/withdraw"), post(user_withdraw))
        // 支付网关回调 / 返回
        .route(&format!("{prefix}/alipay/pay"), post(alipay_notify))
        // PayPal 既是 webhook（POST），也是用户付款后跳回的返回页（GET ?token=），两种都收
        .route(&format!("{prefix}/paypal/pay"), get(paypal_notify).post(paypal_notify))
        .route(&format!("{prefix}/paypal/cancel"), get(paypal_cancel))
        // 微信支付 V2：异步通知 + 主动查单
        .route(&format!("{prefix}/wechat/notify"), post(wechat_notify))
        .route(&format!("{prefix}/wechat/query"), get(wechat_query))
        // 兼容既有实现配置的网关地址（微信那条已被 api_interact 占用，这里不重复注册）：
        // 老站点的支付宝/PayPal 商户后台填的就是 /notify/... 与 /return/...
        .route(&format!("{prefix}/notify/alipay/pay"), post(alipay_notify))
        .route(&format!("{prefix}/notify/paypal/pay"), post(paypal_notify))
        .route(&format!("{prefix}/return/paypal/pay"), get(paypal_notify))
        .route(&format!("{prefix}/return/paypal/cancel"), get(paypal_cancel))
}

// ---------------------------------------------------------------------------
// 公共辅助
// ---------------------------------------------------------------------------

/// 带顶层 `total` 的成功信封（既有实现分页接口的口径）。
fn ok_with_total(total: i64, data: impl serde::Serialize) -> Response {
    Json(json!({ "code": 0, "msg": "", "total": total, "data": data })).into_response()
}

/// 从请求里取会员令牌：请求头 `token` > `Authorization: Bearer` > Cookie `token`。
fn user_token(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers.get("token").and_then(|value| value.to_str().ok()) {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    if let Some(value) = headers.get(header::AUTHORIZATION).and_then(|value| value.to_str().ok()) {
        if let Some(token) = value.strip_prefix("Bearer ") {
            let token = token.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    if let Some(value) = headers.get(header::COOKIE).and_then(|value| value.to_str().ok()) {
        for part in value.split(';') {
            let part = part.trim();
            if let Some(token) = part.strip_prefix("token=") {
                let token = token.trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }
    None
}

/// 取当前登录会员 ID（按 `ParseUserToken` 的语义）。
async fn current_user(state: &AppState, headers: &HeaderMap) -> Result<u32, Response> {
    let Some(pool) = state.db() else {
        return Err(fail(NO_DB));
    };
    let Some(token) = user_token(headers) else {
        return Err(fail("请先登录"));
    };
    let secret = match auth::token_secret(pool).await {
        Ok(secret) => secret,
        Err(err) => return Err(failed(err)),
    };
    if let Ok(claims) = auth::verify(auth::TokenKind::User, &secret, &token) {
        if let Some(id) = claims.subject(auth::TokenKind::User) {
            let status: Option<i8> = sqlx::query_scalar("SELECT status FROM users WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten();
            match status {
                Some(1) => return Ok(id),
                Some(_) => return Err(fail("账号已被禁用")),
                None => return Err(fail("账号不存在")),
            }
        }
    }
    // JWT 无效时回退到会员 API Key（按 `GetUserByApiToken` 的语义）。
    let api_token: Option<u32> =
        sqlx::query_scalar("SELECT id FROM users WHERE api_token = ? AND status = 1 LIMIT 1")
            .bind(&token)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    match api_token {
        Some(id) => Ok(id),
        None => Err(fail("登录已失效，请重新登录")),
    }
}

/// 解析 JSON 请求体（空体 / 非法 JSON 都按默认值处理）。
fn parse_json<T: serde::de::DeserializeOwned + Default>(body: &str) -> Result<T, Response> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Ok(T::default());
    }
    serde_json::from_str::<T>(trimmed).map_err(|err| fail(format!("请求参数格式错误: {err}")))
}

/// 解析 `application/x-www-form-urlencoded` 请求体（支付网关回调用）。
fn parse_form(body: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for part in body.split('&') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (key, value) = match part.split_once('=') {
            Some(pair) => pair,
            None => (part, ""),
        };
        let decode = |raw: &str| {
            percent_encoding::percent_decode_str(&raw.replace('+', " "))
                .decode_utf8_lossy()
                .to_string()
        };
        map.insert(decode(key), decode(value));
    }
    map
}

/// 分页参数（兼容既有的 `page`/`current` 与 `pageSize` 两种写法）。
#[derive(Debug, Default, Deserialize)]
struct PageQuery {
    #[serde(default, alias = "current")]
    page: i64,
    #[serde(default, alias = "pageSize", alias = "page_size")]
    page_size: i64,
}

/// 订单列表查询参数。
#[derive(Debug, Default, Deserialize)]
struct OrderListQuery {
    #[serde(default, alias = "current")]
    page: i64,
    #[serde(default, alias = "pageSize", alias = "page_size")]
    page_size: i64,
    #[serde(default)]
    status: String,
    #[serde(default, rename = "type")]
    kind: String,
}

/// 单号查询参数（订单详情 / 支付检查）。
#[derive(Debug, Default, Deserialize)]
struct OrderIdQuery {
    #[serde(default)]
    order_id: String,
}

/// 购物车操作请求（按 `request.CartRequest` 的语义）。
#[derive(Debug, Default, Deserialize)]
struct CartRequest {
    #[serde(default)]
    archive_id: i64,
    #[serde(default)]
    cart_id: u32,
    #[serde(default)]
    quantity: i32,
}

/// 订单号请求（取消 / 完成 / 退款申请）。
#[derive(Debug, Default, Deserialize)]
struct OrderIdRequest {
    #[serde(default)]
    order_id: String,
}

// ---------------------------------------------------------------------------
// 购物车
// ---------------------------------------------------------------------------

/// `GET /cart/list`
async fn cart_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::cart_list(pool, user_id).await {
        Ok((items, total)) => ok(json!({
            "list": items,
            "total": total,
            "count": items.len(),
        })),
        Err(err) => failed(err),
    }
}

/// `POST /cart/add`
async fn cart_add(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let request: CartRequest = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let cart = match trade::add_cart(pool, user_id, request.archive_id, request.quantity).await {
        Ok(cart) => cart,
        Err(err) => return crate::routes::model_error(err),
    };
    let count = match trade::count_cart(pool, user_id).await {
        Ok(count) => count,
        Err(err) => return failed(err),
    };
    ok(json!({ "cart": cart, "count": count }))
}

/// `POST /cart/update`
async fn cart_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let request: CartRequest = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if let Err(err) = trade::update_cart_quantity(pool, user_id, request.cart_id, request.quantity).await
    {
        return crate::routes::model_error(err);
    }
    match trade::cart_list(pool, user_id).await {
        Ok((items, total)) => ok(json!({ "list": items, "total": total })),
        Err(err) => failed(err),
    }
}

/// `POST /cart/delete`
async fn cart_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let request: CartRequest = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if let Err(err) = trade::delete_cart(pool, user_id, request.cart_id).await {
        return crate::routes::model_error(err);
    }
    match trade::cart_list(pool, user_id).await {
        Ok((items, total)) => ok(json!({ "list": items, "total": total })),
        Err(err) => failed(err),
    }
}

/// `POST /cart/clear`
async fn cart_clear(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::clear_cart(pool, user_id).await {
        Ok(()) => ok(json!({})),
        Err(err) => crate::routes::model_error(err),
    }
}

/// `POST /cart/checkout`
async fn cart_checkout(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let request: CreateOrderInput = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    if request.address.is_none() && request.address_id == 0 {
        return fail("请选择收货地址");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::checkout_cart(pool, user_id, request).await {
        Ok(order) => ok(order),
        Err(err) => crate::routes::model_error(err),
    }
}

// ---------------------------------------------------------------------------
// 订单
// ---------------------------------------------------------------------------

/// `GET /orders`
async fn order_list(
    State(state): State<AppState>,
    Query(query): Query<OrderListQuery>,
    headers: HeaderMap,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::order_list(
        pool,
        user_id,
        &query.status,
        &query.kind,
        query.page,
        query.page_size,
    )
    .await
    {
        Ok((total, rows)) => ok_with_total(total, rows),
        Err(err) => failed(err),
    }
}

/// `POST /order/create`
async fn order_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let request: CreateOrderInput = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    // 允许匿名下单（站点配置 `order.no_need_login` 时）。
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => {
            let Some(pool) = state.db() else {
                return fail(NO_DB);
            };
            let config = crate::models::order::config_get(pool).await.unwrap_or_else(|_| json!({}));
            let allow = config
                .get("no_need_login")
                .map(|value| match value {
                    Value::Bool(flag) => *flag,
                    Value::Number(number) => number.as_i64().unwrap_or(0) != 0,
                    Value::String(text) => text.trim() == "1" || text.trim() == "true",
                    _ => false,
                })
                .unwrap_or(false);
            if !allow {
                return res;
            }
            0
        }
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::create_order(pool, user_id, request).await {
        Ok(order) => ok(order),
        Err(err) => crate::routes::model_error(err),
    }
}

/// `GET /order/detail`
async fn order_detail(
    State(state): State<AppState>,
    Query(query): Query<OrderIdQuery>,
    headers: HeaderMap,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    if query.order_id.trim().is_empty() {
        return fail("订单号不能为空");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let order = match trade::order_detail(pool, query.order_id.trim()).await {
        Ok(Some(order)) => order,
        Ok(None) => return fail("订单不存在"),
        Err(err) => return failed(err),
    };
    if order.order.user_id != user_id {
        return fail("权限不足");
    }
    ok(order)
}

/// `POST /order/cancel`
async fn order_cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let request: OrderIdRequest = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    if request.order_id.trim().is_empty() {
        return fail("订单号不能为空");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::cancel_order(pool, user_id, request.order_id.trim()).await {
        Ok(()) => ok_msg("订单已取消"),
        Err(err) => crate::routes::model_error(err),
    }
}

/// `POST /order/finish`
async fn order_finish(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let request: OrderIdRequest = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    if request.order_id.trim().is_empty() {
        return fail("订单号不能为空");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::finish_order(pool, user_id, request.order_id.trim()).await {
        Ok(()) => ok_msg("订单已完成"),
        Err(err) => crate::routes::model_error(err),
    }
}

/// `POST /order/refund`
async fn order_refund(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let request: OrderIdRequest = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    if request.order_id.trim().is_empty() {
        return fail("订单号不能为空");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::apply_order_refund(pool, user_id, request.order_id.trim()).await {
        Ok(()) => ok_msg("退款申请已提交"),
        Err(err) => crate::routes::model_error(err),
    }
}

/// `POST /order/payment`
async fn order_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let request: PaymentInput = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    if request.order_id.trim().is_empty() {
        return fail("订单号不能为空");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    // 登录态（允许匿名下单的站点可没有登录态）。
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => {
            let config = crate::models::order::config_get(pool).await.unwrap_or_else(|_| json!({}));
            let allow = config
                .get("no_need_login")
                .map(|value| match value {
                    Value::Bool(flag) => *flag,
                    Value::Number(number) => number.as_i64().unwrap_or(0) != 0,
                    Value::String(text) => text.trim() == "1" || text.trim() == "true",
                    _ => false,
                })
                .unwrap_or(false);
            if !allow {
                return res;
            }
            0
        }
    };

    let order = match trade::load_order(pool, request.order_id.trim()).await {
        Ok(Some(order)) => order,
        Ok(None) => return fail("订单不存在"),
        Err(err) => return failed(err),
    };
    // 归属校验：只能支付自己的订单（匿名下单 user_id=0 放行）。
    if user_id != 0 && order.user_id != user_id {
        return fail("权限不足");
    }

    // 支付方式：为空时取第一个可用方式（`GetPaymentWays` 的语义）。
    let mut pay_way = request.pay_way.trim().to_string();
    if pay_way.is_empty() {
        let ways = match trade::payment_ways(pool).await {
            Ok(ways) => ways,
            Err(err) => return failed(err),
        };
        match ways.first() {
            Some(way) => pay_way = way.clone(),
            None => return fail("无法创建支付订单"),
        }
    }

    let payment = match trade::generate_payment(pool, &order, &pay_way).await {
        Ok(payment) => payment,
        Err(err) => return crate::routes::model_error(err),
    };

    if pay_way == trade::PAY_WAY_BALANCE {
        if let Err(err) = trade::balance_pay(pool, &order, &payment).await {
            return crate::routes::model_error(err);
        }
        if let Err(err) = trade::success_paid_order(pool, &order, &payment).await {
            return crate::routes::model_error(err);
        }
        return ok(json!({
            "payment_id": payment.payment_id,
            "order_id": order.order_id,
            "status": 1,
        }));
    }

    // 其余支付方式走真实网关（微信 Native/JSAPI、支付宝电脑网站、PayPal）。
    match pay_way.as_str() {
        trade::PAY_WAY_WECHAT => wechat_create(pool, &payment, &headers).await,
        trade::PAY_WAY_WEAPP => weapp_create(pool, &payment, &headers).await,
        trade::PAY_WAY_ALIPAY => alipay_create(&state, pool, &payment).await,
        trade::PAY_WAY_PAYPAL => paypal_create(pool, &payment).await,
        trade::PAY_WAY_OFFLINE => fail("线下支付请在订单备注里说明，由商家确认收款"),
        other => fail(format!("不支持的支付方式「{other}」")),
    }
}

// ---------------------------------------------------------------------------
// 支付网关下单（微信 / 小程序 / 支付宝 / PayPal）
// ---------------------------------------------------------------------------

/// 站点地址：`(base_url, front_url)`，都以 `/` 结尾（拼网关回调地址用）。
async fn site_urls(pool: &sqlx::mysql::MySqlPool) -> (String, String) {
    let urls = crate::models::front_content::site_urls(pool)
        .await
        .unwrap_or_default();
    let join = |url: &str| {
        let trimmed = url.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            String::new()
        } else {
            format!("{trimmed}/")
        }
    };
    (join(&urls.base_url), join(&urls.front_url))
}

/// 客户端 IP：优先 `X-Forwarded-For` / `X-Real-IP`（站点一般挂在 nginx 后面），
/// 取不到退回 `127.0.0.1`（取不到对端地址时的兜底值）。
fn client_ip(headers: &HeaderMap) -> String {
    for name in ["x-forwarded-for", "x-real-ip"] {
        if let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) {
            let first = value.split(',').next().unwrap_or("").trim();
            if !first.is_empty() {
                return first.to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}

/// 微信 Native 扫码支付下单（`createWechatPayment` 的语义）。
///
/// 返回 `data.code_url`：微信原生 `weixin://wxpay/bizpayurl?...` 链接。
/// 既有实现把它渲染成 base64 PNG（`data.image/png`）；本项目没有二维码
/// 库（不引新依赖），因此直接下发原始链接，由前端用 JS 二维码组件渲染。
async fn wechat_create(
    pool: &sqlx::mysql::MySqlPool,
    payment: &trade::Payment,
    headers: &HeaderMap,
) -> Response {
    let account = match payment::valid_account(pool, trade::PAY_WAY_WECHAT, payment.amount).await {
        Ok(Some(account)) => account,
        Ok(None) => return fail("没有可用的微信支付账户，请先在后台配置"),
        Err(err) => return failed(err),
    };
    let config = payment::pay_config_json(&account);
    let cfg = match payment::wechat::WechatConfig::from_json(&config) {
        Ok(cfg) => cfg,
        Err(err) => return fail(err),
    };
    let (base, _) = site_urls(pool).await;
    if base.is_empty() {
        return fail("请在后台「站点设置」里填写站点地址（base_url）后再支付");
    }
    let notify_url = format!("{base}wechat/notify");
    let subject = payment
        .remark
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    let result = match payment::wechat::unified_order_native(
        &cfg,
        &payment.payment_id,
        payment.amount,
        &subject,
        &notify_url,
        &client_ip(headers),
    )
    .await
    {
        Ok(result) => result,
        Err(err) => return fail(err),
    };
    if let Err(err) = payment::set_payment_account(pool, payment.id, account.id).await {
        return failed(err);
    }
    ok(json!({
        "pay_way": trade::PAY_WAY_WECHAT,
        "payment_id": payment.payment_id,
        "order_id": payment.order_id,
        "code_url": result.code_url,
        "prepay_id": result.prepay_id,
    }))
}

/// 小程序（JSAPI）支付下单（`createWeappPayment` 的语义）。
async fn weapp_create(
    pool: &sqlx::mysql::MySqlPool,
    payment: &trade::Payment,
    headers: &HeaderMap,
) -> Response {
    let account = match payment::valid_account(pool, trade::PAY_WAY_WEAPP, payment.amount).await {
        Ok(Some(account)) => account,
        Ok(None) => return fail("没有可用的小程序支付账户，请先在后台配置"),
        Err(err) => return failed(err),
    };
    let openid: Option<String> =
        match sqlx::query_scalar("SELECT openid FROM user_wechats WHERE user_id = ? ORDER BY id DESC LIMIT 1")
            .bind(payment.user_id)
            .fetch_optional(pool)
            .await
        {
            Ok(value) => value,
            Err(err) => return failed(err.into()),
        };
    let Some(openid) = openid.filter(|value| !value.trim().is_empty()) else {
        return fail("该会员还没有绑定小程序 openid，无法使用小程序支付");
    };
    let config = payment::pay_config_json(&account);
    let cfg = match payment::wechat::WechatConfig::from_json(&config) {
        Ok(cfg) => cfg,
        Err(err) => return fail(err),
    };
    let (base, _) = site_urls(pool).await;
    if base.is_empty() {
        return fail("请在后台「站点设置」里填写站点地址（base_url）后再支付");
    }
    let notify_url = format!("{base}wechat/notify");
    let subject = payment
        .remark
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    let params = match payment::wechat::unified_order_jsapi(
        &cfg,
        &payment.payment_id,
        payment.amount,
        &subject,
        &notify_url,
        &client_ip(headers),
        &openid,
    )
    .await
    {
        Ok(params) => params,
        Err(err) => return fail(err),
    };
    if let Err(err) = payment::set_payment_account(pool, payment.id, account.id).await {
        return failed(err);
    }
    ok(json!({
        "pay_way": trade::PAY_WAY_WEAPP,
        "payment_id": payment.payment_id,
        "order_id": payment.order_id,
        "appId": cfg.app_id,
        "timeStamp": params.time_stamp,
        "nonceStr": params.nonce_str,
        "package": params.package,
        "signType": params.sign_type,
        "paySign": params.pay_sign,
    }))
}

/// 读取支付账户记录（网关回调里按 `payment.payment_account_id` 取配置）。
async fn payment_account(
    pool: &sqlx::mysql::MySqlPool,
    account_id: i32,
    pay_way: &str,
) -> Result<payment::PaymentAccount, String> {
    if account_id > 0 {
        if let Some(account) = payment::get(pool, account_id as i64)
            .await
            .map_err(|err| format!("读取支付账户失败：{err}"))?
        {
            return Ok(account);
        }
    }
    // 回退到该方式下第一个启用的账户（回调里账户被删也能救回来）
    if let Some(account) = payment::valid_account(pool, pay_way, 0)
        .await
        .map_err(|err| format!("读取支付账户失败：{err}"))?
    {
        return Ok(account);
    }
    Err(format!("没有可用的 {pay_way} 支付账户"))
}

/// 支付宝电脑网站支付下单（`createAlipayPayment` 的语义）。
///
/// 返回 `data.jump_url`：带签名的一手跳转地址（用户直接在浏览器打开）。
async fn alipay_create(
    state: &AppState,
    pool: &sqlx::mysql::MySqlPool,
    payment: &trade::Payment,
) -> Response {
    let account = match payment::valid_account(pool, trade::PAY_WAY_ALIPAY, payment.amount).await {
        Ok(Some(account)) => account,
        Ok(None) => return fail("没有可用的支付宝支付账户，请先在后台配置"),
        Err(err) => return failed(err),
    };
    let config = payment::pay_config_json(&account);
    // 支付宝公钥在证书模式下来自 `data/cert/<public_cert_path>`（与既有约定一致路径）；
    // 没配 public_cert_path 时退回 cert_path（模型注释里 cert_path 也标过「应用公钥」）。
    let cert_dir = state.with_config(|config| config.data_dir()).join("cert");
    let public_key = alipay_public_key(&cert_dir, &config).await;
    let cfg = match payment::alipay::AlipayConfig::from_json(&config, &public_key) {
        Ok(cfg) => cfg,
        Err(err) => return fail(err),
    };
    let (base, front) = site_urls(pool).await;
    if base.is_empty() {
        return fail("请在后台「站点设置」里填写站点地址（base_url）后再支付");
    }
    let notify_url = format!("{base}alipay/pay");
    let return_url = if front.is_empty() {
        base.clone()
    } else {
        front
    };
    let subject = payment
        .remark
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    let jump_url = match payment::alipay::page_pay_url(
        &cfg,
        &payment.payment_id,
        payment.amount,
        &subject,
        &notify_url,
        &return_url,
        &payment::alipay::timestamp_now(),
    ) {
        Ok(url) => url,
        Err(err) => return fail(err),
    };
    if let Err(err) = payment::set_payment_account(pool, payment.id, account.id).await {
        return failed(err);
    }
    ok(json!({
        "pay_way": trade::PAY_WAY_ALIPAY,
        "payment_id": payment.payment_id,
        "order_id": payment.order_id,
        "jump_url": jump_url,
    }))
}

/// 读支付宝公钥证书内容（`<data>/cert/<文件名>`）。
async fn alipay_public_key(dir: &std::path::Path, config: &Value) -> String {
    let read_path = |key: &str| {
        config
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let candidates = [read_path("public_cert_path"), read_path("cert_path")];
    for name in candidates {
        if name.is_empty() {
            continue;
        }
        let path = dir.join(&name);
        match tokio::fs::read_to_string(&path).await {
            Ok(content) if !content.trim().is_empty() => return content,
            Ok(_) => continue,
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "读取支付宝公钥证书失败");
            }
        }
    }
    String::new()
}

/// `GET /order/addresses`
async fn order_addresses(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::order_addresses(pool, user_id).await {
        Ok(list) => ok(list),
        Err(err) => failed(err),
    }
}

/// `GET /order/address`
async fn order_address(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::order_address(pool, user_id).await {
        Ok(Some(address)) => ok(address),
        Ok(None) => fail("收货地址不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /order/address`
async fn order_address_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let request: OrderAddressInput = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::save_order_address(pool, user_id, &request).await {
        Ok(address) => ok(address),
        Err(err) => crate::routes::model_error(err),
    }
}

/// `GET /payment/check`
async fn payment_check(
    State(state): State<AppState>,
    Query(query): Query<OrderIdQuery>,
    headers: HeaderMap,
) -> Response {
    let _ = current_user(&state, &headers).await.ok();
    if query.order_id.trim().is_empty() {
        return fail("订单号不能为空");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let order = match trade::load_order(pool, query.order_id.trim()).await {
        Ok(Some(order)) => order,
        Ok(None) => return fail("订单不存在"),
        Err(err) => return failed(err),
    };
    if order.status != trade::ORDER_STATUS_WAITING {
        return ok_msg("支付成功");
    }
    let payment = match trade::payment_by_order_id(pool, &order.order_id).await {
        Ok(Some(payment)) => payment,
        Ok(None) => return fail("未支付"),
        Err(err) => return failed(err),
    };
    if payment.paid_time > 0 {
        return ok_msg("支付成功");
    }
    // 既有实现会轮询 6 秒等待异步回调，这里保持同样口径。
    for _ in 0..6 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        match trade::load_order(pool, &order.order_id).await {
            Ok(Some(current)) if current.status != trade::ORDER_STATUS_WAITING => {
                return ok_msg("支付成功")
            }
            Ok(_) => {}
            Err(err) => return failed(err),
        }
    }
    fail("未支付")
}

/// `GET /point/logs`
async fn point_logs(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
    headers: HeaderMap,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::point_logs(pool, user_id, query.page, query.page_size).await {
        Ok((total, rows)) => ok(json!({ "list": rows, "total": total })),
        Err(err) => failed(err),
    }
}

// ---------------------------------------------------------------------------
// 店铺 / 保证金
// ---------------------------------------------------------------------------

/// 已售订单查询参数。
#[derive(Debug, Default, Deserialize)]
struct SalesQuery {
    #[serde(default, alias = "current")]
    page: i64,
    #[serde(default, alias = "pageSize", alias = "page_size")]
    page_size: i64,
    #[serde(default)]
    status: String,
}

/// `GET /shop/info`
async fn shop_info(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let shop = match trade::ensure_shop(pool, user_id).await {
        Ok(shop) => shop,
        Err(err) => return crate::routes::model_error(err),
    };
    let decoration = trade::shop_decoration(&shop);
    let stats = match trade::seller_stats(pool, user_id).await {
        Ok(stats) => stats,
        Err(err) => return failed(err),
    };
    ok(json!({
        "shop": shop,
        "decoration": decoration,
        "order_count": stats.order_count,
        "sale_amount": stats.sale_amount,
        "pending": stats.pending,
        "today_amount": stats.today_amount,
    }))
}

/// `POST /shop/save`
async fn shop_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let request: ShopInput = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::save_shop(pool, user_id, &request).await {
        Ok(shop) => ok(json!({ "shop": shop })),
        Err(err) => crate::routes::model_error(err),
    }
}

/// `GET /shop/reviews`
async fn shop_reviews(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
    headers: HeaderMap,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::received_comments(pool, user_id, query.page, query.page_size).await {
        Ok((total, rows)) => ok(json!({ "list": rows, "total": total })),
        Err(err) => failed(err),
    }
}

/// `GET /shop/sales`
async fn shop_sales(
    State(state): State<AppState>,
    Query(query): Query<SalesQuery>,
    headers: HeaderMap,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::seller_orders(
        pool,
        user_id,
        &query.status,
        query.page,
        query.page_size,
    )
    .await
    {
        Ok((total, rows)) => ok(json!({ "list": rows, "total": total })),
        Err(err) => failed(err),
    }
}

/// `GET /shop/deposit/logs`
async fn shop_deposit_logs(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
    headers: HeaderMap,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::deposit_logs(pool, user_id, query.page, query.page_size).await {
        Ok((total, rows)) => ok(json!({ "list": rows, "total": total })),
        Err(err) => failed(err),
    }
}

/// `POST /shop/deposit/pay`
async fn shop_deposit_pay(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let request: DepositInput = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::pay_deposit(pool, user_id, request.amount).await {
        Ok(shop) => ok(json!({ "shop": shop })),
        Err(err) => crate::routes::model_error(err),
    }
}

/// `POST /shop/deposit/refund`
async fn shop_deposit_refund(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let request: DepositInput = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::refund_deposit(pool, user_id, request.amount).await {
        Ok(shop) => ok(json!({ "shop": shop })),
        Err(err) => crate::routes::model_error(err),
    }
}

// ---------------------------------------------------------------------------
// 分销
// ---------------------------------------------------------------------------

/// 分销员查询参数。
#[derive(Debug, Default, Deserialize)]
struct RetailerQuery {
    #[serde(default)]
    retailer_id: u32,
}

/// 分销员资料更新请求。
#[derive(Debug, Default, Deserialize)]
struct RetailerUpdateRequest {
    #[serde(default)]
    real_name: String,
}

/// `GET /retailer/info`
async fn retailer_info(
    State(state): State<AppState>,
    Query(query): Query<RetailerQuery>,
    headers: HeaderMap,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let retailer_id = if query.retailer_id > 0 {
        query.retailer_id
    } else {
        user_id
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::retailer_info(pool, retailer_id).await {
        Ok(Some(user)) => ok(user),
        Ok(None) => fail("查询失败"),
        Err(err) => failed(err),
    }
}

/// `GET /retailer/statistics`
async fn retailer_statistics(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::retailer_statistics(pool, user_id).await {
        Ok(stats) => ok(json!({
            "can_withdraw_amount": stats.can_withdraw_amount,
            "paid_withdraw_amount": stats.paid_withdraw_amount,
            "unfinished_withdraw_amount": stats.unfinished_withdraw_amount,
            "member_count": stats.member_count,
        })),
        Err(err) => failed(err),
    }
}

/// `POST /retailer/update`
async fn retailer_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let request: RetailerUpdateRequest = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    if request.real_name.trim().is_empty() {
        return fail("姓名不能为空");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::update_real_name(pool, user_id, &request.real_name).await {
        Ok(()) => ok_msg("保存成功"),
        Err(err) => crate::routes::model_error(err),
    }
}

/// `GET /retailer/orders`
async fn retailer_orders(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
    headers: HeaderMap,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::retailer_orders(pool, user_id, query.page, query.page_size).await {
        Ok((total, rows)) => ok_with_total(total, rows),
        Err(err) => failed(err),
    }
}

/// `GET /retailer/members`
async fn retailer_members(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
    headers: HeaderMap,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::retailer_members(pool, user_id, query.page, query.page_size).await {
        Ok((total, rows)) => ok_with_total(total, rows),
        Err(err) => failed(err),
    }
}

/// `GET /retailer/commissions`
async fn retailer_commissions(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
    headers: HeaderMap,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::retailer_commissions(pool, user_id, query.page, query.page_size).await {
        Ok((total, rows)) => ok_with_total(total, rows),
        Err(err) => failed(err),
    }
}

/// `GET /retailer/withdraw`
async fn retailer_withdraws(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
    headers: HeaderMap,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::retailer_withdraws(pool, user_id, query.page, query.page_size).await {
        Ok((total, rows)) => ok_with_total(total, rows),
        Err(err) => failed(err),
    }
}

/// `POST /retailer/withdraw`
async fn retailer_withdraw(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::retailer_apply_withdraw(pool, user_id).await {
        Ok(_) => ok_msg("提现申请已提交"),
        Err(err) => crate::routes::model_error(err),
    }
}

// ---------------------------------------------------------------------------
// 会员资金
// ---------------------------------------------------------------------------

/// `POST /user/recharge`
async fn user_recharge(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let request: RechargeInput = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::apply_recharge(pool, user_id, &request).await {
        Ok(recharge) => ok(recharge),
        Err(err) => crate::routes::model_error(err),
    }
}

/// `POST /user/withdraw`
async fn user_withdraw(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let user_id = match current_user(&state, &headers).await {
        Ok(id) => id,
        Err(res) => return res,
    };
    let request: WithdrawInput = match parse_json(&body) {
        Ok(value) => value,
        Err(res) => return res,
    };
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::apply_balance_withdraw(pool, user_id, &request).await {
        Ok(withdraw) => ok(withdraw),
        Err(err) => crate::routes::model_error(err),
    }
}

// ---------------------------------------------------------------------------
// 支付网关回调 / 返回
// ---------------------------------------------------------------------------

/// 纯文本应答（支付宝要求回 `success` / `fail`，不是 JSON）。
fn text_reply(body: &'static str) -> Response {
    (
        axum::http::StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body,
    )
        .into_response()
}

/// 微信支付回调应答（`text/xml`）。
fn wechat_reply(success: bool, message: &str) -> Response {
    (
        axum::http::StatusCode::OK,
        [(header::CONTENT_TYPE, "text/xml; charset=utf-8")],
        payment::wechat::reply_xml(success, message),
    )
        .into_response()
}

/// `POST /alipay/pay`：支付宝异步通知（按 `NotifyAlipay` 的语义）。
///
/// 逐条遵循接口契约：
/// 1. 解析表单 → 按 `out_trade_no`（= 支付单号）查支付单，查不到回 `fail`；
/// 2. 支付单已支付 → 直接回 `success`（网关会重试，必须幂等）；
/// 3. 取支付账户 → 读支付宝公钥（`data/cert/<public_cert_path>`）→ 验签
/// （`sign_type=RSA2` 走 SHA256withRSA，`MD5` 走字典序 MD5）；
/// 4. 订单已支付 → 回 `success`（既有的注释是「这个 payment 需要退款」，这里同样不重复收尾）；
/// 5. 退款通知（带 `refund_id`）→ 更新 `order_refunds` 状态与时间；
/// 6. 支付通知 → `payment::settle_gateway_payment`（payments / orders / finances /
/// `success_paid_order` / 账户统计）；
/// 7. 全部成功回纯文本 `success`，失败回 `fail`。
async fn alipay_notify(State(state): State<AppState>, body: String) -> Response {
    let form = parse_form(&body);
    let params: BTreeMap<String, String> = form.into_iter().collect();
    let out_trade_no = params
        .get("out_trade_no")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if out_trade_no.is_empty() {
        return text_reply("fail");
    }
    let Some(pool) = state.db() else {
        return text_reply("fail");
    };
    let payment = match trade::payment_by_id(pool, &out_trade_no).await {
        Ok(Some(payment)) => payment,
        Ok(None) => {
            tracing::warn!(out_trade_no, "支付宝回调：支付单不存在");
            // 支付宝只认 `success` 为成功，其它文本一律当失败重试；这里给出可读原因便于排查
            return text_reply("支付单不存在");
        }
        Err(err) => {
            tracing::error!(error = %err, "支付宝回调：查询支付单失败");
            return text_reply("fail");
        }
    };
    if payment.paid_time > 0 {
        return text_reply("success");
    }

    // 账户 → 公钥 → 验签
    let account = match payment_account(pool, payment.payment_account_id, trade::PAY_WAY_ALIPAY).await {
        Ok(account) => account,
        Err(err) => {
            tracing::warn!(error = %err, "支付宝回调：支付账户不可用");
            return text_reply("fail");
        }
    };
    let config = payment::pay_config_json(&account);
    let cert_dir = state.with_config(|config| config.data_dir()).join("cert");
    let public_key = alipay_public_key(&cert_dir, &config).await;
    let cfg = match payment::alipay::AlipayConfig::from_json(&config, &public_key) {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(error = %err, "支付宝回调：账户配置无效");
            return text_reply("fail");
        }
    };
    if let Err(err) = payment::alipay::verify_sign(&params, &cfg) {
        tracing::warn!(error = %err, "支付宝回调验签失败");
        return text_reply("fail");
    }

    let order = match trade::load_order(pool, &payment.order_id).await {
        Ok(Some(order)) => order,
        Ok(None) => {
            tracing::warn!(order_id = %payment.order_id, "支付宝回调：订单不存在");
            return text_reply("fail");
        }
        Err(err) => {
            tracing::error!(error = %err, "支付宝回调：查询订单失败");
            return text_reply("fail");
        }
    };
    if order.paid_time > 0 {
        // 与既有约定一致：订单已支付则不再处理（重复支付需要人工退款）
        return text_reply("success");
    }

    if payment::alipay::is_refund_notice(&params) {
        // 退款通知：只落 order_refunds 的状态与时间（既有实现改的是 refund.Status/RefundTime），
        // 退款流水与库存回补由后台退款审核流程负责，回调不重复执行。
        let refund_status = params
            .get("refund_status")
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        let status: i8 = if payment::alipay::refund_succeeded(&params) {
            1
        } else {
            -1
        };
        let ts = trade::now();
        let _ = sqlx::query(
            "UPDATE order_refunds SET status = ?, remark = ?, refund_time = ?, updated_time = ? \
             WHERE order_id = ?",
        )
        .bind(status)
        .bind(&refund_status)
        .bind(ts)
        .bind(ts)
        .bind(&payment.order_id)
        .execute(pool)
        .await;
        return text_reply("success");
    }

    let paid = payment::GatewayPaid {
        pay_way: trade::PAY_WAY_ALIPAY,
        terrace_id: params.get("trade_no").map(String::as_str).unwrap_or(""),
        buyer_id: params
            .get("buyer_open_id")
            .filter(|value| !value.trim().is_empty())
            .or_else(|| params.get("buyer_id"))
            .map(String::as_str)
            .unwrap_or(""),
        buyer_info: params
            .get("buyer_logon_id")
            .map(String::as_str)
            .unwrap_or(""),
    };
    match payment::settle_gateway_payment(pool, &payment, paid).await {
        Ok(()) => text_reply("success"),
        Err(err) => {
            tracing::error!(error = %err, "支付宝回调：支付收尾失败");
            text_reply("fail")
        }
    }
}

/// `POST /wechat/notify`：微信支付 V2 异步通知（按 `NotifyWechatPay` 的语义）。
///
/// 说明：既有实现把微信回调挂在 `/notify/wechat/pay`。本项目 `api_interact` 已经占用了
/// 那个路径（且只做了简化处理），这里的实现挂在 `/wechat/notify`，下单时的 `notify_url`
/// 也指向这里，避免两条实现互相覆盖。
///
/// 流程：解析 XML → 查支付单 → 按账户 api_key 验签 → 幂等判断 → 金额核对 →
/// 退款通知只更 `order_refunds` → 支付通知走 `settle_gateway_payment`。
pub(crate) async fn wechat_notify(State(state): State<AppState>, body: String) -> Response {
    let fields = payment::wechat::parse_xml(&body);
    if fields.is_empty() {
        return wechat_reply(false, "报文为空或无法解析");
    }
    if fields.get("return_code").map(String::as_str) != Some("SUCCESS") {
        // 微信也会推「支付失败」的通知；既有实现先查支付单再验签，这里同样往下走
        tracing::info!(msg = fields.get("return_msg").map(String::as_str).unwrap_or(""), "微信回调：非成功通知");
    }
    let out_trade_no = fields
        .get("out_trade_no")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if out_trade_no.is_empty() {
        return wechat_reply(false, "缺少 out_trade_no");
    }
    let Some(pool) = state.db() else {
        return wechat_reply(false, "数据库不可用");
    };
    let payment = match trade::payment_by_id(pool, &out_trade_no).await {
        Ok(Some(payment)) => payment,
        Ok(None) => {
            tracing::warn!(out_trade_no, "微信回调：支付单不存在");
            return wechat_reply(false, "支付单不存在");
        }
        Err(err) => {
            tracing::error!(error = %err, "微信回调：查询支付单失败");
            return wechat_reply(false, "查询支付单失败");
        }
    };
    let account =
        match payment_account(pool, payment.payment_account_id, trade::PAY_WAY_WECHAT).await {
            Ok(account) => account,
            Err(err) => {
                tracing::warn!(error = %err, "微信回调：支付账户不可用");
                return wechat_reply(false, "支付账户不可用");
            }
        };
    let config = payment::pay_config_json(&account);
    let cfg = match payment::wechat::WechatConfig::from_json(&config) {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(error = %err, "微信回调：账户配置无效");
            return wechat_reply(false, "支付账户配置无效");
        }
    };
    // 沙盒环境用沙盒密钥验签（非沙盒直接返回 api_key）
    let verify_key = match payment::wechat::sandbox_sign_key(&cfg).await {
        Ok(key) => key,
        Err(err) => {
            tracing::warn!(error = %err, "微信回调：取签名密钥失败");
            return wechat_reply(false, "签名密钥不可用");
        }
    };
    if let Err(err) = payment::wechat::verify_sign(&fields, &verify_key) {
        tracing::warn!(error = %err, "微信回调验签失败");
        return wechat_reply(false, "签名校验失败");
    }
    if payment.paid_time > 0 {
        return wechat_reply(true, "OK");
    }

    // 金额核对（既有实现没做，这里补上：能挡住伪造/串单的通知，正常通知不受影响）
    let total_fee = fields
        .get("total_fee")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(-1);
    if !payment::amount_matches(payment.amount, total_fee) {
        tracing::warn!(
            out_trade_no,
            expect = payment.amount,
            got = total_fee,
            "微信回调金额与支付单不一致"
        );
        return wechat_reply(false, "金额不一致");
    }

    let order = match trade::load_order(pool, &payment.order_id).await {
        Ok(Some(order)) => order,
        Ok(None) => return wechat_reply(false, "订单不存在"),
        Err(err) => {
            tracing::error!(error = %err, "微信回调：查询订单失败");
            return wechat_reply(false, "查询订单失败");
        }
    };
    if order.paid_time > 0 {
        return wechat_reply(true, "OK");
    }

    let refund_id = fields
        .get("refund_id")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if !refund_id.is_empty() {
        let refund_status = fields
            .get("refund_status")
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        let status: i8 = if refund_status == "SUCCESS" { 1 } else { -1 };
        let ts = trade::now();
        let _ = sqlx::query(
            "UPDATE order_refunds SET status = ?, remark = ?, refund_time = ?, updated_time = ? \
             WHERE order_id = ?",
        )
        .bind(status)
        .bind(&refund_status)
        .bind(ts)
        .bind(ts)
        .bind(&payment.order_id)
        .execute(pool)
        .await;
        return wechat_reply(true, "OK");
    }

    let paid = payment::GatewayPaid {
        pay_way: trade::PAY_WAY_WECHAT,
        terrace_id: fields
            .get("transaction_id")
            .map(String::as_str)
            .unwrap_or(""),
        buyer_id: fields.get("openid").map(String::as_str).unwrap_or(""),
        buyer_info: "",
    };
    match payment::settle_gateway_payment(pool, &payment, paid).await {
        Ok(()) => wechat_reply(true, "OK"),
        Err(err) => {
            tracing::error!(error = %err, "微信回调：支付收尾失败");
            wechat_reply(false, "支付收尾失败")
        }
    }
}

/// `GET /wechat/query?order_id=…`：微信主动查单（gopay `QueryOrder`）。
///
/// 回调丢失 / 用户支付后直接关页时的兜底：查单返回 `trade_state=SUCCESS` 且支付单
/// 还没落库的，按与回调完全相同的口径收尾（`settle_gateway_payment`）。
async fn wechat_query(
    State(state): State<AppState>,
    Query(query): Query<OrderIdQuery>,
    headers: HeaderMap,
) -> Response {
    let _ = current_user(&state, &headers).await.ok();
    if query.order_id.trim().is_empty() {
        return fail("订单号不能为空");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let order = match trade::load_order(pool, query.order_id.trim()).await {
        Ok(Some(order)) => order,
        Ok(None) => return fail("订单不存在"),
        Err(err) => return failed(err),
    };
    let payment = match trade::payment_by_order_id(pool, &order.order_id).await {
        Ok(Some(payment)) => payment,
        Ok(None) => return fail("支付单不存在"),
        Err(err) => return failed(err),
    };
    if payment.paid_time > 0 {
        return ok(json!({
            "pay_way": payment.pay_way,
            "trade_state": "SUCCESS",
            "paid": true,
        }));
    }
    let account =
        match payment_account(pool, payment.payment_account_id, trade::PAY_WAY_WECHAT).await {
            Ok(account) => account,
            Err(err) => return fail(err),
        };
    let config = payment::pay_config_json(&account);
    let cfg = match payment::wechat::WechatConfig::from_json(&config) {
        Ok(cfg) => cfg,
        Err(err) => return fail(err),
    };
    let result = match payment::wechat::order_query(&cfg, &payment.payment_id).await {
        Ok(result) => result,
        Err(err) => return fail(err),
    };
    if result.trade_state != "SUCCESS" {
        return ok(json!({
            "pay_way": trade::PAY_WAY_WECHAT,
            "trade_state": result.trade_state,
            "paid": false,
        }));
    }
    if result.total_fee > 0 && !payment::amount_matches(payment.amount, result.total_fee) {
        return fail("查单金额与支付单不一致");
    }
    let paid = payment::GatewayPaid {
        pay_way: trade::PAY_WAY_WECHAT,
        terrace_id: &result.transaction_id,
        buyer_id: &result.openid,
        buyer_info: "",
    };
    if let Err(err) = payment::settle_gateway_payment(pool, &payment, paid).await {
        return crate::routes::model_error(err);
    }
    ok(json!({
        "pay_way": trade::PAY_WAY_WECHAT,
        "trade_state": "SUCCESS",
        "paid": true,
        "transaction_id": result.transaction_id,
    }))
}

/// `GET|POST /paypal/pay`：PayPal 返回页 / Webhook 通知。
///
/// 既有实现分成两条路：`/return/paypal/pay`（用户付款后浏览器跳回，`?token=` 是
/// PayPal 订单号，走 `TraceQuery`）与 `/notify/paypal/pay`（Webhook）。这里合并成一个
/// 处理函数，两种入口都收：`token` 既可能是支付单号，也可能是 PayPal 订单号
/// （下单时写进了 `payments.terrace_id`），查询时两者都试。
///
/// 已支付直接返回成功；未支付则换 OAuth token → `capture` → 成功后按既有约定收尾。
async fn paypal_notify(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
    body: String,
) -> Response {
    let mut token = q
        .get("token")
        .map(String::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if token.is_empty() {
        // 兼容旧调用把 token 放在表单里的情况
        let form = parse_form(&body);
        token = form.get("token").cloned().unwrap_or_default();
    }
    if token.trim().is_empty() {
        return fail("支付参数错误");
    }
    paypal_capture_payment(&state, token.trim()).await
}

/// 按支付单号（或 PayPal 订单号）执行 capture 并落库。
///
/// 既有契约的取参口径：返回页按 `token`（PayPal 订单号）查支付单，
/// 回调侧直接按 capture 结果落库。
/// PayPal 返回页带的 `token` 是**它自己的订单号**（存在 `payments.terrace_id`），
/// 所以这里先按支付单号查，查不到再按 `terrace_id` 查。
async fn paypal_capture_payment(state: &AppState, token: &str) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let payment = match trade::payment_by_id(pool, token).await {
        Ok(Some(payment)) => payment,
        Ok(None) => match trade::payment_by_terrace_id(pool, token).await {
            Ok(Some(payment)) => payment,
            Ok(None) => return fail("支付单不存在"),
            Err(err) => return failed(err),
        },
        Err(err) => return failed(err),
    };
    if payment.paid_time > 0 {
        return ok_msg("success");
    }
    if payment.terrace_id.trim().is_empty() {
        return fail("支付单缺少 PayPal 订单号");
    }

    let (client_id, client_secret, sandbox) =
        match paypal_account(pool, payment.payment_account_id).await {
            Ok(account) => account,
            Err(err) => return fail(err),
        };
    let access_token = match paypal_access_token(&client_id, &client_secret, sandbox).await {
        Ok(token) => token,
        Err(err) => return fail(err),
    };
    let (status, response) =
        match paypal_capture(&access_token, sandbox, payment.terrace_id.trim()).await {
            Ok(value) => value,
            Err(err) => return fail(err),
        };
    if status != "COMPLETED" {
        return fail(format!("PayPal 订单未完成：{status}"));
    }

    let buyer_id = response
        .get("payer")
        .and_then(|value| value.get("payer_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let buyer_info = response
        .get("payment_source")
        .map(|value| value.to_string())
        .unwrap_or_default();
    let paid = payment::GatewayPaid {
        pay_way: trade::PAY_WAY_PAYPAL,
        terrace_id: payment.terrace_id.trim(),
        buyer_id: &buyer_id,
        buyer_info: &buyer_info,
    };
    match payment::settle_gateway_payment(pool, &payment, paid).await {
        Ok(()) => ok_msg("success"),
        Err(err) => crate::routes::model_error(err),
    }
}

/// PayPal 下单（按 `createPaypalPayment` 的语义）。
///
/// `intent=CAPTURE` + `purchase_units[0].reference_id = 支付单号`，
/// 拿到 `id` 后写回 `payments.terrace_id`（回调/返回页靠它找回支付单），
/// 并把 `links` 里的付款链接作为 `jump_url` 返回（既有实现取的是 `Links[1].Href`，
/// 这里优先找 `rel=payer-action`，其次 `approve`，都没有才退回下标 1）。
async fn paypal_create(pool: &sqlx::mysql::MySqlPool, payment: &trade::Payment) -> Response {
    let account = match payment::valid_account(pool, trade::PAY_WAY_PAYPAL, payment.amount).await {
        Ok(Some(account)) => account,
        Ok(None) => return fail("没有可用的 PayPal 支付账户，请先在后台配置"),
        Err(err) => return failed(err),
    };
    let (client_id, client_secret, sandbox) =
        match paypal_account(pool, account.id as i32).await {
            Ok(account) => account,
            Err(err) => return fail(err),
        };
    let access_token = match paypal_access_token(&client_id, &client_secret, sandbox).await {
        Ok(token) => token,
        Err(err) => return fail(err),
    };
    // PayPal 只需要前台的返回地址（notify 用 Webhook 自己的地址）
    let (_, front) = site_urls(pool).await;
    if front.is_empty() {
        return fail("请在后台「站点设置」里填写站点地址（base_url）后再支付");
    }
    let amount = format!("{:.2}", payment.amount as f64 / 100.0);
    let subject = payment
        .remark
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    let mut payload = json!({
        "intent": "CAPTURE",
        "purchase_units": [{
            "reference_id": payment.payment_id,
            "amount": { "currency_code": "USD", "value": amount },
        }],
        "payment_source": {
            "paypal": {
                "experience_context": {
                    "brand_name": subject,
                    "locale": "en-US",
                    "shipping_preference": "NO_SHIPPING",
                    "user_action": "PAY_NOW",
                    "return_url": format!("{front}paypal/pay"),
                    "cancel_url": format!("{front}paypal/cancel"),
                }
            }
        }
    });
    if let Some(units) = payload.get_mut("purchase_units").and_then(Value::as_array_mut) {
        if let Some(unit) = units.first_mut() {
            if subject.is_empty() {
                if let Some(object) = unit.as_object_mut() {
                    object.remove("description");
                }
            } else if let Some(object) = unit.as_object_mut() {
                object.insert("description".to_string(), json!(subject));
            }
        }
    }
    let url = if sandbox {
        "https://api-m.sandbox.paypal.com/v2/checkout/orders"
    } else {
        "https://api-m.paypal.com/v2/checkout/orders"
    };
    let request = rustcms_ext::http::Request::post(url)
        .with_header("Authorization", format!("Bearer {access_token}"))
        .with_header("Accept", "application/json")
        .with_json(payload);
    let response = match rustcms_ext::http::send(&request).await {
        Ok(response) => response,
        Err(err) => return fail(format!("PayPal 下单请求失败：{err}")),
    };
    let text = response.text();
    let value: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
    if !response.is_success() {
        return fail(format!(
            "PayPal 下单失败（HTTP {}）：{}",
            response.status,
            text.trim()
        ));
    }
    let order_id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if order_id.is_empty() {
        return fail("PayPal 下单未返回订单号");
    }
    let jump_url = paypal_approve_link(&value).unwrap_or_default();
    if let Err(err) = payment::set_payment_account(pool, payment.id, account.id).await {
        return failed(err);
    }
    if let Err(err) = payment::set_payment_terrace(pool, payment.id, &order_id).await {
        return failed(err);
    }
    // `approve_url` 是本项目的规范字段；`jump_url` 保留给沿用既有实现命名的客户端
    ok(json!({
        "pay_way": trade::PAY_WAY_PAYPAL,
        "payment_id": payment.payment_id,
        "order_id": payment.order_id,
        "paypal_order_id": order_id,
        "approve_url": jump_url,
        "jump_url": jump_url,
    }))
}

/// 从 PayPal 下单响应里取付款链接（`rel=payer-action` > `approve` > 下标 1）。
fn paypal_approve_link(value: &Value) -> Option<String> {
    let links = value.get("links")?.as_array()?;
    let find = |rel: &str| {
        links
            .iter()
            .find(|link| link.get("rel").and_then(Value::as_str) == Some(rel))
            .and_then(|link| link.get("href"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    find("payer-action")
        .or_else(|| find("approve"))
        .or_else(|| {
            links
                .get(1)
                .and_then(|link| link.get("href"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

/// 读取 PayPal 账户配置：优先支付单上的账户，回退第一个启用的 paypal 账户。
async fn paypal_account(
    pool: &sqlx::mysql::MySqlPool,
    account_id: i32,
) -> Result<(String, String, bool), String> {
    let mut raw: Option<String> = None;
    if account_id > 0 {
        raw = sqlx::query_scalar::<_, String>(
            "SELECT COALESCE(pay_config, '') FROM payment_accounts WHERE id = ? LIMIT 1",
        )
        .bind(account_id)
        .fetch_optional(pool)
        .await
        .map_err(|err| format!("读取 PayPal 支付账户失败：{err}"))?;
    }
    if raw.as_deref().map(str::trim).unwrap_or("").is_empty() {
        raw = sqlx::query_scalar::<_, String>(
            "SELECT COALESCE(pay_config, '') FROM payment_accounts \
             WHERE pay_way = 'paypal' AND status = 1 ORDER BY id ASC LIMIT 1",
        )
        .fetch_optional(pool)
        .await
        .map_err(|err| format!("读取 PayPal 支付账户失败：{err}"))?;
    }
    let Some(raw) = raw.filter(|value| !value.trim().is_empty()) else {
        return Err("请先配置 PayPal 支付账户".to_string());
    };
    let config: Value = serde_json::from_str(&raw)
        .map_err(|err| format!("PayPal 支付账户配置解析失败：{err}"))?;
    let client_id = config
        .get("app_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let client_secret = config
        .get("app_secret")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if client_id.is_empty() || client_secret.is_empty() {
        return Err("PayPal 支付账户缺少 client_id / client_secret".to_string());
    }
    let sandbox = match config.get("sandbox") {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_i64().unwrap_or(0) != 0,
        Some(Value::String(text)) => matches!(text.as_str(), "1" | "true" | "True"),
        _ => false,
    };
    Ok((client_id, client_secret, sandbox))
}

/// PayPal OAuth：Basic(client_id:secret) + `grant_type=client_credentials`。
async fn paypal_access_token(
    client_id: &str,
    client_secret: &str,
    sandbox: bool,
) -> Result<String, String> {
    let base = if sandbox {
        "https://api-m.sandbox.paypal.com"
    } else {
        "https://api-m.paypal.com"
    };
    let basic = rustcms_ext::smtp::base64_encode(
        format!("{client_id}:{client_secret}").as_bytes(),
    );
    let request = rustcms_ext::http::Request::post(format!("{base}/v1/oauth2/token"))
        .with_header("Authorization", format!("Basic {basic}"))
        .with_header("Accept", "application/json")
        .with_form(vec![(
            "grant_type".to_string(),
            "client_credentials".to_string(),
        )]);
    let response = rustcms_ext::http::send(&request)
        .await
        .map_err(|err| format!("PayPal OAuth 请求失败：{err}"))?;
    if !response.is_success() {
        return Err(format!(
            "PayPal OAuth 失败（HTTP {}）：{}",
            response.status,
            response.text().trim()
        ));
    }
    let value: Value = response
        .json()
        .map_err(|err| format!("PayPal OAuth 响应解析失败：{err}"))?;
    value
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "PayPal OAuth 未返回 access_token".to_string())
}

/// 调用 `/v2/checkout/orders/{id}/capture`，返回订单状态与响应 JSON。
async fn paypal_capture(
    access_token: &str,
    sandbox: bool,
    order_id: &str,
) -> Result<(String, Value), String> {
    let base = if sandbox {
        "https://api-m.sandbox.paypal.com"
    } else {
        "https://api-m.paypal.com"
    };
    let request = rustcms_ext::http::Request::post(format!(
        "{base}/v2/checkout/orders/{order_id}/capture"
    ))
    .with_header("Authorization", format!("Bearer {access_token}"))
    .with_header("Content-Type", "application/json");
    let response = rustcms_ext::http::send(&request)
        .await
        .map_err(|err| format!("PayPal capture 请求失败：{err}"))?;
    let text = response.text();
    let value: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
    if !response.is_success() {
        return Err(format!(
            "PayPal capture 失败（HTTP {}）：{}",
            response.status,
            text.trim()
        ));
    }
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok((status, value))
}

/// `GET /paypal/cancel`：PayPal 取消支付的返回页。
async fn paypal_cancel() -> Response {
    fail("支付已取消")
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn parse_json_falls_back_to_defaults() {
        let empty: CartRequest = parse_json("").unwrap();
        assert_eq!(empty.quantity, 0);

        let parsed: CartRequest = parse_json(r#"{"archive_id":12,"quantity":3}"#).unwrap();
        assert_eq!(parsed.archive_id, 12);
        assert_eq!(parsed.quantity, 3);

        // 非法 JSON → 业务失败信封
        assert!(parse_json::<CartRequest>("{oops").is_err());
    }

    #[test]
    fn parse_form_reads_gateway_post() {
        let form = parse_form("out_trade_no=abc123&total_fee=1.00&sign=%E4%B8%AD%E6%96%87");
        assert_eq!(form.get("out_trade_no").map(String::as_str), Some("abc123"));
        assert_eq!(form.get("sign").map(String::as_str), Some("中文"));
        assert_eq!(form.get("missing"), None);
    }

    #[test]
    fn user_token_priority() {
        let mut headers = HeaderMap::new();
        assert!(user_token(&headers).is_none());

        headers.insert("token", HeaderValue::from_static("header-token"));
        assert_eq!(user_token(&headers).as_deref(), Some("header-token"));

        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer bearer-token"),
        );
        assert_eq!(user_token(&headers).as_deref(), Some("header-token"));

        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer bearer-token"),
        );
        assert_eq!(user_token(&headers).as_deref(), Some("bearer-token"));

        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_static("a=1; token=cookie-token"));
        assert_eq!(user_token(&headers).as_deref(), Some("cookie-token"));
    }

    #[test]
    fn router_builds() {
        // 路由表能构建出来（axum 在路径重复时会 panic，这里相当于一个冒烟测试）。
        let _ = router();
    }

    #[test]
    fn paypal_link_prefers_payer_action() {
        let value = json!({
            "links": [
                {"rel": "self", "href": "https://api/self"},
                {"rel": "approve", "href": "https://api/approve"},
                {"rel": "payer-action", "href": "https://api/payer-action"},
            ]
        });
        assert_eq!(
            paypal_approve_link(&value).as_deref(),
            Some("https://api/payer-action")
        );

        // 没有 payer-action 时退回 approve
        let value = json!({"links": [
            {"rel": "self", "href": "https://api/self"},
            {"rel": "approve", "href": "https://api/approve"},
        ]});
        assert_eq!(
            paypal_approve_link(&value).as_deref(),
            Some("https://api/approve")
        );

        // 只有 self/update 时退回下标 1（既有实现取 Links[1].Href）
        let value = json!({"links": [
            {"rel": "self", "href": "https://api/self"},
            {"rel": "update", "href": "https://api/update"},
        ]});
        assert_eq!(
            paypal_approve_link(&value).as_deref(),
            Some("https://api/update")
        );

        // 没有 links
        assert_eq!(paypal_approve_link(&json!({})), None);
    }

    #[test]
    fn client_ip_prefers_proxy_headers() {
        let mut headers = HeaderMap::new();
        assert_eq!(client_ip(&headers), "127.0.0.1");

        headers.insert("x-real-ip", HeaderValue::from_static("10.0.0.2"));
        assert_eq!(client_ip(&headers), "10.0.0.2");

        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static(" 10.0.0.1 , 10.0.0.9"),
        );
        assert_eq!(client_ip(&headers), "10.0.0.1");
    }

    #[test]
    fn gateway_replies_build() {
        // 支付宝要纯文本 success / fail
        assert_eq!(text_reply("success").status(), axum::http::StatusCode::OK);
        // 微信要 XML
        assert_eq!(
            wechat_reply(true, "OK").status(),
            axum::http::StatusCode::OK
        );
    }
}

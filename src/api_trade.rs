//! 交易 API（批次 C）：购物车 / 订单 / 店铺 / 支付 / 分销。
//!
//! 对应 Go 版 `controller/apiCommerce.go`、`controller/apiOrder.go`、
//! `controller/apiAgency.go`、`controller/member_center.go` 与
//! `controller/apiNotify.go`，路由挂在 `route/base.go` 的 `/api` 分组下。
//!
//! # 路由前缀
//!
//! Go 版这一批接口的真实路径是 `/api/cart/add`、`/api/order/create` …。
//! 本文件的 `router()` 同时注册**相对路径**（`/cart/add`）与
//! **`/api` 前缀路径**（`/api/cart/add`）两套，
//! 这样无论主线是 `.nest("/api", api_trade::router())` 还是
//! `.merge(api_trade::router())`（顶层），都能命中 Go 的 `/api/...` 路径。
//! 只需要一套时删掉另一套即可，不影响其它批次。
//!
//! # 鉴权
//!
//! 会员接口的登录态沿用 Go 的 `middleware.ParseUserToken`：
//! 请求头 `token` > `Authorization: Bearer` > Cookie `token`；
//! JWT 失效时回退到会员 API Key（`users.api_token`）。
//!
//! # 响应信封
//!
//! 统一 `{"code":0,"msg":"","data":…}`；分页类接口按 Go 版额外带顶层 `total`。

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth;
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
        .route(&format!("{prefix}/paypal/pay"), post(paypal_notify))
        .route(&format!("{prefix}/paypal/cancel"), get(paypal_cancel))
}

// ---------------------------------------------------------------------------
// 公共辅助
// ---------------------------------------------------------------------------

/// 带顶层 `total` 的成功信封（Go 版分页接口的口径）。
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

/// 取当前登录会员 ID（对齐 Go 的 `middleware.ParseUserToken`）。
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
    // JWT 无效时回退到会员 API Key（Go 的 `GetUserByApiToken`）。
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

/// 分页参数（兼容 Go 的 `page`/`current` 与 `pageSize` 两种写法）。
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

/// 购物车操作请求（Go 的 `request.CartRequest`）。
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
// 购物车（Go `controller/apiCommerce.go`）
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
// 订单（Go `controller/apiOrder.go`）
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

    // 支付方式：为空时取第一个可用方式（Go `GetPaymentWays`）。
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

    // 其余支付方式需要真实网关（微信 / 支付宝 / PayPal）。
    fail(format!("支付方式「{pay_way}」需要对接支付网关，Rust 版尚未实现"))
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
    // Go 版会轮询 6 秒等待异步回调，这里保持同样口径。
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
// 店铺 / 保证金（Go `controller/apiCommerce.go`）
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
// 分销（Go `controller/apiAgency.go`）
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
// 会员资金（Go `controller/member_center.go`）
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
// 支付网关回调 / 返回（Go `controller/apiNotify.go`）
// ---------------------------------------------------------------------------

/// `POST /alipay/pay`：支付宝异步通知。
///
/// Go 版要读表单 → 查支付单 → 用支付宝公钥验签 → 改支付单与订单。
/// Rust 版没有支付宝 SDK，验签与网关对账未实现：支付单存在性是真实校验，
/// 验签环节明确返回「未实现」。
async fn alipay_notify(State(state): State<AppState>, body: String) -> Response {
    let form = parse_form(&body);
    let out_trade_no = form.get("out_trade_no").cloned().unwrap_or_default();
    if out_trade_no.trim().is_empty() {
        return fail("支付参数错误");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match trade::payment_by_id(pool, out_trade_no.trim()).await {
        Ok(Some(payment)) => {
            if payment.paid_time > 0 {
                return ok_msg("success");
            }
            fail("支付宝支付回调尚未实现（需要支付宝证书验签）")
        }
        Ok(None) => fail("支付单不存在"),
        Err(err) => failed(err),
    }
}

/// `POST /paypal/pay`：PayPal Webhook / 返回页通知。
///
/// Go 版从 query 取 `token`（支付单号），再用 PayPal client 对
/// `payment.terrace_id` 执行 capture。这里同样先按 `token` 查支付单，
/// 已支付直接成功；未支付则换取 OAuth token 后调用 capture，成功后更新
/// `payments` / `orders` / 会员收尾。
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

/// 按支付单号执行 PayPal capture 并落库。
async fn paypal_capture_payment(state: &AppState, payment_id: &str) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let payment = match trade::payment_by_id(pool, payment_id).await {
        Ok(Some(payment)) => payment,
        Ok(None) => return fail("支付单不存在"),
        Err(err) => return failed(err),
    };
    if payment.paid_time > 0 {
        return ok_msg("success");
    }
    if payment.terrace_id.trim().is_empty() {
        return fail("支付单缺少 PayPal 订单号");
    }

    let (client_id, client_secret, sandbox) = match paypal_account(pool, &payment).await {
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
    let now = chrono::Utc::now().timestamp();
    if let Err(err) = sqlx::query(
        "UPDATE payments SET pay_way = 'paypal', paid_time = ?, status = 1, buyer_id = ?, \
         buyer_info = ?, updated_time = ? WHERE id = ?",
    )
    .bind(now)
    .bind(&buyer_id)
    .bind(&buyer_info)
    .bind(now)
    .bind(payment.id)
    .execute(pool)
    .await
    {
        return failed(err.into());
    }

    if let Ok(Some(order)) = trade::load_order(pool, &payment.order_id).await {
        let _ = sqlx::query("UPDATE orders SET payment_id = ? WHERE id = ?")
            .bind(&payment.payment_id)
            .bind(order.id)
            .execute(pool)
            .await;
        if let Err(err) = trade::success_paid_order(pool, &order, &payment).await {
            return failed(err.into());
        }
    }
    ok_msg("success")
}

/// 读取 PayPal 账户配置：优先支付单上的账户，回退第一个启用的 paypal 账户。
async fn paypal_account(
    pool: &sqlx::mysql::MySqlPool,
    payment: &trade::Payment,
) -> Result<(String, String, bool), String> {
    let mut raw: Option<String> = None;
    if payment.payment_account_id > 0 {
        raw = sqlx::query_scalar::<_, String>(
            "SELECT COALESCE(pay_config, '') FROM payment_accounts WHERE id = ? LIMIT 1",
        )
        .bind(payment.payment_account_id)
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
}

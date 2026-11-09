//! 交易数据访问（批次 C）：购物车 / 订单 / 店铺 / 支付 / 分销 / 积分。
//!
//! 覆盖范围：
//! - 购物车（加购 / 改数量 / 勾选 / 结算前校验）
//! - 店铺 / 保证金 / 已售订单 / 收到的评价
//! - 积分
//! - 订单（下单 / 订单列表 / 取消 / 退款申请 / 完成 / 支付单）
//! - 分销佣金 / 提现
//! - 余额提现 / 充值申请
//!
//! 口径（对外契约）：
//! - 金额一律 `i64`（分），禁止浮点。
//! - 订单 `status`：-1 已取消 / 0 待支付 / 1 已支付 / 2 配送中 / 3 已完成 / 8 退款中 / 9 已退款。
//! - 佣金 `status`：0 未提现 / 1 已提现 / -1 已失效。
//! - 资金流水 `direction`：1 收入 / 2 支出；`action`：1 销售 2 购买 3 退款 4 充值
//! 5 提现 6 分销 7 返现 8 佣金 9 保证金。
//! - 动钱路径（下单 / 支付 / 保证金 / 提现 / 完成结算）一律走事务 + `FOR UPDATE` 行锁。
//!
//! ⚠️ 沿用既有约定的「已知缺陷」（不擅自修正，见完工报告）：
//! - 下单不扣库存（既有实现只在取消 / 退款成功时把库存加回去）。
//! - `archives` 没有 `status` 列，内容是否上架由栏目 / 草稿表判断。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::mysql::{MySqlConnection, MySqlPool};
use sqlx::FromRow;

use super::{front_content, order as order_config, retailer as retailer_config};
use super::ModelError;

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

/// 订单状态：已取消。
pub const ORDER_STATUS_CANCELED: i8 = -1;
/// 订单状态：待支付。
pub const ORDER_STATUS_WAITING: i8 = 0;
/// 订单状态：已支付待发货。
pub const ORDER_STATUS_PAID: i8 = 1;
/// 订单状态：配送中。
pub const ORDER_STATUS_DELIVERING: i8 = 2;
/// 订单状态：已完成。
pub const ORDER_STATUS_COMPLETED: i8 = 3;
/// 订单状态：退款中。
pub const ORDER_STATUS_REFUNDING: i8 = 8;
/// 订单状态：已退款。
pub const ORDER_STATUS_REFUNDED: i8 = 9;

/// 订单类型：普通商品。
pub const ORDER_TYPE_GOODS: &str = "goods";
/// 订单类型：会员组（VIP）。
pub const ORDER_TYPE_VIP: &str = "vip";

/// 佣金状态：未提现。
pub const COMMISSION_STATUS_WAIT: i8 = 0;
/// 佣金状态：已提现。
pub const COMMISSION_STATUS_PAID: i8 = 1;
/// 佣金状态：已失效（退款）。
pub const COMMISSION_STATUS_CANCEL: i8 = -1;

/// 资金流水方向：收入。
pub const FINANCE_INCOME: i8 = 1;
/// 资金流水方向：支出。
pub const FINANCE_OUTPUT: i8 = 2;

/// 资金流水动作：销售收入。
pub const FINANCE_ACTION_SALE: i8 = 1;
/// 资金流水动作：购买支出。
pub const FINANCE_ACTION_BUY: i8 = 2;
/// 资金流水动作：退款支出。
pub const FINANCE_ACTION_REFUND: i8 = 3;
/// 资金流水动作：充值。
pub const FINANCE_ACTION_CHARGE: i8 = 4;
/// 资金流水动作：提现。
pub const FINANCE_ACTION_WITHDRAW: i8 = 5;
/// 资金流水动作：分销。
pub const FINANCE_ACTION_SPREAD: i8 = 6;
/// 资金流水动作：返现。
pub const FINANCE_ACTION_CASH_BACK: i8 = 7;
/// 资金流水动作：佣金。
pub const FINANCE_ACTION_COMMISSION: i8 = 8;
/// 资金流水动作：店铺保证金缴纳 / 退还。
pub const FINANCE_ACTION_DEPOSIT: i8 = 9;

/// 支付方式：微信网页支付。
pub const PAY_WAY_WECHAT: &str = "wechat";
/// 支付方式：微信小程序支付。
pub const PAY_WAY_WEAPP: &str = "weapp";
/// 支付方式：支付宝。
pub const PAY_WAY_ALIPAY: &str = "alipay";
/// 支付方式：线下支付。
pub const PAY_WAY_OFFLINE: &str = "offline";
/// 支付方式：余额支付。
pub const PAY_WAY_BALANCE: &str = "balance";
/// 支付方式：PayPal。
pub const PAY_WAY_PAYPAL: &str = "paypal";

/// 积分流水类型：签到。
pub const POINT_TYPE_SIGN: i8 = 1;
/// 积分流水类型：消费。
pub const POINT_TYPE_USE: i8 = 2;
/// 积分流水类型：管理调整。
pub const POINT_TYPE_ADMIN: i8 = 3;
/// 积分流水类型：兑换。
pub const POINT_TYPE_REDEEM: i8 = 4;

/// 保证金流水类型：缴纳。
pub const DEPOSIT_TYPE_PAY: i8 = 1;
/// 保证金流水类型：退还。
pub const DEPOSIT_TYPE_REFUND: i8 = 2;
/// 保证金流水类型：扣划。
pub const DEPOSIT_TYPE_DEDUCT: i8 = 3;

/// 店铺状态：待审核。
pub const SHOP_STATUS_PENDING: i8 = 0;
/// 店铺状态：正常。
pub const SHOP_STATUS_NORMAL: i8 = 1;
/// 店铺状态：已关闭。
pub const SHOP_STATUS_CLOSED: i8 = 2;

/// 提现方式：微信。
pub const WITHDRAW_WAY_WECHAT: i8 = 1;
/// 提现方式：支付宝。
pub const WITHDRAW_WAY_ALIPAY: i8 = 2;
/// 提现方式：银行卡。
pub const WITHDRAW_WAY_BANK: i8 = 3;

/// 提现状态：待审核。
pub const WITHDRAW_STATUS_WAITING: i8 = 0;

/// 单个购物车条目的数量上限。
pub const CART_ITEM_LIMIT: i32 = 999;
/// 积分 / 保证金流水单页上限。
pub const POINT_LOG_LIMIT: i64 = 50;
/// 签到基础积分。
pub const SIGN_BASE_POINTS: i64 = 5;
/// 连续签到额外积分上限。
pub const SIGN_EXTRA_MAX: i64 = 10;
/// 充值金额上限（分）。
pub const RECHARGE_AMOUNT_MAX: i64 = 100_000_000;
/// 提现到微信的最低金额（分，2 元）。
pub const WITHDRAW_MIN_AMOUNT: i64 = 200;

// ---------------------------------------------------------------------------
// 结构体
// ---------------------------------------------------------------------------

/// 购物车条目（表 `carts`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Cart {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_id: u32,
    pub archive_id: i64,
    pub quantity: i32,
}

/// 购物车列表条目（`Cart` + 计算字段）。
#[derive(Debug, Clone, Serialize)]
pub struct CartItem {
    #[serde(flatten)]
    pub cart: Cart,
    /// 商品快照（内嵌 `*model.Archive`）。
    pub archive: Option<Value>,
    /// 小计（分）。
    pub amount: i64,
    /// 封面地址。
    pub thumb: String,
    /// 商品详情页地址。
    pub link: String,
}

/// 订单（表 `orders`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct TradeOrder {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub order_id: String,
    pub payment_id: String,
    pub user_id: u32,
    pub address_id: u32,
    pub remark: String,
    /// 库列名是 `type`（MySQL 关键字）。
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub kind: String,
    pub status: i8,
    pub refund_status: i8,
    pub origin_amount: i64,
    pub amount: i64,
    pub paid_time: i64,
    pub end_time: i64,
    pub deliver_time: i64,
    pub finished_time: i64,
    pub discount_amount: i64,
    pub user_discount: i64,
    pub coupon_discount: i64,
    pub coupon_code_id: String,
    pub seller_id: u32,
    pub seller_amount: i64,
    pub share_user_id: u32,
    pub share_parent_user_id: u32,
    pub share_amount: i64,
    pub share_parent_amount: i64,
    pub express_company: String,
    pub tracking_number: String,
}

/// 订单输出结构（`Order` + `details` / `order_address` / `user`）。
#[derive(Debug, Clone, Serialize)]
pub struct OrderView {
    #[serde(flatten)]
    pub order: TradeOrder,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_address: Option<OrderAddress>,
    pub user: Option<UserBrief>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_user: Option<UserBrief>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_user: Option<UserBrief>,
    pub is_updated: i32,
    pub details: Vec<OrderDetailView>,
}

/// 订单明细（表 `order_details`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct OrderDetail {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub order_id: String,
    pub user_id: u32,
    pub goods_id: i64,
    pub goods_sku_id: i64,
    pub price: i64,
    pub origin_price: i64,
    pub amount: i64,
    pub origin_amount: i64,
    pub quantity: i32,
    pub status: i8,
}

/// 订单明细输出（`OrderDetail` + `goods` / `group`）。
#[derive(Debug, Clone, Serialize)]
pub struct OrderDetailView {
    #[serde(flatten)]
    pub detail: OrderDetail,
    pub goods: Option<Value>,
    pub group: Option<Value>,
}

/// 收货地址（表 `order_addresses`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct OrderAddress {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_id: u32,
    pub name: String,
    pub last_name: String,
    pub phone: String,
    pub email: String,
    pub province: String,
    pub province_code: String,
    pub city: String,
    pub town: String,
    pub country: String,
    pub country_code: String,
    pub address_info: String,
    pub company: String,
    pub postcode: String,
    pub status: i8,
}

/// 收货地址入参（`OrderAddressRequest`）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OrderAddressInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub last_name: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub subscribed: String,
    #[serde(default)]
    pub province: String,
    #[serde(default)]
    pub province_code: String,
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub town: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub country_code: String,
    #[serde(default)]
    pub address_info: String,
    #[serde(default)]
    pub company: String,
    #[serde(default)]
    pub postcode: String,
    #[serde(default)]
    pub status: i8,
}

/// 退款申请（表 `order_refunds`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct OrderRefund {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub refund_id: String,
    pub order_id: String,
    pub detail_id: i32,
    pub user_id: u32,
    pub amount: i64,
    pub refund_time: i64,
    pub status: i8,
    pub error_times: i32,
    pub last_time: i64,
    pub remark: Option<String>,
}

/// 支付单（表 `payments`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Payment {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub payment_id: String,
    pub terrace_id: String,
    pub user_id: u32,
    pub order_id: String,
    pub amount: i64,
    pub status: i8,
    pub payment_account_id: i32,
    pub pay_way: String,
    pub paid_time: i64,
    pub remark: Option<String>,
    pub buyer_id: String,
    pub buyer_info: String,
}

/// 店铺（表 `shops`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Shop {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_id: u32,
    pub name: String,
    pub logo: String,
    pub banner: String,
    pub intro: String,
    pub notice: String,
    pub contact: String,
    pub theme_color: String,
    pub decoration: Option<String>,
    pub status: i8,
    pub deposit: i64,
    pub deposit_total: i64,
    pub product_count: i64,
    pub order_count: i64,
    pub sale_amount: i64,
    /// 库里存相对路径，展示时补全地址。
    pub logo_url: String,
    pub banner_url: String,
}

/// 店铺装修配置（`ShopDecoration`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShopDecoration {
    /// 布局：wide / compact。
    #[serde(default = "default_layout")]
    pub layout: String,
    /// 顶部横幅。
    #[serde(default)]
    pub banner: String,
    /// 是否展示公告。
    #[serde(default = "default_true")]
    pub show_notice: bool,
    /// 公告内容。
    #[serde(default)]
    pub notice: String,
    /// 展示模块顺序。
    #[serde(default = "default_modules")]
    pub modules: Vec<String>,
    /// 主题色。
    #[serde(default)]
    pub theme_color: String,
    /// 页脚。
    #[serde(default)]
    pub footer: String,
}

fn default_layout() -> String {
    "wide".to_string()
}

fn default_true() -> bool {
    true
}

fn default_modules() -> Vec<String> {
    vec![
        "products".to_string(),
        "notice".to_string(),
        "contact".to_string(),
    ]
}

impl Default for ShopDecoration {
    fn default() -> Self {
        Self {
            layout: default_layout(),
            banner: String::new(),
            show_notice: true,
            notice: String::new(),
            modules: default_modules(),
            theme_color: String::new(),
            footer: String::new(),
        }
    }
}

/// 保证金流水（表 `shop_deposit_logs`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ShopDepositLog {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub shop_id: u32,
    pub user_id: u32,
    pub amount: i64,
    pub after_amount: i64,
    /// 库列名是 `type`（MySQL 关键字）。
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub kind: i8,
    pub remark: String,
}

/// 积分流水（表 `point_logs`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct PointLog {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_id: u32,
    pub points: i64,
    pub after_points: i64,
    /// 库列名是 `type`（MySQL 关键字）。
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub kind: i8,
    pub remark: String,
}

/// 签到记录（表 `user_signs`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct UserSign {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_id: u32,
    pub sign_date: String,
    pub days: i32,
    pub points: i64,
}

/// 资金流水（表 `finances`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct FinanceRow {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_id: u32,
    pub direction: i8,
    pub amount: i64,
    pub after_amount: i64,
    pub action: i8,
    pub order_id: String,
    pub status: i8,
    pub remark: String,
}

/// 分销佣金（表 `commissions`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Commission {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_id: u32,
    pub order_id: String,
    pub order_amount: i64,
    pub amount: i64,
    pub status: i8,
    /// 库列是有符号 `int(10)`。
    pub withdraw_id: i32,
    pub remark: String,
}

/// 提现申请（表 `user_withdraws`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct UserWithdraw {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_id: u32,
    pub amount: i64,
    pub success_time: Option<i64>,
    pub withdraw_way: i8,
    pub status: i8,
    pub error_times: i64,
    pub last_time: i64,
    pub remark: String,
}

/// 充值申请（表 `recharges`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Recharge {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_id: u32,
    pub amount: i64,
    pub pay_way: String,
    pub remark: String,
    pub status: i8,
    pub confirm_time: i64,
}

/// 会员简要信息（订单 / 佣金里嵌的用户）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct UserBrief {
    pub id: u32,
    pub user_name: String,
    pub real_name: String,
    pub avatar_url: String,
    pub group_id: u32,
    pub is_retailer: i8,
}

/// 会员（交易相关字段，`User` 子集）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct TradeUser {
    pub id: u32,
    pub created_time: Option<i64>,
    pub user_name: String,
    pub real_name: String,
    pub avatar_url: String,
    pub introduce: String,
    pub email: String,
    pub phone: String,
    pub group_id: u32,
    /// 库列是有符号 `int(10)`。
    pub parent_id: i32,
    pub is_retailer: i8,
    pub balance: i64,
    pub total_reward: i64,
    pub order_count: u32,
    pub points: i64,
    pub status: i8,
}

/// 收到的评价（表 `comments` + 内容标题）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ReceivedComment {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub archive_id: i64,
    pub user_id: u32,
    pub user_name: String,
    pub email: String,
    pub ip: String,
    pub vote_count: i32,
    pub content: Option<String>,
    pub parent_id: u32,
    pub to_uid: u32,
    pub status: i8,
    pub rating: i8,
    /// 所属内容标题。
    pub item_title: String,
}

/// 下单明细入参（`OrderDetail`）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OrderDetailInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub order_id: String,
    #[serde(default)]
    pub user_id: u32,
    #[serde(default)]
    pub goods_id: i64,
    #[serde(default)]
    pub goods_sku_id: i64,
    #[serde(default)]
    pub price: i64,
    #[serde(default)]
    pub origin_price: i64,
    #[serde(default)]
    pub amount: i64,
    #[serde(default)]
    pub real_amount: i64,
    #[serde(default)]
    pub quantity: i32,
    #[serde(default)]
    pub status: i8,
}

/// 下单入参（`OrderRequest` 里用得到的字段）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreateOrderInput {
    #[serde(default)]
    pub order_id: String,
    #[serde(default)]
    pub user_id: u32,
    #[serde(default)]
    pub address_id: u32,
    #[serde(default)]
    pub remark: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub status: i8,
    #[serde(default)]
    pub refund_status: i8,
    #[serde(default)]
    pub origin_amount: i64,
    #[serde(default)]
    pub amount: i64,
    #[serde(default)]
    pub paid_time: i64,
    #[serde(default)]
    pub end_time: i64,
    #[serde(default)]
    pub deliver_time: i64,
    #[serde(default)]
    pub finished_time: i64,
    #[serde(default)]
    pub discount_amount: i64,
    #[serde(default)]
    pub coupon_id: i64,
    #[serde(default)]
    pub coupon_code_id: i64,
    #[serde(default)]
    pub coupon_code: String,
    #[serde(default)]
    pub share_user_id: u32,
    #[serde(default)]
    pub share_amount: i64,
    #[serde(default)]
    pub share_parent_amount: i64,
    #[serde(default)]
    pub share_self_amount: i64,
    #[serde(default)]
    pub address: Option<OrderAddressInput>,
    #[serde(default)]
    pub details: Vec<OrderDetailInput>,
    /// 「直接买」：不带 details 时使用。
    #[serde(default)]
    pub goods_id: i64,
    #[serde(default)]
    pub quantity: i32,
}

/// 支付入参（`PaymentRequest`）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PaymentInput {
    #[serde(default)]
    pub order_id: String,
    #[serde(default)]
    pub user_id: u32,
    #[serde(default)]
    pub pay_way: String,
}

/// 店铺资料入参（`ShopRequest`）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ShopInput {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub logo: String,
    #[serde(default)]
    pub banner: String,
    #[serde(default)]
    pub intro: String,
    #[serde(default)]
    pub notice: String,
    #[serde(default)]
    pub contact: String,
    #[serde(default)]
    pub theme_color: String,
    /// 店铺装修 JSON。
    #[serde(default)]
    pub decoration: String,
}

/// 余额提现入参。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WithdrawInput {
    #[serde(default)]
    pub amount: i64,
    #[serde(default)]
    pub way: i8,
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub real_name: String,
    #[serde(default)]
    pub remark: String,
}

/// 充值申请入参。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RechargeInput {
    #[serde(default)]
    pub amount: i64,
    #[serde(default)]
    pub pay_way: String,
    #[serde(default)]
    pub remark: String,
}

/// 保证金操作入参（`DepositRequest`）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DepositInput {
    /// 单位：分。
    #[serde(default)]
    pub amount: i64,
}

/// 卖家统计。
#[derive(Debug, Clone, Copy, Default)]
pub struct SellerStats {
    /// 订单数。
    pub order_count: i64,
    /// 销售额（分）。
    pub sale_amount: i64,
    /// 待发货订单数。
    pub pending: i64,
    /// 今日成交额（分）。
    pub today_amount: i64,
}

/// 分销统计。
#[derive(Debug, Clone, Copy, Default)]
pub struct RetailerStats {
    /// 可提现金额（分）。
    pub can_withdraw_amount: i64,
    /// 已提现金额（分）。
    pub paid_withdraw_amount: i64,
    /// 未结算金额（分）。
    pub unfinished_withdraw_amount: i64,
    /// 团队人数。
    pub member_count: i64,
}

// ---------------------------------------------------------------------------
// 公共辅助
// ---------------------------------------------------------------------------

/// 当前时间戳（秒）。
pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 生成业务单号（`AfterCreate`：`20060102150405` + 自增 ID 后 4 位）。
pub fn make_biz_no(ts: i64, id: u32) -> String {
    let stamp = chrono::DateTime::from_timestamp(ts, 0)
        .map(|time| time.format("%Y%m%d%H%M%S").to_string())
        .unwrap_or_else(|| ts.to_string());
    let suffix = format!("{id:04}");
    let tail: String = suffix
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    format!("{stamp}{tail}")
}

/// 读会员（交易用到的字段）。
pub async fn user_row(pool: &MySqlPool, user_id: u32) -> anyhow::Result<Option<TradeUser>> {
    Ok(sqlx::query_as::<_, TradeUser>(
        "SELECT id, created_time, user_name, real_name, avatar_url, introduce, email, phone, \
         group_id, parent_id, is_retailer, balance, total_reward, order_count, points, status \
         FROM users WHERE id = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?)
}

/// 读会员简要信息（多条）。
pub async fn user_briefs(pool: &MySqlPool, ids: &[u32]) -> anyhow::Result<HashMap<u32, UserBrief>> {
    let mut map = HashMap::new();
    if ids.is_empty() {
        return Ok(map);
    }
    let mut sql = String::from(
        "SELECT id, user_name, real_name, avatar_url, group_id, is_retailer FROM users WHERE id IN (",
    );
    for (index, _) in ids.iter().enumerate() {
        if index > 0 {
            sql.push(',');
        }
        sql.push('?');
    }
    sql.push(')');
    let mut query = sqlx::query_as::<_, UserBrief>(&sql);
    for id in ids {
        query = query.bind(*id);
    }
    for row in query.fetch_all(pool).await? {
        map.insert(row.id, row);
    }
    Ok(map)
}

/// 读用户组配置（`user_groups.setting` 里的 JSON）。
pub async fn group_setting(pool: &MySqlPool, group_id: u32) -> Value {
    let raw: Option<String> = sqlx::query_scalar("SELECT setting FROM user_groups WHERE id = ?")
        .bind(group_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    raw.and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(|value| value.is_object())
        .unwrap_or_else(|| json!({}))
}

/// 从用户组配置里取整数（缺省 0）。
pub fn group_int(setting: &Value, key: &str) -> i64 {
    setting
        .get(key)
        .map(|value| match value {
            Value::Number(number) => number.as_i64().unwrap_or(0),
            Value::String(text) => text.trim().parse::<i64>().unwrap_or(0),
            _ => 0,
        })
        .unwrap_or(0)
}

/// 读存储地址（`settings.storage.storage_url`）。
pub async fn storage_url(pool: &MySqlPool) -> String {
    super::site_setting::get_section(pool, "storage")
        .await
        .ok()
        .and_then(|value| {
            value
                .get("storage_url")
                .and_then(|item| item.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// 订单状态串转数值（`GetOrderList` 的 status 映射）。
pub fn status_value(status: &str) -> Option<i8> {
    match status.trim() {
        "waiting" => Some(ORDER_STATUS_WAITING),
        "paid" => Some(ORDER_STATUS_PAID),
        "delivery" => Some(ORDER_STATUS_DELIVERING),
        "finished" => Some(ORDER_STATUS_COMPLETED),
        "refunding" => Some(ORDER_STATUS_REFUNDING),
        "closed" => Some(ORDER_STATUS_CANCELED),
        _ => None,
    }
}

/// 写一条资金流水。
pub async fn insert_finance(
    tx: &mut MySqlConnection,
    user_id: u32,
    direction: i8,
    amount: i64,
    after_amount: i64,
    action: i8,
    order_id: &str,
    remark: &str,
) -> anyhow::Result<()> {
    let ts = now();
    sqlx::query(
        "INSERT INTO finances (created_time, updated_time, user_id, direction, amount, \
         after_amount, action, order_id, status, remark) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?)",
    )
    .bind(ts)
    .bind(ts)
    .bind(user_id)
    .bind(direction)
    .bind(amount)
    .bind(after_amount)
    .bind(action)
    .bind(order_id)
    .bind(remark)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 购物车
// ---------------------------------------------------------------------------

/// 购物车列表，同时算出每条小计与合计（单位：分）。
///
/// 与既有行为一致：商品已被删除的购物车记录会被顺手清掉。
pub async fn cart_list(pool: &MySqlPool, user_id: u32) -> anyhow::Result<(Vec<CartItem>, i64)> {
    if user_id == 0 {
        return Ok((Vec::new(), 0));
    }
    let carts = sqlx::query_as::<_, Cart>(
        "SELECT id, created_time, updated_time, user_id, archive_id, quantity \
         FROM carts WHERE user_id = ? ORDER BY id DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    if carts.is_empty() {
        return Ok((Vec::new(), 0));
    }

    let ids: Vec<i64> = carts.iter().map(|item| item.archive_id).collect();
    let archives = load_archives(pool, &ids).await?;
    let links = front_content::LinkBuilder::load(pool).await.ok();
    let storage = storage_url(pool).await;

    let mut total: i64 = 0;
    let mut list: Vec<CartItem> = Vec::with_capacity(carts.len());
    for item in carts {
        let Some(archive) = archives.get(&item.archive_id) else {
            // 商品已被删除，直接清掉这条购物车记录
            sqlx::query("DELETE FROM carts WHERE id = ?")
                .bind(item.id)
                .execute(pool)
                .await?;
            continue;
        };
        let mut value = front_content::archive_json(archive);
        let link = links
            .as_ref()
            .map(|builder| builder.archive(archive))
            .unwrap_or_default();
        let thumb = front_content::join_storage_url(&storage, &archive.cover());
        if let Value::Object(map) = &mut value {
            map.insert("link".into(), Value::String(link.clone()));
            map.insert("thumb".into(), Value::String(thumb.clone()));
        }
        let amount = archive.price * i64::from(item.quantity);
        total += amount;
        list.push(CartItem {
            cart: item,
            archive: Some(value),
            amount,
            thumb,
            link,
        });
    }

    Ok((list, total))
}

/// 批量取内容整行（保持传入顺序）。
async fn load_archives(
    pool: &MySqlPool,
    ids: &[i64],
) -> anyhow::Result<HashMap<i64, super::archive::Archive>> {
    let mut map = HashMap::new();
    if ids.is_empty() {
        return Ok(map);
    }
    let mut sql = String::from("SELECT * FROM archives WHERE id IN (");
    for (index, _) in ids.iter().enumerate() {
        if index > 0 {
            sql.push(',');
        }
        sql.push('?');
    }
    sql.push(')');
    let mut query = sqlx::query_as::<_, super::archive::Archive>(&sql);
    for id in ids {
        query = query.bind(*id);
    }
    for row in query.fetch_all(pool).await? {
        map.insert(row.id, row);
    }
    Ok(map)
}

/// 购物车条目数。
pub async fn count_cart(pool: &MySqlPool, user_id: u32) -> anyhow::Result<i64> {
    if user_id == 0 {
        return Ok(0);
    }
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM carts WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await?)
}

/// 加入购物车，同一商品累加数量。
pub async fn add_cart(
    pool: &MySqlPool,
    user_id: u32,
    archive_id: i64,
    quantity: i32,
) -> Result<Cart, ModelError> {
    if user_id == 0 {
        return Err(ModelError::invalid("请先登录"));
    }
    let quantity = if quantity <= 0 { 1 } else { quantity };
    let Some(archive) = super::archive::get(pool, archive_id).await? else {
        return Err(ModelError::invalid("内容不存在"));
    };
    if archive.user_id == user_id {
        return Err(ModelError::invalid("不能购买自己的商品"));
    }
    if archive.stock > 0 && i64::from(quantity) > archive.stock {
        return Err(ModelError::invalid("库存不足"));
    }

    let existing: Option<u32> = sqlx::query_scalar("SELECT id FROM carts WHERE user_id = ? AND archive_id = ?")
        .bind(user_id)
        .bind(archive_id)
        .fetch_optional(pool)
        .await?;
    let ts = now();
    let id: u32 = match existing {
        Some(id) => {
            let current: i32 = sqlx::query_scalar("SELECT quantity FROM carts WHERE id = ?")
                .bind(id)
                .fetch_one(pool)
                .await?;
            let next = (current + quantity).min(CART_ITEM_LIMIT);
            sqlx::query("UPDATE carts SET quantity = ?, updated_time = ? WHERE id = ?")
                .bind(next)
                .bind(ts)
                .bind(id)
                .execute(pool)
                .await?;
            id
        }
        None => {
            let result = sqlx::query(
                "INSERT INTO carts (created_time, updated_time, user_id, archive_id, quantity) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(ts)
            .bind(ts)
            .bind(user_id)
            .bind(archive_id)
            .bind(quantity)
            .execute(pool)
            .await?;
            result.last_insert_id() as u32
        }
    };

    let row = sqlx::query_as::<_, Cart>(
        "SELECT id, created_time, updated_time, user_id, archive_id, quantity FROM carts WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// 修改购物车数量，`quantity <= 0` 表示移除。
pub async fn update_cart_quantity(
    pool: &MySqlPool,
    user_id: u32,
    cart_id: u32,
    quantity: i32,
) -> Result<(), ModelError> {
    let exists: Option<u32> =
        sqlx::query_scalar("SELECT id FROM carts WHERE id = ? AND user_id = ?")
            .bind(cart_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    if exists.is_none() {
        return Err(ModelError::invalid("内容不存在"));
    }
    if quantity <= 0 {
        sqlx::query("DELETE FROM carts WHERE id = ? AND user_id = ?")
            .bind(cart_id)
            .bind(user_id)
            .execute(pool)
            .await?;
        return Ok(());
    }
    let quantity = quantity.min(CART_ITEM_LIMIT);
    sqlx::query("UPDATE carts SET quantity = ?, updated_time = ? WHERE id = ? AND user_id = ?")
        .bind(quantity)
        .bind(now())
        .bind(cart_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 移除购物车条目。
pub async fn delete_cart(
    pool: &MySqlPool,
    user_id: u32,
    cart_id: u32,
) -> Result<(), ModelError> {
    sqlx::query("DELETE FROM carts WHERE id = ? AND user_id = ?")
        .bind(cart_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 清空购物车。
pub async fn clear_cart(pool: &MySqlPool, user_id: u32) -> Result<(), ModelError> {
    sqlx::query("DELETE FROM carts WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 购物车结算：把购物车条目转成订单明细，复用下单逻辑。
pub async fn checkout_cart(
    pool: &MySqlPool,
    user_id: u32,
    mut input: CreateOrderInput,
) -> Result<TradeOrder, ModelError> {
    let (carts, total) = cart_list(pool, user_id).await?;
    if carts.is_empty() {
        return Err(ModelError::invalid("购物车为空"));
    }
    if total <= 0 {
        return Err(ModelError::invalid("订单金额不正确"));
    }
    let mut details: Vec<OrderDetailInput> = Vec::with_capacity(carts.len());
    for item in &carts {
        let price = item
            .archive
            .as_ref()
            .and_then(|value| value.get("price"))
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        details.push(OrderDetailInput {
            goods_id: item.cart.archive_id,
            price,
            origin_price: price,
            amount: item.amount,
            real_amount: item.amount,
            quantity: item.cart.quantity,
            user_id,
            ..Default::default()
        });
    }
    input.details = details;
    input.user_id = user_id;
    let order = create_order(pool, user_id, input).await?;
    clear_cart(pool, user_id).await?;
    Ok(order)
}

// ---------------------------------------------------------------------------
// 订单
// ---------------------------------------------------------------------------

/// 读 `settings.order` 里的开关（数字 / 布尔都认）。
fn order_flag(config: &Value, key: &str) -> bool {
    match config.get(key) {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_i64().unwrap_or(0) != 0,
        Some(Value::String(text)) => text.trim() == "1" || text.trim() == "true",
        _ => false,
    }
}

/// 读 `settings.order` 里的整数。
fn order_number(config: &Value, key: &str) -> i64 {
    match config.get(key) {
        Some(Value::Number(number)) => number.as_i64().unwrap_or(0),
        Some(Value::String(text)) => text.trim().parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

/// 用户组折扣（`GetUserDiscount` 的第一个返回值）。
async fn user_discount(pool: &MySqlPool, user: Option<&TradeUser>) -> i64 {
    let Some(user) = user else {
        return 0;
    };
    let mut discount = group_int(&group_setting(pool, user.group_id).await, "discount");
    if user.parent_id > 0 {
        if let Ok(Some(parent)) = user_row(pool, user.parent_id as u32).await {
            let share = group_int(&group_setting(pool, parent.group_id).await, "share_discount");
            if share > discount {
                discount = share;
            }
        }
    }
    discount
}

/// 在事务里保存收货地址（`SaveOrderAddress`）。
async fn save_order_address_tx(
    tx: &mut MySqlConnection,
    user_id: u32,
    input: &OrderAddressInput,
) -> Result<OrderAddress, ModelError> {
    let mut row: Option<OrderAddress> = None;
    if input.id > 0 {
        let found = sqlx::query_as::<_, OrderAddress>(
            "SELECT id, created_time, updated_time, user_id, name, last_name, phone, email, \
             province, province_code, city, town, country, country_code, address_info, company, \
             postcode, status FROM order_addresses WHERE id = ?",
        )
        .bind(input.id)
        .fetch_optional(&mut *tx)
        .await?;
        match found {
            Some(found) if found.user_id == user_id => row = Some(found),
            _ => return Err(ModelError::invalid("收货地址不存在")),
        }
    }
    let mut row = match row {
        Some(row) => row,
        None => OrderAddress {
            id: 0,
            created_time: None,
            updated_time: None,
            user_id,
            name: String::new(),
            last_name: String::new(),
            phone: String::new(),
            email: String::new(),
            province: String::new(),
            province_code: String::new(),
            city: String::new(),
            town: String::new(),
            country: String::new(),
            country_code: String::new(),
            address_info: String::new(),
            company: String::new(),
            postcode: String::new(),
            status: 0,
        },
    };
    row.name = input.name.clone();
    row.last_name = input.last_name.clone();
    row.phone = input.phone.clone();
    row.email = input.email.clone();
    row.province = input.province.clone();
    row.province_code = input.province_code.clone();
    row.city = input.city.clone();
    row.town = input.town.clone();
    row.country = input.country.clone();
    row.country_code = input.country_code.clone();
    row.address_info = input.address_info.clone();
    row.company = input.company.clone();
    row.postcode = input.postcode.clone();
    row.status = 1;

    let ts = now();
    let id = if row.id > 0 {
        sqlx::query(
            "UPDATE order_addresses SET updated_time = ?, user_id = ?, name = ?, last_name = ?, \
             phone = ?, email = ?, province = ?, province_code = ?, city = ?, town = ?, \
             country = ?, country_code = ?, address_info = ?, company = ?, postcode = ?, status = ? \
             WHERE id = ?",
        )
        .bind(ts)
        .bind(row.user_id)
        .bind(&row.name)
        .bind(&row.last_name)
        .bind(&row.phone)
        .bind(&row.email)
        .bind(&row.province)
        .bind(&row.province_code)
        .bind(&row.city)
        .bind(&row.town)
        .bind(&row.country)
        .bind(&row.country_code)
        .bind(&row.address_info)
        .bind(&row.company)
        .bind(&row.postcode)
        .bind(row.status)
        .bind(row.id)
        .execute(&mut *tx)
        .await?;
        row.id
    } else {
        let result = sqlx::query(
            "INSERT INTO order_addresses (created_time, updated_time, user_id, name, last_name, \
             phone, email, province, province_code, city, town, country, country_code, \
             address_info, company, postcode, status) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)",
        )
        .bind(ts)
        .bind(ts)
        .bind(row.user_id)
        .bind(&row.name)
        .bind(&row.last_name)
        .bind(&row.phone)
        .bind(&row.email)
        .bind(&row.province)
        .bind(&row.province_code)
        .bind(&row.city)
        .bind(&row.town)
        .bind(&row.country)
        .bind(&row.country_code)
        .bind(&row.address_info)
        .bind(&row.company)
        .bind(&row.postcode)
        .execute(&mut *tx)
        .await?;
        result.last_insert_id() as u32
    };
    row.id = id;
    row.created_time = Some(ts);
    row.updated_time = Some(ts);
    Ok(row)
}

/// 保存收货地址（对外版本，自动开事务）。
pub async fn save_order_address(
    pool: &MySqlPool,
    user_id: u32,
    input: &OrderAddressInput,
) -> Result<OrderAddress, ModelError> {
    let mut tx = pool.begin().await?;
    let row = save_order_address_tx(&mut *tx, user_id, input).await?;
    tx.commit().await?;
    Ok(row)
}

/// 我的收货地址列表（`id desc`）。
pub async fn order_addresses(
    pool: &MySqlPool,
    user_id: u32,
) -> anyhow::Result<Vec<OrderAddress>> {
    Ok(sqlx::query_as::<_, OrderAddress>(
        "SELECT id, created_time, updated_time, user_id, name, last_name, phone, email, \
         province, province_code, city, town, country, country_code, address_info, company, \
         postcode, status FROM order_addresses WHERE user_id = ? ORDER BY id DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

/// 最近一条收货地址。
pub async fn order_address(
    pool: &MySqlPool,
    user_id: u32,
) -> anyhow::Result<Option<OrderAddress>> {
    Ok(sqlx::query_as::<_, OrderAddress>(
        "SELECT id, created_time, updated_time, user_id, name, last_name, phone, email, \
         province, province_code, city, town, country, country_code, address_info, company, \
         postcode, status FROM order_addresses WHERE user_id = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?)
}

/// 按 ID 取收货地址。
pub async fn order_address_by_id(
    pool: &MySqlPool,
    id: u32,
) -> anyhow::Result<Option<OrderAddress>> {
    Ok(sqlx::query_as::<_, OrderAddress>(
        "SELECT id, created_time, updated_time, user_id, name, last_name, phone, email, \
         province, province_code, city, town, country, country_code, address_info, company, \
         postcode, status FROM order_addresses WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// 下单（`CreateOrder`）。
///
/// 事务边界与既有行为一致：地址、明细、订单在同一个事务里写；
/// 库存**不**扣（既有实现也没有在下单时扣库存）。
pub async fn create_order(
    pool: &MySqlPool,
    user_id: u32,
    mut input: CreateOrderInput,
) -> Result<TradeOrder, ModelError> {
    let config = order_config::config_get(pool).await.unwrap_or_else(|_| json!({}));
    let no_need_login = order_flag(&config, "no_need_login");
    let no_process = order_flag(&config, "no_process");
    let seller_percent = order_number(&config, "seller_percent");

    let user = user_row(pool, user_id).await?;
    if user.is_none() && !no_need_login {
        return Err(ModelError::invalid("请先登录"));
    }
    if input.details.is_empty() && input.goods_id == 0 {
        return Err(ModelError::invalid("请选择商品"));
    }
    if input.details.is_empty() {
        input.details = vec![OrderDetailInput {
            goods_id: input.goods_id,
            quantity: input.quantity,
            ..Default::default()
        }];
    }

    let discount = user_discount(pool, user.as_ref()).await;
    let kind = if input.kind.trim().is_empty() {
        ORDER_TYPE_GOODS.to_string()
    } else {
        input.kind.trim().to_string()
    };

    let mut tx = pool.begin().await?;

    // 收货地址：既有实现只在「不需要自动处理」时才把地址写库。
    let mut address_id = input.address_id;
    if !no_process {
        if let Some(address) = input.address.as_ref() {
            let saved = save_order_address_tx(&mut *tx, user_id, address).await?;
            address_id = saved.id;
        }
    }

    // 备注：为空时取商品标题（VIP 单取用户组名）。
    let mut remark = input.remark.trim().to_string();
    let mut seller_id: u32 = 0;
    if kind == ORDER_TYPE_VIP {
        if remark.is_empty() {
            if let Some(detail) = input.details.first() {
                let title: Option<String> =
                    sqlx::query_scalar("SELECT title FROM user_groups WHERE id = ?")
                        .bind(detail.goods_id as u32)
                        .fetch_optional(&mut *tx)
                        .await?;
                remark = title.unwrap_or_default();
            }
        }
    } else {
        for detail in &input.details {
            let archive = load_archive_tx(&mut *tx, detail.goods_id).await?;
            let Some(archive) = archive else {
                
                return Err(ModelError::invalid("内容不存在"));
            };
            if archive.user_id > 0 {
                if seller_id == 0 {
                    seller_id = archive.user_id;
                } else if seller_id != archive.user_id {
                    
                    return Err(ModelError::invalid("不支持跨店购买"));
                }
            }
        }
        if remark.is_empty() {
            if let Some(detail) = input.details.first() {
                if let Some(archive) = load_archive_tx(&mut *tx, detail.goods_id).await? {
                    remark = archive.title.clone();
                    if input.details.len() > 1 {
                        remark.push_str("...");
                    }
                }
            }
        }
    }

    // 订单主记录（单号在拿到自增 ID 后回填，与 AfterCreate 一致）。
    let ts = now();
    let temp_no = format!("tmp{ts}{}", rand::random::<u32>() % 10000);
    let result = sqlx::query(
        "INSERT INTO orders (created_time, updated_time, order_id, payment_id, user_id, \
         address_id, remark, `type`, status, refund_status, share_user_id, seller_id) \
         VALUES (?, ?, ?, '', ?, ?, ?, ?, 0, 0, ?, ?)",
    )
    .bind(ts)
    .bind(ts)
    .bind(&temp_no)
    .bind(user_id)
    .bind(address_id)
    .bind(&remark)
    .bind(&kind)
    .bind(user.as_ref().map(|item| item.parent_id).unwrap_or(0).max(0) as u32)
    .bind(seller_id)
    .execute(&mut *tx)
    .await?;
    let order_pk = result.last_insert_id() as u32;
    let order_no = make_biz_no(ts, order_pk);
    sqlx::query("UPDATE orders SET order_id = ? WHERE id = ?")
        .bind(&order_no)
        .bind(order_pk)
        .execute(&mut *tx)
        .await?;

    // 明细与金额。
    let mut amount: i64 = 0;
    let mut origin_amount: i64 = 0;
    for detail in &input.details {
        let Some(archive) = load_archive_tx(&mut *tx, detail.goods_id).await? else {
            
            return Err(ModelError::invalid("内容不存在"));
        };
        let quantity = if detail.quantity == 0 { 1 } else { detail.quantity };
        let origin_price = archive.price;
        let price = if discount > 0 {
            origin_price * discount / 100
        } else {
            origin_price
        };
        let detail_amount = price * i64::from(quantity);
        let origin_detail_amount = origin_price * i64::from(quantity);
        amount += detail_amount;
        origin_amount += origin_detail_amount;
        sqlx::query(
            "INSERT INTO order_details (created_time, updated_time, order_id, user_id, goods_id, \
             goods_sku_id, price, origin_price, amount, origin_amount, quantity, status) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)",
        )
        .bind(ts)
        .bind(ts)
        .bind(&order_no)
        .bind(user_id)
        .bind(detail.goods_id)
        .bind(detail.goods_sku_id)
        .bind(price)
        .bind(origin_price)
        .bind(detail_amount)
        .bind(origin_detail_amount)
        .bind(quantity)
        .execute(&mut *tx)
        .await?;
    }

    // 分销奖励（`CreateOrder` 的 share 计算）。
    let retailer = retailer_config::config_get(pool).await.unwrap_or_else(|_| json!({}));
    let become_retailer = order_flag(&retailer, "become_retailer");
    let allow_self = order_flag(&retailer, "allow_self");
    let mut share_user_id = user.as_ref().map(|item| item.parent_id).unwrap_or(0).max(0) as u32;
    let mut share_amount: i64 = 0;
    let mut share_parent_user_id: u32 = 0;
    let mut share_parent_amount: i64 = 0;
    let is_retailer = user.as_ref().map(|item| item.is_retailer).unwrap_or(0);
    if allow_self && (become_retailer || is_retailer == 1) {
        share_user_id = user_id;
    }
    if share_user_id > 0 {
        if let Ok(Some(share_user)) = user_row(pool, share_user_id).await {
            if become_retailer || share_user.is_retailer == 1 {
                let setting = group_setting(pool, share_user.group_id).await;
                let share_reward = group_int(&setting, "share_reward");
                if share_reward > 0 {
                    share_amount = amount * share_reward / 100;
                    let parent_reward = group_int(&setting, "parent_reward");
                    if share_user.parent_id > 0 && parent_reward > 0 {
                        if let Ok(Some(parent)) = user_row(pool, share_user.parent_id as u32).await {
                            if become_retailer || parent.is_retailer == 1 {
                                share_parent_amount = amount * parent_reward / 100;
                                share_parent_user_id = parent.id;
                            }
                        }
                    }
                }
            }
        }
    }

    // 卖家抽成。
    let mut seller_amount: i64 = 0;
    if seller_id > 0 && seller_percent > 0 {
        seller_amount = (amount - share_amount - share_parent_amount) * seller_percent / 100;
    }

    sqlx::query(
        "UPDATE orders SET amount = ?, origin_amount = ?, seller_amount = ?, share_amount = ?, \
         share_parent_amount = ?, share_parent_user_id = ?, share_user_id = ? WHERE id = ?",
    )
    .bind(amount)
    .bind(origin_amount)
    .bind(seller_amount)
    .bind(share_amount)
    .bind(share_parent_amount)
    .bind(share_parent_user_id)
    .bind(share_user_id)
    .bind(order_pk)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    let order = load_order(pool, &order_no).await?;
    order.ok_or_else(|| ModelError::invalid("订单不存在"))
}

/// 事务里读内容整行。
async fn load_archive_tx(
    tx: &mut MySqlConnection,
    id: i64,
) -> anyhow::Result<Option<super::archive::Archive>> {
    Ok(sqlx::query_as::<_, super::archive::Archive>("SELECT * FROM archives WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?)
}

/// 按订单号读订单主记录。
pub async fn load_order(
    pool: &MySqlPool,
    order_id: &str,
) -> anyhow::Result<Option<TradeOrder>> {
    Ok(sqlx::query_as::<_, TradeOrder>(
        "SELECT id, created_time, updated_time, order_id, payment_id, user_id, address_id, \
         remark, `type`, status, refund_status, origin_amount, amount, paid_time, end_time, \
         deliver_time, finished_time, discount_amount, user_discount, coupon_discount, \
         coupon_code_id, seller_id, seller_amount, share_user_id, share_parent_user_id, \
         share_amount, share_parent_amount, express_company, tracking_number \
         FROM orders WHERE order_id = ?",
    )
    .bind(order_id)
    .fetch_optional(pool)
    .await?)
}

/// 订单明细。
pub async fn order_details(
    pool: &MySqlPool,
    order_id: &str,
) -> anyhow::Result<Vec<OrderDetail>> {
    Ok(sqlx::query_as::<_, OrderDetail>(
        "SELECT id, created_time, updated_time, order_id, user_id, goods_id, goods_sku_id, price, \
         origin_price, amount, origin_amount, quantity, status FROM order_details \
         WHERE order_id = ?",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await?)
}

/// 订单列表（`GetOrderList`）。
pub async fn order_list(
    pool: &MySqlPool,
    user_id: u32,
    status: &str,
    kind: &str,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<OrderView>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;

    let mut where_sql = String::from(" WHERE 1 = 1");
    if user_id > 0 {
        where_sql.push_str(" AND user_id = ?");
    }
    if !kind.trim().is_empty() {
        where_sql.push_str(" AND `type` = ?");
    }
    if let Some(_) = status_value(status) {
        where_sql.push_str(" AND status = ?");
    }

    let count_sql = format!("SELECT COUNT(*) FROM orders{where_sql}");
    let mut counter = sqlx::query_scalar::<_, i64>(&count_sql);
    if user_id > 0 {
        counter = counter.bind(user_id);
    }
    if !kind.trim().is_empty() {
        counter = counter.bind(kind.trim());
    }
    if let Some(value) = status_value(status) {
        counter = counter.bind(value);
    }
    let total = counter.fetch_one(pool).await?;

    let list_sql = format!(
        "SELECT id, created_time, updated_time, order_id, payment_id, user_id, address_id, \
         remark, `type`, status, refund_status, origin_amount, amount, paid_time, end_time, \
         deliver_time, finished_time, discount_amount, user_discount, coupon_discount, \
         coupon_code_id, seller_id, seller_amount, share_user_id, share_parent_user_id, \
         share_amount, share_parent_amount, express_company, tracking_number \
         FROM orders{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?"
    );
    let mut query = sqlx::query_as::<_, TradeOrder>(&list_sql);
    if user_id > 0 {
        query = query.bind(user_id);
    }
    if !kind.trim().is_empty() {
        query = query.bind(kind.trim());
    }
    if let Some(value) = status_value(status) {
        query = query.bind(value);
    }
    let rows = query.bind(page_size).bind(offset).fetch_all(pool).await?;
    let views = attach_order_views(pool, rows).await?;
    Ok((total, views))
}

/// 订单详情（`GetOrderInfoByOrderId`）。
pub async fn order_detail(
    pool: &MySqlPool,
    order_id: &str,
) -> anyhow::Result<Option<OrderView>> {
    let Some(order) = load_order(pool, order_id).await? else {
        return Ok(None);
    };
    let views = attach_order_views(pool, vec![order]).await?;
    Ok(views.into_iter().next())
}

/// 给订单补上明细 / 地址 / 用户（`GetOrderList` 尾段）。
async fn attach_order_views(
    pool: &MySqlPool,
    orders: Vec<TradeOrder>,
) -> anyhow::Result<Vec<OrderView>> {
    if orders.is_empty() {
        return Ok(Vec::new());
    }
    let order_ids: Vec<String> = orders.iter().map(|item| item.order_id.clone()).collect();
    let mut detail_sql = String::from(
        "SELECT id, created_time, updated_time, order_id, user_id, goods_id, goods_sku_id, price, \
         origin_price, amount, origin_amount, quantity, status FROM order_details WHERE order_id IN (",
    );
    for (index, _) in order_ids.iter().enumerate() {
        if index > 0 {
            detail_sql.push(',');
        }
        detail_sql.push('?');
    }
    detail_sql.push(')');
    let mut detail_query = sqlx::query_as::<_, OrderDetail>(&detail_sql);
    for id in &order_ids {
        detail_query = detail_query.bind(id);
    }
    let details = detail_query.fetch_all(pool).await?;

    let mut goods_ids: Vec<i64> = Vec::new();
    for detail in &details {
        if detail.goods_id > 0 && !goods_ids.contains(&detail.goods_id) {
            goods_ids.push(detail.goods_id);
        }
    }
    let archives = load_archives(pool, &goods_ids).await?;

    let mut user_ids: Vec<u32> = Vec::new();
    for order in &orders {
        if order.user_id > 0 && !user_ids.contains(&order.user_id) {
            user_ids.push(order.user_id);
        }
        if order.share_user_id > 0 && !user_ids.contains(&order.share_user_id) {
            user_ids.push(order.share_user_id);
        }
        if order.share_parent_user_id > 0 && !user_ids.contains(&order.share_parent_user_id) {
            user_ids.push(order.share_parent_user_id);
        }
    }
    let users = user_briefs(pool, &user_ids).await?;

    let mut address_ids: Vec<u32> = Vec::new();
    for order in &orders {
        if order.address_id > 0 && !address_ids.contains(&order.address_id) {
            address_ids.push(order.address_id);
        }
    }
    let mut addresses: HashMap<u32, OrderAddress> = HashMap::new();
    for id in address_ids {
        if let Some(row) = order_address_by_id(pool, id).await? {
            addresses.insert(id, row);
        }
    }

    let mut views: Vec<OrderView> = Vec::with_capacity(orders.len());
    for order in orders {
        let mut items: Vec<OrderDetailView> = Vec::new();
        for detail in details.iter().filter(|item| item.order_id == order.order_id) {
            let goods = archives.get(&detail.goods_id).map(front_content::archive_json);
            items.push(OrderDetailView {
                detail: detail.clone(),
                goods,
                group: None,
            });
        }
        let share_user = if order.share_user_id > 0 {
            users.get(&order.share_user_id).cloned()
        } else {
            None
        };
        let parent_user = if order.share_parent_user_id > 0 {
            users.get(&order.share_parent_user_id).cloned()
        } else {
            None
        };
        views.push(OrderView {
            order_address: addresses.get(&order.address_id).cloned(),
            user: users.get(&order.user_id).cloned(),
            share_user,
            parent_user,
            is_updated: 0,
            details: items,
            order,
        });
    }
    Ok(views)
}

/// 取消订单（`SetOrderCanceled`）：退库存 + 支付单置为已取消。
pub async fn cancel_order(
    pool: &MySqlPool,
    user_id: u32,
    order_id: &str,
) -> Result<(), ModelError> {
    let mut tx = pool.begin().await?;
    let Some(order) = lock_order_tx(&mut *tx, order_id).await? else {
        
        return Err(ModelError::invalid("订单不存在"));
    };
    if order.user_id != user_id {
        
        return Err(ModelError::invalid("该订单不可操作"));
    }
    let ts = now();
    sqlx::query("UPDATE orders SET status = ?, finished_time = ? WHERE id = ?")
        .bind(ORDER_STATUS_CANCELED)
        .bind(ts)
        .bind(order.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE payments SET status = ?, updated_time = ? WHERE order_id = ?")
        .bind(ORDER_STATUS_CANCELED)
        .bind(ts)
        .bind(&order.order_id)
        .execute(&mut *tx)
        .await?;
    if order.kind != ORDER_TYPE_VIP {
        let details = sqlx::query_as::<_, OrderDetail>(
            "SELECT id, created_time, updated_time, order_id, user_id, goods_id, goods_sku_id, \
             price, origin_price, amount, origin_amount, quantity, status FROM order_details \
             WHERE order_id = ?",
        )
        .bind(&order.order_id)
        .fetch_all(&mut *tx)
        .await?;
        for detail in details {
            if detail.goods_id > 0 {
                sqlx::query("UPDATE archives SET stock = stock + ? WHERE id = ?")
                    .bind(detail.quantity)
                    .bind(detail.goods_id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(())
}

/// 事务里锁一行订单。
async fn lock_order_tx(
    tx: &mut MySqlConnection,
    order_id: &str,
) -> anyhow::Result<Option<TradeOrder>> {
    Ok(sqlx::query_as::<_, TradeOrder>(
        "SELECT id, created_time, updated_time, order_id, payment_id, user_id, address_id, \
         remark, `type`, status, refund_status, origin_amount, amount, paid_time, end_time, \
         deliver_time, finished_time, discount_amount, user_discount, coupon_discount, \
         coupon_code_id, seller_id, seller_amount, share_user_id, share_parent_user_id, \
         share_amount, share_parent_amount, express_company, tracking_number \
         FROM orders WHERE order_id = ? FOR UPDATE",
    )
    .bind(order_id)
    .fetch_optional(&mut *tx)
    .await?)
}

/// 申请退款（`ApplyOrderRefund`）。
pub async fn apply_order_refund(
    pool: &MySqlPool,
    user_id: u32,
    order_id: &str,
) -> Result<(), ModelError> {
    let mut tx = pool.begin().await?;
    let Some(order) = lock_order_tx(&mut *tx, order_id).await? else {
        
        return Err(ModelError::invalid("订单不存在"));
    };
    if order.user_id != user_id {
        
        return Err(ModelError::invalid("该订单不可操作"));
    }
    // 已经申请过就直接返回（既有实现是「有记录就当成功」）。
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM order_refunds WHERE order_id = ? LIMIT 1")
            .bind(&order.order_id)
            .fetch_optional(&mut *tx)
            .await?;
    if existing.is_some() {
        tx.commit().await.ok();
        return Ok(());
    }
    let ts = now();
    let refund = sqlx::query(
        "INSERT INTO order_refunds (created_time, updated_time, refund_id, order_id, detail_id, \
         user_id, amount, refund_time, status, error_times, last_time, remark) \
         VALUES (?, ?, '', ?, 0, ?, ?, 0, 0, 0, 0, ?)",
    )
    .bind(ts)
    .bind(ts)
    .bind(&order.order_id)
    .bind(user_id)
    .bind(order.amount)
    .bind("用户申请退款")
    .execute(&mut *tx)
    .await?;
    let refund_id = make_biz_no(ts, refund.last_insert_id() as u32);
    sqlx::query("UPDATE order_refunds SET refund_id = ? WHERE id = ?")
        .bind(&refund_id)
        .bind(refund.last_insert_id())
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE orders SET refund_status = ? WHERE id = ?")
        .bind(ORDER_STATUS_REFUNDING)
        .bind(order.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// 给某个用户记一笔佣金（结算时的分成）。
async fn grant_commission(
    tx: &mut MySqlConnection,
    user_id: u32,
    order_id: &str,
    order_amount: i64,
    amount: i64,
    action: i8,
    remark: &str,
) -> anyhow::Result<()> {
    if amount <= 0 || user_id == 0 {
        return Ok(());
    }
    let ts = now();
    sqlx::query(
        "INSERT INTO commissions (created_time, updated_time, user_id, order_id, order_amount, \
         amount, status, withdraw_id, remark) VALUES (?, ?, ?, ?, ?, ?, 0, 0, ?)",
    )
    .bind(ts)
    .bind(ts)
    .bind(user_id)
    .bind(order_id)
    .bind(order_amount)
    .bind(amount)
    .bind(remark)
    .execute(&mut *tx)
    .await?;

    // 锁住会员行再改余额，避免并发把余额算错。
    let balance: Option<i64> =
        sqlx::query_scalar("SELECT balance FROM users WHERE id = ? FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(_) = balance else {
        return Ok(());
    };
    sqlx::query("UPDATE users SET balance = balance + ? WHERE id = ?")
        .bind(amount)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let after: i64 = sqlx::query_scalar("SELECT balance FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
    insert_finance(
        tx,
        user_id,
        FINANCE_INCOME,
        amount,
        after,
        action,
        order_id,
        remark,
    )
    .await?;
    sqlx::query(
        "UPDATE users SET total_reward = (SELECT CAST(COALESCE(SUM(amount), 0) AS SIGNED) \
         FROM commissions WHERE user_id = ?) WHERE id = ?",
    )
    .bind(user_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// 完成订单（`SetOrderFinished`）：结算卖家 / 分销佣金。
pub async fn finish_order(
    pool: &MySqlPool,
    user_id: u32,
    order_id: &str,
) -> Result<(), ModelError> {
    let mut tx = pool.begin().await?;
    let Some(order) = lock_order_tx(&mut *tx, order_id).await? else {
        
        return Err(ModelError::invalid("订单不存在"));
    };
    if order.user_id != user_id {
        
        return Err(ModelError::invalid("该订单不可操作"));
    }
    let ts = now();
    sqlx::query("UPDATE orders SET status = ?, finished_time = ? WHERE id = ?")
        .bind(ORDER_STATUS_COMPLETED)
        .bind(ts)
        .bind(order.id)
        .execute(&mut *tx)
        .await?;

    grant_commission(
        &mut *tx,
        order.seller_id,
        &order.order_id,
        order.amount,
        order.seller_amount,
        FINANCE_ACTION_SALE,
        "销售收益",
    )
    .await?;
    if order.share_amount > 0 {
        grant_commission(
            &mut *tx,
            order.share_user_id,
            &order.order_id,
            order.amount,
            order.share_amount,
            FINANCE_ACTION_COMMISSION,
            "",
        )
        .await?;
    }
    if order.share_parent_amount > 0 {
        grant_commission(
            &mut *tx,
            order.share_parent_user_id,
            &order.order_id,
            order.amount,
            order.share_parent_amount,
            FINANCE_ACTION_COMMISSION,
            "",
        )
        .await?;
    }

    // 会员组订单：顺延会员有效期（`SetOrderFinished` 的 VIP 分支）。
    if order.kind == ORDER_TYPE_VIP {
        let details = sqlx::query_as::<_, OrderDetail>(
            "SELECT id, created_time, updated_time, order_id, user_id, goods_id, goods_sku_id, \
             price, origin_price, amount, origin_amount, quantity, status FROM order_details \
             WHERE order_id = ? LIMIT 1",
        )
        .bind(&order.order_id)
        .fetch_one(&mut *tx)
        .await?;
        let group_id = details.goods_id as u32;
        let setting = group_setting_tx(&mut *tx, group_id).await?;
        let expire_day = group_int(&setting, "expire_day");
        let expire_day = if expire_day == 0 { 365 } else { expire_day };
        let quantity = details.quantity.max(1) as i64;
        let current: Option<i64> = sqlx::query_scalar("SELECT expire_time FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
        let mut start = current.unwrap_or(0);
        if start < ts {
            start = ts;
        }
        let end = if expire_day == 365 {
            // 既有实现用 AddDate(years) 加整年，这里按 365 天/年折算（差异只在闰年）。
            start + quantity * 365 * 86400
        } else if expire_day == 30 {
            start + quantity * 30 * 86400
        } else {
            start + expire_day * quantity * 86400
        };
        sqlx::query("UPDATE users SET expire_time = ?, group_id = ? WHERE id = ?")
            .bind(end)
            .bind(group_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// 事务里读用户组配置。
async fn group_setting_tx(
    tx: &mut MySqlConnection,
    group_id: u32,
) -> anyhow::Result<Value> {
    let raw: Option<String> = sqlx::query_scalar("SELECT setting FROM user_groups WHERE id = ?")
        .bind(group_id)
        .fetch_optional(&mut *tx)
        .await?;
    Ok(raw
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(|value| value.is_object())
        .unwrap_or_else(|| json!({})))
}

// ---------------------------------------------------------------------------
// 支付
// ---------------------------------------------------------------------------

/// 按订单号取支付单。
pub async fn payment_by_order_id(
    pool: &MySqlPool,
    order_id: &str,
) -> anyhow::Result<Option<Payment>> {
    Ok(sqlx::query_as::<_, Payment>(
        "SELECT id, created_time, updated_time, payment_id, terrace_id, user_id, order_id, \
         amount, status, payment_account_id, pay_way, paid_time, remark, buyer_id, buyer_info \
         FROM payments WHERE order_id = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(order_id)
    .fetch_optional(pool)
    .await?)
}

/// 按支付单号取支付单。
pub async fn payment_by_id(
    pool: &MySqlPool,
    payment_id: &str,
) -> anyhow::Result<Option<Payment>> {
    Ok(sqlx::query_as::<_, Payment>(
        "SELECT id, created_time, updated_time, payment_id, terrace_id, user_id, order_id, \
         amount, status, payment_account_id, pay_way, paid_time, remark, buyer_id, buyer_info \
         FROM payments WHERE payment_id = ?",
    )
    .bind(payment_id)
    .fetch_optional(pool)
    .await?)
}

/// 按网关交易号取支付单（`GetPaymentInfoByTerraceId`）。
///
/// 支付宝 / 微信回调里带的是支付单号，PayPal 返回页带的是 PayPal 订单号
/// （= `payments.terrace_id`），所以两种都要能查。
pub async fn payment_by_terrace_id(
    pool: &MySqlPool,
    terrace_id: &str,
) -> anyhow::Result<Option<Payment>> {
    Ok(sqlx::query_as::<_, Payment>(
        "SELECT id, created_time, updated_time, payment_id, terrace_id, user_id, order_id, \
         amount, status, payment_account_id, pay_way, paid_time, remark, buyer_id, buyer_info \
         FROM payments WHERE terrace_id = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(terrace_id)
    .fetch_optional(pool)
    .await?)
}

/// 可用支付方式（`payment_accounts` 里启用状态的 pay_way 去重）。
pub async fn payment_ways(pool: &MySqlPool) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT pay_way FROM payment_accounts WHERE status = 1",
    )
    .fetch_all(pool)
    .await?)
}

/// 生成支付单（`GeneratePayment`）：已存在则复用。
pub async fn generate_payment(
    pool: &MySqlPool,
    order: &TradeOrder,
    pay_way: &str,
) -> Result<Payment, ModelError> {
    if let Some(existing) = payment_by_order_id(pool, &order.order_id).await? {
        return Ok(existing);
    }
    let ts = now();
    let result = sqlx::query(
        "INSERT INTO payments (created_time, updated_time, payment_id, terrace_id, user_id, \
         order_id, amount, status, payment_account_id, pay_way, paid_time, remark, buyer_id, \
         buyer_info) VALUES (?, ?, '', '', ?, ?, ?, 0, 0, ?, 0, ?, '', '')",
    )
    .bind(ts)
    .bind(ts)
    .bind(order.user_id)
    .bind(&order.order_id)
    .bind(order.amount)
    .bind(pay_way)
    .bind(&order.remark)
    .execute(pool)
    .await?;
    let pk = result.last_insert_id() as u32;
    let payment_no = make_biz_no(ts, pk);
    sqlx::query("UPDATE payments SET payment_id = ? WHERE id = ?")
        .bind(&payment_no)
        .bind(pk)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE orders SET payment_id = ? WHERE id = ?")
        .bind(&payment_no)
        .bind(order.id)
        .execute(pool)
        .await?;
    let payment = payment_by_id(pool, &payment_no)
        .await?
        .ok_or_else(|| ModelError::invalid("无法创建支付订单"))?;
    Ok(payment)
}

/// 余额支付（`createBalancePayment`）：余额 → 订单，同事务内扣款 + 记流水。
pub async fn balance_pay(
    pool: &MySqlPool,
    order: &TradeOrder,
    payment: &Payment,
) -> Result<(), ModelError> {
    let mut tx = pool.begin().await?;
    let balance: Option<i64> =
        sqlx::query_scalar("SELECT balance FROM users WHERE id = ? FOR UPDATE")
            .bind(order.user_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(balance) = balance else {
        
        return Err(ModelError::invalid("余额不足"));
    };
    if balance < payment.amount {
        
        return Err(ModelError::invalid("余额不足"));
    }
    let ts = now();
    sqlx::query(
        "UPDATE payments SET pay_way = ?, paid_time = ?, terrace_id = ?, status = 1, \
         updated_time = ? WHERE id = ?",
    )
    .bind(PAY_WAY_BALANCE)
    .bind(ts)
    .bind(ts.to_string())
    .bind(ts)
    .bind(payment.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE orders SET payment_id = ? WHERE id = ?")
        .bind(&payment.payment_id)
        .bind(order.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE users SET balance = balance - ? WHERE id = ? AND balance >= ?")
        .bind(payment.amount)
        .bind(order.user_id)
        .bind(payment.amount)
        .execute(&mut *tx)
        .await?;
    let after: i64 = sqlx::query_scalar("SELECT balance FROM users WHERE id = ?")
        .bind(order.user_id)
        .fetch_one(&mut *tx)
        .await?;
    insert_finance(
        &mut *tx,
        order.user_id,
        FINANCE_OUTPUT,
        payment.amount,
        after,
        FINANCE_ACTION_BUY,
        &order.order_id,
        "",
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 支付成功后的收尾（`SuccessPaidOrder`）。
pub async fn success_paid_order(
    pool: &MySqlPool,
    order: &TradeOrder,
    _payment: &Payment,
) -> Result<(), ModelError> {
    if order.status == ORDER_STATUS_PAID {
        return Ok(());
    }
    let ts = now();
    sqlx::query("UPDATE orders SET paid_time = ?, status = ? WHERE id = ? AND status = ?")
        .bind(ts)
        .bind(ORDER_STATUS_PAID)
        .bind(order.id)
        .bind(order.status)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE users SET order_count = order_count + 1 WHERE id = ?")
        .bind(order.user_id)
        .execute(pool)
        .await?;
    let config = order_config::config_get(pool).await.unwrap_or_else(|_| json!({}));
    if order_flag(&config, "no_process") || order.kind == ORDER_TYPE_VIP {
        finish_order(pool, order.user_id, &order.order_id).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 店铺 / 保证金
// ---------------------------------------------------------------------------

/// 读店铺（不存在返回 `None`）。
pub async fn shop_by_user(pool: &MySqlPool, user_id: u32) -> anyhow::Result<Option<Shop>> {
    Ok(sqlx::query_as::<_, Shop>(
        "SELECT id, created_time, updated_time, user_id, name, logo, banner, intro, notice, \
         contact, theme_color, decoration, status, deposit, deposit_total, product_count, \
         order_count, sale_amount, '' AS logo_url, '' AS banner_url \
         FROM shops WHERE user_id = ? LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?)
}

/// 读店铺并补全图片地址。
async fn shop_with_urls(pool: &MySqlPool, user_id: u32) -> anyhow::Result<Option<Shop>> {
    let Some(mut shop) = shop_by_user(pool, user_id).await? else {
        return Ok(None);
    };
    let storage = storage_url(pool).await;
    shop.logo_url = front_content::join_storage_url(&storage, &shop.logo);
    shop.banner_url = front_content::join_storage_url(&storage, &shop.banner);
    Ok(Some(shop))
}

/// 取店铺，没有则创建默认店铺（`EnsureShop`）。
pub async fn ensure_shop(pool: &MySqlPool, user_id: u32) -> Result<Shop, ModelError> {
    if user_id == 0 {
        return Err(ModelError::invalid("请先登录"));
    }
    if let Some(shop) = shop_with_urls(pool, user_id).await? {
        return Ok(shop);
    }
    let user = user_row(pool, user_id).await?;
    let name = user.as_ref().map(|item| item.user_name.clone()).unwrap_or_default();
    let decoration = serde_json::to_string(&ShopDecoration::default()).unwrap_or_default();
    let ts = now();
    let result = sqlx::query(
        "INSERT INTO shops (created_time, updated_time, user_id, name, logo, banner, intro, \
         notice, contact, theme_color, decoration, status, deposit, deposit_total, \
         product_count, order_count, sale_amount) \
         VALUES (?, ?, ?, ?, '', '', ?, '', '', '', ?, 1, 0, 0, 0, 0, 0)",
    )
    .bind(ts)
    .bind(ts)
    .bind(user_id)
    .bind(&name)
    .bind("欢迎光临本店")
    .bind(&decoration)
    .execute(pool)
    .await;
    if result.is_err() {
        // 并发下可能已被其它请求创建
        if let Some(shop) = shop_with_urls(pool, user_id).await? {
            return Ok(shop);
        }
    }
    let shop = shop_with_urls(pool, user_id)
        .await?
        .ok_or_else(|| ModelError::invalid("店铺创建失败"))?;
    Ok(shop)
}

/// 保存店铺资料（`SaveShop`）。
pub async fn save_shop(
    pool: &MySqlPool,
    user_id: u32,
    input: &ShopInput,
) -> Result<Shop, ModelError> {
    let mut shop = ensure_shop(pool, user_id).await?;
    if !input.name.trim().is_empty() {
        shop.name = input.name.trim().to_string();
    }
    shop.logo = input.logo.clone();
    shop.banner = input.banner.clone();
    shop.intro = input.intro.clone();
    shop.notice = input.notice.clone();
    shop.contact = input.contact.clone();
    shop.theme_color = input.theme_color.clone();
    if !input.decoration.trim().is_empty() {
        // 只接受合法 JSON，避免把装修配置写坏
        let parsed: ShopDecoration = serde_json::from_str(input.decoration.trim())
            .map_err(|_| ModelError::invalid("装修配置格式错误"))?;
        shop.decoration = Some(serde_json::to_string(&parsed).unwrap_or_default());
    }
    sqlx::query(
        "UPDATE shops SET updated_time = ?, name = ?, logo = ?, banner = ?, intro = ?, notice = ?, \
         contact = ?, theme_color = ?, decoration = ? WHERE id = ?",
    )
    .bind(now())
    .bind(&shop.name)
    .bind(&shop.logo)
    .bind(&shop.banner)
    .bind(&shop.intro)
    .bind(&shop.notice)
    .bind(&shop.contact)
    .bind(&shop.theme_color)
    .bind(shop.decoration.as_deref())
    .bind(shop.id)
    .execute(pool)
    .await?;
    let storage = storage_url(pool).await;
    shop.logo_url = front_content::join_storage_url(&storage, &shop.logo);
    shop.banner_url = front_content::join_storage_url(&storage, &shop.banner);
    Ok(shop)
}

/// 解析店铺装修配置（`GetShopDecoration`）。
pub fn shop_decoration(shop: &Shop) -> ShopDecoration {
    let mut decoration = ShopDecoration::default();
    if let Some(raw) = shop.decoration.as_deref() {
        if let Ok(parsed) = serde_json::from_str::<ShopDecoration>(raw) {
            decoration = parsed;
        }
    }
    if decoration.modules.is_empty() {
        decoration.modules = default_modules();
    }
    if !shop.theme_color.is_empty() && decoration.theme_color.is_empty() {
        decoration.theme_color = shop.theme_color.clone();
    }
    decoration
}

/// 缴纳保证金：余额 → 保证金（`PayDeposit`）。
pub async fn pay_deposit(
    pool: &MySqlPool,
    user_id: u32,
    amount: i64,
) -> Result<Shop, ModelError> {
    if amount <= 0 {
        return Err(ModelError::invalid("金额不正确"));
    }
    let shop = ensure_shop(pool, user_id).await?;
    let mut tx = pool.begin().await?;
    let balance: Option<i64> =
        sqlx::query_scalar("SELECT balance FROM users WHERE id = ? FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(balance) = balance else {
        
        return Err(ModelError::invalid("余额不足"));
    };
    if balance < amount {
        
        return Err(ModelError::invalid("余额不足"));
    }
    sqlx::query("UPDATE users SET balance = balance - ? WHERE id = ? AND balance >= ?")
        .bind(amount)
        .bind(user_id)
        .bind(amount)
        .execute(&mut *tx)
        .await?;
    let after: i64 = sqlx::query_scalar("SELECT balance FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
    insert_finance(
        &mut *tx,
        user_id,
        FINANCE_OUTPUT,
        amount,
        after,
        FINANCE_ACTION_DEPOSIT,
        "",
        "缴纳店铺保证金",
    )
    .await?;
    sqlx::query("UPDATE shops SET deposit = deposit + ?, deposit_total = deposit_total + ? WHERE id = ?")
        .bind(amount)
        .bind(amount)
        .bind(shop.id)
        .execute(&mut *tx)
        .await?;
    let deposit: i64 = sqlx::query_scalar("SELECT deposit FROM shops WHERE id = ?")
        .bind(shop.id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO shop_deposit_logs (created_time, updated_time, shop_id, user_id, amount, \
         after_amount, `type`, remark) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(now())
    .bind(now())
    .bind(shop.id)
    .bind(user_id)
    .bind(amount)
    .bind(deposit)
    .bind(DEPOSIT_TYPE_PAY)
    .bind("缴纳店铺保证金")
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    shop_with_urls(pool, user_id)
        .await?
        .ok_or_else(|| ModelError::invalid("店铺不存在"))
}

/// 退还保证金：保证金 → 余额（`RefundDeposit`）。
pub async fn refund_deposit(
    pool: &MySqlPool,
    user_id: u32,
    amount: i64,
) -> Result<Shop, ModelError> {
    if amount <= 0 {
        return Err(ModelError::invalid("金额不正确"));
    }
    let Some(shop) = shop_by_user(pool, user_id).await? else {
        return Err(ModelError::invalid("店铺不存在"));
    };
    let mut tx = pool.begin().await?;
    let deposit: Option<i64> = sqlx::query_scalar("SELECT deposit FROM shops WHERE id = ? FOR UPDATE")
        .bind(shop.id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(deposit) = deposit else {
        
        return Err(ModelError::invalid("店铺不存在"));
    };
    if deposit < amount {
        
        return Err(ModelError::invalid("保证金不足"));
    }
    sqlx::query("UPDATE shops SET deposit = deposit - ? WHERE id = ? AND deposit >= ?")
        .bind(amount)
        .bind(shop.id)
        .bind(amount)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE users SET balance = balance + ? WHERE id = ?")
        .bind(amount)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let after: i64 = sqlx::query_scalar("SELECT balance FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
    insert_finance(
        &mut *tx,
        user_id,
        FINANCE_INCOME,
        amount,
        after,
        FINANCE_ACTION_DEPOSIT,
        "",
        "退还店铺保证金",
    )
    .await?;
    let deposit_after: i64 = sqlx::query_scalar("SELECT deposit FROM shops WHERE id = ?")
        .bind(shop.id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO shop_deposit_logs (created_time, updated_time, shop_id, user_id, amount, \
         after_amount, `type`, remark) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(now())
    .bind(now())
    .bind(shop.id)
    .bind(user_id)
    .bind(-amount)
    .bind(deposit_after)
    .bind(DEPOSIT_TYPE_REFUND)
    .bind("退还店铺保证金")
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    shop_with_urls(pool, user_id)
        .await?
        .ok_or_else(|| ModelError::invalid("店铺不存在"))
}

/// 保证金流水。
pub async fn deposit_logs(
    pool: &MySqlPool,
    user_id: u32,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<ShopDepositLog>)> {
    if user_id == 0 {
        return Ok((0, Vec::new()));
    }
    let page = page.max(1);
    let page_size = if page_size < 1 || page_size > POINT_LOG_LIMIT {
        POINT_LOG_LIMIT
    } else {
        page_size
    };
    let offset = (page - 1) * page_size;
    let total = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM shop_deposit_logs WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as::<_, ShopDepositLog>(
        "SELECT id, created_time, updated_time, shop_id, user_id, amount, after_amount, `type`, \
         remark FROM shop_deposit_logs WHERE user_id = ? ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok((total, rows))
}

/// 已售订单（`seller_id = 当前用户`）。
pub async fn seller_orders(
    pool: &MySqlPool,
    seller_id: u32,
    status: &str,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<OrderView>)> {
    if seller_id == 0 {
        return Ok((0, Vec::new()));
    }
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;
    let mut where_sql = String::from(" WHERE seller_id = ?");
    if let Some(_) = status_value(status) {
        where_sql.push_str(" AND status = ?");
    }
    let count_sql = format!("SELECT COUNT(*) FROM orders{where_sql}");
    let mut counter = sqlx::query_scalar::<_, i64>(&count_sql);
    counter = counter.bind(seller_id);
    if let Some(value) = status_value(status) {
        counter = counter.bind(value);
    }
    let total = counter.fetch_one(pool).await?;

    let list_sql = format!(
        "SELECT id, created_time, updated_time, order_id, payment_id, user_id, address_id, \
         remark, `type`, status, refund_status, origin_amount, amount, paid_time, end_time, \
         deliver_time, finished_time, discount_amount, user_discount, coupon_discount, \
         coupon_code_id, seller_id, seller_amount, share_user_id, share_parent_user_id, \
         share_amount, share_parent_amount, express_company, tracking_number \
         FROM orders{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?"
    );
    let mut query = sqlx::query_as::<_, TradeOrder>(&list_sql);
    query = query.bind(seller_id);
    if let Some(value) = status_value(status) {
        query = query.bind(value);
    }
    let rows = query.bind(page_size).bind(offset).fetch_all(pool).await?;
    let views = attach_order_views(pool, rows).await?;
    Ok((total, views))
}

/// 卖家统计：订单数、销售额、待发货、今日成交。
pub async fn seller_stats(pool: &MySqlPool, seller_id: u32) -> anyhow::Result<SellerStats> {
    if seller_id == 0 {
        return Ok(SellerStats::default());
    }
    let mut stats = SellerStats::default();
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM orders WHERE seller_id = ? AND status >= 0")
            .bind(seller_id)
            .fetch_one(pool)
            .await?;
    stats.order_count = count;
    let amount: i64 = sqlx::query_scalar(
        "SELECT CAST(COALESCE(SUM(seller_amount), 0) AS SIGNED) FROM orders \
         WHERE seller_id = ? AND status >= 0",
    )
    .bind(seller_id)
    .fetch_one(pool)
    .await?;
    stats.sale_amount = amount;
    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM orders WHERE seller_id = ? AND status = ?")
            .bind(seller_id)
            .bind(ORDER_STATUS_PAID)
            .fetch_one(pool)
            .await?;
    stats.pending = pending;
    // 今日成交：既有实现这里恒为 0（未赋值），Rust 版按当日已支付订单的卖家分成求和。
    let today_start = chrono::Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| naive.and_local_timezone(chrono::Local).single())
        .map(|time| time.timestamp())
        .unwrap_or(0);
    let today: i64 = sqlx::query_scalar(
        "SELECT CAST(COALESCE(SUM(seller_amount), 0) AS SIGNED) FROM orders \
         WHERE seller_id = ? AND status >= 0 AND paid_time >= ?",
    )
    .bind(seller_id)
    .bind(today_start)
    .fetch_one(pool)
    .await?;
    stats.today_amount = today;
    Ok(stats)
}

/// 收到的评价（他人对我发布内容的评论）。
pub async fn received_comments(
    pool: &MySqlPool,
    user_id: u32,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<ReceivedComment>)> {
    if user_id == 0 {
        return Ok((0, Vec::new()));
    }
    let page = page.max(1);
    let page_size = if page_size < 1 || page_size > POINT_LOG_LIMIT {
        POINT_LOG_LIMIT
    } else {
        page_size
    };
    let offset = (page - 1) * page_size;
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM comments c INNER JOIN archives a ON a.id = c.archive_id \
         WHERE a.user_id = ? AND c.status = 1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    let rows = sqlx::query_as::<_, ReceivedComment>(
        "SELECT c.id, c.created_time, c.updated_time, c.archive_id, c.user_id, c.user_name, \
         c.email, c.ip, c.vote_count, c.content, c.parent_id, c.to_uid, c.status, c.rating, \
         IFNULL(a.title, '') AS item_title \
         FROM comments c INNER JOIN archives a ON a.id = c.archive_id \
         WHERE a.user_id = ? AND c.status = 1 ORDER BY c.id DESC LIMIT ? OFFSET ?",
    )
    .bind(user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok((count, rows))
}

/// 积分流水。
pub async fn point_logs(
    pool: &MySqlPool,
    user_id: u32,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<PointLog>)> {
    if user_id == 0 {
        return Ok((0, Vec::new()));
    }
    let page = page.max(1);
    let page_size = if page_size < 1 || page_size > POINT_LOG_LIMIT {
        POINT_LOG_LIMIT
    } else {
        page_size
    };
    let offset = (page - 1) * page_size;
    let total = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM point_logs WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as::<_, PointLog>(
        "SELECT id, created_time, updated_time, user_id, points, after_points, `type`, remark \
         FROM point_logs WHERE user_id = ? ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok((total, rows))
}

// ---------------------------------------------------------------------------
// 分销
// ---------------------------------------------------------------------------

/// 分销员资料（`ApiGetRetailerInfo` 返回的会员对象）。
pub async fn retailer_info(
    pool: &MySqlPool,
    retailer_id: u32,
) -> anyhow::Result<Option<TradeUser>> {
    user_row(pool, retailer_id).await
}

/// 分销统计（`ApiGetRetailerStatistics`）。
pub async fn retailer_statistics(
    pool: &MySqlPool,
    user_id: u32,
) -> anyhow::Result<RetailerStats> {
    let mut stats = RetailerStats::default();
    if user_id == 0 {
        return Ok(stats);
    }
    let can: i64 = sqlx::query_scalar(
        "SELECT CAST(COALESCE(SUM(amount), 0) AS SIGNED) FROM commissions \
         WHERE user_id = ? AND status = ?",
    )
    .bind(user_id)
    .bind(COMMISSION_STATUS_WAIT)
    .fetch_one(pool)
    .await?;
    stats.can_withdraw_amount = can;
    let paid: i64 = sqlx::query_scalar(
        "SELECT CAST(COALESCE(SUM(amount), 0) AS SIGNED) FROM commissions \
         WHERE user_id = ? AND status = ?",
    )
    .bind(user_id)
    .bind(COMMISSION_STATUS_PAID)
    .fetch_one(pool)
    .await?;
    stats.paid_withdraw_amount = paid;
    // 未结算：既有实现统计的是这些订单的订单总额（不是佣金额），这里保持一致。
    let unfinished: i64 = sqlx::query_scalar(
        "SELECT CAST(COALESCE(SUM(amount), 0) AS SIGNED) FROM orders \
         WHERE (share_user_id = ? OR share_parent_user_id = ?) AND status IN (?, ?)",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(ORDER_STATUS_PAID)
    .bind(ORDER_STATUS_DELIVERING)
    .fetch_one(pool)
    .await?;
    stats.unfinished_withdraw_amount = unfinished;
    let members: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE parent_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    stats.member_count = members;
    Ok(stats)
}

/// 分销订单（`share_user_id` 或 `share_parent_user_id` 是本人）。
pub async fn retailer_orders(
    pool: &MySqlPool,
    user_id: u32,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<OrderView>)> {
    if user_id == 0 {
        return Ok((0, Vec::new()));
    }
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;
    let where_sql = " WHERE (share_user_id = ? OR share_parent_user_id = ?)";
    let total = sqlx::query_scalar::<_, i64>(&format!(
        "SELECT COUNT(*) FROM orders{where_sql}"
    ))
    .bind(user_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    let rows = sqlx::query_as::<_, TradeOrder>(&format!(
        "SELECT id, created_time, updated_time, order_id, payment_id, user_id, address_id, \
         remark, `type`, status, refund_status, origin_amount, amount, paid_time, end_time, \
         deliver_time, finished_time, discount_amount, user_discount, coupon_discount, \
         coupon_code_id, seller_id, seller_amount, share_user_id, share_parent_user_id, \
         share_amount, share_parent_amount, express_company, tracking_number \
         FROM orders{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?"
    ))
    .bind(user_id)
    .bind(user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let views = attach_order_views(pool, rows).await?;
    Ok((total, views))
}

/// 分销提现记录。
pub async fn retailer_withdraws(
    pool: &MySqlPool,
    user_id: u32,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<UserWithdraw>)> {
    if user_id == 0 {
        return Ok((0, Vec::new()));
    }
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;
    let total =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM user_withdraws WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(pool)
            .await?;
    let rows = sqlx::query_as::<_, UserWithdraw>(
        "SELECT id, created_time, updated_time, user_id, amount, success_time, withdraw_way, \
         status, error_times, last_time, remark FROM user_withdraws WHERE user_id = ? \
         ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok((total, rows))
}

/// 分销佣金记录。
pub async fn retailer_commissions(
    pool: &MySqlPool,
    user_id: u32,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Commission>)> {
    if user_id == 0 {
        return Ok((0, Vec::new()));
    }
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;
    let total =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM commissions WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(pool)
            .await?;
    let rows = sqlx::query_as::<_, Commission>(
        "SELECT id, created_time, updated_time, user_id, order_id, order_amount, amount, status, \
         withdraw_id, remark FROM commissions WHERE user_id = ? ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok((total, rows))
}

/// 我的团队成员（`parent_id = 本人`）。
pub async fn retailer_members(
    pool: &MySqlPool,
    user_id: u32,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<TradeUser>)> {
    if user_id == 0 {
        return Ok((0, Vec::new()));
    }
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;
    let total = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE parent_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as::<_, TradeUser>(
        "SELECT id, created_time, user_name, real_name, avatar_url, introduce, email, phone, \
         group_id, parent_id, is_retailer, balance, total_reward, order_count, points, status \
         FROM users WHERE parent_id = ? ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok((total, rows))
}

/// 申请提现佣金（`RetailerApplyWithdraw`）：把未提现佣金打包成一笔提现。
pub async fn retailer_apply_withdraw(
    pool: &MySqlPool,
    user_id: u32,
) -> Result<UserWithdraw, ModelError> {
    if user_id == 0 {
        return Err(ModelError::invalid("请先登录"));
    }
    let commissions = sqlx::query_as::<_, Commission>(
        "SELECT id, created_time, updated_time, user_id, order_id, order_amount, amount, status, \
         withdraw_id, remark FROM commissions WHERE user_id = ? AND status = ?",
    )
    .bind(user_id)
    .bind(COMMISSION_STATUS_WAIT)
    .fetch_all(pool)
    .await?;
    let total: i64 = commissions.iter().map(|item| item.amount).sum();
    if total <= 0 {
        return Err(ModelError::invalid("没有可提现金额"));
    }
    // 低于 2 元不可提现到微信
    if total < WITHDRAW_MIN_AMOUNT {
        return Err(ModelError::invalid("低于2元不可提现到微信"));
    }

    let mut tx = pool.begin().await?;
    let ts = now();
    let result = sqlx::query(
        "INSERT INTO user_withdraws (created_time, updated_time, user_id, amount, success_time, \
         withdraw_way, status, error_times, last_time, remark) \
         VALUES (?, ?, ?, ?, 0, ?, ?, 0, 0, '')",
    )
    .bind(ts)
    .bind(ts)
    .bind(user_id)
    .bind(total)
    .bind(WITHDRAW_WAY_WECHAT)
    .bind(WITHDRAW_STATUS_WAITING)
    .execute(&mut *tx)
    .await?;
    let withdraw_id = result.last_insert_id() as u32;
    for item in &commissions {
        sqlx::query(
            "UPDATE commissions SET withdraw_id = ?, status = ?, updated_time = ? \
             WHERE id = ? AND status = ?",
        )
        .bind(withdraw_id)
        .bind(COMMISSION_STATUS_PAID)
        .bind(ts)
        .bind(item.id)
        .bind(COMMISSION_STATUS_WAIT)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    sqlx::query_as::<_, UserWithdraw>(
        "SELECT id, created_time, updated_time, user_id, amount, success_time, withdraw_way, \
         status, error_times, last_time, remark FROM user_withdraws WHERE id = ?",
    )
    .bind(withdraw_id)
    .fetch_one(pool)
    .await
    .map_err(ModelError::from)
}

/// 更新分销员真实姓名（`UpdateUserRealName`）。
pub async fn update_real_name(
    pool: &MySqlPool,
    user_id: u32,
    real_name: &str,
) -> Result<(), ModelError> {
    sqlx::query("UPDATE users SET real_name = ?, updated_time = ? WHERE id = ?")
        .bind(real_name.trim())
        .bind(now())
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 余额提现 / 充值
// ---------------------------------------------------------------------------

/// 提现备注：`方式|姓名|账号|用户备注`（后台能直接看到收款信息）。
pub fn build_withdraw_remark(way: i8, real_name: &str, account: &str, remark: &str) -> String {
    let way_name = match way {
        WITHDRAW_WAY_BANK => "银行卡",
        WITHDRAW_WAY_WECHAT => "微信",
        _ => "支付宝",
    };
    let mut text = format!("{way_name}|{real_name}|{account}");
    let remark = remark.trim();
    if !remark.is_empty() {
        text.push('|');
        text.push_str(remark);
    }
    text
}

/// 余额提现申请（`ApplyBalanceWithdraw`）：余额 → 提现申请。
pub async fn apply_balance_withdraw(
    pool: &MySqlPool,
    user_id: u32,
    input: &WithdrawInput,
) -> Result<UserWithdraw, ModelError> {
    if user_id == 0 {
        return Err(ModelError::invalid("请先登录"));
    }
    if input.amount <= 0 {
        return Err(ModelError::invalid("请输入金额"));
    }
    let account = input.account.trim();
    let real_name = input.real_name.trim();
    if account.is_empty() || real_name.is_empty() {
        return Err(ModelError::invalid("请填写提现账户"));
    }
    let way = if input.way != WITHDRAW_WAY_ALIPAY
        && input.way != WITHDRAW_WAY_BANK
        && input.way != WITHDRAW_WAY_WECHAT
    {
        WITHDRAW_WAY_ALIPAY
    } else {
        input.way
    };

    let mut tx = pool.begin().await?;
    let balance: Option<i64> =
        sqlx::query_scalar("SELECT balance FROM users WHERE id = ? FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(balance) = balance else {
        
        return Err(ModelError::invalid("余额不足"));
    };
    if balance < input.amount {
        
        return Err(ModelError::invalid("余额不足"));
    }
    sqlx::query("UPDATE users SET balance = balance - ? WHERE id = ? AND balance >= ?")
        .bind(input.amount)
        .bind(user_id)
        .bind(input.amount)
        .execute(&mut *tx)
        .await?;
    let after: i64 = sqlx::query_scalar("SELECT balance FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;

    let remark = build_withdraw_remark(way, real_name, account, &input.remark);
    let ts = now();
    let result = sqlx::query(
        "INSERT INTO user_withdraws (created_time, updated_time, user_id, amount, success_time, \
         withdraw_way, status, error_times, last_time, remark) VALUES (?, ?, ?, ?, 0, ?, ?, 0, 0, ?)",
    )
    .bind(ts)
    .bind(ts)
    .bind(user_id)
    .bind(input.amount)
    .bind(way)
    .bind(WITHDRAW_STATUS_WAITING)
    .bind(&remark)
    .execute(&mut *tx)
    .await?;
    let withdraw_id = result.last_insert_id() as u32;
    insert_finance(
        &mut *tx,
        user_id,
        FINANCE_OUTPUT,
        input.amount,
        after,
        FINANCE_ACTION_WITHDRAW,
        &format!("W{withdraw_id}"),
        "",
    )
    .await?;
    tx.commit().await?;

    sqlx::query_as::<_, UserWithdraw>(
        "SELECT id, created_time, updated_time, user_id, amount, success_time, withdraw_way, \
         status, error_times, last_time, remark FROM user_withdraws WHERE id = ?",
    )
    .bind(withdraw_id)
    .fetch_one(pool)
    .await
    .map_err(ModelError::from)
}

/// 充值申请（`ApplyRecharge`）：只落单，实际到账由后台确认。
pub async fn apply_recharge(
    pool: &MySqlPool,
    user_id: u32,
    input: &RechargeInput,
) -> Result<Recharge, ModelError> {
    if user_id == 0 {
        return Err(ModelError::invalid("请先登录"));
    }
    if input.amount <= 0 {
        return Err(ModelError::invalid("请输入金额"));
    }
    if input.amount > RECHARGE_AMOUNT_MAX {
        return Err(ModelError::invalid("充值金额过大"));
    }
    let remark: String = input.remark.chars().take(200).collect();
    let pay_way = {
        let raw = input.pay_way.trim();
        if raw.is_empty() {
            "支付宝".to_string()
        } else {
            raw.to_string()
        }
    };
    let ts = now();
    let result = sqlx::query(
        "INSERT INTO recharges (created_time, updated_time, user_id, amount, pay_way, remark, \
         status, confirm_time) VALUES (?, ?, ?, ?, ?, ?, 0, 0)",
    )
    .bind(ts)
    .bind(ts)
    .bind(user_id)
    .bind(input.amount)
    .bind(&pay_way)
    .bind(&remark)
    .execute(pool)
    .await?;
    let id = result.last_insert_id() as u32;
    sqlx::query_as::<_, Recharge>(
        "SELECT id, created_time, updated_time, user_id, amount, pay_way, remark, status, \
         confirm_time FROM recharges WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .map_err(ModelError::from)
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    /// 找一个正常状态的会员做测试主体。
    async fn pick_user(pool: &MySqlPool) -> Option<u32> {
        sqlx::query_scalar::<_, u32>("SELECT id FROM users WHERE status = 1 ORDER BY id LIMIT 1")
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
    }

    /// 找一个不是该会员发布的商品。
    async fn pick_goods(pool: &MySqlPool, user_id: u32) -> Option<i64> {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM archives WHERE user_id <> ? AND price > 0 ORDER BY id DESC LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
    }

    #[tokio::test]
    async fn test_cart_crud() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let Some(user_id) = pick_user(&pool).await else {
            return;
        };
        let Some(goods_id) = pick_goods(&pool, user_id).await else {
            return;
        };

        // 先清掉历史残留
        sqlx::query("DELETE FROM carts WHERE user_id = ?")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();

        let cart = add_cart(&pool, user_id, goods_id, 2).await.unwrap();
        assert_eq!(cart.quantity, 2);
        assert_eq!(cart.archive_id, goods_id);

        // 同一商品再加入是累加
        let cart2 = add_cart(&pool, user_id, goods_id, 1).await.unwrap();
        assert_eq!(cart2.quantity, 3);
        assert_eq!(cart2.id, cart.id);

        let (items, total) = cart_list(&pool, user_id).await.unwrap();
        assert_eq!(items.len(), 1);
        assert!(total > 0);
        assert_eq!(items[0].amount, total);
        assert_eq!(count_cart(&pool, user_id).await.unwrap(), 1);

        // 改数量
        update_cart_quantity(&pool, user_id, cart.id, 5).await.unwrap();
        let quantity: i32 = sqlx::query_scalar("SELECT quantity FROM carts WHERE id = ?")
            .bind(cart.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(quantity, 5);

        // 别人的购物车改不了
        assert!(update_cart_quantity(&pool, user_id + 9999, cart.id, 1).await.is_err());

        // 删除
        delete_cart(&pool, user_id, cart.id).await.unwrap();
        assert_eq!(count_cart(&pool, user_id).await.unwrap(), 0);

        // 清空
        add_cart(&pool, user_id, goods_id, 1).await.unwrap();
        clear_cart(&pool, user_id).await.unwrap();
        assert_eq!(count_cart(&pool, user_id).await.unwrap(), 0);

        // 自己的商品不能加购
        let own: Option<i64> =
            sqlx::query_scalar("SELECT id FROM archives WHERE user_id = ? LIMIT 1")
                .bind(user_id)
                .fetch_optional(&pool)
                .await
                .ok()
                .flatten();
        if let Some(own) = own {
            assert!(add_cart(&pool, user_id, own, 1).await.is_err());
        }
        // 不存在的商品
        assert!(add_cart(&pool, user_id, 999_999_999, 1).await.is_err());
    }

    #[tokio::test]
    async fn test_order_create_cancel() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let Some(user_id) = pick_user(&pool).await else {
            return;
        };
        let Some(goods_id) = pick_goods(&pool, user_id).await else {
            return;
        };
        let stock_before: i64 = sqlx::query_scalar("SELECT stock FROM archives WHERE id = ?")
            .bind(goods_id)
            .fetch_one(&pool)
            .await
            .unwrap();

        let input = CreateOrderInput {
            details: vec![OrderDetailInput {
                goods_id,
                quantity: 2,
                ..Default::default()
            }],
            ..Default::default()
        };
        let order = create_order(&pool, user_id, input).await.unwrap();
        assert_eq!(order.user_id, user_id);
        assert_eq!(order.status, ORDER_STATUS_WAITING);
        assert!(!order.order_id.is_empty());

        // 详情能查到
        let detail = order_detail(&pool, &order.order_id).await.unwrap().unwrap();
        assert_eq!(detail.details.len(), 1);
        assert_eq!(detail.details[0].detail.quantity, 2);

        // 列表能查到
        let (total, rows) = order_list(&pool, user_id, "", "", 1, 20).await.unwrap();
        assert!(total >= 1);
        assert!(rows.iter().any(|item| item.order.order_id == order.order_id));

        // 取消订单：状态转 -1，库存回补
        cancel_order(&pool, user_id, &order.order_id).await.unwrap();
        let after = load_order(&pool, &order.order_id).await.unwrap().unwrap();
        assert_eq!(after.status, ORDER_STATUS_CANCELED);
        let stock_after: i64 = sqlx::query_scalar("SELECT stock FROM archives WHERE id = ?")
            .bind(goods_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stock_after, stock_before + 2);

        // 别人的订单不能取消
        assert!(cancel_order(&pool, user_id + 9999, &order.order_id).await.is_err());

        // 清理
        sqlx::query("UPDATE archives SET stock = ? WHERE id = ?")
            .bind(stock_before)
            .bind(goods_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM payments WHERE order_id = ?")
            .bind(&order.order_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM order_details WHERE order_id = ?")
            .bind(&order.order_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM orders WHERE order_id = ?")
            .bind(&order.order_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_shop_deposit() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let Some(user_id) = pick_user(&pool).await else {
            return;
        };
        let balance: i64 = sqlx::query_scalar("SELECT balance FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();

        sqlx::query("DELETE FROM shops WHERE user_id = ?")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();

        let shop = ensure_shop(&pool, user_id).await.unwrap();
        assert_eq!(shop.user_id, user_id);
        assert_eq!(shop.status, SHOP_STATUS_NORMAL);

        // 余额不足
        assert!(pay_deposit(&pool, user_id, 10_000_000).await.is_err());

        // 临时给点余额
        sqlx::query("UPDATE users SET balance = ? WHERE id = ?")
            .bind(balance + 100_000)
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        let shop = pay_deposit(&pool, user_id, 5_000).await.unwrap();
        assert_eq!(shop.deposit, 5_000);
        assert_eq!(shop.deposit_total, 5_000);

        let (total, logs) = deposit_logs(&pool, user_id, 1, 20).await.unwrap();
        assert!(total >= 1);
        assert_eq!(logs[0].kind, DEPOSIT_TYPE_PAY);
        assert_eq!(logs[0].amount, 5_000);

        // 退还
        let shop = refund_deposit(&pool, user_id, 2_000).await.unwrap();
        assert_eq!(shop.deposit, 3_000);
        // 退的比保证金多 → 失败
        assert!(refund_deposit(&pool, user_id, 999_999).await.is_err());

        // 装修配置
        let saved = save_shop(
            &pool,
            user_id,
            &ShopInput {
                name: "测试店铺".into(),
                intro: "介绍".into(),
                decoration: "{\"layout\":\"compact\"}".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(saved.name, "测试店铺");
        let decoration = shop_decoration(&saved);
        assert_eq!(decoration.layout, "compact");
        assert!(!decoration.modules.is_empty());
        // 非法 JSON 装修配置要报错
        assert!(save_shop(
            &pool,
            user_id,
            &ShopInput {
                decoration: "not-json".into(),
                ..Default::default()
            }
        )
        .await
        .is_err());

        // 清理
        sqlx::query("DELETE FROM shop_deposit_logs WHERE user_id = ?")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM shops WHERE user_id = ?")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET balance = ? WHERE id = ?")
            .bind(balance)
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_retailer_commission_withdraw() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let Some(user_id) = pick_user(&pool).await else {
            return;
        };
        let ts = now();
        let commission_id = sqlx::query(
            "INSERT INTO commissions (created_time, updated_time, user_id, order_id, \
             order_amount, amount, status, withdraw_id, remark) \
             VALUES (?, ?, ?, ?, 1000, 300, 0, 0, '测试佣金')",
        )
        .bind(ts)
        .bind(ts)
        .bind(user_id)
        .bind(format!("rustcms-test-{ts}"))
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_id();

        let stats = retailer_statistics(&pool, user_id).await.unwrap();
        assert!(stats.can_withdraw_amount >= 300);

        let withdraw = retailer_apply_withdraw(&pool, user_id).await.unwrap();
        assert_eq!(withdraw.status, WITHDRAW_STATUS_WAITING);
        assert!(withdraw.amount >= 300);

        // 佣金变成已提现
        let status: i8 = sqlx::query_scalar("SELECT status FROM commissions WHERE id = ?")
            .bind(commission_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, COMMISSION_STATUS_PAID);

        let (_, rows) = retailer_commissions(&pool, user_id, 1, 20).await.unwrap();
        assert!(rows.iter().any(|item| item.id == commission_id as u32));
        let (_, withdraws) = retailer_withdraws(&pool, user_id, 1, 20).await.unwrap();
        assert!(withdraws.iter().any(|item| item.id == withdraw.id));

        // 清理
        sqlx::query("DELETE FROM user_withdraws WHERE id = ?")
            .bind(withdraw.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM commissions WHERE id = ?")
            .bind(commission_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_user_recharge_and_withdraw() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let Some(user_id) = pick_user(&pool).await else {
            return;
        };
        let balance: i64 = sqlx::query_scalar("SELECT balance FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();

        // 充值申请
        let recharge = apply_recharge(
            &pool,
            user_id,
            &RechargeInput {
                amount: 1_000,
                pay_way: String::new(),
                remark: "测试充值".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(recharge.amount, 1_000);
        assert_eq!(recharge.pay_way, "支付宝");
        assert_eq!(recharge.status, 0);

        // 金额非法
        assert!(apply_recharge(&pool, user_id, &RechargeInput::default())
            .await
            .is_err());
        assert!(apply_recharge(
            &pool,
            user_id,
            &RechargeInput {
                amount: RECHARGE_AMOUNT_MAX + 1,
                ..Default::default()
            }
        )
        .await
        .is_err());

        // 提现：缺账户信息要报错
        assert!(apply_balance_withdraw(&pool, user_id, &WithdrawInput { amount: 100, ..Default::default() })
            .await
            .is_err());

        sqlx::query("UPDATE users SET balance = ? WHERE id = ?")
            .bind(balance + 10_000)
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        let withdraw = apply_balance_withdraw(
            &pool,
            user_id,
            &WithdrawInput {
                amount: 1_000,
                way: WITHDRAW_WAY_ALIPAY,
                account: "test@example.com".into(),
                real_name: "测试".into(),
                remark: "备注".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(withdraw.amount, 1_000);
        assert_eq!(withdraw.withdraw_way, WITHDRAW_WAY_ALIPAY);
        assert!(withdraw.remark.contains("支付宝"));

        // 清理
        sqlx::query("DELETE FROM finances WHERE order_id = ?")
            .bind(format!("W{}", withdraw.id))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM user_withdraws WHERE id = ?")
            .bind(withdraw.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM recharges WHERE id = ?")
            .bind(recharge.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET balance = ? WHERE id = ?")
            .bind(balance)
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
    }
}







//! 订单 / 支付单 / 财务流水（`pluginOrder.go`、`pluginFinance.go`）。
//!
//! 状态枚举（与 Go `config/constant.go` 一致）：
//! 订单 `status`：-1 已取消 / 0 待支付 / 1 已支付 / 2 配送中 / 3 已完成 / 8 退款中 / 9 已退款。
//! 财务 `direction`：1 收入 / 2 支出；`action`：2 购买等。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::{site_setting, ModelError};

/// 订单配置键。
pub const ORDER_SETTING: &str = "order";

/// 订单。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Order {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub order_id: String,
    pub payment_id: String,
    pub user_id: u32,
    pub address_id: u32,
    pub remark: String,
    /// 数据库列名是 `type`（MySQL 关键字，故字段改名 + 两侧都声明映射）。
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

/// 支付单。
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

/// 财务流水。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Finance {
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

/// 订单列表筛选。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OrderFilter {
    #[serde(default)]
    pub order_id: String,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default, rename = "type")]
    pub kind: String,
}

fn status_value(raw: &str) -> Option<i8> {
    match raw.trim() {
        "" | "all" => None,
        "waiting" => Some(0),
        "paid" => Some(1),
        "delivery" => Some(2),
        "finished" => Some(3),
        "refunding" => Some(8),
        "refunded" => Some(9),
        "closed" => Some(-1),
        _ => None,
    }
}

/// 读订单配置（补默认值）。
pub async fn config_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, ORDER_SETTING).await?;
    if let Value::Object(map) = &mut value {
        map.entry("no_process").or_insert(json!(false));
        map.entry("auto_finish_day").or_insert(json!(0));
        map.entry("auto_close_minute").or_insert(json!(0));
        map.entry("seller_percent").or_insert(json!(0));
        map.entry("no_need_login").or_insert(json!(false));
    }
    Ok(value)
}

/// 保存订单配置（字段级 merge）。
pub async fn config_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    if !patch.is_object() {
        return Err(anyhow::anyhow!("配置必须是 JSON 对象"));
    }
    site_setting::save_section(pool, ORDER_SETTING, patch).await?;
    config_get(pool).await
}

/// 分页查订单，返回 `(总数, 本页记录)`。
pub async fn list(
    pool: &MySqlPool,
    filter: &OrderFilter,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Order>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;

    let mut where_sql = String::from(" WHERE 1 = 1");
    if !filter.kind.trim().is_empty() {
        where_sql.push_str(" AND `type` = ?");
    }
    if !filter.order_id.trim().is_empty() {
        where_sql.push_str(" AND order_id = ?");
    }
    if !filter.user_name.trim().is_empty() {
        where_sql.push_str(
            " AND user_id IN (SELECT id FROM users WHERE user_name LIKE ?)",
        );
    }
    let status = status_value(&filter.status);
    if status.is_some() {
        where_sql.push_str(" AND status = ?");
    }

    let user_like = format!("%{}%", filter.user_name.trim());

    let count_sql = format!("SELECT COUNT(*) FROM orders{where_sql}");
    let mut counter = sqlx::query_scalar::<_, i64>(&count_sql);
    if !filter.kind.trim().is_empty() {
        counter = counter.bind(filter.kind.trim().to_string());
    }
    if !filter.order_id.trim().is_empty() {
        counter = counter.bind(filter.order_id.trim().to_string());
    }
    if !filter.user_name.trim().is_empty() {
        counter = counter.bind(&user_like);
    }
    if let Some(status) = status {
        counter = counter.bind(status);
    }
    let total = counter.fetch_one(pool).await?;

    let list_sql = format!("SELECT * FROM orders{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?");
    let mut query = sqlx::query_as::<_, Order>(&list_sql);
    if !filter.kind.trim().is_empty() {
        query = query.bind(filter.kind.trim().to_string());
    }
    if !filter.order_id.trim().is_empty() {
        query = query.bind(filter.order_id.trim().to_string());
    }
    if !filter.user_name.trim().is_empty() {
        query = query.bind(&user_like);
    }
    if let Some(status) = status {
        query = query.bind(status);
    }
    let rows = query.bind(page_size).bind(offset).fetch_all(pool).await?;
    Ok((total, rows))
}

/// 按订单号取订单。
pub async fn get(pool: &MySqlPool, order_id: &str) -> anyhow::Result<Option<Order>> {
    Ok(sqlx::query_as::<_, Order>("SELECT * FROM orders WHERE order_id = ?")
        .bind(order_id)
        .fetch_optional(pool)
        .await?)
}

/// 按订单号取支付单。
pub async fn payment_get(pool: &MySqlPool, order_id: &str) -> anyhow::Result<Option<Payment>> {
    Ok(sqlx::query_as::<_, Payment>("SELECT * FROM payments WHERE order_id = ? LIMIT 1")
        .bind(order_id)
        .fetch_optional(pool)
        .await?)
}

async fn user_brief(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Value>> {
    let Some((user_name, real_name, avatar_url)): Option<(String, String, String)> =
        sqlx::query_as("SELECT user_name, real_name, avatar_url FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?
    else {
        return Ok(None);
    };
    Ok(Some(json!({
        "id": id,
        "user_name": user_name,
        "real_name": real_name,
        "avatar_url": avatar_url,
    })))
}

/// 订单详情（补买家 / 分享用户 / 上级分享用户）。
pub async fn detail(pool: &MySqlPool, order_id: &str) -> Result<Value, ModelError> {
    let order = get(pool, order_id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("订单不存在"))?;

    let mut value = serde_json::to_value(&order).unwrap_or(Value::Null);
    let user = user_brief(pool, order.user_id).await.ok().flatten();
    let share_user = if order.share_user_id > 0 {
        user_brief(pool, order.share_user_id).await.ok().flatten()
    } else {
        None
    };
    let parent_user = if order.share_parent_user_id > 0 {
        user_brief(pool, order.share_parent_user_id).await.ok().flatten()
    } else {
        None
    };
    if let Value::Object(map) = &mut value {
        map.insert("user".into(), user.unwrap_or(Value::Null));
        map.insert("share_user".into(), share_user.unwrap_or(Value::Null));
        map.insert("parent_user".into(), parent_user.unwrap_or(Value::Null));
    }
    Ok(value)
}

/// 手动标记订单已支付（生成支付单 + 记财务流水 + 支付成功流转）。
pub async fn set_pay(pool: &MySqlPool, order_id: &str, pay_way: &str) -> Result<(), ModelError> {
    let order = get(pool, order_id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("订单不存在"))?;
    if order.paid_time > 0 {
        return Err(ModelError::invalid("该订单已支付"));
    }

    let pay_way = pay_way.trim();
    let pay_way = if pay_way.is_empty() { "offline" } else { pay_way };
    let now = chrono::Utc::now().timestamp();

    // 生成支付单（1:1，payment_id 用随机串）
    let payment = match payment_get(pool, order_id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
    {
        Some(mut payment) => {
            if payment.paid_time > 0 {
                return Err(ModelError::invalid("该订单已支付"));
            }
            payment.pay_way = pay_way.to_string();
            payment.paid_time = now;
            payment.terrace_id = now.to_string();
            payment.status = 1;
            sqlx::query(
                "UPDATE payments SET pay_way = ?, paid_time = ?, terrace_id = ?, status = 1, \
                    updated_time = ? WHERE payment_id = ?",
            )
            .bind(pay_way)
            .bind(now)
            .bind(now.to_string())
            .bind(now)
            .bind(&payment.payment_id)
            .execute(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
            payment.payment_id
        }
        None => {
            let payment_id = crate::auth::generate_secret(32);
            sqlx::query(
                "INSERT INTO payments (created_time, updated_time, payment_id, terrace_id, \
                    user_id, order_id, amount, status, pay_way, paid_time) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
            )
            .bind(now)
            .bind(now)
            .bind(&payment_id)
            .bind(now.to_string())
            .bind(order.user_id)
            .bind(order_id)
            .bind(order.amount)
            .bind(pay_way)
            .bind(now)
            .execute(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
            payment_id
        }
    };

    // 订单状态流转：no_process（或 vip）付款即完成，否则停在「已支付待发货」
    // （对齐 Go SuccessPaidOrder：置 Paid=1，NoProcess 时再 SetOrderFinished→3）
    let no_process = config_get(pool)
        .await
        .ok()
        .and_then(|value| value.get("no_process").and_then(Value::as_bool))
        .unwrap_or(false);
    let (status, finished_time) = if no_process {
        (3i8, now)
    } else {
        (1i8, 0i64)
    };
    sqlx::query(
        "UPDATE orders SET payment_id = ?, paid_time = ?, status = ?, finished_time = ?, \
            updated_time = ? WHERE order_id = ?",
    )
    .bind(&payment)
    .bind(now)
    .bind(status)
    .bind(finished_time)
    .bind(now)
    .bind(order_id)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;

    // 记财务流水（方向=支出，动作=购买）
    let balance: i64 = sqlx::query_scalar("SELECT balance FROM users WHERE id = ?")
        .bind(order.user_id)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    sqlx::query(
        "INSERT INTO finances (created_time, updated_time, user_id, direction, amount, \
            after_amount, action, order_id, status, remark) \
         VALUES (?, ?, ?, 2, ?, ?, 2, ?, 1, '购买')",
    )
    .bind(now)
    .bind(now)
    .bind(order.user_id)
    .bind(order.amount)
    .bind(balance)
    .bind(order_id)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("支付失败: {err}")))?;

    Ok(())
}

/// 设置发货（要求已支付）。
pub async fn set_deliver(
    pool: &MySqlPool,
    order_id: &str,
    express_company: &str,
    tracking_number: &str,
) -> Result<(), ModelError> {
    let order = get(pool, order_id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("订单不存在"))?;
    if order.status != 1 {
        return Err(ModelError::invalid("仅已支付的订单才能发货"));
    }
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "UPDATE orders SET status = 2, deliver_time = ?, express_company = ?, tracking_number = ?, \
            updated_time = ? WHERE order_id = ?",
    )
    .bind(now)
    .bind(express_company.trim())
    .bind(tracking_number.trim())
    .bind(now)
    .bind(order_id)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    Ok(())
}

/// 标记完成。
pub async fn set_finished(pool: &MySqlPool, order_id: &str) -> Result<(), ModelError> {
    if get(pool, order_id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .is_none()
    {
        return Err(ModelError::invalid("订单不存在"));
    }
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "UPDATE orders SET status = 3, finished_time = ?, updated_time = ? WHERE order_id = ?",
    )
    .bind(now)
    .bind(now)
    .bind(order_id)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    Ok(())
}

/// 标记取消。
pub async fn set_canceled(pool: &MySqlPool, order_id: &str) -> Result<(), ModelError> {
    if get(pool, order_id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .is_none()
    {
        return Err(ModelError::invalid("订单不存在"));
    }
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "UPDATE orders SET status = -1, end_time = ?, updated_time = ? WHERE order_id = ?",
    )
    .bind(now)
    .bind(now)
    .bind(order_id)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    Ok(())
}

/// 退款状态流转。
///
/// `apply = true` 发起退款（status 8、refund_status 0）；否则按传入状态：
/// 1 退款成功（status 9）、-1 退款失败（回落到已支付 status 1）。
pub async fn set_refund(
    pool: &MySqlPool,
    order_id: &str,
    status: i8,
    apply: bool,
) -> Result<(), ModelError> {
    if get(pool, order_id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .is_none()
    {
        return Err(ModelError::invalid("订单不存在"));
    }
    let now = chrono::Utc::now().timestamp();
    let (order_status, refund_status) = if apply {
        (8i8, 0i8)
    } else if status == 1 {
        (9i8, 1i8)
    } else {
        (1i8, -1i8)
    };
    sqlx::query(
        "UPDATE orders SET status = ?, refund_status = ?, updated_time = ? WHERE order_id = ?",
    )
    .bind(order_status)
    .bind(refund_status)
    .bind(now)
    .bind(order_id)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    Ok(())
}

/// 导出订单（按状态 + 时间范围过滤），返回 `(表头, 行数据)`。
pub async fn export(
    pool: &MySqlPool,
    status: i8,
    start: i64,
    end: i64,
) -> anyhow::Result<(Vec<String>, Vec<Vec<Value>>)> {
    let mut where_sql = String::from(" WHERE 1 = 1");
    if status != i8::MAX {
        where_sql.push_str(" AND status = ?");
    }
    if start > 0 {
        where_sql.push_str(" AND created_time >= ?");
    }
    if end > 0 {
        where_sql.push_str(" AND created_time <= ?");
    }

    let sql = format!("SELECT * FROM orders{where_sql} ORDER BY id DESC");
    let mut query = sqlx::query_as::<_, Order>(&sql);
    if status != i8::MAX {
        query = query.bind(status);
    }
    if start > 0 {
        query = query.bind(start);
    }
    if end > 0 {
        query = query.bind(end);
    }
    let rows = query.fetch_all(pool).await?;

    let header = vec![
        "order_id", "user_id", "type", "status", "amount", "paid_time", "created_time",
        "express_company", "tracking_number", "remark",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();

    let content = rows
        .iter()
        .map(|order| {
            vec![
                json!(order.order_id),
                json!(order.user_id),
                json!(order.kind),
                json!(order.status),
                json!(order.amount),
                json!(order.paid_time),
                json!(order.created_time.unwrap_or(0)),
                json!(order.express_company),
                json!(order.tracking_number),
                json!(order.remark),
            ]
        })
        .collect();
    Ok((header, content))
}

// ---------------------------- 财务 -----------------------------------------

/// 分页查财务流水，返回 `(总数, 本页记录)`。
pub async fn finance_list(
    pool: &MySqlPool,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Finance>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM finances")
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as::<_, Finance>("SELECT * FROM finances ORDER BY id DESC LIMIT ? OFFSET ?")
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    Ok((total, rows))
}

/// 按 ID 取财务流水。
pub async fn finance_get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Finance>> {
    Ok(
        sqlx::query_as::<_, Finance>("SELECT * FROM finances WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, setting, test_pool};

    #[tokio::test]
    async fn order_lifecycle_and_finance() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, ORDER_SETTING).await.unwrap();

        let now = chrono::Utc::now().timestamp();
        let order_id = format!("rustcms-order-{now}");
        let user_id: u32 = sqlx::query_scalar("SELECT id FROM users ORDER BY id LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let order_db_id = sqlx::query(
            "INSERT INTO orders (created_time, updated_time, order_id, payment_id, user_id, \
                remark, `type`, status, amount) VALUES (?, ?, ?, '', ?, '', 'goods', 0, 1000)",
        )
        .bind(now)
        .bind(now)
        .bind(&order_id)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_id() as u32;

        // 列表 + 状态筛选
        let filter = OrderFilter {
            order_id: order_id.clone(),
            ..Default::default()
        };
        let (total, rows) = list(&pool, &filter, 1, 20).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].id, order_db_id);

        // 详情
        let detail = detail(&pool, &order_id).await.unwrap();
        assert!(detail.get("user").is_some(), "{detail}");

        // 支付前不能发货
        assert!(set_deliver(&pool, &order_id, "sf", "123").await.is_err());

        // 支付
        set_pay(&pool, &order_id, "offline").await.unwrap();
        let order = get(&pool, &order_id).await.unwrap().unwrap();
        assert!(order.paid_time > 0);
        assert_eq!(order.status, 1, "非 no_process 应停在已支付待发货");
        assert_eq!(order.payment_id.len(), 32);

        // 重复支付被拒
        assert!(set_pay(&pool, &order_id, "offline").await.is_err());

        // 发货
        set_deliver(&pool, &order_id, "顺丰", "SF123").await.unwrap();
        let order = get(&pool, &order_id).await.unwrap().unwrap();
        assert_eq!(order.status, 2);
        assert_eq!(order.express_company, "顺丰");

        // 完成
        set_finished(&pool, &order_id).await.unwrap();
        assert_eq!(get(&pool, &order_id).await.unwrap().unwrap().status, 3);

        // 财务流水应有购买记录
        let (total, rows) = finance_list(&pool, 1, 20).await.unwrap();
        assert!(total >= 1);
        let buy = rows
            .iter()
            .find(|item| item.order_id == order_id)
            .expect("应有购买流水");
        assert_eq!(buy.direction, 2);
        assert_eq!(buy.action, 2);
        assert_eq!(buy.amount, 1000);

        // 导出
        let (header, content) = export(&pool, i8::MAX, 0, 0).await.unwrap();
        assert_eq!(header[0], "order_id");
        assert!(content.len() >= 1);

        // 取消另一单
        let order2 = format!("rustcms-order2-{now}");
        sqlx::query(
            "INSERT INTO orders (created_time, updated_time, order_id, payment_id, user_id, \
                `type`, status, amount) VALUES (?, ?, ?, '', ?, 'goods', 0, 100)",
        )
        .bind(now)
        .bind(now)
        .bind(&order2)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
        set_canceled(&pool, &order2).await.unwrap();
        assert_eq!(get(&pool, &order2).await.unwrap().unwrap().status, -1);

        // 退款流转
        set_refund(&pool, &order2, 0, true).await.unwrap();
        let order = get(&pool, &order2).await.unwrap().unwrap();
        assert_eq!(order.status, 8);
        set_refund(&pool, &order2, 1, false).await.unwrap();
        assert_eq!(get(&pool, &order2).await.unwrap().unwrap().status, 9);

        // 收尾
        let _ = sqlx::query("DELETE FROM payments WHERE order_id IN (?, ?)")
            .bind(&order_id)
            .bind(&order2)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM finances WHERE order_id IN (?, ?)")
            .bind(&order_id)
            .bind(&order2)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM orders WHERE order_id IN (?, ?)")
            .bind(&order_id)
            .bind(&order2)
            .execute(&pool)
            .await;

        match backup {
            Some(raw) => setting::save_raw(&pool, ORDER_SETTING, &raw).await.unwrap(),
            None => setting::remove(&pool, ORDER_SETTING).await.unwrap(),
        }
    }

    #[tokio::test]
    async fn config_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, ORDER_SETTING).await.unwrap();

        let loaded = config_get(&pool).await.unwrap();
        assert_eq!(loaded["no_process"], json!(false));
        assert_eq!(loaded["seller_percent"], json!(0));

        let saved = config_save(
            &pool,
            &json!({"no_process": true, "seller_percent": 30}),
        )
        .await
        .unwrap();
        assert_eq!(saved["no_process"], json!(true));
        assert_eq!(saved["seller_percent"], json!(30));
        assert_eq!(saved["no_need_login"], json!(false));

        match backup {
            Some(raw) => setting::save_raw(&pool, ORDER_SETTING, &raw).await.unwrap(),
            None => setting::remove(&pool, ORDER_SETTING).await.unwrap(),
        }
    }
}

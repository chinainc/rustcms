//! 支付账户与收款统计。
//!
//! `payment_accounts` / `payment_statistics` 两张表。金额一律 int64（分），
//! `pay_config` 在库里是 TEXT 存 JSON 字符串，API 输出对象（缺字段补默认值，
//! 与零值结构体序列化结果一致）。

use serde::{Deserialize, Serialize, Serializer};
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::ModelError;

/// `PayConfig` 的默认值（各字段取零值）。
fn default_pay_config() -> Value {
    json!({
        "app_id": "",
        "app_secret": "",
        "api_key": "",
        "account": "",
        "cert_path": "",
        "public_cert_path": "",
        "root_cert_path": "",
        "webhook_id": "",
        "sandbox": false,
    })
}

/// `pay_config` 列（TEXT JSON）序列化成对象，解析失败或为空回落默认结构。
fn serialize_pay_config<S: Serializer>(raw: &Option<String>, serializer: S) -> Result<S::Ok, S::Error> {
    let mut value = raw
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or_else(default_pay_config);
    if let Value::Object(map) = &mut value {
        if let Value::Object(def) = default_pay_config() {
            for (key, item) in def {
                map.entry(key).or_insert(item);
            }
        }
    }
    value.serialize(serializer)
}

/// 支付账户（对应 `PaymentAccount`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct PaymentAccount {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub pay_way: String,
    /// 库列名是 `name`，API 字段是 `account_name`。
    #[sqlx(rename = "name")]
    #[serde(rename = "account_name")]
    pub account_name: String,
    pub status: i8,
    pub health_score: i32,
    #[serde(serialize_with = "serialize_pay_config")]
    pub pay_config: Option<String>,
    pub min_amount: i64,
    pub max_amount: i64,
    pub daily_amount_limit: i64,
    pub daily_count_limit: i64,
    pub monthly_amount_limit: i64,
    pub last_used_time: i64,
    pub used_count: i64,
    pub used_amount: i64,
    pub weight: i64,
    pub is_default: bool,
}

/// 收款统计（每日一条，`month_amount` 为当月累计，对应 `PaymentStatistic`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct PaymentStatistic {
    pub id: u32,
    pub account_id: u32,
    pub stat_time: i64,
    pub daily_amount: i64,
    pub monthly_amount: i64,
    pub daily_count: u32,
}

/// 保存支付账户的请求体（`PaymentAccountRequest`）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PaymentAccountSave {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub pay_way: String,
    #[serde(default)]
    pub account_name: String,
    #[serde(default)]
    pub status: i8,
    #[serde(default)]
    pub health_score: i32,
    #[serde(default)]
    pub pay_config: Value,
    #[serde(default)]
    pub min_amount: i64,
    #[serde(default)]
    pub max_amount: i64,
    #[serde(default)]
    pub daily_amount_limit: i64,
    #[serde(default)]
    pub daily_count_limit: i64,
    #[serde(default)]
    pub monthly_amount_limit: i64,
    #[serde(default)]
    pub weight: i64,
    #[serde(default)]
    pub is_default: bool,
}

/// 全部支付账户（`id desc`）。
pub async fn accounts(pool: &MySqlPool) -> anyhow::Result<Vec<PaymentAccount>> {
    let rows = sqlx::query_as::<_, PaymentAccount>(
        "SELECT id, created_time, updated_time, pay_way, name, status, health_score, pay_config, \
         min_amount, max_amount, daily_amount_limit, daily_count_limit, monthly_amount_limit, \
         last_used_time, used_count, used_amount, weight, is_default \
         FROM payment_accounts ORDER BY id DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 按 ID 取支付账户。
pub async fn get(pool: &MySqlPool, id: i64) -> anyhow::Result<Option<PaymentAccount>> {
    Ok(sqlx::query_as::<_, PaymentAccount>(
        "SELECT id, created_time, updated_time, pay_way, name, status, health_score, pay_config, \
         min_amount, max_amount, daily_amount_limit, daily_count_limit, monthly_amount_limit, \
         last_used_time, used_count, used_amount, weight, is_default \
         FROM payment_accounts WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// 保存（新增或更新）支付账户，返回保存后的记录（`SavePaymentAccount`）。
pub async fn save(
    pool: &MySqlPool,
    req: &PaymentAccountSave,
) -> Result<PaymentAccount, ModelError> {
    // 既有实现 PayConfig 是结构体整体覆盖；这里序列化整个 JSON 对象入库。
    let pay_config = if req.pay_config.is_null() {
        "{}".to_string()
    } else {
        req.pay_config.to_string()
    };
    let now = chrono::Utc::now().timestamp();

    let new_id = if req.id > 0 {
        get(pool, req.id)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .ok_or_else(|| ModelError::invalid("支付账户不存在"))?;
        sqlx::query(
            "UPDATE payment_accounts SET updated_time = ?, pay_way = ?, name = ?, status = ?, \
             health_score = ?, pay_config = ?, min_amount = ?, max_amount = ?, \
             daily_amount_limit = ?, daily_count_limit = ?, monthly_amount_limit = ?, \
             weight = ?, is_default = ? WHERE id = ?",
        )
        .bind(now)
        .bind(req.pay_way.trim())
        .bind(req.account_name.trim())
        .bind(req.status)
        .bind(req.health_score)
        .bind(&pay_config)
        .bind(req.min_amount)
        .bind(req.max_amount)
        .bind(req.daily_amount_limit)
        .bind(req.daily_count_limit)
        .bind(req.monthly_amount_limit)
        .bind(req.weight)
        .bind(req.is_default)
        .bind(req.id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
        req.id as u32
    } else {
        sqlx::query(
            "INSERT INTO payment_accounts (created_time, updated_time, pay_way, name, status, \
             health_score, pay_config, min_amount, max_amount, daily_amount_limit, \
             daily_count_limit, monthly_amount_limit, weight, is_default) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(now)
        .bind(now)
        .bind(req.pay_way.trim())
        .bind(req.account_name.trim())
        .bind(req.status)
        .bind(req.health_score)
        .bind(&pay_config)
        .bind(req.min_amount)
        .bind(req.max_amount)
        .bind(req.daily_amount_limit)
        .bind(req.daily_count_limit)
        .bind(req.monthly_amount_limit)
        .bind(req.weight)
        .bind(req.is_default)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?
        .last_insert_id() as u32
    };

    // 保存 paypal 账户后本应外呼注册 webhook，
    // 本项目没有 HTTP 客户端，此步骤跳过，不影响配置本身。

    get(pool, new_id as i64)
        .await?
        .ok_or_else(|| ModelError::invalid("支付账户不存在"))
}

/// 删除支付账户（先确认存在）。
pub async fn delete(pool: &MySqlPool, id: i64) -> Result<(), ModelError> {
    get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("支付账户不存在"))?;
    sqlx::query("DELETE FROM payment_accounts WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?;
    Ok(())
}

/// 收款统计分页（可按账户筛选，`id desc`）。
pub async fn statistic(
    pool: &MySqlPool,
    account_id: i64,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<PaymentStatistic>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;

    let total: i64 = if account_id > 0 {
        sqlx::query_scalar("SELECT COUNT(*) FROM payment_statistics WHERE account_id = ?")
            .bind(account_id)
            .fetch_one(pool)
            .await?
    } else {
        sqlx::query_scalar("SELECT COUNT(*) FROM payment_statistics")
            .fetch_one(pool)
            .await?
    };
    let rows = if account_id > 0 {
        sqlx::query_as::<_, PaymentStatistic>(
            "SELECT id, account_id, stat_time, daily_amount, monthly_amount, daily_count \
             FROM payment_statistics WHERE account_id = ? ORDER BY id DESC LIMIT ? OFFSET ?",
        )
        .bind(account_id)
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, PaymentStatistic>(
            "SELECT id, account_id, stat_time, daily_amount, monthly_amount, daily_count \
             FROM payment_statistics ORDER BY id DESC LIMIT ? OFFSET ?",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await?
    };
    Ok((total, rows))
}

// ---------------------------------------------------------------------------
// 支付网关（微信 V2 / 支付宝 / PayPal 共用）
// ---------------------------------------------------------------------------

pub mod alipay;
pub mod crypto;
pub mod rsa;
pub mod wechat;

use chrono::Datelike;

use super::trade::{self, Payment};

/// 取 `pay_config` 的 JSON 形态（解析失败当空对象，调用方自行校验缺字段）。
pub fn pay_config_json(account: &PaymentAccount) -> Value {
    account
        .pay_config
        .as_deref()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or_else(|| json!({}))
}

/// 选一个可用的支付账户。
///
/// 规则与既有行为一致：先按 `pay_way` + `status=1` 取全部账户；只有一个直接用；
/// 多个则按金额区间过滤，再按日/月限额过滤，都放行时随机挑一个。
/// **差异**：既有实现在「全部账户都被限额挡掉」时会下标越界 panic，
/// 这里退回到默认账户（再没有就取第一个），避免把站点打挂。
pub async fn valid_account(
    pool: &MySqlPool,
    pay_way: &str,
    amount: i64,
) -> anyhow::Result<Option<PaymentAccount>> {
    let accounts = sqlx::query_as::<_, PaymentAccount>(
        "SELECT id, created_time, updated_time, pay_way, name, status, health_score, pay_config, \
         min_amount, max_amount, daily_amount_limit, daily_count_limit, monthly_amount_limit, \
         last_used_time, used_count, used_amount, weight, is_default \
         FROM payment_accounts WHERE pay_way = ? AND status = 1 ORDER BY id ASC",
    )
    .bind(pay_way)
    .fetch_all(pool)
    .await?;
    if accounts.is_empty() {
        return Ok(None);
    }
    if accounts.len() == 1 {
        return Ok(accounts.into_iter().next());
    }

    let local_now = chrono::Local::now();
    let today = local_now.date_naive();
    let today_stamp = today
        .and_hms_opt(0, 0, 0)
        .and_then(|value| value.and_local_timezone(chrono::Local).single())
        .map(|value| value.timestamp())
        .unwrap_or(0);
    let month_stamp = today
        .with_day(1)
        .and_then(|value| value.and_hms_opt(0, 0, 0))
        .and_then(|value| value.and_local_timezone(chrono::Local).single())
        .map(|value| value.timestamp())
        .unwrap_or(0);

    let mut valid: Vec<PaymentAccount> = Vec::new();
    for account in accounts.iter() {
        if account.min_amount > 0 && amount < account.min_amount {
            continue;
        }
        if account.max_amount > 0 && amount > account.max_amount {
            continue;
        }
        if account.daily_amount_limit == 0
            && account.monthly_amount_limit == 0
            && account.daily_count_limit == 0
        {
            valid.push(account.clone());
            continue;
        }
        let last: Option<PaymentStatistic> = sqlx::query_as::<_, PaymentStatistic>(
            "SELECT id, account_id, stat_time, daily_amount, monthly_amount, daily_count \
             FROM payment_statistics WHERE account_id = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(account.id)
        .fetch_optional(pool)
        .await?;
        let Some(last) = last else {
            continue;
        };
        if last.stat_time < month_stamp {
            valid.push(account.clone());
            continue;
        }
        if account.monthly_amount_limit > 0 && last.monthly_amount >= account.monthly_amount_limit {
            continue;
        }
        if last.stat_time < today_stamp {
            valid.push(account.clone());
            continue;
        }
        if (account.daily_count_limit > 0 && last.daily_count as i64 >= account.daily_count_limit)
            || (account.daily_amount_limit > 0 && last.daily_amount >= account.daily_amount_limit)
        {
            continue;
        }
        valid.push(account.clone());
    }

    if valid.len() == 1 {
        return Ok(valid.into_iter().next());
    }
    if valid.is_empty() {
        // 既有实现在这里会 panic；改成退回默认账户 / 第一个账户
        if let Some(default) = accounts.iter().find(|account| account.is_default) {
            return Ok(Some(default.clone()));
        }
        return Ok(accounts.into_iter().next());
    }
    let index = (rand::random::<u32>() as usize) % valid.len();
    Ok(valid.into_iter().nth(index))
}

/// 把支付单关联到实际使用的支付账户（`DB.Model(payment).UpdateColumn("payment_account_id", id)`）。
pub async fn set_payment_account(
    pool: &MySqlPool,
    payment_pk: u32,
    account_id: u32,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE payments SET payment_account_id = ? WHERE id = ?")
        .bind(account_id)
        .bind(payment_pk)
        .execute(pool)
        .await?;
    Ok(())
}

/// 回写网关订单号（PayPal 下单后 `payment.TerraceId = resp.Id` 那一步）。
pub async fn set_payment_terrace(
    pool: &MySqlPool,
    payment_pk: u32,
    terrace_id: &str,
) -> anyhow::Result<()> {
    let now = trade::now();
    sqlx::query("UPDATE payments SET terrace_id = ?, updated_time = ? WHERE id = ?")
        .bind(terrace_id)
        .bind(now)
        .bind(payment_pk)
        .execute(pool)
        .await?;
    Ok(())
}

/// 回调金额与支付单金额是否一致（`got < 0` 表示网关没给金额，跳过校验）。
///
/// 既有实现没做这个校验；这里补上，只用来挡「伪造 / 串单」的通知，
/// 正常通知两边都是以「分」为单位的整数，不会误伤。
pub fn amount_matches(expected: i64, got: i64) -> bool {
    got < 0 || expected == got
}

/// 网关回调「支付成功」的统一落库参数。
#[derive(Debug, Clone, Default)]
pub struct GatewayPaid<'a> {
    /// 支付方式（`wechat` / `alipay` / `paypal`）
    pub pay_way: &'a str,
    /// 网关交易号（微信 `transaction_id`、支付宝 `trade_no`、PayPal capture id）
    pub terrace_id: &'a str,
    /// 网关买家标识（微信 `openid`、支付宝 `buyer_id`）
    pub buyer_id: &'a str,
    /// 买家信息（支付宝 `buyer_logon_id`、PayPal `payment_source`）
    pub buyer_info: &'a str,
}

/// 网关支付成功的统一收尾。
///
/// 入口：微信 / 支付宝异步通知，以及
/// `PaypalReturnResult → TraceQuery` 的「支付成功」分支：
/// 1. `payments`：写 `pay_way`/`terrace_id`/`paid_time`/`buyer_id`/`buyer_info`，`status=1`；
/// 2. `orders`：挂上 `payment_id`；
/// 3. `finances`：写一条「支出 + 买入」流水（`after_amount` 取当前余额，与既有行为一致）；
/// 4. 调 `trade::success_paid_order`（订单置已支付、`order_count+1`、自动完成分支）；
/// 5. `payment_statistics` / `payment_accounts` 累加统计。
///
/// 传入前调用方必须确认 `payment.paid_time == 0`（幂等由调用方判定，与既有行为一样）。
pub async fn settle_gateway_payment(
    pool: &MySqlPool,
    payment: &Payment,
    paid: GatewayPaid<'_>,
) -> Result<(), ModelError> {
    let ts = trade::now();
    // 带 `paid_time = 0` 条件做一次 CAS：并发回调（微信重试 + 主动查单同时到）
    // 只有一条能把支付单从「未支付」推到「已支付」，避免重复写资金流水。
    let updated = sqlx::query(
        "UPDATE payments SET pay_way = ?, terrace_id = ?, paid_time = ?, buyer_id = ?, \
         buyer_info = ?, status = 1, updated_time = ? WHERE id = ? AND paid_time = 0",
    )
    .bind(paid.pay_way)
    .bind(paid.terrace_id)
    .bind(ts)
    .bind(paid.buyer_id)
    .bind(paid.buyer_info)
    .bind(ts)
    .bind(payment.id)
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        // 已被另一条回调处理过，幂等返回
        return Ok(());
    }

    let Some(order) = trade::load_order(pool, &payment.order_id).await? else {
        return Err(ModelError::invalid("订单不存在"));
    };
    sqlx::query("UPDATE orders SET payment_id = ? WHERE id = ?")
        .bind(&payment.payment_id)
        .bind(order.id)
        .execute(pool)
        .await?;

    // 用户支付流水（在 SuccessPaidOrder 之前单独插一条 Finance）
    let balance: i64 = sqlx::query_scalar("SELECT balance FROM users WHERE id = ?")
        .bind(payment.user_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or(0);
    let mut conn = pool.acquire().await?;
    trade::insert_finance(
        &mut conn,
        payment.user_id,
        trade::FINANCE_OUTPUT,
        payment.amount,
        balance,
        trade::FINANCE_ACTION_BUY,
        &payment.order_id,
        "",
    )
    .await?;
    drop(conn);

    trade::success_paid_order(pool, &order, payment).await?;

    if payment.payment_account_id > 0 {
        // 既有实现把这段放进后台任务异步做；这里同步执行，结果一致且便于排查
        if let Err(err) =
            bump_account_statistic(pool, payment.payment_account_id, payment.amount).await
        {
            tracing::warn!(error = %err, "支付账户统计更新失败");
        }
    }
    Ok(())
}

/// 统计行的滚动规则（纯函数，便于单测）。
///
/// 与结算收尾里的账号统计一致：
/// 1. 上次统计时间早于本月 1 号 → 整行重置（月度累计也清零）；
/// 2. 早于今天 0 点 → 只保留 `monthly_amount`，其余清零；
/// 3. 累加金额与笔数。
///
/// 返回值里的 `id == 0` 表示需要 INSERT 新行，否则 UPDATE 原行。
fn roll_statistic(
    last: Option<PaymentStatistic>,
    account_id: i32,
    today_stamp: i64,
    month_stamp: i64,
    amount: i64,
) -> PaymentStatistic {
    let mut row = last.unwrap_or(PaymentStatistic {
        id: 0,
        account_id: account_id as u32,
        stat_time: 0,
        daily_amount: 0,
        monthly_amount: 0,
        daily_count: 0,
    });
    if row.stat_time < month_stamp {
        row = PaymentStatistic {
            id: 0,
            account_id: account_id as u32,
            stat_time: 0,
            daily_amount: 0,
            monthly_amount: 0,
            daily_count: 0,
        };
    }
    if row.stat_time < today_stamp {
        row = PaymentStatistic {
            id: 0,
            account_id: account_id as u32,
            stat_time: 0,
            daily_amount: 0,
            monthly_amount: row.monthly_amount,
            daily_count: 0,
        };
    }
    row.account_id = account_id as u32;
    row.stat_time = today_stamp;
    row.daily_amount += amount;
    row.monthly_amount += amount;
    row.daily_count += 1;
    row
}

/// 当前本地时间的「今天 0 点」「本月 1 号 0 点」时间戳（按本地时区的当天 0 点与当月 1 号 0 点）。
fn today_and_month_stamp() -> (i64, i64) {
    let local_now = chrono::Local::now();
    let today = local_now.date_naive();
    let start = |date: chrono::NaiveDate| {
        date.and_hms_opt(0, 0, 0)
            .and_then(|value| value.and_local_timezone(chrono::Local).single())
            .map(|value| value.timestamp())
            .unwrap_or(0)
    };
    let today_stamp = start(today);
    let month_stamp = today.with_day(1).map(start).unwrap_or(today_stamp);
    (today_stamp, month_stamp)
}

/// 支付账户统计累加（结算收尾里的异步部分，这里同步执行）。
pub async fn bump_account_statistic(
    pool: &MySqlPool,
    account_id: i32,
    amount: i64,
) -> anyhow::Result<()> {
    let (today_stamp, month_stamp) = today_and_month_stamp();

    let last = sqlx::query_as::<_, PaymentStatistic>(
        "SELECT id, account_id, stat_time, daily_amount, monthly_amount, daily_count \
         FROM payment_statistics WHERE account_id = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;

    let row = roll_statistic(last, account_id, today_stamp, month_stamp, amount);

    if row.id > 0 {
        sqlx::query(
            "UPDATE payment_statistics SET account_id = ?, stat_time = ?, daily_amount = ?, \
             monthly_amount = ?, daily_count = ? WHERE id = ?",
        )
        .bind(row.account_id)
        .bind(row.stat_time)
        .bind(row.daily_amount)
        .bind(row.monthly_amount)
        .bind(row.daily_count)
        .bind(row.id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO payment_statistics (account_id, stat_time, daily_amount, monthly_amount, \
             daily_count) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(row.account_id)
        .bind(row.stat_time)
        .bind(row.daily_amount)
        .bind(row.monthly_amount)
        .bind(row.daily_count)
        .execute(pool)
        .await?;
    }

    // 更新账户自身的统计列（在 SQL 里自增）
    sqlx::query(
        "UPDATE payment_accounts SET last_used_time = ?, used_count = used_count + 1, \
         used_amount = used_amount + ? WHERE id = ?",
    )
    .bind(trade::now())
    .bind(amount)
    .bind(account_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[tokio::test]
    async fn payment_account_crud() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };

        // 新增：pay_config 缺字段读回应补默认值
        let req = PaymentAccountSave {
            pay_way: "wechat".into(),
            account_name: "测试账户".into(),
            status: 1,
            health_score: 90,
            pay_config: json!({"app_id": "wx123"}),
            min_amount: 100,
            max_amount: 100_000,
            daily_amount_limit: 50_000,
            daily_count_limit: 10,
            monthly_amount_limit: 1_000_000,
            weight: 5,
            is_default: true,
            ..Default::default()
        };
        let saved = save(&pool, &req).await.unwrap();
        assert!(saved.id > 0);
        assert_eq!(saved.account_name, "测试账户");
        assert_eq!(saved.pay_config.as_deref().unwrap().contains("wx123"), true);

        let got = get(&pool, saved.id as i64).await.unwrap().unwrap();
        // API 输出（序列化层）应补齐 pay_config 缺省字段
        let payload = serde_json::to_value(&got).unwrap();
        assert_eq!(payload["account_name"], json!("测试账户"));
        assert_eq!(payload["pay_config"]["app_id"], json!("wx123"));
        assert_eq!(payload["pay_config"]["app_secret"], json!(""), "缺字段补默认值");
        assert_eq!(payload["pay_config"]["sandbox"], json!(false));
        assert_eq!(got.is_default, true);

        // 更新
        let req = PaymentAccountSave {
            id: saved.id as i64,
            account_name: "改名账户".into(),
            status: 0,
            ..req.clone()
        };
        let updated = save(&pool, &req).await.unwrap();
        assert_eq!(updated.account_name, "改名账户");
        assert_eq!(updated.status, 0);

        // 列表
        let rows = accounts(&pool).await.unwrap();
        assert!(rows.iter().any(|row| row.id == saved.id));

        // 统计：插一条再查
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO payment_statistics (account_id, stat_time, daily_amount, \
             monthly_amount, daily_count) VALUES (?, ?, 100, 200, 3)",
        )
        .bind(saved.id)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let (total, rows) = statistic(&pool, saved.id as i64, 1, 20).await.unwrap();
        assert!(total >= 1);
        assert!(rows.iter().any(|row| row.account_id == saved.id));
        let (filtered_total, _) = statistic(&pool, 999999999, 1, 20).await.unwrap();
        assert_eq!(filtered_total, 0);

        // 不存在的更新 / 删除
        let missing = PaymentAccountSave {
            id: 429496729,
            ..Default::default()
        };
        assert!(save(&pool, &missing).await.is_err());
        assert!(delete(&pool, 429496729).await.is_err());

        // 删除 + 统计清理
        delete(&pool, saved.id as i64).await.unwrap();
        assert!(get(&pool, saved.id as i64).await.unwrap().is_none());
        let _ = sqlx::query("DELETE FROM payment_statistics WHERE account_id = ?")
            .bind(saved.id)
            .execute(&pool)
            .await;
    }

    /// 跨天只保留月度累计、跨月整行重置。
    #[test]
    fn statistic_rolls_by_day_and_month() {
        const DAY: i64 = 86_400;
        let today = 1_800_000_000i64;
        let month_start = today - 3 * DAY;

        // 当天已有记录：原地累加
        let last = PaymentStatistic {
            id: 7,
            account_id: 3,
            stat_time: today,
            daily_amount: 100,
            monthly_amount: 500,
            daily_count: 2,
        };
        let row = roll_statistic(Some(last), 3, today, month_start, 50);
        assert_eq!(row.id, 7, "同一天要更新原行");
        assert_eq!(row.daily_amount, 150);
        assert_eq!(row.monthly_amount, 550);
        assert_eq!(row.daily_count, 3);
        assert_eq!(row.stat_time, today);

        // 昨天：保留月度累计，日累计与笔数清零
        let last = PaymentStatistic {
            id: 9,
            account_id: 3,
            stat_time: today - DAY,
            daily_amount: 100,
            monthly_amount: 500,
            daily_count: 2,
        };
        let row = roll_statistic(Some(last), 3, today, month_start, 50);
        assert_eq!(row.id, 0, "跨天要插新行");
        assert_eq!(row.daily_amount, 50);
        assert_eq!(row.monthly_amount, 550);
        assert_eq!(row.daily_count, 1);

        // 上个月：整行重置（月度累计也清零）
        let last = PaymentStatistic {
            id: 11,
            account_id: 3,
            stat_time: month_start - DAY,
            daily_amount: 100,
            monthly_amount: 500,
            daily_count: 2,
        };
        let row = roll_statistic(Some(last), 3, today, month_start, 50);
        assert_eq!(row.id, 0);
        assert_eq!(row.daily_amount, 50);
        assert_eq!(row.monthly_amount, 50);
        assert_eq!(row.daily_count, 1);

        // 没有历史：直接新建
        let row = roll_statistic(None, 5, today, month_start, 20);
        assert_eq!(row.id, 0);
        assert_eq!(row.account_id, 5);
        assert_eq!(row.daily_amount, 20);
        assert_eq!(row.monthly_amount, 20);
        assert_eq!(row.daily_count, 1);
        assert_eq!(row.stat_time, today);
    }

    /// 支付时机（今天/本月）的边界只依赖时间戳大小，不受时区换算结果影响。
    #[test]
    fn today_stamp_is_ordered() {
        let (today, month) = today_and_month_stamp();
        assert!(month <= today);
        assert_eq!(today % 60, 0, "应落在整分钟上");
    }

    /// 金额校验：网关没给金额（-1）跳过，给了就必须与支付单一致。
    #[test]
    fn amount_match_rules() {
        assert!(amount_matches(100, 100));
        assert!(!amount_matches(100, 99));
        assert!(!amount_matches(100, 10000), "元/分单位混用必须被挡");
        assert!(amount_matches(100, -1), "网关未返回金额时不校验");
        assert!(amount_matches(0, 0));
    }

    /// `pay_config` 解析失败当空对象（调用方按缺字段报错）。
    #[test]
    fn pay_config_json_falls_back_to_object() {
        let account = PaymentAccount {
            id: 1,
            created_time: None,
            updated_time: None,
            pay_way: "wechat".into(),
            account_name: "x".into(),
            status: 1,
            health_score: 0,
            pay_config: None,
            min_amount: 0,
            max_amount: 0,
            daily_amount_limit: 0,
            daily_count_limit: 0,
            monthly_amount_limit: 0,
            last_used_time: 0,
            used_count: 0,
            used_amount: 0,
            weight: 0,
            is_default: false,
        };
        assert_eq!(pay_config_json(&account), json!({}));
        let account = PaymentAccount {
            pay_config: Some("not-json".into()),
            ..account
        };
        assert_eq!(pay_config_json(&account), json!({}));
        let account = PaymentAccount {
            pay_config: Some(r#"{"app_id":"wx1"}"#.into()),
            ..account
        };
        assert_eq!(pay_config_json(&account)["app_id"], json!("wx1"));
    }
}

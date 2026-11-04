//! 支付账户与收款统计（`pluginPay.go`、Go `provider/pay.go`）。
//!
//! `payment_accounts` / `payment_statistics` 两张表。金额一律 int64（分），
//! `pay_config` 在库里是 TEXT 存 JSON 字符串，API 输出对象（缺字段补默认值，
//! 与 Go 零值结构体序列化结果一致）。

use serde::{Deserialize, Serialize, Serializer};
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::ModelError;

/// `PayConfig` 的默认值（与 Go `model.PayConfig` 零值一致）。
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

/// 支付账户（对应 Go `model.PaymentAccount`）。
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

/// 收款统计（每日一条，`month_amount` 为当月累计，对应 Go `model.PaymentStatistic`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct PaymentStatistic {
    pub id: u32,
    pub account_id: u32,
    pub stat_time: i64,
    pub daily_amount: i64,
    pub monthly_amount: i64,
    pub daily_count: u32,
}

/// 保存支付账户的请求体（Go `request.PaymentAccountRequest`）。
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

/// 全部支付账户（`id desc`，Go `GetPaymentAccounts`）。
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

/// 保存（新增或更新）支付账户，返回保存后的记录（Go `SavePaymentAccount`）。
pub async fn save(
    pool: &MySqlPool,
    req: &PaymentAccountSave,
) -> Result<PaymentAccount, ModelError> {
    // Go 版 PayConfig 是结构体整体覆盖；这里序列化整个 JSON 对象入库。
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

    // Go 版保存 paypal 账户后会外呼注册 webhook（UpdatePaypalWebhook），
    // 本项目没有 HTTP 客户端，此步骤跳过，不影响配置本身。

    get(pool, new_id as i64)
        .await?
        .ok_or_else(|| ModelError::invalid("支付账户不存在"))
}

/// 删除支付账户（先确认存在，Go `DeletePaymentAccount`）。
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

/// 收款统计分页（可按账户筛选，`id desc`，Go `GetPaymentAccountStatistic`）。
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
}

//! 后台会员管理（`pluginUser.go`）。
//!
//! 与前台 `models/user.rs` 分开：这里要 `users` 表的完整字段，且**不返回密码**。
//! `status` / `subscribed` 等是 `tinyint(1)`，sqlx 按布尔解析。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::{site_setting, ModelError};

/// 用户插件配置键（默认分组 / 默认状态 / 扩展字段）。
pub const SETTING_KEY: &str = "user";

/// 会员（不含密码）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct UserRow {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub parent_id: i32,
    pub user_name: String,
    pub url_token: String,
    pub real_name: String,
    pub first_name: String,
    pub last_name: String,
    pub birthday: Option<i64>,
    pub avatar_url: String,
    pub introduce: String,
    pub email: String,
    pub email_verified: bool,
    pub phone: String,
    pub group_id: u32,
    pub google_id: String,
    pub reset_password: bool,
    pub status: bool,
    pub is_retailer: bool,
    pub balance: i64,
    pub total_reward: i64,
    pub invite_code: String,
    pub last_login: Option<i64>,
    pub expire_time: Option<i64>,
    pub subscribed: bool,
    pub order_count: i64,
    pub points: i64,
    pub api_token: String,
}

/// 会员列表筛选。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UserFilter {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub group_id: u32,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub real_name: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub q: String,
    /// `normal` / `blocked` / `pending`
    #[serde(default)]
    pub status: String,
    /// `subscribed` / `ordered` / `repurchase`
    #[serde(default)]
    pub user_type: String,
    /// 分销员列表专用：只看 `is_retailer = 1` 的会员。
    #[serde(default)]
    pub is_retailer: Option<bool>,
}

/// 用户组。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct UserGroup {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub title: String,
    pub description: String,
    pub level: i32,
    pub status: bool,
    pub price: i64,
    pub setting: Option<String>,
}

/// 用户组入参。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UserGroupInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub level: i32,
    #[serde(default)]
    pub status: Option<i8>,
    #[serde(default)]
    pub price: i64,
    #[serde(default)]
    pub setting: Option<Value>,
}

/// 会员入参。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UserInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub user_name: Option<String>,
    #[serde(default)]
    pub real_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub group_id: Option<u32>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub introduce: Option<String>,
    /// `1` 正常，`0` 待审，`-1` 封禁
    #[serde(default)]
    pub status: Option<i8>,
    /// PATCH 语义：只覆盖显式传入的字段
    #[serde(default)]
    pub partial: bool,
}

const USER_COLUMNS: &str = "id, created_time, updated_time, parent_id, user_name, url_token, \
    real_name, first_name, last_name, birthday, avatar_url, introduce, email, email_verified, \
    phone, group_id, google_id, reset_password, status, is_retailer, balance, total_reward, \
    invite_code, last_login, expire_time, subscribed, order_count, points, api_token";

fn apply_str(target: &mut String, input: Option<&str>, update_all: bool) {
    if let Some(value) = input {
        if update_all || !value.is_empty() {
            *target = value.to_string();
        }
    }
}

/// URL 别名清洗。
fn sanitize_token(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// 字段名合法性（与 Go 的 `^[a-z][0-9a-z_]+$` 一致）。
pub fn valid_field_name(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    let rest: Vec<char> = chars.collect();
    !rest.is_empty()
        && rest
            .iter()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || *ch == '_')
}

/// 读用户插件配置。
pub async fn setting_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, SETTING_KEY).await?;
    if let Value::Object(map) = &mut value {
        map.entry("default_group_id").or_insert(json!(0));
        map.entry("default_status").or_insert(json!(0));
        if !map.get("fields").is_some_and(Value::is_array) {
            map.insert("fields".into(), json!([]));
        }
    }
    Ok(value)
}

/// 保存用户插件配置：校验字段名、去重、`required` 强制 false。
pub async fn setting_save(pool: &MySqlPool, patch: &Value) -> Result<Value, ModelError> {
    if !patch.is_object() {
        return Err(ModelError::invalid("配置格式错误"));
    }
    let mut patch = patch.clone();
    if let Value::Object(map) = &mut patch {
        if let Some(fields) = map.get("fields").and_then(Value::as_array) {
            let mut seen = std::collections::HashSet::new();
            let mut kept = Vec::new();
            for field in fields {
                let mut field = field.clone();
                let name = field
                    .get("field_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if !valid_field_name(&name) {
                    return Err(ModelError::invalid(format!("{name}命名不正确")));
                }
                if let Value::Object(item) = &mut field {
                    // 会员扩展字段不参与前台必填校验
                    item.insert("required".into(), json!(false));
                }
                if seen.insert(name) {
                    kept.push(field);
                }
            }
            map.insert("fields".into(), Value::Array(kept));
        }
    }
    site_setting::save_section(pool, SETTING_KEY, &patch)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    setting_get(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))
}

/// 删除用户扩展字段：同步从配置移除并 DROP 列。
pub async fn field_delete(pool: &MySqlPool, field_name: &str) -> Result<bool, ModelError> {
    let field_name = field_name.trim();
    if !valid_field_name(field_name) {
        return Err(ModelError::invalid("字段名不正确"));
    }

    let mut config = setting_get(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("读取配置失败: {err}")))?;
    let before = config
        .get("fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let kept: Vec<Value> = before
        .iter()
        .filter(|field| field.get("field_name").and_then(Value::as_str) != Some(field_name))
        .cloned()
        .collect();
    if kept.len() == before.len() {
        return Ok(false);
    }

    // 列存在才 DROP（历史库可能已经没有了）
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.columns WHERE table_schema = DATABASE() \
         AND table_name = 'users' AND column_name = ?",
    )
    .bind(field_name)
    .fetch_one(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;
    if exists.unwrap_or(0) > 0 {
        let sql = format!("ALTER TABLE users DROP COLUMN `{field_name}`");
        sqlx::query(&sql)
            .execute(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("删除字段失败: {err}")))?;
    }

    if let Value::Object(map) = &mut config {
        map.insert("fields".into(), Value::Array(kept));
    }
    site_setting::save_section(pool, SETTING_KEY, &config)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    Ok(true)
}

/// 分页查会员，返回 `(总数, 本页记录)`。
pub async fn list(
    pool: &MySqlPool,
    filter: &UserFilter,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<UserRow>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;

    let mut where_sql = String::from(" WHERE 1 = 1");
    if filter.id > 0 {
        where_sql.push_str(" AND id = ?");
    }
    if filter.group_id > 0 {
        where_sql.push_str(" AND group_id = ?");
    }

    let q = filter.q.trim();
    let has_q = !q.is_empty();
    if has_q {
        where_sql.push_str(" AND (user_name LIKE ? OR real_name LIKE ? OR phone LIKE ?)");
    } else {
        if !filter.phone.trim().is_empty() {
            where_sql.push_str(" AND phone = ?");
        }
        if !filter.user_name.trim().is_empty() {
            where_sql.push_str(" AND user_name LIKE ?");
        }
        if !filter.real_name.trim().is_empty() {
            where_sql.push_str(" AND real_name LIKE ?");
        }
    }

    let status_value = match filter.status.trim() {
        "normal" => Some(1i8),
        "blocked" => Some(-1i8),
        "pending" => Some(0i8),
        _ => None,
    };
    if status_value.is_some() {
        where_sql.push_str(" AND status = ?");
    }
    match filter.user_type.trim() {
        "subscribed" => where_sql.push_str(" AND subscribed = 1"),
        "ordered" => where_sql.push_str(" AND order_count > 0"),
        "repurchase" => where_sql.push_str(" AND order_count > 1"),
        _ => {}
    }
    if filter.is_retailer == Some(true) {
        where_sql.push_str(" AND is_retailer = 1");
    }

    let like_q = format!("%{q}%");

    // 计数
    let count_sql = format!("SELECT COUNT(*) FROM users{where_sql}");
    let mut counter = sqlx::query_scalar::<_, i64>(&count_sql);
    if filter.id > 0 {
        counter = counter.bind(filter.id);
    }
    if filter.group_id > 0 {
        counter = counter.bind(filter.group_id);
    }
    if has_q {
        counter = counter
            .bind(&like_q)
            .bind(&like_q)
            .bind(&like_q);
    } else {
        if !filter.phone.trim().is_empty() {
            counter = counter.bind(filter.phone.trim().to_string());
        }
        if !filter.user_name.trim().is_empty() {
            counter = counter.bind(format!("%{}%", filter.user_name.trim()));
        }
        if !filter.real_name.trim().is_empty() {
            counter = counter.bind(format!("%{}%", filter.real_name.trim()));
        }
    }
    if let Some(status) = status_value {
        counter = counter.bind(status);
    }
    let total = counter.fetch_one(pool).await?;

    // 列表
    let list_sql = format!(
        "SELECT {USER_COLUMNS} FROM users{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?"
    );
    let mut query = sqlx::query_as::<_, UserRow>(&list_sql);
    if filter.id > 0 {
        query = query.bind(filter.id);
    }
    if filter.group_id > 0 {
        query = query.bind(filter.group_id);
    }
    if has_q {
        query = query.bind(&like_q).bind(&like_q).bind(&like_q);
    } else {
        if !filter.phone.trim().is_empty() {
            query = query.bind(filter.phone.trim().to_string());
        }
        if !filter.user_name.trim().is_empty() {
            query = query.bind(format!("%{}%", filter.user_name.trim()));
        }
        if !filter.real_name.trim().is_empty() {
            query = query.bind(format!("%{}%", filter.real_name.trim()));
        }
    }
    if let Some(status) = status_value {
        query = query.bind(status);
    }
    let rows = query.bind(page_size).bind(offset).fetch_all(pool).await?;
    Ok((total, rows))
}

/// 按 ID 取会员。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<UserRow>> {
    let sql = format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?");
    Ok(sqlx::query_as::<_, UserRow>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

async fn get_by_name(pool: &MySqlPool, user_name: &str) -> anyhow::Result<Option<(u32,)>> {
    Ok(
        sqlx::query_as::<_, (u32,)>("SELECT id FROM users WHERE user_name = ? LIMIT 1")
            .bind(user_name)
            .fetch_optional(pool)
            .await?,
    )
}

/// 新建 / 更新会员。
pub async fn save(pool: &MySqlPool, input: &UserInput) -> Result<UserRow, ModelError> {
    let update_all = !input.partial;
    let now = chrono::Utc::now().timestamp();
    let password = input
        .password
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if input.id > 0 {
        let Some(mut row) = get(pool, input.id)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        else {
            return Err(ModelError::invalid("会员不存在"));
        };

        apply_str(&mut row.user_name, input.user_name.as_deref(), update_all);
        apply_str(&mut row.real_name, input.real_name.as_deref(), update_all);
        apply_str(&mut row.email, input.email.as_deref(), update_all);
        apply_str(&mut row.phone, input.phone.as_deref(), update_all);
        apply_str(&mut row.avatar_url, input.avatar_url.as_deref(), update_all);
        apply_str(&mut row.introduce, input.introduce.as_deref(), update_all);
        if let Some(group_id) = input.group_id {
            if update_all || group_id > 0 {
                row.group_id = group_id;
            }
        }
        if let Some(status) = input.status {
            row.status = status > 0;
        }
        if row.user_name.trim().is_empty() {
            return Err(ModelError::invalid("请填写用户名"));
        }
        if let Some(found) = get_by_name(pool, row.user_name.trim())
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        {
            if found.0 != row.id {
                return Err(ModelError::invalid("用户名已存在"));
            }
        }

        sqlx::query(
            "UPDATE users SET updated_time = ?, user_name = ?, real_name = ?, email = ?, \
                phone = ?, avatar_url = ?, introduce = ?, group_id = ?, status = ? WHERE id = ?",
        )
        .bind(now)
        .bind(row.user_name.trim())
        .bind(&row.real_name)
        .bind(&row.email)
        .bind(&row.phone)
        .bind(&row.avatar_url)
        .bind(&row.introduce)
        .bind(row.group_id)
        .bind(row.status)
        .bind(row.id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;

        if let Some(password) = password {
            let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)
                .map_err(|err| ModelError::invalid(format!("密码加密失败: {err}")))?;
            sqlx::query("UPDATE users SET password = ?, reset_password = 0 WHERE id = ?")
                .bind(hash)
                .bind(row.id)
                .execute(pool)
                .await
                .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
        }

        return get(pool, row.id)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .ok_or_else(|| ModelError::invalid("会员不存在"));
    }

    // 新建
    let user_name = input
        .user_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ModelError::invalid("请填写用户名"))?;
    if get_by_name(pool, user_name)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .is_some()
    {
        return Err(ModelError::invalid("用户名已存在"));
    }
    let Some(password) = password else {
        return Err(ModelError::invalid("请填写密码"));
    };
    let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)
        .map_err(|err| ModelError::invalid(format!("密码加密失败: {err}")))?;

    let mut token = sanitize_token(user_name);
    if token.is_empty() {
        token = format!("user-{}", chrono::Utc::now().timestamp_millis());
    }
    let status = input.status.unwrap_or(1) > 0;

    let id = sqlx::query(
        "INSERT INTO users (created_time, updated_time, user_name, url_token, real_name, email, \
            phone, group_id, password, status, balance, total_reward, points, order_count) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0, 0, 0)",
    )
    .bind(now)
    .bind(now)
    .bind(user_name)
    .bind(&token)
    .bind(input.real_name.as_deref().unwrap_or(""))
    .bind(input.email.as_deref().unwrap_or(""))
    .bind(input.phone.as_deref().unwrap_or(""))
    .bind(input.group_id.unwrap_or(0))
    .bind(hash)
    .bind(status)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?
    .last_insert_id() as u32;

    get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("会员创建失败"))
}

/// 变更余额（单位：分）。`amount` 不能为 0，`remark` 必填。
///
/// 说明：本库没有余额流水表，因此只改 `users.balance`，备注仅用于日志。
pub async fn change_balance(
    pool: &MySqlPool,
    user_id: u32,
    amount: i64,
    remark: &str,
) -> Result<i64, ModelError> {
    if amount == 0 {
        return Err(ModelError::invalid("请填写金额"));
    }
    if remark.trim().is_empty() {
        return Err(ModelError::invalid("请填写备注"));
    }
    if get(pool, user_id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .is_none()
    {
        return Err(ModelError::invalid("会员不存在"));
    }

    sqlx::query("UPDATE users SET balance = balance + ?, updated_time = ? WHERE id = ?")
        .bind(amount)
        .bind(chrono::Utc::now().timestamp())
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;

    let balance: i64 = sqlx::query_scalar("SELECT balance FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;
    Ok(balance)
}

/// 删除会员及其关联的认证记录。
pub async fn delete(pool: &MySqlPool, id: u32) -> Result<bool, ModelError> {
    if get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .is_none()
    {
        return Ok(false);
    }
    let _ = sqlx::query("DELETE FROM user_auths WHERE user_id = ?")
        .bind(id)
        .execute(pool)
        .await;
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?;
    Ok(true)
}

// ---------------------------- 用户组 ---------------------------------------

/// 全部用户组。
pub async fn group_list(pool: &MySqlPool) -> anyhow::Result<Vec<UserGroup>> {
    Ok(sqlx::query_as::<_, UserGroup>("SELECT * FROM user_groups ORDER BY id ASC")
        .fetch_all(pool)
        .await?)
}

/// 按 ID 取用户组。
pub async fn group_get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<UserGroup>> {
    Ok(
        sqlx::query_as::<_, UserGroup>("SELECT * FROM user_groups WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 新建 / 更新用户组（名称唯一）。
pub async fn group_save(pool: &MySqlPool, input: &UserGroupInput) -> Result<UserGroup, ModelError> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err(ModelError::invalid("请填写用户组名称"));
    }
    let exists: Option<u32> =
        sqlx::query_scalar("SELECT id FROM user_groups WHERE title = ? LIMIT 1")
            .bind(title)
            .fetch_optional(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;
    if let Some(found) = exists {
        if found != input.id {
            return Err(ModelError::invalid("用户组名称已存在"));
        }
    }

    let now = chrono::Utc::now().timestamp();
    let status = input.status.unwrap_or(1) > 0;
    let setting = input
        .setting
        .as_ref()
        .map(|value| value.to_string())
        .or_else(|| None);

    let id = if input.id > 0 {
        sqlx::query(
            "UPDATE user_groups SET updated_time = ?, title = ?, description = ?, level = ?, \
                status = ?, price = ?, setting = COALESCE(?, setting) WHERE id = ?",
        )
        .bind(now)
        .bind(title)
        .bind(input.description.trim())
        .bind(input.level)
        .bind(status)
        .bind(input.price)
        .bind(setting)
        .bind(input.id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
        input.id
    } else {
        sqlx::query(
            "INSERT INTO user_groups (created_time, updated_time, title, description, level, \
                status, price, setting) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(now)
        .bind(now)
        .bind(title)
        .bind(input.description.trim())
        .bind(input.level)
        .bind(status)
        .bind(input.price)
        .bind(setting)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?
        .last_insert_id() as u32
    };

    group_get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("用户组不存在"))
}

/// 删除用户组（组内还有会员时拒绝）。
pub async fn group_delete(pool: &MySqlPool, id: u32) -> Result<bool, ModelError> {
    if group_get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .is_none()
    {
        return Ok(false);
    }
    let members: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE group_id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;
    if members > 0 {
        return Err(ModelError::invalid(format!(
            "该用户组下还有 {members} 个会员，不能删除"
        )));
    }
    sqlx::query("DELETE FROM user_groups WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, setting, test_pool};

    #[test]
    fn validates_field_names() {
        assert!(valid_field_name("author"));
        assert!(valid_field_name("wx_openid"));
        assert!(!valid_field_name("Author"));
        assert!(!valid_field_name("1abc"));
        assert!(!valid_field_name("a"));
        assert!(!valid_field_name(""));
    }

    #[test]
    fn sanitizes_tokens() {
        assert_eq!(sanitize_token("John Doe"), "john-doe");
        assert_eq!(sanitize_token("中文"), "");
    }

    #[tokio::test]
    async fn user_crud_and_balance() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM users WHERE user_name LIKE 'rustcms-user%'")
            .execute(&pool)
            .await;

        // 缺用户名 / 密码
        assert_eq!(
            save(&pool, &UserInput::default()).await.unwrap_err().to_string(),
            "请填写用户名"
        );
        assert_eq!(
            save(
                &pool,
                &UserInput {
                    user_name: Some("rustcms-user".into()),
                    ..Default::default()
                }
            )
            .await
            .unwrap_err()
            .to_string(),
            "请填写密码"
        );

        let created = save(
            &pool,
            &UserInput {
                user_name: Some("rustcms-user".into()),
                real_name: Some("测试会员".into()),
                email: Some("rustcms-user@example.com".into()),
                password: Some("pass-123456".into()),
                group_id: Some(0),
                status: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(created.id > 0);
        assert!(created.status);
        assert_eq!(created.url_token, "rustcms-user");

        // 重名
        let err = save(
            &pool,
            &UserInput {
                user_name: Some("rustcms-user".into()),
                password: Some("x".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "用户名已存在");

        // 列表筛选
        let filter = UserFilter {
            q: "rustcms-user".into(),
            ..Default::default()
        };
        let (total, rows) = list(&pool, &filter, 1, 20).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].id, created.id);

        let filter = UserFilter {
            status: "blocked".into(),
            ..Default::default()
        };
        let (total, _) = list(&pool, &filter, 1, 20).await.unwrap();
        assert_eq!(total, 0);

        // PATCH：只改手机号
        let patched = save(
            &pool,
            &UserInput {
                id: created.id,
                phone: Some("13800000000".into()),
                partial: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(patched.phone, "13800000000");
        assert_eq!(patched.user_name, "rustcms-user");

        // 余额
        assert_eq!(
            change_balance(&pool, created.id, 0, "备注")
                .await
                .unwrap_err()
                .to_string(),
            "请填写金额"
        );
        assert_eq!(
            change_balance(&pool, created.id, 100, " ")
                .await
                .unwrap_err()
                .to_string(),
            "请填写备注"
        );
        assert_eq!(
            change_balance(&pool, created.id, 100, "充值")
                .await
                .unwrap(),
            100
        );
        assert_eq!(
            change_balance(&pool, created.id, -30, "扣费")
                .await
                .unwrap(),
            70
        );

        // 删除
        assert!(delete(&pool, created.id).await.unwrap());
        assert!(get(&pool, created.id).await.unwrap().is_none());
        assert!(!delete(&pool, created.id).await.unwrap());
    }

    #[tokio::test]
    async fn group_crud_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM user_groups WHERE title LIKE 'rustcms-组%'")
            .execute(&pool)
            .await;

        assert_eq!(
            group_save(&pool, &UserGroupInput::default())
                .await
                .unwrap_err()
                .to_string(),
            "请填写用户组名称"
        );

        let created = group_save(
            &pool,
            &UserGroupInput {
                title: "rustcms-组".into(),
                description: "测试组".into(),
                level: 2,
                status: Some(1),
                price: 500,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(created.id > 0);
        assert_eq!(created.level, 2);
        assert!(created.status);

        // 重名
        let err = group_save(
            &pool,
            &UserGroupInput {
                title: "rustcms-组".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "用户组名称已存在");

        // 列表
        let list = group_list(&pool).await.unwrap();
        assert!(list.iter().any(|item| item.id == created.id));

        // 删除
        assert!(group_delete(&pool, created.id).await.unwrap());
        assert!(!group_delete(&pool, created.id).await.unwrap());
    }

    #[tokio::test]
    async fn setting_validates_and_dedupes() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, SETTING_KEY).await.unwrap();

        // 非法字段名
        let err = setting_save(
            &pool,
            &json!({"fields": [{"name": "作者", "field_name": "Author"}]}),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("命名不正确"), "{err}");

        let saved = setting_save(
            &pool,
            &json!({
                "default_group_id": 2,
                "fields": [
                    {"name": "作者", "field_name": "author", "required": true},
                    {"name": "作者2", "field_name": "author"},
                    {"name": "微信", "field_name": "wx_openid"}
                ]
            }),
        )
        .await
        .unwrap();
        let fields = saved["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 2, "{saved}");
        // required 被强制为 false
        assert_eq!(fields[0]["required"], json!(false));

        match backup {
            Some(raw) => setting::save_raw(&pool, SETTING_KEY, &raw).await.unwrap(),
            None => setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
    }
}

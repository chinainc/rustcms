//! 会员与账户数据访问（批次 B）。
//!
//! 覆盖范围：
//! - 会员资料 / 登录 / 注册 / 改密码 / 找回密码；
//! - 评论管理 / 留言管理 / 我的发布；
//! - 实名 / 企业认证；
//! - 会员开放接口 Key；
//! - 积分 / 签到；
//! - 我的发布 / 资金流水。
//!
//! 类型约定（与既有表结构共用同一份库，列类型必须对齐）：
//! - `users.id` / `group_id` 是 `int(10) unsigned` → `u32`；
//! - `users.parent_id` 是 `int(10)` 有符号 → `i32`；
//! - `status` / `is_retailer` 是 `tinyint(1)`，但语义上可以是 `-1`（封禁），
//! 驱动默认按 BOOLEAN 解析会把它变成 `true`，所以统一
//! `CAST(col AS SIGNED)` 取成整数；
//! - `bigint(20)` 有符号 → `i64`；`int(10) unsigned` → `u32`。

use rand::Rng;
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::ModelError;


/// 签到基础积分。
pub const SIGN_BASE_POINTS: i64 = 5;
/// 连续签到额外积分上限。
pub const SIGN_EXTRA_MAX: i64 = 10;
/// 积分 / 资金流水单页上限。
pub const POINT_LOG_LIMIT: i64 = 50;
/// 会员开放接口 Key 长度。
pub const MEMBER_API_TOKEN_LENGTH: usize = 40;
/// 会员开放接口 REST 前缀。
pub const MEMBER_OPEN_API_BASE_PATH: &str = "/api";
/// 会员专属 MCP 端点。
pub const MEMBER_MCP_PATH: &str = "/api/open/mcp";
/// 认证状态。
pub const USER_AUTH_STATUS_PENDING: i32 = 0;
pub const USER_AUTH_STATUS_PASSED: i32 = 1;
pub const USER_AUTH_STATUS_REJECTED: i32 = -1;
/// 认证类型。
pub const USER_AUTH_TYPE_PERSON: i32 = 1;
pub const USER_AUTH_TYPE_COMPANY: i32 = 2;

/// `users` 表列集（`status` / `is_retailer` 用 CAST 取整数）。
const MEMBER_COLUMNS: &str = "id, created_time, updated_time, parent_id, user_name, url_token, \
    real_name, first_name, last_name, birthday, avatar_url, introduce, email, email_verified, \
    phone, group_id, google_id, reset_password, CAST(status AS SIGNED) AS status, \
    CAST(is_retailer AS SIGNED) AS is_retailer, balance, points, total_reward, invite_code, \
    last_login, expire_time, subscribed, order_count, password";

// ============================================================================
// 会员
// ============================================================================

/// 会员行（`users` 表）。
#[derive(Debug, Clone, FromRow)]
pub struct MemberRow {
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
    pub status: i32,
    pub is_retailer: i32,
    pub balance: i64,
    pub points: i64,
    pub total_reward: i64,
    pub invite_code: String,
    pub last_login: Option<i64>,
    pub expire_time: Option<i64>,
    pub subscribed: bool,
    pub order_count: i64,
    /// 密码散列（只用于登录/改密比对，**不进 JSON**）。
    pub password: String,
}

impl MemberRow {
    /// 会员行的 JSON 形态（不含密码、不含 `api_token`）。
    ///
    /// 字段顺序与结构体声明顺序一致（`serde_json` 开了 `preserve_order`）。
    pub fn to_json(&self) -> Value {
        self.to_json_with_token(None)
    }

    /// 同 [`MemberRow::to_json`]，但带上登录令牌（在 `user.Token` 里放 JWT）。
    pub fn to_json_with_token(&self, token: Option<&str>) -> Value {
        json!({
            "id": self.id,
            "created_time": self.created_time.unwrap_or(0),
            "updated_time": self.updated_time.unwrap_or(0),
            "parent_id": self.parent_id,
            "user_name": self.user_name,
            "url_token": self.url_token,
            "real_name": self.real_name,
            "first_name": self.first_name,
            "last_name": self.last_name,
            "birthday": self.birthday.unwrap_or(0),
            "avatar_url": self.avatar_url,
            "introduce": self.introduce,
            "email": self.email,
            "email_verified": self.email_verified,
            "phone": self.phone,
            "group_id": self.group_id,
            "google_id": self.google_id,
            "reset_password": self.reset_password,
            "status": self.status,
            "is_retailer": self.is_retailer,
            "balance": self.balance,
            "points": self.points,
            "total_reward": self.total_reward,
            "invite_code": self.invite_code,
            "last_login": self.last_login.unwrap_or(0),
            "expire_time": self.expire_time.unwrap_or(0),
            "subscribed": self.subscribed,
            "order_count": self.order_count,
            "extra": Value::Null,
            "token": token.map(|value| Value::String(value.to_string())).unwrap_or(Value::Null),
            "group": Value::Null,
            "full_avatar_url": self.avatar_url,
            "link": "",
        })
    }
}

/// 按 ID 取会员。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<MemberRow>> {
    let sql = format!("SELECT {MEMBER_COLUMNS} FROM users WHERE id = ?");
    Ok(sqlx::query_as::<_, MemberRow>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// 按用户名取会员。
pub async fn get_by_name(pool: &MySqlPool, user_name: &str) -> anyhow::Result<Option<MemberRow>> {
    let sql = format!("SELECT {MEMBER_COLUMNS} FROM users WHERE user_name = ? LIMIT 1");
    Ok(sqlx::query_as::<_, MemberRow>(&sql)
        .bind(user_name)
        .fetch_optional(pool)
        .await?)
}

/// 按邮箱取会员。
pub async fn get_by_email(pool: &MySqlPool, email: &str) -> anyhow::Result<Option<MemberRow>> {
    let sql = format!("SELECT {MEMBER_COLUMNS} FROM users WHERE email = ? LIMIT 1");
    Ok(sqlx::query_as::<_, MemberRow>(&sql)
        .bind(email)
        .fetch_optional(pool)
        .await?)
}

/// 按手机号取会员。
pub async fn get_by_phone(pool: &MySqlPool, phone: &str) -> anyhow::Result<Option<MemberRow>> {
    let sql = format!("SELECT {MEMBER_COLUMNS} FROM users WHERE phone = ? LIMIT 1");
    Ok(sqlx::query_as::<_, MemberRow>(&sql)
        .bind(phone)
        .fetch_optional(pool)
        .await?)
}

/// 记录登录时间（`user.LogLogin`）。
pub async fn touch_login(pool: &MySqlPool, id: u32) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query("UPDATE users SET last_login = ?, updated_time = ? WHERE id = ?")
        .bind(now)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 标记邮箱已验证。
pub async fn set_email_verified(pool: &MySqlPool, id: u32) -> anyhow::Result<()> {
    sqlx::query("UPDATE users SET email_verified = 1, updated_time = ? WHERE id = ?")
        .bind(chrono::Utc::now().timestamp())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 会员资料更新入参（对齐 `UserRequest`，只覆盖非空字段）。
#[derive(Debug, Clone, Default)]
pub struct ProfileInput {
    pub user_name: Option<String>,
    pub real_name: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub avatar_url: Option<String>,
    pub introduce: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub birthday: Option<i64>,
}

/// 更新会员资料（`UpdateUserInfo`）。
///
/// 只有非空字段会被覆盖；用户名 / 手机号 / 邮箱要查重。
pub async fn update_profile(
    pool: &MySqlPool,
    id: u32,
    input: &ProfileInput,
) -> Result<(), ModelError> {
    let Some(mut row) = get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
    else {
        return Err(ModelError::invalid("用户不存在"));
    };

    if let Some(user_name) = input.user_name.as_deref().map(str::trim) {
        if !user_name.is_empty() {
            if let Some(found) = get_by_name(pool, user_name)
                .await
                .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            {
                if found.id != id {
                    return Err(ModelError::invalid("该用户名已被注册"));
                }
            }
            row.user_name = user_name.to_string();
        }
    }
    if let Some(phone) = input.phone.as_deref().map(str::trim) {
        if !phone.is_empty() {
            if let Some(found) = get_by_phone(pool, phone)
                .await
                .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            {
                if found.id != id {
                    return Err(ModelError::invalid("该手机号已被注册"));
                }
            }
            row.phone = phone.to_string();
        }
    }
    if let Some(email) = input.email.as_deref().map(str::trim) {
        if !email.is_empty() {
            if !valid_email(email) {
                return Err(ModelError::invalid("邮箱不正确"));
            }
            if let Some(found) = get_by_email(pool, email)
                .await
                .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            {
                if found.id != id {
                    return Err(ModelError::invalid("该邮箱已被注册"));
                }
            }
            row.email = email.to_string();
        }
    }
    if let Some(real_name) = input.real_name.as_deref() {
        row.real_name = real_name.trim().to_string();
    }
    if let Some(first_name) = input.first_name.as_deref() {
        row.first_name = first_name.trim().to_string();
    }
    if let Some(last_name) = input.last_name.as_deref() {
        row.last_name = last_name.trim().to_string();
    }
    if !row.first_name.is_empty() && !row.last_name.is_empty() && row.real_name.is_empty() {
        row.real_name = format!("{} {}", row.first_name, row.last_name);
    }
    if let Some(avatar_url) = input.avatar_url.as_deref() {
        row.avatar_url = avatar_url.trim().to_string();
    }
    if let Some(introduce) = input.introduce.as_deref() {
        row.introduce = introduce.trim().to_string();
    }
    if let Some(birthday) = input.birthday {
        row.birthday = Some(birthday);
    }
    if row.group_id == 0 {
        row.group_id = default_group_id(pool).await;
    }

    sqlx::query(
        "UPDATE users SET updated_time = ?, user_name = ?, real_name = ?, first_name = ?, \
            last_name = ?, birthday = ?, avatar_url = ?, introduce = ?, email = ?, phone = ?, \
            group_id = ? WHERE id = ?",
    )
    .bind(chrono::Utc::now().timestamp())
    .bind(&row.user_name)
    .bind(&row.real_name)
    .bind(&row.first_name)
    .bind(&row.last_name)
    .bind(row.birthday)
    .bind(&row.avatar_url)
    .bind(&row.introduce)
    .bind(&row.email)
    .bind(&row.phone)
    .bind(row.group_id)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    Ok(())
}

/// 更新密码（`user.EncryptPassword` + `DB.Save(user)`）。
pub async fn update_password(pool: &MySqlPool, id: u32, hash: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE users SET password = ?, reset_password = 0, updated_time = ? WHERE id = ?")
        .bind(hash)
        .bind(chrono::Utc::now().timestamp())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 注册入参（对齐 `ApiRegisterRequest`）。
#[derive(Debug, Clone, Default)]
pub struct RegisterInput {
    pub user_name: String,
    pub password: String,
    pub real_name: String,
    pub first_name: String,
    pub last_name: String,
    pub avatar_url: String,
    pub introduce: String,
    pub email: String,
    pub phone: String,
    pub birthday: i64,
    /// 邀请人会员 ID（从 cookie `invite` 读）。
    pub parent_id: u32,
}

/// 注册会员，返回新会员 ID。
///
/// 校验顺序与 `RegisterUser` 一致：
/// 用户名/密码必填 → 密码至少 6 位 → 用户名查重 → 手机号格式与查重 →
/// 邮箱格式与查重 → 邮箱与手机号至少填一个。
pub async fn register(pool: &MySqlPool, input: &RegisterInput) -> Result<u32, ModelError> {
    let user_name = input.user_name.trim();
    if user_name.is_empty() || input.password.trim().is_empty() {
        return Err(ModelError::invalid("请正确填写用户名和密码"));
    }
    if input.password.trim().chars().count() < 6 {
        return Err(ModelError::invalid("请填写6位以上的密码"));
    }
    if get_by_name(pool, user_name)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .is_some()
    {
        return Err(ModelError::invalid("该用户名已被注册"));
    }

    let phone = input.phone.trim();
    if !phone.is_empty() {
        if !valid_phone(phone) {
            return Err(ModelError::invalid("手机号不正确"));
        }
        if get_by_phone(pool, phone)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .is_some()
        {
            return Err(ModelError::invalid("该手机号已被注册"));
        }
    }

    let email = input.email.trim();
    if !email.is_empty() {
        if !valid_email(email) {
            return Err(ModelError::invalid("邮箱不正确"));
        }
        if get_by_email(pool, email)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .is_some()
        {
            return Err(ModelError::invalid("该邮箱已被注册"));
        }
    }
    if phone.is_empty() && email.is_empty() {
        return Err(ModelError::invalid("邮箱和手机号至少填写一个"));
    }

    let hash = crate::auth::hash_password(input.password.trim())
        .map_err(|err| ModelError::invalid(format!("密码加密失败: {err}")))?;
    let now = chrono::Utc::now().timestamp();
    let group_id = default_group_id(pool).await;
    let status = default_status(pool).await;
    let url_token = sanitize_token(user_name);
    let invite_code = random_code(8);

    let id = sqlx::query(
        "INSERT INTO users (created_time, updated_time, parent_id, user_name, url_token, \
            real_name, first_name, last_name, birthday, avatar_url, introduce, email, \
            email_verified, phone, group_id, google_id, password, reset_password, status, \
            is_retailer, balance, points, total_reward, invite_code, last_login, expire_time, \
            subscribed, order_count, api_token) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, '', ?, 0, ?, 0, 0, 0, 0, ?, 0, 0, 0, 0, '')",
    )
    .bind(now)
    .bind(now)
    .bind(input.parent_id as i32)
    .bind(user_name)
    .bind(&url_token)
    .bind(input.real_name.trim())
    .bind(input.first_name.trim())
    .bind(input.last_name.trim())
    .bind(input.birthday)
    .bind(input.avatar_url.trim())
    .bind(input.introduce.trim())
    .bind(email)
    .bind(phone)
    .bind(group_id)
    .bind(&hash)
    .bind(status)
    .bind(&invite_code)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?
    .last_insert_id() as u32;

    Ok(id)
}

/// 取会员当前的开放接口 Key（空串表示未开通）。
pub async fn api_token(pool: &MySqlPool, id: u32) -> anyhow::Result<String> {
    let token: Option<String> = sqlx::query_scalar("SELECT api_token FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(token.unwrap_or_default())
}

/// 生成（或重新生成）开放接口 Key，旧 Key 立即失效。
pub async fn reset_api_token(pool: &MySqlPool, id: u32) -> anyhow::Result<String> {
    for _ in 0..5 {
        let token = random_code(MEMBER_API_TOKEN_LENGTH);
        let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE api_token = ?")
            .bind(&token)
            .fetch_one(pool)
            .await?;
        if exists > 0 {
            continue;
        }
        sqlx::query("UPDATE users SET api_token = ?, updated_time = ? WHERE id = ?")
            .bind(&token)
            .bind(chrono::Utc::now().timestamp())
            .bind(id)
            .execute(pool)
            .await?;
        return Ok(token);
    }
    Err(anyhow::anyhow!("系统繁忙，请稍后重试"))
}

/// 停用开放接口 Key。
pub async fn clear_api_token(pool: &MySqlPool, id: u32) -> anyhow::Result<()> {
    sqlx::query("UPDATE users SET api_token = '', updated_time = ? WHERE id = ?")
        .bind(chrono::Utc::now().timestamp())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 展示用掩码：`abcd****wxyz`（`MaskApiToken`）。
pub fn mask_api_token(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    if chars.len() <= 10 {
        return format!("{}****", chars.iter().take(2).collect::<String>());
    }
    format!(
        "{}****{}",
        chars.iter().take(4).collect::<String>(),
        chars.iter().skip(chars.len() - 4).collect::<String>()
    )
}

/// 会员分组（`user_groups` 表）。
#[derive(Debug, Clone, FromRow)]
pub struct MemberGroup {
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

impl MemberGroup {
    /// 解析 `setting` 列（`UserGroupSetting`）。
    pub fn setting_value(&self) -> Value {
        self.setting
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}))
    }

    /// 优惠比例（`setting.discount`，百分数）。
    pub fn discount(&self) -> i64 {
        self.setting_value()
            .get("discount")
            .and_then(Value::as_i64)
            .unwrap_or(0)
    }

    /// `UserGroup` JSON 形态（含 `favorable_price`）。
    pub fn to_json(&self, favorable_price: i64) -> Value {
        json!({
            "id": self.id,
            "created_time": self.created_time.unwrap_or(0),
            "updated_time": self.updated_time.unwrap_or(0),
            "title": self.title,
            "description": self.description,
            "level": self.level,
            "status": self.status,
            "price": self.price,
            "setting": self.setting_value(),
            "favorable_price": favorable_price,
        })
    }
}

/// 全部用户组（`GetUserGroups`：按 level、id 升序）。
pub async fn groups(pool: &MySqlPool) -> anyhow::Result<Vec<MemberGroup>> {
    Ok(sqlx::query_as::<_, MemberGroup>(
        "SELECT id, created_time, updated_time, title, description, level, status, price, setting \
         FROM user_groups ORDER BY level ASC, id ASC",
    )
    .fetch_all(pool)
    .await?)
}

/// 按 ID 取用户组。
pub async fn group(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<MemberGroup>> {
    Ok(sqlx::query_as::<_, MemberGroup>(
        "SELECT id, created_time, updated_time, title, description, level, status, price, setting \
         FROM user_groups WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

// ============================================================================
// 我的发布 / 资金流水 / 积分流水
// ============================================================================

/// 「我的发布」条目（`UserArchiveItem`）。
#[derive(Debug, Clone, FromRow)]
pub struct PublishedItem {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub views: u32,
    pub created_time: Option<i64>,
    pub module_id: u32,
    pub category_id: u32,
    pub url_token: String,
    pub images: Option<String>,
    pub cat_token: Option<String>,
    pub module_token: Option<String>,
    pub draft: bool,
    pub status: i32,
}

/// 我的发布：正式内容 + 待审核草稿（`GetUserPublished`）。
///
/// 先把草稿与正式内容合并，再按 `created_time` 倒序，最后做内存分页。
pub async fn published(
    pool: &MySqlPool,
    user_id: u32,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Value>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 || page_size > 50 {
        50
    } else {
        page_size
    };
    if user_id == 0 {
        return Ok((0, Vec::new()));
    }

    let mut items: Vec<PublishedItem> = Vec::new();

    // 草稿：status = 99（ContentStatusDelete）是回收站，不进「我的发布」
    let drafts = sqlx::query_as::<_, PublishedItem>(
        "SELECT d.id, d.title, d.description, d.views, d.created_time, d.module_id, \
            d.category_id, d.url_token, d.images, c.url_token AS cat_token, \
            m.url_token AS module_token, 1 AS draft, CAST(d.status AS SIGNED) AS status \
         FROM archive_drafts d \
         LEFT JOIN categories c ON c.id = d.category_id \
         LEFT JOIN modules m ON m.id = d.module_id \
         WHERE d.user_id = ? AND d.status <> 99",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    items.extend(drafts);

    let archives = sqlx::query_as::<_, PublishedItem>(
        "SELECT a.id, a.title, a.description, a.views, a.created_time, a.module_id, \
            a.category_id, a.url_token, a.images, c.url_token AS cat_token, \
            m.url_token AS module_token, 0 AS draft, 1 AS status \
         FROM archives a \
         LEFT JOIN categories c ON c.id = a.category_id \
         LEFT JOIN modules m ON m.id = a.module_id \
         WHERE a.user_id = ?",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    items.extend(archives);

    // 按创建时间倒序（用的是稳定排序）
    items.sort_by(|a, b| {
        b.created_time
            .unwrap_or(0)
            .cmp(&a.created_time.unwrap_or(0))
            .then(b.id.cmp(&a.id))
    });

    let total = items.len() as i64;
    let start = ((page - 1) * page_size) as usize;
    if start >= items.len() {
        return Ok((total, Vec::new()));
    }
    let end = ((start as i64 + page_size) as usize).min(items.len());

    let mode = crate::models::url::load_mode(pool).await.unwrap_or(2);
    let list = items[start..end]
        .iter()
        .map(|item| {
            let link = crate::models::url::archive_url(
                mode,
                item.cat_token.as_deref().unwrap_or(""),
                item.module_token.as_deref().unwrap_or(""),
                item.id,
                &item.url_token,
                item.category_id,
                1,
            );
            json!({
                "id": item.id,
                "title": item.title,
                "description": item.description,
                "link": link,
                "thumb": first_image(item.images.as_deref()),
                "views": item.views,
                "created_time": item.created_time.unwrap_or(0),
                "module_id": item.module_id,
                "draft": item.draft,
                "status": item.status,
            })
        })
        .collect();
    Ok((total, list))
}

/// 资金流水（`GetFinanceLogs`）。
pub async fn finances(
    pool: &MySqlPool,
    user_id: u32,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Value>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 || page_size > POINT_LOG_LIMIT {
        POINT_LOG_LIMIT
    } else {
        page_size
    };
    if user_id == 0 {
        return Ok((0, Vec::new()));
    }
    let offset = (page - 1) * page_size;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM finances WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await?;

    let rows = sqlx::query(
        "SELECT id, created_time, updated_time, user_id, CAST(direction AS SIGNED) AS direction, \
            amount, after_amount, CAST(action AS SIGNED) AS action, order_id, \
            CAST(status AS SIGNED) AS status, remark \
         FROM finances WHERE user_id = ? ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    use sqlx::Row;
    let list = rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<u32, _>("id"),
                "created_time": row.get::<Option<i64>, _>("created_time").unwrap_or(0),
                "updated_time": row.get::<Option<i64>, _>("updated_time").unwrap_or(0),
                "user_id": row.get::<u32, _>("user_id"),
                "direction": row.get::<i32, _>("direction"),
                "amount": row.get::<i64, _>("amount"),
                "after_amount": row.get::<i64, _>("after_amount"),
                "action": row.get::<i32, _>("action"),
                "order_id": row.get::<String, _>("order_id"),
                "status": row.get::<i32, _>("status"),
                "remark": row.get::<String, _>("remark"),
                "user_name": "",
            })
        })
        .collect();
    Ok((total, list))
}

/// 积分流水（`GetPointLogs`）。
pub async fn point_logs(
    pool: &MySqlPool,
    user_id: u32,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Value>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 || page_size > POINT_LOG_LIMIT {
        POINT_LOG_LIMIT
    } else {
        page_size
    };
    if user_id == 0 {
        return Ok((0, Vec::new()));
    }
    let offset = (page - 1) * page_size;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM point_logs WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await?;

    let rows = sqlx::query(
        "SELECT id, created_time, updated_time, user_id, points, after_points, \
            CAST(`type` AS SIGNED) AS `type`, remark \
         FROM point_logs WHERE user_id = ? ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    use sqlx::Row;
    let list = rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<u32, _>("id"),
                "created_time": row.get::<Option<i64>, _>("created_time").unwrap_or(0),
                "updated_time": row.get::<Option<i64>, _>("updated_time").unwrap_or(0),
                "user_id": row.get::<u32, _>("user_id"),
                "points": row.get::<i64, _>("points"),
                "after_points": row.get::<i64, _>("after_points"),
                "type": row.get::<i32, _>("type"),
                "remark": row.get::<String, _>("remark"),
            })
        })
        .collect();
    Ok((total, list))
}

// ============================================================================
// 评论管理（我发布的文档收到的评论）
// ============================================================================

/// 评论行（`comments` 表 + 所属文档标题）。
#[derive(Debug, Clone, FromRow)]
pub struct CommentRow {
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
    pub status: i32,
    pub rating: i32,
    pub archive_title: String,
}

impl CommentRow {
    /// `Comment` JSON 形态 + `archive_title` / `archive_url` / `parent_content`。
    pub fn to_json(&self, archive_url: &str, parent_content: &str) -> Value {
        json!({
            "id": self.id,
            "created_time": self.created_time.unwrap_or(0),
            "updated_time": self.updated_time.unwrap_or(0),
            "archive_id": self.archive_id,
            "user_id": self.user_id,
            "user_name": self.user_name,
            "email": self.email,
            "ip": self.ip,
            "vote_count": self.vote_count,
            "content": self.content.clone().unwrap_or_default(),
            "parent_id": self.parent_id,
            "to_uid": self.to_uid,
            "status": self.status,
            "rating": self.rating,
            "item_title": "",
            "parent": Value::Null,
            "active": false,
            "archive_title": self.archive_title,
            "archive_url": archive_url,
            "parent_content": parent_content,
        })
    }
}

const COMMENT_COLUMNS: &str = "c.id, c.created_time, c.updated_time, c.archive_id, c.user_id, \
    c.user_name, c.email, c.ip, c.vote_count, c.content, c.parent_id, c.to_uid, \
    CAST(c.status AS SIGNED) AS status, CAST(c.rating AS SIGNED) AS rating, a.title AS archive_title";

/// 我发布的文档收到的评论（`GetUserArchiveComments`）。
///
/// `status < 0` 表示不过滤状态。
pub async fn comments(
    pool: &MySqlPool,
    user_id: u32,
    status: i32,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Value>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 {
        12
    } else {
        page_size
    };
    if user_id == 0 {
        return Ok((0, Vec::new()));
    }
    let offset = (page - 1) * page_size;

    let mut where_sql = String::from(" WHERE a.user_id = ?");
    if status >= 0 {
        where_sql.push_str(" AND c.status = ?");
    }

    let count_sql = format!(
        "SELECT COUNT(*) FROM comments c INNER JOIN archives a ON a.id = c.archive_id{where_sql}"
    );
    let mut counter = sqlx::query_scalar::<_, i64>(&count_sql).bind(user_id);
    if status >= 0 {
        counter = counter.bind(status);
    }
    let total = counter.fetch_one(pool).await?;

    let list_sql = format!(
        "SELECT {COMMENT_COLUMNS} FROM comments c \
         INNER JOIN archives a ON a.id = c.archive_id{where_sql} \
         ORDER BY c.id DESC LIMIT ? OFFSET ?"
    );
    let mut query = sqlx::query_as::<_, CommentRow>(&list_sql).bind(user_id);
    if status >= 0 {
        query = query.bind(status);
    }
    let rows = query.bind(page_size).bind(offset).fetch_all(pool).await?;

    let mode = crate::models::url::load_mode(pool).await.unwrap_or(2);
    let mut list = Vec::with_capacity(rows.len());
    for row in rows {
        let url = archive_link(pool, mode, row.archive_id).await;
        let parent_content = if row.parent_id > 0 {
            comment_content(pool, row.parent_id)
                .await
                .ok()
                .flatten()
                .unwrap_or_default()
        } else {
            String::new()
        };
        list.push(row.to_json(&url, &parent_content));
    }
    Ok((total, list))
}

/// 待审核 / 全部评论数（`GetUserCommentStat`）。
pub async fn comment_stat(pool: &MySqlPool, user_id: u32) -> anyhow::Result<(i64, i64)> {
    if user_id == 0 {
        return Ok((0, 0));
    }
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM comments c INNER JOIN archives a ON a.id = c.archive_id \
         WHERE a.user_id = ?",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM comments c INNER JOIN archives a ON a.id = c.archive_id \
         WHERE a.user_id = ? AND c.status = 0",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok((pending, total))
}

/// 取评论并校验它属于「我发布的文档」（`GetUserArchiveComment`）。
pub async fn own_comment(
    pool: &MySqlPool,
    user_id: u32,
    comment_id: u32,
) -> Result<CommentRow, ModelError> {
    if user_id == 0 || comment_id == 0 {
        return Err(ModelError::invalid("文档不存在"));
    }
    let sql = format!(
        "SELECT {COMMENT_COLUMNS} FROM comments c \
         INNER JOIN archives a ON a.id = c.archive_id WHERE c.id = ?"
    );
    let row = sqlx::query_as::<_, CommentRow>(&sql)
        .bind(comment_id)
        .fetch_optional(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("评论不存在"))?;

    let owner: Option<u32> =
        sqlx::query_scalar("SELECT user_id FROM archives WHERE id = ?")
            .bind(row.archive_id)
            .fetch_optional(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;
    if owner != Some(user_id) {
        return Err(ModelError::invalid("无权操作该内容"));
    }
    Ok(row)
}

/// 改评论状态（0 未处理 / 1 正常 / 3 垃圾）。
pub async fn set_comment_status(pool: &MySqlPool, id: u32, status: i32) -> anyhow::Result<()> {
    sqlx::query("UPDATE comments SET status = ?, updated_time = ? WHERE id = ?")
        .bind(status)
        .bind(chrono::Utc::now().timestamp())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 删除评论，并同步文档的评论数（`comment.Delete`）。
pub async fn delete_comment(pool: &MySqlPool, id: u32, archive_id: i64) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM comments WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM comments WHERE archive_id = ?")
        .bind(archive_id)
        .fetch_one(pool)
        .await?;
    sqlx::query("UPDATE archives SET comment_count = ? WHERE id = ?")
        .bind(total as u32)
        .bind(archive_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 回复评论（`ApiUserCommentReply`）。
pub async fn reply_comment(
    pool: &MySqlPool,
    user_id: u32,
    user_name: &str,
    comment: &CommentRow,
    content: &str,
    ip: &str,
) -> Result<u32, ModelError> {
    let now = chrono::Utc::now().timestamp();
    let id = sqlx::query(
        "INSERT INTO comments (created_time, updated_time, archive_id, user_id, user_name, \
            email, ip, vote_count, content, parent_id, to_uid, status, rating) \
         VALUES (?, ?, ?, ?, ?, '', ?, 0, ?, ?, ?, 1, 0)",
    )
    .bind(now)
    .bind(now)
    .bind(comment.archive_id)
    .bind(user_id)
    .bind(user_name)
    .bind(ip)
    .bind(content)
    .bind(comment.id)
    .bind(comment.user_id)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?
    .last_insert_id() as u32;

    // 文档评论数保持准确（`reply.UpdateCommentCount`）
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM comments WHERE archive_id = ?")
        .bind(comment.archive_id)
        .fetch_one(pool)
        .await?;
    let _ = sqlx::query("UPDATE archives SET comment_count = ? WHERE id = ?")
        .bind(total as u32)
        .bind(comment.archive_id)
        .execute(pool)
        .await;
    Ok(id)
}

/// 读评论正文（用于展示被回复的内容）。
pub async fn comment_content(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT content FROM comments WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

// ============================================================================
// 留言管理
// ============================================================================

/// 留言行（`guestbooks` 表）。
#[derive(Debug, Clone, FromRow)]
pub struct GuestbookRow {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_id: u32,
    pub user_name: String,
    pub contact: String,
    pub content: Option<String>,
    pub ip: String,
    pub status: i32,
    pub refer: String,
    pub site_id: u32,
    pub extra_data: Option<String>,
}

impl GuestbookRow {
    /// `Guestbook` JSON 形态。
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "created_time": self.created_time.unwrap_or(0),
            "updated_time": self.updated_time.unwrap_or(0),
            "user_id": self.user_id,
            "user_name": self.user_name,
            "contact": self.contact,
            "content": self.content.clone().unwrap_or_default(),
            "ip": self.ip,
            "status": self.status,
            "refer": self.refer,
            "site_id": self.site_id,
            "extra_data": self
                .extra_data
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .unwrap_or(Value::Null),
        })
    }
}

/// 我提交的留言（`GetUserGuestbooks`）。
pub async fn guestbooks(
    pool: &MySqlPool,
    user_id: u32,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Value>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 {
        12
    } else {
        page_size
    };
    if user_id == 0 {
        return Ok((0, Vec::new()));
    }
    let offset = (page - 1) * page_size;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM guestbooks WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await?;

    let rows = sqlx::query_as::<_, GuestbookRow>(
        "SELECT id, created_time, updated_time, user_id, user_name, contact, content, ip, \
            CAST(status AS SIGNED) AS status, refer, site_id, extra_data \
         FROM guestbooks WHERE user_id = ? ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok((total, rows.iter().map(GuestbookRow::to_json).collect()))
}

/// 删除自己的留言（`DeleteUserGuestbook`）。
pub async fn delete_guestbook(pool: &MySqlPool, user_id: u32, id: u32) -> Result<(), ModelError> {
    if user_id == 0 || id == 0 {
        return Err(ModelError::invalid("留言不存在"));
    }
    let affected = sqlx::query("DELETE FROM guestbooks WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?
        .rows_affected();
    if affected == 0 {
        return Err(ModelError::invalid("留言不存在"));
    }
    Ok(())
}

// ============================================================================
// 实名 / 企业认证
// ============================================================================

/// 认证记录（`user_auths` 表）。
#[derive(Debug, Clone, FromRow)]
pub struct MemberAuth {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_id: u32,
    pub kind: i32,
    pub real_name: String,
    pub id_card: String,
    pub id_card_front: String,
    pub id_card_back: String,
    pub id_card_hand: String,
    pub company_name: String,
    pub company_no: String,
    pub company_legal: String,
    pub company_licence: String,
    pub phone: String,
    pub status: i32,
    pub remark: String,
    pub audit_time: i64,
    pub audit_admin_id: u32,
}

/// 认证入参（`UserAuthRequest`）。
#[derive(Debug, Clone, Default)]
pub struct AuthInput {
    pub kind: i32,
    pub real_name: String,
    pub id_card: String,
    pub id_card_front: String,
    pub id_card_back: String,
    pub id_card_hand: String,
    pub company_name: String,
    pub company_no: String,
    pub company_legal: String,
    pub company_licence: String,
    pub phone: String,
}

const AUTH_COLUMNS: &str = "id, created_time, updated_time, user_id, CAST(`type` AS SIGNED) AS kind, \
    real_name, id_card, id_card_front, id_card_back, id_card_hand, company_name, company_no, \
    company_legal, company_licence, phone, CAST(status AS SIGNED) AS status, remark, \
    audit_time, audit_admin_id";

/// 取会员的认证记录（没有则 `None`）。
pub async fn auth_by_user(pool: &MySqlPool, user_id: u32) -> anyhow::Result<Option<MemberAuth>> {
    if user_id == 0 {
        return Ok(None);
    }
    let sql = format!("SELECT {AUTH_COLUMNS} FROM user_auths WHERE user_id = ? LIMIT 1");
    Ok(sqlx::query_as::<_, MemberAuth>(&sql)
        .bind(user_id)
        .fetch_optional(pool)
        .await?)
}

/// 提交（或重新提交）认证申请（`ApplyUserAuth`）。
///
/// 已通过的记录不允许直接覆盖，必须由管理员先撤销。
pub async fn apply_auth(
    pool: &MySqlPool,
    user_id: u32,
    input: &AuthInput,
) -> Result<MemberAuth, ModelError> {
    if user_id == 0 {
        return Err(ModelError::invalid("请登录"));
    }
    let kind = if input.kind == USER_AUTH_TYPE_COMPANY {
        USER_AUTH_TYPE_COMPANY
    } else {
        USER_AUTH_TYPE_PERSON
    };

    let real_name = input.real_name.trim().to_string();
    let id_card = input.id_card.trim().to_uppercase();
    let company_name = input.company_name.trim().to_string();
    let company_no = input.company_no.trim().to_uppercase();

    if kind == USER_AUTH_TYPE_COMPANY {
        if company_name.is_empty()
            || input.company_licence.trim().is_empty()
            || input.company_legal.trim().is_empty()
        {
            return Err(ModelError::invalid("请填写完整的认证资料"));
        }
    } else {
        if real_name.is_empty()
            || id_card.is_empty()
            || input.id_card_front.trim().is_empty()
            || input.id_card_back.trim().is_empty()
        {
            return Err(ModelError::invalid("请填写完整的认证资料"));
        }
        if id_card.chars().count() != 18 {
            return Err(ModelError::invalid("身份证号格式不正确"));
        }
    }

    let existing = auth_by_user(pool, user_id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;
    if let Some(found) = &existing {
        if found.status == USER_AUTH_STATUS_PASSED {
            return Err(ModelError::invalid("认证已通过，如需修改请联系客服"));
        }
    }

    let now = chrono::Utc::now().timestamp();
    if let Some(found) = existing {
        sqlx::query(
            "UPDATE user_auths SET updated_time = ?, `type` = ?, real_name = ?, id_card = ?, \
                id_card_front = ?, id_card_back = ?, id_card_hand = ?, company_name = ?, \
                company_no = ?, company_legal = ?, company_licence = ?, phone = ?, \
                status = ?, remark = '', audit_time = 0, audit_admin_id = 0 WHERE id = ?",
        )
        .bind(now)
        .bind(kind)
        .bind(&real_name)
        .bind(&id_card)
        .bind(input.id_card_front.trim())
        .bind(input.id_card_back.trim())
        .bind(input.id_card_hand.trim())
        .bind(&company_name)
        .bind(&company_no)
        .bind(input.company_legal.trim())
        .bind(input.company_licence.trim())
        .bind(input.phone.trim())
        .bind(USER_AUTH_STATUS_PENDING)
        .bind(found.id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;

        return auth_by_user(pool, user_id)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .ok_or_else(|| ModelError::invalid("认证记录不存在"));
    }

    sqlx::query(
        "INSERT INTO user_auths (created_time, updated_time, user_id, `type`, real_name, \
            id_card, id_card_front, id_card_back, id_card_hand, company_name, company_no, \
            company_legal, company_licence, phone, status, remark, audit_time, audit_admin_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, '', 0, 0)",
    )
    .bind(now)
    .bind(now)
    .bind(user_id)
    .bind(kind)
    .bind(&real_name)
    .bind(&id_card)
    .bind(input.id_card_front.trim())
    .bind(input.id_card_back.trim())
    .bind(input.id_card_hand.trim())
    .bind(&company_name)
    .bind(&company_no)
    .bind(input.company_legal.trim())
    .bind(input.company_licence.trim())
    .bind(input.phone.trim())
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;

    auth_by_user(pool, user_id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("认证记录不存在"))
}

// ============================================================================
// 签到与积分
// ============================================================================

/// 签到概况（`SignInfo`）。
#[derive(Debug, Clone, Copy, Default)]
pub struct SignInfo {
    pub checked_today: bool,
    pub days: i32,
    pub month_count: i64,
    pub points: i64,
}

/// 签到概况。
pub async fn sign_info(pool: &MySqlPool, user_id: u32) -> anyhow::Result<SignInfo> {
    let mut info = SignInfo::default();
    if user_id == 0 {
        return Ok(info);
    }
    let today = today_string();
    let yesterday = yesterday_string();

    if let Some(row) = sqlx::query_as::<_, (i32,)>(
        "SELECT days FROM user_signs WHERE user_id = ? AND sign_date = ? LIMIT 1",
    )
    .bind(user_id)
    .bind(&today)
    .fetch_optional(pool)
    .await?
    {
        info.checked_today = true;
        info.days = row.0;
    } else if let Some(row) = sqlx::query_as::<_, (String, i32)>(
        "SELECT sign_date, days FROM user_signs WHERE user_id = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    {
        if row.0 == yesterday {
            info.days = row.1;
        }
    }

    let month_start = month_start_string();
    info.month_count = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_signs WHERE user_id = ? AND sign_date >= ?",
    )
    .bind(user_id)
    .bind(&month_start)
    .fetch_one(pool)
    .await?;

    info.points = sqlx::query_scalar("SELECT points FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or(0);
    Ok(info)
}

/// 今日签到（`SignToday`）。
///
/// 连续签到额外积分：`(连续天数 - 1)`，上限 [`SIGN_EXTRA_MAX`]。
pub async fn sign_today(pool: &MySqlPool, user_id: u32) -> Result<Value, ModelError> {
    if user_id == 0 {
        return Err(ModelError::invalid("请登录"));
    }
    let today = today_string();
    let yesterday = yesterday_string();

    let mut days = 1i32;
    if let Some(row) = sqlx::query_as::<_, (i32,)>(
        "SELECT days FROM user_signs WHERE user_id = ? AND sign_date = ? LIMIT 1",
    )
    .bind(user_id)
    .bind(&yesterday)
    .fetch_optional(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
    {
        days = row.0 + 1;
    }

    let mut points = SIGN_BASE_POINTS;
    let extra = (days as i64 - 1).min(SIGN_EXTRA_MAX);
    if extra > 0 {
        points += extra;
    }

    // 已签到直接拦下（靠唯一索引冲突，这里先查一次，报错文案一致）
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_signs WHERE user_id = ? AND sign_date = ?",
    )
    .bind(user_id)
    .bind(&today)
    .fetch_one(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;
    if exists > 0 {
        return Err(ModelError::invalid("今日已签到"));
    }

    let now = chrono::Utc::now().timestamp();
    let sign_id = sqlx::query(
        "INSERT INTO user_signs (created_time, user_id, sign_date, days, points) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(now)
    .bind(user_id)
    .bind(&today)
    .bind(days)
    .bind(points)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("签到失败: {err}")))?
    .last_insert_id();

    // 发放积分（失败则撤销签到记录，与既有行为一致）
    match add_points(pool, user_id, points, &format!("签到第 {days} 天奖励")).await {
        Ok(_) => {}
        Err(err) => {
            let _ = sqlx::query("DELETE FROM user_signs WHERE id = ?")
                .bind(sign_id)
                .execute(pool)
                .await;
            return Err(err);
        }
    }

    Ok(json!({
        "id": sign_id,
        "created_time": now,
        "user_id": user_id,
        "sign_date": today,
        "days": days,
        "points": points,
    }))
}

/// 增减积分（`AddPoints`），返回变更后的积分。
pub async fn add_points(
    pool: &MySqlPool,
    user_id: u32,
    points: i64,
    remark: &str,
) -> Result<i64, ModelError> {
    if user_id == 0 {
        return Err(ModelError::invalid("请登录"));
    }
    if points == 0 {
        let current: i64 = sqlx::query_scalar("SELECT points FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .unwrap_or(0);
        return Ok(current);
    }

    let now = chrono::Utc::now().timestamp();
    let affected = if points < 0 {
        sqlx::query("UPDATE users SET points = points + ?, updated_time = ? WHERE id = ? AND points >= ?")
            .bind(points)
            .bind(now)
            .bind(user_id)
            .bind(-points)
            .execute(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("积分变更失败: {err}")))?
            .rows_affected()
    } else {
        sqlx::query("UPDATE users SET points = points + ?, updated_time = ? WHERE id = ?")
            .bind(points)
            .bind(now)
            .bind(user_id)
            .execute(pool)
            .await
            .map_err(|err| ModelError::invalid(format!("积分变更失败: {err}")))?
            .rows_affected()
    };
    if affected == 0 {
        return Err(ModelError::invalid("积分不足"));
    }

    let after: i64 = sqlx::query_scalar("SELECT points FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?;

    sqlx::query(
        "INSERT INTO point_logs (created_time, updated_time, user_id, points, after_points, \
            `type`, remark) VALUES (?, ?, ?, ?, ?, 1, ?)",
    )
    .bind(now)
    .bind(now)
    .bind(user_id)
    .bind(points)
    .bind(after)
    .bind(remark)
    .execute(pool)
    .await
    .map_err(|err| ModelError::invalid(format!("写入流水失败: {err}")))?;

    Ok(after)
}

// ============================================================================
// 内容详情（会员中心「我的一条内容」）
// ============================================================================

/// 内容归属人（`GetArchiveOwner`）。
pub async fn archive_owner(pool: &MySqlPool, id: i64) -> anyhow::Result<Option<u32>> {
    let owner: Option<u32> = sqlx::query_scalar("SELECT user_id FROM archives WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    if owner.is_some() {
        return Ok(owner);
    }
    Ok(sqlx::query_scalar("SELECT user_id FROM archive_drafts WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// 会员自己发布的内容详情（正式内容优先，其次草稿）。
///
/// 返回 `ApiUserArchiveDetail` 那份 JSON。
pub async fn archive_detail(pool: &MySqlPool, id: i64) -> anyhow::Result<Option<Value>> {
    let mut out = json!({ "id": id, "draft": false });

    let archive = sqlx::query(
        "SELECT id, title, keywords, description, category_id, module_id, url_token, \
            created_time, images FROM archives WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    if let Some(row) = archive {
        use sqlx::Row;
        let cat_id: u32 = row.get("category_id");
        let module_id: u32 = row.get("module_id");
        let mode = crate::models::url::load_mode(pool).await.unwrap_or(2);
        let link = archive_link(pool, mode, id).await;
        if let Value::Object(map) = &mut out {
            map.insert("title".into(), json!(row.get::<String, _>("title")));
            map.insert("keywords".into(), json!(row.get::<String, _>("keywords")));
            map.insert("description".into(), json!(row.get::<String, _>("description")));
            map.insert("category_id".into(), json!(cat_id));
            map.insert("module_id".into(), json!(module_id));
            map.insert("url_token".into(), json!(row.get::<String, _>("url_token")));
            map.insert("created_time".into(), json!(row.get::<Option<i64>, _>("created_time")));
            map.insert("link".into(), json!(link));
        }
    } else {
        let draft = sqlx::query(
            "SELECT id, title, keywords, description, category_id, module_id, url_token, \
                created_time FROM archive_drafts WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        let Some(row) = draft else {
            return Ok(None);
        };
        use sqlx::Row;
        if let Value::Object(map) = &mut out {
            map.insert("draft".into(), json!(true));
            map.insert("title".into(), json!(row.get::<String, _>("title")));
            map.insert("keywords".into(), json!(row.get::<String, _>("keywords")));
            map.insert("description".into(), json!(row.get::<String, _>("description")));
            map.insert("category_id".into(), json!(row.get::<u32, _>("category_id")));
            map.insert("module_id".into(), json!(row.get::<u32, _>("module_id")));
            map.insert("url_token".into(), json!(row.get::<String, _>("url_token")));
            map.insert("created_time".into(), json!(row.get::<Option<i64>, _>("created_time")));
        }
    }

    let content: Option<String> = sqlx::query_scalar("SELECT content FROM archive_data WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    if let Some(content) = content {
        if let Value::Object(map) = &mut out {
            map.insert("content".into(), json!(content));
        }
    }

    let tags = archive_tags(pool, id).await;
    if !tags.is_empty() {
        if let Value::Object(map) = &mut out {
            map.insert("tags".into(), json!(tags));
        }
    }

    Ok(Some(out))
}

/// 内容关联的标签名（`GetTagsByItemId`）。
pub async fn archive_tags(pool: &MySqlPool, id: i64) -> Vec<String> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT t.title FROM tag_data d INNER JOIN tags t ON t.id = d.tag_id \
         WHERE d.item_id = ? AND t.title <> '' ORDER BY d.id ASC",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows
}

/// 内容详情页地址。
pub async fn archive_link(pool: &MySqlPool, mode: i32, id: i64) -> String {
    let row = sqlx::query(
        "SELECT a.url_token, a.category_id, a.module_id, c.url_token AS cat_token, \
            m.url_token AS module_token FROM archives a \
         LEFT JOIN categories c ON c.id = a.category_id \
         LEFT JOIN modules m ON m.id = a.module_id WHERE a.id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await;
    let Ok(Some(row)) = row else {
        return String::new();
    };
    use sqlx::Row;
    crate::models::url::archive_url(
        mode,
        row.get::<Option<String>, _>("cat_token").as_deref().unwrap_or(""),
        row.get::<Option<String>, _>("module_token")
            .as_deref()
            .unwrap_or(""),
        id,
        &row.get::<String, _>("url_token"),
        row.get::<u32, _>("category_id"),
        1,
    )
}

// ============================================================================
// 工具
// ============================================================================

/// 邮箱格式校验（`VerifyEmailFormat`）。
pub fn valid_email(email: &str) -> bool {
    let email = email.trim();
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    if local.is_empty() || domain.is_empty() {
        return false;
    }
    if !domain.contains('.') {
        return false;
    }
    if email.chars().any(|ch| ch.is_whitespace()) {
        return false;
    }
    local
        .chars()
        .all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '-' || ch == '.' || ch == '+')
        && domain
            .chars()
            .all(|ch| ch.is_alphanumeric() || ch == '.' || ch == '-')
}

/// 手机号格式校验（`VerifyCellphoneFormat`）。
pub fn valid_phone(phone: &str) -> bool {
    let phone = phone.trim();
    if phone.len() != 11 || !phone.starts_with('1') {
        return false;
    }
    phone.chars().all(|ch| ch.is_ascii_digit())
}

/// URL 别名清洗（`GetPinyin` 的简化版：保留字母数字）。
pub fn sanitize_token(raw: &str) -> String {
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
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        format!("user-{}", chrono::Utc::now().timestamp_millis())
    } else {
        trimmed
    }
}

/// 随机字母数字串（`GenerateRandString`）。
pub fn random_code(len: usize) -> String {
    const POOL: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| POOL[rng.gen_range(0..POOL.len())] as char)
        .collect()
}

/// 密码重置令牌（`Md5(user.Email + user.Password)`）。
///
/// 密码一改令牌就失效，所以天然是一次性的。
pub fn md5_reset_token(email: &str, password_hash: &str) -> String {
    super::attachment::md5_hex(format!("{email}{password_hash}").as_bytes())
}

/// `images` 列里的第一张图（`GetThumb`）。
pub fn first_image(images: Option<&str>) -> String {
    super::json_string_list(images).into_iter().next().unwrap_or_default()
}

/// 默认用户组 ID（缺省 1）。
pub async fn default_group_id(pool: &MySqlPool) -> u32 {
    let value = super::site_setting::get_section(pool, "user").await.unwrap_or_default();
    value
        .get("default_group_id")
        .and_then(Value::as_i64)
        .filter(|id| *id > 0)
        .unwrap_or(1) as u32
}

/// 默认会员状态（缺省 `normal` = 1）。
pub async fn default_status(pool: &MySqlPool) -> i32 {
    let value = super::site_setting::get_section(pool, "user").await.unwrap_or_default();
    match value.get("default_status").and_then(Value::as_str) {
        Some("pending") => 0,
        Some("blocked") => -1,
        _ => 1,
    }
}

/// 今天的日期串（`2006-01-02`）。
pub fn today_string() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// 昨天的日期串。
pub fn yesterday_string() -> String {
    (chrono::Local::now() - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string()
}

/// 本月 1 号的日期串。
pub fn month_start_string() -> String {
    let now = chrono::Local::now();
    format!("{}-{:02}-01", now.format("%Y"), now.format("%m"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn validates_email_and_phone() {
        assert!(valid_email("a@b.com"));
        assert!(valid_email("rustcms+1@example.com"));
        assert!(!valid_email("a@b"));
        assert!(!valid_email("a b@c.com"));
        assert!(!valid_email("@b.com"));
        assert!(!valid_email(""));

        assert!(valid_phone("13800000000"));
        assert!(!valid_phone("1380000000"));
        assert!(!valid_phone("23800000000"));
        assert!(!valid_phone("1380000000a"));
    }

    #[test]
    fn masks_api_token() {
        assert_eq!(mask_api_token(""), "");
        assert_eq!(mask_api_token("abcdef"), "ab****");
        assert_eq!(
            mask_api_token("0123456789abcdef0123456789"),
            "0123****6789"
        );
    }

    #[test]
    fn sanitizes_token_and_random_code() {
        assert_eq!(sanitize_token("John Doe"), "john-doe");
        assert!(sanitize_token("中文").starts_with("user-"));
        assert_eq!(random_code(40).len(), 40);
    }

    #[tokio::test]
    async fn reads_real_members() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let id: Option<u32> = sqlx::query_scalar("SELECT id FROM users ORDER BY id LIMIT 1")
            .fetch_optional(&pool)
            .await
            .unwrap();
        let Some(id) = id else {
            return;
        };

        let row = get(&pool, id).await.unwrap().expect("会员应存在");
        assert_eq!(row.id, id);
        let json = row.to_json();
        assert_eq!(json["id"], json!(id));
        assert!(json.get("password").is_none(), "不能带出密码");
        assert!(json.get("api_token").is_none(), "不能带出 api_token");
        assert_eq!(json["token"], Value::Null);

        assert!(get(&pool, u32::MAX).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn registers_and_cleans_up_member() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM users WHERE user_name LIKE 'rustcms-reg%'")
            .execute(&pool)
            .await;

        // 用户名为空
        let err = register(
            &pool,
            &RegisterInput {
                user_name: String::new(),
                password: "pass-123456".into(),
                email: "rustcms-reg@example.com".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "请正确填写用户名和密码");

        // 密码太短
        let err = register(
            &pool,
            &RegisterInput {
                user_name: "rustcms-reg".into(),
                password: "123".into(),
                email: "rustcms-reg@example.com".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "请填写6位以上的密码");

        // 邮箱与手机号都没填
        let err = register(
            &pool,
            &RegisterInput {
                user_name: "rustcms-reg".into(),
                password: "pass-123456".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "邮箱和手机号至少填写一个");

        // 正常注册
        let id = register(
            &pool,
            &RegisterInput {
                user_name: "rustcms-reg".into(),
                password: "pass-123456".into(),
                email: "rustcms-reg@example.com".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(id > 0);

        // 重名
        let err = register(
            &pool,
            &RegisterInput {
                user_name: "rustcms-reg".into(),
                password: "pass-123456".into(),
                email: "rustcms-reg2@example.com".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "该用户名已被注册");

        // 密码能校验通过
        let row = get(&pool, id).await.unwrap().unwrap();
        assert!(crate::auth::check_password("pass-123456", &row.password));

        // 开放接口 Key
        let token = reset_api_token(&pool, id).await.unwrap();
        assert_eq!(token.len(), MEMBER_API_TOKEN_LENGTH);
        assert_eq!(api_token(&pool, id).await.unwrap(), token);
        clear_api_token(&pool, id).await.unwrap();
        assert_eq!(api_token(&pool, id).await.unwrap(), "");

        let _ = sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    async fn reads_groups_and_member_lists() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };

        let groups = groups(&pool).await.unwrap();
        for item in &groups {
            assert!(item.to_json(item.price).get("favorable_price").is_some());
        }
        if let Some(first) = groups.first() {
            let got = group(&pool, first.id).await.unwrap().unwrap();
            assert_eq!(got.title, first.title);
        }

        let (total, list) = published(&pool, 1, 1, 10).await.unwrap();
        assert!(total >= 0);
        assert!(list.len() as i64 <= 10);

        let (total, list) = finances(&pool, 1, 1, 10).await.unwrap();
        assert!(total >= 0);
        assert!(list.len() as i64 <= 10);

        let (total, list) = point_logs(&pool, 1, 1, 10).await.unwrap();
        assert!(total >= 0);
        assert!(list.len() as i64 <= 10);

        let (pending, all) = comment_stat(&pool, 1).await.unwrap();
        assert!(all >= pending);

        let (total, list) = comments(&pool, 1, -1, 1, 10).await.unwrap();
        assert!(total >= 0);
        assert!(list.len() as i64 <= 10);
    }

    #[tokio::test]
    async fn sign_info_and_today() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM users WHERE user_name LIKE 'rustcms-sign%'")
            .execute(&pool)
            .await;

        let id = register(
            &pool,
            &RegisterInput {
                user_name: "rustcms-sign".into(),
                password: "pass-123456".into(),
                email: "rustcms-sign@example.com".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let info = sign_info(&pool, id).await.unwrap();
        assert!(!info.checked_today);
        assert_eq!(info.points, 0);

        let sign = sign_today(&pool, id).await.unwrap();
        assert_eq!(sign["days"], json!(1));
        assert_eq!(sign["points"], json!(SIGN_BASE_POINTS));

        let info = sign_info(&pool, id).await.unwrap();
        assert!(info.checked_today);
        assert_eq!(info.points, SIGN_BASE_POINTS);

        // 重复签到被拦
        let err = sign_today(&pool, id).await.unwrap_err();
        assert_eq!(err.to_string(), "今日已签到");

        let _ = sqlx::query("DELETE FROM point_logs WHERE user_id = ?")
            .bind(id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM user_signs WHERE user_id = ?")
            .bind(id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await;
    }
}

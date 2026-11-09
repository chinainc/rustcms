//! 微信公众号 / 小程序（既有约定、既有约定）。
//!
//! 配置存 `settings.wechat` / `settings.weapp`（两者结构相同，公众号多两个验证相关字段）。
//! 消息、自动回复、菜单三张表纯 DB 读写；菜单同步与客服消息推送走微信服务器 API。
//!
//! `access_token` 由 [`access_token`] 统一管理：进程内缓存 + 提前 5 分钟过期，
//! 并发的多个请求只会真正去微信取一次。

pub mod crypto;
pub mod wxa;

use std::collections::HashMap;
use std::sync::Mutex;

use rustcms_ext::http::{self, Request};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::{site_setting, ModelError};

/// 公众号配置键。
pub const WECHAT: &str = "wechat";
/// 小程序配置键。
pub const WEAPP: &str = "weapp";

/// 公众号消息。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct WechatMessage {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub openid: String,
    pub content: String,
    pub reply: String,
    pub reply_time: i64,
}

/// 自动回复规则。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct WechatReplyRule {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub keyword: String,
    pub content: Option<String>,
    pub is_default: bool,
}

/// 公众号菜单。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct WechatMenu {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub parent_id: i32,
    pub name: String,
    /// 数据库列名是 `type`（Rust 关键字，故字段改名 + 两侧都声明映射）
    #[serde(rename = "type")]
    #[sqlx(rename = "type")]
    pub kind: String,
    pub value: String,
    pub sort: i32,
}

/// 菜单入参（`type` 是关键字，改名 `kind`）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WechatMenuInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub parent_id: i32,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub sort: Option<i32>,
}

/// 自动回复规则入参。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WechatRuleInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub keyword: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub is_default: Option<bool>,
}

fn config_defaults(value: &mut Value) {
    if let Value::Object(map) = value {
        for key in ["app_id", "app_secret", "token", "encoding_aes_key"] {
            map.entry(key).or_insert(json!(""));
        }
        // 公众号专有
        map.entry("verify_key").or_insert(json!(""));
        map.entry("verify_msg").or_insert(json!(""));
        map.entry("server_url").or_insert(json!(""));
    }
}

/// 读公众号配置（补 `server_url = base_url + /api/wechat`）。
pub async fn wechat_get(pool: &MySqlPool, base_url: &str) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, WECHAT).await?;
    config_defaults(&mut value);
    if let Value::Object(map) = &mut value {
        map.insert(
            "server_url".into(),
            json!(format!("{}/api/wechat", base_url.trim_end_matches('/'))),
        );
    }
    Ok(value)
}

/// 保存公众号配置（`server_url` 是派生字段，不落库）。
pub async fn wechat_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    if !patch.is_object() {
        return Err(anyhow::anyhow!("配置格式错误"));
    }
    let mut patch = patch.clone();
    if let Value::Object(map) = &mut patch {
        map.remove("server_url");
    }
    site_setting::save_section(pool, WECHAT, &patch).await?;
    wechat_get(pool, "").await
}

/// 读小程序配置。
pub async fn weapp_get(pool: &MySqlPool, base_url: &str) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, WEAPP).await?;
    config_defaults(&mut value);
    if let Value::Object(map) = &mut value {
        map.insert(
            "server_url".into(),
            json!(format!("{}/api/wechat", base_url.trim_end_matches('/'))),
        );
    }
    Ok(value)
}

/// 保存小程序配置。
pub async fn weapp_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    if !patch.is_object() {
        return Err(anyhow::anyhow!("配置格式错误"));
    }
    let mut patch = patch.clone();
    if let Value::Object(map) = &mut patch {
        map.remove("server_url");
    }
    site_setting::save_section(pool, WEAPP, &patch).await?;
    weapp_get(pool, "").await
}

// ---------------------------- 消息 -----------------------------------------

/// 分页查消息，返回 `(总数, 本页记录)`（id 倒序）。
pub async fn message_list(
    pool: &MySqlPool,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<WechatMessage>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wechat_messages")
        .fetch_one(pool)
        .await?;
    let rows =
        sqlx::query_as::<_, WechatMessage>("SELECT * FROM wechat_messages ORDER BY id DESC LIMIT ? OFFSET ?")
            .bind(page_size)
            .bind(offset)
            .fetch_all(pool)
            .await?;
    Ok((total, rows))
}

/// 按 ID 取消息。
pub async fn message_get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<WechatMessage>> {
    Ok(
        sqlx::query_as::<_, WechatMessage>("SELECT * FROM wechat_messages WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 删除消息。
pub async fn message_delete(pool: &MySqlPool, id: u32) -> anyhow::Result<bool> {
    Ok(sqlx::query("DELETE FROM wechat_messages WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// 记录回复内容，并在公众号已配置时通过客服接口推送给用户。
///
/// 公众号未配置（缺 AppID / AppSecret）时只落库；已配置但推送失败则整体失败，
/// 与既有接口契约一致。
pub async fn message_reply(
    pool: &MySqlPool,
    id: u32,
    reply: &str,
) -> Result<WechatMessage, ModelError> {
    let reply = reply.trim();
    if reply.is_empty() {
        return Err(ModelError::invalid("请填写回复内容"));
    }
    let message = message_get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("消息不存在"))?;

    // 推送失败就整体失败，不要出现「后台显示回复成功、用户其实没收到」
    match push_text(pool, &message.openid, reply).await? {
        true => {}
        false => {
            return Err(ModelError::invalid(
                "请先在公众号配置中填写 AppID 和 AppSecret，无法推送给微信",
            ))
        }
    }

    let now = chrono::Utc::now().timestamp();
    sqlx::query("UPDATE wechat_messages SET reply = ?, reply_time = ?, updated_time = ? WHERE id = ?")
        .bind(reply)
        .bind(now)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;

    message_get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("消息不存在"))
}

// ---------------------------- 自动回复 -------------------------------------

/// 分页查自动回复规则，返回 `(总数, 本页记录)`。
pub async fn rule_list(
    pool: &MySqlPool,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<WechatReplyRule>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wechat_reply_rules")
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as::<_, WechatReplyRule>(
        "SELECT * FROM wechat_reply_rules ORDER BY is_default DESC, id DESC LIMIT ? OFFSET ?",
    )
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok((total, rows))
}

/// 按 ID 取规则。
pub async fn rule_get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<WechatReplyRule>> {
    Ok(
        sqlx::query_as::<_, WechatReplyRule>("SELECT * FROM wechat_reply_rules WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 新建 / 更新规则（`is_default` 全局只允许一条）。
pub async fn rule_save(
    pool: &MySqlPool,
    input: &WechatRuleInput,
) -> Result<WechatReplyRule, ModelError> {
    let keyword = input.keyword.trim();
    if keyword.is_empty() {
        return Err(ModelError::invalid("请填写关键词"));
    }
    // `wechat_reply_rules.keyword` 是 varchar(128)
    if keyword.chars().count() > 128 {
        return Err(ModelError::invalid("关键词不能超过 128 个字符"));
    }
    let now = chrono::Utc::now().timestamp();
    let is_default = input.is_default.unwrap_or(false);

    let id = if input.id > 0 {
        if rule_get(pool, input.id)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .is_none()
        {
            return Err(ModelError::invalid("规则不存在"));
        }
        sqlx::query(
            "UPDATE wechat_reply_rules SET updated_time = ?, keyword = ?, content = ?, \
                is_default = ? WHERE id = ?",
        )
        .bind(now)
        .bind(keyword)
        .bind(&input.content)
        .bind(is_default)
        .bind(input.id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
        input.id
    } else {
        sqlx::query(
            "INSERT INTO wechat_reply_rules (created_time, updated_time, keyword, content, \
                is_default) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(now)
        .bind(now)
        .bind(keyword)
        .bind(&input.content)
        .bind(is_default)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?
        .last_insert_id() as u32
    };

    // 默认规则唯一
    if is_default {
        let _ = sqlx::query("UPDATE wechat_reply_rules SET is_default = 0 WHERE id <> ?")
            .bind(id)
            .execute(pool)
            .await;
    }

    rule_get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("规则不存在"))
}

/// 删除规则。
pub async fn rule_delete(pool: &MySqlPool, id: u32) -> anyhow::Result<bool> {
    Ok(sqlx::query("DELETE FROM wechat_reply_rules WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

// ---------------------------- 菜单 -----------------------------------------

/// 全部菜单（按 `sort` 升序）。
pub async fn menu_list(pool: &MySqlPool) -> anyhow::Result<Vec<WechatMenu>> {
    Ok(sqlx::query_as::<_, WechatMenu>(
        "SELECT * FROM wechat_menus ORDER BY sort ASC, id ASC",
    )
    .fetch_all(pool)
    .await?)
}

/// 按 ID 取菜单。
pub async fn menu_get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<WechatMenu>> {
    Ok(
        sqlx::query_as::<_, WechatMenu>("SELECT * FROM wechat_menus WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 新建 / 更新菜单。
pub async fn menu_save(
    pool: &MySqlPool,
    input: &WechatMenuInput,
) -> Result<WechatMenu, ModelError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(ModelError::invalid("请填写菜单名称"));
    }
    // `wechat_menus.name` 是 varchar(20)，提前拦住，别让用户看到 MySQL 1406
    if name.chars().count() > 20 {
        return Err(ModelError::invalid("菜单名称不能超过 20 个字符"));
    }
    if input.value.trim().chars().count() > 255 {
        return Err(ModelError::invalid("菜单值不能超过 255 个字符"));
    }
    if input.parent_id > 0 {
        if menu_get(pool, input.parent_id as u32)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .is_none()
        {
            return Err(ModelError::invalid("上级菜单不存在"));
        }
        if input.parent_id as u32 == input.id {
            return Err(ModelError::invalid("上级菜单不能是自己"));
        }
    }

    let now = chrono::Utc::now().timestamp();
    let sort = input.sort.unwrap_or(10);

    let id = if input.id > 0 {
        if menu_get(pool, input.id)
            .await
            .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
            .is_none()
        {
            return Err(ModelError::invalid("菜单不存在"));
        }
        sqlx::query(
            "UPDATE wechat_menus SET updated_time = ?, parent_id = ?, name = ?, `type` = ?, \
                value = ?, sort = ? WHERE id = ?",
        )
        .bind(now)
        .bind(input.parent_id)
        .bind(name)
        .bind(input.kind.trim())
        .bind(input.value.trim())
        .bind(sort)
        .bind(input.id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
        input.id
    } else {
        sqlx::query(
            "INSERT INTO wechat_menus (created_time, updated_time, parent_id, name, `type`, \
                value, sort) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(now)
        .bind(now)
        .bind(input.parent_id)
        .bind(name)
        .bind(input.kind.trim())
        .bind(input.value.trim())
        .bind(sort)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?
        .last_insert_id() as u32
    };

    menu_get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .ok_or_else(|| ModelError::invalid("菜单不存在"))
}

/// 删除菜单（连带子菜单）。
pub async fn menu_delete(pool: &MySqlPool, id: u32) -> Result<u64, ModelError> {
    if menu_get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .is_none()
    {
        return Ok(0);
    }
    let mut deleted = sqlx::query("DELETE FROM wechat_menus WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?
        .rows_affected();
    deleted += sqlx::query("DELETE FROM wechat_menus WHERE parent_id = ?")
        .bind(id as i32)
        .execute(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?
        .rows_affected();
    Ok(deleted)
}

// ---------------------------- 微信服务器 API --------------------------------

/// 微信公众平台接口基址。
const API_BASE: &str = "https://api.weixin.qq.com";

/// 查询串百分号编码（`appid` / `secret` / `access_token` 都是查询参数）。
fn url_encode(raw: &str) -> String {
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

/// 发一次请求并解析 JSON，HTTP 状态非 2xx 视为失败。
async fn send_json(request: Request) -> Result<Value, ModelError> {
    let response = http::send(&request)
        .await
        .map_err(|err| ModelError::invalid(format!("调用微信接口失败: {err}")))?;
    if !response.is_success() {
        return Err(ModelError::invalid(format!(
            "微信接口返回 HTTP {}",
            response.status
        )));
    }
    response
        .json()
        .map_err(|err| ModelError::invalid(format!("微信接口响应解析失败: {err}")))
}

/// 微信响应里的 `errcode` 非 0 时转成业务错误。
fn wechat_error(value: &Value) -> Option<String> {
    let code = value.get("errcode").and_then(Value::as_i64).unwrap_or(0);
    if code == 0 {
        return None;
    }
    let message = value
        .get("errmsg")
        .and_then(Value::as_str)
        .unwrap_or("未知错误");
    Some(format!("微信接口错误({code})：{message}"))
}

/// 配置分区：公众号 / 小程序（两者各有一套 AppID / AppSecret / Token）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Wechat,
    Weapp,
}

impl Section {
    /// `settings` 表里的键。
    pub fn setting_key(&self) -> &'static str {
        match self {
            Section::Wechat => WECHAT,
            Section::Weapp => WEAPP,
        }
    }

    /// 中文名（错误文案里用）。
    pub fn label(&self) -> &'static str {
        match self {
            Section::Wechat => "公众号",
            Section::Weapp => "小程序",
        }
    }

    /// 缓存键前缀（同一进程可能同时配公众号与小程序）。
    fn cache_prefix(&self) -> &'static str {
        match self {
            Section::Wechat => "wechat",
            Section::Weapp => "weapp",
        }
    }
}

/// 读指定分区的 AppID / AppSecret（缺一即视为未配置）。
async fn credentials(pool: &MySqlPool, section: Section) -> Result<(String, String), ModelError> {
    let config = site_setting::get_section(pool, section.setting_key())
        .await
        .map_err(|err| {
            ModelError::invalid(format!("读取{}配置失败: {err}", section.label()))
        })?;
    let app_id = config
        .get("app_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let app_secret = config
        .get("app_secret")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if app_id.is_empty() || app_secret.is_empty() {
        return Err(ModelError::invalid(format!(
            "请先在{}配置中填写 AppID 和 AppSecret",
            section.label()
        )));
    }
    Ok((app_id, app_secret))
}

// ---------------------------- access_token 缓存 -----------------------------

/// 进程内缓存的一条凭据。
#[derive(Debug, Clone)]
struct TokenEntry {
    token: String,
    /// 到期时间（秒级时间戳），已经扣掉了提前量。
    expires_at: i64,
}

/// 凭据缓存的提前量：`expires_in - 300` 过期，避免临界点撞上「token 已失效」。
const TOKEN_EXPIRE_AHEAD: i64 = 300;

/// 进程内缓存。
fn token_cache() -> &'static Mutex<HashMap<String, TokenEntry>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, TokenEntry>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 取凭据时的单飞锁：并发请求里只有一个真正去微信换 token。
fn token_fetch_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn token_cache_key(section: Section, app_id: &str) -> String {
    format!("{}:{app_id}", section.cache_prefix())
}

/// 清掉某个分区的凭据缓存（微信回报 token 失效时调用）。
pub fn invalidate_access_token(section: Section) {
    let prefix = format!("{}:", section.cache_prefix());
    let mut guard = token_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    guard.retain(|key, _| !key.starts_with(&prefix));
}

/// 微信「token 无效 / 过期」错误码。
fn is_token_expired_code(code: i64) -> bool {
    matches!(code, 40001 | 40014 | 42001)
}

/// 响应里是否是 token 失效（只看 JSON；二进制响应不可能是）。
fn token_expired_in(value: &Value) -> bool {
    value
        .get("errcode")
        .and_then(Value::as_i64)
        .map(is_token_expired_code)
        .unwrap_or(false)
}

/// 直接向微信换一次凭据，返回 `(token, expires_in)`。
pub async fn fetch_access_token(
    app_id: &str,
    app_secret: &str,
) -> Result<(String, i64), ModelError> {
    let url = format!(
        "{API_BASE}/cgi-bin/token?grant_type=client_credential&appid={}&secret={}",
        url_encode(app_id),
        url_encode(app_secret)
    );
    let value = send_json(Request::get(url)).await?;
    if let Some(message) = wechat_error(&value) {
        return Err(ModelError::invalid(message));
    }
    let token = value
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ModelError::invalid("微信接口未返回 access_token"))?;
    let expires_in = value
        .get("expires_in")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(7200);
    Ok((token, expires_in))
}

/// 取指定分区的 `access_token`（带缓存；并发只取一次）。
async fn cached_access_token(pool: &MySqlPool, section: Section) -> Result<String, ModelError> {
    let (app_id, app_secret) = credentials(pool, section).await?;
    let key = token_cache_key(section, &app_id);
    let now = chrono::Utc::now().timestamp();

    if let Some(entry) = token_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&key)
        .filter(|entry| entry.expires_at > now)
        .cloned()
    {
        return Ok(entry.token);
    }

    // 缓存未命中：同一时刻只让一个请求去换
    let _flight = token_fetch_lock().lock().await;

    // 拿到锁后再看一眼，可能已经被别的请求填好了
    let now = chrono::Utc::now().timestamp();
    if let Some(entry) = token_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&key)
        .filter(|entry| entry.expires_at > now)
        .cloned()
    {
        return Ok(entry.token);
    }

    let (token, expires_in) = fetch_access_token(&app_id, &app_secret).await?;
    let expires_at = now + (expires_in - TOKEN_EXPIRE_AHEAD).max(60);
    token_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(
            key,
            TokenEntry {
                token: token.clone(),
                expires_at,
            },
        );
    Ok(token)
}

/// 公众号 `access_token`。
pub async fn access_token(pool: &MySqlPool) -> Result<String, ModelError> {
    cached_access_token(pool, Section::Wechat).await
}

/// 小程序 `access_token`。
pub async fn weapp_access_token(pool: &MySqlPool) -> Result<String, ModelError> {
    cached_access_token(pool, Section::Weapp).await
}

/// 带 token 调一次微信接口；token 失效（40001 / 40014 / 42001）时清缓存重试一次。
///
/// `build` 会拿到 token（已做查询串编码），返回完整请求。
async fn send_with_token(
    pool: &MySqlPool,
    section: Section,
    build: impl Fn(&str) -> Request,
) -> Result<http::Response, ModelError> {
    let mut retried = false;
    loop {
        let token = cached_access_token(pool, section).await?;
        let response = http::send(&build(&url_encode(&token)))
            .await
            .map_err(|err| ModelError::invalid(format!("调用微信接口失败: {err}")))?;
        if !response.is_success() {
            return Err(ModelError::invalid(format!(
                "微信接口返回 HTTP {}",
                response.status
            )));
        }
        if !retried {
            // 二进制响应（小程序码）不是 JSON，`json` 失败就按成功处理
            if let Ok(value) = response.json() {
                if token_expired_in(&value) {
                    invalidate_access_token(section);
                    retried = true;
                    continue;
                }
            }
        }
        return Ok(response);
    }
}

/// 把本地菜单组装成 `menu/create` 的请求体。
///
/// 与既有接口契约一致：`view` 类型用 `url`，其余类型用 `key`。
/// 微信限制一级菜单最多 3 个、每个一级菜单最多 5 个二级菜单，超出时提前报错。
pub fn menu_payload(menus: &[WechatMenu]) -> Result<Value, ModelError> {
    if menus.is_empty() {
        return Err(ModelError::invalid("请先添加公众号菜单"));
    }
    let tops: Vec<&WechatMenu> = menus.iter().filter(|menu| menu.parent_id <= 0).collect();
    if tops.len() > 3 {
        return Err(ModelError::invalid("一级菜单最多 3 个"));
    }

    let mut buttons = Vec::with_capacity(tops.len());
    for top in tops {
        let mut button = json!({ "name": top.name, "type": top.kind });
        if top.kind == "view" {
            button["url"] = json!(top.value);
        } else {
            button["key"] = json!(top.value);
        }

        let children: Vec<&WechatMenu> = menus
            .iter()
            .filter(|menu| menu.parent_id > 0 && menu.parent_id as u32 == top.id)
            .collect();
        if children.len() > 5 {
            return Err(ModelError::invalid(format!(
                "一级菜单「{}」的二级菜单最多 5 个",
                top.name
            )));
        }
        if !children.is_empty() {
            let mut sub_buttons = Vec::with_capacity(children.len());
            for child in children {
                let mut item = json!({ "name": child.name, "type": child.kind });
                if child.kind == "view" {
                    item["url"] = json!(child.value);
                } else {
                    item["key"] = json!(child.value);
                }
                sub_buttons.push(item);
            }
            button["sub_button"] = Value::Array(sub_buttons);
        }
        buttons.push(button);
    }

    Ok(json!({ "button": buttons }))
}

/// 把本地菜单同步到微信服务器（`menu/create`），返回同步的菜单总数。
pub async fn menu_sync(pool: &MySqlPool) -> Result<usize, ModelError> {
    // 未配置 AppID / AppSecret 时提前报错，别先取菜单白跑一趟
    credentials(pool, Section::Wechat).await?;
    let menus = menu_list(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("读取菜单失败: {err}")))?;
    let payload = menu_payload(&menus)?;

    let response = send_with_token(pool, Section::Wechat, |token| {
        Request::post(format!(
            "{API_BASE}/cgi-bin/menu/create?access_token={token}"
        ))
        .with_json(payload.clone())
    })
    .await?;
    let value = response
        .json()
        .map_err(|err| ModelError::invalid(format!("微信接口响应解析失败: {err}")))?;
    if let Some(message) = wechat_error(&value) {
        return Err(ModelError::invalid(message));
    }
    Ok(menus.len())
}

/// 用客服接口把文本回复推给用户。
///
/// 公众号未配置 AppID / AppSecret 时返回 `Ok(false)`（仅落库，不推送），
/// 已配置但推送失败则返回错误。
pub async fn push_text(pool: &MySqlPool, openid: &str, content: &str) -> Result<bool, ModelError> {
    if credentials(pool, Section::Wechat).await.is_err() {
        // 未配置公众号：只落库，不推送（与既有约定失败不阻断保存一致）
        return Ok(false);
    }
    let openid = openid.trim();
    if openid.is_empty() {
        return Err(ModelError::invalid("消息缺少 openid，无法推送到微信"));
    }
    if content.trim().is_empty() {
        return Err(ModelError::invalid("请填写回复内容"));
    }

    let payload = json!({
        "touser": openid,
        "msgtype": "text",
        "text": { "content": content },
    });
    let response = send_with_token(pool, Section::Wechat, |token| {
        Request::post(format!(
            "{API_BASE}/cgi-bin/message/custom/send?access_token={token}"
        ))
        .with_json(payload.clone())
    })
    .await?;
    let value = response
        .json()
        .map_err(|err| ModelError::invalid(format!("微信接口响应解析失败: {err}")))?;
    if let Some(message) = wechat_error(&value) {
        return Err(ModelError::invalid(message));
    }
    Ok(true)
}

/// 客服消息推送是否可用（配置齐全）。
pub async fn push_ready(pool: &MySqlPool) -> bool {
    credentials(pool, Section::Wechat).await.is_ok()
}

/// CDATA 内容里不能出现 `]]>`，出现就断开转义。
fn cdata(raw: &str) -> String {
    raw.replace("]]>", "]]]]><![CDATA[>")
}

/// 被动回复的文本消息 XML。
///
/// 公众号收到消息后必须在 5 秒内回包，被动回复是最稳的方式；
/// 客服消息接口（[`push_text`]）用于后台手动回复与验证码下发。
pub fn text_reply_xml(to_user: &str, from_user: &str, content: &str, timestamp: i64) -> String {
    format!(
        "<xml><ToUserName><![CDATA[{to}]]></ToUserName>\
<FromUserName><![CDATA[{from}]]></FromUserName>\
<CreateTime>{timestamp}</CreateTime>\
<MsgType><![CDATA[text]]></MsgType>\
<Content><![CDATA[{content}]]></Content></xml>",
        to = cdata(to_user),
        from = cdata(from_user),
        content = cdata(content),
    )
}

/// 网页授权：用 `code` 换 `access_token` 与 `openid`。
pub async fn oauth_access_token(
    app_id: &str,
    app_secret: &str,
    code: &str,
) -> Result<(String, String), ModelError> {
    let url = format!(
        "{API_BASE}/sns/oauth2/access_token?appid={}&secret={}&code={}&grant_type=authorization_code",
        url_encode(app_id),
        url_encode(app_secret),
        url_encode(code)
    );
    let value = send_json(Request::get(url)).await?;
    if let Some(message) = wechat_error(&value) {
        return Err(ModelError::invalid(message));
    }
    let token = value
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| ModelError::invalid("微信授权未返回 access_token"))?;
    let openid = value
        .get("openid")
        .and_then(Value::as_str)
        .ok_or_else(|| ModelError::invalid("微信授权未返回 openid"))?;
    Ok((token.to_string(), openid.to_string()))
}

/// 网页授权：读取微信用户资料。
pub async fn oauth_user_info(access_token: &str, openid: &str) -> Result<Value, ModelError> {
    let url = format!(
        "{API_BASE}/sns/userinfo?access_token={}&openid={}&lang=zh_CN",
        url_encode(access_token),
        url_encode(openid)
    );
    let value = send_json(Request::get(url)).await?;
    if let Some(message) = wechat_error(&value) {
        return Err(ModelError::invalid(message));
    }
    Ok(value)
}

/// 生成小程序码（`/wxa/getwxacode`），返回 PNG 字节。
///
/// 与既有接口契约对齐：请求体只有 `path`，
/// 成功时微信直接回图片二进制，失败时回 JSON（`errcode` / `errmsg`）。
pub async fn weapp_qrcode_image(pool: &MySqlPool, path: &str) -> Result<Vec<u8>, ModelError> {
    credentials(pool, Section::Weapp).await?;
    let path = path.trim();
    if path.is_empty() {
        return Err(ModelError::invalid("生成小程序码需要 path 参数"));
    }

    let response = send_with_token(pool, Section::Weapp, |token| {
        Request::post(format!("{API_BASE}/wxa/getwxacode?access_token={token}"))
            .with_json(json!({ "path": path }))
    })
    .await?;

    let content_type = response
        .header("content-type")
        .unwrap_or("")
        .to_ascii_lowercase();
    if content_type.starts_with("image") {
        if response.body.is_empty() {
            return Err(ModelError::invalid("微信接口返回了空的二维码图片"));
        }
        return Ok(response.body);
    }
    // 失败时微信返回 JSON（errcode / errmsg）
    let value: Value = response
        .json()
        .map_err(|err| ModelError::invalid(format!("微信接口响应解析失败: {err}")))?;
    Err(ModelError::invalid(
        wechat_error(&value).unwrap_or_else(|| "微信接口未返回二维码图片".to_string()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, setting, test_pool};

    fn sample_menu(id: u32, parent_id: i32, name: &str, kind: &str, value: &str) -> WechatMenu {
        WechatMenu {
            id,
            created_time: None,
            updated_time: None,
            parent_id,
            name: name.to_string(),
            kind: kind.to_string(),
            value: value.to_string(),
            sort: 10,
        }
    }

    #[test]
    fn builds_wechat_menu_payload() {
        let menus = vec![
            sample_menu(1, 0, "首页", "view", "https://example.com"),
            sample_menu(2, 0, "关于", "click", "ABOUT"),
            sample_menu(3, 1, "子菜单", "view", "https://example.com/a"),
        ];
        let payload = menu_payload(&menus).unwrap();
        assert_eq!(payload["button"].as_array().unwrap().len(), 2);
        assert_eq!(payload["button"][0]["type"], "view");
        assert_eq!(payload["button"][0]["url"], "https://example.com");
        assert_eq!(
            payload["button"][0]["sub_button"][0]["name"], "子菜单",
            "{payload}"
        );
        assert_eq!(payload["button"][1]["type"], "click");
        assert_eq!(payload["button"][1]["key"], "ABOUT");
    }

    #[test]
    fn rejects_too_many_top_menus() {
        let menus: Vec<WechatMenu> = (1..=4)
            .map(|id| sample_menu(id, 0, &format!("菜单{id}"), "click", "K"))
            .collect();
        assert_eq!(
            menu_payload(&menus).unwrap_err().to_string(),
            "一级菜单最多 3 个"
        );
        assert_eq!(
            menu_payload(&[]).unwrap_err().to_string(),
            "请先添加公众号菜单"
        );
    }

    #[test]
    fn rejects_too_many_children() {
        let mut menus = vec![sample_menu(1, 0, "父级", "click", "K")];
        for id in 2..=7 {
            menus.push(sample_menu(id, 1, &format!("子{id}"), "click", "K"));
        }
        assert_eq!(
            menu_payload(&menus).unwrap_err().to_string(),
            "一级菜单「父级」的二级菜单最多 5 个"
        );
    }

    #[tokio::test]
    async fn config_roundtrip_with_server_url() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let mut backups = Vec::new();
        for key in [WECHAT, WEAPP] {
            backups.push((key, setting::get_raw(&pool, key).await.unwrap()));
        }

        let loaded = wechat_get(&pool, "https://example.com/").await.unwrap();
        assert_eq!(loaded["server_url"], "https://example.com/api/wechat");
        // 真实库可能已有配置，只断言默认字段被补齐即可
        assert!(loaded.get("app_id").is_some());
        assert!(loaded.get("verify_key").is_some());

        let saved = wechat_save(
            &pool,
            &json!({"app_id": "wx123", "server_url": "https://fake/hack"}),
        )
        .await
        .unwrap();
        assert_eq!(saved["app_id"], "wx123");
        // server_url 是派生字段，不该被写进配置
        assert_eq!(saved["server_url"], "/api/wechat");

        let stored = setting::get_raw(&pool, WECHAT).await.unwrap().unwrap();
        assert!(!stored.contains("fake/hack"), "{stored}");

        let saved = weapp_save(&pool, &json!({"app_secret": "sec"})).await.unwrap();
        assert_eq!(saved["app_secret"], "sec");
        assert!(saved.get("app_id").is_some());

        for (key, raw) in backups {
            match raw {
                Some(raw) => setting::save_raw(&pool, key, &raw).await.unwrap(),
                None => setting::remove(&pool, key).await.unwrap(),
            }
        }
    }

    #[tokio::test]
    async fn message_rule_menu_crud() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM wechat_messages WHERE openid = 'rustcms-openid'")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM wechat_reply_rules WHERE keyword LIKE 'rustcms-wx%'")
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM wechat_menus WHERE name LIKE 'rustcms-wx%'")
            .execute(&pool)
            .await;

        // 消息
        let now = chrono::Utc::now().timestamp();
        let msg_id = sqlx::query(
            "INSERT INTO wechat_messages (created_time, updated_time, openid, content, reply, \
                reply_time) VALUES (?, ?, 'rustcms-openid', '你好', '', 0)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_id() as u32;

        let (total, rows) = message_list(&pool, 1, 20).await.unwrap();
        assert!(total >= 1);
        assert!(rows.iter().any(|item| item.id == msg_id));

        // 回复：空回复被拒
        assert_eq!(
            message_reply(&pool, msg_id, "  ").await.unwrap_err().to_string(),
            "请填写回复内容"
        );
        // 未配置公众号时不能「假装推送成功」。这里先把公众号配置临时清空，
        // 免得用例真的去连微信（配置可能已被站长填过）。
        // 注意：还原要在断言之前做，断言失败也不能把线上配置留在空值上。
        let backup = setting::get_raw(&pool, WECHAT).await.unwrap();
        wechat_save(&pool, &json!({"app_id": "", "app_secret": ""}))
            .await
            .unwrap();
        let reply_error = message_reply(&pool, msg_id, "收到")
            .await
            .unwrap_err()
            .to_string();
        // 推送失败时不能改动消息记录
        let untouched = message_get(&pool, msg_id).await.unwrap().unwrap().reply;
        match backup {
            Some(raw) => setting::save_raw(&pool, WECHAT, &raw).await.unwrap(),
            None => setting::remove(&pool, WECHAT).await.unwrap(),
        }
        assert_eq!(
            reply_error,
            "请先在公众号配置中填写 AppID 和 AppSecret，无法推送给微信"
        );
        assert_eq!(untouched, "");

        assert!(message_delete(&pool, msg_id).await.unwrap());
        assert!(message_get(&pool, msg_id).await.unwrap().is_none());

        // 规则
        assert_eq!(
            rule_save(&pool, &WechatRuleInput::default())
                .await
                .unwrap_err()
                .to_string(),
            "请填写关键词"
        );
        let rule = rule_save(
            &pool,
            &WechatRuleInput {
                keyword: "rustcms-wx-key".into(),
                content: "回复内容".into(),
                is_default: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(rule.is_default);

        // 再建一条默认规则 → 前一条自动取消
        let second = rule_save(
            &pool,
            &WechatRuleInput {
                keyword: "rustcms-wx-key2".into(),
                content: "x".into(),
                is_default: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let first = rule_get(&pool, rule.id).await.unwrap().unwrap();
        assert!(!first.is_default, "默认规则应唯一");
        assert!(rule_get(&pool, second.id).await.unwrap().unwrap().is_default);

        let (total, _) = rule_list(&pool, 1, 20).await.unwrap();
        assert!(total >= 2);

        assert!(rule_delete(&pool, rule.id).await.unwrap());
        assert!(!rule_delete(&pool, rule.id).await.unwrap());

        // 菜单
        assert_eq!(
            menu_save(&pool, &WechatMenuInput::default())
                .await
                .unwrap_err()
                .to_string(),
            "请填写菜单名称"
        );
        let parent = menu_save(
            &pool,
            &WechatMenuInput {
                name: "rustcms-wx-menu".into(),
                kind: "view".into(),
                value: "https://example.com".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(parent.sort, 10);

        let child = menu_save(
            &pool,
            &WechatMenuInput {
                name: "rustcms-wx-sub".into(),
                parent_id: parent.id as i32,
                kind: "click".into(),
                value: "KEY".into(),
                sort: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // 上级不存在
        assert!(menu_save(
            &pool,
            &WechatMenuInput {
                name: "rustcms-wx-bad".into(),
                parent_id: 999_999,
                ..Default::default()
            }
        )
        .await
        .is_err());

        let list = menu_list(&pool).await.unwrap();
        assert!(list.iter().any(|item| item.id == parent.id));
        assert_eq!(
            list.iter()
                .find(|item| item.id == child.id)
                .map(|item| item.kind.clone()),
            Some("click".to_string())
        );

        // 删除父级连带子级
        let deleted = menu_delete(&pool, parent.id).await.unwrap();
        assert_eq!(deleted, 2);
        assert!(menu_get(&pool, child.id).await.unwrap().is_none());

        let _ = sqlx::query("DELETE FROM wechat_reply_rules WHERE keyword LIKE 'rustcms-wx%'")
            .execute(&pool)
            .await;
    }

    #[test]
    fn detects_expired_token_codes() {
        // 微信文档：40001 凭据无效 / 40014 不合法 / 42001 已过期
        assert!(is_token_expired_code(40001));
        assert!(is_token_expired_code(40014));
        assert!(is_token_expired_code(42001));
        assert!(!is_token_expired_code(0));
        assert!(!is_token_expired_code(40013));

        assert!(token_expired_in(&json!({"errcode": 42001, "errmsg": "expired"})));
        assert!(!token_expired_in(&json!({"errcode": 0, "errmsg": "ok"})));
        assert!(!token_expired_in(&json!({"access_token": "x"})));
    }

    #[test]
    fn token_cache_key_covers_section_and_appid() {
        let key = token_cache_key(Section::Wechat, "wx123");
        assert_eq!(key, "wechat:wx123");
        assert_eq!(token_cache_key(Section::Weapp, "wx123"), "weapp:wx123");
        assert_ne!(key, token_cache_key(Section::Weapp, "wx123"));
        // 两个分区互不影响
        assert_eq!(Section::Wechat.setting_key(), WECHAT);
        assert_eq!(Section::Weapp.setting_key(), WEAPP);
        assert_eq!(Section::Wechat.label(), "公众号");
        assert_eq!(Section::Weapp.label(), "小程序");
    }

    #[test]
    fn invalidates_only_target_section() {
        {
            let mut guard = token_cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            guard.clear();
            guard.insert(
                token_cache_key(Section::Wechat, "wx1"),
                TokenEntry {
                    token: "a".into(),
                    expires_at: i64::MAX,
                },
            );
            guard.insert(
                token_cache_key(Section::Weapp, "wx1"),
                TokenEntry {
                    token: "b".into(),
                    expires_at: i64::MAX,
                },
            );
        }
        invalidate_access_token(Section::Wechat);
        let guard = token_cache()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(!guard.contains_key("wechat:wx1"));
        assert!(guard.contains_key("weapp:wx1"));
        drop(guard);
        invalidate_access_token(Section::Weapp);
    }

    #[test]
    fn builds_passive_text_reply() {
        let xml = text_reply_xml("openid_1", "gh_1", "你好", 1700000000);
        assert_eq!(
            xml,
            "<xml><ToUserName><![CDATA[openid_1]]></ToUserName>\
<FromUserName><![CDATA[gh_1]]></FromUserName>\
<CreateTime>1700000000</CreateTime>\
<MsgType><![CDATA[text]]></MsgType>\
<Content><![CDATA[你好]]></Content></xml>"
        );
        // CDATA 里的 ]]> 必须断开，否则微信解析报错
        let xml = text_reply_xml("a", "b", "x]]>y", 1);
        assert!(xml.contains("x]]]]><![CDATA[>y"), "{xml}");
        assert!(!xml.contains("x]]>y"));
    }
}

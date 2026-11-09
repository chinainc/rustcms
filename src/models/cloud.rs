//! 云端服务账号 + AI 对话（`settings.anqi` / `settings.ai_setting` /
//! `ai_agents` / `ai_agent_logs` / `ai_chat_messages`）。
//!
//! 云端服务与 AI 对话的数据访问层：
//! - 云端服务登录态存 `settings.anqi`。**登录与校验已实现外呼**：
//! `login` 打云端登录接口并按约定回写 token/过期时间，`auth_check` 支持 force 强校验、
//! 失败不覆盖本地态；`auth_info_get` 仍保留「读站内已存态并现算 `valid`」的快速路径；
//! - 云端通用请求：`cloud_post` / `cloud_post_raw`（地址由 `api_base` 决定，可被
//! `settings.anqi.api_url` 覆盖），`parse_envelope` 统一解信封；
//! - 翻译/伪原创走 `cloud_translate`；自建引擎优先时由 `translate` 模块负责；
//! - AI：`ai_provider` 解析 `settings.ai_setting` 的 `configs`（OpenAI 兼容 base_url/key/model），
//! `chat_payload` / `parse_chat_content` / `ai_complete` 做对话，`image_payload` /
//! `parse_image_result` / `ai_generate_image` 做生图（云端为主路径，`custom:<下标>` 走兼容分支）；
//! - AI 接口配置存 `settings.ai_setting`，`configs` 数组整份
//! 替换、`last_model` / `mcp` 保留；
//! - 会话消息持久化在 `ai_chat_messages`（发送即同步落库），
//! `/ai/history` 与 `/ai/sessions` 直接从表里读，不依赖内存态；
//! - 智能体在 `ai_agents` / `ai_agent_logs`。
//!
//! ⚠️ 单测只覆盖信封解析、payload 构造、结果解析等纯逻辑，不发起真实外呼。

use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use sqlx::Row;

use super::{setting, ModelError};

/// 云端服务账号配置键。
pub const SETTING_KEY: &str = "anqi";
/// AI 接口配置键（与 `site_setting::AI` 同值）。
pub const AI_SETTING_KEY: &str = "ai_setting";

// ─────────────────────────── 云端服务授权信息 ───────────────────────────

/// 云端账号配置的完整默认值。
fn default_cloud_user() -> Value {
    json!({
        "user_name": "",
        "auth_id": 0,
        "login_time": 0,
        "check_time": 0,
        "created_time": 0,
        "hash_key": "",
        "expire_time": 0,
        "integral": 0,
        "status": 0,
        "token": "",
        "free_token": 0,
        "total_token": 0,
        "un_pay_token": 0,
        "is_owe_fee": 0,
        "valid": false,
    })
}

/// 读取站内已存的云端服务登录态，并现算 `valid`。
///
/// 这是**不发起外呼**的快速路径：`GET /anqi/info` 用它。
/// 需要与云端对账的场景请用 `auth_check(pool, true)`（`GET /anqi/check`）。
pub async fn auth_info_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let stored = setting::get_json(pool, SETTING_KEY).await?;
    let mut base = default_cloud_user();
    if let (Value::Object(base_map), Some(Value::Object(stored_map))) =
        (&mut base, stored.as_ref())
    {
        for (key, value) in stored_map {
            if key == "valid" {
                continue; // valid 永远远程现算，不读库里旧值
            }
            base_map.insert(key.clone(), value.clone());
        }
    }
    let expire_time = base.get("expire_time").and_then(Value::as_i64).unwrap_or(0);
    let now = chrono::Utc::now().timestamp();
    if let Value::Object(map) = &mut base {
        map.insert("valid".into(), json!(expire_time > now));
    }
    Ok(base)
}

// ─────────────────────────── 云端服务外呼 ───────────────────────────
//
// 所有请求打到云端 API 根地址，统一带三个头：
// `token`（`settings.anqi.token`）、`domain`（站点 `settings.system.base_url`）、
// `User-Agent: <产品名>/<版本>`；响应信封 `{code,msg,data}`，`code != 0` 即业务失败。

/// 云端服务地址的环境变量名。
pub const CLOUD_API_ENV: &str = "RUSTCMS_CLOUD_API";

/// 云端服务配置的 settings 键（`{"api_base": "https://..."}`）。
pub const CLOUD_SETTING_KEY: &str = "cloud";

/// 本项目默认**不连任何外部服务**：独立部署不配置就是纯本地运行。
/// 需要云端能力时，用 `settings.cloud.api_base` 或环境变量 `RUSTCMS_CLOUD_API`
/// 显式指向自建网关，响应信封仍按 `{code,msg,data}` 解析。
pub fn api_base() -> String {
    api_base_from(std::env::var(CLOUD_API_ENV).ok().as_deref())
}

/// [`api_base`] 的纯函数形态：未配置/空串 → 空串，尾部 `/` 去掉。
pub fn api_base_from(override_value: Option<&str>) -> String {
    override_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_default()
}

/// 取本次请求要用的云端地址：优先 `settings.cloud.api_base`，再退回环境变量。
pub async fn api_base_for(pool: &MySqlPool) -> String {
    let configured = setting::get_json(pool, CLOUD_SETTING_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|value| {
            value
                .get("api_base")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    if configured.trim().is_empty() {
        api_base()
    } else {
        api_base_from(Some(configured.as_str()))
    }
}

/// 取云端地址；未配置时返回可直接展示给前端的错误（**绝不外呼**）。
pub async fn api_base_or_error(pool: &MySqlPool) -> Result<String, CloudError> {
    let base = api_base_for(pool).await;
    if base.is_empty() {
        return Err(CloudError::Network(format!(
            "云端服务未配置：请设置 `settings.cloud.api_base` 或环境变量 {CLOUD_API_ENV}"
        )));
    }
    Ok(base)
}

/// 云端调用失败：区分「网络/解析失败」与「云端业务拒绝」。
///
/// 两者的处理不同——网络失败只更新 `check_time`，`code != 0` 才清空登录态。
#[derive(Debug)]
pub enum CloudError {
    /// 请求发不出去 / 响应不是合法信封
    Network(String),
    /// 云端回 `code != 0`
    Rejected { code: i64, msg: String },
}

impl CloudError {
    /// 可直接展示给前端的中文文案。
    pub fn message(&self) -> String {
        match self {
            Self::Network(msg) => msg.clone(),
            Self::Rejected { code, msg } => {
                if msg.trim().is_empty() {
                    format!("云端服务返回失败（code={code}）")
                } else {
                    msg.clone()
                }
            }
        }
    }
}

/// 截断异常正文，避免把整段响应塞进错误提示。
fn short_text(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// 云端请求头：token / domain / User-Agent。
async fn cloud_headers(pool: &MySqlPool) -> Vec<(String, String)> {
    let token = setting::get_json(pool, SETTING_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|value| {
            value
                .get("token")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    let domain = super::site_setting::get_section(pool, super::site_setting::SYSTEM)
        .await
        .ok()
        .and_then(|value| {
            value
                .get("base_url")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string();
    vec![
        ("token".to_string(), token),
        ("domain".to_string(), domain),
        (
            "User-Agent".to_string(),
            format!("rustcms/{}", env!("CARGO_PKG_VERSION")),
        ),
    ]
}

/// 解析云端信封：`code != 0` → [`CloudError::Rejected`]。
pub fn parse_envelope(text: &str) -> Result<Value, CloudError> {
    let value: Value = serde_json::from_str(text)
        .map_err(|_| CloudError::Network(format!("云端服务返回异常：{}", short_text(text, 200))))?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let msg = value
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        return Err(CloudError::Rejected { code, msg });
    }
    Ok(value)
}

/// POST JSON 到云端服务，返回完整信封（保留错误类型）。
pub async fn cloud_post_raw(
    pool: &MySqlPool,
    path: &str,
    payload: &Value,
) -> Result<Value, CloudError> {
    let base = api_base_or_error(pool).await?;
    let mut request = rustcms_ext::http::Request::post(format!("{base}{path}"))
        .with_json(payload.clone())
        .with_timeout(std::time::Duration::from_secs(300));
    for (key, value) in cloud_headers(pool).await {
        request = request.with_header(key, value);
    }
    let response = rustcms_ext::http::send(&request)
        .await
        .map_err(|err| CloudError::Network(format!("云端服务请求失败：{err}")))?;
    let text = response.text();
    // 非 2xx 且响应不是信封（网关错误页等）：按网络失败处理
    if !response.is_success() && serde_json::from_str::<Value>(&text).is_err() {
        return Err(CloudError::Network(format!(
            "云端服务请求失败（HTTP {}）",
            response.status
        )));
    }
    parse_envelope(&text)
}

/// POST JSON 到云端服务（失败统一转成中文文案）。
pub async fn cloud_post(pool: &MySqlPool, path: &str, payload: &Value) -> Result<Value, String> {
    cloud_post_raw(pool, path, payload)
        .await
        .map_err(|err| err.message())
}

/// 云端账号登录：先清空本地登录态，成功后再落盘云端返回的账号信息。
pub async fn login(pool: &MySqlPool, user_name: &str, password: &str) -> Result<Value, String> {
    let user_name = user_name.trim();
    if user_name.is_empty() || password.is_empty() {
        return Err("请输入云端服务账号和密码".to_string());
    }
    // 先重置，避免登录失败时还挂着上一个账号的 token
    setting::save_json(pool, SETTING_KEY, &json!({}))
        .await
        .map_err(|err| format!("保存登录态失败：{err}"))?;

    let payload = json!({ "user_name": user_name, "password": password });
    let value = cloud_post(pool, "/login", &payload).await?;

    let now = chrono::Utc::now().timestamp();
    let mut stored = value.get("data").cloned().unwrap_or_else(|| json!({}));
    if !stored.is_object() {
        stored = json!({});
    }
    if let Value::Object(map) = &mut stored {
        map.insert("login_time".to_string(), json!(now));
        map.insert("check_time".to_string(), json!(now));
    }
    setting::save_json(pool, SETTING_KEY, &stored)
        .await
        .map_err(|err| format!("保存登录态失败：{err}"))?;
    auth_info_get(pool)
        .await
        .map_err(|err| format!("读取登录态失败：{err}"))
}

/// 云端登录态校验。
///
/// - `auth_id == 0`（从未登录）直接返回，不发请求；
/// - `force = false` 且一小时内校验过则跳过（3600 秒节流）；
/// - 网络失败只更新 `check_time` 并回错误；云端 `code != 0` 清空登录态。
pub async fn auth_check(pool: &MySqlPool, force: bool) -> Result<(), String> {
    let info = auth_info_get(pool)
        .await
        .map_err(|err| format!("读取云端服务登录态失败：{err}"))?;
    let auth_id = info.get("auth_id").and_then(Value::as_i64).unwrap_or(0);
    if auth_id == 0 {
        return Ok(());
    }
    let now = chrono::Utc::now().timestamp();
    let check_time = info.get("check_time").and_then(Value::as_i64).unwrap_or(0);
    if !force && check_time > now - 3600 {
        return Ok(());
    }

    match cloud_post_raw(pool, "/check", &info).await {
        Ok(value) => {
            let mut current = info.clone();
            if let (Value::Object(map), Some(data)) =
                (&mut current, value.get("data").filter(|data| data.is_object()))
            {
                // 只覆盖云端给了的字段（缺字段保持原值）
                for key in [
                    "auth_id",
                    "user_name",
                    "expire_time",
                    "integral",
                    "status",
                    "free_token",
                    "total_token",
                    "un_pay_token",
                    "is_owe_fee",
                    "token",
                ] {
                    if let Some(field) = data.get(key) {
                        map.insert(key.to_string(), field.clone());
                    }
                }
            }
            if let Value::Object(map) = &mut current {
                map.insert("check_time".to_string(), json!(now));
                map.remove("valid"); // valid 永远现算
            }
            let _ = setting::save_json(pool, SETTING_KEY, &current).await;
            Ok(())
        }
        Err(CloudError::Rejected { .. }) => {
            // 云端明确说登录态无效：清空本地登录态
            let _ = setting::save_json(pool, SETTING_KEY, &json!({})).await;
            Ok(())
        }
        Err(err @ CloudError::Network(_)) => {
            // 网络问题不代表账号失效：只记一次校验时间，保留登录态
            let mut current = info;
            if let Value::Object(map) = &mut current {
                map.insert("check_time".to_string(), json!(now));
            }
            let _ = setting::save_json(pool, SETTING_KEY, &current).await;
            Err(err.message())
        }
    }
}

/// 云端翻译（云端分支）：整段文本一次性提交。
pub async fn cloud_translate(
    pool: &MySqlPool,
    texts: &[String],
    language: &str,
    to_language: &str,
) -> Result<Vec<String>, String> {
    if to_language.trim().is_empty() {
        return Err("请选择目标语言".to_string());
    }
    let payload = json!({
        "text": texts,
        "language": language,
        "to_language": to_language,
    });
    let value = cloud_post(pool, "/translate", &payload).await?;
    let translated = parse_translate_texts(&value);
    if translated.len() < texts.len() {
        return Err("翻译结果数量不足".to_string());
    }
    // 记一条整段翻译记录
    super::translate::add_log(pool, language, to_language, &translated).await;
    Ok(translated)
}

/// 从云端 `/translate` 响应里取 `data.text`。
pub fn parse_translate_texts(value: &Value) -> Vec<String> {
    value
        .get("data")
        .and_then(|data| data.get("text"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// ── 站内 AI 接口（OpenAI 兼容）──

/// 取配置项字符串（兼容多个字段别名）。
fn config_string(config: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| config.get(*key).and_then(Value::as_str))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 从 `settings.ai_setting.configs` 里挑第一个可用的接口。
///
/// 返回 `(base_url, api_key, model, max_tokens, temperature)`。
pub async fn ai_provider(pool: &MySqlPool) -> Result<(String, String, String, u64, f64), String> {
    let ai = ai_settings_get(pool)
        .await
        .map_err(|err| format!("读取 AI 配置失败：{err}"))?;
    let configs = ai
        .get("configs")
        .and_then(Value::as_array)
        .map(|list| list.as_slice())
        .unwrap_or(&[]);
    for config in configs {
        let base_url = config_string(config, &["base_url", "url"]);
        let api_key = config_string(config, &["api_key", "token"]);
        let model = config_string(config, &["model", "name"]);
        if let (Some(base_url), Some(api_key), Some(model)) = (base_url, api_key, model) {
            let max_tokens = config
                .get("max_tokens")
                .and_then(Value::as_u64)
                .filter(|value| *value > 0)
                .unwrap_or(4096);
            let temperature = config
                .get("temperature")
                .and_then(Value::as_f64)
                .unwrap_or(0.7);
            return Ok((base_url, api_key, model, max_tokens, temperature));
        }
    }
    Err("没有选择AI生成来源".to_string())
}

/// 构造 `/chat/completions` 请求体（非流式）。
pub fn chat_payload(
    model: &str,
    prompt: &str,
    max_tokens: u64,
    temperature: f64,
) -> Value {
    json!({
        "model": model,
        "messages": [{ "role": "user", "content": prompt }],
        "max_tokens": max_tokens,
        "temperature": temperature,
        "stream": false,
    })
}

/// 解析 `/chat/completions` 响应里的助手回复。
pub fn parse_chat_content(text: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|_| format!("AI 服务返回异常：{}", short_text(text, 200)))?;
    let content = value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if content.is_empty() {
        return Err("AI 服务返回内容为空".to_string());
    }
    Ok(content)
}

/// 用站内 AI 接口做一次非流式对话，返回助手回复文本。
pub async fn ai_complete(pool: &MySqlPool, prompt: &str) -> Result<String, String> {
    let (base_url, api_key, model, max_tokens, temperature) = ai_provider(pool).await?;
    let link = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let request = rustcms_ext::http::Request::post(link)
        .with_header("authorization", format!("Bearer {api_key}"))
        .with_header("content-type", "application/json")
        .with_json(chat_payload(&model, prompt, max_tokens, temperature))
        .with_timeout(std::time::Duration::from_secs(120));
    let response = rustcms_ext::http::send(&request)
        .await
        .map_err(|err| format!("AI 服务请求失败：{err}"))?;
    let text = response.text();
    if !response.is_success() {
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .or_else(|| value.get("msg").and_then(Value::as_str))
            .unwrap_or("AI 服务返回错误")
            .to_string();
        return Err(message);
    }
    parse_chat_content(&text)
}

/// 构造 `/images/generations` 请求体（非流式，单张）。
pub fn image_payload(model: &str, prompt: &str, size: &str) -> Value {
    let mut payload = json!({
        "model": model,
        "prompt": prompt,
        "n": 1,
    });
    if !size.trim().is_empty() {
        payload["size"] = json!(size.trim());
    }
    payload
}

/// 解析 `/images/generations` 响应：优先 `url`，其次 `b64_json`。
pub fn parse_image_result(text: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|_| format!("AI 生图返回异常：{}", short_text(text, 200)))?;
    let Some(item) = value.get("data").and_then(Value::as_array).and_then(|items| items.first())
    else {
        return Err("AI 生图返回空结果".to_string());
    };
    if let Some(url) = item.get("url").and_then(Value::as_str) {
        if !url.trim().is_empty() {
            return Ok(url.trim().to_string());
        }
    }
    if let Some(raw) = item.get("b64_json").and_then(Value::as_str) {
        if !raw.trim().is_empty() {
            return Ok(format!("data:image/png;base64,{}", raw.trim()));
        }
    }
    Err("AI 生图返回空结果".to_string())
}

/// 用站内 AI 接口生图（OpenAI 兼容 `images/generations`），返回图片地址。
///
/// 云端服务不可用时这是唯一可用的生图通道；返回结构由调用方包装成云端口径。
pub async fn ai_generate_image(
    pool: &MySqlPool,
    prompt: &str,
    size: &str,
) -> Result<Value, String> {
    let (base_url, api_key, model, _, _) = ai_provider(pool).await?;
    let link = format!("{}/images/generations", base_url.trim_end_matches('/'));
    let request = rustcms_ext::http::Request::post(link)
        .with_header("authorization", format!("Bearer {api_key}"))
        .with_header("content-type", "application/json")
        .with_json(image_payload(&model, prompt, size))
        .with_timeout(std::time::Duration::from_secs(300));
    let response = rustcms_ext::http::send(&request)
        .await
        .map_err(|err| format!("AI 生图请求失败：{err}"))?;
    let text = response.text();
    if !response.is_success() {
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .or_else(|| value.get("msg").and_then(Value::as_str))
            .unwrap_or("AI 生图失败")
            .to_string();
        return Err(message);
    }
    let result = parse_image_result(&text)?;
    Ok(json!({
        "id": 0,
        "usage": 0,
        "prompt": prompt,
        "type": 0,
        "status": "success",
        "result": result,
        "created_time": chrono::Utc::now().timestamp(),
    }))
}

// ─────────────────────────── AI 接口配置 ───────────────────────────

/// `eino.Configs` 的完整默认值（含 `mcp` 子对象）。
fn default_ai_configs() -> Value {
    json!({
        "configs": [],
        "last_model": "",
        "mcp": {
            "enabled": false,
            "token": "",
            "exposed_tools": [],
            "exposed_intents": [],
            "tool_list_mode": "",
            "enable_set_scope": false,
            "rate_limit": 0,
            "invoke_admin_id": 0,
            "api_exposure": {
                "mode": "",
                "allow_ns": [],
                "deny_ns": [],
                "deny_endpoints": [],
            },
        },
    })
}

/// 读 AI 接口配置（缺省字段按零值补齐）。
pub async fn ai_settings_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let stored = setting::get_json(pool, AI_SETTING_KEY).await?;
    let mut base = default_ai_configs();
    if let (Value::Object(base_map), Some(Value::Object(stored_map))) =
        (&mut base, stored.as_ref())
    {
        for (key, value) in stored_map {
            base_map.insert(key.clone(), value.clone());
        }
    }
    Ok(base)
}

/// 保存 AI 接口配置：请求体是 `eino.Config` 对象数组，**整份替换** `configs`，
/// `last_model` / `mcp` 保留原值。
pub async fn ai_settings_save(
    pool: &MySqlPool,
    configs: &[Value],
) -> Result<Value, ModelError> {
    let mut current = ai_settings_get(pool).await?;
    if let Value::Object(map) = &mut current {
        map.insert("configs".into(), Value::Array(configs.to_vec()));
    }
    setting::save_json(pool, AI_SETTING_KEY, &current).await?;
    Ok(current)
}

// ─────────────────────────── AI 智能体 ───────────────────────────

fn agent_json(row: &sqlx::mysql::MySqlRow) -> Value {
    json!({
        "id": row.get::<u32, _>("id"),
        "created_time": row.get::<Option<i64>, _>("created_time").unwrap_or(0),
        "updated_time": row.get::<Option<i64>, _>("updated_time").unwrap_or(0),
        "session_id": row.get::<Option<String>, _>("session_id").unwrap_or_default(),
        "name": row.get::<Option<String>, _>("name").unwrap_or_default(),
        "strategy": row.get::<Option<String>, _>("strategy").unwrap_or_default(),
        "cron_expr": row.get::<Option<String>, _>("cron_expr").unwrap_or_default(),
        "enabled": row.get::<i32, _>("enabled"),
        "last_run_at": row.get::<Option<i64>, _>("last_run_at").unwrap_or(0),
        "next_run_at": row.get::<Option<i64>, _>("next_run_at").unwrap_or(0),
        "run_count": row.get::<i32, _>("run_count"),
        "max_runs": row.get::<i32, _>("max_runs"),
        "max_rounds": row.get::<i32, _>("max_rounds"),
        "last_summary": row.get::<Option<String>, _>("last_summary").unwrap_or_default(),
    })
}

/// 全部智能体（`id ASC`）。
pub async fn agent_list(pool: &MySqlPool) -> anyhow::Result<Vec<Value>> {
    let rows =
        sqlx::query("SELECT * FROM ai_agents ORDER BY id ASC").fetch_all(pool).await?;
    Ok(rows.iter().map(agent_json).collect())
}

/// 智能体是否存在。
pub async fn agent_exists(pool: &MySqlPool, agent_id: u32) -> anyhow::Result<bool> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ai_agents WHERE id = ?")
        .bind(agent_id)
        .fetch_one(pool)
        .await?;
    Ok(count > 0)
}

/// 读取单个智能体（不存在返回 `None`）。
pub async fn agent_get(pool: &MySqlPool, agent_id: u32) -> anyhow::Result<Option<Value>> {
    let row = sqlx::query("SELECT * FROM ai_agents WHERE id = ?")
        .bind(agent_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(agent_json))
}

/// 指定智能体的执行日志（`id DESC` 分页；不回 total）。
pub async fn agent_logs(
    pool: &MySqlPool,
    agent_id: u32,
    page: i64,
    page_size: i64,
) -> anyhow::Result<Vec<Value>> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size };
    let offset = (page - 1) * page_size;
    let rows = sqlx::query(
        "SELECT * FROM ai_agent_logs WHERE agent_id = ? ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(agent_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<u32, _>("id"),
                "created_time": row.get::<Option<i64>, _>("created_time").unwrap_or(0),
                "agent_id": row.get::<u32, _>("agent_id"),
                "session_id": row.get::<Option<String>, _>("session_id").unwrap_or_default(),
                "status": row.get::<i32, _>("status"),
                "summary": row.get::<Option<String>, _>("summary").unwrap_or_default(),
                "error": row.get::<Option<String>, _>("error").unwrap_or_default(),
                "tool_calls": row.get::<i32, _>("tool_calls"),
                "started_at": row.get::<Option<i64>, _>("started_at").unwrap_or(0),
                "finished_at": row.get::<Option<i64>, _>("finished_at").unwrap_or(0),
            })
        })
        .collect())
}

// ─────────────────────────── 会话消息（ai_chat_messages）──

/// 单条会话消息的 JSON 形态；
/// `files` 库里是 JSON 文本，解析失败时按缺省处理）。
fn chat_message_json(row: &sqlx::mysql::MySqlRow) -> Value {
    let files_raw: String = row.get::<Option<String>, _>("files").unwrap_or_default();
    let files = serde_json::from_str::<Value>(&files_raw)
        .ok()
        .filter(Value::is_array)
        .unwrap_or_else(|| json!([]));
    json!({
        "id": row.get::<u32, _>("id"),
        "created_time": row.get::<Option<i64>, _>("created_time").unwrap_or(0),
        "seq": row.get::<u32, _>("seq"),
        "session_id": row.get::<Option<String>, _>("session_id").unwrap_or_default(),
        "role": row.get::<Option<String>, _>("role").unwrap_or_default(),
        "content": row.get::<Option<String>, _>("content").unwrap_or_default(),
        "tool_call_id": row.get::<Option<String>, _>("tool_call_id").unwrap_or_default(),
        "tool_name": row.get::<Option<String>, _>("tool_name").unwrap_or_default(),
        "tool_calls": row.get::<Option<String>, _>("tool_calls").unwrap_or_default(),
        "turn_id": row.get::<u32, _>("turn_id"),
        "files": files,
    })
}

/// 指定会话的历史消息，按 `seq ASC` 从库里重建。
pub async fn chat_messages(
    pool: &MySqlPool,
    session_id: &str,
) -> anyhow::Result<Vec<Value>> {
    let rows = sqlx::query(
        "SELECT * FROM ai_chat_messages WHERE session_id = ? ORDER BY seq ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(chat_message_json).collect())
}

/// 全部会话的摘要列表：
/// `title` 取第一条用户消息（>50 字加 `...`），`last_message` 取最后一条
/// （>100 字加 `...`）。
pub async fn chat_sessions(pool: &MySqlPool) -> anyhow::Result<Vec<Value>> {
    let rows = sqlx::query(
        "SELECT session_id, MIN(created_time) AS created_time, \
         MAX(created_time) AS updated_at, COUNT(*) AS msg_count \
         FROM ai_chat_messages GROUP BY session_id ORDER BY created_time ASC",
    )
    .fetch_all(pool)
    .await?;
    let mut result = Vec::new();
    for row in &rows {
        let session_id: String =
            row.get::<Option<String>, _>("session_id").unwrap_or_default();
        let title: String = sqlx::query_scalar(
            "SELECT content FROM ai_chat_messages \
             WHERE session_id = ? AND role = 'user' ORDER BY seq ASC LIMIT 1",
        )
        .bind(&session_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or_default();
        let last: String = sqlx::query_scalar(
            "SELECT content FROM ai_chat_messages WHERE session_id = ? ORDER BY seq DESC LIMIT 1",
        )
        .bind(&session_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or_default();
        result.push(json!({
            "session_id": session_id,
            "title": truncate_runes(&title, 50),
            "created_time": row.get::<Option<i64>, _>("created_time").unwrap_or(0),
            "updated_at": row.get::<Option<i64>, _>("updated_at").unwrap_or(0),
            "msg_count": row.get::<i64, _>("msg_count"),
            "last_message": truncate_runes(&last, 100),
        }));
    }
    Ok(result)
}

/// 按 rune 截断，超长补 `...`。
fn truncate_runes(text: &str, max: usize) -> String {
    let runes: Vec<char> = text.chars().collect();
    if runes.len() > max {
        let cut: String = runes[..max].iter().collect();
        format!("{cut}...")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    /// 表不存在时跳过用例（表由建表脚本创建）。
    async fn table_exists(pool: &MySqlPool, name: &str) -> bool {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_name = ?",
        )
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        count > 0
    }

    #[tokio::test]
    async fn auth_info_roundtrip_and_valid_flag() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, SETTING_KEY).await.unwrap();

        // 默认值补齐 + valid=false
        let info = auth_info_get(&pool).await.unwrap();
        assert_eq!(info["user_name"], json!(""), "默认配置：{info}");
        assert_eq!(info["valid"], json!(false));

        // 未过期 → valid=true；已过期 → false
        let now = chrono::Utc::now().timestamp();
        setting::save_json(
            &pool,
            SETTING_KEY,
            &json!({"user_name": "tester", "auth_id": 7, "expire_time": now + 3600}),
        )
        .await
        .unwrap();
        let info = auth_info_get(&pool).await.unwrap();
        assert_eq!(info["user_name"], json!("tester"));
        assert_eq!(info["auth_id"], json!(7));
        assert_eq!(info["valid"], json!(true));

        setting::save_json(&pool, SETTING_KEY, &json!({"expire_time": now - 10}))
            .await
            .unwrap();
        let info = auth_info_get(&pool).await.unwrap();
        assert_eq!(info["valid"], json!(false));

        match backup {
            Some(raw) => setting::save_raw(&pool, SETTING_KEY, &raw).await.unwrap(),
            None => setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
    }

    #[tokio::test]
    async fn ai_settings_replace_configs_keep_rest() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, AI_SETTING_KEY).await.unwrap();
        setting::remove(&pool, AI_SETTING_KEY).await.unwrap();

        let loaded = ai_settings_get(&pool).await.unwrap();
        assert_eq!(loaded["configs"], json!([]), "默认配置：{loaded}");
        assert_eq!(loaded["mcp"]["enabled"], json!(false));
        assert!(loaded["mcp"]["api_exposure"].is_object());

        // 保存：configs 整份替换
        let saved = ai_settings_save(
            &pool,
            &[json!({
                "api_key": "sk-test",
                "model": "deepseek-chat",
                "base_url": "https://api.deepseek.com/v1",
                "max_tokens": 4096,
                "temperature": 0.7,
                "name": "默认"
            })],
        )
        .await
        .unwrap();
        assert_eq!(saved["configs"].as_array().unwrap().len(), 1);
        assert_eq!(saved["configs"][0]["api_key"], json!("sk-test"));

        // last_model / mcp 保留；configs 再替换
        setting::save_json(&pool, AI_SETTING_KEY, &json!({"last_model": "custom:0"}))
            .await
            .unwrap();
        let saved = ai_settings_save(&pool, &[]).await.unwrap();
        assert_eq!(saved["last_model"], json!("custom:0"), "last_model 不该被清掉：{saved}");
        assert_eq!(saved["configs"], json!([]));
        assert!(saved["mcp"]["api_exposure"].is_object(), "mcp 默认值保留：{saved}");

        match backup {
            Some(raw) => setting::save_raw(&pool, AI_SETTING_KEY, &raw).await.unwrap(),
            None => setting::remove(&pool, AI_SETTING_KEY).await.unwrap(),
        }
    }

    #[tokio::test]
    async fn agents_and_logs_read_from_tables() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        if !table_exists(&pool, "ai_agents").await {
            eprintln!("跳过用例：ai_agents 表不存在");
            return;
        }
        let agents = agent_list(&pool).await.unwrap();
        // 只验证可读 + 字段齐全（不依赖真实数据）
        for agent in agents.iter().take(1) {
            assert!(agent.get("id").is_some());
            assert!(agent.get("name").is_some());
            assert!(agent.get("cron_expr").is_some());
        }
        if !table_exists(&pool, "ai_agent_logs").await {
            eprintln!("跳过断言：ai_agent_logs 表不存在");
            return;
        }
        let logs = agent_logs(&pool, 0, 1, 20).await.unwrap();
        assert!(logs.is_empty() || logs[0].get("agent_id").is_some());
    }

    #[tokio::test]
    async fn chat_messages_and_sessions_aggregate() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        if !table_exists(&pool, "ai_chat_messages").await {
            eprintln!("跳过用例：ai_chat_messages 表不存在");
            return;
        }
        let session = "__rustcms_ai_session__";
        sqlx::query("DELETE FROM ai_chat_messages WHERE session_id = ?")
            .bind(session)
            .execute(&pool)
            .await
            .unwrap();

        let now = chrono::Utc::now().timestamp();
        let rows = [
            (1, "user", "第一条用户消息，作为会话标题", now),
            (2, "assistant", "第一条回复", now + 1),
            (3, "user", "第二条", now + 2),
        ];
        for (seq, role, content, created) in rows {
            sqlx::query(
                "INSERT INTO ai_chat_messages (created_time, seq, session_id, role, content) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(created)
            .bind(seq)
            .bind(session)
            .bind(role)
            .bind(content)
            .execute(&pool)
            .await
            .unwrap();
        }

        let messages = chat_messages(&pool, session).await.unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], json!("user"));
        assert_eq!(messages[0]["seq"], json!(1));
        assert!(messages[0]["files"].is_array());

        let sessions = chat_sessions(&pool).await.unwrap();
        let found = sessions
            .iter()
            .find(|item| item["session_id"] == json!(session))
            .expect("会话应出现在列表里");
        assert_eq!(found["title"], json!("第一条用户消息，作为会话标题"));
        assert_eq!(found["last_message"], json!("第二条"));
        assert_eq!(found["msg_count"], json!(3));
        assert_eq!(found["updated_at"], json!(now + 2));

        // 长文本截断
        let long = "字".repeat(120);
        sqlx::query("UPDATE ai_chat_messages SET content = ? WHERE session_id = ? AND seq = 3")
            .bind(&long)
            .bind(session)
            .execute(&pool)
            .await
            .unwrap();
        let sessions = chat_sessions(&pool).await.unwrap();
        let found = sessions
            .iter()
            .find(|item| item["session_id"] == json!(session))
            .unwrap();
        let last = found["last_message"].as_str().unwrap();
        assert!(last.ends_with("...") && last.chars().count() == 103, "{last}");

        sqlx::query("DELETE FROM ai_chat_messages WHERE session_id = ?")
            .bind(session)
            .execute(&pool)
            .await
            .unwrap();
    }

    // ---------------------------- 云端外呼纯逻辑 ----------------------------

    #[test]
    fn envelope_distinguishes_rejected_and_garbage() {
        // 正常信封
        let value = parse_envelope(r#"{"code":0,"msg":"","data":{"token":"t"}}"#).unwrap();
        assert_eq!(value["data"]["token"], json!("t"));

        // code != 0 → 业务拒绝，msg 原样带回
        match parse_envelope(r#"{"code":1001,"msg":"账号或密码错误"}"#) {
            Err(CloudError::Rejected { code, msg }) => {
                assert_eq!(code, 1001);
                assert_eq!(msg, "账号或密码错误");
            }
            other => panic!("应识别为业务拒绝：{other:?}"),
        }
        // msg 为空时兜底成中文（带 code，便于排查）
        match parse_envelope(r#"{"code":7}"#) {
            Err(err) => assert!(err.message().contains("code=7"), "{}", err.message()),
            Ok(_) => panic!("应失败"),
        }
        // 不是 JSON → 网络/解析失败（不能当成账号失效）
        match parse_envelope("<html>502 Bad Gateway</html>") {
            Err(CloudError::Network(msg)) => assert!(msg.contains("返回异常"), "{msg}"),
            other => panic!("应识别为解析失败：{other:?}"),
        }
    }

    #[test]
    fn cloud_api_base_defaults_to_disabled_and_can_be_overridden() {
        // 独立部署不配置就不外呼：空串是「未配置」的哨兵值。
        assert_eq!(api_base_from(None), "");
        assert_eq!(api_base_from(Some("   ")), "");
        // 尾部斜杠要去掉，避免拼出 `//login`
        assert_eq!(
            api_base_from(Some(" http://127.0.0.1:1/auth/ ")),
            "http://127.0.0.1:1/auth"
        );
        assert_eq!(CLOUD_API_ENV, "RUSTCMS_CLOUD_API");
        assert_eq!(CLOUD_SETTING_KEY, "cloud");
        // 真实入口在没有环境变量时同样是「未配置」
        if std::env::var(CLOUD_API_ENV).is_err() {
            assert_eq!(api_base(), "");
        }
    }

    #[test]
    fn translate_texts_extracted_from_cloud() {
        let value = json!({"code":0,"data":{"text":["t1","t2","t3"]}});
        assert_eq!(parse_translate_texts(&value), vec!["t1", "t2", "t3"]);
        // 非字符串项被跳过（防御坏数据）
        let value = json!({"code":0,"data":{"text":["t1", 3, null]}});
        assert_eq!(parse_translate_texts(&value), vec!["t1"]);
        // 没有 text 字段 → 空
        assert!(parse_translate_texts(&json!({"data":{}})).is_empty());
    }

    #[test]
    fn chat_payload_and_reply_parsing() {
        let payload = chat_payload("deepseek-chat", "你好", 4096, 0.7);
        assert_eq!(payload["model"], json!("deepseek-chat"));
        assert_eq!(payload["messages"][0]["role"], json!("user"));
        assert_eq!(payload["messages"][0]["content"], json!("你好"));
        assert_eq!(payload["stream"], json!(false));
        assert_eq!(payload["max_tokens"], json!(4096));

        let reply = parse_chat_content(
            r#"{"choices":[{"message":{"role":"assistant","content":" 你好呀 "}}]}"#,
        )
        .unwrap();
        assert_eq!(reply, "你好呀");
        // 空内容 / 坏 JSON 都要给中文提示
        let err = parse_chat_content(r#"{"choices":[{"message":{"content":""}}]}"#).unwrap_err();
        assert!(err.contains("内容为空"), "{err}");
        let err = parse_chat_content("not-json").unwrap_err();
        assert!(err.contains("返回异常"), "{err}");
    }

    #[test]
    fn image_payload_and_result_parsing() {
        let payload = image_payload("dall-e-3", "一只猫", "1024x1024");
        assert_eq!(payload["prompt"], json!("一只猫"));
        assert_eq!(payload["n"], json!(1));
        assert_eq!(payload["size"], json!("1024x1024"));
        // 尺寸为空时不带该字段（由服务端用默认值）
        assert!(image_payload("m", "p", "  ").get("size").is_none());

        let url = parse_image_result(r#"{"data":[{"url":"https://x/y.png"}]}"#).unwrap();
        assert_eq!(url, "https://x/y.png");
        // 只给 b64 时包成 data URI
        let data = parse_image_result(r#"{"data":[{"b64_json":"QUJD"}]}"#).unwrap();
        assert_eq!(data, "data:image/png;base64,QUJD");
        let err = parse_image_result(r#"{"data":[]}"#).unwrap_err();
        assert!(err.contains("空结果"), "{err}");
    }
}

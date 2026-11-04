//! 云端服务账号 + AI 对话（`settings.anqi` / `settings.ai_setting` /
//! `ai_agents` / `ai_agent_logs` / `ai_chat_messages`）。
//!
//! 对齐 Go 版 `anqi.go` + `aiChat.go` 控制器背后的 provider/model：
//! - 云端服务登录态存 `settings.anqi`（`config.AnqiUserConfig`），登录/校验本身
//!   需要外呼云端服务，这里只做**站内已存登录态**的读取（`valid` 现算）；
//! - AI 接口配置存 `settings.ai_setting`（`eino.Configs`），`configs` 数组整份
//!   替换、`last_model` / `mcp` 保留（Go `SaveAiSettings` 语义）；
//! - 会话消息持久化在 `ai_chat_messages`（Go `AddMessage` 同步落库），
//!   `/ai/history` 与 `/ai/sessions` 直接从表里读，不依赖内存态；
//! - 智能体在 `ai_agents` / `ai_agent_logs`。

use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use sqlx::Row;

use super::{setting, ModelError};

/// 云端服务账号配置键（Go `provider.AnqiSettingKey`）。
pub const SETTING_KEY: &str = "anqi";
/// AI 接口配置键（Go `provider.AiSettingKey`，与 `site_setting::AI` 同值）。
pub const AI_SETTING_KEY: &str = "ai_setting";

// ─────────────────────────── 云端服务授权信息 ───────────────────────────

/// `config.AnqiUserConfig` 的完整默认值。
fn default_anqi_user() -> Value {
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

/// 读取站内已存的云端服务登录态，并现算 `valid`（Go `GetAuthInfo`）。
///
/// Go 的登录 / 云端校验需要外呼云端 API，未实现；
/// 这里保证 `GET /anqi/info` / `GET /anqi/check` 至少能把已存登录态读出来。
pub async fn auth_info_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let stored = setting::get_json(pool, SETTING_KEY).await?;
    let mut base = default_anqi_user();
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

/// 读 AI 接口配置（Go `LoadAiSetting`：缺省字段按零值补齐）。
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
/// `last_model` / `mcp` 保留原值（Go `SaveAiSettings`）。
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

/// 全部智能体（Go `AiAgentList`：`id ASC`）。
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

/// 指定智能体的执行日志（`id DESC` 分页；Go `AiAgentLog` 不回 total）。
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

/// 单条会话消息（Go `dbMessageToChatMessage` 的 JSON 形态；
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

/// 指定会话的历史消息，按 `seq ASC`（Go `GetMessages` 从库里重建）。
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

/// 全部会话的摘要列表（Go `ListSessions` 的 DB 版）：
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

/// 按 rune 截断，超长补 `...`（Go `ListSessions` 的截断方式）。
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

    /// 表不存在时跳过用例（表由 Go 版 AutoMigrate 建）。
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
}

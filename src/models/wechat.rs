//! 微信公众号 / 小程序（`pluginWechat.go`、`pluginWeapp.go`）。
//!
//! 配置存 `settings.wechat` / `settings.weapp`（两者结构相同，公众号多两个验证相关字段）。
//! 消息、自动回复、菜单三张表纯 DB 读写；**与微信服务器的交互（拉消息、同步菜单）未实现**。

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

/// 记录回复内容。
///
/// 注意：真正把回复发到微信需要 access_token 与 HTTP 客户端，这里只落库。
pub async fn message_reply(
    pool: &MySqlPool,
    id: u32,
    reply: &str,
) -> Result<WechatMessage, ModelError> {
    let reply = reply.trim();
    if reply.is_empty() {
        return Err(ModelError::invalid("请填写回复内容"));
    }
    if message_get(pool, id)
        .await
        .map_err(|err| ModelError::invalid(format!("查询失败: {err}")))?
        .is_none()
    {
        return Err(ModelError::invalid("消息不存在"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, setting, test_pool};

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
        let replied = message_reply(&pool, msg_id, "收到").await.unwrap();
        assert_eq!(replied.reply, "收到");
        assert!(replied.reply_time > 0);

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
}

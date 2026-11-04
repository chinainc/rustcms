//! 翻译插件（`settings.translate` + `translate_logs` / `translate_text_logs` /
//! `translate_html_logs` 表）。
//!
//! 对齐 Go 版 `pluginTranslate.go` + `model/translateLog.go`：
//! - 配置存 `settings.translate`（`config.PluginTranslateConfig` 的 6 个字段）；
//! - 翻译日志（整页翻译）与文本翻译日志是两张表，接口只做分页 / 修正 / 删除；
//! - 真正调用翻译引擎外呼的部分未实现；
//! - 修正译文后，Go 会异步把 `cache/multiLang/<lang>/<md5(uri)>` 缓存文件里的
//!   旧译文替换掉，这里同步执行同样的替换（`ReplaceTranslateText` 的 4 组正则）。

use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use sqlx::Row;

use super::{attachment, site_setting, ModelError};

/// 翻译配置键（Go `provider.TranslateSettingKey`）。
pub const SETTING_KEY: &str = "translate";

/// 配置完整默认值（对应 Go `config.PluginTranslateConfig` 的零值）。
fn default_config() -> Value {
    json!({
        "engine": "",
        "baidu_app_id": "",
        "baidu_app_secret": "",
        "youdao_app_key": "",
        "youdao_app_secret": "",
        "deepl_auth_key": "",
    })
}

/// 读翻译插件配置。
pub async fn config_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let stored = site_setting::get_section(pool, SETTING_KEY).await?;
    let mut base = default_config();
    if let (Value::Object(base_map), Value::Object(stored_map)) = (&mut base, &stored) {
        for (key, value) in stored_map {
            base_map.insert(key.clone(), value.clone());
        }
    }
    Ok(base)
}

/// 保存翻译插件配置（字段级 merge）。
pub async fn config_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    site_setting::save_section(pool, SETTING_KEY, patch).await?;
    config_get(pool).await
}

// ─────────────────────────── 翻译日志（整页） ───────────────────────────

/// 分页取翻译日志，`id desc`（Go `PluginTranslateLogList`）。
pub async fn log_list(
    pool: &MySqlPool,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Value>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size };
    let offset = (page - 1) * page_size;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM translate_logs")
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query("SELECT * FROM translate_logs ORDER BY id DESC LIMIT ? OFFSET ?")
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await?;

    let items = rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<u32, _>("id"),
                "created_time": row.get::<Option<i64>, _>("created_time").unwrap_or(0),
                "md5": row.get::<Option<String>, _>("md5").unwrap_or_default(),
                "origin_title": row.get::<Option<String>, _>("origin_title").unwrap_or_default(),
                "origin_content": row.get::<Option<String>, _>("origin_content").unwrap_or_default(),
                "title": row.get::<Option<String>, _>("title").unwrap_or_default(),
                "content": row.get::<Option<String>, _>("content").unwrap_or_default(),
                "language": row.get::<Option<String>, _>("language").unwrap_or_default(),
                "to_language": row.get::<Option<String>, _>("to_language").unwrap_or_default(),
            })
        })
        .collect();
    Ok((total, items))
}

// ─────────────────────────── 文本翻译日志 ───────────────────────────

fn text_log_json(row: &sqlx::mysql::MySqlRow) -> Value {
    json!({
        "id": row.get::<u32, _>("id"),
        "created_time": row.get::<Option<i64>, _>("created_time").unwrap_or(0),
        "md5": row.get::<Option<String>, _>("md5").unwrap_or_default(),
        "language": row.get::<Option<String>, _>("language").unwrap_or_default(),
        "to_language": row.get::<Option<String>, _>("to_language").unwrap_or_default(),
        "text": row.get::<Option<String>, _>("text").unwrap_or_default(),
        "translated": row.get::<Option<String>, _>("translated").unwrap_or_default(),
    })
}

/// 分页取文本翻译日志，可按原文 / 译文前缀筛选（Go `PluginGetTranslateTextLog`）。
pub async fn text_log_list(
    pool: &MySqlPool,
    page: i64,
    page_size: i64,
    text: &str,
    translated: &str,
) -> anyhow::Result<(i64, Vec<Value>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size };
    let offset = (page - 1) * page_size;

    let mut where_clause = String::from("WHERE 1 = 1");
    if !text.is_empty() {
        where_clause.push_str(&format!(" AND `text` LIKE '{}%'", text.replace('\'', "''")));
    }
    if !translated.is_empty() {
        where_clause.push_str(&format!(
            " AND `translated` LIKE '{}%'",
            translated.replace('\'', "''")
        ));
    }

    let total: i64 =
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM translate_text_logs {where_clause}"))
            .fetch_one(pool)
            .await?;
    let rows = sqlx::query(&format!(
        "SELECT * FROM translate_text_logs {where_clause} ORDER BY id DESC LIMIT ? OFFSET ?"
    ))
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok((total, rows.iter().map(text_log_json).collect()))
}

/// 删除文本翻译日志（Go `PluginRemoveTranslateTextLog`）：
/// - `all = true` 清空整表；
/// - `id > 0` 删单条，**先确认存在**（Go 版专门修过这里：GORM 删不存在的 id
///   是 0 行受影响但不报错，会让调用方误以为删掉了）；
/// - 两者都没给：明确报错，绝不静默「删除成功」。
pub async fn text_log_remove(pool: &MySqlPool, id: u32, all: bool) -> Result<(), ModelError> {
    if all {
        sqlx::query("DELETE FROM translate_text_logs WHERE `id` > 0")
            .execute(pool)
            .await?;
        return Ok(());
    }
    if id > 0 {
        let exist: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM translate_text_logs WHERE `id` = ?")
            .bind(id)
            .fetch_one(pool)
            .await?;
        if exist == 0 {
            return Err(ModelError::invalid(format!(
                "翻译记录 id={id} 不存在，未做任何修改"
            )));
        }
        sqlx::query("DELETE FROM translate_text_logs WHERE `id` = ?")
            .bind(id)
            .execute(pool)
            .await?;
        return Ok(());
    }
    Err(ModelError::invalid(
        "请提供要删除的 id，或设置 all=true 删除全部翻译记录（当前未做任何修改）",
    ))
}

/// 文本翻译保存入参（Go `request.TranslateTextLog`）。
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct TextLogInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub to_language: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub translated: String,
}

/// 添加 / 更新文本翻译，用于修正自动翻译错误（Go `PluginSaveTranslateTextLog`）。
///
/// 按 `id`（优先）或 `md5(language-to_language-text)` 找已有记录做更新，否则新增；
/// 译文有变化时，同步把 `cache/multiLang/<to_language>/` 下相关 HTML 缓存里的
/// 旧译文替换掉（Go 版是异步，这里同步完成）。
pub async fn text_log_save(
    pool: &MySqlPool,
    input: &TextLogInput,
    cache_dir: &std::path::Path,
) -> Result<Value, ModelError> {
    if input.text.is_empty() {
        return Err(ModelError::invalid("请填写内容"));
    }

    let mut language = input.language.trim().to_string();
    if language.is_empty() {
        let system = site_setting::get_section(pool, site_setting::SYSTEM).await?;
        language = system
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or("zh")
            .to_string();
    }
    let to_language = input.to_language.trim().to_string();
    let text_md5 = attachment::md5_hex(format!("{language}-{to_language}-{}", input.text).as_bytes());

    let mut log_id: u32 = 0;
    let mut created_time: i64 = 0;
    let mut log_language = language.clone();
    let mut log_to_language = to_language.clone();
    let mut log_text = input.text.clone();
    let mut origin_text = input.text.clone();

    if input.id > 0 {
        if let Some(row) = sqlx::query("SELECT * FROM translate_text_logs WHERE `id` = ?")
            .bind(input.id)
            .fetch_optional(pool)
            .await?
        {
            // 更新
            log_id = row.get::<u32, _>("id");
            created_time = row.get::<Option<i64>, _>("created_time").unwrap_or(0);
            log_language = row.get::<Option<String>, _>("language").unwrap_or_default();
            log_to_language = row
                .get::<Option<String>, _>("to_language")
                .unwrap_or_default();
            log_text = row.get::<Option<String>, _>("text").unwrap_or_default();
            origin_text = row
                .get::<Option<String>, _>("translated")
                .unwrap_or_default();
        }
    } else if let Some(row) =
        sqlx::query("SELECT * FROM translate_text_logs WHERE `md5` = ?").bind(&text_md5)
            .fetch_optional(pool)
            .await?
    {
        // 更新
        log_id = row.get::<u32, _>("id");
        created_time = row.get::<Option<i64>, _>("created_time").unwrap_or(0);
        log_language = row.get::<Option<String>, _>("language").unwrap_or_default();
        log_to_language = row
            .get::<Option<String>, _>("to_language")
            .unwrap_or_default();
        log_text = row.get::<Option<String>, _>("text").unwrap_or_default();
        origin_text = row
            .get::<Option<String>, _>("translated")
            .unwrap_or_default();
    }

    if log_id > 0 {
        sqlx::query("UPDATE translate_text_logs SET `translated` = ? WHERE `id` = ?")
            .bind(&input.translated)
            .bind(log_id)
            .execute(pool)
            .await?;
    } else {
        let now = chrono::Utc::now().timestamp();
        let result = sqlx::query(
            "INSERT INTO translate_text_logs (created_time, `md5`, language, to_language, \
             `text`, `translated`) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(now)
        .bind(&text_md5)
        .bind(&log_language)
        .bind(&log_to_language)
        .bind(&log_text)
        .bind(&input.translated)
        .execute(pool)
        .await?;
        log_id = result.last_insert_id() as u32;
        created_time = now;
    }

    // 译文有变化 → 全局替换缓存里的旧译文
    if !origin_text.is_empty() && origin_text != input.translated {
        replace_html_caches(pool, cache_dir, &log_to_language, &origin_text, &input.translated)
            .await?;
    }

    Ok(json!({
        "id": log_id,
        "created_time": created_time,
        "md5": text_md5,
        "language": log_language,
        "to_language": log_to_language,
        "text": log_text,
        "translated": input.translated,
    }))
}

/// 遍历 `translate_html_logs` 里目标语言的记录，替换对应缓存文件中的旧译文
/// （Go 版 `PluginSaveTranslateTextLog` 里的 goroutine，这里同步执行）。
async fn replace_html_caches(
    pool: &MySqlPool,
    cache_dir: &std::path::Path,
    to_language: &str,
    old_text: &str,
    new_text: &str,
) -> anyhow::Result<()> {
    if to_language.is_empty() || old_text.is_empty() {
        return Ok(());
    }
    let root = cache_dir.join("multiLang").join(to_language);
    let mut start_id: u32 = 0;
    loop {
        let rows = sqlx::query(
            "SELECT id, uri FROM translate_html_logs WHERE id > ? AND to_language = ? \
             ORDER BY id ASC LIMIT 100",
        )
        .bind(start_id)
        .bind(to_language)
        .fetch_all(pool)
        .await?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            start_id = row.get::<u32, _>("id");
            let uri: String = row.get::<Option<String>, _>("uri").unwrap_or_default();
            let cache_path = root.join(attachment::md5_hex(uri.as_bytes()));
            if !cache_path.is_file() {
                continue;
            }
            let Ok(buf) = std::fs::read(&cache_path) else {
                continue;
            };
            let (buf, replaced) = replace_translate_text(&buf, old_text, new_text);
            if replaced {
                let _ = std::fs::write(&cache_path, buf);
            }
        }
    }
    Ok(())
}

/// 对 HTML 字节做受控替换（Go `provider.ReplaceTranslateText`）：
/// 只替换标签文本（`>...<` 之间）与 `alt` / `title` / `content` 属性里的内容。
/// 返回 `(新内容, 是否命中过旧文本)`，命中与否以「字节里包含旧文本」为准（与 Go 一致）。
pub fn replace_translate_text(buf: &[u8], old_text: &str, new_text: &str) -> (Vec<u8>, bool) {
    if old_text.is_empty() {
        return (buf.to_vec(), false);
    }
    let Ok(text) = std::str::from_utf8(buf) else {
        return (buf.to_vec(), false);
    };
    if !text.contains(old_text) {
        return (buf.to_vec(), false);
    }

    let quoted = regex::escape(old_text);
    let patterns = [
        format!(r"(>[^<]*?){quoted}([^<]*<)"),
        format!(r#"(\balt\s*=\s*["'][^"']*?){quoted}([^"']*?["'])"#),
        format!(r#"(\btitle\s*=\s*["'][^"']*?){quoted}([^"']*?["'])"#),
        format!(r#"(\bcontent\s*=\s*["'][^"']*?){quoted}([^"']*?["'])"#),
    ];
    let mut out = text.to_string();
    for pattern in patterns {
        if let Ok(re) = regex::Regex::new(&pattern) {
            out = re
                .replace_all(&out, |caps: &regex::Captures| {
                    format!("{}{new_text}{}", &caps[1], &caps[2])
                })
                .into_owned();
        }
    }
    (out.into_bytes(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, setting, test_pool};

    #[test]
    fn replace_only_touches_text_and_attrs() {
        let html = b"<p>hello</p><img alt=\"hello\" src=\"hello.png\"><span>hello world</span>";
        let (out, replaced) = replace_translate_text(html, "hello", "你好");
        assert!(replaced);
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("<p>你好</p>"), "{out}");
        assert!(out.contains("alt=\"你好\""), "{out}");
        // src 属性不在替换范围内
        assert!(out.contains("src=\"hello.png\""), "{out}");
        // 标签之间的普通文本正常替换
        assert!(out.contains("<span>你好 world</span>"), "{out}");

        // 不包含时不做任何替换
        let (out, replaced) = replace_translate_text(b"<p>abc</p>", "hello", "你好");
        assert!(!replaced);
        assert_eq!(out, b"<p>abc</p>");

        // 空旧文本直接返回
        let (out, replaced) = replace_translate_text(b"<p>x</p>", "", "y");
        assert!(!replaced);
        assert_eq!(out, b"<p>x</p>");
    }

    #[tokio::test]
    async fn config_roundtrip_and_defaults() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, SETTING_KEY).await.unwrap();

        let loaded = config_get(&pool).await.unwrap();
        assert_eq!(loaded["engine"], json!(""), "默认配置补齐：{loaded}");
        assert_eq!(loaded["baidu_app_id"], json!(""));

        let saved = config_save(&pool, &json!({"engine": "baidu", "baidu_app_id": "id-1"}))
            .await
            .unwrap();
        assert_eq!(saved["engine"], json!("baidu"));
        assert_eq!(saved["baidu_app_id"], json!("id-1"));

        // 字段级 merge：只改 engine，app_id 保留
        let saved = config_save(&pool, &json!({"engine": "deepl"})).await.unwrap();
        assert_eq!(saved["engine"], json!("deepl"));
        assert_eq!(saved["baidu_app_id"], json!("id-1"));

        match backup {
            Some(raw) => setting::save_raw(&pool, SETTING_KEY, &raw).await.unwrap(),
            None => setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
    }

    #[tokio::test]
    async fn text_log_save_find_update_remove() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let cache_dir = std::env::temp_dir().join(format!("rustcms-tl-{}", std::process::id()));
        // 清掉本次测试残留
        sqlx::query("DELETE FROM translate_text_logs WHERE `text` LIKE '__rustcms_tl_%'")
            .execute(&pool)
            .await
            .unwrap();

        let input = TextLogInput {
            to_language: "en".into(),
            text: "__rustcms_tl_正文".into(),
            translated: "hello".into(),
            ..Default::default()
        };

        // 空 text 被拒
        assert!(text_log_save(&pool, &TextLogInput::default(), &cache_dir)
            .await
            .is_err());

        let saved = text_log_save(&pool, &input, &cache_dir).await.unwrap();
        assert!(saved["id"].as_u64().unwrap() > 0, "{saved}");
        assert!(saved["md5"].as_str().unwrap().len() == 32, "{saved}");
        // language 留空时取系统默认语言
        assert!(!saved["language"].as_str().unwrap().is_empty(), "{saved}");

        // 相同 (language, to_language, text) 再次保存 → 更新而不是新增
        let again = text_log_save(&pool, &input, &cache_dir).await.unwrap();
        assert_eq!(again["id"], saved["id"], "按 md5 命中同一条：{again}");
        assert_eq!(again["translated"], json!("hello"));

        // 按 id 更新译文
        let updated = text_log_save(
            &pool,
            &TextLogInput {
                id: saved["id"].as_u64().unwrap() as u32,
                text: "__rustcms_tl_正文".into(),
                translated: "hi there".into(),
                ..Default::default()
            },
            &cache_dir,
        )
        .await
        .unwrap();
        assert_eq!(updated["id"], saved["id"]);
        assert_eq!(updated["translated"], json!("hi there"));

        // 不存在的 id 走新增分支（与 Go 一致）
        let created = text_log_save(
            &pool,
            &TextLogInput {
                id: 9_999_999,
                text: "__rustcms_tl_不存在".into(),
                translated: "x".into(),
                ..Default::default()
            },
            &cache_dir,
        )
        .await
        .unwrap();
        assert_ne!(created["id"], json!(9_999_999), "不存在的 id 应新增：{created}");
        assert_ne!(created["id"], saved["id"]);

        // 列表 + 前缀筛选（正文 + 不存在 共 2 条不同记录）
        let (total, items) = text_log_list(&pool, 1, 20, "__rustcms_tl_", "").await.unwrap();
        assert!(total >= 2, "total={total}");
        assert!(items.iter().any(|item| item["text"] == json!("__rustcms_tl_正文")));
        let (miss_total, _) = text_log_list(&pool, 1, 20, "__rustcms_tl_", "no-such").await.unwrap();
        assert_eq!(miss_total, 0);

        // 删除：id 不存在要报错
        let err = text_log_remove(&pool, 9_999_999, false).await.unwrap_err();
        assert!(err.to_string().contains("不存在"), "{err}");
        // 不带 id 也不带 all 要报错
        let err = text_log_remove(&pool, 0, false).await.unwrap_err();
        assert!(err.to_string().contains("请提供"), "{err}");
        // 单删
        text_log_remove(&pool, created["id"].as_u64().unwrap() as u32, false)
            .await
            .unwrap();
        // 清理本用例数据与临时目录
        sqlx::query("DELETE FROM translate_text_logs WHERE `text` LIKE '__rustcms_tl_%'")
            .execute(&pool)
            .await
            .unwrap();
        let _ = std::fs::remove_dir_all(&cache_dir);
    }
}

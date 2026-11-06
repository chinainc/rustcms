//! 敏感词（`sensitiveWords.go`）。
//!
//! 配置是**字符串数组**，存 `settings.sensitive_words`，三种写法：
//! - `第一`：纯敏感词，命中后替换成等长的 `*`；
//! - `第一|很好`：命中后替换成 `很好`；
//! - `{\d{6,12}}`：花括号包住正则，命中后同样替换成等长 `*`（也支持 `{...}|替换词`）。

use std::time::Duration;

use regex::Regex;
use serde_json::Value;
use sqlx::mysql::MySqlPool;

use super::{anqi, site_setting, ModelError};

/// 敏感词配置键。
pub const SETTING_KEY: &str = "sensitive_words";

/// 云端服务地址（对齐 Go `provider.AnqiApi`）。
pub const ANQI_API: &str = "https://auth.anqicms.com/auth";

/// 一条规则。
#[derive(Debug, Clone)]
pub enum Rule {
    /// 纯文本敏感词
    Plain {
        word: String,
        replacement: Option<String>,
    },
    /// 正则表达式
    Pattern {
        source: String,
        regex: Regex,
        replacement: Option<String>,
    },
}

/// 解析规则列表（非法正则会被跳过，不影响其它词）。
pub fn parse(raw: &[String]) -> Vec<Rule> {
    let mut rules = Vec::new();
    for item in raw {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }

        // 花括号包起来的按正则处理
        // 注意用 rfind：量词 `{6,12}` 自身带 `}`，取第一个会把正则截断
        if let Some(rest) = item.strip_prefix('{') {
            let (pattern, tail) = match rest.rfind('}') {
                Some(index) => (&rest[..index], &rest[index + 1..]),
                None => continue,
            };
            if pattern.trim().is_empty() {
                continue;
            }
            let Ok(regex) = Regex::new(pattern) else {
                continue;
            };
            let replacement = tail
                .strip_prefix('|')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.to_string());
            rules.push(Rule::Pattern {
                source: format!("{{{pattern}}}"),
                regex,
                replacement,
            });
            continue;
        }

        // 竖线分隔替换词
        let (word, replacement) = match item.split_once('|') {
            Some((word, replacement)) => {
                let replacement = replacement.trim();
                (
                    word.trim(),
                    if replacement.is_empty() {
                        None
                    } else {
                        Some(replacement.to_string())
                    },
                )
            }
            None => (item, None),
        };
        if word.is_empty() {
            continue;
        }
        rules.push(Rule::Plain {
            word: word.to_string(),
            replacement,
        });
    }
    rules
}

/// 读原始敏感词数组。
pub async fn get(pool: &MySqlPool) -> anyhow::Result<Vec<String>> {
    Ok(site_setting::get_array(pool, SETTING_KEY)
        .await?
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|item| item.as_str().map(|text| text.to_string()))
                .collect()
        })
        .unwrap_or_default())
}

/// 保存敏感词（整份覆盖，必须是字符串数组）。
pub async fn save(pool: &MySqlPool, words: &[String]) -> Result<(), ModelError> {
    let value = serde_json::Value::Array(
        words
            .iter()
            .map(|word| serde_json::Value::String(word.clone()))
            .collect(),
    );
    site_setting::save_value(pool, SETTING_KEY, &value)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    Ok(())
}

/// 解析云端同步回包，返回敏感词列表（对齐 Go `AnqiSensitiveResult`）。
fn parse_sync_result(value: &Value) -> Result<Vec<String>, ModelError> {
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let message = value
            .get("msg")
            .and_then(Value::as_str)
            .filter(|msg| !msg.trim().is_empty())
            .unwrap_or("云端返回错误");
        return Err(ModelError::invalid(message.to_string()));
    }
    Ok(value
        .get("data")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|word| !word.is_empty())
                .map(|word| word.to_string())
                .collect()
        })
        .unwrap_or_default())
}

/// 从云端同步敏感词：成功且带回非空列表时整份覆盖本地配置（Go `AnqiSyncSensitiveWords`）。
pub async fn sync_from_cloud(pool: &MySqlPool) -> Result<usize, ModelError> {
    let auth = anqi::auth_info_get(pool).await?;
    let token = auth
        .get("token")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if token.is_empty() {
        return Err(ModelError::invalid("请先登录云端服务账号"));
    }

    let system = site_setting::get_section(pool, site_setting::SYSTEM).await?;
    let domain = system
        .get("base_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");

    let request = rustcms_ext::http::Request::post(format!("{ANQI_API}/sensitive/sync"))
        .with_header("token", token)
        .with_header("domain", domain)
        .with_header(
            "User-Agent",
            format!("rustcms/{}", env!("CARGO_PKG_VERSION")),
        )
        .with_json(serde_json::json!({}))
        .with_timeout(Duration::from_secs(300));

    let response = rustcms_ext::http::send(&request)
        .await
        .map_err(|err| ModelError::invalid(format!("连接云端服务失败: {err}")))?;
    let value: Value = response
        .json()
        .map_err(|err| ModelError::invalid(format!("云端返回数据无法解析: {err}")))?;
    let words = parse_sync_result(&value)?;

    if words.is_empty() {
        return Ok(0);
    }
    save(pool, &words).await?;
    Ok(words.len())
}

fn stars(count: usize) -> String {
    "*".repeat(count.max(1))
}

/// 把命中的敏感词替换掉（无替换词的用等长 `*`）。
pub fn mask(content: &str, rules: &[Rule]) -> String {
    let mut output = content.to_string();
    for rule in rules {
        match rule {
            Rule::Plain { word, replacement } => {
                if output.contains(word.as_str()) {
                    let target = replacement.clone().unwrap_or_else(|| stars(word.chars().count()));
                    output = output.replace(word.as_str(), &target);
                }
            }
            Rule::Pattern {
                regex, replacement, ..
            } => {
                match replacement {
                    Some(target) => {
                        output = regex.replace_all(&output, target.as_str()).to_string();
                    }
                    None => {
                        output = regex
                            .replace_all(&output, |caps: &regex::Captures| {
                                stars(caps[0].chars().count())
                            })
                            .to_string();
                    }
                }
            }
        }
    }
    output
}

/// 找出命中的敏感词（去重、保持规则顺序）。
pub fn hits(content: &str, rules: &[Rule]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for rule in rules {
        match rule {
            Rule::Plain { word, .. } => {
                if content.contains(word.as_str()) && !found.contains(word) {
                    found.push(word.clone());
                }
            }
            Rule::Pattern { source, regex, .. } => {
                if regex.is_match(content) && !found.contains(source) {
                    found.push(source.clone());
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn parses_three_modes() {
        let rules = parse(&[
            "第一".to_string(),
            " 第二 | 很好 ".to_string(),
            r"{\d{6,12}}".to_string(),
            "   ".to_string(),
            "{([unclosed}".to_string(),
            "|空词".to_string(),
        ]);
        assert_eq!(rules.len(), 3, "空词与非法正则应被跳过");

        match &rules[0] {
            Rule::Plain { word, replacement } => {
                assert_eq!(word, "第一");
                assert!(replacement.is_none());
            }
            _ => panic!("应为纯文本规则"),
        }
        match &rules[1] {
            Rule::Plain { word, replacement } => {
                assert_eq!(word, "第二");
                assert_eq!(replacement.as_deref(), Some("很好"));
            }
            _ => panic!("应为纯文本规则"),
        }
        match &rules[2] {
            Rule::Pattern { replacement, .. } => assert!(replacement.is_none()),
            _ => panic!("应为正则规则"),
        }
    }

    #[test]
    fn masks_plain_and_regex() {
        let rules = parse(&[
            "第一".to_string(),
            "第二|很好".to_string(),
            r"{\d{6,12}}".to_string(),
        ]);
        let masked = mask("第一 和 第二 以及 123456789", &rules);
        assert_eq!(masked, "** 和 很好 以及 *********");
    }

    #[test]
    fn finds_hits() {
        let rules = parse(&["第一".to_string(), r"{\d{6,12}}".to_string()]);
        let found = hits("第一 123456", &rules);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], "第一");
        assert_eq!(found[1], r"{\d{6,12}}");
        assert!(hits("无关内容", &rules).is_empty());
    }

    #[test]
    fn parses_cloud_sync_result() {
        let value = serde_json::json!({
            "code": 0,
            "data": [" 第一 ", "", "第二|很好"]
        });
        assert_eq!(
            parse_sync_result(&value).unwrap(),
            vec!["第一".to_string(), "第二|很好".to_string()]
        );

        let failed = serde_json::json!({"code": 1, "msg": "token 无效"});
        let err = parse_sync_result(&failed).unwrap_err();
        assert!(err.to_string().contains("token 无效"));

        let empty = serde_json::json!({"code": 0, "data": []});
        assert!(parse_sync_result(&empty).unwrap().is_empty());
    }

    #[tokio::test]
    async fn roundtrip_setting() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = crate::models::setting::get_raw(&pool, SETTING_KEY).await.unwrap();

        save(&pool, &["第一".to_string(), "第二|很好".to_string()])
            .await
            .unwrap();
        let loaded = get(&pool).await.unwrap();
        assert_eq!(loaded, vec!["第一".to_string(), "第二|很好".to_string()]);

        match backup {
            Some(raw) => crate::models::setting::save_raw(&pool, SETTING_KEY, &raw)
                .await
                .unwrap(),
            None => crate::models::setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
    }
}

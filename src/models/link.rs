//! 友情链接（表 `links`）。
//!
//! 对齐 Go 版 `controller/manageController/pluginLink.go`：
//! - 列表不分页，按 `sort ASC, id DESC`；
//! - 保存时 `status` **强制归零**（待回链检查），真正置为 1 要等 `PluginLinkCheck` 通过；
//! - 回链检查抓取对方页面，解析其中 `<a>` 标签，比对本站链接与标题。

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::{site_setting, ModelError};

/// 回链状态：正常。
pub const STATUS_OK: u8 = 1;
/// 回链状态：对方加了 `rel="nofollow"`。
pub const STATUS_NOFOLLOW: u8 = 2;
/// 回链状态：链接在，但标题不匹配。
pub const STATUS_NOT_TITLE: u8 = 3;
/// 回链状态：页面里没找到本站链接。
pub const STATUS_NOT_MATCH: u8 = 4;

/// 友情链接。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Link {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub title: String,
    pub link: String,
    pub back_link: String,
    pub my_title: String,
    pub my_link: String,
    pub contact: String,
    pub remark: String,
    /// 0 = 不添加 nofollow，1 = 添加
    pub nofollow: u8,
    pub sort: u32,
    /// 0 = 待审核，1 = 正常
    pub status: u8,
    pub checked_time: i64,
    pub logo: String,
}

/// 保存入参（对应 Go 的 `request.PluginLink`）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LinkInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub back_link: String,
    #[serde(default)]
    pub my_title: String,
    #[serde(default)]
    pub my_link: String,
    #[serde(default)]
    pub contact: String,
    #[serde(default)]
    pub remark: String,
    #[serde(default)]
    pub nofollow: u8,
    #[serde(default)]
    pub sort: Option<u32>,
    #[serde(default)]
    pub logo: String,
}

/// 全部友情链接。
pub async fn list(pool: &MySqlPool) -> anyhow::Result<Vec<Link>> {
    Ok(sqlx::query_as::<_, Link>(
        "SELECT * FROM links ORDER BY sort ASC, id DESC",
    )
    .fetch_all(pool)
    .await?)
}

/// 按 ID 取链接。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Link>> {
    Ok(sqlx::query_as::<_, Link>("SELECT * FROM links WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// 新建 / 更新友情链接（`status` 一律归零，等待回链检查）。
pub async fn save(pool: &MySqlPool, input: &LinkInput) -> Result<Link, ModelError> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err(ModelError::invalid("请填写链接名称"));
    }
    let link = input.link.trim();
    if link.is_empty() {
        return Err(ModelError::invalid("请填写链接地址"));
    }

    let now = chrono::Utc::now().timestamp();
    let sort = input.sort.unwrap_or(99);

    if input.id > 0 && get(pool, input.id).await?.is_some() {
        sqlx::query(
            "UPDATE links SET updated_time = ?, title = ?, `link` = ?, back_link = ?, \
                my_title = ?, my_link = ?, contact = ?, remark = ?, nofollow = ?, sort = ?, \
                logo = ?, status = 0 WHERE id = ?",
        )
        .bind(now)
        .bind(title)
        .bind(link)
        .bind(input.back_link.trim())
        .bind(&input.my_title)
        .bind(&input.my_link)
        .bind(&input.contact)
        .bind(&input.remark)
        .bind(input.nofollow)
        .bind(sort)
        .bind(&input.logo)
        .bind(input.id)
        .execute(pool)
        .await?;
        return get(pool, input.id)
            .await?
            .ok_or_else(|| ModelError::invalid("链接不存在"));
    }

    let result = sqlx::query(
        "INSERT INTO links (created_time, updated_time, title, `link`, back_link, my_title, \
            my_link, contact, remark, nofollow, sort, status, checked_time, logo) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?)",
    )
    .bind(now)
    .bind(now)
    .bind(title)
    .bind(link)
    .bind(input.back_link.trim())
    .bind(&input.my_title)
    .bind(&input.my_link)
    .bind(&input.contact)
    .bind(&input.remark)
    .bind(input.nofollow)
    .bind(sort)
    .bind(&input.logo)
    .execute(pool)
    .await?;
    let id = result.last_insert_id() as u32;
    get(pool, id)
        .await?
        .ok_or_else(|| ModelError::invalid("链接创建失败"))
}

/// 删除链接，返回是否真的删掉。
pub async fn delete(pool: &MySqlPool, id: u32) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM links WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

fn anchor_open_regex() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"(?is)<a\b[^>]*>").expect("a 标签正则应合法"))
}

fn anchor_close_regex() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"(?is)</a\s*>").expect("a 标签闭合正则应合法"))
}

fn href_regex() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r#"(?is)\bhref\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#)
            .expect("href 正则应合法")
    })
}

fn rel_regex() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r#"(?is)\brel\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#)
            .expect("rel 正则应合法")
    })
}

fn tag_regex() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"(?is)<[^>]*>").expect("标签正则应合法"))
}

/// 取属性值（三个捕获组分别对应双引号 / 单引号 / 无引号写法）。
fn attr_value(tag: &str, regex: &Regex) -> Option<String> {
    let caps = regex.captures(tag)?;
    let value = caps
        .get(1)
        .or_else(|| caps.get(2))
        .or_else(|| caps.get(3))?
        .as_str();
    Some(value.trim().to_string())
}

/// 粗略解码常见 HTML 实体（用于标题比对）。
fn decode_entities(raw: &str) -> String {
    raw.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#039;", "'")
}

/// 遍历页面里的 `<a>` 标签，返回 `(href, rel, text)`。
fn anchors(html: &str) -> Vec<(String, Option<String>, String)> {
    let mut list = Vec::new();
    let open_re = anchor_open_regex();
    let close_re = anchor_close_regex();
    let href_re = href_regex();
    let rel_re = rel_regex();
    let tag_re = tag_regex();

    for matched in open_re.find_iter(html) {
        let tag = matched.as_str();
        let Some(href) = attr_value(tag, href_re) else {
            continue;
        };
        let rel = attr_value(tag, rel_re);
        let text = match close_re.find_at(html, matched.end()) {
            Some(close) => tag_re.replace_all(&html[matched.end()..close.start()], ""),
            None => std::borrow::Cow::Borrowed(""),
        };
        let text = decode_entities(text.trim());
        list.push((href, rel, text));
    }
    list
}

/// 链接是否指向本站（忽略末尾斜杠差异）。
fn href_matches(href: &str, my_link: &str) -> bool {
    let href = href.trim().trim_end_matches('/');
    let mine = my_link.trim().trim_end_matches('/');
    !mine.is_empty() && href == mine
}

/// 根据页面内容判定回链状态（对齐 Go `PluginLinkCheck`）。
fn evaluate(html: &str, my_link: &str, my_title: &str) -> u8 {
    for (href, rel, text) in anchors(html) {
        if !href_matches(&href, my_link) {
            continue;
        }
        let mut status = STATUS_OK;
        if !my_title.is_empty() && text.trim() != my_title {
            status = STATUS_NOT_TITLE;
        }
        if rel
            .as_deref()
            .map(|value| value.eq_ignore_ascii_case("nofollow"))
            .unwrap_or(false)
        {
            status = STATUS_NOFOLLOW;
        }
        return status;
    }
    STATUS_NOT_MATCH
}

/// 本站链接：优先用链接配置的 `my_link`，未填时退回站点前台地址。
async fn site_link(pool: &MySqlPool, configured: &str) -> String {
    let configured = configured.trim();
    if !configured.is_empty() {
        return configured.to_string();
    }
    let system = site_setting::get_section(pool, site_setting::SYSTEM)
        .await
        .unwrap_or_else(|_| serde_json::json!({}));
    let front = system
        .get("front_url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            system
                .get("base_url")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
        })
        .unwrap_or("");
    front.trim_end_matches('/').to_string()
}

/// 抓取对方页面、判定回链状态并落库。
pub async fn check(pool: &MySqlPool, id: u32) -> Result<Link, ModelError> {
    let mut link = get(pool, id)
        .await?
        .ok_or_else(|| ModelError::invalid("链接不存在"))?;

    // 优先抓回链地址，未填时退回链接本身（Go `PluginLinkCheck`）
    let remote = if link.back_link.trim().is_empty() {
        link.link.trim()
    } else {
        link.back_link.trim()
    };
    if remote.is_empty() {
        return Err(ModelError::invalid("链接地址为空"));
    }

    let html = rustcms_ext::http::get_text(remote)
        .await
        .map_err(|err| ModelError::invalid(format!("抓取对方页面失败: {err}")))?;

    let my_link = site_link(pool, &link.my_link).await;
    let status = evaluate(&html, &my_link, &link.my_title);
    let checked_time = chrono::Utc::now().timestamp();

    sqlx::query("UPDATE links SET status = ?, checked_time = ? WHERE id = ?")
        .bind(status)
        .bind(checked_time)
        .bind(id)
        .execute(pool)
        .await?;

    link.status = status;
    link.checked_time = checked_time;
    Ok(link)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn parses_anchors_and_attributes() {
        let html = r#"
            <a href="https://example.com" rel="nofollow">示例</a>
            <a class="x" target="_blank" href='https://a.com/x'>A</a>
            <a data-x="1" href=https://b.com>B</a>
            <a name="anchor">无链接</a>
        "#;
        let list = anchors(html);
        assert_eq!(list.len(), 3, "没有 href 的 a 标签应被跳过");
        assert_eq!(list[0].0, "https://example.com");
        assert_eq!(list[0].1.as_deref(), Some("nofollow"));
        assert_eq!(list[0].2, "示例");
        assert_eq!(list[1].0, "https://a.com/x");
        assert_eq!(list[1].2, "A");
        assert_eq!(list[2].0, "https://b.com");
    }

    #[test]
    fn evaluates_back_link_status() {
        let my_link = "https://rustcms.example";
        let ok = r#"<a href="https://rustcms.example/">站</a>"#;
        assert_eq!(evaluate(ok, my_link, ""), STATUS_OK);
        assert_eq!(evaluate(ok, my_link, "站点名"), STATUS_NOT_TITLE);
        assert_eq!(
            evaluate(r#"<a href="https://rustcms.example" rel="nofollow">站</a>"#, my_link, ""),
            STATUS_NOFOLLOW
        );
        assert_eq!(
            evaluate(r#"<a href="https://other.example">别的站</a>"#, my_link, ""),
            STATUS_NOT_MATCH
        );
        assert_eq!(evaluate("<p>没有链接</p>", my_link, ""), STATUS_NOT_MATCH);
    }

    #[tokio::test]
    async fn link_crud_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let _ = sqlx::query("DELETE FROM links WHERE title = 'rustcms-链接测试'")
            .execute(&pool)
            .await;

        // 必填校验
        assert_eq!(
            save(&pool, &LinkInput::default())
                .await
                .unwrap_err()
                .to_string(),
            "请填写链接名称"
        );

        let created = save(
            &pool,
            &LinkInput {
                title: "rustcms-链接测试".into(),
                link: "https://example.com".into(),
                back_link: "https://example.com/links".into(),
                sort: Some(5),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(created.id > 0);
        // 保存后状态一律归零
        assert_eq!(created.status, 0);
        assert_eq!(created.sort, 5);

        // 列表里能看到
        let list = list(&pool).await.unwrap();
        assert!(list.iter().any(|item| item.id == created.id));

        // 更新
        let updated = save(
            &pool,
            &LinkInput {
                id: created.id,
                title: "rustcms-链接测试（改）".into(),
                link: "https://example.org".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.title, "rustcms-链接测试（改）");
        assert_eq!(updated.link, "https://example.org");

        // 删除
        assert!(delete(&pool, created.id).await.unwrap());
        assert!(get(&pool, created.id).await.unwrap().is_none());
        assert!(!delete(&pool, created.id).await.unwrap());
    }
}

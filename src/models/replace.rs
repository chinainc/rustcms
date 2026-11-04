//! 全站内容替换（`pluginReplace.go`）。
//!
//! Go 版按「位置（places）」逐表批量替换，支持两组语义：
//! - `replace_tag = true`：连 HTML 标签内的属性一起替换（纯文本替换）；
//! - `replace_tag = false`：只替换**可见文本**，跳过 `<...>` 里的内容。
//!
//! 这里对正文字段用 Rust 侧的扫描器实现（尊重 `replace_tag`），
//! 其余简单字段直接走 SQL `REPLACE()`。

use sqlx::mysql::MySqlPool;

/// 每个「位置」可替换的普通文本列（正文字段另行处理）。
const PLACES: &[(&str, &[&str])] = &[
    ("archive", &["title", "keywords", "description"]),
    (
        "category",
        &["title", "seo_title", "keywords", "description"],
    ),
    ("tag", &["title", "seo_title", "keywords", "description"]),
    ("anchor", &["title", "link"]),
    ("keyword", &["title"]),
    ("nav", &["title", "link"]),
    ("link", &["title", "link", "my_title", "my_link"]),
    ("redirect", &["from_url", "to_url"]),
    ("attachment", &["file_name"]),
    ("place", &["title"]),
    ("guestbook", &["user_name", "content"]),
    ("comment", &["user_name", "content"]),
];

/// 把 `from` 替换成 `to`，但跳过 `<...>` 标签内部。
///
/// 返回 `(新文本, 替换次数)`。
pub fn replace_outside_tags(content: &str, from: &str, to: &str) -> (String, usize) {
    if from.is_empty() {
        return (content.to_string(), 0);
    }

    let mut out = String::with_capacity(content.len());
    let mut buffer = String::new();
    let mut count = 0usize;
    let mut in_tag = false;

    for ch in content.chars() {
        if in_tag {
            out.push(ch);
            if ch == '>' {
                in_tag = false;
            }
            continue;
        }
        if ch == '<' {
            count += push_replaced(&buffer, from, to, &mut out);
            buffer.clear();
            in_tag = true;
            out.push(ch);
            continue;
        }
        buffer.push(ch);
    }
    count += push_replaced(&buffer, from, to, &mut out);
    (out, count)
}

fn push_replaced(buffer: &str, from: &str, to: &str, out: &mut String) -> usize {
    let mut count = 0usize;
    let mut last = 0usize;
    for (index, _) in buffer.match_indices(from) {
        out.push_str(&buffer[last..index]);
        out.push_str(to);
        last = index + from.len();
        count += 1;
    }
    out.push_str(&buffer[last..]);
    count
}

/// 找出所有 `<a ...>...</a>` 的字节区间（只比较 ASCII，UTF-8 安全）。
fn anchor_ranges(content: &str) -> Vec<(usize, usize)> {
    let bytes = content.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0usize;

    while index + 1 < bytes.len() {
        let is_open = bytes[index] == b'<'
            && (bytes[index + 1] | 0x20) == b'a'
            && (index + 2 >= bytes.len()
                || matches!(bytes[index + 2], b' ' | b'>' | b'\n' | b'\r' | b'\t'));
        if !is_open {
            index += 1;
            continue;
        }

        // 找配对的 </a>
        let mut cursor = index;
        let mut end = bytes.len();
        while cursor + 3 < bytes.len() {
            if bytes[cursor] == b'<'
                && bytes[cursor + 1] == b'/'
                && (bytes[cursor + 2] | 0x20) == b'a'
                && bytes[cursor + 3] == b'>'
            {
                end = cursor + 4;
                break;
            }
            cursor += 1;
        }
        ranges.push((index, end));
        index = end;
    }
    ranges
}

/// 把正文里**未被链接**的 `title` 包成 `<a href="href">title</a>`。
///
/// 最多替换 `max` 处（`max = 0` 视为不限），返回 `(新正文, 替换次数)`。
pub fn linkify(content: &str, title: &str, href: &str, max: usize) -> (String, usize) {
    if title.trim().is_empty() {
        return (content.to_string(), 0);
    }
    let ranges = anchor_ranges(content);
    let mut out = String::with_capacity(content.len() + 64);
    let mut last = 0usize;
    let mut count = 0usize;

    for (index, _) in content.match_indices(title) {
        if max > 0 && count >= max {
            break;
        }
        if ranges
            .iter()
            .any(|(start, end)| index >= *start && index < *end)
        {
            continue;
        }
        out.push_str(&content[last..index]);
        out.push_str(&format!("<a href=\"{href}\" title=\"{title}\">{title}</a>"));
        last = index + title.len();
        count += 1;
    }
    if count == 0 {
        return (content.to_string(), 0);
    }
    out.push_str(&content[last..]);
    (out, count)
}

/// 执行全站替换，返回受影响的行数合计。
pub async fn replace_values(
    pool: &MySqlPool,
    places: &[String],
    keywords: &[(String, String)],
    replace_tag: bool,
) -> anyhow::Result<usize> {
    let pairs: Vec<(&str, &str)> = keywords
        .iter()
        .filter(|(from, _)| !from.trim().is_empty())
        .map(|(from, to)| (from.as_str(), to.as_str()))
        .collect();
    if pairs.is_empty() {
        return Ok(0);
    }

    let wanted: std::collections::HashSet<&str> =
        places.iter().map(|item| item.trim()).collect();
    let mut total = 0usize;

    for (place, columns) in PLACES {
        if !wanted.contains(place) {
            continue;
        }
        for (from, to) in &pairs {
            for column in *columns {
                let sql = format!("UPDATE `{place}` SET `{column}` = REPLACE(`{column}`, ?, ?)");
                // 表/列不存在时忽略（不同站点装过的插件不一样）
                if let Ok(result) = sqlx::query(&sql).bind(*from).bind(*to).execute(pool).await {
                    total += result.rows_affected() as usize;
                }
            }
        }
    }

    // 正文：archives 走 archive_data.content，标签走 tag_contents.content
    if wanted.contains("archive") {
        total += replace_content_table(pool, "archive_data", "id", &pairs, replace_tag).await?;
    }
    if wanted.contains("tag") {
        total += replace_content_table(pool, "tag_contents", "id", &pairs, replace_tag).await?;
    }

    // settings：整份 JSON 值替换
    if wanted.contains("setting") {
        let rows: Vec<(String, Option<String>)> = sqlx::query_as("SELECT `key`, `value` FROM settings")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
        for (key, value) in rows {
            let Some(value) = value else { continue };
            let mut current = value.clone();
            for (from, to) in &pairs {
                if current.contains(from) {
                    current = current.replace(from, to);
                }
            }
            if current != value {
                let _ = sqlx::query("UPDATE settings SET `value` = ? WHERE `key` = ?")
                    .bind(&current)
                    .bind(&key)
                    .execute(pool)
                    .await;
                total += 1;
            }
        }
    }

    Ok(total)
}

/// 正文字段替换（尊重 `replace_tag`）。
async fn replace_content_table(
    pool: &MySqlPool,
    table: &str,
    id_column: &str,
    pairs: &[(&str, &str)],
    replace_tag: bool,
) -> anyhow::Result<usize> {
    // 注意：`archive_data.id` 是 `bigint unsigned`、`tag_contents.id` 是 `int unsigned`，
    // 直接解码成 i64 会失败（sqlx 按无符号处理），所以用 CAST 成字符串再解析。
    let sql = format!(
        "SELECT CAST(`{id_column}` AS CHAR), `content` FROM `{table}` WHERE `content` IS NOT NULL"
    );
    let rows: Vec<(String, Option<String>)> = match sqlx::query_as(&sql).fetch_all(pool).await {
        Ok(rows) => rows,
        // 表不存在就跳过
        Err(_) => return Ok(0),
    };

    let mut total = 0usize;
    for (raw_id, content) in rows {
        let Ok(id) = raw_id.parse::<i64>() else {
            continue;
        };
        let Some(content) = content else { continue };
        if !pairs.iter().any(|(from, _)| content.contains(from)) {
            continue;
        }

        let mut updated = content.clone();
        for (from, to) in pairs {
            updated = if replace_tag {
                updated.replace(from, to)
            } else {
                replace_outside_tags(&updated, from, to).0
            };
        }
        if updated == content {
            continue;
        }

        let sql = format!("UPDATE `{table}` SET `content` = ? WHERE `{id_column}` = ?");
        let _ = sqlx::query(&sql)
            .bind(&updated)
            .bind(id)
            .execute(pool)
            .await;
        total += 1;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_text_but_skips_tags() {
        let html = r#"<p>旧词在这里</p><img alt="旧词">"#;
        let (out, count) = replace_outside_tags(html, "旧词", "新词");
        assert_eq!(out, r#"<p>新词在这里</p><img alt="旧词">"#);
        assert_eq!(count, 1);
    }

    #[test]
    fn handles_empty_pattern() {
        let (out, count) = replace_outside_tags("abc", "", "x");
        assert_eq!(out, "abc");
        assert_eq!(count, 0);
    }

    #[test]
    fn linkifies_only_first_n_outside_links() {
        let html = r#"<p>示例站点 很好，示例站点 很快，<a href="/x">示例站点</a></p>"#;
        let (out, count) = linkify(html, "示例站点", "/about.html", 2);
        assert_eq!(count, 2, "{out}");
        assert!(out.contains(r#"<a href="/about.html" title="示例站点">示例站点</a> 很好"#), "{out}");
        // 已有链接里的同名文本不应被再次包裹
        assert!(out.contains(r#"<a href="/x">示例站点</a>"#), "{out}");
    }

    #[test]
    fn linkify_respects_max() {
        let html = "aa aa aa";
        let (out, count) = linkify(html, "aa", "/l", 1);
        assert_eq!(count, 1);
        assert_eq!(out, r#"<a href="/l" title="aa">aa</a> aa aa"#);
    }

    #[test]
    fn detects_anchor_ranges() {
        let html = r#"<p>x</p><a href="/1">y</a><a class="z">w</a>"#;
        let ranges = anchor_ranges(html);
        assert_eq!(ranges.len(), 2);
        assert_eq!(&html[ranges[0].0..ranges[0].1], r#"<a href="/1">y</a>"#);
    }

    #[tokio::test]
    async fn replaces_across_tables() {
        use crate::models::{db_lock, test_pool};
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };

        // 造一条临时内容（走 archive_write 太麻烦，直接插最小行）
        let _ = sqlx::query("DELETE FROM archives WHERE title = 'rustcms-replace-test'")
            .execute(&pool)
            .await;
        let category_id: u32 = sqlx::query_scalar(
            "SELECT id FROM categories WHERE `type` = 1 ORDER BY id LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let module_id: u32 =
            sqlx::query_scalar("SELECT id FROM modules ORDER BY id LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        let now = chrono::Utc::now().timestamp();
        let result = sqlx::query(
            "INSERT INTO archives (created_time, updated_time, title, module_id, category_id, \
                url_token, status) VALUES (?, ?, 'rustcms-replace-test', ?, ?, ?, 1)",
        )
        .bind(now)
        .bind(now)
        .bind(module_id)
        .bind(category_id)
        .bind(format!("rustcms-replace-{now}"))
        .execute(&pool)
        .await;
        // 该表没有 status 列时退回最小插入
        let id = match result {
            Ok(value) => value.last_insert_id() as i64,
            Err(_) => sqlx::query(
                "INSERT INTO archives (created_time, updated_time, title, module_id, category_id, \
                    url_token) VALUES (?, ?, 'rustcms-replace-test', ?, ?, ?)",
            )
            .bind(now)
            .bind(now)
            .bind(module_id)
            .bind(category_id)
            .bind(format!("rustcms-replace-{now}"))
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_id() as i64,
        };

        let _ = sqlx::query("DELETE FROM archive_data WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await;
        sqlx::query("INSERT INTO archive_data (id, content) VALUES (?, ?)")
            .bind(id)
            .bind(r#"<p>旧词正文</p><img alt="旧词">"#)
            .execute(&pool)
            .await
            .unwrap();

        // replace_tag = false：只改可见文本
        let count = replace_values(
            &pool,
            &["archive".to_string()],
            &[("旧词".to_string(), "新词".to_string())],
            false,
        )
        .await
        .unwrap();
        assert!(count >= 1);

        let content: String = sqlx::query_scalar("SELECT content FROM archive_data WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(content, r#"<p>新词正文</p><img alt="旧词">"#);

        let title: String = sqlx::query_scalar("SELECT title FROM archives WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(title, "rustcms-replace-test");

        // 收尾
        let _ = sqlx::query("DELETE FROM archive_data WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM archives WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await;
    }
}

//! URL 规则（伪静态）与路径匹配。
//!
//! 对齐 Go 版 `provider/rewrite.go`：`settings.rewrite` 的 `mode` 决定内置规则，
//! 0=ID 模式、1=模型+别名、2=栏目别名+ID、3=栏目别名+内容别名、4=自定义正则。
//!
//! 生成与解析共用同一张规则表：`(_{page})` 这类圆括号段表示「可选」，
//! 第 1 页时整段去掉（`/zixun/`），第 2 页起展开成 `_2`（`/zixun/11_2.html`）。

use std::collections::HashMap;

use regex::Regex;
use serde_json::Value;
use sqlx::mysql::MySqlPool;

/// 伪静态模式：ID 模式
pub const MODE_NUMBER: i32 = 0;
/// 伪静态模式：模型 + URL 别名
pub const MODE_STRING1: i32 = 1;
/// 伪静态模式：URL 别名 + ID
pub const MODE_STRING2: i32 = 2;
/// 伪静态模式：分类 + URL 别名
pub const MODE_STRING3: i32 = 3;
/// 伪静态模式：自定义正则
pub const MODE_PATTERN: i32 = 4;

/// 规则种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Archive,
    Category,
    Page,
    ArchiveIndex,
    TagIndex,
    Tag,
    /// 搜索页（Go 默认 `/search(/{module})`）
    Search,
    /// 用户列表（Go 默认 `/peoples(/{page})`）
    PeopleIndex,
    /// 用户主页（Go 默认 `/people/{id}.html`）
    People,
}

const NUMBER_PATTERNS: &[(Kind, &str)] = &[
    (Kind::Archive, "/{module}/{id}(_{page}).html"),
    (Kind::Category, "/{module}/{catid}(/{page})"),
    (Kind::Page, "/{id}.html"),
    (Kind::ArchiveIndex, "/{module}(_{page})"),
    (Kind::TagIndex, "/tags(/{page})"),
    (Kind::Tag, "/tag/{id}(/{page})"),
    // Go 版 GetRewritePattern 对所有模式统一补的默认规则
    (Kind::Search, "/search(/{module})"),
    (Kind::PeopleIndex, "/peoples(/{page})"),
    (Kind::People, "/people/{id}.html"),
];

const STRING1_PATTERNS: &[(Kind, &str)] = &[
    (Kind::Archive, "/{module}/{filename}(_{page}).html"),
    (Kind::Category, "/{module}/{catname}(/{page})"),
    (Kind::Page, "/{filename}.html"),
    (Kind::ArchiveIndex, "/{module}(_{page})"),
    (Kind::TagIndex, "/tags(/{page})"),
    (Kind::Tag, "/tag/{filename}(/{page})"),
    (Kind::Search, "/search(/{module})"),
    (Kind::PeopleIndex, "/peoples(/{page})"),
    (Kind::People, "/people/{id}.html"),
];

const STRING2_PATTERNS: &[(Kind, &str)] = &[
    (Kind::Archive, "/{catname}/{id}(_{page}).html"),
    (Kind::Category, "/{catname}(/{page})"),
    (Kind::Page, "/{filename}.html"),
    (Kind::ArchiveIndex, "/{module}(_{page})"),
    (Kind::TagIndex, "/tags(/{page})"),
    (Kind::Tag, "/tag/{id}(/{page})"),
    (Kind::Search, "/search(/{module})"),
    (Kind::PeopleIndex, "/peoples(/{page})"),
    (Kind::People, "/people/{id}.html"),
];

const STRING3_PATTERNS: &[(Kind, &str)] = &[
    (Kind::Archive, "/{catname}/{filename}(_{page}).html"),
    (Kind::Category, "/{catname}(/{page})"),
    (Kind::Page, "/{filename}.html"),
    (Kind::ArchiveIndex, "/{module}(_{page})"),
    (Kind::TagIndex, "/tags(/{page})"),
    (Kind::Tag, "/tag/{filename}(/{page})"),
    (Kind::Search, "/search(/{module})"),
    (Kind::PeopleIndex, "/peoples(/{page})"),
    (Kind::People, "/people/{id}.html"),
];

/// 某一模式下的规则表。
pub fn patterns(mode: i32) -> &'static [(Kind, &'static str)] {
    match mode {
        MODE_NUMBER => NUMBER_PATTERNS,
        MODE_STRING1 => STRING1_PATTERNS,
        MODE_STRING3 => STRING3_PATTERNS,
        // 2 与自定义正则都回落 STRING2（自定义正则暂不解析生成，只用于匹配）
        _ => STRING2_PATTERNS,
    }
}

/// 取某类规则的模板串。
pub fn pattern(mode: i32, kind: Kind) -> &'static str {
    patterns(mode)
        .iter()
        .find(|(item, _)| *item == kind)
        .map(|(_, pattern)| *pattern)
        .unwrap_or("")
}

/// 从数据库读伪静态配置，返回 mode。
pub async fn load_mode(pool: &MySqlPool) -> anyhow::Result<i32> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE `key` = 'rewrite'")
            .fetch_optional(pool)
            .await?;
    let Some(raw) = raw else {
        return Ok(MODE_NUMBER);
    };
    let value: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    Ok(value
        .get("mode")
        .and_then(|mode| mode.as_i64())
        .unwrap_or(MODE_NUMBER as i64) as i32)
}

/// 去掉 `( ... )` 可选段（支持嵌套一层）。
fn strip_optional(pattern: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for ch in pattern.chars() {
        match ch {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            _ => out.push(ch),
        }
    }
    out
}

fn fill(pattern: &str, values: &[(&str, String)], page: u32) -> String {
    let mut out = if page <= 1 {
        strip_optional(pattern)
    } else {
        pattern.replace(['(', ')'], "")
    };
    for (key, value) in values {
        out = out.replace(&format!("{{{key}}}"), value);
    }
    out = out.replace("{page}", &page.to_string());
    if !out.starts_with('/') {
        out.insert(0, '/');
    }
    out
}

/// 列表类地址在第 1 页统一带尾斜杠（`/zixun/`、`/news/`），与实际线上地址一致。
fn with_trailing_slash(url: String, page: u32) -> String {
    if page <= 1 && !url.ends_with('/') {
        format!("{url}/")
    } else {
        url
    }
}

/// 生成内容详情页地址。
#[allow(clippy::too_many_arguments)]
pub fn archive_url(
    mode: i32,
    catname: &str,
    module_token: &str,
    id: i64,
    filename: &str,
    catid: u32,
    page: u32,
) -> String {
    let values = [
        ("catname", catname.to_string()),
        ("module", module_token.to_string()),
        ("id", id.to_string()),
        ("catid", catid.to_string()),
        ("filename", filename.to_string()),
    ];
    fill(pattern(mode, Kind::Archive), &values, page)
}

/// 生成栏目列表页地址。
pub fn category_url(mode: i32, catname: &str, module_token: &str, catid: u32, page: u32) -> String {
    let values = [
        ("catname", catname.to_string()),
        ("module", module_token.to_string()),
        ("catid", catid.to_string()),
    ];
    with_trailing_slash(fill(pattern(mode, Kind::Category), &values, page), page)
}

/// 生成模块列表页地址（`/news/`）。
pub fn archive_index_url(mode: i32, module_token: &str, page: u32) -> String {
    let values = [("module", module_token.to_string())];
    with_trailing_slash(fill(pattern(mode, Kind::ArchiveIndex), &values, page), page)
}

/// 单页地址。
pub fn page_url(mode: i32, filename: &str, page: u32) -> String {
    let values = [("filename", filename.to_string())];
    fill(pattern(mode, Kind::Page), &values, page)
}

/// 标签详情页地址（字符串模式用 `{filename}`，其余用 `{id}`）。
pub fn tag_url(mode: i32, id: u32, filename: &str, page: u32) -> String {
    let values = [
        ("id", id.to_string()),
        ("filename", filename.to_string()),
    ];
    fill(pattern(mode, Kind::Tag), &values, page)
}

/// 标签列表页地址（`/tags/`）。
pub fn tag_index_url(mode: i32, page: u32) -> String {
    with_trailing_slash(fill(pattern(mode, Kind::TagIndex), &[], page), page)
}

fn to_regex(pattern: &str) -> (String, Vec<String>) {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::from("^");
    let mut tags = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '{' {
            if let Some(offset) = chars[index..].iter().position(|item| *item == '}') {
                let tag: String = chars[index + 1..index + offset].iter().collect();
                let group = match tag.as_str() {
                    "id" | "catid" | "page" => "([0-9]+)",
                    "year" => "([0-9]{4})",
                    "month" | "day" | "hour" | "minute" | "second" => "([0-9]{2})",
                    _ => "([^/]+?)",
                };
                out.push_str(group);
                tags.push(tag);
                index += offset + 1;
                continue;
            }
        }
        match ch {
            '(' => out.push_str("(?:"),
            ')' => out.push_str(")?"),
            '.' => out.push_str("\\."),
            '/' | '-' | '_' => out.push(ch),
            other if other.is_ascii_alphanumeric() => out.push(other),
            other => {
                out.push('\\');
                out.push(other);
            }
        }
        index += 1;
    }
    // 列表类地址允许省略尾斜杠
    out.push_str("/?$");
    (out, tags)
}

/// 用某个模式的规则去匹配请求路径，返回占位符取值。
pub fn match_path(mode: i32, kind: Kind, path: &str) -> Option<HashMap<String, String>> {
    let pattern = pattern(mode, kind);
    if pattern.is_empty() {
        return None;
    }
    let (regex_text, tags) = to_regex(pattern);
    let regex = Regex::new(&regex_text).ok()?;
    let captures = regex.captures(path)?;
    let mut result = HashMap::new();
    for (index, tag) in tags.iter().enumerate() {
        if let Some(value) = captures.get(index + 1) {
            result.insert(tag.clone(), value.as_str().to_string());
        }
    }
    Some(result)
}

/// 判断是否为首页。
pub fn is_home(path: &str) -> bool {
    path == "/" || path.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_urls_per_mode() {
        // mode 2：栏目别名 + ID
        assert_eq!(
            category_url(MODE_STRING2, "zixun", "news", 13, 1),
            "/zixun/"
        );
        assert_eq!(
            category_url(MODE_STRING2, "zixun", "news", 13, 2),
            "/zixun/2"
        );
        assert_eq!(
            archive_url(MODE_STRING2, "zixun", "news", 11, "a", 13, 1),
            "/zixun/11.html"
        );
        assert_eq!(
            archive_url(MODE_STRING2, "zixun", "news", 11, "a", 13, 2),
            "/zixun/11_2.html"
        );

        // mode 0：ID 模式（模型 + ID）
        assert_eq!(
            archive_url(MODE_NUMBER, "zixun", "news", 11, "a", 13, 1),
            "/news/11.html"
        );
        assert_eq!(
            category_url(MODE_NUMBER, "zixun", "news", 13, 1),
            "/news/13/"
        );

        // mode 1：模型 + 内容别名
        assert_eq!(
            archive_url(MODE_STRING1, "zixun", "news", 11, "my-post", 13, 1),
            "/news/my-post.html"
        );

        // mode 3：栏目别名 + 内容别名
        assert_eq!(
            archive_url(MODE_STRING3, "zixun", "news", 11, "my-post", 13, 1),
            "/zixun/my-post.html"
        );

        // 模块列表页与单页
        assert_eq!(archive_index_url(MODE_STRING2, "news", 1), "/news/");
        assert_eq!(archive_index_url(MODE_STRING2, "news", 2), "/news_2");
        assert_eq!(page_url(MODE_STRING2, "about", 1), "/about.html");

        // 标签页
        assert_eq!(tag_index_url(MODE_STRING2, 1), "/tags/");
        assert_eq!(tag_index_url(MODE_STRING2, 2), "/tags/2");
        assert_eq!(tag_url(MODE_STRING2, 5, "hot", 1), "/tag/5");
        assert_eq!(tag_url(MODE_STRING2, 5, "hot", 2), "/tag/5/2");
        assert_eq!(tag_url(MODE_STRING1, 5, "hot", 1), "/tag/hot");
    }

    #[test]
    fn matches_paths() {
        let cat = match_path(MODE_STRING2, Kind::Category, "/zixun/").unwrap();
        assert_eq!(cat["catname"], "zixun");

        let cat_page = match_path(MODE_STRING2, Kind::Category, "/zixun/3").unwrap();
        assert_eq!(cat_page["catname"], "zixun");
        assert_eq!(cat_page["page"], "3");

        let archive = match_path(MODE_STRING2, Kind::Archive, "/zixun/11.html").unwrap();
        assert_eq!(archive["catname"], "zixun");
        assert_eq!(archive["id"], "11");

        let archive_page = match_path(MODE_STRING2, Kind::Archive, "/zixun/11_2.html").unwrap();
        assert_eq!(archive_page["id"], "11");
        assert_eq!(archive_page["page"], "2");

        // mode 0 的 /news/11.html
        let num = match_path(MODE_NUMBER, Kind::Archive, "/news/11.html").unwrap();
        assert_eq!(num["module"], "news");
        assert_eq!(num["id"], "11");

        // 不匹配的路径
        assert!(match_path(MODE_STRING2, Kind::Archive, "/zixun/").is_none());
        assert!(match_path(MODE_STRING2, Kind::Category, "/zixun/11.html").is_none());

        // 搜索页与用户页（Go 默认规则）
        let search = match_path(MODE_STRING2, Kind::Search, "/search/").unwrap();
        assert_eq!(search.len(), 0);
        let search2 = match_path(MODE_STRING2, Kind::Search, "/search/article").unwrap();
        assert_eq!(search2["module"], "article");
        let people = match_path(MODE_STRING2, Kind::People, "/people/9.html").unwrap();
        assert_eq!(people["id"], "9");
        let peoples = match_path(MODE_STRING2, Kind::PeopleIndex, "/peoples/").unwrap();
        assert_eq!(peoples.len(), 0);

        // 标签页匹配
        let tag = match_path(MODE_STRING2, Kind::Tag, "/tag/5").unwrap();
        assert_eq!(tag["id"], "5");
        let tag_page = match_path(MODE_STRING2, Kind::Tag, "/tag/5/2").unwrap();
        assert_eq!(tag_page["id"], "5");
        assert_eq!(tag_page["page"], "2");
        let tag_str = match_path(MODE_STRING1, Kind::Tag, "/tag/hot").unwrap();
        assert_eq!(tag_str["filename"], "hot");
        let tags_index = match_path(MODE_STRING2, Kind::TagIndex, "/tags/").unwrap();
        assert_eq!(tags_index.len(), 0);
    }

    #[tokio::test]
    async fn reads_rewrite_mode() {
        let _guard = crate::models::db_lock().await;
        let Some(pool) = crate::models::test_pool().await else {
            return;
        };
        let mode = load_mode(&pool).await.unwrap();
        assert!((0..=4).contains(&mode), "mode={mode}");
    }
}

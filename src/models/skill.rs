//! AI 技能（文件系统：`data/skills/<name>/SKILL.md`）。
//!
//! 对齐 Go 版 `controller/manageController/skill.go`：
//! - 每个技能是一个目录，元信息写在 `SKILL.md` 的 YAML frontmatter 里；
//! - 技能名只允许字母、数字、`-`、`_`（同时是目录名，防目录穿越）；
//! - 列表附带目录文件数与 `SKILL.md` 的修改时间。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::ModelError;

/// 技能列表项。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub category: String,
    pub version: String,
    pub tags: Vec<String>,
    pub updated_at: String,
    pub file_count: usize,
}

/// 技能详情。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SkillDetail {
    pub name: String,
    pub description: String,
    pub category: String,
    pub version: String,
    pub author: String,
    pub tags: Vec<String>,
    pub content: String,
    pub base_dir: String,
    pub updated_at: String,
}

/// 技能保存入参。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SkillInput {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub content: String,
}

/// 技能名合法性（与 Go 一致：字母/数字/中划线/下划线）。
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// 技能根目录（`<data>/skills`）。
pub fn skills_root(data_dir: &Path) -> PathBuf {
    data_dir.join("skills")
}

fn skill_dir(data_dir: &Path, name: &str) -> Result<PathBuf, ModelError> {
    if !valid_name(name) {
        return Err(ModelError::invalid("name只能包含字母、数字、中划线和下划线"));
    }
    Ok(skills_root(data_dir).join(name))
}

fn format_time(time: std::time::SystemTime) -> String {
    let datetime: chrono::DateTime<chrono::Local> = time.into();
    datetime.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 拆出 frontmatter 文本与正文。
///
/// frontmatter 形如：
/// ```text
/// ---
/// name: xx
/// tags: [a, b]
/// ---
///
/// 正文
/// ```
fn split_frontmatter(raw: &str) -> (String, String) {
    let trimmed = raw.trim_start_matches('\u{feff}').trim_start_matches(['\r', '\n']);
    if !trimmed.starts_with("---") {
        return (String::new(), trimmed.to_string());
    }
    let rest = &trimmed[3..];
    let rest = rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n')).unwrap_or(rest);

    let mut offset = 0usize;
    for line in rest.split_inclusive('\n') {
        if line.trim() == "---" {
            let head = rest[..offset].to_string();
            let body = rest[offset + line.len()..].trim_start_matches(['\r', '\n']).to_string();
            return (head, body);
        }
        offset += line.len();
    }
    (String::new(), trimmed.to_string())
}

fn parse_tags(value: &str) -> Vec<String> {
    let value = value.trim();
    let inner = value
        .strip_prefix('[')
        .and_then(|item| item.strip_suffix(']'))
        .unwrap_or(value);
    inner
        .split(',')
        .map(|item| item.trim().trim_matches('"').trim_matches('\'').trim())
        .filter(|item| !item.is_empty())
        .map(|item| item.to_string())
        .collect()
}

/// 解析 frontmatter，返回 `(字段表, 正文)`。
fn parse_document(raw: &str) -> (std::collections::HashMap<String, String>, String) {
    let (head, body) = split_frontmatter(raw);
    let mut map = std::collections::HashMap::new();
    for line in head.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    (map, body)
}

fn read_skill(data_dir: &Path, name: &str) -> Option<SkillDetail> {
    let dir = skill_dir(data_dir, name).ok()?;
    let md = dir.join("SKILL.md");
    let raw = std::fs::read_to_string(&md).ok()?;
    let (meta, body) = parse_document(&raw);
    let updated_at = std::fs::metadata(&md)
        .and_then(|meta| meta.modified())
        .map(format_time)
        .unwrap_or_default();

    Some(SkillDetail {
        name: meta.get("name").cloned().unwrap_or_else(|| name.to_string()),
        description: meta.get("description").cloned().unwrap_or_default(),
        category: meta.get("category").cloned().unwrap_or_default(),
        version: meta.get("version").cloned().unwrap_or_default(),
        author: meta.get("author").cloned().unwrap_or_default(),
        tags: meta.get("tags").map(|value| parse_tags(value)).unwrap_or_default(),
        content: body,
        base_dir: dir.to_string_lossy().to_string(),
        updated_at,
    })
}

/// 列出全部技能（按名称排序）。
pub fn list(data_dir: &Path) -> Vec<SkillSummary> {
    let root = skills_root(data_dir);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };

    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .filter_map(|entry| entry.file_name().to_str().map(|name| name.to_string()))
        .collect();
    names.sort();

    let mut result = Vec::new();
    for name in names {
        let Some(detail) = read_skill(data_dir, &name) else {
            continue;
        };
        let file_count = std::fs::read_dir(skills_root(data_dir).join(&name))
            .map(|entries| entries.count())
            .unwrap_or(0);
        result.push(SkillSummary {
            name: detail.name,
            description: detail.description,
            category: detail.category,
            version: detail.version,
            tags: detail.tags,
            updated_at: detail.updated_at,
            file_count,
        });
    }
    result
}

/// 取技能详情。
pub fn get(data_dir: &Path, name: &str) -> Result<Option<SkillDetail>, ModelError> {
    if !valid_name(name) {
        return Err(ModelError::invalid("name只能包含字母、数字、中划线和下划线"));
    }
    Ok(read_skill(data_dir, name))
}

/// 新建 / 更新技能。
pub fn save(data_dir: &Path, input: &SkillInput) -> Result<(), ModelError> {
    let dir = skill_dir(data_dir, &input.name)?;
    std::fs::create_dir_all(&dir).map_err(|err| ModelError::invalid(format!("创建目录失败: {err}")))?;

    let mut frontmatter = String::from("---\n");
    frontmatter.push_str(&format!("name: {}\n", input.name));
    if !input.description.is_empty() {
        frontmatter.push_str(&format!("description: {}\n", input.description));
    }
    if !input.category.is_empty() {
        frontmatter.push_str(&format!("category: {}\n", input.category));
    }
    if !input.version.is_empty() {
        frontmatter.push_str(&format!("version: {}\n", input.version));
    }
    if !input.author.is_empty() {
        frontmatter.push_str(&format!("author: {}\n", input.author));
    }
    if !input.tags.is_empty() {
        frontmatter.push_str(&format!("tags: [{}]\n", input.tags.join(", ")));
    }
    frontmatter.push_str("---\n\n");

    let content = format!("{frontmatter}{}", input.content);
    let md = dir.join("SKILL.md");
    std::fs::write(&md, content).map_err(|err| ModelError::invalid(format!("写入文件失败: {err}")))?;
    Ok(())
}

/// 删除技能目录，返回是否真的删掉。
pub fn delete(data_dir: &Path, name: &str) -> Result<bool, ModelError> {
    let dir = skill_dir(data_dir, name)?;
    if !dir.exists() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&dir).map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rustcms-skills-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn validates_names() {
        assert!(valid_name("seo-helper"));
        assert!(valid_name("a_b1"));
        assert!(!valid_name("../etc"));
        assert!(!valid_name("中文"));
        assert!(!valid_name(""));
    }

    #[test]
    fn parses_frontmatter_and_body() {
        let raw = "---\nname: demo\ntags: [a, b]\n---\n\n正文内容\n";
        let (meta, body) = parse_document(raw);
        assert_eq!(meta.get("name").map(String::as_str), Some("demo"));
        assert_eq!(body.trim(), "正文内容");
        // 没有 frontmatter 时整篇都是正文
        let (meta2, body2) = parse_document("纯正文");
        assert!(meta2.is_empty());
        assert_eq!(body2, "纯正文");
    }

    #[test]
    fn parses_tags() {
        assert_eq!(parse_tags("[a, b ,c]"), vec!["a", "b", "c"]);
        assert!(parse_tags("[]").is_empty());
        assert_eq!(parse_tags("x,y"), vec!["x", "y"]);
    }

    #[test]
    fn saves_lists_and_deletes_skill() {
        let root = temp_root("crud");
        let input = SkillInput {
            name: "demo-skill".into(),
            description: "一句话".into(),
            category: "SEO".into(),
            version: "1.0".into(),
            author: "tester".into(),
            tags: vec!["a".into(), "b".into()],
            content: "# 正文\nhello".into(),
        };
        save(&root, &input).unwrap();

        let list = super::list(&root);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "demo-skill");
        assert_eq!(list[0].tags, vec!["a", "b"]);
        assert!(list[0].file_count >= 1);

        let detail = get(&root, "demo-skill").unwrap().unwrap();
        assert_eq!(detail.author, "tester");
        assert!(detail.content.contains("hello"));

        assert!(get(&root, "../etc").is_err());
        assert!(delete(&root, "demo-skill").unwrap());
        assert!(!delete(&root, "demo-skill").unwrap());
        assert!(super::list(&root).is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }
}

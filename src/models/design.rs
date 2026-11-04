//! 模板设计器（纯文件系统）。
//!
//! 对齐 Go 版 `provider/design.go` 的目录约定：
//! - 模板源码：`<root>/template/<package>/`（元信息在同目录 `config.json`）；
//! - 模板静态资源：`<root>/public/static/<package>/`；
//! - 历史版本：`<root>/cache/.history/<package>/<md5(文件相对路径)>/<md5(内容)>`。
//!
//! 所有路径都经过 [`safe_path`] 校验，禁止 `..` / 盘符 / 绝对路径穿越。

use std::path::{Path, PathBuf};

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

use super::ModelError;

/// 模板里的一个文件。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DesignFile {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub remark: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub last_mod: String,
}

/// 模板包元信息（`config.json`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DesignPackage {
    #[serde(default)]
    pub template_id: i64,
    #[serde(default)]
    pub auth_id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub package: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub created: String,
    /// 0 = 自适应，1 = 代码适配，2 = 电脑+手机
    #[serde(default)]
    pub template_type: i32,
    #[serde(default)]
    pub status: i32,
    #[serde(default)]
    pub tpl_files: Vec<DesignFile>,
    #[serde(default)]
    pub static_files: Vec<DesignFile>,
    #[serde(default)]
    pub preview_data: bool,
}

/// 文件详情（含正文）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct DesignFileDetail {
    pub path: String,
    pub remark: String,
    pub size: u64,
    pub last_mod: String,
    pub content: String,
    /// 非 UTF-8 文本（图片等）时为 true，`content` 为空
    pub is_binary: bool,
}

/// 历史版本。
#[derive(Debug, Clone, Default, Serialize)]
pub struct DesignHistory {
    pub hash: String,
    pub last_mod: String,
    pub size: u64,
}

/// 模板包名校验：只允许字母、数字、`-`、`_`。
pub fn valid_package(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// 把相对路径安全地拼到 base 下；含 `..` / 盘符 / 绝对路径时返回 `None`。
fn safe_path(base: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.trim().replace('\\', "/");
    let rel = rel.trim_matches('/');
    if rel.is_empty() || rel.contains(':') {
        return None;
    }
    if rel.split('/').any(|seg| seg == "..") {
        return None;
    }
    Some(base.join(rel))
}

fn format_time(time: std::time::SystemTime) -> String {
    let datetime: chrono::DateTime<chrono::Local> = time.into();
    datetime.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn md5_hex(bytes: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// 递归列出目录下所有文件（路径相对 `root`）。
fn scan_dir(root: &Path) -> Vec<DesignFile> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            out.push(DesignFile {
                path: rel.to_string_lossy().replace('\\', "/"),
                remark: String::new(),
                size: meta.len(),
                last_mod: meta.modified().map(format_time).unwrap_or_default(),
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// 模板仓库（三个根目录）。
#[derive(Debug, Clone)]
pub struct DesignStore {
    pub template_root: PathBuf,
    pub static_root: PathBuf,
    pub history_root: PathBuf,
}

impl DesignStore {
    /// `root` 是站点根目录（`config.json` 所在目录）。
    pub fn new(root: &Path) -> Self {
        Self {
            template_root: root.join("template"),
            static_root: root.join("public").join("static"),
            history_root: root.join("cache").join(".history"),
        }
    }

    fn base(&self, package: &str, kind: &str) -> Result<PathBuf, ModelError> {
        if !valid_package(package) {
            return Err(ModelError::invalid("模板包名不合法"));
        }
        Ok(if kind == "static" {
            self.static_root.join(package)
        } else {
            self.template_root.join(package)
        })
    }

    fn config_path(&self, package: &str) -> PathBuf {
        self.template_root.join(package).join("config.json")
    }

    /// 列出全部模板包（`template/` 下的目录）。
    pub fn list(&self) -> Vec<DesignPackage> {
        let Ok(entries) = std::fs::read_dir(&self.template_root) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
            .filter_map(|entry| entry.file_name().to_str().map(|name| name.to_string()))
            .filter(|name| !name.starts_with('.'))
            .collect();
        names.sort();

        let mut result = Vec::new();
        for name in names {
            let mut package = self.read_config(&name).unwrap_or_else(|| DesignPackage {
                name: name.clone(),
                package: name.clone(),
                created: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                ..Default::default()
            });
            package.package = name;
            if package.name.trim().is_empty() {
                package.name = package.package.clone();
            }
            result.push(package);
        }
        result
    }

    fn read_config(&self, package: &str) -> Option<DesignPackage> {
        let raw = std::fs::read_to_string(self.config_path(package)).ok()?;
        serde_json::from_str::<DesignPackage>(&raw).ok()
    }

    /// 取模板包；`scan = true` 时重新扫描磁盘文件列表。
    pub fn get(&self, package: &str, scan: bool) -> Result<DesignPackage, ModelError> {
        if !valid_package(package) {
            return Err(ModelError::invalid("模板包名不合法"));
        }
        let mut info = match self.read_config(package) {
            Some(info) => info,
            None => {
                // 目录存在但没有 config.json：按 Go 的行为「尝试生成」
                if !self.template_root.join(package).is_dir() {
                    return Err(ModelError::invalid("模板不存在"));
                }
                DesignPackage {
                    name: package.to_string(),
                    package: package.to_string(),
                    created: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                    ..Default::default()
                }
            }
        };
        info.package = package.to_string();
        if info.name.trim().is_empty() {
            info.name = package.to_string();
        }
        if scan {
            info.tpl_files = self.scan_template_files(package);
            info.static_files = scan_dir(&self.static_root.join(package));
        }
        Ok(info)
    }

    fn scan_template_files(&self, package: &str) -> Vec<DesignFile> {
        scan_dir(&self.template_root.join(package))
            .into_iter()
            .filter(|file| {
                !file.path.ends_with("config.json") && !file.path.ends_with("data.db")
            })
            .collect()
    }

    /// 保存模板名与类型（其余字段保持 config.json 原值）。
    pub fn save_info(
        &self,
        package: &str,
        name: &str,
        template_type: i32,
    ) -> Result<DesignPackage, ModelError> {
        let mut info = self.get(package, false)?;
        if !name.trim().is_empty() {
            info.name = name.trim().to_string();
        }
        info.template_type = template_type;

        let path = self.config_path(package);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| ModelError::invalid(format!("写入失败: {err}")))?;
        }
        let raw = serde_json::to_string_pretty(&info)?;
        std::fs::write(&path, raw).map_err(|err| ModelError::invalid(format!("写入失败: {err}")))?;
        Ok(info)
    }

    /// 删除模板：移到 `cache/.history/<package>/`（`default` 不允许删）。
    pub fn delete(&self, package: &str) -> Result<(), ModelError> {
        if !valid_package(package) {
            return Err(ModelError::invalid("模板包名不合法"));
        }
        if package == "default" {
            return Err(ModelError::invalid("默认模板不能删除"));
        }
        let template_dir = self.template_root.join(package);
        if !template_dir.is_dir() {
            return Err(ModelError::invalid("模板不存在"));
        }

        let target = self.history_root.join(package);
        let _ = std::fs::remove_dir_all(&target);
        std::fs::create_dir_all(&target)
            .map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?;

        if let Err(err) = std::fs::rename(&template_dir, target.join("template")) {
            return Err(ModelError::invalid(format!("删除失败: {err}")));
        }
        let static_dir = self.static_root.join(package);
        if static_dir.is_dir() {
            let _ = std::fs::rename(&static_dir, target.join("static"));
        }
        Ok(())
    }

    fn file_path(&self, package: &str, rel: &str, kind: &str) -> Result<PathBuf, ModelError> {
        let base = self.base(package, kind)?;
        safe_path(&base, rel).ok_or_else(|| ModelError::invalid("文件不存在"))
    }

    /// 读文件详情。
    pub fn file_detail(
        &self,
        package: &str,
        rel: &str,
        kind: &str,
    ) -> Result<DesignFileDetail, ModelError> {
        let path = self.file_path(package, rel, kind)?;
        let meta = std::fs::metadata(&path).map_err(|_| ModelError::invalid("文件不存在"))?;
        let bytes = std::fs::read(&path).map_err(|_| ModelError::invalid("文件不存在"))?;

        let (content, is_binary) = match String::from_utf8(bytes) {
            Ok(text) => (text, false),
            Err(_) => (String::new(), true),
        };

        Ok(DesignFileDetail {
            path: rel.trim().replace('\\', "/"),
            remark: String::new(),
            size: meta.len(),
            last_mod: meta.modified().map(format_time).unwrap_or_default(),
            content,
            is_binary,
        })
    }

    /// 历史目录：`history/<package>/<md5(相对路径)>`。
    fn history_dir(&self, package: &str, rel: &str) -> PathBuf {
        self.history_root.join(package).join(md5_hex(rel.as_bytes()))
    }

    /// 把当前内容存一份历史（写文件前调用）。
    fn store_history(&self, package: &str, rel: &str, content: &[u8]) -> Result<(), ModelError> {
        let dir = self.history_dir(package, rel);
        std::fs::create_dir_all(&dir)
            .map_err(|err| ModelError::invalid(format!("写入历史版本失败: {err}")))?;
        let hash = md5_hex(content);
        std::fs::write(dir.join(hash), content)
            .map_err(|err| ModelError::invalid(format!("写入历史版本失败: {err}")))?;
        Ok(())
    }

    /// 覆盖文件内容（先备份历史）。
    pub fn save_file(
        &self,
        package: &str,
        rel: &str,
        kind: &str,
        content: &str,
    ) -> Result<(), ModelError> {
        let path = self.file_path(package, rel, kind)?;
        if let Ok(previous) = std::fs::read(&path) {
            self.store_history(package, rel, &previous)?;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| ModelError::invalid(format!("写入失败: {err}")))?;
        }
        std::fs::write(&path, content).map_err(|err| ModelError::invalid(format!("写入失败: {err}")))?;
        Ok(())
    }

    /// 重命名文件（同目录或跨目录）。
    pub fn rename_file(
        &self,
        package: &str,
        rel: &str,
        new_rel: &str,
        kind: &str,
    ) -> Result<(), ModelError> {
        let from = self.file_path(package, rel, kind)?;
        let to = self.file_path(package, new_rel, kind)?;
        if !from.is_file() {
            return Err(ModelError::invalid("文件不存在"));
        }
        if to.exists() {
            return Err(ModelError::invalid("目标文件已存在"));
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| ModelError::invalid(format!("重命名失败: {err}")))?;
        }
        std::fs::rename(&from, &to)
            .map_err(|err| ModelError::invalid(format!("重命名失败: {err}")))?;
        Ok(())
    }

    /// 复制文件。
    pub fn copy_file(
        &self,
        package: &str,
        rel: &str,
        new_rel: &str,
        kind: &str,
    ) -> Result<(), ModelError> {
        let from = self.file_path(package, rel, kind)?;
        let to = self.file_path(package, new_rel, kind)?;
        if !from.is_file() {
            return Err(ModelError::invalid("文件不存在"));
        }
        if to.exists() {
            return Err(ModelError::invalid("目标文件已存在"));
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| ModelError::invalid(format!("复制失败: {err}")))?;
        }
        std::fs::copy(&from, &to).map_err(|err| ModelError::invalid(format!("复制失败: {err}")))?;
        Ok(())
    }

    /// 删除文件（先备份历史）。
    pub fn delete_file(&self, package: &str, rel: &str, kind: &str) -> Result<(), ModelError> {
        let path = self.file_path(package, rel, kind)?;
        if !path.is_file() {
            return Err(ModelError::invalid("文件不存在"));
        }
        if let Ok(previous) = std::fs::read(&path) {
            self.store_history(package, rel, &previous)?;
        }
        std::fs::remove_file(&path).map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?;
        Ok(())
    }

    /// 上传文件到模板目录下的指定子目录。
    pub fn upload_file(
        &self,
        package: &str,
        dir: &str,
        file_name: &str,
        kind: &str,
        bytes: &[u8],
    ) -> Result<String, ModelError> {
        let file_name = file_name.trim().replace('\\', "/");
        if file_name.is_empty() || file_name.contains("..") {
            return Err(ModelError::invalid("文件名不合法"));
        }
        let rel = if dir.trim().is_empty() {
            file_name
        } else {
            format!("{}/{}", dir.trim().trim_matches('/'), file_name)
        };
        let path = self.file_path(package, &rel, kind)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| ModelError::invalid(format!("上传失败: {err}")))?;
        }
        std::fs::write(&path, bytes).map_err(|err| ModelError::invalid(format!("上传失败: {err}")))?;
        Ok(rel)
    }

    /// 某文件的历史版本（按时间倒序）。
    pub fn histories(&self, package: &str, rel: &str) -> Vec<DesignHistory> {
        let dir = self.history_dir(package, rel);
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut out: Vec<DesignHistory> = entries
            .flatten()
            .filter_map(|entry| {
                let meta = entry.metadata().ok()?;
                if !meta.is_file() {
                    return None;
                }
                Some(DesignHistory {
                    hash: entry.file_name().to_str()?.to_string(),
                    last_mod: meta.modified().map(format_time).unwrap_or_default(),
                    size: meta.len(),
                })
            })
            .collect();
        out.sort_by(|a, b| b.last_mod.cmp(&a.last_mod));
        out
    }

    /// 历史版本内容。
    pub fn history_content(
        &self,
        package: &str,
        rel: &str,
        hash: &str,
    ) -> Result<DesignFileDetail, ModelError> {
        if hash.trim().is_empty() || hash.contains("..") || hash.contains('/') {
            return Err(ModelError::invalid("历史版本不存在"));
        }
        let path = self.history_dir(package, rel).join(hash.trim());
        let meta = std::fs::metadata(&path).map_err(|_| ModelError::invalid("历史版本不存在"))?;
        let bytes = std::fs::read(&path).map_err(|_| ModelError::invalid("历史版本不存在"))?;
        let (content, is_binary) = match String::from_utf8(bytes) {
            Ok(text) => (text, false),
            Err(_) => (String::new(), true),
        };
        Ok(DesignFileDetail {
            path: rel.trim().to_string(),
            remark: String::new(),
            size: meta.len(),
            last_mod: meta.modified().map(format_time).unwrap_or_default(),
            content,
            is_binary,
        })
    }

    /// 把历史版本恢复成当前文件。
    pub fn restore_history(
        &self,
        package: &str,
        rel: &str,
        hash: &str,
        kind: &str,
    ) -> Result<(), ModelError> {
        // 先确认历史版本存在
        self.history_content(package, rel, hash)?;
        let target = self.file_path(package, rel, kind)?;

        // 先备份当前内容，再写回历史内容
        if let Ok(previous) = std::fs::read(&target) {
            self.store_history(package, rel, &previous)?;
        }
        let history_file = self.history_dir(package, rel).join(hash.trim());
        let bytes = std::fs::read(history_file)
            .map_err(|_| ModelError::invalid("历史版本不存在"))?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| ModelError::invalid(format!("恢复失败: {err}")))?;
        }
        std::fs::write(&target, bytes)
            .map_err(|err| ModelError::invalid(format!("恢复失败: {err}")))?;
        Ok(())
    }

    /// 删除历史版本（`hash` 为空时清空该文件的全部历史）。
    pub fn delete_histories(
        &self,
        package: &str,
        rel: &str,
        hash: &str,
    ) -> Result<usize, ModelError> {
        let dir = self.history_dir(package, rel);
        let hash = hash.trim();
        if hash.is_empty() {
            let removed = self.histories(package, rel).len();
            let _ = std::fs::remove_dir_all(&dir);
            return Ok(removed);
        }
        if hash.contains("..") || hash.contains('/') {
            return Err(ModelError::invalid("历史版本不存在"));
        }
        let path = dir.join(hash);
        if !path.is_file() {
            return Err(ModelError::invalid("历史版本不存在"));
        }
        std::fs::remove_file(&path)
            .map_err(|err| ModelError::invalid(format!("删除失败: {err}")))?;
        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rustcms-design-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("template").join("demo")).unwrap();
        std::fs::create_dir_all(dir.join("public").join("static").join("demo")).unwrap();
        std::fs::write(
            dir.join("template").join("demo").join("config.json"),
            r#"{"name":"演示模板","package":"demo","version":"1.0","template_type":1}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("template").join("demo").join("index.html"),
            "<h1>hi</h1>",
        )
        .unwrap();
        std::fs::write(
            dir.join("public").join("static").join("demo").join("a.css"),
            "body{}",
        )
        .unwrap();
        dir
    }

    #[test]
    fn rejects_traversal() {
        let base = Path::new("/srv/template/demo");
        assert!(safe_path(base, "../etc/passwd").is_none());
        assert!(safe_path(base, "").is_none());
        assert!(safe_path(base, "C:/windows").is_none());
        assert!(safe_path(base, "index.html").is_some());
        assert!(safe_path(base, "/index.html").is_some());
    }

    #[test]
    fn validates_package_names() {
        assert!(valid_package("default"));
        assert!(valid_package("my_theme-2"));
        assert!(!valid_package("../etc"));
        assert!(!valid_package("模板"));
        assert!(!valid_package(""));
    }

    #[test]
    fn lists_and_reads_package() {
        let root = temp_root("list");
        let store = DesignStore::new(&root);

        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].package, "demo");
        assert_eq!(list[0].name, "演示模板");

        let scanned = store.get("demo", true).unwrap();
        assert!(scanned.tpl_files.iter().any(|f| f.path == "index.html"));
        assert!(!scanned.tpl_files.iter().any(|f| f.path == "config.json"));
        assert!(scanned.static_files.iter().any(|f| f.path == "a.css"));

        let detail = store.file_detail("demo", "index.html", "template").unwrap();
        assert_eq!(detail.content, "<h1>hi</h1>");
        assert!(!detail.is_binary);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn saves_renames_copies_and_deletes_with_history() {
        let root = temp_root("crud");
        let store = DesignStore::new(&root);

        store
            .save_file("demo", "index.html", "template", "<h1>v2</h1>")
            .unwrap();
        // 旧内容进历史
        let histories = store.histories("demo", "index.html");
        assert_eq!(histories.len(), 1);

        store
            .rename_file("demo", "index.html", "home.html", "template")
            .unwrap();
        assert!(store.file_detail("demo", "home.html", "template").is_ok());

        store
            .copy_file("demo", "home.html", "home-copy.html", "template")
            .unwrap();
        assert!(store
            .file_detail("demo", "home-copy.html", "template")
            .is_ok());

        store.delete_file("demo", "home-copy.html", "template").unwrap();
        assert!(store
            .file_detail("demo", "home-copy.html", "template")
            .is_err());

        // 恢复历史
        let hash = store.histories("demo", "index.html");
        assert_eq!(hash.len(), 1);
        store
            .restore_history("demo", "index.html", &hash[0].hash, "template")
            .unwrap();
        let detail = store.file_detail("demo", "index.html", "template").unwrap();
        assert_eq!(detail.content, "<h1>hi</h1>");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_to_delete_default_template() {
        let root = temp_root("del");
        let store = DesignStore::new(&root);
        std::fs::create_dir_all(root.join("template").join("default")).unwrap();
        std::fs::write(
            root.join("template").join("default").join("config.json"),
            r#"{"name":"默认","package":"default"}"#,
        )
        .unwrap();
        assert!(store.delete("default").is_err());
        assert!(store.delete("demo").is_ok());
        assert!(!root.join("template").join("demo").exists());
        assert!(root.join("cache").join(".history").join("demo").is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn saves_design_info() {
        let root = temp_root("info");
        let store = DesignStore::new(&root);
        let saved = store.save_info("demo", "改名后", 2).unwrap();
        assert_eq!(saved.name, "改名后");
        assert_eq!(saved.template_type, 2);
        // 其它字段保留
        assert_eq!(saved.version, "1.0");
        let _ = std::fs::remove_dir_all(&root);
    }
}

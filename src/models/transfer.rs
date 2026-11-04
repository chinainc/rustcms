//! 数据迁移（`pluginTransfer.go`）。
//!
//! Go 版的迁移任务是**纯内存对象** + 对远端站点 `*2rustcms.php` 发起 HTTP 请求拉数据，
//! 本项目暂无 HTTP 客户端，因此：
//! - `task` 恒为空（没有持久化，重启即丢，与 Go 一致）；
//! - `create` / `modules` / `start` 明确返回未实现；
//! - **`download` 是真实可用的**：把内置的 `clientFiles/<provider>2rustcms.php` 发给用户，
//!   用户在自己旧站上放好这个文件后，才能配迁移。

use std::path::{Path, PathBuf};

/// 支持的建站系统标识（与 `clientFiles/` 下的文件名一一对应）。
pub const PROVIDERS: &[&str] = &["dedecms", "empire", "pbootcms", "wordpress", "xunruicms", "train"];

/// 迁移任务（Go 里是内存结构，这里只用于回包）。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TransferTask {
    pub name: String,
    pub base_url: String,
    pub token: String,
    pub target_id: String,
    pub provider: String,
    /// 0 等待、1 进行中、2 完成
    pub status: i32,
    pub error_msg: String,
    pub current: String,
    pub last_id: i64,
    pub last_mod: String,
}

/// 建站系统标识合法性：只允许字母、数字、`_`、`-`。
pub fn valid_provider(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

/// 客户端文件的文件名：`train` 是 wpm 插件包，其余是 php。
pub fn client_file_name(provider: &str) -> String {
    if provider.eq_ignore_ascii_case("train") {
        "train2rustcms.wpm".to_string()
    } else {
        format!("{provider}2rustcms.php")
    }
}

/// 定位 `<root>/clientFiles/<provider>2rustcms.php`，并确保没有路径穿越、文件存在。
pub fn client_file(root: &Path, provider: &str) -> Option<PathBuf> {
    if !valid_provider(provider) {
        return None;
    }
    let base = root.join("clientFiles");
    let path = base.join(client_file_name(provider));
    // 双保险：拼出来的路径必须仍在 clientFiles 下
    if !path.starts_with(&base) || !path.is_file() {
        return None;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_provider_names() {
        assert!(valid_provider("dedecms"));
        assert!(valid_provider("xunruicms"));
        assert!(!valid_provider("../etc"));
        assert!(!valid_provider("织梦"));
        assert!(!valid_provider(""));
    }

    #[test]
    fn maps_client_file_names() {
        assert_eq!(client_file_name("wordpress"), "wordpress2rustcms.php");
        assert_eq!(client_file_name("train"), "train2rustcms.wpm");
    }

    #[test]
    fn finds_real_client_files() {
        // 仓库里确实带了这些文件（`<repo>/clientFiles/`），顺便验证路径拼接
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crate 目录应在上层目录下");
        for provider in PROVIDERS {
            let found = client_file(root, provider);
            assert!(found.is_some(), "缺少 {provider} 的客户端文件");
            assert!(found.unwrap().starts_with(root.join("clientFiles")));
        }
        // 非法名字一律 None
        assert!(client_file(root, "../config").is_none());
        assert!(client_file(root, "notexist").is_none());
    }
}

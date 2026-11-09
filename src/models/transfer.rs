//! 数据迁移。
//!
//! 迁移任务是**纯内存对象** + 对远端站点 `*2rustcms.php` 发起 HTTP 请求拉数据，
//! 本包实现：
//! - `create`：校验参数并请求远端 `a=config` 做连通性确认，返回任务对象；
//! - **`download` 是真实可用的**：把内置的 `clientFiles/<provider>2rustcms.php` 发给用户，
//! 用户在自己旧站上放好这个文件后，才能配迁移；
//! - `task` / `modules` / `start` 仍保持原有占位行为（不在本包范围）。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use super::ModelError;

/// 支持的建站系统标识（与 `clientFiles/` 下的文件名一一对应）。
pub const PROVIDERS: &[&str] = &["dedecms", "empire", "pbootcms", "wordpress", "xunruicms", "train"];

/// 迁移任务（既有实现里是内存结构，这里只用于回包）。
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
#[serde(default)]
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

/// 创建迁移任务：校验参数后请求远端站点 `a=config` 确认客户端文件可用。
///
/// 遵循既有约定：参数非法 / 远端返回 `code != 0` / 响应无法解析时
/// 都直接报错，不会把任务保存到任何地方（既有实现同样只存在内存里）。
pub async fn create(input: TransferTask) -> Result<TransferTask, ModelError> {
    let provider = input.provider.trim().to_string();
    if provider.is_empty() {
        return Err(ModelError::invalid("请选择迁移系统"));
    }
    if !valid_provider(&provider) {
        return Err(ModelError::invalid("迁移系统标识不合法"));
    }

    let base_url = input.base_url.trim().trim_end_matches('/').to_string();
    if base_url.is_empty() {
        return Err(ModelError::invalid("请填写旧站地址"));
    }

    let task = TransferTask {
        name: input.name.trim().to_string(),
        base_url: base_url.clone(),
        token: input.token.trim().to_string(),
        target_id: input.target_id.trim().to_string(),
        provider: provider.clone(),
        status: 0,
        ..Default::default()
    };

    let target_id = percent_encoding::utf8_percent_encode(
        &task.target_id,
        percent_encoding::NON_ALPHANUMERIC,
    );
    let url = format!(
        "{base_url}/{provider}2rustcms.php?a=config&from=rustcms&target_id={target_id}"
    );

    let request = rustcms_ext::http::Request::post(url)
        .with_json(json!({
            "baseUrl": task.base_url,
            "token": task.token,
            "targetId": task.target_id,
            "provider": task.provider,
        }))
        .with_timeout(Duration::from_secs(300));

    let response = rustcms_ext::http::send(&request)
        .await
        .map_err(|err| ModelError::invalid(format!("连接旧站失败: {err}")))?;
    if !response.is_success() {
        return Err(ModelError::invalid(format!(
            "旧站返回异常状态码 {}",
            response.status
        )));
    }

    let value: Value = response
        .json()
        .map_err(|err| ModelError::invalid(format!("旧站返回数据无法解析: {err}")))?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let message = value
            .get("msg")
            .and_then(Value::as_str)
            .filter(|msg| !msg.trim().is_empty())
            .unwrap_or("旧站返回错误");
        return Err(ModelError::invalid(message.to_string()));
    }

    Ok(task)
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
        // 逻辑验证：在一个临时 root 下铺全套客户端文件，验证路径拼接与防穿越。
        // （部署包里的 `clientFiles/` 由运维随包放置，仓库与测试环境不一定有，
        // 所以真实目录只做「有则校验」，不作为失败条件。）
        let root = std::env::temp_dir().join(format!("rustcms-transfer-{}", std::process::id()));
        let base = root.join("clientFiles");
        std::fs::create_dir_all(&base).expect("建临时 clientFiles");
        for provider in PROVIDERS {
            let name = client_file_name(provider);
            std::fs::write(base.join(&name), b"<?php // test").expect("写临时客户端文件");
            let found = client_file(&root, provider).expect("应能定位到客户端文件");
            assert_eq!(found, base.join(&name));
        }
        // 非法名字一律 None
        assert!(client_file(&root, "../config").is_none());
        assert!(client_file(&root, "notexist").is_none());
        // 目录在但文件名字不匹配（例如只放了其它产品的同名迁移脚本）也是 None
        std::fs::write(base.join("legacy-migration.php"), b"<?php // x").expect("写干扰文件");
        std::fs::remove_file(base.join("dedecms2rustcms.php")).expect("删掉 dedecms 客户端文件");
        assert!(client_file(&root, "dedecms").is_none());
        let _ = std::fs::remove_dir_all(&root);

        // 站点根（`config.json` 所在目录）若真的带了 `clientFiles/`，顺带校验一遍
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crate 目录应在上层目录下");
        if repo_root.join("clientFiles").is_dir() {
            for provider in PROVIDERS {
                let found = client_file(repo_root, provider);
                assert!(found.is_some(), "缺少 {provider} 的客户端文件");
                assert!(found.unwrap().starts_with(repo_root.join("clientFiles")));
            }
            assert!(client_file(repo_root, "../config").is_none());
            assert!(client_file(repo_root, "notexist").is_none());
        }
    }
}

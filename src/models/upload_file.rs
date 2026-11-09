//! 验证文件上传。
//!
//! 用于放搜索引擎/第三方平台要求的校验文件（`BingSiteAuth.xml` 之类），
//! 只允许 `txt / htm / html / xml`，且**必须落在 `public` 根目录**（不能带路径）。
//! 文件清单存 `settings.upload_file`，形如
//! `[{"hash":"<md5(文件名)>","file_name":"xx.xml","created_time":1700000000}]`。

use std::path::Path;

use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;

use super::{site_setting, ModelError};

/// 配置键。
pub const SETTING_KEY: &str = "upload_file";

/// 允许的扩展名。
pub const ALLOWED_EXTS: &[&str] = &["txt", "htm", "html", "xml"];

fn extension_of(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

/// 文件名合法性：单一文件名、不含路径分隔符、不以 `.` 开头、扩展名在白名单内。
pub fn valid_name(file_name: &str) -> bool {
    let name = file_name.trim();
    if name.is_empty() || name.starts_with('.') {
        return false;
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return false;
    }
    ALLOWED_EXTS.contains(&extension_of(name).as_str())
}

/// 读文件清单（补 `link`）。
pub async fn list(pool: &MySqlPool, base_url: &str) -> anyhow::Result<Vec<Value>> {
    // 这个键存的是**数组**，必须用 get_array（get_section 只认对象）
    let value = site_setting::get_array(pool, SETTING_KEY).await?;
    let base = base_url.trim_end_matches('/');
    let mut items: Vec<Value> = match value {
        Value::Array(list) => list,
        Value::Object(map) => map
            .get("files")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    for item in &mut items {
        let file_name = item
            .get("file_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Value::Object(map) = item {
            map.insert("link".into(), json!(format!("{base}/{file_name}")));
        }
    }
    Ok(items)
}

async fn save_list(pool: &MySqlPool, items: &[Value]) -> Result<(), ModelError> {
    site_setting::save_value(pool, SETTING_KEY, &Value::Array(items.to_vec()))
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    Ok(())
}

/// 上传验证文件：写入 `public/<文件名>` 并登记到清单（同名不重复登记）。
pub async fn add(
    pool: &MySqlPool,
    public_dir: &Path,
    file_name: &str,
    bytes: &[u8],
) -> Result<Value, ModelError> {
    if !valid_name(file_name) {
        return Err(ModelError::invalid("仅允许上传 txt, htm, html, xml 文件"));
    }
    if bytes.is_empty() {
        return Err(ModelError::invalid("文件内容为空"));
    }

    std::fs::create_dir_all(public_dir)
        .map_err(|err| ModelError::invalid(format!("创建目录失败: {err}")))?;
    std::fs::write(public_dir.join(file_name), bytes)
        .map_err(|err| ModelError::invalid(format!("保存文件失败: {err}")))?;

    let hash = super::attachment::md5_hex(file_name.as_bytes());
    let mut items = list(pool, "").await.unwrap_or_default();
    let exists = items
        .iter()
        .any(|item| item.get("file_name").and_then(Value::as_str) == Some(file_name));
    if !exists {
        items.push(json!({
            "hash": hash,
            "file_name": file_name,
            "created_time": chrono::Utc::now().timestamp(),
        }));
        save_list(pool, &items).await?;
    }

    Ok(json!({ "hash": hash, "file_name": file_name }))
}

/// 删除验证文件（按 hash 定位）：先删磁盘，再更新清单。
pub async fn remove(pool: &MySqlPool, public_dir: &Path, hash: &str) -> Result<bool, ModelError> {
    let hash = hash.trim();
    if hash.is_empty() {
        return Err(ModelError::invalid("文件不正确"));
    }

    let items = list(pool, "").await.unwrap_or_default();
    let Some(target) = items
        .iter()
        .find(|item| item.get("hash").and_then(Value::as_str) == Some(hash))
        .cloned()
    else {
        return Err(ModelError::invalid("文件不正确"));
    };
    let file_name = target
        .get("file_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !valid_name(&file_name) {
        return Err(ModelError::invalid("文件不正确"));
    }

    let path = public_dir.join(&file_name);
    if path.is_file() {
        std::fs::remove_file(&path)
            .map_err(|err| ModelError::invalid(format!("删除文件失败: {err}")))?;
    }

    let kept: Vec<Value> = items
        .into_iter()
        .filter(|item| item.get("hash").and_then(Value::as_str) != Some(hash))
        .collect();
    save_list(pool, &kept).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, setting, test_pool};

    #[test]
    fn validates_file_names() {
        assert!(valid_name("BingSiteAuth.xml"));
        assert!(valid_name("verify.txt"));
        assert!(valid_name("a.htm"));
        assert!(!valid_name("shell.php"));
        assert!(!valid_name("../a.xml"));
        assert!(!valid_name("a/b.xml"));
        assert!(!valid_name(".env"));
        assert!(!valid_name(""));
    }

    #[tokio::test]
    async fn upload_and_delete_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, SETTING_KEY).await.unwrap();
        // 清空清单，避免受真实数据影响
        site_setting::save_value(&pool, SETTING_KEY, &json!([]))
            .await
            .unwrap();

        let dir = std::env::temp_dir().join(format!("rustcms-uf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let saved = add(&pool, &dir, "rustcms-verify.xml", b"<xml/>")
            .await
            .unwrap();
        assert_eq!(saved["file_name"], "rustcms-verify.xml");
        assert!(dir.join("rustcms-verify.xml").is_file());

        let items = list(&pool, "https://example.com/").await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["link"], "https://example.com/rustcms-verify.xml");

        // 同名重复上传不重复登记
        add(&pool, &dir, "rustcms-verify.xml", b"<xml/>").await.unwrap();
        assert_eq!(list(&pool, "").await.unwrap().len(), 1);

        let hash = saved["hash"].as_str().unwrap().to_string();
        assert!(remove(&pool, &dir, &hash).await.unwrap());
        assert!(!dir.join("rustcms-verify.xml").exists());
        assert!(list(&pool, "").await.unwrap().is_empty());

        // 再删一次 → 文件不正确
        assert!(remove(&pool, &dir, &hash).await.is_err());

        match backup {
            Some(raw) => setting::save_raw(&pool, SETTING_KEY, &raw).await.unwrap(),
            None => setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

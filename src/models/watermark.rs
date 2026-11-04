//! 图片水印（`pluginWatermark.go`）。
//!
//! 配置存 `settings.watermark`。**真正的水印渲染未实现**（需要图片处理库），
//! 但配置读写与资源上传（水印图 / 字体）是完整可用的。

use std::path::Path;

use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;

use super::{site_setting, ModelError};

/// 配置键。
pub const SETTING_KEY: &str = "watermark";

const UPLOAD_DIR: &str = "uploads/watermark";
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp"];

/// 读配置（补齐默认值，与 Go 的归一逻辑一致）。
pub async fn config_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, SETTING_KEY).await?;
    if let Value::Object(map) = &mut value {
        map.entry("open").or_insert(json!(false));
        map.entry("type").or_insert(json!(0));
        map.entry("image_path").or_insert(json!(""));
        map.entry("text").or_insert(json!(""));
        map.entry("font_path").or_insert(json!(""));
        map.entry("size").or_insert(json!(20));
        map.entry("color").or_insert(json!("#ffffff"));
        map.entry("position").or_insert(json!(9));
        map.entry("opacity").or_insert(json!(100));
        map.entry("min_size").or_insert(json!(400));

        // 归零值回落到默认（Go 里就是 position=9 / opacity=100 / min_size=400）
        for (key, default) in [("size", 20), ("position", 9), ("opacity", 100), ("min_size", 400)] {
            let current = map.get(key).and_then(Value::as_i64).unwrap_or(0);
            if current <= 0 {
                map.insert(key.into(), json!(default));
            }
        }
        if map
            .get("color")
            .and_then(Value::as_str)
            .map(|value| value.trim().is_empty())
            .unwrap_or(true)
        {
            map.insert("color".into(), json!("#ffffff"));
        }
    }
    Ok(value)
}

/// 保存配置（字段级 merge + 归零回落）。
pub async fn config_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    if !patch.is_object() {
        return Err(anyhow::anyhow!("配置必须是 JSON 对象"));
    }
    let mut patch = patch.clone();
    if let Value::Object(map) = &mut patch {
        for (key, default) in [("size", 20), ("position", 9), ("opacity", 100), ("min_size", 400)] {
            if let Some(value) = map.get(key).and_then(Value::as_i64) {
                if value <= 0 {
                    map.insert(key.into(), json!(default));
                }
            }
        }
    }
    site_setting::save_section(pool, SETTING_KEY, &patch).await?;
    config_get(pool).await
}

fn extension_of(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

/// 上传水印资源，返回相对 `public` 的路径。
///
/// `kind`：`font_path`（只收 `.ttf`，固定名 `watermark_font.ttf`）或 `image_path`
/// （`watermark_image.<ext>`）。
pub fn upload(
    public_dir: &Path,
    kind: &str,
    file_name: &str,
    bytes: &[u8],
) -> Result<String, ModelError> {
    if bytes.is_empty() {
        return Err(ModelError::invalid("文件内容为空"));
    }

    let relative = match kind {
        "font_path" => {
            if extension_of(file_name) != "ttf" {
                return Err(ModelError::invalid("仅支持 ttf 格式字体"));
            }
            format!("{UPLOAD_DIR}/watermark_font.ttf")
        }
        "image_path" => {
            let ext = extension_of(file_name);
            if !IMAGE_EXTS.contains(&ext.as_str()) {
                return Err(ModelError::invalid("不支持的图片格式"));
            }
            format!("{UPLOAD_DIR}/watermark_image.{ext}")
        }
        _ => return Err(ModelError::invalid("文件名无效")),
    };

    let full = public_dir.join(&relative);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| ModelError::invalid(format!("创建目录失败: {err}")))?;
    }
    std::fs::write(&full, bytes).map_err(|err| ModelError::invalid(format!("保存文件失败: {err}")))?;
    Ok(relative)
}

/// 上传后把路径写回配置（`font_path` 或 `image_path`）。
pub async fn set_path(pool: &MySqlPool, kind: &str, path: &str) -> anyhow::Result<()> {
    let key = if kind == "font_path" {
        "font_path"
    } else {
        "image_path"
    };
    let mut patch = serde_json::Map::new();
    patch.insert(key.to_string(), json!(path));
    site_setting::save_section(pool, SETTING_KEY, &Value::Object(patch)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, setting, test_pool};

    #[tokio::test]
    async fn config_defaults_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, SETTING_KEY).await.unwrap();

        let loaded = config_get(&pool).await.unwrap();
        assert_eq!(loaded["size"], json!(20));
        assert_eq!(loaded["position"], json!(9));
        assert_eq!(loaded["opacity"], json!(100));
        assert_eq!(loaded["min_size"], json!(400));
        assert_eq!(loaded["color"], json!("#ffffff"));

        // 归零回落
        let saved = config_save(
            &pool,
            &json!({"open": true, "size": 0, "opacity": 0, "color": "  "}),
        )
        .await
        .unwrap();
        assert_eq!(saved["open"], json!(true));
        assert_eq!(saved["size"], json!(20));
        assert_eq!(saved["opacity"], json!(100));
        assert_eq!(saved["color"], json!("#ffffff"));

        // 正常值保留
        let saved = config_save(&pool, &json!({"size": 36, "position": 1}))
            .await
            .unwrap();
        assert_eq!(saved["size"], json!(36));
        assert_eq!(saved["position"], json!(1));

        match backup {
            Some(raw) => setting::save_raw(&pool, SETTING_KEY, &raw).await.unwrap(),
            None => setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
    }

    #[test]
    fn uploads_font_and_image() {
        let dir = std::env::temp_dir().join(format!("rustcms-wm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        assert!(upload(&dir, "font_path", "font.otf", b"x").is_err());
        let path = upload(&dir, "font_path", "f.TTF", b"ttf").unwrap();
        assert_eq!(path, "uploads/watermark/watermark_font.ttf");
        assert!(dir.join(&path).is_file());

        let path = upload(&dir, "image_path", "logo.PNG", b"png").unwrap();
        assert_eq!(path, "uploads/watermark/watermark_image.png");
        assert!(dir.join(&path).is_file());

        assert!(upload(&dir, "image_path", "a.exe", b"x").is_err());
        assert!(upload(&dir, "unknown", "a.png", b"x").is_err());
        assert!(upload(&dir, "image_path", "a.png", b"").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

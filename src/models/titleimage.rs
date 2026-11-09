//! 标题生成图片。
//!
//! 配置存 `settings.title_image`。
//!
//! 渲染分工（两条路都保留，互不冲突）：
//! - **位图管线**（[`background`] + [`draw`]）：生成背景位图——有背景图则等比裁剪，
//! 否则按标题摘要生成深色细带 + 噪声再放大；配色算法
//! （`sumTitle` / `RandColor` / `RandDeepColor`）逐项照搬既有实现。再用自带的 TrueType
//! 光栅化（[`super::watermark::font`]）按同样的口径把标题画上去（自动字号、
//! 居中折行、可选文字底色带）。图片解码/编码/缩放见 [`image_ops`]（PNG / JPEG / GIF / BMP）。
//! - **SVG 管线**（路由层）：作为**兜底**保留 —— 一个可用字体都找不到时，预览与批量生成
//! 仍能出图（内嵌 `@font-face`，由浏览器负责光栅化）。
//!
//! 资源上传：
//! - 字体只收 `.ttf`，固定落到 `uploads/titleimage/title_font.ttf`；
//! - 背景图按 `md5(原文件名)` 命名，落到 `uploads/titleimage/`。

pub mod background;
pub mod draw;
pub mod gif;
pub mod image_ops;
pub mod jpeg;
pub mod jpeg_encode;

use std::path::Path;

use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;

use super::{site_setting, ModelError};

// 供路由层/后台直接调用（当前只有测试在用，故屏蔽未使用告警）
#[allow(unused_imports)]
pub use background::{background_image, deep_color, random_color, title_sum};

/// 标题图片配置键。
pub const SETTING_KEY: &str = "title_image";

/// 资源上传目录（相对 `public`）。
const UPLOAD_DIR: &str = "uploads/titleimage";

/// 允许的背景图扩展名。
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp"];

/// 读配置（补齐默认值）。
pub async fn config_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, SETTING_KEY).await?;
    if let Value::Object(map) = &mut value {
        map.entry("open").or_insert(json!(false));
        map.entry("draw_sub").or_insert(json!(false));
        if !map.get("bg_images").is_some_and(Value::is_array) {
            map.insert("bg_images".into(), json!([]));
        }
        map.entry("font_path").or_insert(json!(""));
        map.entry("font_size").or_insert(json!(32));
        map.entry("font_color").or_insert(json!(""));
        map.entry("font_bg_color").or_insert(json!(""));
        map.entry("width").or_insert(json!(800));
        map.entry("height").or_insert(json!(600));
        map.entry("noise").or_insert(json!(false));

        // 归零的尺寸/字号回落到默认值
        for (key, default) in [("width", 800), ("height", 600), ("font_size", 32)] {
            let current = map.get(key).and_then(Value::as_i64).unwrap_or(0);
            if current <= 0 {
                map.insert(key.into(), json!(default));
            }
        }
    }
    Ok(value)
}

/// 保存配置（字段级 merge + 尺寸归一）。
pub async fn config_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    if !patch.is_object() {
        return Err(anyhow::anyhow!("配置必须是 JSON 对象"));
    }
    let mut patch = patch.clone();
    if let Value::Object(map) = &mut patch {
        for (key, default) in [("width", 800), ("height", 600), ("font_size", 32)] {
            if let Some(size) = map.get(key).and_then(Value::as_i64) {
                if size <= 0 {
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

/// 上传资源，返回相对 `public` 的路径。
///
/// `kind`：`font_path`（字体，只收 `.ttf`）或 `bg_image`（背景图）。
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
            format!("{UPLOAD_DIR}/title_font.ttf")
        }
        "bg_image" => {
            let ext = extension_of(file_name);
            if !IMAGE_EXTS.contains(&ext.as_str()) {
                return Err(ModelError::invalid("不支持的图片格式"));
            }
            let hash = super::attachment::md5_hex(file_name.as_bytes());
            format!("{UPLOAD_DIR}/{hash}.{ext}")
        }
        _ => return Err(ModelError::invalid("文件名无效")),
    };

    let full = public_dir.join(&relative);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| ModelError::invalid(format!("创建目录失败: {err}")))?;
    }
    std::fs::write(&full, bytes)
        .map_err(|err| ModelError::invalid(format!("保存文件失败: {err}")))?;
    Ok(relative)
}

/// 上传字体后把路径写回配置。
pub async fn set_font_path(pool: &MySqlPool, path: &str) -> anyhow::Result<()> {
    site_setting::save_section(pool, SETTING_KEY, &json!({ "font_path": path })).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, setting, test_pool};

    #[tokio::test]
    async fn config_roundtrip_with_defaults() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, SETTING_KEY).await.unwrap();

        let loaded = config_get(&pool).await.unwrap();
        assert_eq!(loaded["width"], json!(800));
        assert_eq!(loaded["height"], json!(600));
        assert_eq!(loaded["font_size"], json!(32));
        assert!(loaded["bg_images"].is_array());

        // 归零尺寸被回落
        let saved = config_save(
            &pool,
            &json!({"open": true, "width": 0, "height": 0, "font_size": 0}),
        )
        .await
        .unwrap();
        assert_eq!(saved["open"], json!(true));
        assert_eq!(saved["width"], json!(800));
        assert_eq!(saved["height"], json!(600));
        assert_eq!(saved["font_size"], json!(32));

        // 正常值保留
        let saved = config_save(&pool, &json!({"width": 1200, "height": 630}))
            .await
            .unwrap();
        assert_eq!(saved["width"], json!(1200));
        assert_eq!(saved["height"], json!(630));

        assert!(config_save(&pool, &json!([])).await.is_err());

        match backup {
            Some(raw) => setting::save_raw(&pool, SETTING_KEY, &raw).await.unwrap(),
            None => setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
    }

    #[test]
    fn uploads_font_and_image() {
        let dir = std::env::temp_dir().join(format!("rustcms-ti-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // 字体：只收 ttf，固定文件名
        assert!(upload(&dir, "font_path", "font.otf", b"x").is_err());
        let path = upload(&dir, "font_path", "my-font.TTF", b"font-bytes").unwrap();
        assert_eq!(path, "uploads/titleimage/title_font.ttf");
        assert!(dir.join(&path).is_file());

        // 背景图：按 md5 命名
        let path = upload(&dir, "bg_image", "bg.PNG", b"image-bytes").unwrap();
        assert!(path.starts_with("uploads/titleimage/"), "{path}");
        assert!(path.ends_with(".png"), "{path}");
        assert!(dir.join(&path).is_file());

        // 非法类型 / 空内容
        assert!(upload(&dir, "bg_image", "a.exe", b"x").is_err());
        assert!(upload(&dir, "unknown", "a.png", b"x").is_err());
        assert!(upload(&dir, "bg_image", "a.png", b"").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

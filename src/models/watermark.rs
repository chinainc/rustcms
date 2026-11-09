//! 水印插件。
//!
//! 配置存 `settings.watermark`。渲染能力：
//! - **图片水印**：[`render::draw`] 按九宫格位置 + 缩放比例 + 透明度，把水印图合成到
//! 目标图（alpha 混合，见 [`super::titleimage::image_ops`]）；
//! - **文字水印**：自带 TrueType 轮廓光栅化（[`font`]，`glyf` 表 + 非零环绕填充 +
//! 子采样抗锯齿），按 `text` / `size` / `color` / `opacity` 画字，与图片水印共用
//! 九宫格位置与最小尺寸规则；CFF/OTF 与缺字体等情形一律返回明确中文提示；
//! - [`apply_file`] 可直接把磁盘上的图片加水印后写回，[`render::preview_data_url`]
//! 给出 800×600 的 PNG 预览（与批量生成同一条渲染路径）。
//!
//! 位置编号按九宫格：1 左上、2 上中、3 右上、4 左中、5 居中、6 右中、
//! 7 左下、8 下中、9 右下（既有实现只显式判了 1/3/5/7，其余落到右下，这里补齐九宫格）。

pub mod font;
pub mod render;

use std::path::Path;

use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;

use super::titleimage::image_ops;
use super::{site_setting, ModelError};

// 供路由层/后台直接调用（当前只有测试在用，故屏蔽未使用告警）
#[allow(unused_imports)]
pub use render::{draw, draw_with_font, max_mark_size, position_of, preview_data_url};

/// 配置键。
pub const SETTING_KEY: &str = "watermark";

const UPLOAD_DIR: &str = "uploads/watermark";
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp"];

/// 读配置（补齐默认值）。
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

        // 归零值回落到默认（既有实现里就是 position=9 / opacity=100 / min_size=400）
        for (key, default) in [
            ("size", 20),
            ("position", 9),
            ("opacity", 100),
            ("min_size", 400),
        ] {
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
        for (key, default) in [
            ("size", 20),
            ("position", 9),
            ("opacity", 100),
            ("min_size", 400),
        ] {
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
            // 水印图必须能被图片引擎解码，避免「上传成功但渲染时静默无效果」
            image_ops::decode(bytes)
                .map_err(|err| ModelError::invalid(format!("水印图片无法解码：{err}")))?;
            format!("{UPLOAD_DIR}/watermark_image.{ext}")
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

/// 给 `public_dir` 下的图片文件加水印并**写回原文件**。
///
/// 扩展名会被保留（JPEG 写回 JPEG、PNG 写回 PNG），不会出现「内容与扩展名不符」。
pub async fn apply_file(
    public_dir: &Path,
    relative: &str,
    config: &Value,
) -> Result<(), ModelError> {
    let ext = extension_of(relative);
    if !image_ops::supports_extension(&ext) {
        return Err(ModelError::invalid(format!(
            "暂不支持处理 .{ext} 格式的图片（支持 png / jpg / bmp）"
        )));
    }
    let full = public_dir.join(relative.trim_start_matches('/'));
    let bytes = tokio::fs::read(&full)
        .await
        .map_err(|err| ModelError::invalid(format!("读取图片失败: {err}")))?;
    let target = image_ops::decode(&bytes).map_err(ModelError::invalid)?;

    // 图片水印读水印图、文字水印读字体文件；两者都缺时给出可执行的提示。
    let mut mark = None;
    let mut face = None;
    if config.get("type").and_then(Value::as_i64).unwrap_or(0) == 0 {
        let image_path = config
            .get("image_path")
            .and_then(Value::as_str)
            .unwrap_or("");
        let mark_bytes = tokio::fs::read(public_dir.join(image_path.trim_start_matches('/')))
            .await
            .map_err(|_| ModelError::invalid("水印图片未配置或无法读取"))?;
        mark = Some(image_ops::decode(&mark_bytes).map_err(ModelError::invalid)?);
    } else {
        let font_path = config
            .get("font_path")
            .and_then(Value::as_str)
            .unwrap_or("");
        let font_bytes = tokio::fs::read(public_dir.join(font_path.trim_start_matches('/')))
            .await
            .map_err(|_| ModelError::invalid("水印字体未配置或无法读取"))?;
        face = Some(font::Face::parse(&font_bytes).map_err(ModelError::invalid)?);
    }

    let output = render::draw_with_font(&target, config, mark.as_ref(), face.as_ref())
        .map_err(ModelError::invalid)?;
    let encoded = image_ops::encode(&output, &ext).map_err(ModelError::invalid)?;
    tokio::fs::write(&full, encoded)
        .await
        .map_err(|err| ModelError::invalid(format!("写回图片失败: {err}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::titleimage::image_ops::Image;
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

        let png = image_ops::encode(&Image::filled(2, 2, [255, 0, 0, 255]), "png").unwrap();
        let path = upload(&dir, "image_path", "logo.PNG", &png).unwrap();
        assert_eq!(path, "uploads/watermark/watermark_image.png");
        assert!(dir.join(&path).is_file());

        assert!(upload(&dir, "image_path", "a.exe", b"x").is_err());
        assert!(upload(&dir, "unknown", "a.png", b"x").is_err());
        assert!(upload(&dir, "image_path", "a.png", b"").is_err());
        // 扩展名像图片但内容不是图片：直接拦下（避免渲染时静默失败）
        let err = upload(&dir, "image_path", "bad.png", b"not-an-image").unwrap_err();
        assert!(err.to_string().contains("无法解码"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn applies_watermark_to_disk_file() {
        let dir = std::env::temp_dir().join(format!("rustcms-wm-apply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("uploads/watermark")).unwrap();

        // 准备水印图与目标图
        let mark = Image::filled(8, 8, [255, 255, 255, 255]);
        std::fs::write(
            dir.join("uploads/watermark/watermark_image.png"),
            image_ops::encode(&mark, "png").unwrap(),
        )
        .unwrap();
        let target = Image::filled(80, 80, [0, 0, 0, 255]);
        std::fs::write(
            dir.join("uploads/target.png"),
            image_ops::encode(&target, "png").unwrap(),
        )
        .unwrap();

        let config = json!({
            "type": 0, "size": 20, "position": 9, "opacity": 100, "min_size": 1,
            "image_path": "uploads/watermark/watermark_image.png"
        });
        apply_file(&dir, "uploads/target.png", &config)
            .await
            .unwrap();

        // 写回后仍是可解码的 PNG，且右下角已被覆盖
        let bytes = std::fs::read(dir.join("uploads/target.png")).unwrap();
        let image = image_ops::decode(&bytes).unwrap();
        assert_eq!((image.width, image.height), (80, 80));
        assert_eq!(image.get(0, 0), [0, 0, 0, 255]);
        // 8×8 的水印贴在右下角（留白 80/15 = 5）：覆盖 67..75
        assert_eq!(image.get(70, 70), [255, 255, 255, 255]);

        // 不支持的格式要报明确错误
        std::fs::write(dir.join("uploads/a.webp"), b"RIFF").unwrap();
        let err = apply_file(&dir, "uploads/a.webp", &config)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("webp"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

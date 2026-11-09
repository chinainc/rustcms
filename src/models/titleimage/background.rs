//! 标题图片的背景配色与位图生成（纯图像部分，不依赖数据库）。
//!
//! 对应既有约定的 `sumTitle` / `RandColor` / `RandDeepColor` /
//! `makeBackground`：
//! - 先由标题算出 32 位数字摘要，再从摘要里取数字段推导出「基础色」与「深色」；
//! - 没有背景图时，画一条 6 像素高的深色细带（可叠噪声），再放大成目标尺寸
//! —— 这正是 既有实现的做法：放大后得到柔和的纯色渐变底；
//! - 有背景图时按等比裁剪（cover）铺满目标尺寸。
//!
//! 文字本身（标题）不在这里绘制，由 [`super::draw`] 负责（TrueType 光栅化 + 折行）。

use serde_json::Value;

use super::super::attachment::md5_hex;
use super::image_ops::{self, Image};

/// 标题的 32 位数字摘要（遵循既有约定：`md5` 后把大于 `'9'` 的字节减 49）。
pub fn title_sum(title: &str) -> String {
    md5_hex(title.as_bytes())
        .bytes()
        .map(|byte| char::from(if byte > 57 { byte - 49 } else { byte }))
        .collect()
}

/// 从摘要里取一段数字（对应既有约定）。
fn sum_number(sum: &str, addon: usize) -> i64 {
    sum.get(addon..addon + 9)
        .and_then(|part| part.parse::<i64>().ok())
        .unwrap_or(0)
}

/// 基础随机色（遵循既有约定）。
fn random_color_at(sum: &str, addon: usize) -> [u8; 4] {
    let number = sum_number(sum, addon);
    let red = number % 255;
    let green = number / 1000 % 255;
    let blue = if red + green > 400 {
        0
    } else {
        (400 - green - red).min(255)
    };
    [red as u8, green as u8, blue as u8, 255]
}

/// 在基础色上压暗一档（遵循既有约定）。
fn deep_color_at(sum: &str, addon: usize) -> [u8; 4] {
    let base = random_color_at(sum, addon);
    let increase = 30 + sum_number(sum, 22 - addon.min(22)) % 255;
    let channel = |value: u8| -> u8 { (value as i64 - increase).abs().min(255) as u8 };
    [channel(base[0]), channel(base[1]), channel(base[2]), 255]
}

/// 按标题取基础色。
pub fn random_color(title: &str) -> [u8; 4] {
    random_color_at(&title_sum(title), 0)
}

/// 按标题取深色（无背景图时的默认底色）。
pub fn deep_color(title: &str) -> [u8; 4] {
    deep_color_at(&title_sum(title), 0)
}

/// 生成标题图片的背景位图（遵循既有约定，整体用双线性缩放）。
///
/// - 传了 `background` 且能解码：等比裁剪（cover）到配置尺寸；
/// - 否则：按标题摘要生成 6 像素高的深色色带（可选噪声），再放大到配置尺寸。
pub fn background_image(
    config: &Value,
    title: &str,
    background: Option<&[u8]>,
) -> Result<Image, String> {
    let width = config
        .get("width")
        .and_then(Value::as_u64)
        .unwrap_or(800)
        .clamp(1, 4096) as u32;
    let height = config
        .get("height")
        .and_then(Value::as_u64)
        .unwrap_or(600)
        .clamp(1, 4096) as u32;

    if let Some(bytes) = background {
        if let Ok(image) = image_ops::decode(bytes) {
            return Ok(image_ops::resize_cover(&image, width, height));
        }
        // 背景图解不开时退回纯色底（遵循既有约定：忽略该背景图继续生成）
    }

    let sum = title_sum(title);
    let bytes = sum.as_bytes();
    let base = deep_color_at(&sum, 0);
    let band_height = 6u32;
    let band_width = ((width as f32 / height as f32) * band_height as f32)
        .round()
        .max(1.0) as u32;
    let mut band = Image::filled(band_width, band_height, base);

    // 噪声：确定性生成（同一标题每次结果一致），便于缓存与测试
    if config
        .get("noise")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && !bytes.is_empty()
    {
        for y in 0..band_height as usize {
            if bytes[y % bytes.len()] % 2 != 0 {
                continue;
            }
            for x in 0..band_width as usize {
                let index = (x * 7 + y * 13) % bytes.len();
                if bytes[index] % 3 != 0 {
                    continue;
                }
                let addon = (bytes[index] as usize) % 22;
                band.put(x as u32, y as u32, deep_color_at(&sum, addon));
            }
        }
    }

    Ok(image_ops::resize(&band, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn title_sum_and_colors_are_stable() {
        let sum = title_sum("欢迎使用 RustCMS");
        assert_eq!(sum.chars().count(), 32);
        // 摘要必须全是 ASCII 数字（下标取值依赖这一点）
        assert!(sum.chars().all(|ch| ch.is_ascii_digit()), "{sum}");
        // 同一标题必须稳定（背景色会随缓存复用）
        assert_eq!(sum, title_sum("欢迎使用 RustCMS"));

        let base = random_color("rustcms");
        let deep = deep_color("rustcms");
        assert_eq!(base[3], 255);
        assert_eq!(deep[3], 255);
        assert_eq!(base, random_color("rustcms"));
        assert_eq!(deep, deep_color("rustcms"));
        // 不同标题一般得到不同底色
        assert_ne!(random_color("rustcms"), random_color("示例站点"));
    }

    #[test]
    fn background_image_uses_config_size() {
        let config = json!({"width": 64, "height": 32, "noise": false});
        let image = background_image(&config, "测试标题", None).unwrap();
        assert_eq!((image.width, image.height), (64, 32));
        // 纯色底：不透明
        assert_eq!(image.get(0, 0)[3], 255);
        // 相同输入必须得到相同像素（可缓存）
        let again = background_image(&config, "测试标题", None).unwrap();
        assert_eq!(image, again);

        // 噪声开关不应改变尺寸
        let noisy = background_image(
            &json!({"width": 32, "height": 16, "noise": true}),
            "测试标题",
            None,
        )
        .unwrap();
        assert_eq!((noisy.width, noisy.height), (32, 16));
    }

    #[test]
    fn background_image_crops_decodable_background() {
        // 4×2 的纯色 PNG，等比裁剪到 10×10
        let mut source = Image::filled(4, 2, [10, 200, 30, 255]);
        source.put(0, 0, [255, 0, 0, 255]);
        let bytes = image_ops::encode(&source, "png").unwrap();
        let image = background_image(
            &json!({"width": 10, "height": 10}),
            "带背景图",
            Some(&bytes),
        )
        .unwrap();
        assert_eq!((image.width, image.height), (10, 10));
        assert_eq!(image.get(0, 0)[3], 255);

        // 背景图解不开时回退纯色，不报错
        let fallback = background_image(
            &json!({"width": 8, "height": 8}),
            "坏背景",
            Some(b"not-an-image"),
        )
        .unwrap();
        assert_eq!((fallback.width, fallback.height), (8, 8));
    }
}

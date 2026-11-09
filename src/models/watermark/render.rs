//! 水印渲染的纯图像部分（不依赖数据库/异步 IO）。
//!
//! 单独拆一个模块的好处：这部分只依赖 [`image_ops`] 与 [`font`]，可以脱离数据库与网络
//! 单独编译、单独跑单元测试；[`super::apply_file`] 这类带磁盘 IO 的流程再在这之上组合。

use serde_json::{json, Value};

use super::super::titleimage::image_ops::{self, Image};
use super::font;

/// 九宫格位置对应的左上角坐标。
///
/// `edge` 是四周留白（既有实现：`最小(图像宽/15, 50)`）。水印比画布还大时返回负坐标，
/// 交给 [`Image::composite`] 自动裁掉越界部分。
pub fn position_of(
    position: i64,
    canvas_width: u32,
    canvas_height: u32,
    mark_width: u32,
    mark_height: u32,
    edge: u32,
) -> (i64, i64) {
    let left = edge as i64;
    let top = edge as i64;
    let right = canvas_width as i64 - edge as i64 - mark_width as i64;
    let bottom = canvas_height as i64 - edge as i64 - mark_height as i64;
    let center_x = (canvas_width as i64 - mark_width as i64) / 2;
    let center_y = (canvas_height as i64 - mark_height as i64) / 2;
    match position {
        1 => (left, top),
        2 => (center_x, top),
        3 => (right, top),
        4 => (left, center_y),
        5 => (center_x, center_y),
        6 => (right, center_y),
        7 => (left, bottom),
        8 => (center_x, bottom),
        // 9 与未知值都是右下（遵循既有约定默认分支）
        _ => (right, bottom),
    }
}

/// 水印的最大尺寸：宽高各取目标图的 `size`%（既有实现：`size` 默认 20）。
pub fn max_mark_size(canvas_width: u32, canvas_height: u32, size: u32) -> (u32, u32) {
    let size = size.clamp(1, 200);
    (
        (canvas_width * size / 100).max(1),
        (canvas_height * size / 100).max(1),
    )
}

/// 把水印叠加到目标图上，返回新图（不修改入参）。
///
/// `mark`：已经解码好的水印图（图片水印必填）。文字水印请用 [`draw_with_font`]。
pub fn draw(target: &Image, config: &Value, mark: Option<&Image>) -> Result<Image, String> {
    draw_with_font(target, config, mark, None)
}

/// 支持文字水印的完整版本。
///
/// `type = 0` 用 `mark`（水印图片）；其它值用 `face`（已解析的 TrueType 字体）按
/// `text` / `size` / `color` 画字 —— 位置仍按 既有实现的估算宽度（ASCII 半字号、中文全字号）
/// 来定，这样预览与批量生成的落点完全一致。
///
/// 与既有实现的差异：既有实现用 `imaging.Fit`（水印图小于上限时会**放大**），这里用
/// [`image_ops::resize_fit`]（只缩不放），避免把小 logo 拉糊。
pub fn draw_with_font(
    target: &Image,
    config: &Value,
    mark: Option<&Image>,
    face: Option<&font::Face>,
) -> Result<Image, String> {
    let min_size = config
        .get("min_size")
        .and_then(Value::as_i64)
        .unwrap_or(400)
        .max(1) as u32;
    if target.width < min_size && target.height < min_size {
        return Err(format!(
            "图片尺寸 {}×{} 小于水印最小尺寸 {min_size}，已跳过",
            target.width, target.height
        ));
    }

    let size = config
        .get("size")
        .and_then(Value::as_i64)
        .unwrap_or(20)
        .clamp(1, 200) as u32;
    let position = config.get("position").and_then(Value::as_i64).unwrap_or(9);
    let opacity = config
        .get("opacity")
        .and_then(Value::as_i64)
        .unwrap_or(100)
        .clamp(0, 100) as u32;
    let edge = (target.width / 15).min(50);

    if config.get("type").and_then(Value::as_i64).unwrap_or(0) != 0 {
        return draw_text(target, config, face, size, position, opacity, edge);
    }

    let Some(source) = mark else {
        return Err("水印图片未配置或无法读取".to_string());
    };
    let (max_width, max_height) = max_mark_size(target.width, target.height, size);
    let scaled = image_ops::resize_fit(source, max_width, max_height);
    let (x, y) = position_of(
        position,
        target.width,
        target.height,
        scaled.width,
        scaled.height,
        edge,
    );

    // 0~100 的百分比换算成 0~255 的合成透明度
    let alpha = (opacity * 255 / 100) as u8;
    let mut output = target.clone();
    output.composite(&scaled, x, y, alpha);
    Ok(output)
}

/// 文字水印：按 `text` 估算的宽高定位，再把字画在基线上。
fn draw_text(
    target: &Image,
    config: &Value,
    face: Option<&font::Face>,
    size: u32,
    position: i64,
    opacity: u32,
    edge: u32,
) -> Result<Image, String> {
    let text = config
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return Err("水印文字未配置".to_string());
    }
    let Some(face) = face else {
        return Err("水印字体未配置或无法读取".to_string());
    };

    // 与既有接口契约一致：中文按 1 个字号、英文按 0.5 个字号估算文字宽度
    let width: u32 = text
        .chars()
        .map(|ch| if ch.is_ascii() { (size / 2).max(1) } else { size })
        .sum();
    let mark_width = width + width / 10;
    let mark_height = size + size / 10;
    let (x, y) = position_of(
        position,
        target.width,
        target.height,
        mark_width,
        mark_height,
        edge,
    );

    let color = parse_hex_color(config.get("color").and_then(Value::as_str).unwrap_or(""));
    let mut output = target.clone();
    // 基线落在水印框底边（与 SVG 预览 `y + size` 同口径）
    font::draw_text(
        &mut output,
        face,
        text,
        x as f32,
        (y + size as i64) as f32,
        size as f32,
        color,
        opacity as f32 / 100.0,
    )?;
    Ok(output)
}

/// `#RRGGBB` / `#RGB` → RGBA；非法值回白色（与既有约定同口径）。
fn parse_hex_color(value: &str) -> [u8; 4] {
    let text = value.trim().trim_start_matches('#');
    let nibble = |byte: u8| -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    };
    let channels: Option<[u8; 3]> = (|| {
        let bytes = text.as_bytes();
        let mut out = [0u8; 3];
        match bytes.len() {
            6 => {
                for (index, slot) in out.iter_mut().enumerate() {
                    let high = nibble(*bytes.get(index * 2)?)?;
                    let low = nibble(*bytes.get(index * 2 + 1)?)?;
                    *slot = high * 16 + low;
                }
                Some(out)
            }
            3 => {
                for (index, slot) in out.iter_mut().enumerate() {
                    *slot = nibble(*bytes.get(index)?)? * 17;
                }
                Some(out)
            }
            _ => None,
        }
    })();
    match channels {
        Some([red, green, blue]) => [red, green, blue, 255],
        None => [255, 255, 255, 255],
    }
}

/// 预览图最长边（后台页面直接内联展示，体积必须受控）。
pub const PREVIEW_MAX_SIDE: u32 = 800;

/// 预览 data URL 的字节上限：超出就逐级缩小重编码，避免后台页面卡死。
const PREVIEW_MAX_BYTES: usize = 1_500_000;

/// 生成水印预览图，返回 `data:image/png;base64,...`。
///
/// 画布固定 `800×600`（最长边 = [`PREVIEW_MAX_SIDE`]）；若编码结果仍然过大
/// （例如水印本身是满屏噪点图），会按 1/2 逐级缩小直到体积受控。
///
/// `mark_bytes` 给图片水印、`font_bytes` 给文字水印，与批量生成走同一条渲染路径
/// （预览看到的就是最终效果，不会出现「预览能看、批量没效果」）。
pub fn preview_data_url(
    config: &Value,
    mark_bytes: Option<&[u8]>,
    font_bytes: Option<&[u8]>,
) -> Result<String, String> {
    // 预览不受「未开启」和「最小尺寸」限制，也不落盘
    let mut preview = config.clone();
    if let Value::Object(map) = &mut preview {
        map.insert("open".to_string(), json!(true));
        map.insert("min_size".to_string(), json!(1));
    }
    let mark = match mark_bytes {
        Some(bytes) => Some(image_ops::decode(bytes)?),
        None => None,
    };
    let face = match font_bytes {
        Some(bytes) => Some(font::Face::parse(bytes)?),
        None => None,
    };
    let canvas = Image::filled(PREVIEW_MAX_SIDE, PREVIEW_MAX_SIDE * 3 / 4, [96, 125, 139, 255]);
    let mut image = draw_with_font(&canvas, &preview, mark.as_ref(), face.as_ref())?;
    let mut png = image_ops::encode(&image, "png")?;
    while png.len() > PREVIEW_MAX_BYTES && image.width > 100 && image.height > 75 {
        image = image_ops::resize(&image, image.width / 2, image.height / 2);
        png = image_ops::encode(&image, "png")?;
    }
    Ok(format!("data:image/png;base64,{}", base64_encode(&png)))
}

/// 标准 base64 编码（带 `=` 填充）。
pub fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len() / 3 * 4 + 4);
    for chunk in data.chunks(3) {
        let first = chunk[0] as u32;
        let second = chunk.get(1).copied().unwrap_or(0) as u32;
        let third = chunk.get(2).copied().unwrap_or(0) as u32;
        let combined = (first << 16) | (second << 8) | third;
        out.push(TABLE[(combined >> 18) as usize & 0x3F] as char);
        out.push(TABLE[(combined >> 12) as usize & 0x3F] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(combined >> 6) as usize & 0x3F] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[combined as usize & 0x3F] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_grid_positions() {
        // 800×600 画布、100×50 水印、四周留白 50
        let (width, height, mark_width, mark_height, edge) = (800, 600, 100, 50, 50);
        let position =
            |value: i64| position_of(value, width, height, mark_width, mark_height, edge);
        assert_eq!(position(1), (50, 50)); // 左上
        assert_eq!(position(2), (350, 50)); // 上中
        assert_eq!(position(3), (650, 50)); // 右上
        assert_eq!(position(4), (50, 275)); // 左中
        assert_eq!(position(5), (350, 275)); // 居中
        assert_eq!(position(6), (650, 275)); // 右中
        assert_eq!(position(7), (50, 500)); // 左下
        assert_eq!(position(8), (350, 500)); // 下中
        assert_eq!(position(9), (650, 500)); // 右下
        assert_eq!(position(0), (650, 500)); // 未知值按右下（遵循既有约定）

        // 水印比画布还大：坐标变负，合成时自动裁掉越界部分
        let (x, y) = position_of(9, 100, 100, 400, 400, 6);
        assert!(x < 0 && y < 0, "({x}, {y})");
    }

    #[test]
    fn mark_size_follows_percentage() {
        assert_eq!(max_mark_size(1000, 500, 20), (200, 100));
        // 至少 1 像素，避免极小的水印图被算成 0
        assert_eq!(max_mark_size(10, 10, 1), (1, 1));
        // size 越界被夹取
        assert_eq!(max_mark_size(1000, 1000, 500), (2000, 2000));
    }

    #[test]
    fn draw_image_watermark() {
        let target = Image::filled(100, 100, [0, 0, 0, 255]);
        let mark = Image::filled(50, 50, [255, 255, 255, 255]);
        let config = json!({
            "type": 0, "size": 20, "position": 9, "opacity": 100, "min_size": 1
        });

        let output = draw(&target, &config, Some(&mark)).unwrap();
        assert_eq!((output.width, output.height), (100, 100));
        // 左上角未受影响
        assert_eq!(output.get(0, 0), [0, 0, 0, 255]);
        // 右下角被水印覆盖（20×20，留白 100/15 = 6）
        assert_eq!(output.get(84, 84), [255, 255, 255, 255]);
        // 水印区域之外仍是黑底
        assert_eq!(output.get(50, 50), [0, 0, 0, 255]);

        // 半透明：白水印压在黑底上应得到中灰
        let half = json!({
            "type": 0, "size": 20, "position": 5, "opacity": 50, "min_size": 1
        });
        let output = draw(&target, &half, Some(&mark)).unwrap();
        let pixel = output.get(50, 50);
        assert!(pixel[0] > 100 && pixel[0] < 160, "{pixel:?}");
        assert_eq!(pixel[3], 255);
    }

    #[test]
    fn draw_skips_small_images_and_reports_missing_assets() {
        let target = Image::filled(100, 100, [0, 0, 0, 255]);
        let mark = Image::filled(10, 10, [255, 255, 255, 255]);

        // 小于最小尺寸：明确报错（调用方据此跳过，不写坏文件）
        let small = json!({"type": 0, "min_size": 400});
        let err = draw(&target, &small, Some(&mark)).unwrap_err();
        assert!(err.contains("已跳过"), "{err}");

        // 文字水印没给字体：明确提示，不能静默「什么都没画」
        let text = json!({"type": 1, "text": "水印", "min_size": 1});
        let err = draw(&target, &text, None).unwrap_err();
        assert!(err.contains("字体"), "{err}");

        // 文字水印缺文字
        let Some(face) = system_face() else {
            return;
        };
        let empty = json!({"type": 1, "text": "  ", "min_size": 1});
        let err = draw_with_font(&target, &empty, None, Some(&face)).unwrap_err();
        assert!(err.contains("水印文字未配置"), "{err}");

        // 图片水印缺素材
        let image = json!({"type": 0, "min_size": 1});
        assert!(draw(&target, &image, None).is_err());
    }

    /// 系统字体（找不到就跳过依赖字体的用例）。
    fn system_face() -> Option<font::Face> {
        const CANDIDATES: &[&str] = &[
            "C:/Windows/Fonts/arial.ttf",
            "C:/Windows/Fonts/segoeui.ttf",
            "C:/Windows/Fonts/msyh.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ];
        CANDIDATES
            .iter()
            .find_map(|path| std::fs::read(path).ok())
            .and_then(|bytes| font::Face::parse(&bytes).ok())
    }

    #[test]
    fn draw_text_watermark_bottom_right() {
        let Some(face) = system_face() else {
            return;
        };
        let target = Image::filled(400, 300, [0, 0, 0, 255]);
        let config = json!({
            "type": 1,
            "text": "TMA",
            "size": 40,
            "position": 9,
            "opacity": 100,
            "min_size": 1,
            "color": "#ffffff",
        });
        let output = draw_with_font(&target, &config, None, Some(&face)).unwrap();

        // 右下角必须真的有字：统计四个象限的亮像素
        let mut quadrants = [0usize; 4];
        for y in 0..output.height {
            for x in 0..output.width {
                if output.get(x, y)[0] > 40 {
                    let index = (if y >= output.height / 2 { 2 } else { 0 })
                        + usize::from(x >= output.width / 2);
                    quadrants[index] += 1;
                }
            }
        }
        assert!(quadrants[3] > 50, "{quadrants:?}");
        assert_eq!(quadrants[0] + quadrants[1] + quadrants[2], 0, "{quadrants:?}");
    }

    #[test]
    fn draw_text_watermark_honours_color_and_opacity() {
        let Some(face) = system_face() else {
            return;
        };
        let target = Image::filled(300, 200, [0, 0, 0, 255]);
        let config = json!({
            "type": 1,
            "text": "A",
            "size": 60,
            "position": 5,
            "opacity": 50,
            "min_size": 1,
            "color": "#ff0000",
        });
        let output = draw_with_font(&target, &config, None, Some(&face)).unwrap();
        let brightest = (0..output.height)
            .flat_map(|y| (0..output.width).map(move |x| (x, y)))
            .map(|(x, y)| output.get(x, y))
            .max_by_key(|pixel| pixel[0])
            .unwrap();
        // 红色、半透明压在黑底上：红通道约 128，绿蓝接近 0
        assert!(brightest[0] > 90 && brightest[0] < 170, "{brightest:?}");
        assert!(brightest[1] < 40 && brightest[2] < 40, "{brightest:?}");
        assert_eq!(brightest[3], 255);
    }

    #[test]
    fn preview_returns_png_data_url() {
        let mark = Image::filled(20, 20, [255, 0, 0, 255]);
        let bytes = image_ops::encode(&mark, "png").unwrap();
        let config = json!({"type": 0, "size": 20, "position": 9, "opacity": 100});
        let url = preview_data_url(&config, Some(&bytes), None).unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{}", &url[..40]);
        assert!(url.len() > 100);
    }

    #[test]
    fn preview_stays_small_even_with_noisy_watermark() {
        // 满屏噪点水印：PNG 几乎不可压缩，必须触发「缩小重编码」把体积压下来
        let mut noise = Image::new(800, 600);
        let mut state = 7u32;
        for y in 0..600u32 {
            for x in 0..800u32 {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                noise.put(
                    x,
                    y,
                    [
                        (state >> 24) as u8,
                        (state >> 16) as u8,
                        (state >> 8) as u8,
                        255,
                    ],
                );
            }
        }
        let bytes = image_ops::encode(&noise, "png").unwrap();
        let config = json!({"type": 0, "size": 200, "position": 5, "opacity": 100});
        let url = preview_data_url(&config, Some(&bytes), None).unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{}", &url[..40]);
        // 上限 1.5MB，base64 编码后约 2MB
        assert!(url.len() <= 2_100_000, "预览体积过大: {}", url.len());
    }

    #[test]
    fn preview_renders_text_watermark_when_font_given() {
        let Some(bytes) = std::fs::read("C:/Windows/Fonts/arial.ttf")
            .ok()
            .or_else(|| std::fs::read("C:/Windows/Fonts/segoeui.ttf").ok())
        else {
            return;
        };
        let config = json!({
            "type": 1,
            "text": "水印 Watermark",
            "size": 36,
            "position": 9,
            "opacity": 100,
            "color": "#ffffff",
        });
        let url = preview_data_url(&config, None, Some(&bytes)).unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{}", &url[..40]);
        assert!(url.len() > 1000, "{}", url.len());
    }

    #[test]
    fn hex_colors_parse_like_go() {
        assert_eq!(parse_hex_color("#ff0000"), [255, 0, 0, 255]);
        assert_eq!(parse_hex_color("00ff00"), [0, 255, 0, 255]);
        assert_eq!(parse_hex_color("#f0a"), [255, 0, 170, 255]);
        // 非法值一律回白色（既有实现的 hex_color_or 默认值）
        assert_eq!(parse_hex_color(""), [255, 255, 255, 255]);
        assert_eq!(parse_hex_color("#zzz"), [255, 255, 255, 255]);
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(&[0xFF, 0x00, 0x10]), "/wAQ");
    }
}

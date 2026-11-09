//! 标题图片的文字光栅化（遵循既有约定）。
//!
//! 分工：[`super::background`] 只负责底图（背景图或按标题生成的纯色渐变），这里把标题
//! 画上去，得到一张**真正的位图**（此前只能由路由层输出内嵌 `@font-face` 的 SVG，
//! 位图管线本身没有文字）。
//!
//! 与常规做法的差异（都是「没有对应依赖」导致的，不影响成品尺寸与观感）：
//! - 常规做法用成熟的字体光栅化库，这里用 [`super::super::watermark::font`]
//! 自带的 TrueType 光栅化（不做 hinting / kerning，边缘有亚像素差别）；
//! - 常规做法用中文分词决定折行点，这里按「CJK 逐字、ASCII 连写」切分，
//! 折行位置可能略有不同，但同一行绝不超出可用宽度；
//! - 空颜色值按既有约定得到全透明（等于不画字），这里留空按白色处理，
//! 与本模块 SVG 预览的默认色一致。

use std::path::Path;

use serde_json::Value;

use super::super::watermark::font::{self, Face};
use super::background;
use super::image_ops::Image;

/// 行高倍数（既有实现：`realSize * 1.6`）。
const LINE_HEIGHT: f32 = 1.6;
/// 左右留白（既有实现：`gap := 100`）。
const SIDE_GAP: u32 = 100;
/// 自动字号上限（既有实现：`maxSize := 100`）。
const MAX_FONT_SIZE: f32 = 100.0;
/// 配置没给字号时的缺省值（既有实现：`fontSize < 16` 时按 32）。
const DEFAULT_FONT_SIZE: f32 = 32.0;
/// 文字默认色（配置留空时）。
const DEFAULT_FONT_COLOR: [u8; 4] = [255, 255, 255, 255];

/// 解析 `#rgb` / `#rrggbb` / `#rrggbbaa`（也接受不带 `#`）。
pub fn parse_color(text: &str) -> Option<[u8; 4]> {
    let hex = text.trim().trim_start_matches('#');
    let digits: Vec<u8> = hex
        .chars()
        .map(|ch| ch.to_digit(16).map(|value| value as u8))
        .collect::<Option<Vec<u8>>>()?;
    match digits.len() {
        3 => Some([digits[0] * 17, digits[1] * 17, digits[2] * 17, 255]),
        6 => Some([
            digits[0] * 16 + digits[1],
            digits[2] * 16 + digits[3],
            digits[4] * 16 + digits[5],
            255,
        ]),
        8 => Some([
            digits[0] * 16 + digits[1],
            digits[2] * 16 + digits[3],
            digits[4] * 16 + digits[5],
            digits[6] * 16 + digits[7],
        ]),
        _ => None,
    }
}

/// 配置里的画布尺寸（与 [`super::background`] 的口径一致）。
fn canvas_size(config: &Value) -> (u32, u32) {
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
    (width, height)
}

/// 配置里的基础字号（小于 16 或没配按 32，与既有实现的构造函数一致）。
pub fn base_font_size(config: &Value) -> f32 {
    match config.get("font_size").and_then(Value::as_f64) {
        Some(size) if size >= 16.0 => size as f32,
        _ => DEFAULT_FONT_SIZE,
    }
}

/// 按标题字数自动算字号：`可用宽度 / 字数`，再夹到 `[基础字号, 100]`。
pub fn auto_font_size(face: &Face, title: &str, base: f32, width: u32) -> f32 {
    let _ = face; // 字号只与字数有关（既有实现也是这样估的），保留参数便于将来按字宽细算
    let max_text_width = (width.saturating_sub(SIDE_GAP)).max(1) as f32;
    let count = title.chars().count().max(1) as f32;
    let size = max_text_width / count;
    size.clamp(base.min(MAX_FONT_SIZE).max(1.0), MAX_FONT_SIZE)
}

/// 折行用的切分：CJK 逐字、ASCII/数字连写成一个词，换行符自成一个「强制断行」标记。
///
/// 返回的各段按顺序拼接起来（去掉换行符）就是原文 —— 折行只决定断点，不改字。
pub fn split_words(title: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut ascii_run = false;

    for ch in title.chars() {
        if ch == '\n' || ch == '\r' {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            words.push("\n".to_string());
            ascii_run = false;
            continue;
        }
        let is_ascii_word = ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '\'';
        if is_ascii_word {
            word.push(ch);
            ascii_run = true;
            continue;
        }
        if ascii_run {
            words.push(std::mem::take(&mut word));
            ascii_run = false;
        }
        if ch.is_whitespace() {
            // 空格并到前一个词上（与既有实现的 contain 语义一致：原字符不丢）
            match words.last_mut() {
                Some(last) if last != "\n" => last.push(ch),
                _ => words.push(ch.to_string()),
            }
            continue;
        }
        word.push(ch);
        words.push(std::mem::take(&mut word));
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// 按 `max_width` 折行；`\n` 强制断行。
pub fn wrap_lines(face: &Face, title: &str, size: f32, max_width: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut used = 0.0f32;

    for word in split_words(title) {
        if word == "\n" {
            lines.push(std::mem::take(&mut current));
            used = 0.0;
            continue;
        }
        let width = face.text_width(&word, size);
        if used + width > max_width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            used = 0.0;
        }
        current.push_str(&word);
        used += width;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// 把标题画到画布上（基线定位、自动字号、居中折行、可选文字底色带，逐项遵循既有约定）。
pub fn draw_title(
    image: &mut Image,
    face: &Face,
    config: &Value,
    title: &str,
) -> Result<(), String> {
    let (width, height) = canvas_size(config);
    let size = auto_font_size(face, title, base_font_size(config), width);
    let max_text_width = (width.saturating_sub(SIDE_GAP)).max(1) as f32;
    let lines = wrap_lines(face, title, size, max_text_width);

    let color = config
        .get("font_color")
        .and_then(Value::as_str)
        .and_then(parse_color)
        .unwrap_or(DEFAULT_FONT_COLOR);

    // 既有实现的整数运算顺序：int(size/1.5) + int((lineCount-1)*size*1.6)/2
    let step = (size * LINE_HEIGHT) as i64;
    let head_gap = (size / 1.5) as i64;
    let extra = (((lines.len() as f32 - 1.0) * size) * LINE_HEIGHT) as i64;
    let start_y = height as i64 / 2 - (head_gap + extra / 2) + size as i64;

    // 文字底色带（配置了 font_bg_color 才画）
    if let Some(band) = config
        .get("font_bg_color")
        .and_then(Value::as_str)
        .and_then(parse_color)
    {
        let band_top = start_y - size as i64 - size as i64 / 2;
        let band_bottom =
            start_y + step * (lines.len() as i64 - 1) + (size * LINE_HEIGHT / 2.0) as i64;
        for y in band_top..band_bottom {
            for x in 0..width as i64 {
                image.blend(x, y, band, 255);
            }
        }
    }

    for (index, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let line_width = face.text_width(line, size);
        let origin_x = (width as f32 - line_width) / 2.0;
        let baseline = start_y as f32 + index as f32 * step as f32;
        font::draw_text(image, face, line, origin_x, baseline, size, color, 1.0)?;
    }
    Ok(())
}

/// 底图 + 文字，一步出图。`face` 为 `None` 时只出底图（没有字体就别硬画）。
pub fn render(
    config: &Value,
    title: &str,
    face: Option<&Face>,
    background: Option<&[u8]>,
) -> Result<Image, String> {
    let mut image = background::background_image(config, title, background)?;
    if let Some(face) = face {
        if !title.trim().is_empty() {
            draw_title(&mut image, face, config, title)?;
        }
    }
    Ok(image)
}

/// 载入标题图字体：优先配置里的 `font_path`（相对 `public`），否则退回系统字体。
///
/// 与既有约定一个思路 —— 站长没上传字体时，也要能出带字的图。
pub fn load_face(public_dir: &Path, config: &Value) -> Option<Face> {
    let configured = config
        .get("font_path")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !configured.trim().is_empty() {
        let relative = configured.trim().trim_start_matches('/');
        if let Ok(bytes) = std::fs::read(public_dir.join(relative)) {
            if let Ok(face) = Face::parse(&bytes) {
                return Some(face);
            }
        }
        // 配置的字体坏了就继续找系统字体（遵循既有约定：解析失败视为没有自定义字体）
    }
    for path in SYSTEM_FONTS {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(face) = Face::parse(&bytes) {
                return Some(face);
            }
        }
    }
    None
}

/// 系统字体候选（优先带中文字形的，其次通用西文）。
const SYSTEM_FONTS: &[&str] = &[
    "C:/Windows/Fonts/msyh.ttc",
    "C:/Windows/Fonts/simhei.ttf",
    "C:/Windows/Fonts/simsun.ttc",
    "C:/Windows/Fonts/arial.ttf",
    "C:/Windows/Fonts/segoeui.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 找一个可用字体（找不到就跳过用例，不同机器路径不同）。
    fn system_face() -> Option<Face> {
        SYSTEM_FONTS
            .iter()
            .find_map(|path| std::fs::read(path).ok())
            .and_then(|bytes| Face::parse(&bytes).ok())
    }

    #[test]
    fn parses_hex_colors() {
        assert_eq!(parse_color("#fff"), Some([255, 255, 255, 255]));
        assert_eq!(parse_color("ff0000"), Some([255, 0, 0, 255]));
        assert_eq!(parse_color("#1a2b3c"), Some([0x1a, 0x2b, 0x3c, 255]));
        assert_eq!(parse_color("#1a2b3c80"), Some([0x1a, 0x2b, 0x3c, 0x80]));
        assert_eq!(parse_color(""), None);
        assert_eq!(parse_color("#12"), None);
        assert_eq!(parse_color("zzzzzz"), None);
    }

    #[test]
    fn splits_words_without_losing_text() {
        let title = "RustCMS 1.0 发布：性能提升 20%";
        let words = split_words(title);
        assert_eq!(words.concat(), title);
        // ASCII 连写成一个词，CJK 逐字
        assert!(words.iter().any(|word| word.starts_with("RustCMS")));
        assert!(words.iter().any(|word| word == "发"));
        // 换行符单独成标记
        let words = split_words("第一行\n第二行");
        assert!(words.contains(&"\n".to_string()));
        assert_eq!(words.concat(), "第一行\n第二行");
    }

    #[test]
    fn auto_size_shrinks_for_long_titles() {
        let Some(face) = system_face() else {
            return;
        };
        let short = auto_font_size(&face, "标题", 32.0, 800);
        let long = auto_font_size(&face, &"标题".repeat(40), 32.0, 800);
        assert!(short <= MAX_FONT_SIZE && short >= 32.0);
        assert!(long >= 32.0, "字号不得低于基础字号：{long}");
        assert!(long < short, "字数多时字号应变小：{long} vs {short}");
        // 字数极多时夹到基础字号，不再继续变小（与既有实现的 minSize 一致）
        let huge = auto_font_size(&face, &"字".repeat(500), 32.0, 800);
        assert!((huge - 32.0).abs() < f32::EPSILON, "{huge}");
    }

    #[test]
    fn wrap_keeps_lines_within_width() {
        let Some(face) = system_face() else {
            return;
        };
        let title = "RustCMS 标题图片自动换行测试 with a fairly long english tail";
        let size = 24.0;
        let max_width = 300.0;
        let lines = wrap_lines(&face, title, size, max_width);
        assert!(lines.len() > 1, "应折成多行：{lines:?}");
        for line in &lines {
            let width = face.text_width(line, size);
            // 单个词超宽时允许溢出（既有实现同样会把整个词放上去）
            if line.chars().count() > 1 {
                assert!(width <= max_width + f32::EPSILON, "{line} 超宽 {width}");
            }
        }
        // 强制断行
        let forced = wrap_lines(&face, "第一行\n第二行", 24.0, 1000.0);
        assert_eq!(forced, vec!["第一行".to_string(), "第二行".to_string()]);
    }

    #[test]
    fn renders_text_on_background() {
        let Some(face) = system_face() else {
            return;
        };
        let config = json!({
            "width": 320,
            "height": 160,
            "font_size": 32,
            "font_color": "#ffffff",
            "noise": false
        });
        // 用 ASCII 标题做像素断言：只装了纯西文字体（如 DejaVu）的机器上也能跑，
        // 中文渲染不在单测里强求（不同机器字体覆盖差异太大）。
        let plain = render(&config, "RustCMS TI", None, None).unwrap();
        let with_text = render(&config, "RustCMS TI", Some(&face), None).unwrap();
        assert_eq!((with_text.width, with_text.height), (320, 160));
        assert_eq!(
            (with_text.width, with_text.height),
            (plain.width, plain.height)
        );
        assert_ne!(plain, with_text, "画了文字就不是纯底图了");

        // 文字必须落在画面内：找出与纯底图不同的像素范围
        let mut changed = 0u32;
        let mut min_y = u32::MAX;
        let mut max_y = 0u32;
        for y in 0..with_text.height {
            for x in 0..with_text.width {
                if with_text.get(x, y) != plain.get(x, y) {
                    changed += 1;
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }
        assert!(changed > 100, "文字像素太少：{changed}");
        assert!(max_y < with_text.height, "文字超出画布下沿");
        assert!(min_y > 0, "文字贴到了画布上沿：{min_y}");

        // 配了文字底色：底色带必须出现
        let banded = render(
            &json!({
                "width": 320,
                "height": 160,
                "font_size": 32,
                "font_color": "#ffffff",
                "font_bg_color": "#000000",
            }),
            "RustCMS TI",
            Some(&face),
            None,
        )
        .unwrap();
        assert_ne!(banded, with_text);
        let center_row = banded.height / 2;
        assert!(
            (0..banded.width)
                .map(|x| banded.get(x, center_row)[3])
                .all(|alpha| alpha == 255),
            "底色带应完全不透明"
        );

        // 空标题只出底图（底图色由标题摘要决定，所以要和同标题的无字体版比）
        let blank = render(&config, "   ", Some(&face), None).unwrap();
        let blank_plain = render(&config, "   ", None, None).unwrap();
        assert_eq!(blank, blank_plain);
    }

    #[test]
    fn encodes_rendered_image_as_png() {
        // 路由层就是「render → image_ops::encode(png) → base64」，这里把最后两步走一遍
        let Some(face) = system_face() else {
            return;
        };
        let config = json!({"width": 200, "height": 100, "font_size": 24, "font_color": "#ffffff"});
        let image = render(&config, "标题 PNG", Some(&face), None).unwrap();
        let bytes = super::super::image_ops::encode(&image, "png").unwrap();
        assert_eq!(
            &bytes[..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
            "PNG 魔数不对"
        );
        let decoded = super::super::image_ops::decode(&bytes).unwrap();
        assert_eq!((decoded.width, decoded.height), (200, 100));
    }
}

//! 本包内置的图片处理引擎（纯 Rust，不依赖第三方图片库）。
//!
//! 能力范围：
//! - 解码：PNG（1/2/4/8/16 位，非隔行与 Adam7 隔行，灰度/RGB/调色板/灰度+透明/
//! RGBA，含 tRNS 键色透明）、BMP（16/24/32 位，BI_RGB 与 BI_BITFIELDS 位域）、
//! JPEG（基线 SOF0/SOF1，见 [`super::jpeg`]）、GIF（首帧，见 [`super::gif`]）；
//! - 编码：PNG（带行过滤 + DEFLATE 压缩）、BMP（24 位）、JPEG（基线 4:2:0，
//! 见 [`super::jpeg_encode`]）；
//! - 变换：双线性缩放（按预乘 alpha 插值，透明边缘不会出现黑边）、等比缩放、
//! 居中裁剪（cover）、留白（contain）、区域裁剪、alpha 合成；
//! - 识别：[`probe`] 可只读头部拿到格式、尺寸、是否带透明、是否动图，
//! 供上传校验与错误提示使用。
//!
//! 不支持（一律返回**明确的中文错误**，不做假成功）：渐进式 JPEG、算术编码 JPEG、
//! WebP（含 VP8/VP8L/VP8X）、RLE 压缩的 BMP、动图多帧。

use super::{gif, jpeg, jpeg_encode};

/// 解码失败文案（供上层直接展示）。
pub const UNSUPPORTED_FORMAT: &str = "暂不支持该图片格式的解码（当前支持 PNG / JPEG / GIF / BMP）";

/// 单张图片的像素上限，避免解码超大文件耗尽内存。
const MAX_PIXELS: u64 = 40_000_000;

/// RGBA8 位图（行优先）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// 长度 = `width * height * 4`。
    pub pixels: Vec<u8>,
}

impl Image {
    /// 全透明画布。
    pub fn new(width: u32, height: u32) -> Image {
        Image {
            width,
            height,
            pixels: vec![0; width as usize * height as usize * 4],
        }
    }

    /// 纯色画布。
    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Image {
        let mut image = Image::new(width, height);
        for pixel in image.pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&rgba);
        }
        image
    }

    /// 由 RGBA8 字节流构造（长度不足时自动裁剪）。
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Image {
        let expected = width as usize * height as usize * 4;
        let mut pixels = pixels;
        pixels.resize(expected, 0);
        Image {
            width,
            height,
            pixels,
        }
    }

    /// 读像素。
    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let index = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.pixels[index],
            self.pixels[index + 1],
            self.pixels[index + 2],
            self.pixels[index + 3],
        ]
    }

    /// 写像素（不做混合）。
    pub fn put(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        let index = (y as usize * self.width as usize + x as usize) * 4;
        self.pixels[index..index + 4].copy_from_slice(&rgba);
    }

    /// 以 `src` 为源、按 straight alpha 合成到当前画布。
    ///
    /// `opacity`：0~255 的额外整体透明度。
    pub fn blend(&mut self, x: i64, y: i64, src: [u8; 4], opacity: u8) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        // 源 alpha（叠加整体透明度）
        let src_alpha = (src[3] as u32 * opacity as u32 + 127) / 255;
        if src_alpha == 0 {
            return;
        }
        let dst = self.get(x as u32, y as u32);
        let dst_alpha = dst[3] as u32;
        let inverse = 255 - src_alpha;
        let out_alpha = src_alpha + dst_alpha * inverse / 255;
        if out_alpha == 0 {
            return;
        }
        // 预乘后相加，再还原成 straight alpha（整数运算，结果稳定可测）
        let mix = |source: u8, target: u8| -> u8 {
            let value = (source as u32 * src_alpha
                + target as u32 * dst_alpha * inverse / 255
                + out_alpha / 2)
                / out_alpha;
            value.min(255) as u8
        };
        self.put(
            x as u32,
            y as u32,
            [
                mix(src[0], dst[0]),
                mix(src[1], dst[1]),
                mix(src[2], dst[2]),
                out_alpha.min(255) as u8,
            ],
        );
    }

    /// 把整张 `src` 合成到 `(x, y)`（越界自动裁掉）。
    pub fn composite(&mut self, src: &Image, x: i64, y: i64, opacity: u8) {
        for sy in 0..src.height as i64 {
            for sx in 0..src.width as i64 {
                let pixel = src.get(sx as u32, sy as u32);
                if pixel[3] > 0 {
                    self.blend(x + sx, y + sy, pixel, opacity);
                }
            }
        }
    }

    /// 用给定颜色铺底（把透明像素压成不透明），用于 JPEG 这类无 alpha 的格式。
    pub fn flatten(&self, background: [u8; 4]) -> Image {
        let mut out = self.clone();
        for index in (0..out.pixels.len()).step_by(4) {
            let alpha = out.pixels[index + 3] as u32;
            if alpha == 255 {
                continue;
            }
            let inverse = 255 - alpha;
            for channel in 0..3usize {
                out.pixels[index + channel] = ((out.pixels[index + channel] as u32 * alpha
                    + background[channel] as u32 * inverse
                    + 127)
                    / 255) as u8;
            }
            out.pixels[index + 3] = 255;
        }
        out
    }
}

/// 图片格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Bmp,
    Jpeg,
    Gif,
    Webp,
}

impl Format {
    /// 上报给前端的格式名（小写）。
    pub fn name(&self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Bmp => "bmp",
            Format::Jpeg => "jpg",
            Format::Gif => "gif",
            Format::Webp => "webp",
        }
    }
}

/// 只读头部得到的图片信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    /// 是否可能带透明像素。
    pub has_alpha: bool,
    /// 是否动图（GIF 多帧 / 动态 WebP）。
    pub animated: bool,
}

/// 识别格式与尺寸（不解码像素，因此对大图也很快）。
pub fn probe(bytes: &[u8]) -> Result<ImageInfo, String> {
    if is_png(bytes) {
        let (width, height, color_type) = png_header(bytes)?;
        // 灰度/RGB/调色板还可能靠 tRNS 给出透明（键色 / 调色板 alpha）
        let has_trns = matches!(color_type, 0 | 2 | 3) && png_trns_present(bytes);
        return Ok(ImageInfo {
            format: Format::Png,
            width,
            height,
            has_alpha: matches!(color_type, 4 | 6) || has_trns,
            animated: false,
        });
    }
    if bytes.len() >= 2 && &bytes[0..2] == b"BM" {
        let (width, height) = bmp_header(bytes)?;
        return Ok(ImageInfo {
            format: Format::Bmp,
            width,
            height,
            has_alpha: false,
            animated: false,
        });
    }
    if jpeg::is_jpeg(bytes) {
        let header = jpeg::info(bytes)?;
        return Ok(ImageInfo {
            format: Format::Jpeg,
            width: header.width,
            height: header.height,
            has_alpha: false,
            animated: false,
        });
    }
    if gif::is_gif(bytes) {
        let header = gif::info(bytes)?;
        return Ok(ImageInfo {
            format: Format::Gif,
            width: header.width,
            height: header.height,
            has_alpha: true,
            animated: header.animated,
        });
    }
    if is_webp(bytes) {
        let (width, height, animated) = webp_header(bytes)?;
        return Ok(ImageInfo {
            format: Format::Webp,
            width,
            height,
            has_alpha: true,
            animated,
        });
    }
    Err(UNSUPPORTED_FORMAT.to_string())
}

/// 解码成 RGBA8。
pub fn decode(bytes: &[u8]) -> Result<Image, String> {
    if is_png(bytes) {
        return decode_png(bytes);
    }
    if bytes.len() >= 2 && &bytes[0..2] == b"BM" {
        return decode_bmp(bytes);
    }
    if jpeg::is_jpeg(bytes) {
        let (width, height, pixels) = jpeg::decode(bytes)?;
        return Ok(Image::from_rgba(width, height, pixels));
    }
    if gif::is_gif(bytes) {
        let (width, height, pixels) = gif::decode(bytes)?;
        return Ok(Image::from_rgba(width, height, pixels));
    }
    if is_webp(bytes) {
        let (width, height, _) = webp_header(bytes).unwrap_or((0, 0, false));
        return Err(format!(
            "暂不支持 WebP 解码（图片尺寸 {width}×{height}），请另存为 JPEG / PNG"
        ));
    }
    Err(UNSUPPORTED_FORMAT.to_string())
}

/// 扩展名是否属于「可解码 **且** 可回写」的格式。
///
/// GIF 只做解码（首帧），不能回写，所以不在此列 —— 这样缩略图/水印批量任务会
/// 像 既有实现一样跳过 GIF，不会出现「解码半天又写不出去」的白工。
pub fn supports_extension(ext: &str) -> bool {
    matches!(
        ext.trim_start_matches('.').to_ascii_lowercase().as_str(),
        "png" | "bmp" | "jpg" | "jpeg"
    )
}

/// 按扩展名编码；不支持写出时返回 `Err`（调用方据此跳过，不写坏文件）。
pub fn encode(img: &Image, ext: &str) -> Result<Vec<u8>, String> {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "png" => Ok(encode_png(img)),
        "bmp" => Ok(encode_bmp(img)),
        // JPEG 没有 alpha，透明区域按白底压平
        "jpg" | "jpeg" => {
            let flattened = img.flatten([255, 255, 255, 255]);
            jpeg_encode::encode(
                flattened.width,
                flattened.height,
                &flattened.pixels,
                jpeg_encode::DEFAULT_QUALITY,
            )
        }
        "gif" => Err("暂不支持写出 GIF（动图编码），请改用 PNG".to_string()),
        "webp" => Err("暂不支持写出 WebP，请改用 PNG / JPEG".to_string()),
        _ => Err("当前引擎仅支持写出 PNG / JPEG / BMP".to_string()),
    }
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && bytes[0..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
}

fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 16 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP"
}

/// 读 PNG 的 IHDR（宽、高、颜色类型）。
fn png_header(bytes: &[u8]) -> Result<(u32, u32, u8), String> {
    if bytes.len() < 33 {
        return Err("PNG 数据不完整".to_string());
    }
    if &bytes[12..16] != b"IHDR" {
        return Err("PNG 缺少 IHDR 段".to_string());
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if width == 0 || height == 0 {
        return Err("PNG 尺寸无效".to_string());
    }
    Ok((width, height, bytes[25]))
}

/// 块链里是否存在 tRNS（它必须出现在第一个 IDAT 之前，扫到 IDAT 即可停）。
fn png_trns_present(bytes: &[u8]) -> bool {
    let mut pos = 8usize;
    while pos + 8 <= bytes.len() {
        let length =
            u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
                as usize;
        let kind = &bytes[pos + 4..pos + 8];
        if kind == b"tRNS" {
            return true;
        }
        if kind == b"IDAT" || kind == b"IEND" {
            return false;
        }
        match pos.checked_add(12).and_then(|v| v.checked_add(length)) {
            Some(next) if next > pos => pos = next,
            _ => return false,
        }
    }
    false
}

/// 读 BMP 头部的宽高。
fn bmp_header(bytes: &[u8]) -> Result<(u32, u32), String> {
    if bytes.len() < 26 {
        return Err("BMP 数据不完整".to_string());
    }
    let width = i32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
    let height = i32::from_le_bytes([bytes[22], bytes[23], bytes[24], bytes[25]]);
    if width <= 0 || height == 0 {
        return Err("BMP 尺寸无效".to_string());
    }
    Ok((width as u32, height.unsigned_abs()))
}

/// 读 WebP 头部（VP8 / VP8L / VP8X），返回宽高与是否动图。
fn webp_header(bytes: &[u8]) -> Result<(u32, u32, bool), String> {
    if bytes.len() < 30 {
        return Err("WebP 数据不完整".to_string());
    }
    match &bytes[12..16] {
        b"VP8 " => {
            let width = (u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3FFF) as u32;
            let height = (u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3FFF) as u32;
            Ok((width, height, false))
        }
        b"VP8L" => {
            let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
            let width = (bits & 0x3FFF) + 1;
            let height = ((bits >> 14) & 0x3FFF) + 1;
            Ok((width, height, false))
        }
        b"VP8X" => {
            let flags = bytes[20];
            let width = (bytes[24] as u32 | (bytes[25] as u32) << 8 | (bytes[26] as u32) << 16) + 1;
            let height =
                (bytes[27] as u32 | (bytes[28] as u32) << 8 | (bytes[29] as u32) << 16) + 1;
            Ok((width, height, flags & 0x02 != 0))
        }
        _ => Err("WebP 子格式无法识别".to_string()),
    }
}

// ---------------------------- PNG 解码 -------------------------------------

/// PNG 解码所需的元信息（IHDR + 调色板 + tRNS 键色）。
struct PngMeta {
    depth: u8,
    color_type: u8,
    channels: usize,
    palette: Vec<[u8; 3]>,
    palette_alpha: Vec<u8>,
    /// `color_type == 0` 的 tRNS 键色（原始位深下的灰度样本值）。
    trns_gray: Option<u32>,
    /// `color_type == 2` 的 tRNS 键色（原始位深下的 RGB 样本值）。
    trns_rgb: Option<(u32, u32, u32)>,
}

impl PngMeta {
    /// 每像素占用的位数。
    fn bits_per_pixel(&self) -> usize {
        self.channels * self.depth as usize
    }

    /// 一行像素占用的字节数（不含行首的过滤器字节）。
    fn stride(&self, width: usize) -> usize {
        (width * self.bits_per_pixel() + 7) / 8
    }

    /// 反过滤的 bpp：向前取整到字节，至少 1。
    fn filter_bpp(&self) -> usize {
        (self.bits_per_pixel() / 8).max(1)
    }

    /// 读取第 `x` 个像素第 `channel` 个通道的**原始样本值**（不缩放到 8 位）。
    fn sample(&self, row: &[u8], x: usize, channel: usize) -> u32 {
        match self.depth {
            16 => {
                let off = (x * self.channels + channel) * 2;
                (row[off] as u32) << 8 | row[off + 1] as u32
            }
            8 => row[x * self.channels + channel] as u32,
            // 1/2/4 位只出现在单通道类型（灰度、调色板）上
            d => png_bit_sample(row, x, d) as u32,
        }
    }

    /// 取第 `x` 个像素的 RGBA（tRNS 键色命中时 alpha 为 0）。
    fn rgba(&self, row: &[u8], x: usize) -> [u8; 4] {
        match self.color_type {
            0 => {
                let raw = self.sample(row, x, 0);
                let gray = png_scale(raw, self.depth);
                let alpha = if self.trns_gray == Some(raw) { 0 } else { 255 };
                [gray, gray, gray, alpha]
            }
            2 => {
                let r = self.sample(row, x, 0);
                let g = self.sample(row, x, 1);
                let b = self.sample(row, x, 2);
                let alpha = if self.trns_rgb == Some((r, g, b)) {
                    0
                } else {
                    255
                };
                [
                    png_scale(r, self.depth),
                    png_scale(g, self.depth),
                    png_scale(b, self.depth),
                    alpha,
                ]
            }
            3 => {
                let index = self.sample(row, x, 0) as usize;
                let color = self.palette.get(index).copied().unwrap_or([0, 0, 0]);
                let alpha = self.palette_alpha.get(index).copied().unwrap_or(255);
                [color[0], color[1], color[2], alpha]
            }
            4 => {
                let gray = png_scale(self.sample(row, x, 0), self.depth);
                let alpha = png_scale(self.sample(row, x, 1), self.depth);
                [gray, gray, gray, alpha]
            }
            _ => [
                png_scale(self.sample(row, x, 0), self.depth),
                png_scale(self.sample(row, x, 1), self.depth),
                png_scale(self.sample(row, x, 2), self.depth),
                png_scale(self.sample(row, x, 3), self.depth),
            ],
        }
    }
}

/// 按位深把样本值缩放到 0..=255。
fn png_scale(value: u32, depth: u8) -> u8 {
    match depth {
        16 => ((value * 255 + 32767) / 65535) as u8,
        8 => value as u8,
        4 => (value * 17) as u8,
        2 => (value * 85) as u8,
        _ => (value * 255) as u8,
    }
}

/// 从 1/2/4 位深的行数据里取第 `index` 个样本（高位在前）。
fn png_bit_sample(row: &[u8], index: usize, depth: u8) -> u8 {
    let per_byte = 8 / depth as usize;
    let byte = match row.get(index / per_byte) {
        Some(v) => *v,
        None => return 0,
    };
    let shift = 8 - depth as usize * (index % per_byte + 1);
    (byte >> shift) & ((1u16 << depth) - 1) as u8
}

/// Adam7 隔行的七遍扫描：(起始 x, 起始 y, x 步长, y 步长)。
const PNG_ADAM7: [(usize, usize, usize, usize); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

fn decode_png(bytes: &[u8]) -> Result<Image, String> {
    let mut pos = 8usize;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut depth = 0u8;
    let mut color_type = 0u8;
    let mut interlace = 0u8;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut idat: Vec<u8> = Vec::new();

    while pos + 8 <= bytes.len() {
        let length =
            u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
                as usize;
        let kind = &bytes[pos + 4..pos + 8];
        let start = pos + 8;
        let end = start.checked_add(length).ok_or("PNG 数据损坏")?;
        if end + 4 > bytes.len() {
            return Err("PNG 数据损坏".to_string());
        }
        match kind {
            b"IHDR" => {
                if length < 13 {
                    return Err("PNG IHDR 数据损坏".to_string());
                }
                width = u32::from_be_bytes([
                    bytes[start],
                    bytes[start + 1],
                    bytes[start + 2],
                    bytes[start + 3],
                ]);
                height = u32::from_be_bytes([
                    bytes[start + 4],
                    bytes[start + 5],
                    bytes[start + 6],
                    bytes[start + 7],
                ]);
                depth = bytes[start + 8];
                color_type = bytes[start + 9];
                interlace = bytes[start + 12];
            }
            b"PLTE" => {
                palette = bytes[start..end]
                    .chunks_exact(3)
                    .map(|chunk| [chunk[0], chunk[1], chunk[2]])
                    .collect();
            }
            // tRNS 的含义随颜色类型变化：调色板是整表 alpha，
            // 灰度/真彩是「键色」，样本值一律按 16 位大端存放。
            b"tRNS" => trns = bytes[start..end].to_vec(),
            b"IDAT" => idat.extend_from_slice(&bytes[start..end]),
            b"IEND" => break,
            _ => {}
        }
        pos = end + 4;
    }

    if width == 0 || height == 0 {
        return Err("PNG 尺寸无效".to_string());
    }
    if width as u64 * height as u64 > MAX_PIXELS {
        return Err("图片尺寸过大".to_string());
    }
    let channels = match color_type {
        0 => 1usize,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => return Err("暂不支持该 PNG 颜色类型".to_string()),
    };
    let depth_ok = match color_type {
        0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
        3 => matches!(depth, 1 | 2 | 4 | 8),
        _ => matches!(depth, 8 | 16),
    };
    if !depth_ok {
        return Err(format!("PNG 位深无效（颜色类型 {color_type} 不支持 {depth} 位）"));
    }

    let mut meta = PngMeta {
        depth,
        color_type,
        channels,
        palette,
        palette_alpha: if color_type == 3 { trns.clone() } else { Vec::new() },
        trns_gray: None,
        trns_rgb: None,
    };
    if color_type == 0 && trns.len() >= 2 {
        meta.trns_gray = Some(u16::from_be_bytes([trns[0], trns[1]]) as u32);
    } else if color_type == 2 && trns.len() >= 6 {
        meta.trns_rgb = Some((
            u16::from_be_bytes([trns[0], trns[1]]) as u32,
            u16::from_be_bytes([trns[2], trns[3]]) as u32,
            u16::from_be_bytes([trns[4], trns[5]]) as u32,
        ));
    }

    // 每一遍（非隔行只有一遍）的几何与行字节数
    let mut blocks: Vec<(usize, usize, usize, usize, usize, usize, usize)> = Vec::new();
    if interlace == 0 {
        let stride = meta.stride(width as usize);
        blocks.push((0, 0, 1, 1, width as usize, height as usize, stride));
    } else {
        for &(x0, y0, dx, dy) in PNG_ADAM7.iter() {
            let (w, h) = (width as usize, height as usize);
            let pw = if w > x0 { (w - x0 + dx - 1) / dx } else { 0 };
            let ph = if h > y0 { (h - y0 + dy - 1) / dy } else { 0 };
            if pw == 0 || ph == 0 {
                continue;
            }
            blocks.push((x0, y0, dx, dy, pw, ph, meta.stride(pw)));
        }
    }

    let mut expected = 0usize;
    for block in &blocks {
        let size = (block.6 + 1).checked_mul(block.5).ok_or("PNG 数据损坏")?;
        expected = expected.checked_add(size).ok_or("PNG 数据损坏")?;
    }
    let raw = inflate(&idat, expected)?;
    if raw.len() < expected {
        return Err("PNG 数据不完整".to_string());
    }

    let mut image = Image::new(width, height);
    let bpp = meta.filter_bpp();
    let mut offset = 0usize;
    for &(sx, sy, dx, dy, pw, ph, stride) in &blocks {
        let need = (stride + 1) * ph;
        if offset + need > raw.len() {
            return Err("PNG 数据不完整".to_string());
        }
        let pixels = png_unfilter(&raw[offset..offset + need], stride, bpp, ph)?;
        offset += need;
        for row in 0..ph {
            let row_data = &pixels[row * stride..(row + 1) * stride];
            let ty = sy + row * dy;
            for x in 0..pw {
                let tx = sx + x * dx;
                image.put(tx as u32, ty as u32, meta.rgba(row_data, x));
            }
        }
    }
    Ok(image)
}

/// PNG 行反过滤：把带过滤器字节的原始数据还原成像素字节。
fn png_unfilter(raw: &[u8], stride: usize, bpp: usize, rows: usize) -> Result<Vec<u8>, String> {
    let mut pixels = vec![0u8; stride * rows];
    for row in 0..rows {
        let base = row * (stride + 1);
        let filter = raw[base];
        let src = &raw[base + 1..base + 1 + stride];
        let dst_start = row * stride;
        for i in 0..stride {
            let left = if i >= bpp {
                pixels[dst_start + i - bpp]
            } else {
                0
            };
            let up = if row > 0 {
                pixels[dst_start - stride + i]
            } else {
                0
            };
            let up_left = if row > 0 && i >= bpp {
                pixels[dst_start - stride + i - bpp]
            } else {
                0
            };
            let value = src[i];
            let out = match filter {
                0 => value,
                1 => value.wrapping_add(left),
                2 => value.wrapping_add(up),
                3 => value.wrapping_add(((left as u16 + up as u16) / 2) as u8),
                4 => value.wrapping_add(paeth(left, up, up_left)),
                _ => return Err("PNG 过滤器无效".to_string()),
            };
            pixels[dst_start + i] = out;
        }
    }
    Ok(pixels)
}

fn paeth(left: u8, up: u8, up_left: u8) -> u8 {
    let p = left as i32 + up as i32 - up_left as i32;
    let pa = (p - left as i32).abs();
    let pb = (p - up as i32).abs();
    let pc = (p - up_left as i32).abs();
    if pa <= pb && pa <= pc {
        left
    } else if pb <= pc {
        up
    } else {
        up_left
    }
}

// ---------------------------- PNG 编码 -------------------------------------

fn encode_png(img: &Image) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&img.width.to_be_bytes());
    ihdr.extend_from_slice(&img.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 位 RGBA，非隔行
    write_chunk(&mut out, b"IHDR", &ihdr);

    // 逐行选最优过滤器（最小绝对值和启发式，与 libpng 一致），显著减小体积
    let stride = img.width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * img.height as usize);
    let mut previous = vec![0u8; stride];
    let mut current = vec![0u8; stride];
    let mut candidate = vec![0u8; stride];
    let mut best = vec![0u8; stride];
    for y in 0..img.height as usize {
        current.copy_from_slice(&img.pixels[y * stride..(y + 1) * stride]);
        let mut best_filter = 0u8;
        let mut best_score = u64::MAX;
        for filter in 0..5u8 {
            filter_row(filter, &current, &previous, stride, 4, &mut candidate);
            let score: u64 = candidate
                .iter()
                .map(|value| (*value as i8 as i64).unsigned_abs())
                .sum();
            if score < best_score {
                best_score = score;
                best_filter = filter;
                best.copy_from_slice(&candidate);
            }
        }
        raw.push(best_filter);
        raw.extend_from_slice(&best);
        std::mem::swap(&mut previous, &mut current);
    }

    write_chunk(&mut out, b"IDAT", &zlib_compress(&raw));
    write_chunk(&mut out, b"IEND", &[]);
    out
}

/// 按 PNG 规范对一行做过滤，结果写入 `out`。
fn filter_row(filter: u8, row: &[u8], previous: &[u8], stride: usize, bpp: usize, out: &mut [u8]) {
    for index in 0..stride {
        let left = if index >= bpp { row[index - bpp] } else { 0 };
        let up = previous[index];
        let up_left = if index >= bpp {
            previous[index - bpp]
        } else {
            0
        };
        out[index] = match filter {
            0 => row[index],
            1 => row[index].wrapping_sub(left),
            2 => row[index].wrapping_sub(up),
            3 => row[index].wrapping_sub(((left as u16 + up as u16) / 2) as u8),
            _ => row[index].wrapping_sub(paeth(left, up, up_left)),
        };
    }
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// zlib 容器（`deflate_compress` 输出）。
fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2 + 32);
    out.extend_from_slice(&[0x78, 0x01]); // CM=8, CINFO=7，校验位正确
    out.extend_from_slice(&deflate_compress(data));
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

// ---------------------------- DEFLATE 压缩 ---------------------------------

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// LSB 优先的位写入器（DEFLATE 用）。
struct BitWriter {
    out: Vec<u8>,
    buffer: u32,
    bits: u8,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter {
            out: Vec::new(),
            buffer: 0,
            bits: 0,
        }
    }

    fn write(&mut self, value: u32, count: u8) {
        self.buffer |= (value & ((1u32 << count) - 1)) << self.bits;
        self.bits += count;
        while self.bits >= 8 {
            self.out.push((self.buffer & 0xFF) as u8);
            self.buffer >>= 8;
            self.bits -= 8;
        }
    }

    /// 写哈夫曼码（码本身高位优先）。
    fn write_code(&mut self, code: u32, length: u8) {
        for index in (0..length).rev() {
            self.write((code >> index) & 1, 1);
        }
    }

    /// 补零到字节边界（DEFLATE 存储块要求按字节对齐）。
    fn align_to_byte(&mut self) {
        if self.bits > 0 {
            self.out.push((self.buffer & 0xFF) as u8);
            self.buffer = 0;
            self.bits = 0;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.bits > 0 {
            self.out.push((self.buffer & 0xFF) as u8);
        }
        self.out
    }
}

/// 固定哈夫曼表的字面量码。
fn fixed_literal_code(symbol: u32) -> (u32, u8) {
    match symbol {
        0..=143 => (0x30 + symbol, 8),
        144..=255 => (0x190 + (symbol - 144), 9),
        256..=279 => (symbol - 256, 7),
        280..=287 => (0xC0 + (symbol - 280), 8),
        _ => (0, 0),
    }
}

/// 只写 DEFLATE 存储块（不压缩，但格式合法），用于超大数据的兜底。
fn deflate_store(data: &[u8]) -> Vec<u8> {
    let mut writer = BitWriter::new();
    if data.is_empty() {
        writer.write(1, 1); // BFINAL
        writer.write(0, 2); // BTYPE = 存储块
        writer.write(0, 16);
        writer.write(0xFFFF, 16);
        return writer.finish();
    }
    let mut offset = 0usize;
    while offset < data.len() {
        let take = (data.len() - offset).min(65535);
        let final_block = offset + take >= data.len();
        writer.write(u32::from(final_block), 1);
        writer.write(0, 2);
        // 存储块要按字节对齐后再写 LEN/NLEN
        writer.align_to_byte();
        writer.write(take as u32, 16);
        writer.write(!(take as u32) & 0xFFFF, 16);
        for byte in &data[offset..offset + take] {
            writer.write(*byte as u32, 8);
        }
        offset += take;
    }
    writer.finish()
}

/// 固定哈夫曼表 + LZ77 压缩（DEFLATE BTYPE=01）。
///
/// 相比「只写存储块」，同一张缩略图通常能小 2~5 倍；纯标准库实现，不引依赖。
fn deflate_compress(data: &[u8]) -> Vec<u8> {
    const WINDOW: usize = 32768;
    const HASH_BITS: usize = 15;
    const HASH_SIZE: usize = 1 << HASH_BITS;
    const MAX_CHAIN: usize = 32;

    // 超过 4GB 的数据用位置哈希会溢出 u32（正常图片远达不到，这里只做兜底）
    if data.len() >= u32::MAX as usize {
        return deflate_store(data);
    }

    let mut writer = BitWriter::new();
    writer.write(1, 1); // BFINAL
    writer.write(1, 2); // BTYPE = 固定哈夫曼

    let hash_of = |position: usize| -> usize {
        let a = data[position] as usize;
        let b = data[position + 1] as usize;
        let c = data[position + 2] as usize;
        ((a << 10) ^ (b << 5) ^ c) & (HASH_SIZE - 1)
    };

    // 哈希链用「窗口大小的环形缓冲」保存，内存占用与图片大小无关（2×128KB）
    const NONE: u32 = u32::MAX;
    let mut head = vec![NONE; HASH_SIZE];
    let mut chain = vec![NONE; WINDOW];
    let mut position = 0usize;
    while position < data.len() {
        let mut best_length = 0usize;
        let mut best_distance = 0usize;
        if position + 3 <= data.len() {
            let hash = hash_of(position);
            let mut candidate = head[hash];
            let mut steps = 0usize;
            let max_length = (data.len() - position).min(258);
            while candidate != NONE && steps < MAX_CHAIN {
                let offset = candidate as usize;
                // 超出窗口的候选不再有效（环形缓冲可能已被覆盖）
                if position - offset > WINDOW {
                    break;
                }
                let mut length = 0usize;
                while length < max_length && data[offset + length] == data[position + length] {
                    length += 1;
                }
                if length > best_length {
                    best_length = length;
                    best_distance = position - offset;
                    if length == max_length {
                        break;
                    }
                }
                candidate = chain[offset % WINDOW];
                steps += 1;
            }
            // 把当前位置挂到链头
            chain[position % WINDOW] = head[hash];
            head[hash] = position as u32;
        }

        if best_length >= 3 {
            let (length_index, extra_value, extra_bits) = length_code(best_length);
            let (code, code_bits) = fixed_literal_code(257 + length_index as u32);
            writer.write_code(code, code_bits);
            if extra_bits > 0 {
                writer.write(extra_value, extra_bits);
            }
            let (distance_index, distance_extra, distance_bits) = distance_code(best_distance);
            writer.write_code(distance_index as u32, 5);
            if distance_bits > 0 {
                writer.write(distance_extra, distance_bits);
            }
            // 把匹配区间内的起点也登记进哈希表，后续匹配才能命中
            for offset in 1..best_length {
                let index = position + offset;
                if index + 3 <= data.len() {
                    let hash = hash_of(index);
                    chain[index % WINDOW] = head[hash];
                    head[hash] = index as u32;
                }
            }
            position += best_length;
        } else {
            let (code, code_bits) = fixed_literal_code(data[position] as u32);
            writer.write_code(code, code_bits);
            position += 1;
        }
    }

    let (end_code, end_bits) = fixed_literal_code(256);
    writer.write_code(end_code, end_bits);
    writer.finish()
}

/// 匹配长度 → (长度码下标, 额外位值, 额外位数)。
fn length_code(length: usize) -> (usize, u32, u8) {
    let mut index = 28usize;
    while index > 0 && (LENGTH_BASE[index] as usize) > length {
        index -= 1;
    }
    let base = LENGTH_BASE[index] as usize;
    (index, (length - base) as u32, LENGTH_EXTRA[index])
}

/// 匹配距离 → (距离码, 额外位值, 额外位数)。
fn distance_code(distance: usize) -> (usize, u32, u8) {
    let mut index = 29usize;
    while index > 0 && (DIST_BASE[index] as usize) > distance {
        index -= 1;
    }
    let base = DIST_BASE[index] as usize;
    (index, (distance - base) as u32, DIST_EXTRA[index])
}

// ---------------------------- DEFLATE 解压 --------------------------------

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader {
            data,
            pos: 0,
            bit: 0,
        }
    }

    fn read_bit(&mut self) -> Result<u32, String> {
        if self.pos >= self.data.len() {
            return Err("压缩数据不完整".to_string());
        }
        let value = (self.data[self.pos] >> self.bit) & 1;
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.pos += 1;
        }
        Ok(value as u32)
    }

    /// 读 `count` 位**低位优先**的整数（DEFLATE 的块类型与额外位都是这个顺序）。
    fn read_bits(&mut self, count: u8) -> Result<u32, String> {
        let mut value = 0u32;
        for index in 0..count {
            value |= self.read_bit()? << index;
        }
        Ok(value)
    }

    fn align(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.pos += 1;
        }
    }

    fn read_byte(&mut self) -> Result<u8, String> {
        if self.pos >= self.data.len() {
            return Err("压缩数据不完整".to_string());
        }
        let value = self.data[self.pos];
        self.pos += 1;
        Ok(value)
    }
}

/// canonical Huffman 表。
struct Huffman {
    counts: [u16; 16],
    first_code: [u16; 16],
    first_index: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Huffman, String> {
        let mut counts = [0u16; 16];
        for &len in lengths {
            if len > 15 {
                return Err("Huffman 码长非法".to_string());
            }
            counts[len as usize] += 1;
        }
        counts[0] = 0; // 0 表示该符号未使用

        let mut first_code = [0u16; 16];
        let mut first_index = [0u16; 16];
        let mut code = 0u16;
        let mut index = 0u16;
        for len in 1..16 {
            first_code[len] = code;
            first_index[len] = index;
            code = (code + counts[len]) << 1;
            index += counts[len];
        }

        let mut symbols = vec![0u16; index as usize];
        let mut offsets = first_index;
        for (symbol, &len) in lengths.iter().enumerate() {
            if len != 0 {
                symbols[offsets[len as usize] as usize] = symbol as u16;
                offsets[len as usize] += 1;
            }
        }
        Ok(Huffman {
            counts,
            first_code,
            first_index,
            symbols,
        })
    }

    fn decode(&self, reader: &mut BitReader) -> Result<u16, String> {
        let mut code = 0u16;
        for len in 1..16 {
            code = (code << 1) | reader.read_bit()? as u16;
            let count = self.counts[len];
            if code >= self.first_code[len] && code - self.first_code[len] < count {
                let index = self.first_index[len] as usize + (code - self.first_code[len]) as usize;
                return Ok(self.symbols[index]);
            }
        }
        Err("Huffman 解码失败".to_string())
    }
}

fn inflate(data: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    if data.len() < 2 {
        return Err("压缩数据不完整".to_string());
    }
    // zlib 头
    if data[0] & 0x0F != 8 {
        return Err("zlib 压缩方法不支持".to_string());
    }
    if data[1] & 0x20 != 0 {
        return Err("不支持带预置字典的 zlib".to_string());
    }
    let mut reader = BitReader::new(&data[2..]);
    let mut out = Vec::new();

    loop {
        let final_block = reader.read_bit()? == 1;
        let block_type = reader.read_bits(2)?;
        match block_type {
            0 => {
                reader.align();
                let len = reader.read_byte()? as usize | (reader.read_byte()? as usize) << 8;
                let nlen = reader.read_byte()? as usize | (reader.read_byte()? as usize) << 8;
                if len != (!nlen & 0xFFFF) {
                    return Err("DEFLATE 存储块长度校验失败".to_string());
                }
                for _ in 0..len {
                    if out.len() >= limit {
                        return Err("图片数据超出预期大小".to_string());
                    }
                    out.push(reader.read_byte()?);
                }
            }
            1 => {
                let mut lit_lengths = [0u8; 288];
                for (i, item) in lit_lengths.iter_mut().enumerate() {
                    *item = match i {
                        0..=143 => 8,
                        144..=255 => 9,
                        256..=279 => 7,
                        _ => 8,
                    };
                }
                let dist_lengths = [5u8; 32];
                let lit = Huffman::new(&lit_lengths)?;
                let dist = Huffman::new(&dist_lengths)?;
                inflate_codes(&mut reader, &lit, &dist, &mut out, limit)?;
            }
            2 => {
                let hlit = reader.read_bits(5)? as usize + 257;
                let hdist = reader.read_bits(5)? as usize + 1;
                let hclen = reader.read_bits(4)? as usize + 4;
                const ORDER: [usize; 19] = [
                    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
                ];
                let mut code_lengths = [0u8; 19];
                for i in 0..hclen {
                    code_lengths[ORDER[i]] = reader.read_bits(3)? as u8;
                }
                let code_huffman = Huffman::new(&code_lengths)?;

                let mut lengths = vec![0u8; hlit + hdist];
                let mut index = 0usize;
                while index < lengths.len() {
                    let symbol = code_huffman.decode(&mut reader)?;
                    match symbol {
                        0..=15 => {
                            lengths[index] = symbol as u8;
                            index += 1;
                        }
                        16 => {
                            if index == 0 {
                                return Err("DEFLATE 重复码无前值".to_string());
                            }
                            let repeat = 3 + reader.read_bits(2)? as usize;
                            let value = lengths[index - 1];
                            for _ in 0..repeat {
                                if index >= lengths.len() {
                                    return Err("DEFLATE 码长越界".to_string());
                                }
                                lengths[index] = value;
                                index += 1;
                            }
                        }
                        17 => {
                            let repeat = 3 + reader.read_bits(3)? as usize;
                            index += repeat;
                        }
                        18 => {
                            let repeat = 11 + reader.read_bits(7)? as usize;
                            index += repeat;
                        }
                        _ => return Err("DEFLATE 码长符号非法".to_string()),
                    }
                }
                if index > lengths.len() {
                    return Err("DEFLATE 码长数量越界".to_string());
                }
                let lit = Huffman::new(&lengths[..hlit])?;
                let dist = Huffman::new(&lengths[hlit..])?;
                inflate_codes(&mut reader, &lit, &dist, &mut out, limit)?;
            }
            _ => return Err("DEFLATE 块类型非法".to_string()),
        }
        if final_block {
            break;
        }
    }
    Ok(out)
}

fn inflate_codes(
    reader: &mut BitReader,
    lit: &Huffman,
    dist: &Huffman,
    out: &mut Vec<u8>,
    limit: usize,
) -> Result<(), String> {
    loop {
        let symbol = lit.decode(reader)?;
        match symbol {
            0..=255 => {
                if out.len() >= limit {
                    return Err("图片数据超出预期大小".to_string());
                }
                out.push(symbol as u8);
            }
            256 => return Ok(()),
            257..=285 => {
                let index = (symbol - 257) as usize;
                let length =
                    LENGTH_BASE[index] as usize + reader.read_bits(LENGTH_EXTRA[index])? as usize;
                let dist_symbol = dist.decode(reader)? as usize;
                if dist_symbol >= 30 {
                    return Err("DEFLATE 距离符号非法".to_string());
                }
                let distance = DIST_BASE[dist_symbol] as usize
                    + reader.read_bits(DIST_EXTRA[dist_symbol])? as usize;
                if distance > out.len() {
                    return Err("DEFLATE 回溯距离非法".to_string());
                }
                for _ in 0..length {
                    let byte = out[out.len() - distance];
                    if out.len() >= limit {
                        return Err("图片数据超出预期大小".to_string());
                    }
                    out.push(byte);
                }
            }
            _ => return Err("DEFLATE 长度符号非法".to_string()),
        }
    }
}

// ---------------------------- BMP 解码 / 编码 -----------------------------

fn decode_bmp(bytes: &[u8]) -> Result<Image, String> {
    if bytes.len() < 54 || &bytes[0..2] != b"BM" {
        return Err("BMP 数据损坏".to_string());
    }
    let pixel_offset = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]) as usize;
    let dib_size = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]) as usize;
    if dib_size < 40 {
        return Err("暂不支持该 BMP 头".to_string());
    }
    let width = i32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
    let height = i32::from_le_bytes([bytes[22], bytes[23], bytes[24], bytes[25]]);
    let bpp = u16::from_le_bytes([bytes[28], bytes[29]]);
    let compression = u32::from_le_bytes([bytes[30], bytes[31], bytes[32], bytes[33]]);
    if width <= 0 || height == 0 {
        return Err("BMP 尺寸无效".to_string());
    }
    if bpp != 16 && bpp != 24 && bpp != 32 {
        return Err("暂不支持该 BMP 位深（仅支持 16/24/32 位）".to_string());
    }
    // BI_BITFIELDS(3) / BI_ALPHABITFIELDS(6) 用通道掩码描述像素布局；
    // 其余压缩方式（RLE8/RLE4/JPEG/PNG）不在这里处理。
    let bitfields = compression == 3 || compression == 6;
    if compression != 0 && !bitfields {
        return Err("暂不支持压缩 BMP".to_string());
    }
    let width = width as u32;
    let height_abs = height.unsigned_abs();
    if width as u64 * height_abs as u64 > MAX_PIXELS {
        return Err("图片尺寸过大".to_string());
    }

    // 通道掩码：位域布局表在 DIB 头之后（文件偏移 54 起），
    // V3 及以上的头（≥56 字节）还带 alpha 掩码。
    let masks = if bitfields {
        if bpp == 24 {
            return Err("BMP 位域压缩仅支持 16/32 位".to_string());
        }
        if bytes.len() < 66 {
            return Err("BMP 位域掩码不完整".to_string());
        }
        let read_mask = |off: usize| {
            u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
        };
        let alpha = if dib_size >= 56 && bytes.len() >= 70 {
            read_mask(66)
        } else {
            0
        };
        let (r, g, b) = (read_mask(54), read_mask(58), read_mask(62));
        if r | g | b == 0 {
            // 掩码全零是坏数据，退回该位深的默认布局
            if bpp == 16 {
                (0x7C00, 0x03E0, 0x001F, 0)
            } else {
                (0x00FF_0000, 0x0000_FF00, 0x0000_00FF, alpha)
            }
        } else {
            (r, g, b, alpha)
        }
    } else if bpp == 16 {
        // BI_RGB 16 位默认 5-5-5
        (0x7C00, 0x03E0, 0x001F, 0)
    } else if bpp == 32 {
        // BI_RGB 32 位是 XRGB，第 4 字节保留不用（不能当 alpha，否则整图透明）
        (0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0)
    } else {
        (0, 0, 0, 0)
    };

    let top_down = height < 0;
    let bytes_per_pixel = (bpp as usize + 7) / 8;
    let row_stride = ((bpp as usize * width as usize + 31) / 32) * 4;
    let mut image = Image::new(width, height_abs);
    for row in 0..height_abs {
        let src_row = if top_down { row } else { height_abs - 1 - row };
        let base = pixel_offset
            .checked_add(src_row as usize * row_stride)
            .ok_or("BMP 数据损坏")?;
        if base + row_stride > bytes.len() {
            return Err("BMP 像素数据不完整".to_string());
        }
        for x in 0..width as usize {
            let p = base + x * bytes_per_pixel;
            let pixel = match bytes_per_pixel {
                2 => {
                    let value = u16::from_le_bytes([bytes[p], bytes[p + 1]]) as u32;
                    bmp_pixel(value, masks)
                }
                4 => {
                    let value = u32::from_le_bytes([
                        bytes[p],
                        bytes[p + 1],
                        bytes[p + 2],
                        bytes[p + 3],
                    ]);
                    bmp_pixel(value, masks)
                }
                _ => [bytes[p + 2], bytes[p + 1], bytes[p], 255],
            };
            image.put(x as u32, row, pixel);
        }
    }
    Ok(image)
}

/// 把掩码描述的分量还原成 0..=255；`empty` 是掩码为 0 时的取值（alpha 给 255）。
fn bmp_channel(value: u32, mask: u32, empty: u8) -> u8 {
    if mask == 0 {
        return empty;
    }
    let shift = mask.trailing_zeros();
    let max = mask >> shift;
    if max == 0 {
        return empty;
    }
    let raw = (value & mask) >> shift;
    ((raw * 255 + max / 2) / max) as u8
}

fn bmp_pixel(value: u32, masks: (u32, u32, u32, u32)) -> [u8; 4] {
    [
        bmp_channel(value, masks.0, 0),
        bmp_channel(value, masks.1, 0),
        bmp_channel(value, masks.2, 0),
        bmp_channel(value, masks.3, 255),
    ]
}

fn encode_bmp(img: &Image) -> Vec<u8> {
    let row_stride = (img.width as usize * 3 + 3) & !3;
    let pixel_bytes = row_stride * img.height as usize;
    let mut out = Vec::with_capacity(54 + pixel_bytes);

    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + pixel_bytes) as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(img.width as i32).to_le_bytes());
    out.extend_from_slice(&(img.height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    for y in (0..img.height as usize).rev() {
        let start = out.len();
        for x in 0..img.width as usize {
            let pixel = img.get(x as u32, y as u32);
            out.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
        out.resize(start + row_stride, 0);
    }
    out
}

// ---------------------------- 缩放 / 裁剪 ---------------------------------

/// 双线性缩放到指定尺寸（按预乘 alpha 插值，透明边缘不会发黑）。
pub fn resize(img: &Image, width: u32, height: u32) -> Image {
    if width == 0 || height == 0 {
        return Image::new(width, height);
    }
    if img.width == 0 || img.height == 0 {
        return Image::new(width, height);
    }
    if img.width == width && img.height == height {
        return img.clone();
    }
    // 预乘 alpha：把 RGB 先乘上透明度，插值完再除回去
    let mut out = Image::new(width, height);
    let scale_x = img.width as f32 / width as f32;
    let scale_y = img.height as f32 / height as f32;
    for y in 0..height {
        let source_y = ((y as f32 + 0.5) * scale_y - 0.5).max(0.0);
        let y0 = source_y.floor() as u32;
        let y1 = (y0 + 1).min(img.height - 1);
        let fy = source_y - y0 as f32;
        for x in 0..width {
            let source_x = ((x as f32 + 0.5) * scale_x - 0.5).max(0.0);
            let x0 = source_x.floor() as u32;
            let x1 = (x0 + 1).min(img.width - 1);
            let fx = source_x - x0 as f32;
            let p00 = premultiply(img.get(x0, y0));
            let p10 = premultiply(img.get(x1, y0));
            let p01 = premultiply(img.get(x0, y1));
            let p11 = premultiply(img.get(x1, y1));
            let mut value = [0f32; 4];
            for channel in 0..4usize {
                let top = p00[channel] * (1.0 - fx) + p10[channel] * fx;
                let bottom = p01[channel] * (1.0 - fx) + p11[channel] * fx;
                value[channel] = top * (1.0 - fy) + bottom * fy;
            }
            out.put(x, y, unpremultiply(value));
        }
    }
    out
}

fn premultiply(pixel: [u8; 4]) -> [f32; 4] {
    let alpha = pixel[3] as f32 / 255.0;
    [
        pixel[0] as f32 * alpha,
        pixel[1] as f32 * alpha,
        pixel[2] as f32 * alpha,
        pixel[3] as f32,
    ]
}

fn unpremultiply(value: [f32; 4]) -> [u8; 4] {
    let alpha = value[3].clamp(0.0, 255.0);
    if alpha <= 0.5 {
        return [0, 0, 0, 0];
    }
    let scale = 255.0 / alpha;
    [
        (value[0] * scale).round().clamp(0.0, 255.0) as u8,
        (value[1] * scale).round().clamp(0.0, 255.0) as u8,
        (value[2] * scale).round().clamp(0.0, 255.0) as u8,
        alpha.round() as u8,
    ]
}

/// 等比缩放，使结果完全落在 `width × height` 内（不放大）。
pub fn resize_fit(img: &Image, width: u32, height: u32) -> Image {
    if img.width == 0 || img.height == 0 {
        return Image::new(width, height);
    }
    if width == 0 || height == 0 {
        return Image::new(width, height);
    }
    let ratio = (width as f32 / img.width as f32).min(height as f32 / img.height as f32);
    if ratio >= 1.0 {
        return img.clone();
    }
    let new_width = ((img.width as f32 * ratio).round() as u32).max(1);
    let new_height = ((img.height as f32 * ratio).round() as u32).max(1);
    resize(img, new_width, new_height)
}

/// 等比缩放后居中裁剪到目标尺寸（对应既有约定裁剪模式）。
pub fn resize_cover(img: &Image, width: u32, height: u32) -> Image {
    if width == 0 || height == 0 {
        return Image::new(width, height);
    }
    let scaled = resize_fill(img, width, height);
    if scaled.width == width && scaled.height == height {
        return scaled;
    }
    let offset_x = (scaled.width.saturating_sub(width)) / 2;
    let offset_y = (scaled.height.saturating_sub(height)) / 2;
    crop(&scaled, offset_x, offset_y, width, height)
}

/// 等比放大到**覆盖**目标尺寸（可能超出，供 cover 再裁）。
fn resize_fill(img: &Image, width: u32, height: u32) -> Image {
    if img.width == 0 || img.height == 0 {
        return Image::new(width, height);
    }
    let ratio = (width as f32 / img.width as f32).max(height as f32 / img.height as f32);
    let new_width = ((img.width as f32 * ratio).round() as u32).max(width);
    let new_height = ((img.height as f32 * ratio).round() as u32).max(height);
    resize(img, new_width, new_height)
}

/// 等比缩放后居中留白到目标尺寸（对应既有约定补白模式）。
pub fn resize_contain(img: &Image, width: u32, height: u32, background: [u8; 4]) -> Image {
    if width == 0 || height == 0 {
        return Image::new(width, height);
    }
    let fitted = resize_fit(img, width, height);
    let mut canvas = Image::filled(width, height, background);
    let x = (width.saturating_sub(fitted.width)) / 2;
    let y = (height.saturating_sub(fitted.height)) / 2;
    // 留白画布不透明，直接覆盖即可（比 alpha 合成快且不会二次混色）
    for fy in 0..fitted.height as usize {
        let row =
            &fitted.pixels[fy * fitted.width as usize * 4..(fy + 1) * fitted.width as usize * 4];
        let start = ((y as usize + fy) * width as usize + x as usize) * 4;
        canvas.pixels[start..start + row.len()].copy_from_slice(row);
    }
    canvas
}

/// 区域裁剪（越界部分透明）。
pub fn crop(img: &Image, x: u32, y: u32, width: u32, height: u32) -> Image {
    let mut out = Image::new(width, height);
    for oy in 0..height {
        for ox in 0..width {
            let sx = x + ox;
            let sy = y + oy;
            if sx < img.width && sy < img.height {
                out.put(ox, oy, img.get(sx, sy));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Image {
        let mut img = Image::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                img.put(
                    x,
                    y,
                    [
                        (x * 60) as u8,
                        (y * 60) as u8,
                        128,
                        if x == 0 { 80 } else { 255 },
                    ],
                );
            }
        }
        img
    }

    #[test]
    fn png_roundtrip() {
        let img = sample();
        let bytes = encode_png(&img);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.width, 4);
        assert_eq!(decoded.height, 4);
        assert_eq!(decoded, img);
    }

    #[test]
    fn png_encoding_compresses_flat_image() {
        // 纯色图的 DEFLATE 压缩应该非常明显（远小于原始数据）
        let img = Image::filled(128, 128, [10, 20, 30, 255]);
        let bytes = encode_png(&img);
        assert!(
            bytes.len() < img.pixels.len() / 4,
            "压缩后 {} 字节，原始 {} 字节",
            bytes.len(),
            img.pixels.len()
        );
        assert_eq!(decode(&bytes).unwrap(), img);
    }

    #[test]
    fn deflate_roundtrip() {
        let mut data = Vec::new();
        for index in 0..4096usize {
            data.push(((index / 7) % 251) as u8);
        }
        let compressed = zlib_compress(&data);
        // inflate 需要上限参数：给足空间
        let restored = inflate(&compressed, data.len() + 1).unwrap();
        assert_eq!(restored, data);
    }

    #[test]
    fn deflate_handles_long_matches() {
        // 重复片段 + 伪随机字节：覆盖各种匹配长度/距离（额外位不为 0 的情形）
        let mut data: Vec<u8> = Vec::with_capacity(20000);
        let mut state = 12_345u32;
        for index in 0..20000usize {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            if index > 500 && index % 97 < 40 {
                let byte = data[index - 480];
                data.push(byte);
            } else {
                data.push((state >> 16) as u8);
            }
        }
        let compressed = zlib_compress(&data);
        // 一半是可复用的重复片段，压缩后必须比原始数据小
        assert!(
            compressed.len() < data.len(),
            "没有产生压缩：{} / {}",
            compressed.len(),
            data.len()
        );
        let restored = inflate(&compressed, data.len() + 1).unwrap();
        assert_eq!(restored, data);
    }

    #[test]
    fn bmp_roundtrip() {
        let img = sample();
        let bytes = encode_bmp(&img);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.width, 4);
        assert_eq!(decoded.height, 4);
        // BMP 无 alpha，解码后 alpha 恒为 255
        for y in 0..4 {
            for x in 0..4 {
                let a = img.get(x, y);
                let b = decoded.get(x, y);
                assert_eq!([a[0], a[1], a[2]], [b[0], b[1], b[2]]);
                assert_eq!(b[3], 255);
            }
        }
    }

    // ---- 手搓 PNG / BMP 用例（覆盖 16 位、低位深、隔行、tRNS、位域）----

    fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0, 0, 0, 0]); // 解码器不校验 CRC
    }

    /// 用已经去掉过滤器字节的行数据拼一张 PNG。
    fn png_build(
        width: u32,
        height: u32,
        depth: u8,
        color_type: u8,
        interlace: u8,
        palette: Option<&[[u8; 3]]>,
        trns: Option<&[u8]>,
        rows: &[Vec<u8>],
    ) -> Vec<u8> {
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[depth, color_type, 0, 0, interlace]);
        png_chunk(&mut out, b"IHDR", &ihdr);
        if let Some(entries) = palette {
            let mut plte = Vec::new();
            for entry in entries {
                plte.extend_from_slice(entry);
            }
            png_chunk(&mut out, b"PLTE", &plte);
        }
        if let Some(data) = trns {
            png_chunk(&mut out, b"tRNS", data);
        }
        let mut raw = Vec::new();
        for row in rows {
            raw.push(0); // 过滤器 None
            raw.extend_from_slice(row);
        }
        png_chunk(&mut out, b"IDAT", &zlib_compress(&raw));
        png_chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn png_decodes_16bit_rgba() {
        let row = vec![
            0xFF, 0xFF, 0x00, 0x00, 0x80, 0x80, 0xFF, 0xFF, // 不透明
            0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, // 全透明
        ];
        let bytes = png_build(2, 1, 16, 6, 0, None, None, &[row]);
        let img = decode(&bytes).unwrap();
        assert_eq!(img.width, 2);
        assert_eq!(img.get(0, 0), [255, 0, 128, 255]);
        assert_eq!(img.get(1, 0), [0, 255, 0, 0]);
    }

    #[test]
    fn png_decodes_16bit_gray() {
        let rows = vec![vec![0xFF, 0xFF], vec![0x00, 0x00]];
        let bytes = png_build(1, 2, 16, 0, 0, None, None, &rows);
        let img = decode(&bytes).unwrap();
        assert_eq!(img.get(0, 0), [255, 255, 255, 255]);
        assert_eq!(img.get(0, 1), [0, 0, 0, 255]);
    }

    #[test]
    fn png_decodes_4bit_gray() {
        // 两个像素挤在一个字节里：0xF -> 255，0x0 -> 0
        let bytes = png_build(2, 1, 4, 0, 0, None, None, &[vec![0xF0]]);
        let img = decode(&bytes).unwrap();
        assert_eq!(img.get(0, 0), [255, 255, 255, 255]);
        assert_eq!(img.get(1, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn png_decodes_1bit_palette_with_trns() {
        let palette = [[10, 20, 30], [200, 100, 50]];
        // 3 个像素：索引 1,0,1 -> 位 101 -> 0b1010_0000
        let bytes = png_build(
            3,
            1,
            1,
            3,
            0,
            Some(&palette),
            Some(&[255, 0]),
            &[vec![0b1010_0000]],
        );
        let img = decode(&bytes).unwrap();
        assert_eq!(img.get(0, 0), [200, 100, 50, 0]);
        assert_eq!(img.get(1, 0), [10, 20, 30, 255]);
        assert_eq!(img.get(2, 0), [200, 100, 50, 0]);
    }

    #[test]
    fn png_probe_reports_trns_alpha() {
        let palette = [[1, 2, 3], [4, 5, 6]];
        let with = png_build(2, 1, 1, 3, 0, Some(&palette), Some(&[255, 0]), &[vec![0x40]]);
        assert!(probe(&with).unwrap().has_alpha);
        let without = png_build(2, 1, 1, 3, 0, Some(&palette), None, &[vec![0x40]]);
        assert!(!probe(&without).unwrap().has_alpha);
    }

    #[test]
    fn png_trns_key_color_gray() {
        // 灰度 8 位，键色 7：命中即全透明
        let bytes = png_build(2, 1, 8, 0, 0, None, Some(&[0, 7]), &[vec![7, 9]]);
        let img = decode(&bytes).unwrap();
        assert_eq!(img.get(0, 0), [7, 7, 7, 0]);
        assert_eq!(img.get(1, 0), [9, 9, 9, 255]);
    }

    #[test]
    fn png_trns_key_color_rgb() {
        let bytes = png_build(
            2,
            1,
            8,
            2,
            0,
            None,
            Some(&[0, 1, 0, 2, 0, 3]),
            &[vec![1, 2, 3, 4, 5, 6]],
        );
        let img = decode(&bytes).unwrap();
        assert_eq!(img.get(0, 0), [1, 2, 3, 0]);
        assert_eq!(img.get(1, 0), [4, 5, 6, 255]);
    }

    /// 把一张 RGBA8 图按 Adam7 拆成七遍的原始行（过滤器 None）。
    fn adam7_rows(img: &Image) -> Vec<Vec<u8>> {
        let mut rows = Vec::new();
        for &(x0, y0, dx, dy) in PNG_ADAM7.iter() {
            let mut y = y0;
            while y < img.height as usize {
                let mut row = Vec::new();
                let mut x = x0;
                while x < img.width as usize {
                    row.extend_from_slice(&img.get(x as u32, y as u32));
                    x += dx;
                }
                if !row.is_empty() {
                    rows.push(row);
                }
                y += dy;
            }
        }
        rows
    }

    #[test]
    fn png_decodes_adam7_interlaced() {
        let mut img = Image::new(7, 5);
        for y in 0..5u32 {
            for x in 0..7u32 {
                img.put(x, y, [(x * 30) as u8, (y * 40) as u8, (x * y) as u8, 255]);
            }
        }
        let rows = adam7_rows(&img);
        let bytes = png_build(7, 5, 8, 6, 1, None, None, &rows);
        let decoded = decode(&bytes).unwrap();
        assert_eq!((decoded.width, decoded.height), (7, 5));
        assert_eq!(decoded, img);
    }

    #[test]
    fn png_rejects_invalid_depth() {
        // 真彩色不支持 4 位
        let bytes = png_build(1, 1, 4, 2, 0, None, None, &[vec![0, 0]]);
        assert!(decode(&bytes).is_err());
    }

    /// 拼一张 BMP：`extra` 紧跟 40 字节 DIB 头（位域掩码放这里）。
    #[allow(clippy::too_many_arguments)]
    fn bmp_build(
        width: i32,
        height: i32,
        bpp: u16,
        compression: u32,
        extra: &[u8],
        rows: &[Vec<u8>],
    ) -> Vec<u8> {
        let dib_size = 40u32 + extra.len() as u32;
        let row_stride = ((bpp as usize * width as usize + 31) / 32) * 4;
        let mut pixels = Vec::new();
        for row in rows {
            let start = pixels.len();
            pixels.extend_from_slice(row);
            pixels.resize(start + row_stride, 0);
        }
        let offset = 14 + dib_size as usize;
        let mut out = Vec::new();
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&((offset + pixels.len()) as u32).to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        out.extend_from_slice(&dib_size.to_le_bytes());
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&bpp.to_le_bytes());
        out.extend_from_slice(&compression.to_le_bytes());
        out.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        out.extend_from_slice(&2835u32.to_le_bytes());
        out.extend_from_slice(&2835u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(extra);
        out.extend_from_slice(&pixels);
        out
    }

    #[test]
    fn bmp_decodes_16bit_bitfields() {
        // 红：0xF800（5-6-5）
        let extra: Vec<u8> = [0xF800u32, 0x07E0, 0x001F]
            .iter()
            .flat_map(|m| m.to_le_bytes())
            .collect();
        let rows = vec![vec![0x00, 0xF8, 0xE0, 0x07]];
        let bytes = bmp_build(2, 1, 16, 3, &extra, &rows);
        let img = decode(&bytes).unwrap();
        assert_eq!(img.get(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.get(1, 0), [0, 255, 0, 255]);
    }

    #[test]
    fn bmp_decodes_32bit_bitfields_with_alpha() {
        // V3 头（56 字节）带 alpha 掩码
        let extra: Vec<u8> = [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000]
            .iter()
            .flat_map(|m| m.to_le_bytes())
            .collect();
        // 小端存储 -> 值 0x80302010：R=0x30 G=0x20 B=0x10 A=0x80
        let rows = vec![vec![0x10, 0x20, 0x30, 0x80]];
        let bytes = bmp_build(1, 1, 32, 6, &extra, &rows);
        let img = decode(&bytes).unwrap();
        assert_eq!(img.get(0, 0), [0x30, 0x20, 0x10, 0x80]);
    }

    #[test]
    fn bmp_32bit_rgb_ignores_reserved_byte() {
        // BI_RGB 32 位第 4 字节是保留位，不能当 alpha
        // 保存顺序是 B,G,R,X（小端），所以 RGB 是 0x30/0x20/0x10，X 被忽略
        let rows = vec![vec![0x10, 0x20, 0x30, 0x00]];
        let bytes = bmp_build(1, 1, 32, 0, &[], &rows);
        let img = decode(&bytes).unwrap();
        assert_eq!(img.get(0, 0), [0x30, 0x20, 0x10, 255]);
    }

    #[test]
    fn bmp_rejects_rle_compression() {
        // BI_RLE8(1)：位深合法但不是位域压缩，必须明确拒绝
        let bytes = bmp_build(1, 1, 16, 1, &[], &[vec![0, 0]]);
        let err = decode(&bytes).unwrap_err();
        assert!(err.contains("压缩"), "{err}");
    }

    #[test]
    fn resize_and_crop() {
        let img = sample();
        let fit = resize_fit(&img, 2, 2);
        assert_eq!((fit.width, fit.height), (2, 2));
        let cover = resize_cover(&img, 2, 2);
        assert_eq!((cover.width, cover.height), (2, 2));
        let contain = resize_contain(&img, 8, 8, [255, 255, 255, 255]);
        assert_eq!((contain.width, contain.height), (8, 8));
        assert_eq!(contain.get(0, 0), [255, 255, 255, 255]);
        let cropped = crop(&img, 1, 1, 2, 2);
        assert_eq!(cropped.get(0, 0), img.get(1, 1));
    }

    #[test]
    fn resize_keeps_alpha_edges_clean() {
        // 左半不透明红、右半完全透明：缩小后透明区域不应出现「红黑边」
        let mut img = Image::new(8, 1);
        for x in 0..4 {
            img.put(x, 0, [255, 0, 0, 255]);
        }
        let scaled = resize(&img, 4, 1);
        let last = scaled.get(3, 0);
        assert_eq!(last[3], 0, "完全透明的像素 alpha 应为 0");
        let first = scaled.get(0, 0);
        assert_eq!(first[3], 255);
        assert!(first[0] > 200, "{first:?}");
    }

    #[test]
    fn composite_alpha() {
        let mut canvas = Image::filled(2, 2, [0, 0, 0, 255]);
        let mut mark = Image::new(2, 2);
        mark.put(0, 0, [255, 255, 255, 128]);
        canvas.composite(&mark, 0, 0, 255);
        let pixel = canvas.get(0, 0);
        assert!(pixel[0] > 120 && pixel[0] < 140, "{pixel:?}");
        assert_eq!(pixel[3], 255);
    }

    #[test]
    fn flatten_removes_transparency() {
        let mut img = Image::filled(1, 1, [10, 20, 30, 0]);
        img.put(0, 0, [10, 20, 30, 0]);
        let flat = img.flatten([255, 255, 255, 255]);
        assert_eq!(flat.get(0, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn jpeg_roundtrip_through_engine() {
        let mut img = Image::filled(24, 16, [200, 120, 60, 255]);
        img.put(0, 0, [255, 255, 255, 255]);
        let bytes = encode(&img, "jpg").unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!((decoded.width, decoded.height), (24, 16));
        let pixel = decoded.get(12, 8);
        assert!((pixel[0] as i32 - 200).abs() < 30, "{pixel:?}");
        assert_eq!(pixel[3], 255);
    }

    #[test]
    fn probe_reads_headers() {
        let png = encode_png(&sample());
        let info = probe(&png).unwrap();
        assert_eq!(info.format, Format::Png);
        assert_eq!((info.width, info.height), (4, 4));
        assert!(info.has_alpha);

        let jpeg = encode(&sample(), "jpg").unwrap();
        let info = probe(&jpeg).unwrap();
        assert_eq!(info.format, Format::Jpeg);
        assert_eq!((info.width, info.height), (4, 4));
        assert!(!info.has_alpha);

        let bmp = encode_bmp(&sample());
        let info = probe(&bmp).unwrap();
        assert_eq!(info.format, Format::Bmp);
    }

    #[test]
    fn rejects_unknown_format() {
        assert!(decode(b"not-an-image").is_err());
        assert!(probe(b"not-an-image").is_err());
        // GIF 不能写出，但可以识别
        assert!(encode(&sample(), "gif").is_err());
        assert!(encode(&sample(), "webp").is_err());
        assert!(supports_extension(".png"));
        assert!(supports_extension("JPG"));
        assert!(supports_extension(".bmp"));
        // GIF 只能解码、不能回写，因此不算「可处理」
        assert!(!supports_extension(".gif"));
        assert!(!supports_extension(".webp"));
        assert!(!supports_extension(".tiff"));
    }

    #[test]
    fn gif_decodes_but_does_not_encode() {
        // 最小 GIF（2×2 单色），只验证引擎的格式分派
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"GIF89a");
        bytes.extend_from_slice(&[0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00]);
        bytes.extend_from_slice(&[255, 255, 255, 0, 0, 0]);
        bytes.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0]);
        bytes.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00, 0x3B]);
        // 上面这段码流只求「能被识别为 GIF」，尺寸解析必须正确
        let info = probe(&bytes).unwrap();
        assert_eq!(info.format, Format::Gif);
        assert_eq!((info.width, info.height), (1, 1));
        assert!(encode(&sample(), "gif").is_err());
    }

    #[test]
    fn webp_reports_size_but_refuses_decode() {
        // 构造一个最小的 VP8X 头部
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(b"WEBP");
        bytes.extend_from_slice(b"VP8X");
        bytes.extend_from_slice(&[10, 0, 0, 0]); // chunk size
        bytes.extend_from_slice(&[0x02, 0, 0, 0]); // flags：含动画
        bytes.extend_from_slice(&[99, 0, 0]); // 宽 - 1 = 99 → 100
        bytes.extend_from_slice(&[49, 0, 0]); // 高 - 1 = 49 → 50
        let info = probe(&bytes).unwrap();
        assert_eq!(info.format, Format::Webp);
        assert_eq!((info.width, info.height), (100, 50));
        assert!(info.animated);
        let err = decode(&bytes).unwrap_err();
        assert!(err.contains("WebP"), "{err}");
    }

    #[test]
    fn length_and_distance_codes_cover_ranges() {
        assert_eq!(length_code(3).0, 0);
        assert_eq!(length_code(258).0, 28);
        // 258 是最后一档，额外位为 0
        assert_eq!(length_code(258).2, 0);

        assert_eq!(distance_code(1).0, 0);
        assert_eq!(distance_code(24577).0, 29);
        assert_eq!(distance_code(24577).2, 13);
    }
}

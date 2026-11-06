//! 本包内置的最小图片处理引擎（P2）。
//!
//! 项目当前没有引入图片处理依赖（需求见 `tmp/coord-P2.md`），这里只做
//! **最小可用**的纯 Rust 实现，供本包的图片相关端点使用：
//! - 解码：PNG（8 位、非隔行，灰度/RGB/调色板/灰度+透明/RGBA）、BMP（24/32 位、BI_RGB）；
//! - 编码：PNG（RGBA8，zlib 存储块）与 BMP（24 位）；
//! - 变换：双线性缩放、等比缩放、居中裁剪（cover）、留白（contain）、透明合成。
//!
//! 未覆盖 JPEG / WebP / GIF 等格式的解码，调用方需按 `Err` 文案提示用户。

/// 解码失败文案（供上层直接展示）。
pub const UNSUPPORTED_FORMAT: &str = "暂不支持该图片格式的解码（当前最小实现仅支持 PNG/BMP）";

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
        let src_a = (src[3] as u32 * opacity as u32 / 255) as u8;
        if src_a == 0 {
            return;
        }
        let dst = self.get(x as u32, y as u32);
        let sa = src_a as f32 / 255.0;
        let da = dst[3] as f32 / 255.0;
        let out_a = sa + da * (1.0 - sa);
        if out_a <= 0.0 {
            return;
        }
        let channel = |s: u8, d: u8| -> u8 {
            ((s as f32 * sa + d as f32 * da * (1.0 - sa)) / out_a).round() as u8
        };
        self.put(
            x as u32,
            y as u32,
            [
                channel(src[0], dst[0]),
                channel(src[1], dst[1]),
                channel(src[2], dst[2]),
                (out_a * 255.0).round() as u8,
            ],
        );
    }

    /// 抢占式合成整张 `src` 到 `(x, y)`。
    pub fn composite(&mut self, src: &Image, x: i64, y: i64, opacity: u8) {
        for sy in 0..src.height {
            for sx in 0..src.width {
                let pixel = src.get(sx, sy);
                if pixel[3] > 0 {
                    self.blend(x + sx as i64, y + sy as i64, pixel, opacity);
                }
            }
        }
    }
}

/// 嗅探并解码 PNG / BMP。
pub fn decode(bytes: &[u8]) -> Result<Image, String> {
    if bytes.len() >= 8 && bytes[0..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return decode_png(bytes);
    }
    if bytes.len() >= 2 && &bytes[0..2] == b"BM" {
        return decode_bmp(bytes);
    }
    Err(UNSUPPORTED_FORMAT.to_string())
}

/// 扩展名是否属于当前引擎可解码格式。
pub fn supports_extension(ext: &str) -> bool {
    matches!(ext.trim_start_matches('.').to_ascii_lowercase().as_str(), "png" | "bmp")
}

/// 按扩展名编码（PNG / BMP）；其它扩展名返回 `Err`。
pub fn encode(img: &Image, ext: &str) -> Result<Vec<u8>, String> {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "png" => Ok(encode_png(img)),
        "bmp" => Ok(encode_bmp(img)),
        _ => Err("当前最小实现仅支持写出 PNG/BMP".to_string()),
    }
}

// ---------------------------- PNG 解码 -------------------------------------

fn decode_png(bytes: &[u8]) -> Result<Image, String> {
    let mut pos = 8usize;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut depth = 0u8;
    let mut color_type = 0u8;
    let mut interlace = 0u8;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut palette_alpha: Vec<u8> = Vec::new();
    let mut idat: Vec<u8> = Vec::new();

    while pos + 8 <= bytes.len() {
        let length = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
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
                width = u32::from_be_bytes([bytes[start], bytes[start + 1], bytes[start + 2], bytes[start + 3]]);
                height = u32::from_be_bytes([bytes[start + 4], bytes[start + 5], bytes[start + 6], bytes[start + 7]]);
                depth = bytes[start + 8];
                color_type = bytes[start + 9];
                interlace = bytes[start + 12];
            }
            b"PLTE" => {
                palette = bytes[start..end]
                    .chunks_exact(3)
                    .map(|c| [c[0], c[1], c[2]])
                    .collect();
            }
            b"tRNS" => {
                if color_type == 3 {
                    palette_alpha = bytes[start..end].to_vec();
                }
            }
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
    if depth != 8 {
        return Err("暂不支持该 PNG 位深（仅支持 8 位）".to_string());
    }
    if interlace != 0 {
        return Err("暂不支持隔行 PNG".to_string());
    }
    let channels = match color_type {
        0 => 1usize,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => return Err("暂不支持该 PNG 颜色类型".to_string()),
    };

    let stride = width as usize * channels;
    let expected = (stride + 1) * height as usize;
    let raw = inflate(&idat, expected)?;
    if raw.len() < expected {
        return Err("PNG 数据不完整".to_string());
    }

    // 反过滤
    let mut pixels = vec![0u8; stride * height as usize];
    let bpp = channels;
    for row in 0..height as usize {
        let filter = raw[row * (stride + 1)];
        let src = &raw[row * (stride + 1) + 1..row * (stride + 1) + 1 + stride];
        let dst_start = row * stride;
        for i in 0..stride {
            let left = if i >= bpp { pixels[dst_start + i - bpp] } else { 0 };
            let up = if row > 0 { pixels[dst_start - stride + i] } else { 0 };
            let up_left = if row > 0 && i >= bpp { pixels[dst_start - stride + i - bpp] } else { 0 };
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

    let mut image = Image::new(width, height);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let base = y * stride + x * channels;
            let rgba = match color_type {
                0 => {
                    let gray = pixels[base];
                    [gray, gray, gray, 255]
                }
                2 => [pixels[base], pixels[base + 1], pixels[base + 2], 255],
                3 => {
                    let index = pixels[base] as usize;
                    let color = palette.get(index).copied().unwrap_or([0, 0, 0]);
                    let alpha = palette_alpha.get(index).copied().unwrap_or(255);
                    [color[0], color[1], color[2], alpha]
                }
                4 => {
                    let gray = pixels[base];
                    [gray, gray, gray, pixels[base + 1]]
                }
                6 => [pixels[base], pixels[base + 1], pixels[base + 2], pixels[base + 3]],
                _ => unreachable!(),
            };
            image.put(x as u32, y as u32, rgba);
        }
    }
    Ok(image)
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

    // 原始扫描线：每行前缀 filter=0
    let mut raw = Vec::with_capacity((img.width as usize * 4 + 1) * img.height as usize);
    for y in 0..img.height as usize {
        raw.push(0);
        raw.extend_from_slice(&img.pixels[y * img.width as usize * 4..(y + 1) * img.width as usize * 4]);
    }
    write_chunk(&mut out, b"IDAT", &zlib_store(&raw));
    write_chunk(&mut out, b"IEND", &[]);
    out
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

/// zlib 容器 + DEFLATE 存储块（不压缩，但格式合法）。
fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 65535 + 16);
    out.extend_from_slice(&[0x78, 0x01]); // CM=8, CINFO=7, FCHECK 正确
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
    } else {
        let mut offset = 0usize;
        while offset < data.len() {
            let take = (data.len() - offset).min(65535);
            let final_block = offset + take >= data.len();
            out.push(if final_block { 0x01 } else { 0x00 });
            out.extend_from_slice(&(take as u16).to_le_bytes());
            out.extend_from_slice(&(!(take as u16)).to_le_bytes());
            out.extend_from_slice(&data[offset..offset + take]);
            offset += take;
        }
    }
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

// ---------------------------- DEFLATE 解压 --------------------------------

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0, bit: 0 }
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

    fn read_bits(&mut self, count: u8) -> Result<u32, String> {
        let mut value = 0u32;
        for _ in 0..count {
            value = (value << 1) | self.read_bit()?;
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
        Ok(Huffman { counts, first_code, first_index, symbols })
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
                const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
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

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

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
                let length = LENGTH_BASE[index] as usize + reader.read_bits(LENGTH_EXTRA[index])? as usize;
                let dist_symbol = dist.decode(reader)? as usize;
                if dist_symbol >= 30 {
                    return Err("DEFLATE 距离符号非法".to_string());
                }
                let distance = DIST_BASE[dist_symbol] as usize + reader.read_bits(DIST_EXTRA[dist_symbol])? as usize;
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
    if compression != 0 {
        return Err("暂不支持压缩 BMP".to_string());
    }
    if bpp != 24 && bpp != 32 {
        return Err("暂不支持该 BMP 位深（仅支持 24/32 位）".to_string());
    }
    let width = width as u32;
    let height_abs = height.unsigned_abs();
    if width as u64 * height_abs as u64 > MAX_PIXELS {
        return Err("图片尺寸过大".to_string());
    }
    let top_down = height < 0;
    let row_stride = ((bpp as usize * width as usize + 31) / 32) * 4;
    let mut image = Image::new(width, height_abs);
    for row in 0..height_abs {
        let src_row = if top_down { row } else { height_abs - 1 - row };
        let base = pixel_offset + src_row as usize * row_stride;
        if base + row_stride > bytes.len() {
            return Err("BMP 像素数据不完整".to_string());
        }
        for x in 0..width as usize {
            let p = base + x * (bpp as usize / 8);
            let b = bytes[p];
            let g = bytes[p + 1];
            let r = bytes[p + 2];
            image.put(x as u32, row, [r, g, b, 255]);
        }
    }
    Ok(image)
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

/// 双线性缩放到指定尺寸。
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
    let mut out = Image::new(width, height);
    let sx = img.width as f32 / width as f32;
    let sy = img.height as f32 / height as f32;
    for y in 0..height {
        let src_y = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
        let y0 = src_y.floor() as u32;
        let y1 = (y0 + 1).min(img.height.saturating_sub(1));
        let fy = src_y - y0 as f32;
        for x in 0..width {
            let src_x = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
            let x0 = src_x.floor() as u32;
            let x1 = (x0 + 1).min(img.width.saturating_sub(1));
            let fx = src_x - x0 as f32;
            let p00 = img.get(x0, y0);
            let p10 = img.get(x1, y0);
            let p01 = img.get(x0, y1);
            let p11 = img.get(x1, y1);
            let mut rgba = [0u8; 4];
            for c in 0..4 {
                let top = p00[c] as f32 * (1.0 - fx) + p10[c] as f32 * fx;
                let bottom = p01[c] as f32 * (1.0 - fx) + p11[c] as f32 * fx;
                rgba[c] = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
            }
            out.put(x, y, rgba);
        }
    }
    out
}

/// 等比缩放，使结果完全落在 `width × height` 内（不放大）。
pub fn resize_fit(img: &Image, width: u32, height: u32) -> Image {
    if img.width == 0 || img.height == 0 {
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

/// 等比缩放后居中裁剪到目标尺寸（对应 Go 的 `ThumbnailCrop` 裁剪模式）。
pub fn resize_cover(img: &Image, width: u32, height: u32) -> Image {
    let scaled = resize_fit_cover(img, width, height);
    if scaled.width == width && scaled.height == height {
        return scaled;
    }
    let offset_x = (scaled.width.saturating_sub(width)) / 2;
    let offset_y = (scaled.height.saturating_sub(height)) / 2;
    crop(&scaled, offset_x, offset_y, width, height)
}

fn resize_fit_cover(img: &Image, width: u32, height: u32) -> Image {
    if img.width == 0 || img.height == 0 {
        return Image::new(width, height);
    }
    let ratio = (width as f32 / img.width as f32).max(height as f32 / img.height as f32);
    let new_width = ((img.width as f32 * ratio).round() as u32).max(width);
    let new_height = ((img.height as f32 * ratio).round() as u32).max(height);
    resize(img, new_width, new_height)
}

/// 等比缩放后居中留白到目标尺寸（对应 Go 的 `ThumbnailCrop` 补白模式）。
pub fn resize_contain(img: &Image, width: u32, height: u32, background: [u8; 4]) -> Image {
    let fitted = resize_fit(img, width, height);
    let mut canvas = Image::filled(width, height, background);
    let x = (width.saturating_sub(fitted.width)) / 2;
    let y = (height.saturating_sub(fitted.height)) / 2;
    canvas.composite(&fitted, x as i64, y as i64, 255);
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
                img.put(x, y, [(x * 60) as u8, (y * 60) as u8, 128, if x == 0 { 80 } else { 255 }]);
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
        let croped = crop(&img, 1, 1, 2, 2);
        assert_eq!(croped.get(0, 0), img.get(1, 1));
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
    fn rejects_unknown_format() {
        assert!(decode(b"not-an-image").is_err());
        assert!(encode(&sample(), "jpg").is_err());
        assert!(supports_extension(".png"));
        assert!(!supports_extension(".jpg"));
    }
}

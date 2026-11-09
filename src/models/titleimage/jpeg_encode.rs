//! 基线 JPEG 编码（4:2:0 采样 + 按图片频率生成的最优哈夫曼表）。
//!
//! 之所以需要自己实现：项目不引第三方图片依赖，而「给 JPEG 图片加水印/生成缩略图」
//! 必须能**写回 JPEG**（写成 PNG 会导致扩展名与内容不符，浏览器/静态服务按扩展名
//! 给 Content-Type，直接变成坏图）。编码器输出 SOF0 基线格式，任何解码器都能读。

use super::jpeg::{cos_table, ZIGZAG};

/// 默认编码质量（与既有接口契约一致）。
pub const DEFAULT_QUALITY: u8 = 90;

/// 标准亮度量化表（自然顺序）。
const STD_LUMA: [u8; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];

/// 标准色度量化表（自然顺序）。
const STD_CHROMA: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

/// 8×8 样本平面。
struct Plane {
    data: Vec<u8>,
    width: usize,
    height: usize,
}

impl Plane {
    fn at(&self, x: usize, y: usize) -> u8 {
        let x = x.min(self.width.saturating_sub(1));
        let y = y.min(self.height.saturating_sub(1));
        self.data[y * self.width + x]
    }
}

/// 编码 RGBA8 为 JPEG 字节流。
pub fn encode(width: u32, height: u32, rgba: &[u8], quality: u8) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err("JPEG 尺寸无效".to_string());
    }
    let expected = width as usize * height as usize * 4;
    if rgba.len() < expected {
        return Err("像素数据长度不足".to_string());
    }
    let width = width as usize;
    let height = height as usize;
    let quality = quality.clamp(1, 100);
    let cos = cos_table();

    let luma_quant = scaled_quant(&STD_LUMA, quality);
    let chroma_quant = scaled_quant(&STD_CHROMA, quality);

    // 组装 Y / Cb / Cr 平面（透明像素按白底合成，JPEG 没有 alpha 通道）。
    let (luma, cb, cr) = build_planes(width, height, rgba);

    let mcu_cols = (width + 15) / 16;
    let mcu_rows = (height + 15) / 16;

    // 第一遍：统计哈夫曼符号频率（0 = 亮度，1 = 色度）。
    let mut dc_freq = [[0u64; 256]; 2];
    let mut ac_freq = [[0u64; 256]; 2];
    let mut predicted = [0i32; 3];

    for my in 0..mcu_rows {
        for mx in 0..mcu_cols {
            for by in 0..2usize {
                for bx in 0..2usize {
                    let samples = y_block(&luma, mx, my, bx, by);
                    let coefficients = quantize(&fdct(&samples, &cos), &luma_quant);
                    record(
                        &coefficients,
                        &mut predicted[0],
                        &mut dc_freq[0],
                        &mut ac_freq[0],
                    );
                }
            }
            let samples = cb_block(&cb, mx, my);
            let coefficients = quantize(&fdct(&samples, &cos), &chroma_quant);
            record(
                &coefficients,
                &mut predicted[1],
                &mut dc_freq[1],
                &mut ac_freq[1],
            );

            let samples = cb_block(&cr, mx, my);
            let coefficients = quantize(&fdct(&samples, &cos), &chroma_quant);
            // Cb / Cr 共用第 1 组色度哈夫曼表（JPEG 只有 4 张表：DC0/AC0/DC1/AC1）
            record(
                &coefficients,
                &mut predicted[2],
                &mut dc_freq[1],
                &mut ac_freq[1],
            );
        }
    }

    let dc_luma = optimal_table(&dc_freq[0]);
    let ac_luma = optimal_table(&ac_freq[0]);
    let dc_chroma = optimal_table(&dc_freq[1]);
    let ac_chroma = optimal_table(&ac_freq[1]);

    // 第二遍：真正写熵编码数据。
    let mut out = Vec::with_capacity(width * height);
    out.extend_from_slice(&[0xFF, 0xD8]); // SOI
    push_segment(
        &mut out,
        0xE0,
        &[
            b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00,
        ],
    );

    // DQT：两张 8 位量化表
    let mut dqt = Vec::with_capacity(130);
    dqt.push(0x00);
    dqt.extend_from_slice(&luma_quant);
    dqt.push(0x01);
    dqt.extend_from_slice(&chroma_quant);
    push_segment(&mut out, 0xDB, &dqt);

    // SOF0：3 分量，Y 4:2:0（h=2,v=2），Cb/Cr 1:1
    let mut sof = Vec::with_capacity(15);
    sof.push(8);
    sof.extend_from_slice(&(height as u16).to_be_bytes());
    sof.extend_from_slice(&(width as u16).to_be_bytes());
    sof.push(3);
    sof.extend_from_slice(&[1, 0x22, 0x00]);
    sof.extend_from_slice(&[2, 0x11, 0x01]);
    sof.extend_from_slice(&[3, 0x11, 0x01]);
    push_segment(&mut out, 0xC0, &sof);

    // DHT：4 张表
    push_dht(&mut out, 0x00, &dc_luma);
    push_dht(&mut out, 0x10, &ac_luma);
    push_dht(&mut out, 0x01, &dc_chroma);
    push_dht(&mut out, 0x11, &ac_chroma);

    // SOS
    let mut sos = Vec::with_capacity(10);
    sos.push(3);
    sos.extend_from_slice(&[1, 0x00]);
    sos.extend_from_slice(&[2, 0x11]);
    sos.extend_from_slice(&[3, 0x11]);
    sos.extend_from_slice(&[0, 63, 0]);
    push_segment(&mut out, 0xDA, &sos);

    let mut writer = BitWriter::new();
    let mut predicted = [0i32; 3];
    for my in 0..mcu_rows {
        for mx in 0..mcu_cols {
            for by in 0..2usize {
                for bx in 0..2usize {
                    let samples = y_block(&luma, mx, my, bx, by);
                    let coefficients = quantize(&fdct(&samples, &cos), &luma_quant);
                    emit_block(
                        &mut writer,
                        &coefficients,
                        &mut predicted[0],
                        &dc_luma,
                        &ac_luma,
                    );
                }
            }
            let samples = cb_block(&cb, mx, my);
            let coefficients = quantize(&fdct(&samples, &cos), &chroma_quant);
            emit_block(
                &mut writer,
                &coefficients,
                &mut predicted[1],
                &dc_chroma,
                &ac_chroma,
            );

            let samples = cb_block(&cr, mx, my);
            let coefficients = quantize(&fdct(&samples, &cos), &chroma_quant);
            emit_block(
                &mut writer,
                &coefficients,
                &mut predicted[2],
                &dc_chroma,
                &ac_chroma,
            );
        }
    }
    writer.flush();
    out.extend_from_slice(&writer.out);
    out.extend_from_slice(&[0xFF, 0xD9]); // EOI
    Ok(out)
}

/// 按质量缩放标准量化表，返回**之字形顺序**的量化表。
fn scaled_quant(base: &[u8; 64], quality: u8) -> [u8; 64] {
    let quality = quality.clamp(1, 100) as u32;
    let scale = if quality < 50 {
        5000 / quality
    } else {
        200 - quality * 2
    };
    let mut out = [0u8; 64];
    for k in 0..64usize {
        let value = (base[ZIGZAG[k]] as u32 * scale + 50) / 100;
        out[k] = value.clamp(1, 255) as u8;
    }
    out
}

/// 把 RGBA 拆成 Y / Cb / Cr 平面（色度半分辨率）。
fn build_planes(width: usize, height: usize, rgba: &[u8]) -> (Plane, Plane, Plane) {
    let chroma_width = (width + 1) / 2;
    let chroma_height = (height + 1) / 2;
    let mut cb_raw = vec![0f32; width * height];
    let mut cr_raw = vec![0f32; width * height];
    let mut luma_raw = vec![0f32; width * height];

    for y in 0..height {
        for x in 0..width {
            let index = (y * width + x) * 4;
            let alpha = rgba[index + 3] as u32;
            let blend =
                |value: u8| -> f32 { ((value as u32 * alpha + 255 * (255 - alpha)) / 255) as f32 };
            let red = blend(rgba[index]);
            let green = blend(rgba[index + 1]);
            let blue = blend(rgba[index + 2]);
            let position = y * width + x;
            luma_raw[position] = 0.299 * red + 0.587 * green + 0.114 * blue;
            cb_raw[position] = -0.168_736 * red - 0.331_264 * green + 0.5 * blue + 128.0;
            cr_raw[position] = 0.5 * red - 0.418_688 * green - 0.081_312 * blue + 128.0;
        }
    }

    let luma = Plane {
        data: luma_raw
            .iter()
            .map(|value| value.round().clamp(0.0, 255.0) as u8)
            .collect(),
        width,
        height,
    };

    let downsample = |source: &[f32]| -> Plane {
        let mut data = vec![0u8; chroma_width * chroma_height];
        for y in 0..chroma_height {
            for x in 0..chroma_width {
                let mut sum = 0f32;
                let mut count = 0f32;
                for dy in 0..2usize {
                    for dx in 0..2usize {
                        let sx = (x * 2 + dx).min(width - 1);
                        let sy = (y * 2 + dy).min(height - 1);
                        sum += source[sy * width + sx];
                        count += 1.0;
                    }
                }
                data[y * chroma_width + x] = (sum / count).round().clamp(0.0, 255.0) as u8;
            }
        }
        Plane {
            data,
            width: chroma_width,
            height: chroma_height,
        }
    };
    let cb = downsample(&cb_raw);
    let cr = downsample(&cr_raw);
    (luma, cb, cr)
}

/// 取一个亮度 8×8 块（越界像素用边缘复制）。
fn y_block(plane: &Plane, mx: usize, my: usize, bx: usize, by: usize) -> [u8; 64] {
    let mut samples = [0u8; 64];
    for y in 0..8usize {
        for x in 0..8usize {
            samples[y * 8 + x] = plane.at(mx * 16 + bx * 8 + x, my * 16 + by * 8 + y);
        }
    }
    samples
}

/// 取一个色度 8×8 块（4:2:0 → 每个 MCU 一个块）。
fn cb_block(plane: &Plane, mx: usize, my: usize) -> [u8; 64] {
    let mut samples = [0u8; 64];
    for y in 0..8usize {
        for x in 0..8usize {
            samples[y * 8 + x] = plane.at(mx * 8 + x, my * 8 + y);
        }
    }
    samples
}

/// 正向 DCT（8×8，含 -128 电平偏移）。
fn fdct(samples: &[u8; 64], cos: &[f32; 64]) -> [f32; 64] {
    const INV_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let mut tmp = [0f32; 64];
    for y in 0..8usize {
        for u in 0..8usize {
            let mut sum = 0f32;
            for x in 0..8usize {
                sum += (samples[y * 8 + x] as f32 - 128.0) * cos[u * 8 + x];
            }
            let factor = if u == 0 { INV_SQRT2 } else { 1.0 };
            tmp[y * 8 + u] = sum * 0.5 * factor;
        }
    }
    let mut out = [0f32; 64];
    for u in 0..8usize {
        for v in 0..8usize {
            let mut sum = 0f32;
            for y in 0..8usize {
                sum += tmp[y * 8 + u] * cos[v * 8 + y];
            }
            let factor = if v == 0 { INV_SQRT2 } else { 1.0 };
            out[v * 8 + u] = sum * 0.5 * factor;
        }
    }
    out
}

/// 量化（返回自然顺序的系数）。
fn quantize(block: &[f32; 64], quant: &[u8; 64]) -> [i16; 64] {
    let mut out = [0i16; 64];
    for k in 0..64usize {
        let natural = ZIGZAG[k];
        let value = block[natural] / quant[k] as f32;
        out[natural] = value.round().clamp(-1023.0, 1023.0) as i16;
    }
    out
}

/// 值的幅度分类（需要的位数）。
fn magnitude(value: i32) -> u8 {
    let mut rest = value.unsigned_abs();
    let mut size = 0u8;
    while rest > 0 {
        size += 1;
        rest >>= 1;
    }
    size
}

/// 幅度的补码式编码位（与解码端 `extend` 互逆）。
fn amplitude(value: i32, size: u8) -> u32 {
    if value >= 0 {
        value as u32
    } else {
        (value + (1i32 << size) - 1) as u32
    }
}

/// 统计一个块的直流/交流符号频率。
fn record(
    coefficients: &[i16; 64],
    predicted: &mut i32,
    dc_freq: &mut [u64; 256],
    ac_freq: &mut [u64; 256],
) {
    let dc = coefficients[0] as i32;
    let diff = dc - *predicted;
    *predicted = dc;
    dc_freq[magnitude(diff) as usize] += 1;

    let mut run = 0usize;
    for k in 1..64usize {
        let value = coefficients[ZIGZAG[k]] as i32;
        if value == 0 {
            run += 1;
            continue;
        }
        while run > 15 {
            ac_freq[0xF0] += 1;
            run -= 16;
        }
        let size = magnitude(value);
        ac_freq[(run << 4) | size as usize] += 1;
        run = 0;
    }
    if run > 0 {
        ac_freq[0] += 1;
    }
}

/// 写一个块到输出位流。
fn emit_block(
    writer: &mut BitWriter,
    coefficients: &[i16; 64],
    predicted: &mut i32,
    dc: &HuffmanCodes,
    ac: &HuffmanCodes,
) {
    let dc_value = coefficients[0] as i32;
    let diff = dc_value - *predicted;
    *predicted = dc_value;
    let size = magnitude(diff);
    writer.write(dc.codes[size as usize], dc.lengths[size as usize]);
    if size > 0 {
        writer.write(amplitude(diff, size), size);
    }

    let mut run = 0usize;
    for k in 1..64usize {
        let value = coefficients[ZIGZAG[k]] as i32;
        if value == 0 {
            run += 1;
            continue;
        }
        while run > 15 {
            writer.write(ac.codes[0xF0], ac.lengths[0xF0]);
            run -= 16;
        }
        let size = magnitude(value);
        let symbol = (run << 4) | size as usize;
        writer.write(ac.codes[symbol], ac.lengths[symbol]);
        writer.write(amplitude(value, size), size);
        run = 0;
    }
    if run > 0 {
        writer.write(ac.codes[0], ac.lengths[0]);
    }
}

/// 一张哈夫曼表的编码侧视图。
struct HuffmanCodes {
    /// 每个符号的码值。
    codes: [u32; 256],
    /// 每个符号的码长。
    lengths: [u8; 256],
    /// 各码长的符号数量（下标 = 码长）。
    bits: [u8; 17],
    /// 按码长排序的符号列表。
    values: Vec<u8>,
}

/// 依据符号频率生成最优哈夫曼表（对齐 libjpeg 的 `jpeg_gen_optimal_table`）。
fn optimal_table(freq: &[u64; 256]) -> HuffmanCodes {
    const MAX_CLEN: usize = 32;
    let mut counts = vec![0u64; 257];
    counts[..256].copy_from_slice(freq);
    // 伪符号 256 保证「最长码不会全为 1」，并让表里至少有两个符号
    counts[256] = 1;

    let mut codesize = [0i32; 257];
    let mut others = [-1i32; 257];

    loop {
        let mut c1: i32 = -1;
        let mut least = u64::MAX;
        for (index, value) in counts.iter().enumerate() {
            if *value != 0 && *value <= least {
                least = *value;
                c1 = index as i32;
            }
        }
        let mut c2: i32 = -1;
        let mut least = u64::MAX;
        for (index, value) in counts.iter().enumerate() {
            if *value != 0 && *value <= least && index as i32 != c1 {
                least = *value;
                c2 = index as i32;
            }
        }
        if c2 < 0 {
            break;
        }
        counts[c1 as usize] += counts[c2 as usize];
        counts[c2 as usize] = 0;

        let mut cursor = c1 as usize;
        codesize[cursor] += 1;
        while others[cursor] >= 0 {
            cursor = others[cursor] as usize;
            codesize[cursor] += 1;
        }
        others[cursor] = c2;

        let mut cursor = c2 as usize;
        codesize[cursor] += 1;
        while others[cursor] >= 0 {
            cursor = others[cursor] as usize;
            codesize[cursor] += 1;
        }
    }

    // 按码长升序（同长度按符号升序）输出符号：这是 DHT 要求的顺序
    let mut values = Vec::with_capacity(257);
    let mut max_length = 0usize;
    for length in 1..=MAX_CLEN {
        for symbol in 0..=255usize {
            if codesize[symbol] as usize == length {
                values.push(symbol as u8);
                max_length = length;
            }
        }
    }

    let mut bits = [0usize; MAX_CLEN + 1];
    if !values.is_empty() && max_length <= 16 {
        // 哈夫曼树的码长天然满足 Kraft 不等式，直接用
        for symbol in values.iter() {
            bits[codesize[*symbol as usize] as usize] += 1;
        }
    } else if !values.is_empty() {
        // 极端长尾导致码长超过 16 位：整体退化成等长码（永远合法，只是略大）
        let mut length = 1usize;
        while (1usize << length) < values.len() {
            length += 1;
        }
        bits[length] = values.len();
    }

    let mut codes = [0u32; 256];
    let mut lengths = [0u8; 256];
    let mut next = [0u32; 17];
    let mut code = 0u32;
    for length in 1..=16usize {
        next[length] = code;
        code = (code + bits[length] as u32) << 1;
    }
    let mut cursor = 0usize;
    for length in 1..=16usize {
        for _ in 0..bits[length] {
            if cursor >= values.len() {
                break;
            }
            let symbol = values[cursor] as usize;
            codes[symbol] = next[length];
            lengths[symbol] = length as u8;
            next[length] += 1;
            cursor += 1;
        }
    }

    let mut bit_counts = [0u8; 17];
    for (index, value) in bits.iter().enumerate().take(17) {
        bit_counts[index] = (*value).min(255) as u8;
    }

    HuffmanCodes {
        codes,
        lengths,
        bits: bit_counts,
        values,
    }
}

/// 追加一个标记段（含长度字段）。
fn push_segment(out: &mut Vec<u8>, marker: u8, payload: &[u8]) {
    out.push(0xFF);
    out.push(marker);
    let length = (payload.len() + 2) as u16;
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(payload);
}

/// 追加一张 DHT 表；`class_id` 的高 4 位是表类型（0 直流 / 1 交流）。
fn push_dht(out: &mut Vec<u8>, class_id: u8, table: &HuffmanCodes) {
    let mut payload = Vec::with_capacity(1 + 16 + table.values.len());
    payload.push(class_id);
    payload.extend_from_slice(&table.bits[1..17]);
    payload.extend_from_slice(&table.values);
    push_segment(out, 0xC4, &payload);
}

/// 熵编码位写入器（MSB 优先，遇 `0xFF` 自动插入 `0x00`）。
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
        if count == 0 {
            return;
        }
        let masked = value & ((1u32 << count) - 1);
        self.buffer = (self.buffer << count) | masked;
        self.bits += count;
        while self.bits >= 8 {
            self.bits -= 8;
            let byte = ((self.buffer >> self.bits) & 0xFF) as u8;
            self.out.push(byte);
            if byte == 0xFF {
                self.out.push(0x00);
            }
        }
    }

    fn flush(&mut self) {
        if self.bits > 0 {
            let pad = 8 - self.bits;
            self.buffer = (self.buffer << pad) | ((1u32 << pad) - 1);
            self.bits += pad;
            while self.bits >= 8 {
                self.bits -= 8;
                let byte = ((self.buffer >> self.bits) & 0xFF) as u8;
                self.out.push(byte);
                if byte == 0xFF {
                    self.out.push(0x00);
                }
            }
        }
        self.buffer = 0;
        self.bits = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::super::jpeg;
    use super::*;

    /// 生成一张带渐变与色块的测试图。
    fn sample_image(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                let red = (x * 255 / width.max(1)) as u8;
                let green = (y * 255 / height.max(1)) as u8;
                let blue = if (x / 8 + y / 8) % 2 == 0 { 220 } else { 40 };
                pixels.extend_from_slice(&[red, green, blue, 255]);
            }
        }
        pixels
    }

    #[test]
    fn roundtrip_through_own_decoder() {
        let width = 34u32;
        let height = 21u32;
        let pixels = sample_image(width, height);
        let encoded = encode(width, height, &pixels, 90).unwrap();

        // 结构检查：SOI 开头、EOI 结尾
        assert_eq!(&encoded[0..2], &[0xFF, 0xD8]);
        assert_eq!(&encoded[encoded.len() - 2..], &[0xFF, 0xD9]);

        let info = jpeg::info(&encoded).unwrap();
        assert_eq!(info.width, width);
        assert_eq!(info.height, height);
        assert_eq!(info.components, 3);

        let (decoded_width, decoded_height, decoded) = jpeg::decode(&encoded).unwrap();
        assert_eq!((decoded_width, decoded_height), (width, height));

        // 有损压缩：平均绝对误差应比较小
        let mut total = 0f64;
        let mut count = 0f64;
        for index in 0..(width as usize * height as usize) {
            for channel in 0..3usize {
                let a = pixels[index * 4 + channel] as f64;
                let b = decoded[index * 4 + channel] as f64;
                total += (a - b).abs();
                count += 1.0;
            }
        }
        let mean = total / count;
        assert!(mean < 25.0, "平均误差过大: {mean}");
    }

    #[test]
    fn quality_scales_quantization_table() {
        let low = scaled_quant(&STD_LUMA, 20);
        let high = scaled_quant(&STD_LUMA, 95);
        // 质量越低，量化步长越大
        assert!(low[1] >= high[1]);
        // 都必须在 1..=255
        assert!(low.iter().all(|value| *value >= 1));
        assert!(high.iter().all(|value| *value >= 1));
        // 之字形顺序：第 0 项对应自然顺序的 0
        assert_eq!(ZIGZAG[0], 0);
    }

    #[test]
    fn optimal_table_is_complete() {
        let mut freq = [0u64; 256];
        freq[0] = 500;
        freq[0x0F] = 10;
        freq[0xF0] = 3;
        let table = optimal_table(&freq);
        // 出现过的符号都必须有码；没出现过的符号允许没有
        for symbol in [0usize, 0x0F, 0xF0] {
            assert!(table.lengths[symbol] > 0, "符号 {symbol} 缺少码");
        }
        // 哈夫曼前缀码：任意两个码不能互相前缀
        for first in 0..256usize {
            if table.lengths[first] == 0 {
                continue;
            }
            for second in 0..256usize {
                if first == second || table.lengths[second] == 0 {
                    continue;
                }
                let (code_a, len_a) = (table.codes[first], table.lengths[first] as u32);
                let (code_b, len_b) = (table.codes[second], table.lengths[second] as u32);
                if len_a <= len_b {
                    let shifted = code_b >> (len_b - len_a);
                    assert_ne!(shifted, code_a, "符号 {first} 的码是 {second} 的前缀");
                }
            }
        }
        // 码长不能超过 16 位（JPEG 硬性限制）
        assert!(table.lengths.iter().all(|length| *length <= 16));
        // DHT 里记录的数量应与符号总数一致
        let total: usize = table.bits.iter().skip(1).map(|value| *value as usize).sum();
        assert_eq!(total, table.values.len());
    }

    #[test]
    fn magnitude_and_amplitude_are_inverse() {
        for value in [-1023i32, -255, -4, -1, 0, 1, 4, 255, 1023] {
            let size = magnitude(value);
            let bits = amplitude(value, size);
            // 与解码端的 extend 互逆
            let threshold = 1i32 << (size.saturating_sub(1));
            let restored = if size == 0 {
                0
            } else if (bits as i32) < threshold {
                bits as i32 - (1 << size) + 1
            } else {
                bits as i32
            };
            assert_eq!(restored, value, "size={size} bits={bits}");
        }
    }

    #[test]
    fn writer_stuffs_ff_bytes() {
        let mut writer = BitWriter::new();
        writer.write(0xFF, 8);
        writer.write(0xFF, 8);
        writer.flush();
        // 两个 0xFF 各自后面插入 0x00
        assert_eq!(writer.out, vec![0xFF, 0x00, 0xFF, 0x00]);
    }
}

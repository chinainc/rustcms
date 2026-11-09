//! 基线 JPEG（SOF0 / SOF1）解码。
//!
//! 覆盖范围（与既有约定的解码能力对齐）：
//! - 8 位精度、单分量灰度 / 三分量 YCbCr；
//! - 任意采样因子（4:4:4 / 4:2:2 / 4:2:0 / 4:1:1 等），上采样用最近邻；
//! - 支持 DRI 重启间隔；
//! - 渐进式（SOF2）、算术编码（SOF9 等）、无损（SOF3）、CMYK 四分量
//! 一律返回**明确的中文错误**，不做假成功。
//!
//! 解码结果统一为 RGBA8（`pixels` 长度 = `width * height * 4`）。

/// 单张图片的像素上限，避免解码超大文件耗尽内存。
const MAX_PIXELS: u64 = 40_000_000;

/// 之字形顺序表：下标是之字形序号，值是行优先（自然）序号。
pub(crate) const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// 余弦查找表：`table[u * 8 + x] = cos((2x + 1) * u * π / 16)`。
///
/// 解码与编码共用；每张图片只算一次，避免每个 8×8 块都重复计算三角函数。
pub(crate) fn cos_table() -> [f32; 64] {
    let mut table = [0f32; 64];
    for u in 0..8usize {
        for x in 0..8usize {
            let angle = (2 * x + 1) as f64 * u as f64 * std::f64::consts::PI / 16.0;
            table[u * 8 + x] = angle.cos() as f32;
        }
    }
    table
}

/// JPEG 头部信息（只解析标记段，不做熵解码）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    /// 分量数量：1 = 灰度，3 = YCbCr。
    pub components: u8,
    /// 是否是渐进式（SOF2），当前不支持解码。
    pub progressive: bool,
}

/// 是否是 JPEG 数据（SOI 标记）。
pub fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xD8
}

/// 解析头部信息（宽高、分量数、是否渐进式）。
pub fn info(bytes: &[u8]) -> Result<Info, String> {
    if !is_jpeg(bytes) {
        return Err("不是有效的 JPEG 数据".to_string());
    }
    let mut pos = 2usize;
    while pos + 4 <= bytes.len() {
        if bytes[pos] != 0xFF {
            pos += 1;
            continue;
        }
        let marker = bytes[pos + 1];
        // 跳过填充字节
        if marker == 0xFF {
            pos += 1;
            continue;
        }
        // 无长度字段的独立标记
        if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            if marker == 0xD9 {
                break;
            }
            pos += 2;
            continue;
        }
        let length = u16::from_be_bytes([bytes[pos + 2], bytes[pos + 3]]) as usize;
        if length < 2 || pos + 2 + length > bytes.len() {
            return Err("JPEG 段长度非法".to_string());
        }
        if is_sof(marker) {
            let seg = &bytes[pos + 4..pos + 2 + length];
            if seg.len() < 6 {
                return Err("JPEG SOF 段数据损坏".to_string());
            }
            let height = u16::from_be_bytes([seg[1], seg[2]]) as u32;
            let width = u16::from_be_bytes([seg[3], seg[4]]) as u32;
            if width == 0 || height == 0 {
                return Err("JPEG 尺寸无效".to_string());
            }
            return Ok(Info {
                width,
                height,
                components: seg[5],
                progressive: marker == 0xC2,
            });
        }
        pos += 2 + length;
    }
    Err("JPEG 头部缺少 SOF 段".to_string())
}

/// 是否是 SOF（帧头）段：0xC0..=0xCF 中排除 C4（DHT）、C8（JPG）、CC（DAC）。
pub(crate) fn is_sof(marker: u8) -> bool {
    matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF)
}

/// 解码成 RGBA8：返回 `(宽, 高, 像素)`。
pub fn decode(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let header = info(bytes)?;
    if header.progressive {
        return Err("暂不支持渐进式 JPEG（SOF2）的解码，请另存为基线 JPEG".to_string());
    }
    if header.components != 1 && header.components != 3 {
        return Err(format!(
            "暂不支持 {} 分量的 JPEG（仅支持灰度与 YCbCr 三分量）",
            header.components
        ));
    }

    let mut pos = 2usize;
    let mut quant = [[0u16; 64]; 4];
    let mut quant_seen = [false; 4];
    let mut dc_tables: [Option<HuffmanTable>; 4] = [None, None, None, None];
    let mut ac_tables: [Option<HuffmanTable>; 4] = [None, None, None, None];
    let mut components: Vec<Component> = Vec::new();
    let mut width = 0u32;
    let mut height = 0u32;
    let mut restart_interval = 0usize;

    while pos + 4 <= bytes.len() {
        if bytes[pos] != 0xFF {
            pos += 1;
            continue;
        }
        let marker = bytes[pos + 1];
        if marker == 0xFF {
            pos += 1;
            continue;
        }
        match marker {
            0xD8 => {
                pos += 2;
                continue;
            }
            0xD9 => break,
            0x01 | 0xD0..=0xD7 => {
                pos += 2;
                continue;
            }
            _ => {}
        }
        let length = u16::from_be_bytes([bytes[pos + 2], bytes[pos + 3]]) as usize;
        if length < 2 || pos + 2 + length > bytes.len() {
            return Err("JPEG 段长度非法".to_string());
        }
        let seg = &bytes[pos + 4..pos + 2 + length];
        match marker {
            0xC0 | 0xC1 => {
                parse_sof(seg, &mut components, &mut width, &mut height)?;
            }
            0xC2 => {
                return Err("暂不支持渐进式 JPEG（SOF2）的解码，请另存为基线 JPEG".to_string());
            }
            0xC3 | 0xC5..=0xCF => {
                return Err("暂不支持该 JPEG 编码方式（仅支持基线 SOF0/SOF1）".to_string());
            }
            0xC4 => parse_dht(seg, &mut dc_tables, &mut ac_tables)?,
            0xDB => parse_dqt(seg, &mut quant, &mut quant_seen)?,
            0xDD => {
                if seg.len() >= 2 {
                    restart_interval = u16::from_be_bytes([seg[0], seg[1]]) as usize;
                }
            }
            0xDA => {
                parse_sos(seg, &mut components)?;
                let scan_start = pos + 2 + length;
                return decode_scan(
                    bytes,
                    scan_start,
                    width,
                    height,
                    &components,
                    &quant,
                    &quant_seen,
                    &dc_tables,
                    &ac_tables,
                    restart_interval,
                );
            }
            _ => {}
        }
        pos += 2 + length;
    }
    Err("JPEG 数据缺少扫描段（SOS）".to_string())
}

/// 单个分量（通道）的解码状态。
pub(crate) struct Component {
    pub id: u8,
    /// 水平采样因子 1..=4。
    pub h: usize,
    /// 垂直采样因子 1..=4。
    pub v: usize,
    pub tq: usize,
    pub td: usize,
    pub ta: usize,
    /// 该分量的样本平面（尺寸按 MCU 对齐的填充尺寸）。
    pub plane: Vec<u8>,
    pub stride: usize,
}

fn parse_sof(
    seg: &[u8],
    components: &mut Vec<Component>,
    width: &mut u32,
    height: &mut u32,
) -> Result<(), String> {
    if seg.len() < 6 {
        return Err("JPEG SOF 段数据损坏".to_string());
    }
    let precision = seg[0];
    if precision != 8 {
        return Err(format!("暂不支持 {precision} 位精度的 JPEG（仅支持 8 位）"));
    }
    let h = u16::from_be_bytes([seg[1], seg[2]]) as u32;
    let w = u16::from_be_bytes([seg[3], seg[4]]) as u32;
    if w == 0 || h == 0 {
        return Err("JPEG 尺寸无效".to_string());
    }
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err("图片尺寸过大".to_string());
    }
    *width = w;
    *height = h;
    let count = seg[5] as usize;
    if count == 0 || seg.len() < 6 + count * 3 {
        return Err("JPEG SOF 段数据损坏".to_string());
    }
    components.clear();
    for index in 0..count {
        let base = 6 + index * 3;
        let id = seg[base];
        let hv = seg[base + 1];
        let tq = seg[base + 2] as usize;
        let ch = (hv >> 4) as usize;
        let cv = (hv & 0x0F) as usize;
        if !(1..=4).contains(&ch) || !(1..=4).contains(&cv) {
            return Err("JPEG 采样因子非法".to_string());
        }
        if tq > 3 {
            return Err("JPEG 量化表编号非法".to_string());
        }
        components.push(Component {
            id,
            h: ch,
            v: cv,
            tq,
            td: 0,
            ta: 0,
            plane: Vec::new(),
            stride: 0,
        });
    }
    Ok(())
}

fn parse_dht(
    seg: &[u8],
    dc_tables: &mut [Option<HuffmanTable>; 4],
    ac_tables: &mut [Option<HuffmanTable>; 4],
) -> Result<(), String> {
    let mut pos = 0usize;
    while pos < seg.len() {
        if pos + 17 > seg.len() {
            return Err("JPEG 哈夫曼表数据损坏".to_string());
        }
        let class = seg[pos] >> 4;
        let id = (seg[pos] & 0x0F) as usize;
        if id > 3 {
            return Err("JPEG 哈夫曼表编号非法".to_string());
        }
        pos += 1;
        let mut bits = [0u8; 16];
        bits.copy_from_slice(&seg[pos..pos + 16]);
        pos += 16;
        let total: usize = bits.iter().map(|value| *value as usize).sum();
        if pos + total > seg.len() {
            return Err("JPEG 哈夫曼表数据损坏".to_string());
        }
        let values = seg[pos..pos + total].to_vec();
        pos += total;
        let table = HuffmanTable::new(&bits, values)?;
        if class == 0 {
            dc_tables[id] = Some(table);
        } else if class == 1 {
            ac_tables[id] = Some(table);
        } else {
            return Err("JPEG 哈夫曼表类型非法".to_string());
        }
    }
    Ok(())
}

fn parse_dqt(
    seg: &[u8],
    quant: &mut [[u16; 64]; 4],
    quant_seen: &mut [bool; 4],
) -> Result<(), String> {
    let mut pos = 0usize;
    while pos < seg.len() {
        let info = seg[pos];
        let precision = info >> 4;
        let id = (info & 0x0F) as usize;
        pos += 1;
        if id > 3 {
            return Err("JPEG 量化表编号非法".to_string());
        }
        if precision == 0 {
            if pos + 64 > seg.len() {
                return Err("JPEG 量化表数据损坏".to_string());
            }
            for index in 0..64 {
                quant[id][index] = seg[pos + index] as u16;
            }
            pos += 64;
        } else {
            if pos + 128 > seg.len() {
                return Err("JPEG 量化表数据损坏".to_string());
            }
            for index in 0..64 {
                quant[id][index] =
                    u16::from_be_bytes([seg[pos + index * 2], seg[pos + index * 2 + 1]]);
            }
            pos += 128;
        }
        quant_seen[id] = true;
    }
    Ok(())
}

fn parse_sos(seg: &[u8], components: &mut [Component]) -> Result<(), String> {
    if seg.is_empty() {
        return Err("JPEG SOS 段数据损坏".to_string());
    }
    let count = seg[0] as usize;
    if seg.len() < 1 + count * 2 {
        return Err("JPEG SOS 段数据损坏".to_string());
    }
    for index in 0..count {
        let id = seg[1 + index * 2];
        let tables = seg[2 + index * 2];
        let Some(component) = components.iter_mut().find(|item| item.id == id) else {
            return Err("JPEG SOS 引用了不存在的分量".to_string());
        };
        component.td = (tables >> 4) as usize;
        component.ta = (tables & 0x0F) as usize;
        if component.td > 3 || component.ta > 3 {
            return Err("JPEG 哈夫曼表编号非法".to_string());
        }
    }
    Ok(())
}

/// 把熵编码数据按重启标记切成若干段，并去掉字节填充（`FF 00` → `FF`）。
fn split_scan(bytes: &[u8], start: usize) -> Vec<Vec<u8>> {
    let mut segments: Vec<Vec<u8>> = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    let mut pos = start;
    while pos < bytes.len() {
        let byte = bytes[pos];
        if byte != 0xFF {
            current.push(byte);
            pos += 1;
            continue;
        }
        if pos + 1 >= bytes.len() {
            break;
        }
        let next = bytes[pos + 1];
        if next == 0x00 {
            current.push(0xFF);
            pos += 2;
            continue;
        }
        if (0xD0..=0xD7).contains(&next) {
            segments.push(std::mem::take(&mut current));
            pos += 2;
            continue;
        }
        // 其它标记：扫描数据结束
        break;
    }
    segments.push(current);
    segments
}

#[allow(clippy::too_many_arguments)]
fn decode_scan(
    bytes: &[u8],
    scan_start: usize,
    width: u32,
    height: u32,
    components: &[Component],
    quant: &[[u16; 64]; 4],
    quant_seen: &[bool; 4],
    dc_tables: &[Option<HuffmanTable>; 4],
    ac_tables: &[Option<HuffmanTable>; 4],
    restart_interval: usize,
) -> Result<(u32, u32, Vec<u8>), String> {
    for component in components {
        if !quant_seen[component.tq] {
            return Err("JPEG 缺少量化表".to_string());
        }
        if dc_tables[component.td].is_none() || ac_tables[component.ta].is_none() {
            return Err("JPEG 缺少哈夫曼表".to_string());
        }
    }

    let hmax = components.iter().map(|item| item.h).max().unwrap_or(1);
    let vmax = components.iter().map(|item| item.v).max().unwrap_or(1);
    let mcu_width = 8 * hmax;
    let mcu_height = 8 * vmax;
    let mcu_cols = (width as usize + mcu_width - 1) / mcu_width;
    let mcu_rows = (height as usize + mcu_height - 1) / mcu_height;
    let total_mcus = mcu_cols * mcu_rows;

    // 为每个分量分配按 MCU 对齐的样本平面。
    let mut planes: Vec<Component> = components
        .iter()
        .map(|component| {
            let stride = mcu_cols * component.h * 8;
            let rows = mcu_rows * component.v * 8;
            Component {
                id: component.id,
                h: component.h,
                v: component.v,
                tq: component.tq,
                td: component.td,
                ta: component.ta,
                plane: vec![0u8; stride * rows],
                stride,
            }
        })
        .collect();

    let cos = cos_table();
    let segments = split_scan(bytes, scan_start);
    let mut mcu_index = 0usize;
    let mut predictors = vec![0i32; planes.len()];

    for segment in segments.iter() {
        if mcu_index >= total_mcus {
            break;
        }
        let count = if restart_interval == 0 {
            total_mcus - mcu_index
        } else {
            restart_interval.min(total_mcus - mcu_index)
        };
        let mut reader = BitReader::new(segment);
        for _ in 0..count {
            let mcu_x = mcu_index % mcu_cols;
            let mcu_y = mcu_index / mcu_cols;
            for (ci, component) in planes.iter_mut().enumerate() {
                let dc = dc_tables[component.td].as_ref().unwrap();
                let ac = ac_tables[component.ta].as_ref().unwrap();
                for by in 0..component.v {
                    for bx in 0..component.h {
                        let block = decode_block(
                            &mut reader,
                            dc,
                            ac,
                            &quant[component.tq],
                            &mut predictors[ci],
                        )?;
                        let samples = idct(&block, &cos);
                        let base_x = (mcu_x * component.h + bx) * 8;
                        let base_y = (mcu_y * component.v + by) * 8;
                        for row in 0..8usize {
                            let offset = (base_y + row) * component.stride + base_x;
                            if offset + 8 <= component.plane.len() {
                                component.plane[offset..offset + 8]
                                    .copy_from_slice(&samples[row * 8..row * 8 + 8]);
                            }
                        }
                    }
                }
            }
            mcu_index += 1;
        }
        // 重启间隔后直流预测值清零
        for predictor in predictors.iter_mut() {
            *predictor = 0;
        }
    }

    if mcu_index < total_mcus {
        return Err("JPEG 扫描数据不完整".to_string());
    }

    // 组装 RGBA
    let mut pixels = vec![255u8; width as usize * height as usize * 4];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let index = (y * width as usize + x) * 4;
            if planes.len() == 1 {
                let value = sample(&planes[0], x, y, hmax, vmax);
                pixels[index] = value;
                pixels[index + 1] = value;
                pixels[index + 2] = value;
                pixels[index + 3] = 255;
                continue;
            }
            let luma = sample(&planes[0], x, y, hmax, vmax) as f32;
            let cb = sample(&planes[1], x, y, hmax, vmax) as f32 - 128.0;
            let cr = sample(&planes[2], x, y, hmax, vmax) as f32 - 128.0;
            let red = luma + 1.402 * cr;
            let green = luma - 0.344_136 * cb - 0.714_136 * cr;
            let blue = luma + 1.772 * cb;
            pixels[index] = red.round().clamp(0.0, 255.0) as u8;
            pixels[index + 1] = green.round().clamp(0.0, 255.0) as u8;
            pixels[index + 2] = blue.round().clamp(0.0, 255.0) as u8;
            pixels[index + 3] = 255;
        }
    }

    Ok((width, height, pixels))
}

/// 取分量平面上的样本（最近邻上采样）。
fn sample(component: &Component, x: usize, y: usize, hmax: usize, vmax: usize) -> u8 {
    let sx = (x * component.h / hmax).min(component.stride.saturating_sub(1));
    let rows = component.plane.len() / component.stride.max(1);
    let sy = (y * component.v / vmax).min(rows.saturating_sub(1));
    component.plane[sy * component.stride + sx]
}

/// 反量化 + 逆 DCT 后的系数块类型（自然顺序，含直流）。
type Block = [f32; 64];

fn decode_block(
    reader: &mut BitReader,
    dc: &HuffmanTable,
    ac: &HuffmanTable,
    quant: &[u16; 64],
    predictor: &mut i32,
) -> Result<Block, String> {
    let mut block: Block = [0f32; 64];
    let size = dc.decode(reader)? as u32;
    if size > 16 {
        return Err("JPEG 直流系数长度非法".to_string());
    }
    let diff = extend(reader.read_bits(size), size);
    *predictor += diff;
    block[0] = *predictor as f32 * quant[0] as f32;

    let mut k = 1usize;
    while k < 64 {
        let symbol = ac.decode(reader)?;
        let run = (symbol >> 4) as usize;
        let size = (symbol & 0x0F) as u32;
        if size == 0 {
            if run == 15 {
                k += 16;
                continue;
            }
            break;
        }
        k += run;
        if k > 63 {
            return Err("JPEG 交流系数越界".to_string());
        }
        let value = extend(reader.read_bits(size), size);
        let natural = ZIGZAG[k];
        block[natural] = value as f32 * quant[k] as f32;
        k += 1;
    }
    Ok(block)
}

/// JPEG 的幅度扩展：把 `size` 位的补码式编码还原成有符号整数。
fn extend(value: u32, size: u32) -> i32 {
    if size == 0 {
        return 0;
    }
    let threshold = 1i32 << (size - 1);
    let value = value as i32;
    if value < threshold {
        value - (1 << size) + 1
    } else {
        value
    }
}

/// 逆 DCT：输入自然顺序的反量化系数，输出 8×8 样本（含 +128 电平偏移）。
fn idct(block: &Block, cos: &[f32; 64]) -> [u8; 64] {
    const INV_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let mut tmp = [0f32; 64];
    for v in 0..8usize {
        for x in 0..8usize {
            let mut sum = 0f32;
            for u in 0..8usize {
                let factor = if u == 0 { INV_SQRT2 } else { 1.0 };
                sum += factor * block[v * 8 + u] * cos[u * 8 + x];
            }
            tmp[v * 8 + x] = sum * 0.5;
        }
    }
    let mut out = [0u8; 64];
    for y in 0..8usize {
        for x in 0..8usize {
            let mut sum = 0f32;
            for v in 0..8usize {
                let factor = if v == 0 { INV_SQRT2 } else { 1.0 };
                sum += factor * tmp[v * 8 + x] * cos[v * 8 + y];
            }
            let value = sum * 0.5 + 128.0;
            out[y * 8 + x] = value.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

// ---------------------------- 熵解码基础件 ---------------------------------

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buffer: u32,
    bits: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader {
            data,
            pos: 0,
            buffer: 0,
            bits: 0,
        }
    }

    fn fill(&mut self) {
        while self.bits <= 24 && self.pos < self.data.len() {
            self.buffer = (self.buffer << 8) | self.data[self.pos] as u32;
            self.pos += 1;
            self.bits += 8;
        }
    }

    fn read_bit(&mut self) -> u32 {
        if self.bits == 0 {
            self.fill();
        }
        if self.bits == 0 {
            // 数据耗尽时按 0 补齐（解码循环有上界，不会死循环）
            return 0;
        }
        self.bits -= 1;
        (self.buffer >> self.bits) & 1
    }

    fn read_bits(&mut self, count: u32) -> u32 {
        let mut value = 0u32;
        for _ in 0..count {
            value = (value << 1) | self.read_bit();
        }
        value
    }
}

/// JPEG 的规范哈夫曼解码表。
pub(crate) struct HuffmanTable {
    /// 每个码长的最大码值，`-1` 表示该长度没有码。
    max_code: [i32; 17],
    /// 每个码长的码值起始下标。
    value_ptr: [i32; 17],
    /// 每个码长起始码值。
    min_code: [i32; 17],
    values: Vec<u8>,
}

impl HuffmanTable {
    pub(crate) fn new(bits: &[u8; 16], values: Vec<u8>) -> Result<HuffmanTable, String> {
        let mut table = HuffmanTable {
            max_code: [-1; 17],
            value_ptr: [0; 17],
            min_code: [0; 17],
            values,
        };
        let mut code = 0i32;
        let mut index = 0i32;
        for length in 1..=16usize {
            let count = bits[length - 1] as i32;
            if count > 0 {
                table.value_ptr[length] = index;
                table.min_code[length] = code;
                code += count;
                index += count;
                table.max_code[length] = code - 1;
            } else {
                table.max_code[length] = -1;
            }
            code <<= 1;
        }
        if table.values.is_empty() {
            return Err("JPEG 哈夫曼表为空".to_string());
        }
        Ok(table)
    }

    fn decode(&self, reader: &mut BitReader) -> Result<u8, String> {
        let mut code = 0i32;
        for length in 1..=16usize {
            code = (code << 1) | reader.read_bit() as i32;
            if self.max_code[length] >= 0 && code <= self.max_code[length] {
                let index = self.value_ptr[length] + code - self.min_code[length];
                return self
                    .values
                    .get(index as usize)
                    .copied()
                    .ok_or_else(|| "JPEG 哈夫曼码越界".to_string());
            }
        }
        Err("JPEG 哈夫曼解码失败".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拼一个只含 APP0 + SOF0 的最小头部（没有扫描数据）。
    fn minimal_header() -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        // APP0（长度 16）
        out.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        out.extend_from_slice(b"JFIF\0");
        out.extend_from_slice(&[0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);
        // SOF0：8 位、高 24、宽 32、1 个分量
        out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x18, 0x00, 0x20, 0x01]);
        out.extend_from_slice(&[0x01, 0x11, 0x00]);
        out.extend_from_slice(&[0xFF, 0xD9]);
        out
    }

    #[test]
    fn parses_header() {
        let bytes = minimal_header();
        assert!(is_jpeg(&bytes));
        let parsed = info(&bytes).unwrap();
        assert_eq!(parsed.width, 32);
        assert_eq!(parsed.height, 24);
        assert_eq!(parsed.components, 1);
        assert!(!parsed.progressive);
    }

    #[test]
    fn rejects_non_jpeg_and_missing_header() {
        assert!(!is_jpeg(b"PNG"));
        assert!(info(b"not-a-jpeg").is_err());
        // 只有 SOI/EOI，没有 SOF
        assert!(info(&[0xFF, 0xD8, 0xFF, 0xD9]).is_err());
        // 截断
        assert!(decode(&[0xFF, 0xD8, 0xFF, 0xC0, 0x00]).is_err());
    }

    #[test]
    fn reports_progressive_as_unsupported() {
        let mut bytes = vec![0xFF, 0xD8];
        // SOF2：渐进式，8 位、高 16、宽 16、1 分量
        bytes.extend_from_slice(&[0xFF, 0xC2, 0x00, 0x0B, 0x08, 0x00, 0x10, 0x00, 0x10, 0x01]);
        bytes.extend_from_slice(&[0x01, 0x11, 0x00]);
        let parsed = info(&bytes).unwrap();
        assert!(parsed.progressive);
        let err = decode(&bytes).unwrap_err();
        assert!(err.contains("渐进式"), "{err}");
    }

    #[test]
    fn extend_signed_values() {
        assert_eq!(extend(0, 0), 0);
        // 1 位：0 → -1，1 → 1
        assert_eq!(extend(0, 1), -1);
        assert_eq!(extend(1, 1), 1);
        // 3 位：0b011 → 3，0b100 → 4，0b011.. 边界
        assert_eq!(extend(4, 3), 4);
        assert_eq!(extend(3, 3), -4);
        // 8 位
        assert_eq!(extend(255, 8), 255);
        assert_eq!(extend(0, 8), -255);
    }

    #[test]
    fn idct_of_dc_only_block_is_flat() {
        let cos = cos_table();
        let mut block: Block = [0f32; 64];
        // 直流系数 = 8 * 100 / 8 ... 取 8*40 时输出应约等于 40+128
        block[0] = 8.0 * 40.0;
        let samples = idct(&block, &cos);
        for value in samples {
            assert!((value as i32 - 168).abs() <= 1, "{value}");
        }
    }

    #[test]
    fn huffman_table_decodes_simple_codes() {
        // 两个码长 1 的码：符号 0 → 码 0，符号 1 → 码 1
        let mut bits = [0u8; 16];
        bits[0] = 2;
        let table = HuffmanTable::new(&bits, vec![0, 1]).unwrap();
        // 码流 0b01 后半段补零
        let data = [0b0100_0000u8];
        let mut reader = BitReader::new(&data);
        assert_eq!(table.decode(&mut reader).unwrap(), 0);
        assert_eq!(table.decode(&mut reader).unwrap(), 1);
    }

    #[test]
    fn zigzag_table_is_a_permutation() {
        let mut seen = [false; 64];
        for index in ZIGZAG {
            assert!(!seen[index], "重复下标 {index}");
            seen[index] = true;
        }
        assert!(seen.iter().all(|value| *value));
        // 前四个是标准之字形前缀
        assert_eq!(&ZIGZAG[0..4], &[0, 1, 8, 16]);
    }
}

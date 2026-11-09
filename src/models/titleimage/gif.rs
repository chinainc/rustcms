//! GIF 解码（只解首帧）。
//!
//! 覆盖：GIF87a / GIF89a、全局/局部调色板、交错帧、透明色索引。
//! 动图只取第一帧（缩略图与水印场景不需要动画），其余帧直接跳过。
//! 输出统一为 RGBA8：帧外的画布区域保持完全透明。

/// 单张图片的像素上限，避免解码超大文件耗尽内存。
const MAX_PIXELS: u64 = 40_000_000;

/// GIF 头部信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    /// 是否是动图（存在多个图像块）。
    pub animated: bool,
}

/// 是否是 GIF 数据。
pub fn is_gif(bytes: &[u8]) -> bool {
    bytes.len() >= 6 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"))
}

fn u16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

/// 解析头部信息（画布尺寸 + 是否动图）。
pub fn info(bytes: &[u8]) -> Result<Info, String> {
    if !is_gif(bytes) {
        return Err("不是有效的 GIF 数据".to_string());
    }
    if bytes.len() < 13 {
        return Err("GIF 数据不完整".to_string());
    }
    let width = u16_le(bytes, 6) as u32;
    let height = u16_le(bytes, 8) as u32;
    if width == 0 || height == 0 {
        return Err("GIF 尺寸无效".to_string());
    }
    // 统计图像块数量判断是否动图
    let mut frames = 0usize;
    let mut pos = 13usize;
    if bytes[10] & 0x80 != 0 {
        pos += 3 * (1usize << ((bytes[10] & 0x07) + 1));
    }
    while pos < bytes.len() {
        match bytes[pos] {
            0x3B => break,
            0x21 => {
                pos += 2;
                pos = skip_sub_blocks(bytes, pos);
            }
            0x2C => {
                frames += 1;
                if pos + 10 > bytes.len() {
                    break;
                }
                let packed = bytes[pos + 9];
                let mut next = pos + 10;
                if packed & 0x80 != 0 {
                    next += 3 * (1usize << ((packed & 0x07) + 1));
                }
                // 跳过 LZW 最小码长字节 + 数据子块
                next += 1;
                pos = skip_sub_blocks(bytes, next);
            }
            _ => break,
        }
    }
    Ok(Info {
        width,
        height,
        animated: frames > 1,
    })
}

/// 跳过数据子块序列，返回下一个块的位置。
fn skip_sub_blocks(bytes: &[u8], mut pos: usize) -> usize {
    while pos < bytes.len() {
        let size = bytes[pos] as usize;
        pos += 1;
        if size == 0 {
            break;
        }
        pos = (pos + size).min(bytes.len());
    }
    pos
}

/// 读取数据子块序列的全部内容。
fn read_sub_blocks(bytes: &[u8], mut pos: usize) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    while pos < bytes.len() {
        let size = bytes[pos] as usize;
        pos += 1;
        if size == 0 {
            break;
        }
        let end = (pos + size).min(bytes.len());
        out.extend_from_slice(&bytes[pos..end]);
        pos = end;
    }
    (out, pos)
}

fn read_palette(bytes: &[u8], offset: usize, entries: usize) -> Vec<[u8; 3]> {
    let mut palette = Vec::with_capacity(entries);
    for index in 0..entries {
        let base = offset + index * 3;
        if base + 3 > bytes.len() {
            break;
        }
        palette.push([bytes[base], bytes[base + 1], bytes[base + 2]]);
    }
    palette
}

/// 解码成 RGBA8：返回 `(宽, 高, 像素)`。
pub fn decode(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if !is_gif(bytes) {
        return Err("不是有效的 GIF 数据".to_string());
    }
    if bytes.len() < 13 {
        return Err("GIF 数据不完整".to_string());
    }
    let canvas_width = u16_le(bytes, 6) as usize;
    let canvas_height = u16_le(bytes, 8) as usize;
    if canvas_width == 0 || canvas_height == 0 {
        return Err("GIF 尺寸无效".to_string());
    }
    if canvas_width as u64 * canvas_height as u64 > MAX_PIXELS {
        return Err("图片尺寸过大".to_string());
    }

    let flags = bytes[10];
    let mut pos = 13usize;
    let global_palette = if flags & 0x80 != 0 {
        let entries = 1usize << ((flags & 0x07) + 1);
        let palette = read_palette(bytes, pos, entries);
        pos += 3 * entries;
        palette
    } else {
        Vec::new()
    };

    let mut transparent: Option<usize> = None;
    let mut pixels = vec![0u8; canvas_width * canvas_height * 4];

    while pos < bytes.len() {
        match bytes[pos] {
            0x3B => break,
            0x21 => {
                // 扩展块：0xF9 是图形控制（含透明色），其余忽略
                let label = *bytes.get(pos + 1).unwrap_or(&0);
                pos += 2;
                if label == 0xF9 && pos + 5 <= bytes.len() {
                    // 子块：块长、控制位、延时（2 字节）、透明色索引
                    let packed = bytes[pos + 1];
                    let transparent_index = bytes[pos + 4];
                    transparent = if packed & 0x01 != 0 {
                        Some(transparent_index as usize)
                    } else {
                        None
                    };
                }
                pos = skip_sub_blocks(bytes, pos);
            }
            0x2C => {
                if pos + 10 > bytes.len() {
                    return Err("GIF 图像块数据不完整".to_string());
                }
                let left = u16_le(bytes, pos + 1) as usize;
                let top = u16_le(bytes, pos + 3) as usize;
                let frame_width = u16_le(bytes, pos + 5) as usize;
                let frame_height = u16_le(bytes, pos + 7) as usize;
                let packed = bytes[pos + 9];
                let interlaced = packed & 0x40 != 0;
                let mut next = pos + 10;
                let palette = if packed & 0x80 != 0 {
                    let entries = 1usize << ((packed & 0x07) + 1);
                    let local = read_palette(bytes, next, entries);
                    next += 3 * entries;
                    local
                } else {
                    global_palette.clone()
                };
                if next >= bytes.len() {
                    return Err("GIF 图像块数据不完整".to_string());
                }
                let min_code_size = bytes[next];
                let (data, _) = read_sub_blocks(bytes, next + 1);

                if frame_width == 0 || frame_height == 0 {
                    return Err("GIF 帧尺寸无效".to_string());
                }
                if frame_width as u64 * frame_height as u64 > MAX_PIXELS {
                    return Err("图片尺寸过大".to_string());
                }
                if palette.is_empty() {
                    return Err("GIF 缺少调色板".to_string());
                }

                let indices = lzw_decode(min_code_size, &data, frame_width * frame_height)?;
                let rows = if interlaced {
                    interlaced_rows(frame_height)
                } else {
                    (0..frame_height).collect()
                };
                // 只画首帧
                for (row_index, target_row) in rows.iter().enumerate() {
                    for column in 0..frame_width {
                        let source = row_index * frame_width + column;
                        let Some(index) = indices.get(source).copied() else {
                            continue;
                        };
                        if Some(index as usize) == transparent {
                            continue;
                        }
                        let color = palette.get(index as usize).copied().unwrap_or([0, 0, 0]);
                        let x = left + column;
                        let y = top + target_row;
                        if x >= canvas_width || y >= canvas_height {
                            continue;
                        }
                        let offset = (y * canvas_width + x) * 4;
                        pixels[offset] = color[0];
                        pixels[offset + 1] = color[1];
                        pixels[offset + 2] = color[2];
                        pixels[offset + 3] = 255;
                    }
                }
                return Ok((canvas_width as u32, canvas_height as u32, pixels));
            }
            _ => return Err("GIF 数据块类型无法识别".to_string()),
        }
    }
    Err("GIF 缺少图像数据".to_string())
}

/// 交错 GIF 的行显示顺序。
fn interlaced_rows(height: usize) -> Vec<usize> {
    let mut rows = Vec::with_capacity(height);
    for (start, step) in [(0usize, 8usize), (4, 8), (2, 4), (1, 2)] {
        let mut row = start;
        while row < height {
            rows.push(row);
            row += step;
        }
    }
    rows
}

/// GIF 变体的 LZW 解码（码长 2..=11，码流按字节低位优先）。
fn lzw_decode(min_code_size: u8, data: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    let min_code_size = min_code_size as usize;
    if !(2..=11).contains(&min_code_size) {
        return Err("GIF LZW 最小码长非法".to_string());
    }
    let clear = 1usize << min_code_size;
    let end = clear + 1;
    let mut dictionary: Vec<Vec<u8>> = Vec::with_capacity(4096);
    let reset = |dictionary: &mut Vec<Vec<u8>>| {
        dictionary.clear();
        for value in 0..clear {
            dictionary.push(vec![value as u8]);
        }
        // 占位：clear 与 end 两个保留码
        dictionary.push(Vec::new());
        dictionary.push(Vec::new());
    };
    reset(&mut dictionary);

    let mut code_size = min_code_size + 1;
    let mut reader = LsbReader::new(data);
    let mut out: Vec<u8> = Vec::with_capacity(expected);
    let mut previous: Option<Vec<u8>> = None;

    loop {
        if out.len() >= expected {
            break;
        }
        let Some(code) = reader.read(code_size) else {
            break;
        };
        if code == clear {
            reset(&mut dictionary);
            code_size = min_code_size + 1;
            previous = None;
            continue;
        }
        if code == end {
            break;
        }
        let entry = if code < dictionary.len() {
            dictionary[code].clone()
        } else if code == dictionary.len() {
            // KwKwK 特例：编码端刚建立的条目
            let Some(prev) = previous.clone() else {
                return Err("GIF LZW 码流无法解析".to_string());
            };
            let mut entry = prev;
            let first = entry.first().copied().unwrap_or(0);
            entry.push(first);
            entry
        } else {
            return Err("GIF LZW 引用了不存在的字典项".to_string());
        };

        out.extend_from_slice(&entry);
        if let Some(prev) = previous {
            let mut next = prev;
            if let Some(&first) = entry.first() {
                next.push(first);
            }
            if dictionary.len() < 4096 {
                dictionary.push(next);
            }
        }
        previous = Some(entry);
        if dictionary.len() == (1usize << code_size) && code_size < 12 {
            code_size += 1;
        }
    }

    if out.len() < expected {
        return Err("GIF 像素数据不完整".to_string());
    }
    out.truncate(expected);
    Ok(out)
}

/// 按字节低位优先读取变长码。
struct LsbReader<'a> {
    data: &'a [u8],
    position: usize,
    buffer: u32,
    bits: u8,
}

impl<'a> LsbReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        LsbReader {
            data,
            position: 0,
            buffer: 0,
            bits: 0,
        }
    }

    fn read(&mut self, count: usize) -> Option<usize> {
        while (self.bits as usize) < count {
            let byte = *self.data.get(self.position)?;
            self.position += 1;
            self.buffer |= (byte as u32) << self.bits;
            self.bits += 8;
        }
        let mask = if count >= 32 {
            u32::MAX
        } else {
            (1u32 << count) - 1
        };
        let value = self.buffer & mask;
        self.buffer >>= count;
        self.bits -= count as u8;
        Some(value as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拼一个 2×2、4 色调色板的最小 GIF。
    fn minimal_gif(with_transparency: bool) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"GIF89a");
        // 逻辑屏幕描述符：2×2，全局调色板 4 项
        out.extend_from_slice(&[0x02, 0x00, 0x02, 0x00, 0x81, 0x00, 0x00]);
        // 全局调色板
        out.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0]);
        if with_transparency {
            // 图形控制扩展：透明色索引 3
            out.extend_from_slice(&[0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x03, 0x00]);
        }
        // 图像描述符
        out.extend_from_slice(&[0x2C, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x02, 0x00, 0x00]);
        // LZW 最小码长 2，码流：CLEAR(4)、0、1、2、3、END(5)
        out.extend_from_slice(&[0x02, 0x03, 0x44, 0x34, 0x05, 0x00]);
        out.extend_from_slice(&[0x3B]);
        out
    }

    #[test]
    fn decodes_first_frame() {
        let bytes = minimal_gif(false);
        assert!(is_gif(&bytes));
        let header = info(&bytes).unwrap();
        assert_eq!((header.width, header.height), (2, 2));
        assert!(!header.animated);

        let (width, height, pixels) = decode(&bytes).unwrap();
        assert_eq!((width, height), (2, 2));
        assert_eq!(pixels.len(), 2 * 2 * 4);
        assert_eq!(&pixels[0..4], &[255, 0, 0, 255]);
        assert_eq!(&pixels[4..8], &[0, 255, 0, 255]);
        assert_eq!(&pixels[8..12], &[0, 0, 255, 255]);
        assert_eq!(&pixels[12..16], &[255, 255, 0, 255]);
    }

    #[test]
    fn honours_transparency() {
        let bytes = minimal_gif(true);
        let (_, _, pixels) = decode(&bytes).unwrap();
        // 索引 3 是透明色 → 最后一个像素 alpha 为 0
        assert_eq!(pixels[15], 0, "透明像素 alpha 应为 0");
        assert_eq!(&pixels[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn rejects_broken_input() {
        assert!(!is_gif(b"GIF8"));
        assert!(info(b"GIF89a").is_err());
        // 头部合法但没有任何图像块
        let mut bytes = minimal_gif(false);
        bytes.truncate(13 + 12);
        assert!(decode(&bytes).is_err());
        // 非 GIF
        assert!(decode(b"not-a-gif").is_err());
    }

    #[test]
    fn interlaced_rows_cover_every_row() {
        let rows = interlaced_rows(7);
        assert_eq!(rows, vec![0, 4, 2, 6, 1, 3, 5]);
        let mut sorted = rows.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..7).collect::<Vec<_>>());
    }

    #[test]
    fn lsb_reader_reads_low_bits_first() {
        // 0b1010_1100 = 0xAC：低 3 位是 0b100 = 4
        let mut reader = LsbReader::new(&[0xAC]);
        assert_eq!(reader.read(3), Some(4));
        assert_eq!(reader.read(5), Some(0b10101));
        assert_eq!(reader.read(1), None);
    }
}

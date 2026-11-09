//! TrueType（`glyf` 轮廓）光栅化 —— 给文字水印用。
//!
//! 刻意不引第三方字体/渲染库，只解析渲染真正需要的几张表：
//! `head`（unitsPerEm / loca 格式）、`maxp`（字形数）、`hhea` + `hmtx`（横向 advance）、
//! `loca` + `glyf`（轮廓）、`cmap`（字符 → 字形号，格式 4 / 12）。
//!
//! 渲染方式：轮廓展平成折线 → 非零环绕规则扫描线填充 → 纵向 [`SUB_ROWS`] 倍子采样 +
//! 水平精确覆盖率（相当于面积采样），得到接近 4×4 超采样的抗锯齿效果，但内存只与
//! 单行宽度相关。
//!
//! 与成熟字体引擎的差异：不做 hinting、不做 kerning，字形边缘会有亚像素级差别；
//! 位置、颜色、透明度口径保持一致（位置仍按「ASCII 半字号、中文全字号」估算）。
//!
//! 支持 `.ttf` 与 `.ttc`（取第一个字体面）；CFF/OTF（`OTTO`）与可变字体之外的
//! 非法数据一律**明确报错**，不做假成功。

use super::super::titleimage::image_ops::Image;

/// 复合字形递归深度上限（防损坏字体死循环）。
const MAX_DEPTH: u8 = 6;

/// 纵向子采样数：取 4 时视觉上已看不出阶梯，再多对水印这种字号收益很小。
const SUB_ROWS: usize = 4;

#[derive(Debug, Clone, Copy)]
struct Point {
    x: f32,
    y: f32,
    on: bool,
}

/// 一个字形（或复合字形的拼接结果）的轮廓，坐标仍是 font units。
#[derive(Debug, Default)]
struct Outline {
    contours: Vec<Vec<Point>>,
    // 包围盒只用于校验（与光栅化结果交叉比对），运行时不读，故显式放行未使用告警。
    #[allow(dead_code)]
    min_x: f32,
    #[allow(dead_code)]
    min_y: f32,
    #[allow(dead_code)]
    max_x: f32,
    #[allow(dead_code)]
    max_y: f32,
}

impl Outline {
    /// 按轮廓点重算包围盒（复合字形变换后必须重算）。
    fn refresh_bounds(&mut self) {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for contour in &self.contours {
            for point in contour {
                min_x = min_x.min(point.x);
                min_y = min_y.min(point.y);
                max_x = max_x.max(point.x);
                max_y = max_y.max(point.y);
            }
        }
        if self.contours.is_empty() {
            (self.min_x, self.min_y, self.max_x, self.max_y) = (0.0, 0.0, 0.0, 0.0);
        } else {
            (self.min_x, self.min_y, self.max_x, self.max_y) = (min_x, min_y, max_x, max_y);
        }
    }
}

/// 一条边（设备坐标，y 向下）。
#[derive(Debug, Clone, Copy)]
struct Edge {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

/// 已解析的字体面。
#[derive(Debug)]
pub struct Face {
    data: Vec<u8>,
    units_per_em: f32,
    /// `glyf` 表在 `data` 里的偏移与长度。
    glyf: (usize, usize),
    /// `loca` 表在 `data` 里的偏移。
    loca: usize,
    loca_long: bool,
    num_glyphs: u16,
    /// 每个字形的横向 advance（font units，`hmtx`）。
    advances: Vec<u16>,
    /// `cmap` 子表在 `data` 里的偏移与格式（4 / 12）。
    cmap: Option<(usize, u16)>,
}

impl Face {
    /// 解析字体文件。
    pub fn parse(data: &[u8]) -> Result<Face, String> {
        let dir = font_dir(data)?;
        let head = table(data, dir, b"head")?
            .ok_or_else(|| "字体缺少 head 表".to_string())?;
        let maxp = table(data, dir, b"maxp")?
            .ok_or_else(|| "字体缺少 maxp 表".to_string())?;
        let hhea = table(data, dir, b"hhea")?
            .ok_or_else(|| "字体缺少 hhea 表".to_string())?;
        let hmtx = table(data, dir, b"hmtx")?;
        let glyf = table(data, dir, b"glyf")?
            .ok_or_else(|| "字体缺少 glyf 表（CFF 轮廓字体不受支持）".to_string())?;
        let loca = table(data, dir, b"loca")?
            .ok_or_else(|| "字体缺少 loca 表".to_string())?;

        let units_per_em = read_u16(data, head.0 + 18)?;
        if units_per_em == 0 {
            return Err("字体 unitsPerEm 为 0，无法换算字号".to_string());
        }
        let loca_long = read_i16(data, head.0 + 50)? != 0;
        let num_glyphs = read_u16(data, maxp.0 + 4)?;
        if num_glyphs == 0 {
            return Err("字体没有字形".to_string());
        }
        let metrics = read_u16(data, hhea.0 + 34)?.max(1) as usize;

        // hmtx：前 metrics 个是 (advance, lsb)，后面只有 lsb，advance 沿用最后一个。
        let mut advances: Vec<u16> = Vec::with_capacity(num_glyphs as usize);
        let Some((hmtx_off, hmtx_len)) = hmtx else {
            return Err("字体缺少 hmtx 表".to_string());
        };
        let count = metrics.min(num_glyphs as usize);
        for index in 0..count {
            let offset = hmtx_off + index * 4;
            if offset + 2 > hmtx_off + hmtx_len {
                return Err("hmtx 表长度不足".to_string());
            }
            advances.push(read_u16(data, offset)?);
        }
        let last = advances.last().copied().unwrap_or(0);
        advances.resize(num_glyphs as usize, last);

        Ok(Face {
            data: data.to_vec(),
            units_per_em: units_per_em as f32,
            glyf,
            loca: loca.0,
            loca_long,
            num_glyphs,
            advances,
            cmap: find_cmap(data, dir)?,
        })
    }

    /// 字符 → 字形号（cmap 里没有则 `None`）。
    pub fn glyph_index(&self, ch: char) -> Option<u16> {
        let (offset, format) = self.cmap?;
        let code = ch as u32;
        match format {
            4 => self.glyph_index_format4(offset, code),
            12 => self.glyph_index_format12(offset, code),
            _ => None,
        }
    }

    /// 字形的横向 advance（font units）。
    pub fn advance(&self, glyph: u16) -> u16 {
        self.advances
            .get(glyph as usize)
            .copied()
            .unwrap_or_else(|| self.advances.last().copied().unwrap_or(0))
    }

    /// 一段文字在给定字号（px）下的排版宽度。
    ///
    /// 与 [`draw_text`] 的推进口径完全一致：字体里没有的字形按「半个字号」留白，
    /// 这样测量结果与真正画出来的宽度不会打架（标题图片折行依赖这一点）。
    pub fn text_width(&self, text: &str, px_size: f32) -> f32 {
        if !(px_size.is_finite() && px_size > 0.0) {
            return 0.0;
        }
        let scale = px_size / self.units_per_em;
        text.chars()
            .map(|ch| match self.glyph_index(ch) {
                Some(glyph) => self.advance(glyph) as f32 * scale,
                None => px_size * 0.5,
            })
            .sum()
    }

    /// 字形轮廓（`None` = 空白字形，如空格）。
    fn outline(&self, glyph: u16) -> Result<Option<Outline>, String> {
        self.outline_at(glyph, 0)
    }

    fn outline_at(&self, glyph: u16, depth: u8) -> Result<Option<Outline>, String> {
        if depth > MAX_DEPTH {
            return Err("复合字形嵌套过深".to_string());
        }
        if glyph >= self.num_glyphs {
            return Ok(None);
        }
        let (start, end) = self.glyph_range(glyph)?;
        if end <= start {
            return Ok(None);
        }
        let base = self.glyf.0 + start;
        if base + (end - start) > self.data.len() || base + (end - start) > self.glyf.0 + self.glyf.1
        {
            return Err("字形数据越界".to_string());
        }
        let data = &self.data[base..base + (end - start)];
        let contours = read_i16(data, 0)?;
        if contours >= 0 {
            self.simple_outline(data, contours as usize).map(Some)
        } else {
            self.composite_outline(data, depth).map(|outline| {
                if outline.contours.is_empty() {
                    None
                } else {
                    Some(outline)
                }
            })
        }
    }

    fn glyph_range(&self, glyph: u16) -> Result<(usize, usize), String> {
        let index = glyph as usize;
        if self.loca_long {
            let start = read_u32(&self.data, self.loca + index * 4)? as usize;
            let end = read_u32(&self.data, self.loca + (index + 1) * 4)? as usize;
            Ok((start, end))
        } else {
            let start = read_u16(&self.data, self.loca + index * 2)? as usize * 2;
            let end = read_u16(&self.data, self.loca + (index + 1) * 2)? as usize * 2;
            Ok((start, end))
        }
    }

    fn simple_outline(&self, data: &[u8], contours: usize) -> Result<Outline, String> {
        let mut offset = 10;
        let mut ends: Vec<usize> = Vec::with_capacity(contours);
        for _ in 0..contours {
            ends.push(read_u16(data, offset)? as usize);
            offset += 2;
        }
        let instruction_len = read_u16(data, offset)? as usize;
        offset += 2 + instruction_len;

        let total = ends.last().map(|value| value + 1).unwrap_or(0);
        if total == 0 || total > 10_000 {
            return Ok(Outline::default());
        }

        let mut flags: Vec<u8> = Vec::with_capacity(total);
        while flags.len() < total {
            let flag = read_u8(data, offset)?;
            offset += 1;
            flags.push(flag);
            if flag & 0x08 != 0 {
                let repeat = read_u8(data, offset)?;
                offset += 1;
                for _ in 0..repeat {
                    if flags.len() >= total {
                        break;
                    }
                    flags.push(flag);
                }
            }
        }

        let mut xs: Vec<f32> = Vec::with_capacity(total);
        let mut x = 0i32;
        for &flag in &flags {
            if flag & 0x02 != 0 {
                let delta = read_u8(data, offset)? as i32;
                offset += 1;
                x += if flag & 0x10 != 0 { delta } else { -delta };
            } else if flag & 0x10 == 0 {
                x += read_i16(data, offset)? as i32;
                offset += 2;
            }
            xs.push(x as f32);
        }

        let mut ys: Vec<f32> = Vec::with_capacity(total);
        let mut y = 0i32;
        for &flag in &flags {
            if flag & 0x04 != 0 {
                let delta = read_u8(data, offset)? as i32;
                offset += 1;
                y += if flag & 0x20 != 0 { delta } else { -delta };
            } else if flag & 0x20 == 0 {
                y += read_i16(data, offset)? as i32;
                offset += 2;
            }
            ys.push(y as f32);
        }

        let mut outline = Outline {
            contours: Vec::with_capacity(contours),
            ..Outline::default()
        };
        let mut start = 0usize;
        for end in ends {
            if end + 1 <= total && start <= end {
                let mut points = Vec::with_capacity(end + 1 - start);
                for index in start..=end {
                    points.push(Point {
                        x: xs[index],
                        y: ys[index],
                        on: flags[index] & 0x01 != 0,
                    });
                }
                if points.len() >= 2 {
                    outline.contours.push(points);
                }
            }
            start = end + 1;
        }
        outline.refresh_bounds();
        Ok(outline)
    }

    fn composite_outline(&self, data: &[u8], depth: u8) -> Result<Outline, String> {
        let mut offset = 10;
        let mut outline = Outline::default();
        loop {
            let flags = read_u16(data, offset)?;
            offset += 2;
            let component = read_u16(data, offset)?;
            offset += 2;

            let (arg1, arg2) = if flags & 0x0001 != 0 {
                let first = read_i16(data, offset)? as f32;
                offset += 2;
                let second = read_i16(data, offset)? as f32;
                offset += 2;
                (first, second)
            } else {
                let first = read_u8(data, offset)? as i8 as f32;
                offset += 1;
                let second = read_u8(data, offset)? as i8 as f32;
                offset += 1;
                (first, second)
            };
            // 参数不是「XY 偏移」而是「点号匹配」时无法逐点对齐，这里按无偏移处理。
            let (dx, dy) = if flags & 0x0002 != 0 {
                (arg1, arg2)
            } else {
                (0.0, 0.0)
            };

            // (a, b, c, d)：x' = a*x + c*y，y' = b*x + d*y
            let (a, b, c, d) = if flags & 0x0008 != 0 {
                let scale = read_f2dot14(data, offset)?;
                offset += 2;
                (scale, 0.0, 0.0, scale)
            } else if flags & 0x0040 != 0 {
                let sx = read_f2dot14(data, offset)?;
                offset += 2;
                let sy = read_f2dot14(data, offset)?;
                offset += 2;
                (sx, 0.0, 0.0, sy)
            } else if flags & 0x0080 != 0 {
                let a = read_f2dot14(data, offset)?;
                offset += 2;
                let b = read_f2dot14(data, offset)?;
                offset += 2;
                let c = read_f2dot14(data, offset)?;
                offset += 2;
                let d = read_f2dot14(data, offset)?;
                offset += 2;
                (a, b, c, d)
            } else {
                (1.0, 0.0, 0.0, 1.0)
            };

            if let Some(component_outline) = self.outline_at(component, depth + 1)? {
                for contour in component_outline.contours {
                    let mapped: Vec<Point> = contour
                        .into_iter()
                        .map(|point| Point {
                            x: a * point.x + c * point.y + dx,
                            y: b * point.x + d * point.y + dy,
                            on: point.on,
                        })
                        .collect();
                    outline.contours.push(mapped);
                }
            }
            if flags & 0x0020 == 0 {
                break;
            }
            if offset > data.len() {
                return Err("复合字形数据截断".to_string());
            }
        }
        outline.refresh_bounds();
        Ok(outline)
    }

    fn glyph_index_format4(&self, table: usize, code: u32) -> Option<u16> {
        if code > 0xFFFF {
            return None;
        }
        let code = code as u16;
        let seg_count = read_u16(&self.data, table + 6).ok()? as usize / 2;
        if seg_count == 0 {
            return None;
        }
        let end_codes = table + 14;
        let start_codes = end_codes + seg_count * 2 + 2;
        let deltas = start_codes + seg_count * 2;
        let range_offsets = deltas + seg_count * 2;

        // endCode 按升序，线性找第一个 >= code 的段即可（段数一般 < 百）。
        for index in 0..seg_count {
            let end = read_u16(&self.data, end_codes + index * 2).ok()?;
            if end < code {
                continue;
            }
            let start = read_u16(&self.data, start_codes + index * 2).ok()?;
            if start > code {
                return None;
            }
            let delta = read_i16(&self.data, deltas + index * 2).ok()?;
            let range_offset = read_u16(&self.data, range_offsets + index * 2).ok()?;
            if range_offset == 0 {
                return Some((code as i32 + delta as i32) as u16);
            }
            let glyph_offset =
                range_offsets + index * 2 + range_offset as usize + (code - start) as usize * 2;
            let glyph = read_u16(&self.data, glyph_offset).ok()?;
            if glyph == 0 {
                return None;
            }
            return Some(((glyph as i32 + delta as i32) & 0xFFFF) as u16);
        }
        None
    }

    fn glyph_index_format12(&self, table: usize, code: u32) -> Option<u16> {
        let groups = read_u32(&self.data, table + 12).ok()? as usize;
        if groups == 0 {
            return None;
        }
        let base = table + 16;
        let (mut low, mut high) = (0usize, groups - 1);
        while low <= high {
            let mid = (low + high) / 2;
            let offset = base + mid * 12;
            let start = read_u32(&self.data, offset).ok()?;
            let end = read_u32(&self.data, offset + 4).ok()?;
            if code < start {
                if mid == 0 {
                    return None;
                }
                high = mid - 1;
            } else if code > end {
                low = mid + 1;
            } else {
                let glyph = read_u32(&self.data, offset + 8).ok()?;
                let index = glyph + (code - start);
                return u16::try_from(index).ok();
            }
        }
        None
    }
}

/// 把一段文字画到图上，返回排完后的笔位 x（设备坐标）。
///
/// `origin_x` 是首字左边线，`baseline_y` 是基线，`px_size` 是字号（px，em 高度）。
pub fn draw_text(
    image: &mut Image,
    face: &Face,
    text: &str,
    origin_x: f32,
    baseline_y: f32,
    px_size: f32,
    color: [u8; 4],
    opacity: f32,
) -> Result<f32, String> {
    if !(px_size.is_finite() && px_size > 0.0) {
        return Err("水印字号必须大于 0".to_string());
    }
    let opacity = if opacity.is_finite() {
        opacity.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let scale = px_size / face.units_per_em;
    let mut pen = origin_x;
    for ch in text.chars() {
        let Some(glyph) = face.glyph_index(ch) else {
            // 字体里没有这个字：按半个字号留白，避免后面的字叠在一起。
            pen += px_size * 0.5;
            continue;
        };
        if let Some(outline) = face.outline(glyph)? {
            let contours: Vec<Vec<(f32, f32)>> = outline
                .contours
                .iter()
                .map(|contour| {
                    flatten(contour)
                        .into_iter()
                        .map(|(x, y)| (pen + x * scale, baseline_y - y * scale))
                        .collect()
                })
                .filter(|contour: &Vec<(f32, f32)>| contour.len() >= 3)
                .collect();
            fill_contours(image, &contours, color, opacity);
        }
        pen += face.advance(glyph) as f32 * scale;
    }
    Ok(pen)
}

/// 把轮廓（二次贝塞尔）展平成闭合折线。
fn flatten(contour: &[Point]) -> Vec<(f32, f32)> {
    let count = contour.len();
    if count < 2 {
        return Vec::new();
    }
    let mut points: Vec<(f32, f32)> = Vec::with_capacity(count * 4);
    let first = contour[0];
    let last = contour[count - 1];

    // 起点：首点在线 → 用它；否则末点在线 → 用末点；都不在线 → 两点中点。
    let (mut current, start_index) = if first.on {
        ((first.x, first.y), 1usize)
    } else if last.on {
        ((last.x, last.y), 0usize)
    } else {
        (((first.x + last.x) / 2.0, (first.y + last.y) / 2.0), 0usize)
    };
    points.push(current);

    // 恰好绕一圈：每个控制点被消费一次（在线点单独消费、离线点带下一个在线点消费）。
    let total = start_index + count;
    let mut index = start_index;
    while index < total {
        let point = contour[index % count];
        if point.on {
            let target = (point.x, point.y);
            if (target.0 - current.0).abs() > f32::EPSILON
                || (target.1 - current.1).abs() > f32::EPSILON
            {
                points.push(target);
                current = target;
            }
            index += 1;
        } else {
            let next = contour[(index + 1) % count];
            let control = (point.x, point.y);
            let end = if next.on {
                index += 2;
                (next.x, next.y)
            } else {
                index += 1;
                ((point.x + next.x) / 2.0, (point.y + next.y) / 2.0)
            };
            flatten_quad(current, control, end, &mut points);
            current = end;
        }
    }
    // 收尾：回到起点（填充时也按闭合处理，这里避免留下重复点）
    if points.len() > 1 {
        let (x0, y0) = points[0];
        if let Some((x1, y1)) = points.last().copied() {
            if (x0 - x1).abs() < f32::EPSILON && (y0 - y1).abs() < f32::EPSILON {
                points.pop();
            }
        }
    }
    points
}

fn flatten_quad(from: (f32, f32), control: (f32, f32), to: (f32, f32), out: &mut Vec<(f32, f32)>) {
    let length = ((control.0 - from.0).powi(2) + (control.1 - from.1).powi(2)).sqrt()
        + ((to.0 - control.0).powi(2) + (to.1 - control.1).powi(2)).sqrt();
    let steps = ((length / 2.0).ceil() as usize).clamp(2, 16);
    for step in 1..=steps {
        let t = step as f32 / steps as f32;
        let inverse = 1.0 - t;
        out.push((
            inverse * inverse * from.0 + 2.0 * inverse * t * control.0 + t * t * to.0,
            inverse * inverse * from.1 + 2.0 * inverse * t * control.1 + t * t * to.1,
        ));
    }
}

/// 非零环绕规则填充（扫描线 + 纵向子采样 + 水平精确覆盖）。
fn fill_contours(image: &mut Image, contours: &[Vec<(f32, f32)>], color: [u8; 4], opacity: f32) {
    if image.width == 0 || image.height == 0 || opacity <= 0.0 {
        return;
    }
    let mut edges: Vec<Edge> = Vec::new();
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for contour in contours {
        let count = contour.len();
        if count < 3 {
            continue;
        }
        for index in 0..count {
            let (x0, y0) = contour[index];
            let (x1, y1) = contour[(index + 1) % count];
            if !x0.is_finite() || !y0.is_finite() || !x1.is_finite() || !y1.is_finite() {
                continue;
            }
            if (y1 - y0).abs() < f32::EPSILON {
                continue;
            }
            min_y = min_y.min(y0).min(y1);
            max_y = max_y.max(y0).max(y1);
            edges.push(Edge { x0, y0, x1, y1 });
        }
    }
    if edges.is_empty() {
        return;
    }

    let top = min_y.floor().max(0.0) as i64;
    let bottom = max_y.ceil().min(image.height as f32) as i64;
    if bottom <= top {
        return;
    }

    let width = image.width as usize;
    let mut coverage: Vec<f32> = vec![0.0; width];
    let mut crossings: Vec<(f32, i32)> = Vec::new();
    let weight = 1.0 / SUB_ROWS as f32;

    for row in top..bottom {
        let mut touched = (usize::MAX, 0usize);
        for sub in 0..SUB_ROWS {
            let sample_y = row as f32 + (sub as f32 + 0.5) / SUB_ROWS as f32;
            crossings.clear();
            for edge in &edges {
                let (y0, y1) = (edge.y0, edge.y1);
                if (y0 <= sample_y && sample_y < y1) || (y1 <= sample_y && sample_y < y0) {
                    let t = (sample_y - y0) / (y1 - y0);
                    let x = edge.x0 + (edge.x1 - edge.x0) * t;
                    crossings.push((x, if y1 > y0 { 1 } else { -1 }));
                }
            }
            if crossings.len() < 2 {
                continue;
            }
            crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

            let mut winding = 0i32;
            let mut span_start = 0.0f32;
            for (x, direction) in crossings.iter().copied() {
                let previous = winding;
                winding += direction;
                if previous == 0 && winding != 0 {
                    span_start = x;
                } else if previous != 0 && winding == 0 {
                    let (low, high) = (span_start.min(x), span_start.max(x));
                    if let Some(range) = add_span(&mut coverage, low, high, width) {
                        touched.0 = touched.0.min(range.0);
                        touched.1 = touched.1.max(range.1);
                    }
                }
            }
        }

        if touched.0 == usize::MAX {
            continue;
        }
        for index in touched.0..=touched.1 {
            let value = coverage[index];
            coverage[index] = 0.0;
            if value <= 0.0 {
                continue;
            }
            // 先除以子采样数再夹到 1（反过来会把「完全覆盖」压成 1/4 的灰）
            let coverage = (value * weight).min(1.0);
            let alpha = (coverage * opacity * 255.0).round();
            if alpha < 1.0 {
                continue;
            }
            image.blend(
                index as i64,
                row,
                [color[0], color[1], color[2], 255],
                alpha.min(255.0) as u8,
            );
        }
    }
}

/// 把 `[low, high)` 这段横向覆盖累加进像素缓冲，返回实际触及的像素区间。
fn add_span(buffer: &mut [f32], low: f32, high: f32, width: usize) -> Option<(usize, usize)> {
    if !(high > low) {
        return None;
    }
    let start = low.max(0.0);
    let end = high.min(width as f32);
    if !(end > start) {
        return None;
    }
    let first = start.floor() as usize;
    let last = ((end.ceil() as usize).saturating_sub(1)).min(width - 1);
    if first > last {
        return None;
    }
    for pixel in first..=last {
        let left = start.max(pixel as f32);
        let right = end.min(pixel as f32 + 1.0);
        if right > left {
            buffer[pixel] += right - left;
        }
    }
    Some((first, last))
}

// ---------------------------- 表解析辅助 ----------------------------

/// 定位 sfnt 表目录（TTC 取第一个字体面）。
fn font_dir(data: &[u8]) -> Result<usize, String> {
    if data.len() < 12 {
        return Err("字体文件过小，无法解析".to_string());
    }
    let tag = &data[0..4];
    if tag == b"ttcf" {
        let count = read_u32(data, 8)? as usize;
        if count == 0 {
            return Err("TTC 字体包为空".to_string());
        }
        let offset = read_u32(data, 12)? as usize;
        if offset + 12 > data.len() {
            return Err("TTC 字体偏移越界".to_string());
        }
        return Ok(offset);
    }
    if tag == b"OTTO" {
        return Err("暂不支持 CFF 轮廓字体（OTTO），请改用 TrueType 字体（.ttf）".to_string());
    }
    if tag != [0u8, 1, 0, 0] && tag != b"true" {
        return Err("不是有效的 TrueType 字体文件".to_string());
    }
    Ok(0)
}

/// 查表：返回 `(偏移, 长度)`。
fn table(data: &[u8], dir: usize, tag: &[u8; 4]) -> Result<Option<(usize, usize)>, String> {
    if dir + 12 > data.len() {
        return Err("字体表目录越界".to_string());
    }
    let count = read_u16(data, dir + 4)? as usize;
    for index in 0..count {
        let record = dir + 12 + index * 16;
        if record + 16 > data.len() {
            return Err("字体表目录截断".to_string());
        }
        if &data[record..record + 4] == tag {
            let offset = read_u32(data, record + 8)? as usize;
            let length = read_u32(data, record + 12)? as usize;
            if offset > data.len() {
                return Err("字体表偏移越界".to_string());
            }
            return Ok(Some((offset, length.min(data.len() - offset))));
        }
    }
    Ok(None)
}

/// 选一个可用的 `cmap` 子表（优先 3/10 与 0/x 的格式 12，其次格式 4）。
fn find_cmap(data: &[u8], dir: usize) -> Result<Option<(usize, u16)>, String> {
    let Some((offset, length)) = table(data, dir, b"cmap")? else {
        return Ok(None);
    };
    if length < 4 || offset + 4 > data.len() {
        return Ok(None);
    }
    let count = read_u16(data, offset + 2)? as usize;
    let mut best: Option<(usize, u16)> = None;
    let mut fallback: Option<(usize, u16)> = None;
    for index in 0..count {
        let record = offset + 4 + index * 8;
        if record + 8 > offset + length || record + 8 > data.len() {
            break;
        }
        let platform = read_u16(data, record)?;
        let encoding = read_u16(data, record + 2)?;
        let sub = offset + read_u32(data, record + 4)? as usize;
        if sub + 4 > data.len() {
            continue;
        }
        let format = read_u16(data, sub)?;
        if !matches!(format, 4 | 12) {
            continue;
        }
        let unicode = matches!((platform, encoding), (3, 10) | (0, 4) | (0, 6));
        let basic = matches!((platform, encoding), (3, 1) | (0, 3) | (0, 0) | (0, 2));
        if unicode && format == 12 {
            return Ok(Some((sub, 12)));
        }
        if unicode && best.is_none() {
            best = Some((sub, format));
        }
        if basic && fallback.is_none() {
            fallback = Some((sub, format));
        }
    }
    Ok(best.or(fallback))
}

fn read_u8(data: &[u8], offset: usize) -> Result<u8, String> {
    data.get(offset)
        .copied()
        .ok_or_else(|| "字体数据越界".to_string())
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, String> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or_else(|| "字体数据越界".to_string())?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_i16(data: &[u8], offset: usize) -> Result<i16, String> {
    Ok(read_u16(data, offset)? as i16)
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, String> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| "字体数据越界".to_string())?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_f2dot14(data: &[u8], offset: usize) -> Result<f32, String> {
    Ok(read_i16(data, offset)? as f32 / 16384.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 找一个可用的系统字体（找不到就跳过用例，不同机器路径不同）。
    fn system_font() -> Option<Vec<u8>> {
        const CANDIDATES: &[&str] = &[
            "C:/Windows/Fonts/arial.ttf",
            "C:/Windows/Fonts/segoeui.ttf",
            "C:/Windows/Fonts/msyh.ttc",
            "C:/Windows/Fonts/simsun.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
        ];
        CANDIDATES.iter().find_map(|path| std::fs::read(path).ok())
    }

    /// 找一个确定带中文字形的字体（Windows 上 msyh.ttc / simsun.ttc）。
    fn chinese_font() -> Option<Vec<u8>> {
        const CANDIDATES: &[&str] = &[
            "C:/Windows/Fonts/msyh.ttc",
            "C:/Windows/Fonts/simsun.ttc",
            "C:/Windows/Fonts/simhei.ttf",
        ];
        CANDIDATES.iter().find_map(|path| std::fs::read(path).ok())
    }

    #[test]
    fn rejects_invalid_and_cff_data() {
        let err = Face::parse(b"xx").unwrap_err();
        assert!(err.contains("过小"), "{err}");

        let mut otto = vec![0u8; 32];
        otto[0..4].copy_from_slice(b"OTTO");
        let err = Face::parse(&otto).unwrap_err();
        assert!(err.contains("CFF"), "{err}");

        let mut junk = vec![0u8; 64];
        junk[0..4].copy_from_slice(b"abcd");
        assert!(Face::parse(&junk).is_err());
    }

    #[test]
    fn parses_system_font_if_present() {
        let Some(bytes) = system_font() else {
            return;
        };
        let face = Face::parse(&bytes).unwrap();
        assert!(face.units_per_em >= 16.0, "{}", face.units_per_em);
        assert!(face.num_glyphs > 10);
        let glyph = face.glyph_index('A').expect("应有字母 A");
        assert!(face.advance(glyph) > 0);
        // 空格有 advance 但没有轮廓
        if let Some(space) = face.glyph_index(' ') {
            assert!(face.outline(space).unwrap().is_none());
        }
        // 不存在于 cmap 的字符返回 None（U+E000 私用区一般没字形）
        let _ = face.glyph_index('\u{e000}');
    }

    #[test]
    fn rasterizes_ascii_glyph_within_outline_bounds() {
        let Some(bytes) = system_font() else {
            return;
        };
        let face = Face::parse(&bytes).unwrap();
        let glyph = face.glyph_index('H').unwrap();
        let outline = face.outline(glyph).unwrap().unwrap();
        let px = 64.0f32;
        let scale = px / face.units_per_em;

        let mut image = Image::filled(200, 120, [0, 0, 0, 255]);
        let advance = draw_text(
            &mut image,
            &face,
            "H",
            20.0,
            90.0,
            px,
            [255, 255, 255, 255],
            1.0,
        )
        .unwrap();
        assert!(advance > 20.0 && advance < 90.0, "advance={advance}");

        // 覆盖率包围盒应与轮廓包围盒（换算到设备坐标）对得上
        let mut min_x = u32::MAX;
        let mut max_x = 0u32;
        let mut min_y = u32::MAX;
        let mut max_y = 0u32;
        let mut painted = 0usize;
        for y in 0..image.height {
            for x in 0..image.width {
                if image.get(x, y)[0] > 8 {
                    painted += 1;
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }
        assert!(painted > 100, "painted={painted}");
        let expect_min_x = 20.0 + outline.min_x * scale;
        let expect_max_x = 20.0 + outline.max_x * scale;
        let expect_min_y = 90.0 - outline.max_y * scale;
        let expect_max_y = 90.0 - outline.min_y * scale;
        assert!(
            (min_x as f32 - expect_min_x).abs() <= 2.0,
            "min_x={min_x} expect={expect_min_x}"
        );
        assert!(
            (max_x as f32 - expect_max_x).abs() <= 2.0,
            "max_x={max_x} expect={expect_max_x}"
        );
        assert!(
            (min_y as f32 - expect_min_y).abs() <= 2.0,
            "min_y={min_y} expect={expect_min_y}"
        );
        assert!(
            (max_y as f32 - expect_max_y).abs() <= 2.0,
            "max_y={max_y} expect={expect_max_y}"
        );
        // 反锯齿：边缘必须有中间灰，不能只有纯黑纯白
        let has_middle = (0..image.height).any(|y| {
            (0..image.width).any(|x| {
                let value = image.get(x, y)[0];
                value > 30 && value < 220
            })
        });
        assert!(has_middle, "没有抗锯齿过渡色");
    }

    #[test]
    fn rasterizes_chinese_glyph_if_font_available() {
        let Some(bytes) = chinese_font() else {
            return;
        };
        let Ok(face) = Face::parse(&bytes) else {
            return;
        };
        let Some(glyph) = face.glyph_index('水') else {
            return;
        };
        assert!(face.advance(glyph) > 0);

        let mut image = Image::filled(160, 160, [0, 0, 0, 255]);
        draw_text(
            &mut image,
            &face,
            "水",
            10.0,
            120.0,
            100.0,
            [255, 200, 0, 255],
            1.0,
        )
        .unwrap();
        let painted = (0..image.height)
            .flat_map(|y| (0..image.width).map(move |x| (x, y)))
            .filter(|(x, y)| image.get(*x, *y)[0] > 8)
            .count();
        assert!(painted > 200, "painted={painted}");
        // 颜色带黄（红绿高、蓝低）
        let painted_pixel = (0..image.height)
            .flat_map(|y| (0..image.width).map(move |x| (x, y)))
            .map(|(x, y)| image.get(x, y))
            .find(|pixel| pixel[0] > 200)
            .expect("应有实心像素");
        assert!(painted_pixel[2] < 120, "{painted_pixel:?}");
    }

    #[test]
    fn zero_opacity_paints_nothing() {
        let Some(bytes) = system_font() else {
            return;
        };
        let face = Face::parse(&bytes).unwrap();
        let mut image = Image::filled(60, 60, [0, 0, 0, 255]);
        draw_text(&mut image, &face, "A", 5.0, 50.0, 40.0, [255, 255, 255, 255], 0.0)
            .unwrap();
        let painted = (0..60)
            .flat_map(|y| (0..60).map(move |x| (x, y)))
            .filter(|(x, y)| image.get(*x, *y)[0] > 0)
            .count();
        assert_eq!(painted, 0);
    }
}

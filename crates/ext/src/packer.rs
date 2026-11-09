//! ZIP 打包 / 解包与 DEFLATE 编解码。
//!
//! 覆盖升级包与模板包处理（zip / tar.gz 分派、模板打包下载、
//! 模板包上传解压）。
//!
//! 实现说明：**零新依赖**。
//!
//! * ZIP 读取支持 `stored` 与 `deflate`（可读任意合规 DEFLATE 流，含外部压缩器产出的包）；
//! * DEFLATE（RFC 1951）编解码器手写实现：
//! 解码支持存储块 / 固定 Huffman / 动态 Huffman；
//! 编码为 LZ77 + 固定/动态 Huffman 择优（不可压缩时回退存储块）；
//! * ZIP 写入对文件条目使用 `deflate`，目录与压缩不划算的内容用 `stored`；
//! * `tar.gz` 读取 = gzip 头 + DEFLATE 解码 + tar 块解析；
//! * 解压带 Zip-Slip 防护（拒绝上级目录与绝对路径）。

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Context, Result};

// ---------------------------------------------------------------------------
// CRC32
// ---------------------------------------------------------------------------

/// CRC32 查表。
fn crc32_table() -> &'static [u32; 256] {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (index, slot) in table.iter_mut().enumerate() {
            let mut value = index as u32;
            for _ in 0..8 {
                value = if value & 1 != 0 {
                    0xEDB8_8320 ^ (value >> 1)
                } else {
                    value >> 1
                };
            }
            *slot = value;
        }
        table
    })
}

/// 计算 CRC32（与 ZIP 规范一致）。
pub fn crc32(data: &[u8]) -> u32 {
    let table = crc32_table();
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc = table[((crc ^ *byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

// ---------------------------------------------------------------------------
// DEFLATE 解码
// ---------------------------------------------------------------------------

/// 位读取器（LSB 优先）。
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buffer: u32,
    count: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            buffer: 0,
            count: 0,
        }
    }

    /// 确保缓冲区里至少有 `need` 位。
    fn fill(&mut self, need: u32) -> Result<()> {
        while self.count < need {
            if self.pos >= self.data.len() {
                bail!("deflate 数据意外结束");
            }
            self.buffer |= (self.data[self.pos] as u32) << self.count;
            self.pos += 1;
            self.count += 8;
        }
        Ok(())
    }

    /// 读取 `need` 位（0..=32）。
    fn bits(&mut self, need: u32) -> Result<u32> {
        if need == 0 {
            return Ok(0);
        }
        self.fill(need)?;
        let value = self.buffer & ((1u32 << need) - 1);
        self.buffer >>= need;
        self.count -= need;
        Ok(value)
    }

    /// 丢弃到字节边界。
    fn align(&mut self) {
        let drop = self.count % 8;
        self.buffer >>= drop;
        self.count -= drop;
    }
}

/// 规范 Huffman 表。
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

/// 由码长数组构造 Huffman 表。
fn build_huffman(lengths: &[u8]) -> Result<Huffman> {
    let mut counts = [0u16; 16];
    for length in lengths {
        if *length > 15 {
            bail!("非法 Huffman 码长 {length}");
        }
        counts[*length as usize] += 1;
    }
    counts[0] = 0;

    // 校验码长集合不过度订阅（允许不完全码，用于单符号表）。
    let mut left: i32 = 1;
    for &count in counts.iter().skip(1) {
        left <<= 1;
        left -= count as i32;
        if left < 0 {
            bail!("Huffman 码过度订阅");
        }
    }

    let mut offsets = [0u16; 16];
    for length in 1..15 {
        offsets[length + 1] = offsets[length] + counts[length];
    }
    let mut symbols = vec![0u16; lengths.len()];
    for (symbol, length) in lengths.iter().enumerate() {
        if *length != 0 {
            let slot = &mut offsets[*length as usize];
            symbols[*slot as usize] = symbol as u16;
            *slot += 1;
        }
    }
    Ok(Huffman { counts, symbols })
}

/// 解码一个符号。
fn decode_symbol(table: &Huffman, reader: &mut BitReader<'_>) -> Result<u16> {
    let mut code: i32 = 0;
    let mut first: i32 = 0;
    let mut index: i32 = 0;
    for length in 1..16 {
        code |= reader.bits(1)? as i32;
        let count = table.counts[length] as i32;
        if code - count < first {
            let position = (index + (code - first)) as usize;
            return table
                .symbols
                .get(position)
                .copied()
                .ok_or_else(|| anyhow!("Huffman 符号越界"));
        }
        index += count;
        first += count;
        first <<= 1;
        code <<= 1;
    }
    bail!("非法 Huffman 码")
}

/// 长度码基础值（257..=285）。
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
/// 长度码附加位数。
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// 距离码基础值。
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
/// 距离码附加位数。
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// 固定 Huffman 表（RFC 1951 第 3.2.6 节）。
fn fixed_tables() -> Result<(Huffman, Huffman)> {
    let mut literal_lengths = vec![0u8; 288];
    for (index, slot) in literal_lengths.iter_mut().enumerate() {
        *slot = if index < 144 {
            8
        } else if index < 256 {
            9
        } else if index < 280 {
            7
        } else {
            8
        };
    }
    let distance_lengths = vec![5u8; 30];
    Ok((
        build_huffman(&literal_lengths)?,
        build_huffman(&distance_lengths)?,
    ))
}

/// 解码一个压缩块（固定表或动态表）。
fn inflate_block(
    reader: &mut BitReader<'_>,
    literal: &Huffman,
    distance: &Huffman,
    output: &mut Vec<u8>,
) -> Result<()> {
    loop {
        let symbol = decode_symbol(literal, reader)?;
        if symbol < 256 {
            output.push(symbol as u8);
            continue;
        }
        if symbol == 256 {
            return Ok(());
        }
        let index = symbol as usize - 257;
        if index >= LENGTH_BASE.len() {
            bail!("非法长度码 {symbol}");
        }
        let length =
            LENGTH_BASE[index] as usize + reader.bits(LENGTH_EXTRA[index] as u32)? as usize;
        let distance_symbol = decode_symbol(distance, reader)? as usize;
        if distance_symbol >= DIST_BASE.len() {
            bail!("非法距离码 {distance_symbol}");
        }
        let dist = DIST_BASE[distance_symbol] as usize
            + reader.bits(DIST_EXTRA[distance_symbol] as u32)? as usize;
        if dist == 0 || dist > output.len() {
            bail!("距离 {dist} 超出已解压数据长度 {}", output.len());
        }
        let start = output.len() - dist;
        for offset in 0..length {
            let byte = output[start + offset];
            output.push(byte);
        }
    }
}

/// 读取动态 Huffman 表。
fn dynamic_tables(reader: &mut BitReader<'_>) -> Result<(Huffman, Huffman)> {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let literal_count = reader.bits(5)? as usize + 257;
    let distance_count = reader.bits(5)? as usize + 1;
    let code_length_count = reader.bits(4)? as usize + 4;

    let mut code_lengths = [0u8; 19];
    for slot in ORDER.iter().take(code_length_count) {
        code_lengths[*slot] = reader.bits(3)? as u8;
    }
    let code_table = build_huffman(&code_lengths)?;

    let mut lengths: Vec<u8> = Vec::with_capacity(literal_count + distance_count);
    while lengths.len() < literal_count + distance_count {
        let symbol = decode_symbol(&code_table, reader)?;
        match symbol {
            0..=15 => lengths.push(symbol as u8),
            16 => {
                let previous = *lengths
                    .last()
                    .ok_or_else(|| anyhow!("码长重复指令没有前值"))?;
                let repeat = 3 + reader.bits(2)? as usize;
                for _ in 0..repeat {
                    lengths.push(previous);
                }
            }
            17 => {
                let repeat = 3 + reader.bits(3)? as usize;
                lengths.extend(std::iter::repeat_n(0, repeat));
            }
            18 => {
                let repeat = 11 + reader.bits(7)? as usize;
                lengths.extend(std::iter::repeat_n(0, repeat));
            }
            other => bail!("非法码长符号 {other}"),
        }
    }
    if lengths.len() > literal_count + distance_count {
        bail!("码长表长度溢出");
    }
    let literal = build_huffman(&lengths[..literal_count])?;
    let distance = build_huffman(&lengths[literal_count..])?;
    Ok((literal, distance))
}

/// 解压 raw DEFLATE 数据（RFC 1951）。
pub fn inflate(data: &[u8]) -> Result<Vec<u8>> {
    let mut reader = BitReader::new(data);
    let mut output: Vec<u8> = Vec::with_capacity(data.len() * 4);
    loop {
        let last = reader.bits(1)?;
        let block_type = reader.bits(2)?;
        match block_type {
            0 => {
                // 存储块：先对齐到字节边界，再读 LEN / NLEN。
                reader.align();
                let length = reader.bits(16)? as usize;
                let inverse = reader.bits(16)? as usize;
                if length != (!inverse & 0xFFFF) {
                    bail!("存储块长度校验失败");
                }
                for _ in 0..length {
                    output.push(reader.bits(8)? as u8);
                }
            }
            1 => {
                let (literal, distance) = fixed_tables()?;
                inflate_block(&mut reader, &literal, &distance, &mut output)?;
            }
            2 => {
                let (literal, distance) = dynamic_tables(&mut reader)?;
                inflate_block(&mut reader, &literal, &distance, &mut output)?;
            }
            _ => bail!("非法 deflate 块类型 {block_type}"),
        }
        if last == 1 {
            break;
        }
    }
    Ok(output)
}

// ---------------------------------------------------------------------------
// DEFLATE 编码（LZ77 + 固定 Huffman）
// ---------------------------------------------------------------------------

/// 滑动窗口大小（DEFLATE 距离上限）。
const WINDOW_SIZE: usize = 32_768;
/// 最短匹配长度。
const MIN_MATCH: usize = 3;
/// 最长匹配长度。
const MAX_MATCH: usize = 258;
/// 3 字节哈希表位数。
const HASH_BITS: u32 = 15;
/// 哈希链最大搜索深度（越大压缩率越高、越慢）。
const MAX_CHAIN: usize = 128;
/// 存储块单块上限（DEFLATE 的 LEN 字段是 16 位）。
const STORED_BLOCK_MAX: usize = 65_535;
/// 哈希链里的空位置标记。
const NO_POSITION: u32 = u32::MAX;

/// 位写入器（普通数值 LSB 优先，Huffman 码单独按 MSB 优先写入）。
#[derive(Debug, Default)]
struct BitWriter {
    out: Vec<u8>,
    accumulator: u32,
    count: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self::default()
    }

    /// 写入 `bits` 位数值（低位先写）。`bits` 必须 ≤ 16，调用点都满足。
    fn write_bits(&mut self, value: u32, bits: u32) {
        if bits == 0 {
            return;
        }
        self.accumulator |= (value & ((1u32 << bits) - 1)) << self.count;
        self.count += bits;
        while self.count >= 8 {
            self.out.push((self.accumulator & 0xFF) as u8);
            self.accumulator >>= 8;
            self.count -= 8;
        }
    }

    /// 写入 Huffman 码：DEFLATE 要求码字按 MSB 先写，故先反转再交给位写入器。
    fn write_code(&mut self, code: u16, bits: u32) {
        let mut reversed = 0u32;
        for index in 0..bits {
            reversed = (reversed << 1) | ((code as u32 >> index) & 1);
        }
        self.write_bits(reversed, bits);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.out.push((self.accumulator & 0xFF) as u8);
        }
        self.out
    }
}

/// 固定 Huffman 表里某个符号的 `(码字, 位数)`。
fn fixed_literal_code(symbol: u16) -> (u16, u32) {
    match symbol {
        0..=143 => (0x30 + symbol, 8),
        144..=255 => (0x190 + (symbol - 144), 9),
        256..=279 => (symbol - 256, 7),
        _ => (0xC0 + (symbol - 280), 8),
    }
}

/// 匹配长度 → `(长度码符号, 附加位数, 附加值)`。
fn length_code(length: usize) -> (u16, u32, u32) {
    if length >= MAX_MATCH {
        // 258 有专用码 285（不要走 227..258 那一档）。
        return (285, 0, 0);
    }
    for index in 0..28 {
        let base = LENGTH_BASE[index] as usize;
        let extra = LENGTH_EXTRA[index] as u32;
        if length >= base && length < base + (1usize << extra) {
            return (257 + index as u16, extra, (length - base) as u32);
        }
    }
    (285, 0, 0)
}

/// 匹配距离 → `(距离码符号, 附加位数, 附加值)`。
fn distance_code(distance: usize) -> (u16, u32, u32) {
    for index in 0..30 {
        let base = DIST_BASE[index] as usize;
        let extra = DIST_EXTRA[index] as u32;
        if distance >= base && distance < base + (1usize << extra) {
            return (index as u16, extra, (distance - base) as u32);
        }
    }
    (29, 13, (distance - DIST_BASE[29] as usize) as u32)
}

/// 3 字节滚动哈希（乘性散列取高位）。
fn hash3(data: &[u8], pos: usize) -> usize {
    let value = ((data[pos] as u32) << 16) | ((data[pos + 1] as u32) << 8) | data[pos + 2] as u32;
    (value.wrapping_mul(2_654_435_761) >> (32 - HASH_BITS)) as usize
}

/// 用存储块（不压缩）编码，输出仍然是合法的 raw DEFLATE。
fn deflate_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / STORED_BLOCK_MAX * 5 + 5);
    if data.is_empty() {
        out.push(0x01); // BFINAL=1，BTYPE=00
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0xFFFFu16.to_le_bytes());
        return out;
    }
    let mut chunks = data.chunks(STORED_BLOCK_MAX).peekable();
    while let Some(chunk) = chunks.next() {
        let last = chunks.peek().is_none();
        out.push(if last { 0x01 } else { 0x00 });
        let length = chunk.len() as u16;
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out
}

/// LZ77 产出的一个符号。
#[derive(Debug, Clone, Copy)]
enum Token {
    Literal(u8),
    Match { length: usize, distance: usize },
}

/// 对 `data` 跑一遍 LZ77，把每个 token 交给 `sink`。
///
/// 两遍式实现（先统计频次、再输出）让编码阶段不需要保存整份 token 列表，
/// 内存只与 32KiB 窗口相关。
fn lz77(data: &[u8], mut sink: impl FnMut(Token)) {
    let length = data.len();
    // 哈希头 + 窗口内的前驱链（常规 LZ77 做法：内存只与窗口相关）。
    let mut head = vec![NO_POSITION; 1 << HASH_BITS];
    let mut prev = vec![NO_POSITION; WINDOW_SIZE];
    let mask = WINDOW_SIZE - 1;
    let mut pos = 0usize;

    while pos < length {
        let mut best_length = 0usize;
        let mut best_distance = 0usize;
        if pos + MIN_MATCH <= length {
            let hash = hash3(data, pos);
            let mut candidate = head[hash];
            prev[pos & mask] = candidate;
            head[hash] = pos as u32;

            let max_length = (length - pos).min(MAX_MATCH);
            let mut remaining = MAX_CHAIN;
            while candidate != NO_POSITION && remaining > 0 {
                let start = candidate as usize;
                // 前驱链按位置递减排列，越界即说明已超出窗口。
                if start >= pos || pos - start > WINDOW_SIZE {
                    break;
                }
                // 快速剔除：最佳长度末尾字节不同就不可能更优。
                if best_length == 0 || data[start + best_length] == data[pos + best_length] {
                    let mut matched = 0usize;
                    while matched < max_length && data[start + matched] == data[pos + matched] {
                        matched += 1;
                    }
                    if matched > best_length {
                        best_length = matched;
                        best_distance = pos - start;
                        if best_length == max_length {
                            break;
                        }
                    }
                }
                candidate = prev[start & mask];
                remaining -= 1;
            }
        }

        if best_length >= MIN_MATCH {
            sink(Token::Match {
                length: best_length,
                distance: best_distance,
            });
            // 匹配区间内除首字节外的位置也入链，后续能匹配到更长的串。
            let end = (pos + best_length).min(length.saturating_sub(2));
            for insert in (pos + 1)..end {
                let hash = hash3(data, insert);
                prev[insert & mask] = head[hash];
                head[hash] = insert as u32;
            }
            pos += best_length;
        } else {
            sink(Token::Literal(data[pos]));
            pos += 1;
        }
    }
}

/// 一次统计结果：两组码表的频次与两种编码方案的位数（用于择优选码）。
struct EncodeStats {
    /// 字面量 / 长度码 0..=285 的出现次数。
    literal_freq: [u32; 286],
    /// 距离码 0..=29 的出现次数。
    distance_freq: [u32; 30],
    /// 固定 Huffman 下所有符号的位数（不含块头与长度/距离附加位）。
    fixed_symbol_bits: u64,
    /// 长度 / 距离附加位数（两种编码相同）。
    extra_bits: u64,
}

/// 统计频次与固定 Huffman 的位成本。
fn collect_stats(data: &[u8]) -> EncodeStats {
    let mut stats = EncodeStats {
        literal_freq: [0; 286],
        distance_freq: [0; 30],
        fixed_symbol_bits: 0,
        extra_bits: 0,
    };
    lz77(data, |token| match token {
        Token::Literal(byte) => {
            stats.literal_freq[byte as usize] += 1;
            stats.fixed_symbol_bits += fixed_literal_code(byte as u16).1 as u64;
        }
        Token::Match { length, distance } => {
            let (length_symbol, length_extra, _) = length_code(length);
            let (distance_symbol, distance_extra, _) = distance_code(distance);
            stats.literal_freq[length_symbol as usize] += 1;
            stats.distance_freq[distance_symbol as usize] += 1;
            stats.fixed_symbol_bits += fixed_literal_code(length_symbol).1 as u64;
            stats.fixed_symbol_bits += 5; // 固定表的距离码恒 5 位
            stats.extra_bits += length_extra as u64 + distance_extra as u64;
        }
    });
    // 块结束符。
    stats.literal_freq[256] += 1;
    stats.fixed_symbol_bits += fixed_literal_code(256).1 as u64;
    stats
}

/// 按频次构造 Huffman 码长；无解（空表 / 无法限长）返回 `None`。
///
/// 与 zlib 的 `gen_bitlen` 一样先在无限长前提下建树，再按 `max_bits` 做
/// 「溢出修复」（把过深的叶子往上提），最后按频次从低到高重新分配码长。
fn huffman_lengths(freqs: &[u32], max_bits: u8) -> Option<Vec<u8>> {
    let mut lengths = vec![0u8; freqs.len()];
    let symbols: Vec<usize> = (0..freqs.len()).filter(|index| freqs[*index] > 0).collect();
    match symbols.len() {
        0 => return None,
        // 单符号表：给 1 位码（DEFLATE 不允许 0 位码）。
        1 => {
            lengths[symbols[0]] = 1;
            return Some(lengths);
        }
        _ => {}
    }

    // 建树：节点下标即数组下标，叶子记录符号。
    let mut weights: Vec<u64> = Vec::with_capacity(symbols.len() * 2);
    let mut children: Vec<Option<(usize, usize)>> = Vec::with_capacity(symbols.len() * 2);
    let mut leaf: Vec<Option<usize>> = Vec::with_capacity(symbols.len() * 2);
    let mut heap: std::collections::BinaryHeap<std::cmp::Reverse<(u64, usize)>> =
        std::collections::BinaryHeap::new();
    for symbol in &symbols {
        let weight = freqs[*symbol] as u64;
        weights.push(weight);
        children.push(None);
        leaf.push(Some(*symbol));
        heap.push(std::cmp::Reverse((weight, weights.len() - 1)));
    }
    while heap.len() > 1 {
        let std::cmp::Reverse((left_weight, left)) = heap.pop()?;
        let std::cmp::Reverse((right_weight, right)) = heap.pop()?;
        let weight = left_weight + right_weight;
        weights.push(weight);
        children.push(Some((left, right)));
        leaf.push(None);
        heap.push(std::cmp::Reverse((weight, weights.len() - 1)));
    }
    // `Reverse((权重, 节点下标))`：先解包 Reverse 再取下标。
    let std::cmp::Reverse((_, root)) = heap.pop()?;

    // 深度优先算每个叶子的码长（边数）。
    let mut depths = vec![0u16; freqs.len()];
    let mut stack = vec![(root, 0u16)];
    while let Some((node, depth)) = stack.pop() {
        match children[node] {
            Some((left, right)) => {
                stack.push((left, depth + 1));
                stack.push((right, depth + 1));
            }
            None => depths[leaf[node]?] = depth,
        }
    }

    let limit = max_bits as usize;
    if depths
        .iter()
        .all(|depth| *depth == 0 || *depth as usize <= limit)
    {
        for (slot, depth) in lengths.iter_mut().zip(depths.iter()) {
            *slot = *depth as u8;
        }
        return Some(lengths);
    }

    // 限长修复：先把超过 limit 的码长截到 limit，再按 Kraft 和回收到满码。
    //
    // 只数「溢出个数」是不够的：被截断的叶子可能远深于 limit，截断后给
    // Kraft 和带来的增量未必是 2^-limit 的整数倍，按「每两个溢出抵一份」
    // 收尾会残留超订（实测 286 个符号的偏斜频次会残留 1~17 个 2^-15，
    // 解码端直接判「Huffman 码过度订阅」，整个备份包都读不回来）。
    // 这里直接盯 Kraft 和：每次「把一个较短的码拆成两个更长的兄弟码 +
    // 收回 limit 层的一个码」都让 Kraft 和净减 2^-limit，收到正好满码为止。
    let mut budget = vec![0u32; limit + 2];
    for depth in &depths {
        if *depth == 0 {
            continue;
        }
        budget[(*depth as usize).min(limit)] += 1;
    }
    let full: u64 = 1u64 << limit;
    let mut kraft: u64 = 0;
    for bits in 1..=limit {
        kraft += budget[bits] as u64 * (1u64 << (limit - bits));
    }
    while kraft > full {
        let mut bits = limit.saturating_sub(1);
        while bits > 0 && budget[bits] == 0 {
            bits -= 1;
        }
        if bits == 0 || budget[limit] == 0 {
            return None;
        }
        budget[bits] -= 1;
        budget[bits + 1] += 2;
        budget[limit] -= 1;
        kraft -= 1;
    }
    if kraft != full {
        // 收不到满码（理论上不会发生）：交给调用方回退其它编码方案。
        return None;
    }

    // 按频次从低到高分配码长：最稀有的符号拿最长的码。
    let mut order = symbols;
    order.sort_by_key(|symbol| (freqs[*symbol], *symbol));
    let mut cursor = 0usize;
    for bits in (1..=limit).rev() {
        let mut count = budget[bits];
        while count > 0 {
            let symbol = *order.get(cursor)?;
            cursor += 1;
            lengths[symbol] = bits as u8;
            count -= 1;
        }
    }
    if cursor != order.len() {
        return None;
    }
    Some(lengths)
}

/// 由码长生成规范 Huffman 码字（DEFLATE 规范 3.2.2）。
fn canonical_codes(lengths: &[u8]) -> Vec<u16> {
    let mut counts = [0u32; 16];
    for length in lengths {
        if *length > 0 {
            counts[*length as usize] += 1;
        }
    }
    let mut next_code = [0u32; 16];
    let mut code = 0u32;
    for bits in 1..16 {
        code = (code + counts[bits - 1]) << 1;
        next_code[bits] = code;
    }
    let mut codes = vec![0u16; lengths.len()];
    for (symbol, length) in lengths.iter().enumerate() {
        if *length > 0 {
            codes[symbol] = next_code[*length as usize] as u16;
            next_code[*length as usize] += 1;
        }
    }
    codes
}

/// 动态 Huffman 的完整码表（含码长表本身）。
struct DynamicTables {
    literal_lengths: Vec<u8>,
    literal_codes: Vec<u16>,
    distance_lengths: Vec<u8>,
    distance_codes: Vec<u16>,
    code_length_lengths: Vec<u8>,
    code_length_codes: Vec<u16>,
    /// 块头里实写的码长个数（已裁掉尾部 0）：`HLIT = literal_count - 257`。
    literal_count: usize,
    /// `HDIST = distance_count - 1`（至少 1）。
    distance_count: usize,
    /// 实写的码长码个数（>= 4）：`HCLEN = code_length_count - 4`。
    code_length_count: usize,
    /// 码长表的 RLE 序列（符号 16/17/18 + 附加位）。
    length_rle: Vec<LengthRun>,
    /// 块头（HLIT/HDIST/HCLEN + 码长表）占用的位数。
    header_bits: u64,
}

/// 码长表 RLE 的一个符号。
#[derive(Debug, Clone, Copy)]
struct LengthRun {
    /// 0..=15 是字面码长值，16/17/18 是 RFC 1951 的重复码。
    symbol: u8,
    extra: u8,
    extra_bits: u32,
}

/// 把码长序列压成 RFC 1951 的码长表符号序列。
///
/// 286 个字面码长 + 30 个距离码长里大量是 0（不用的符号），zlib 用
/// 17（3..=10 个 0）/ 18（11..=138 个 0）把它们压成十几个位；
/// 非 0 的连续重复用 16（重复前一个 3..=6 次）。不做这一步时块头
/// 固定要写 316 个码长（约 100+ 字节），每个压缩块都白付一次。
fn rle_code_lengths(lengths: &[u8]) -> Vec<LengthRun> {
    let mut runs: Vec<LengthRun> = Vec::new();
    let mut index = 0usize;
    while index < lengths.len() {
        let value = lengths[index];
        let mut repeat = 1usize;
        while index + repeat < lengths.len() && lengths[index + repeat] == value {
            repeat += 1;
        }
        index += repeat;
        if value == 0 {
            let mut left = repeat;
            while left >= 11 {
                let take = left.min(138);
                runs.push(LengthRun {
                    symbol: 18,
                    extra: (take - 11) as u8,
                    extra_bits: 7,
                });
                left -= take;
            }
            if left >= 3 {
                runs.push(LengthRun {
                    symbol: 17,
                    extra: (left - 3) as u8,
                    extra_bits: 3,
                });
                left = 0;
            }
            for _ in 0..left {
                runs.push(LengthRun {
                    symbol: 0,
                    extra: 0,
                    extra_bits: 0,
                });
            }
        } else {
            runs.push(LengthRun {
                symbol: value,
                extra: 0,
                extra_bits: 0,
            });
            let mut left = repeat - 1;
            while left >= 3 {
                let take = left.min(6);
                runs.push(LengthRun {
                    symbol: 16,
                    extra: (take - 3) as u8,
                    extra_bits: 2,
                });
                left -= take;
            }
            for _ in 0..left {
                runs.push(LengthRun {
                    symbol: value,
                    extra: 0,
                    extra_bits: 0,
                });
            }
        }
    }
    runs
}

/// 码长序列里最后一个非 0 的下标 + 1（全 0 时返回 `minimum`）。
fn trailing_trim(lengths: &[u8], minimum: usize) -> usize {
    let last = lengths
        .iter()
        .rposition(|length| *length != 0)
        .map(|index| index + 1)
        .unwrap_or(0);
    last.max(minimum).min(lengths.len())
}

/// 码长表的书写顺序（RFC 1951）。
const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// 码长集合是否过度订阅（Kraft 和 > 1）。编码端自检用：宁可回退固定表，
/// 也不能写出解码端（含其它实现）拒收的码表。
fn over_subscribed(lengths: &[u8], max_bits: u8) -> bool {
    let limit = max_bits as u32;
    let mut kraft: u64 = 0;
    for length in lengths {
        if *length == 0 {
            continue;
        }
        if *length > max_bits {
            return true;
        }
        kraft += 1u64 << (limit - *length as u32);
    }
    kraft > 1u64 << limit
}

/// 按统计结果构造动态 Huffman 码表；构造不出（码长超限 / 码表非法）返回 `None`。
fn build_dynamic_tables(stats: &EncodeStats) -> Option<DynamicTables> {
    let literal_lengths = huffman_lengths(&stats.literal_freq, 15)?;
    let mut distance_freq = stats.distance_freq;
    if distance_freq.iter().all(|freq| *freq == 0) {
        // 没有用到距离码时也要给一个合法码。
        distance_freq[0] = 1;
    }
    let distance_lengths = huffman_lengths(&distance_freq, 15)?;

    // 块头只写「用到的」那一段码长：尾部全 0 直接不写（HLIT/HDIST 会跟着变小）。
    // 字面码必须留到 256（块结束符）为止，距离码至少留 1 个。
    let literal_count = trailing_trim(&literal_lengths, 257);
    let distance_count = trailing_trim(&distance_lengths, 1);

    // 码长表本身：先对码长序列做 RLE，再统计 0..=18 各符号的出现次数。
    let mut sequence = Vec::with_capacity(literal_count + distance_count);
    sequence.extend_from_slice(&literal_lengths[..literal_count]);
    sequence.extend_from_slice(&distance_lengths[..distance_count]);
    let length_rle = rle_code_lengths(&sequence);
    let mut code_length_freq = [0u32; 19];
    for run in &length_rle {
        code_length_freq[run.symbol as usize] += 1;
    }
    let code_length_lengths = huffman_lengths(&code_length_freq, 7)?;

    // 三张表都必须可解码，否则整块数据谁也读不回来（备份包最不该出事）。
    if over_subscribed(&literal_lengths, 15)
        || over_subscribed(&distance_lengths, 15)
        || over_subscribed(&code_length_lengths, 7)
    {
        return None;
    }

    // HCLEN 也只写到最后一个非 0 的码长码（至少 4 个）。
    let code_length_count = CODE_LENGTH_ORDER
        .iter()
        .rposition(|slot| code_length_lengths[*slot] != 0)
        .map(|index| index + 1)
        .unwrap_or(0)
        .max(4)
        .min(CODE_LENGTH_ORDER.len());

    let header_bits = 3 // BFINAL + BTYPE
        + 5 + 5 + 4 // HLIT / HDIST / HCLEN
        + code_length_count as u64 * 3 // 码长码表自身（只写用到的部分）
        + length_rle
            .iter()
            .map(|run| {
                code_length_lengths[run.symbol as usize] as u64 + run.extra_bits as u64
            })
            .sum::<u64>();

    Some(DynamicTables {
        literal_codes: canonical_codes(&literal_lengths),
        distance_codes: canonical_codes(&distance_lengths),
        code_length_codes: canonical_codes(&code_length_lengths),
        literal_lengths,
        distance_lengths,
        code_length_lengths,
        literal_count,
        distance_count,
        code_length_count,
        length_rle,
        header_bits,
    })
}

/// 用动态码表编码一个 token。
fn write_token_dynamic(writer: &mut BitWriter, tables: &DynamicTables, token: Token) {
    match token {
        Token::Literal(byte) => {
            let symbol = byte as usize;
            writer.write_code(
                tables.literal_codes[symbol],
                tables.literal_lengths[symbol] as u32,
            );
        }
        Token::Match { length, distance } => {
            let (length_symbol, extra_bits, extra_value) = length_code(length);
            let index = length_symbol as usize;
            writer.write_code(tables.literal_codes[index], tables.literal_lengths[index] as u32);
            writer.write_bits(extra_value, extra_bits);
            let (distance_symbol, extra_bits, extra_value) = distance_code(distance);
            let index = distance_symbol as usize;
            writer.write_code(
                tables.distance_codes[index],
                tables.distance_lengths[index] as u32,
            );
            writer.write_bits(extra_value, extra_bits);
        }
    }
}

/// 用固定码表编码一个 token。
fn write_token_fixed(writer: &mut BitWriter, token: Token) {
    match token {
        Token::Literal(byte) => {
            let (code, bits) = fixed_literal_code(byte as u16);
            writer.write_code(code, bits);
        }
        Token::Match { length, distance } => {
            let (symbol, extra_bits, extra_value) = length_code(length);
            let (code, bits) = fixed_literal_code(symbol);
            writer.write_code(code, bits);
            writer.write_bits(extra_value, extra_bits);
            let (symbol, extra_bits, extra_value) = distance_code(distance);
            writer.write_code(symbol, 5);
            writer.write_bits(extra_value, extra_bits);
        }
    }
}

/// 输出一个固定 Huffman 块。
fn deflate_fixed(data: &[u8]) -> Vec<u8> {
    let mut writer = BitWriter::new();
    writer.write_bits(1, 1); // BFINAL = 1
    writer.write_bits(1, 2); // BTYPE = 01（固定 Huffman）
    lz77(data, |token| write_token_fixed(&mut writer, token));
    let (code, bits) = fixed_literal_code(256); // 块结束
    writer.write_code(code, bits);
    writer.finish()
}

/// 输出一个动态 Huffman 块。
fn deflate_dynamic(data: &[u8], tables: &DynamicTables) -> Vec<u8> {
    let mut writer = BitWriter::new();
    writer.write_bits(1, 1); // BFINAL = 1
    writer.write_bits(2, 2); // BTYPE = 10（动态 Huffman）
    writer.write_bits((tables.literal_count - 257) as u32, 5); // HLIT
    writer.write_bits((tables.distance_count - 1) as u32, 5); // HDIST
    writer.write_bits((tables.code_length_count - 4) as u32, 4); // HCLEN
    for slot in CODE_LENGTH_ORDER.iter().take(tables.code_length_count) {
        writer.write_bits(tables.code_length_lengths[*slot] as u32, 3);
    }
    // 码长表本身也走 RLE（0 的连续段用 17/18，非 0 重复用 16），
    // 再按码长码输出。
    for run in &tables.length_rle {
        let symbol = run.symbol as usize;
        writer.write_code(
            tables.code_length_codes[symbol],
            tables.code_length_lengths[symbol] as u32,
        );
        if run.extra_bits > 0 {
            writer.write_bits(run.extra as u32, run.extra_bits);
        }
    }
    lz77(data, |token| write_token_dynamic(&mut writer, tables, token));
    let symbol = 256usize;
    writer.write_code(
        tables.literal_codes[symbol],
        tables.literal_lengths[symbol] as u32,
    );
    writer.finish()
}

/// 在「固定 Huffman / 动态 Huffman / 存储块」里选体积最小的一种编码。
fn best_encoding(data: &[u8]) -> Vec<u8> {
    let stats = collect_stats(data);
    let fixed_bits = 3 + stats.fixed_symbol_bits + stats.extra_bits;
    let dynamic = build_dynamic_tables(&stats);
    let dynamic_bits = match &dynamic {
        Some(tables) => Some(
            tables.header_bits
                + stats
                    .literal_freq
                    .iter()
                    .enumerate()
                    .map(|(symbol, freq)| {
                        *freq as u64 * tables.literal_lengths[symbol] as u64
                    })
                    .sum::<u64>()
                + stats
                    .distance_freq
                    .iter()
                    .enumerate()
                    .map(|(symbol, freq)| *freq as u64 * tables.distance_lengths[symbol] as u64)
                    .sum::<u64>()
                + stats.extra_bits,
        ),
        None => None,
    };

    // 用位预算比较三种方案，只有估算真的更省时才走对应编码器。
    let fixed_bytes = fixed_bits.div_ceil(8) as usize;
    let dynamic_bytes = dynamic_bits.map(|bits| bits.div_ceil(8) as usize);
    let stored_bytes = data.len() + data.len().div_ceil(STORED_BLOCK_MAX) * 5 + 5;

    match (dynamic, dynamic_bytes) {
        (Some(tables), Some(dynamic_bytes))
            if dynamic_bytes < fixed_bytes.min(stored_bytes) =>
        {
            deflate_dynamic(data, &tables)
        }
        _ if fixed_bytes < stored_bytes => deflate_fixed(data),
        _ => deflate_stored(data),
    }
}

/// 压缩 raw DEFLATE（RFC 1951，无 zlib / gzip 包裹）。
///
/// 自动在固定 Huffman / 动态 Huffman / 存储块之间择优，
/// 因此产出**永远不会明显大于原文**。
pub fn deflate(data: &[u8]) -> Vec<u8> {
    // 位置用 u32 记录，超过 4GiB 的输入直接走存储块（备份场景不会遇到）。
    if data.len() < MIN_MATCH || data.len() > u32::MAX as usize {
        return deflate_stored(data);
    }
    best_encoding(data)
}

/// 压缩并在「压不小」时返回 `None`（ZIP 写入用它决定条目用 deflate 还是 stored）。
pub fn deflate_if_smaller(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < MIN_MATCH || data.len() > u32::MAX as usize {
        return None;
    }
    let compressed = best_encoding(data);
    if compressed.len() < data.len() {
        Some(compressed)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// ZIP 写入（文件条目使用 deflate）
// ---------------------------------------------------------------------------

/// 中央目录条目。
#[derive(Debug)]
struct CentralEntry {
    name: Vec<u8>,
    /// 压缩方法：0 = stored，8 = deflate。
    method: u16,
    crc: u32,
    /// 压缩后的字节数。
    compressed_size: u64,
    /// 原始字节数。
    size: u64,
    offset: u32,
    is_dir: bool,
}

/// ZIP 写入器（文件条目用 `deflate`，目录与不可压缩内容用 `stored`）。
#[derive(Debug, Default)]
pub struct ZipWriter {
    buffer: Vec<u8>,
    entries: Vec<CentralEntry>,
}

impl ZipWriter {
    /// 新建写入器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一个文件条目。
    pub fn add_file(&mut self, name: &str, data: &[u8]) -> Result<()> {
        self.add_entry(name, data, false)
    }

    /// 追加一个目录条目（以 `/` 结尾）。
    pub fn add_dir(&mut self, name: &str) -> Result<()> {
        let mut full = name.to_string();
        if !full.ends_with('/') {
            full.push('/');
        }
        self.add_entry(&full, &[], true)
    }

    fn add_entry(&mut self, name: &str, data: &[u8], is_dir: bool) -> Result<()> {
        let name_bytes = name.as_bytes().to_vec();
        let offset = self.buffer.len() as u32;
        let crc = crc32(data);

        // 目录条目没有内容；其余条目能压小就用 deflate，
        // 压不小（如图片、已压缩文件）则保留 stored，避免包体反而变大。
        let (method, payload): (u16, Cow<'_, [u8]>) = if is_dir || data.is_empty() {
            (0, Cow::Borrowed(data))
        } else {
            match deflate_if_smaller(data) {
                Some(compressed) => (8, Cow::Owned(compressed)),
                None => (0, Cow::Borrowed(data)),
            }
        };
        let compressed_size = payload.len() as u64;

        // 本地文件头
        self.buffer.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        self.buffer.extend_from_slice(&20u16.to_le_bytes()); // 解压所需版本
        self.buffer.extend_from_slice(&0x0800u16.to_le_bytes()); // 通用标志位：名称 UTF-8
        self.buffer.extend_from_slice(&method.to_le_bytes()); // 压缩方法
        self.buffer.extend_from_slice(&0u16.to_le_bytes()); // 修改时间
        self.buffer.extend_from_slice(&0u16.to_le_bytes()); // 修改日期
        self.buffer.extend_from_slice(&crc.to_le_bytes());
        self.buffer
            .extend_from_slice(&(compressed_size as u32).to_le_bytes());
        self.buffer.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.buffer
            .extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        self.buffer.extend_from_slice(&0u16.to_le_bytes()); // 扩展字段长度
        self.buffer.extend_from_slice(&name_bytes);
        self.buffer.extend_from_slice(&payload);

        self.entries.push(CentralEntry {
            name: name_bytes,
            method,
            crc,
            compressed_size,
            size: data.len() as u64,
            offset,
            is_dir,
        });
        Ok(())
    }

    /// 收尾并返回 ZIP 字节流。
    pub fn finish(mut self) -> Vec<u8> {
        let cd_offset = self.buffer.len() as u32;
        let cd_size = {
            let start = self.buffer.len();
            for entry in &self.entries {
                self.buffer.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
                self.buffer.extend_from_slice(&20u16.to_le_bytes()); // 生成版本
                self.buffer.extend_from_slice(&20u16.to_le_bytes()); // 解压所需版本
                self.buffer.extend_from_slice(&0x0800u16.to_le_bytes());
                self.buffer.extend_from_slice(&entry.method.to_le_bytes());
                self.buffer.extend_from_slice(&0u16.to_le_bytes());
                self.buffer.extend_from_slice(&0u16.to_le_bytes());
                self.buffer.extend_from_slice(&entry.crc.to_le_bytes());
                self.buffer
                    .extend_from_slice(&(entry.compressed_size as u32).to_le_bytes());
                self.buffer
                    .extend_from_slice(&(entry.size as u32).to_le_bytes());
                self.buffer
                    .extend_from_slice(&(entry.name.len() as u16).to_le_bytes());
                self.buffer.extend_from_slice(&0u16.to_le_bytes()); // 扩展字段长度
                self.buffer.extend_from_slice(&0u16.to_le_bytes()); // 注释长度
                self.buffer.extend_from_slice(&0u16.to_le_bytes()); // 起始磁盘号
                self.buffer.extend_from_slice(&0u16.to_le_bytes()); // 内部属性
                let external = if entry.is_dir { 0x10u32 } else { 0u32 };
                self.buffer.extend_from_slice(&external.to_le_bytes());
                self.buffer.extend_from_slice(&entry.offset.to_le_bytes());
                self.buffer.extend_from_slice(&entry.name);
            }
            self.buffer.len() - start
        } as u32;

        // 中央目录结束记录
        self.buffer.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        self.buffer.extend_from_slice(&0u16.to_le_bytes());
        self.buffer.extend_from_slice(&0u16.to_le_bytes());
        self.buffer
            .extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        self.buffer
            .extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        self.buffer.extend_from_slice(&cd_size.to_le_bytes());
        self.buffer.extend_from_slice(&cd_offset.to_le_bytes());
        self.buffer.extend_from_slice(&0u16.to_le_bytes()); // 注释长度
        self.buffer
    }
}

// ---------------------------------------------------------------------------
// ZIP 读取
// ---------------------------------------------------------------------------

/// ZIP 中央目录里的一个条目。
#[derive(Debug, Clone)]
pub struct ZipEntry {
    pub name: String,
    /// 压缩方法：0 = stored，8 = deflate。
    pub method: u16,
    pub crc32: u32,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub local_offset: u64,
    pub is_dir: bool,
}

/// ZIP 归档。
#[derive(Debug, Clone)]
pub struct ZipArchive {
    data: Vec<u8>,
    entries: Vec<ZipEntry>,
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16> {
    let slice = data
        .get(offset..offset + 2)
        .ok_or_else(|| anyhow!("ZIP 数据在偏移 {offset} 处越界"))?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32> {
    let slice = data
        .get(offset..offset + 4)
        .ok_or_else(|| anyhow!("ZIP 数据在偏移 {offset} 处越界"))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

impl ZipArchive {
    /// 解析 ZIP 字节流。
    pub fn open(data: Vec<u8>) -> Result<Self> {
        // 从尾部向前找中央目录结束记录。
        let eocd = find_eocd(&data).ok_or_else(|| anyhow!("不是有效的 ZIP：找不到中央目录"))?;
        let total = read_u16(&data, eocd + 10)? as usize;
        let cd_offset = read_u32(&data, eocd + 16)? as usize;

        let mut entries = Vec::with_capacity(total);
        let mut cursor = cd_offset;
        for _ in 0..total {
            if read_u32(&data, cursor)? != 0x0201_4b50 {
                bail!("中央目录签名不匹配");
            }
            let method = read_u16(&data, cursor + 10)?;
            let crc = read_u32(&data, cursor + 16)?;
            let compressed_size = read_u32(&data, cursor + 20)? as u64;
            let uncompressed_size = read_u32(&data, cursor + 24)? as u64;
            let name_len = read_u16(&data, cursor + 28)? as usize;
            let extra_len = read_u16(&data, cursor + 30)? as usize;
            let comment_len = read_u16(&data, cursor + 32)? as usize;
            let external = read_u32(&data, cursor + 38)?;
            let local_offset = read_u32(&data, cursor + 42)? as u64;
            let name_bytes = data
                .get(cursor + 46..cursor + 46 + name_len)
                .ok_or_else(|| anyhow!("条目名越界"))?;
            let name = String::from_utf8_lossy(name_bytes).to_string();
            entries.push(ZipEntry {
                is_dir: name.ends_with('/') || external & 0x10 != 0,
                name,
                method,
                crc32: crc,
                compressed_size,
                uncompressed_size,
                local_offset,
            });
            cursor += 46 + name_len + extra_len + comment_len;
        }
        Ok(Self { data, entries })
    }

    /// 条目列表。
    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }

    /// 按名字取条目。
    pub fn find(&self, name: &str) -> Option<&ZipEntry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    /// 读取条目内容（自动按压缩方法解码并校验 CRC）。
    pub fn read_entry(&self, entry: &ZipEntry) -> Result<Vec<u8>> {
        let offset = entry.local_offset as usize;
        if read_u32(&self.data, offset)? != 0x0403_4b50 {
            bail!("本地文件头签名不匹配：{}", entry.name);
        }
        let name_len = read_u16(&self.data, offset + 26)? as usize;
        let extra_len = read_u16(&self.data, offset + 28)? as usize;
        let start = offset + 30 + name_len + extra_len;
        let end = start + entry.compressed_size as usize;
        let raw = self
            .data
            .get(start..end)
            .ok_or_else(|| anyhow!("条目数据越界：{}", entry.name))?;
        let content = match entry.method {
            0 => raw.to_vec(),
            8 => inflate(raw).with_context(|| format!("解压 deflate 失败：{}", entry.name))?,
            other => bail!("不支持的压缩方法 {other}：{}", entry.name),
        };
        let actual = crc32(&content);
        if actual != entry.crc32 {
            bail!(
                "条目 {} CRC 校验失败（期望 {:08x}，实际 {:08x}）",
                entry.name,
                entry.crc32,
                actual
            );
        }
        Ok(content)
    }

    /// 按名字读取内容。
    pub fn read_by_name(&self, name: &str) -> Result<Vec<u8>> {
        let entry = self
            .find(name)
            .ok_or_else(|| anyhow!("ZIP 中没有条目 {name}"))?;
        self.read_entry(entry)
    }

    /// 解压到目录（带 Zip-Slip 防护），返回写出的文件相对路径。
    pub fn extract_to(&self, dest: &Path) -> Result<Vec<String>> {
        let mut written = Vec::new();
        for entry in &self.entries {
            let relative = safe_relative_path(&entry.name)?;
            let target = dest.join(&relative);
            if entry.is_dir {
                std::fs::create_dir_all(&target)
                    .with_context(|| format!("创建目录失败：{}", target.display()))?;
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("创建目录失败：{}", parent.display()))?;
            }
            let content = self.read_entry(entry)?;
            std::fs::write(&target, &content)
                .with_context(|| format!("写入文件失败：{}", target.display()))?;
            written.push(relative.to_string_lossy().to_string());
        }
        Ok(written)
    }
}

/// 从尾部向前查找中央目录结束记录。
fn find_eocd(data: &[u8]) -> Option<usize> {
    if data.len() < 22 {
        return None;
    }
    let start = data.len().saturating_sub(22 + 0xFFFF);
    let mut index = data.len() - 22;
    loop {
        if data[index..index + 4] == [0x50, 0x4b, 0x05, 0x06] {
            return Some(index);
        }
        if index == start {
            return None;
        }
        index -= 1;
    }
}

/// 把归档内条目名转换成安全的相对路径（拒绝绝对路径与上级目录）。
fn safe_relative_path(name: &str) -> Result<PathBuf> {
    let normalized = name.replace('\\', "/");
    let path = Path::new(&normalized);
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => result.push(part),
            Component::CurDir => {}
            Component::ParentDir => bail!("归档条目路径非法（试图越界）：{name}"),
            Component::RootDir | Component::Prefix(_) => bail!("归档条目路径非法（绝对路径）：{name}"),
        }
    }
    if result.as_os_str().is_empty() {
        bail!("归档条目路径为空：{name}");
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// gzip 与 tar
// ---------------------------------------------------------------------------

/// 解压 gzip 数据（RFC 1952）。
pub fn gunzip(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 18 {
        bail!("gzip 数据太短");
    }
    if data[0] != 0x1f || data[1] != 0x8b {
        bail!("gzip 魔数不匹配");
    }
    if data[2] != 8 {
        bail!("暂不支持的 gzip 压缩方法 {}", data[2]);
    }
    let flags = data[3];
    let mut cursor = 10usize;
    if flags & 0x04 != 0 {
        // FEXTRA
        let extra_len = read_u16(data, cursor)? as usize;
        cursor += 2 + extra_len;
    }
    if flags & 0x08 != 0 {
        // FNAME
        while cursor < data.len() && data[cursor] != 0 {
            cursor += 1;
        }
        cursor += 1;
    }
    if flags & 0x10 != 0 {
        // FCOMMENT
        while cursor < data.len() && data[cursor] != 0 {
            cursor += 1;
        }
        cursor += 1;
    }
    if flags & 0x02 != 0 {
        // FHCRC
        cursor += 2;
    }
    if cursor >= data.len() {
        bail!("gzip 头越界");
    }
    inflate(&data[cursor..]).context("gzip 解压失败")
}

/// tar 归档里的一个条目。
#[derive(Debug, Clone)]
pub struct TarEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub data: Vec<u8>,
}

/// 解析 tar 字节流。
pub fn extract_tar(data: &[u8]) -> Result<Vec<TarEntry>> {
    let mut entries = Vec::new();
    let mut cursor = 0usize;
    let mut pending_long_name: Option<String> = None;

    while cursor + 512 <= data.len() {
        let header = &data[cursor..cursor + 512];
        // 全零块表示结束。
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        cursor += 512;

        let raw_name = cstr(&header[0..100]);
        let size_text = cstr(&header[124..136]);
        let size = usize::from_str_radix(size_text.trim(), 8)
            .map_err(|err| anyhow!("tar 条目大小解析失败：{err}"))?;
        let type_flag = header[156];
        let prefix = cstr(&header[345..500]);
        let is_ustar = &header[257..262] == b"ustar";

        let name = match pending_long_name.take() {
            Some(long) => long,
            None => {
                if is_ustar && !prefix.is_empty() {
                    format!("{prefix}/{raw_name}")
                } else {
                    raw_name.clone()
                }
            }
        };

        let content_end = cursor + size;
        if content_end > data.len() {
            bail!("tar 条目 {name} 内容越界");
        }
        let content = data[cursor..content_end].to_vec();
        cursor += size.div_ceil(512) * 512;

        match type_flag {
            b'L' => {
                // GNU 长文件名：内容就是下一个条目的名字。
                pending_long_name = Some(cstr(&content));
                continue;
            }
            b'0' | 0 | b' ' => entries.push(TarEntry {
                name,
                is_dir: false,
                size: size as u64,
                data: content,
            }),
            b'5' => entries.push(TarEntry {
                name,
                is_dir: true,
                size: 0,
                data: Vec::new(),
            }),
            _ => {}
        }
    }
    Ok(entries)
}

/// 读取 tar.gz 归档。
pub fn extract_tar_gz(data: &[u8]) -> Result<Vec<TarEntry>> {
    let raw = gunzip(data)?;
    extract_tar(&raw)
}

/// 取 C 风格字符串（到 NUL 或结尾）。
fn cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).to_string()
}

// ---------------------------------------------------------------------------
// 便捷函数
// ---------------------------------------------------------------------------

/// 把目录打包成 ZIP（文件条目用 deflate 压缩）。
pub fn zip_directory(dir: &Path) -> Result<Vec<u8>> {
    if !dir.is_dir() {
        bail!("{} 不是目录", dir.display());
    }
    let mut writer = ZipWriter::new();
    collect_directory(dir, dir, &mut writer)?;
    Ok(writer.finish())
}

/// 递归收集目录内容。
fn collect_directory(root: &Path, current: &Path, writer: &mut ZipWriter) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(current)
        .with_context(|| format!("读取目录失败：{}", current.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            writer.add_dir(&relative)?;
            collect_directory(root, &path, writer)?;
        } else {
            let content = std::fs::read(&path)
                .with_context(|| format!("读取文件失败：{}", path.display()))?;
            writer.add_file(&relative, &content)?;
        }
    }
    Ok(())
}

/// 把 ZIP 字节流解压到目录（带 Zip-Slip 防护）。
pub fn unzip_bytes_to_dir(data: &[u8], dest: &Path) -> Result<Vec<String>> {
    let archive = ZipArchive::open(data.to_vec())?;
    archive.extract_to(dest)
}

/// 把 tar.gz 字节流解压到目录（带 Zip-Slip 防护）。
pub fn untar_gz_to_dir(data: &[u8], dest: &Path) -> Result<Vec<String>> {
    let entries = extract_tar_gz(data)?;
    let mut written = Vec::new();
    for entry in entries {
        let relative = safe_relative_path(&entry.name)?;
        let target = dest.join(&relative);
        if entry.is_dir {
            std::fs::create_dir_all(&target)
                .with_context(|| format!("创建目录失败：{}", target.display()))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建目录失败：{}", parent.display()))?;
        }
        std::fs::write(&target, &entry.data)
            .with_context(|| format!("写入文件失败：{}", target.display()))?;
        written.push(relative.to_string_lossy().to_string());
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// .NET 生成的 raw deflate 样本（动态 Huffman）。
    const DEFLATE_SAMPLE_B64: &str =
        "y0jNyclXKCotLknOLVZIrShRyMxLy0ksSVUoSS0uUcgYlR6VHpUelR5q0gA=";
    /// 样本对应的原文。
    const DEFLATE_SAMPLE_TEXT: &str = "hello rustcms ext inflate test ";
    /// .NET 生成的 gzip+tar 样本（内含 hello.txt）。
    const GZIP_TAR_SAMPLE_B64: &str = "H4sIAAAAAAAEAMtIzcnJ1yupKGGgHTAAAjMTEzANBOi0gYGhGYOhCZwHEje2NLdkUKChm+CgtLgksQhoJT3sGoQgAxT/CsAgUCjPL8pJ4Rpo94yCUTAKRsEooA8AAM8f9h0ACAAA";

    fn decode_base64(text: &str) -> Vec<u8> {
        crate::smtp::base64_decode(text).expect("base64")
    }

    /// 构造一个 `stored` 类型的 deflate 块（BFINAL=1，BTYPE=00）。
    fn deflate_stored(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x01u8];
        let length = data.len() as u16;
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    /// 手工构造一个 ZIP 字节流（可指定压缩方法）。
    fn build_zip(name: &str, content: &[u8], method: u16, payload: &[u8]) -> Vec<u8> {
        let name_bytes = name.as_bytes();
        let crc = crc32(content);
        let mut out: Vec<u8> = Vec::new();
        let local_offset = 0u32;
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0x0800u16.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&(content.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(payload);

        let cd_offset = out.len() as u32;
        out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0x0800u16.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&(content.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&local_offset.to_le_bytes());
        out.extend_from_slice(name_bytes);

        let cd_size = out.len() as u32 - cd_offset;
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn crc32_matches_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn inflate_reads_dynamic_huffman_sample() {
        let raw = decode_base64(DEFLATE_SAMPLE_B64);
        let decoded = inflate(&raw).expect("inflate");
        let expected = DEFLATE_SAMPLE_TEXT.repeat(40);
        assert_eq!(decoded.len(), expected.len());
        assert_eq!(decoded, expected.as_bytes());
    }

    #[test]
    fn inflate_reads_stored_block() {
        let payload = b"stored block payload";
        let raw = deflate_stored(payload);
        assert_eq!(inflate(&raw).unwrap(), payload.to_vec());
    }

    #[test]
    fn inflate_rejects_truncated_input() {
        let raw = decode_base64(DEFLATE_SAMPLE_B64);
        let truncated = &raw[..raw.len() / 2];
        assert!(inflate(truncated).is_err());
    }

    #[test]
    fn zip_write_and_read_round_trip() {
        let mut writer = ZipWriter::new();
        writer.add_file("a.txt", b"hello a").unwrap();
        writer.add_dir("sub").unwrap();
        writer.add_file("sub/b.txt", b"hello b").unwrap();
        let bytes = writer.finish();

        let archive = ZipArchive::open(bytes).expect("open");
        assert_eq!(archive.entries().len(), 3);
        assert_eq!(archive.read_by_name("a.txt").unwrap(), b"hello a".to_vec());
        assert_eq!(archive.read_by_name("sub/b.txt").unwrap(), b"hello b".to_vec());
        let dir = archive.find("sub/").expect("dir entry");
        assert!(dir.is_dir);
        assert!(archive.read_by_name("missing.txt").is_err());
    }

    #[test]
    fn zip_reads_deflate_entry() {
        let content = b"deflate inside zip";
        let payload = deflate_stored(content);
        let bytes = build_zip("x.bin", content, 8, &payload);
        let archive = ZipArchive::open(bytes).expect("open");
        assert_eq!(archive.entries().len(), 1);
        assert_eq!(archive.read_by_name("x.bin").unwrap(), content.to_vec());
    }

    #[test]
    fn zip_detects_corrupted_entry() {
        let bytes = build_zip("x.bin", b"good", 0, b"good");
        let mut corrupted = bytes.clone();
        // 破坏文件内容（最后一个字节在 EOCD 之前，改中间的数据区）。
        let data_offset = 30 + "x.bin".len();
        corrupted[data_offset] ^= 0xFF;
        let archive = ZipArchive::open(corrupted).expect("open");
        assert!(archive.read_by_name("x.bin").is_err());
    }

    #[test]
    fn safe_path_rejects_traversal() {
        assert!(safe_relative_path("../evil.txt").is_err());
        assert!(safe_relative_path("a/../../evil.txt").is_err());
        assert!(safe_relative_path("/etc/passwd").is_err());
        assert_eq!(
            safe_relative_path("a/b.txt").unwrap(),
            PathBuf::from("a").join("b.txt")
        );
    }

    #[test]
    fn extract_to_rejects_traversal_entries() {
        let bytes = build_zip("../evil.txt", b"bad", 0, b"bad");
        let archive = ZipArchive::open(bytes).expect("open");
        let dest = std::env::temp_dir().join(format!("rustcms-zip-{}", std::process::id()));
        assert!(archive.extract_to(&dest).is_err());
    }

    #[test]
    fn tar_gz_sample_is_parsed() {
        let raw = decode_base64(GZIP_TAR_SAMPLE_B64);
        let entries = extract_tar_gz(&raw).expect("tar.gz");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "hello.txt");
        assert!(!entries[0].is_dir);
        assert_eq!(entries[0].data, b"hello tar worl".to_vec());
    }

    #[test]
    fn gunzip_rejects_bad_magic() {
        let mut raw = decode_base64(GZIP_TAR_SAMPLE_B64);
        raw[0] = 0x00;
        assert!(gunzip(&raw).is_err());
    }

    #[test]
    fn directory_round_trip() {
        let root = std::env::temp_dir().join(format!("rustcms-packer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.txt"), b"content a").unwrap();
        std::fs::write(root.join("sub/b.txt"), b"content b").unwrap();

        let zipped = zip_directory(&root).expect("zip");
        let dest = std::env::temp_dir().join(format!("rustcms-packer-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dest);
        let written = unzip_bytes_to_dir(&zipped, &dest).expect("unzip");
        assert_eq!(written.len(), 2);
        assert_eq!(
            std::fs::read(dest.join("a.txt")).unwrap(),
            b"content a".to_vec()
        );
        assert_eq!(
            std::fs::read(dest.join("sub/b.txt")).unwrap(),
            b"content b".to_vec()
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&dest);
    }

    /// 伪随机字节（不引 rand，线性同余即可）。
    fn pseudo_random_bytes(length: usize, seed: u32) -> Vec<u8> {
        let mut state = seed | 1;
        let mut out = Vec::with_capacity(length);
        for _ in 0..length {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            out.push((state >> 16) as u8);
        }
        out
    }

    /// 一段像 SQL 备份的文本：重复度高，deflate 应当明显压小。
    fn sql_like_text(repeat: usize) -> Vec<u8> {
        let mut text = String::new();
        for index in 0..repeat {
            text.push_str(&format!(
                "INSERT INTO `archives` (`id`,`title`,`status`) VALUES ({index},'示例标题 {index}',1);\n"
            ));
        }
        text.into_bytes()
    }

    /// 交叉验证用：把样本落盘并打印本实现的压缩体积（供外部对照工具度量同一份数据的压缩率）。
    ///
    /// 默认 `#[ignore]`，只在手工跑互通测量时执行：
    /// `cargo test -p rustcms-ext --lib --offline measure_deflate_samples -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn measure_deflate_samples() {
        let dir = std::env::temp_dir().join("rustcms-deflate-samples");
        std::fs::create_dir_all(&dir).expect("create dir");
        let mut html = String::new();
        while html.len() < 264_000 {
            html.push_str(
                "<div class=\"archive-item\"><h3 class=\"title\">示例标题</h3>\
                 <p class=\"summary\">这里是摘要文本，用于测量模板 HTML 的压缩率。</p>\
                 <a class=\"more\" href=\"/archive/123.html\">阅读全文</a></div>\n",
            );
        }
        let samples: Vec<(&str, Vec<u8>)> = vec![
            ("sql", sql_like_text(55_000)),
            ("html", html.into_bytes()),
            ("random", pseudo_random_bytes(200_000, 7)),
            ("entropy", pseudo_random_bytes(3_200_000, 99)),
        ];
        for (name, data) in &samples {
            std::fs::write(dir.join(format!("{name}.bin")), data).expect("write sample");
            let packed = deflate(data);
            assert_eq!(inflate(&packed).expect("inflate"), *data, "{name} 往返不一致");
            // 落盘供外部校验工具验证「本实现产出的流是合规 DEFLATE、可被完整还原」。
            std::fs::write(dir.join(format!("{name}.deflate")), &packed).expect("write deflate");
            println!(
                "{name}: raw={} deflate={} ratio={:.1}%",
                data.len(),
                packed.len(),
                packed.len() as f64 * 100.0 / data.len() as f64
            );
        }
        println!("SAMPLES_DIR={}", dir.display());
    }

    /// 反向互通：读外部压缩器产出的 `<name>.std.deflate` 样本，本实现必须能解开。
    ///
    /// 配套：先跑 `measure_deflate_samples` 落样本，再用外部脚本产出 `.std.deflate` 样本。
    #[test]
    #[ignore]
    fn verify_std_deflate_samples() {
        let dir = std::env::temp_dir().join("rustcms-deflate-samples");
        for name in ["sql", "html", "random", "entropy"] {
            let raw = std::fs::read(dir.join(format!("{name}.bin"))).expect("sample");
            let packed = std::fs::read(dir.join(format!("{name}.std.deflate"))).expect("std deflate");
            let restored = inflate(&packed).expect("解压标准实现产出");
            assert_eq!(restored, raw, "{name}: 标准实现的 deflate 解压不一致");
            println!("{name}: std deflate {} B -> inflate OK", packed.len());
        }
    }

    #[test]
    fn deflate_round_trips_various_inputs() {
        let cases: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"a".to_vec(),
            b"abc".to_vec(),
            b"hello rustcms deflate round trip".to_vec(),
            b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_vec(),
            sql_like_text(200),
            pseudo_random_bytes(4096, 7),
        ];
        for case in cases {
            let compressed = deflate(&case);
            let restored = inflate(&compressed).expect("inflate");
            assert_eq!(restored, case, "往返不一致（长度 {}）", case.len());
        }
    }

    #[test]
    fn deflate_round_trips_input_larger_than_window() {
        // 超过 32KiB 窗口，覆盖哈希链回绕与前驱链替换。
        let chunk: Vec<u8> = b"rustcms-backup-row-0123456789-abcdefghij-".repeat(3);
        let mut data = Vec::new();
        while data.len() < 300_000 {
            data.extend_from_slice(&chunk);
        }
        data.extend_from_slice(&pseudo_random_bytes(1024, 99));
        let compressed = deflate(&data);
        assert!(
            compressed.len() < data.len() / 2,
            "重复文本应显著压缩：{} -> {}",
            data.len(),
            compressed.len()
        );
        assert_eq!(inflate(&compressed).unwrap(), data);
    }

    #[test]
    fn deflate_emits_fixed_huffman_block() {
        // 直接测固定 Huffman 编码器：BFINAL=1、BTYPE=01 → 首字节低 3 位为 0b011。
        let data = sql_like_text(50);
        let compressed = deflate_fixed(&data);
        assert_eq!(compressed[0] & 0b111, 0b011, "应使用固定 Huffman 块");
        assert_eq!(inflate(&compressed).unwrap(), data);
    }

    #[test]
    fn deflate_uses_dynamic_huffman_for_text() {
        // BFINAL=1、BTYPE=10 → 首字节 bit1..2 为 0b10（即 & 0b110 == 0b100）。
        let data = sql_like_text(400);
        let compressed = deflate(&data);
        assert_eq!(
            compressed[0] & 0b110,
            0b100,
            "文本应使用动态 Huffman 块（首字节 {:08b}）",
            compressed[0]
        );
        assert_eq!(inflate(&compressed).unwrap(), data);
        // 动态表应比固定表更省。
        assert!(
            compressed.len() < deflate_fixed(&data).len(),
            "动态 Huffman 应小于固定 Huffman"
        );
    }

    #[test]
    fn huffman_lengths_respects_max_bits_and_is_complete() {
        // 极端偏斜的频次分布（斐波那契式），不限长会超过 15 位。
        let mut freqs = vec![0u32; 40];
        let mut previous = 1u32;
        let mut current = 1u32;
        for slot in freqs.iter_mut().take(30) {
            *slot = current;
            let next = previous.saturating_add(current);
            previous = current;
            current = next;
        }
        let lengths = huffman_lengths(&freqs, 15).expect("应当能限长");
        assert!(lengths.iter().all(|length| *length <= 15), "{lengths:?}");
        // 出现过频次的符号都必须有待码长。
        for (symbol, freq) in freqs.iter().enumerate() {
            if *freq > 0 {
                assert!(lengths[symbol] > 0, "符号 {symbol} 缺失码长");
            }
        }
        // 更长码长只能给更稀有的符号：码长随频次单调不增。
        let mut pairs: Vec<(u32, u8)> = freqs
            .iter()
            .copied()
            .zip(lengths.iter().copied())
            .filter(|(freq, _)| *freq > 0)
            .collect();
        // 只按频次排序（稳定排序保留符号顺序，等频符号的码长仍单调）。
        pairs.sort_by_key(|pair| pair.0);
        for window in pairs.windows(2) {
            assert!(window[0].1 >= window[1].1, "码长分配与频次不单调：{pairs:?}");
        }
        // 单符号表给 1 位码。
        assert_eq!(huffman_lengths(&[0, 7, 0], 15).unwrap(), vec![0, 1, 0]);
        assert!(huffman_lengths(&[0, 0, 0], 15).is_none());
    }

    /// 码长集合的 Kraft 和，单位是 2^-max_bits；满码时应等于 1 << max_bits。
    fn kraft_units(lengths: &[u8], max_bits: u8) -> u64 {
        lengths
            .iter()
            .filter(|length| **length > 0)
            .map(|length| 1u64 << (max_bits as u32 - *length as u32))
            .sum()
    }

    #[test]
    fn huffman_lengths_never_over_subscribes_after_limiting() {
        // 斐波那契频次会让树深远超 15 位。历史 bug：限长只按「溢出个数」
        // 回收，截断后残留超订（Kraft 和 32769 > 32768），压缩看着正常、
        // 解压端一律报「Huffman 码过度订阅」——3MiB 以上的备份包全读不回来。
        for symbols in [20usize, 30, 40, 100, 200, 286] {
            let mut freqs = vec![0u32; symbols];
            let mut previous = 1u32;
            let mut current = 1u32;
            for slot in freqs.iter_mut().take(symbols.saturating_sub(10)) {
                *slot = current;
                let next = previous.saturating_add(current);
                previous = current;
                current = next;
            }
            let lengths =
                huffman_lengths(&freqs, 15).unwrap_or_else(|| panic!("{symbols} 个符号应当能限长"));
            assert!(
                lengths.iter().all(|length| *length <= 15),
                "{symbols} 个符号出现超长码：{lengths:?}"
            );
            assert_eq!(
                kraft_units(&lengths, 15),
                1u64 << 15,
                "{symbols} 个符号的码长集合不是满码（超订则解码端拒收）"
            );
            for (symbol, freq) in freqs.iter().enumerate() {
                if *freq > 0 {
                    assert!(lengths[symbol] > 0, "符号 {symbol} 缺失码长");
                }
            }
        }
        // 单符号表允许不完整（1 位码 → Kraft 和 = 1/2）。
        assert_eq!(
            kraft_units(&huffman_lengths(&[0, 7, 0], 15).unwrap(), 15),
            1 << 14
        );
    }

    /// 码长表 RLE：展开后必须与原序列逐项一致，且长 0 段要真被压掉。
    #[test]
    fn rle_code_lengths_round_trips_and_shrinks_zero_runs() {
        let mut lengths = vec![0u8; 40];
        lengths.extend_from_slice(&[5, 5, 5, 5, 5, 5, 5]);
        lengths.extend_from_slice(&[0, 0]);
        lengths.push(7);
        lengths.extend(std::iter::repeat(3u8).take(10));

        let runs = rle_code_lengths(&lengths);
        assert_eq!(expand_length_runs(&runs), lengths);

        // 50 + 13 项码长压到个位数符号（18 一次吃掉 40 个 0）。
        assert!(runs.len() <= 12, "RLE 后仍有 {} 个符号", runs.len());
        assert!(
            runs.iter().any(|run| run.symbol == 18),
            "长 0 段应使用 18 号重复码"
        );
        assert!(
            runs.iter().any(|run| run.symbol == 16),
            "非 0 重复应使用 16 号重复码"
        );
    }

    /// 按 RFC 1951 展开码长 RLE（测试自用，用于验证编码端）。
    fn expand_length_runs(runs: &[LengthRun]) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        for run in runs {
            match run.symbol {
                16 => {
                    let previous = *out.last().expect("16 号码前面必须有码长");
                    for _ in 0..run.extra as usize + 3 {
                        out.push(previous);
                    }
                }
                17 => {
                    for _ in 0..run.extra as usize + 3 {
                        out.push(0);
                    }
                }
                18 => {
                    for _ in 0..run.extra as usize + 11 {
                        out.push(0);
                    }
                }
                value => out.push(value),
            }
        }
        out
    }

    #[test]
    fn deflate_falls_back_to_stored_for_incompressible_input() {
        let data = pseudo_random_bytes(2048, 12345);
        let compressed = deflate(&data);
        // 回退存储块：首字节 BFINAL=1 / BTYPE=00。
        assert_eq!(compressed[0] & 0b111, 0b001, "不可压缩数据应回退存储块");
        assert_eq!(inflate(&compressed).unwrap(), data);
        assert!(
            compressed.len() < data.len() + 16,
            "存储块开销应接近 5 字节：{} vs {}",
            compressed.len(),
            data.len()
        );
        // 压不小的时候 ZIP 侧应选择 stored。
        assert!(deflate_if_smaller(&data).is_none());
    }

    #[test]
    fn deflate_compresses_sql_like_text() {
        let data = sql_like_text(400);
        let compressed = deflate(&data);
        let ratio = compressed.len() as f64 / data.len() as f64;
        assert!(
            ratio < 0.25,
            "重复 SQL 文本压缩比应低于 25%，实测 {:.1}%（{} -> {}）",
            ratio * 100.0,
            data.len(),
            compressed.len()
        );
    }

    #[test]
    fn zip_writer_uses_deflate_for_compressible_entries() {
        let data = sql_like_text(100);
        let mut writer = ZipWriter::new();
        writer.add_file("database.sql", &data).unwrap();
        writer.add_dir("template").unwrap();
        let bytes = writer.finish();

        let archive = ZipArchive::open(bytes).expect("open");
        let entry = archive.find("database.sql").expect("entry");
        assert_eq!(entry.method, 8, "文件条目应使用 deflate");
        assert!(entry.compressed_size < entry.uncompressed_size, "{entry:?}");
        assert_eq!(archive.read_by_name("database.sql").unwrap(), data);
        let dir = archive.find("template/").expect("dir");
        assert_eq!(dir.method, 0, "目录条目保持 stored");
    }

    #[test]
    fn zip_writer_keeps_stored_for_tiny_or_binary_entries() {
        let mut writer = ZipWriter::new();
        writer.add_file("a.txt", b"abc").unwrap();
        writer
            .add_file("blob.bin", &pseudo_random_bytes(1024, 4242))
            .unwrap();
        let bytes = writer.finish();
        let archive = ZipArchive::open(bytes).expect("open");
        assert_eq!(archive.find("a.txt").unwrap().method, 0);
        assert_eq!(archive.find("blob.bin").unwrap().method, 0);
        assert_eq!(archive.read_by_name("a.txt").unwrap(), b"abc".to_vec());
    }

    #[test]
    fn zip_reads_external_deflate_entry() {
        // 外部压缩器（动态 Huffman）产出的 deflate 数据，
        // 按 method=8 装进 ZIP，验证这类外部产出的压缩包能被正常读取。
        let content = DEFLATE_SAMPLE_TEXT.repeat(40);
        let payload = decode_base64(DEFLATE_SAMPLE_B64);
        assert!(payload.len() < content.len());
        let bytes = build_zip("hello.txt", content.as_bytes(), 8, &payload);
        let archive = ZipArchive::open(bytes).expect("open");
        assert_eq!(archive.read_by_name("hello.txt").unwrap(), content.as_bytes());
    }
}

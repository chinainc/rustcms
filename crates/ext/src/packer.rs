//! ZIP 打包 / 解包与 DEFLATE 解码。
//!
//! 对齐 Go 版升级包与模板包处理（zip / tar.gz 分派、模板打包下载、
//! 模板包上传解压）。
//!
//! 实现说明：**零新依赖**。
//!
//! * ZIP 读取支持 `stored` 与 `deflate`；
//! * DEFLATE（RFC 1951）解码器手写实现；
//! * ZIP 写入只产出 `stored`（不压缩），产物仍是合法 ZIP；
//! * `tar.gz` 读取 = gzip 头 + DEFLATE 解码 + tar 块解析；
//! * 解压带 Zip-Slip 防护（拒绝上级目录与绝对路径）。

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
    for length in 1..16 {
        left <<= 1;
        left -= counts[length] as i32;
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
                for _ in 0..repeat {
                    lengths.push(0);
                }
            }
            18 => {
                let repeat = 11 + reader.bits(7)? as usize;
                for _ in 0..repeat {
                    lengths.push(0);
                }
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
// ZIP 写入（stored 方法）
// ---------------------------------------------------------------------------

/// 中央目录条目。
#[derive(Debug)]
struct CentralEntry {
    name: Vec<u8>,
    crc: u32,
    size: u64,
    offset: u32,
    is_dir: bool,
}

/// ZIP 写入器（只产出 `stored` 条目）。
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

        // 本地文件头
        self.buffer.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        self.buffer.extend_from_slice(&20u16.to_le_bytes()); // 解压所需版本
        self.buffer.extend_from_slice(&0x0800u16.to_le_bytes()); // 通用标志位：名称 UTF-8
        self.buffer.extend_from_slice(&0u16.to_le_bytes()); // 压缩方法：stored
        self.buffer.extend_from_slice(&0u16.to_le_bytes()); // 修改时间
        self.buffer.extend_from_slice(&0u16.to_le_bytes()); // 修改日期
        self.buffer.extend_from_slice(&crc.to_le_bytes());
        self.buffer.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.buffer.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.buffer
            .extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        self.buffer.extend_from_slice(&0u16.to_le_bytes()); // 扩展字段长度
        self.buffer.extend_from_slice(&name_bytes);
        self.buffer.extend_from_slice(data);

        self.entries.push(CentralEntry {
            name: name_bytes,
            crc,
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
                self.buffer.extend_from_slice(&0u16.to_le_bytes());
                self.buffer.extend_from_slice(&0u16.to_le_bytes());
                self.buffer.extend_from_slice(&0u16.to_le_bytes());
                self.buffer.extend_from_slice(&entry.crc.to_le_bytes());
                self.buffer
                    .extend_from_slice(&(entry.size as u32).to_le_bytes());
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
            b'0' | 0 => entries.push(TarEntry {
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

/// 把目录打包成 ZIP（`stored` 方法）。
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
        assert_eq!(entries[0].data, b"hello tar world\n".to_vec());
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
}

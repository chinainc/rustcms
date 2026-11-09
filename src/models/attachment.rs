//! 附件（表 `attachments`）。
//!
//! 行为约定：
//! - 列表按 `status = 1`、`updated_time DESC`；
//! - 上传按 MD5 去重（`file_md5` 是唯一索引），同一份文件只存一条；
//! - 落盘路径为 `uploads/YYYYMM/DD/<文件名>`。

use md5::{Digest, Md5};
use serde::Serialize;
use sqlx::mysql::{MySql, MySqlPool};
use sqlx::{FromRow, QueryBuilder};

use super::ModelError;

/// 附件。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Attachment {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_id: u32,
    pub file_name: String,
    pub file_location: String,
    pub file_size: u64,
    pub file_md5: String,
    pub width: u32,
    pub height: u32,
    pub category_id: u32,
    /// 1 = 图片，2 = 视频，其它未定义
    pub is_image: i8,
    pub status: u8,
    pub watermark: i8,
    pub is_remote: i8,
    pub logo: String,
}

/// 列表筛选条件。
#[derive(Debug, Default, Clone)]
pub struct AttachmentFilter {
    /// 分类（0 表示不限）
    pub category_id: u32,
    /// 文件名模糊搜索
    pub keyword: String,
    /// 1 = 只图片，2 = 只视频，其它不限
    pub is_image: i32,
}

/// 计算 MD5（小写十六进制）。
pub fn md5_hex(bytes: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// 从文件名里取扩展名（小写、带点）；无扩展名返回空串。
pub fn extension_of(file_name: &str) -> String {
    let ext = file_name
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match ext.as_str() {
        "" => String::new(),
        "jpeg" => ".jpg".to_string(),
        _ => format!(".{ext}"),
    }
}

/// 扩展名是否可接受（遵循既有约定：拒绝 php / jsp），并补齐常见脚本/可执行扩展名。
pub fn is_allowed_ext(ext: &str) -> bool {
    !matches!(
        ext,
        ".php" | ".php3" | ".php4" | ".php5" | ".phtml" | ".jsp" | ".asp" | ".aspx" | ".cgi"
            | ".pl" | ".exe" | ".dll" | ".sh" | ".bat" | ".cmd" | ".com" | ".js" | ".html"
            | ".htm"
    )
}

/// 图片扩展名 → `is_image = 1`。
pub fn is_image_ext(ext: &str) -> bool {
    matches!(ext, ".jpg" | ".png" | ".gif" | ".webp" | ".avif" | ".tiff" | ".bmp" | ".svg")
}

/// 视频扩展名 → `is_image = 2`。
pub fn is_video_ext(ext: &str) -> bool {
    matches!(
        ext,
        ".mp4" | ".mov" | ".avi" | ".mkv" | ".webm" | ".flv" | ".wmv" | ".m4v" | ".mpeg" | ".mpg"
    )
}

/// 从文件头解析图片宽高（PNG / GIF / JPEG / WebP），解析不出返回 `(0, 0)`。
pub fn image_size(bytes: &[u8]) -> (u32, u32) {
    if bytes.len() < 10 {
        return (0, 0);
    }

    // PNG: 89 50 4E 47 0D 0A 1A 0A，IHDR 宽高在大端 16..24
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) && bytes.len() >= 24 {
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return (width, height);
    }

    // GIF: GIF87a / GIF89a，宽高在小端 6..10
    if (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) && bytes.len() >= 10 {
        let width = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
        let height = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
        return (width, height);
    }

    // WebP: RIFF....WEBP
    if bytes.starts_with(b"RIFF") && bytes.len() > 30 && &bytes[8..12] == b"WEBP" {
        match &bytes[12..16] {
            b"VP8 " => {
                let width = (u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3FFF) as u32;
                let height = (u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3FFF) as u32;
                return (width, height);
            }
            b"VP8L" => {
                let b = &bytes[21..25];
                let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                let width = (bits & 0x3FFF) + 1;
                let height = ((bits >> 14) & 0x3FFF) + 1;
                return (width, height);
            }
            b"VP8X" => {
                let width = (bytes[24] as u32 | (bytes[25] as u32) << 8 | (bytes[26] as u32) << 16) + 1;
                let height =
                    (bytes[27] as u32 | (bytes[28] as u32) << 8 | (bytes[29] as u32) << 16) + 1;
                return (width, height);
            }
            _ => {}
        }
    }

    // JPEG: FF D8，扫 SOF 段拿宽高
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut index = 2usize;
        while index + 9 < bytes.len() {
            if bytes[index] != 0xFF {
                index += 1;
                continue;
            }
            let marker = bytes[index + 1];
            // SOF0..SOF3 / SOF5..SOF7 / SOF9..SOF11 / SOF13..SOF15
            if matches!(
                marker,
                0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
            ) {
                let height = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]) as u32;
                let width = u16::from_be_bytes([bytes[index + 7], bytes[index + 8]]) as u32;
                return (width, height);
            }
            let length = u16::from_be_bytes([bytes[index + 2], bytes[index + 3]]) as usize;
            if length < 2 {
                break;
            }
            index += 2 + length;
        }
    }

    (0, 0)
}

fn push_filter(builder: &mut QueryBuilder<'_, MySql>, filter: &AttachmentFilter) {
    builder.push(" AND `status` = 1");
    if filter.category_id > 0 {
        builder.push(" AND `category_id` = ").push_bind(filter.category_id);
    }
    if filter.is_image > 0 {
        builder.push(" AND `is_image` = ").push_bind(filter.is_image);
    }
    let keyword = filter.keyword.trim();
    if !keyword.is_empty() {
        builder
            .push(" AND `file_name` LIKE ")
            .push_bind(format!("%{keyword}%"));
    }
}

/// 分页查附件，返回 `(总数, 本页记录)`。
pub async fn list_page(
    pool: &MySqlPool,
    filter: &AttachmentFilter,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Attachment>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;

    let mut counter =
        QueryBuilder::<MySql>::new("SELECT COUNT(*) FROM attachments WHERE 1 = 1");
    push_filter(&mut counter, filter);
    let total: i64 = counter.build_query_scalar().fetch_one(pool).await?;

    let mut query = QueryBuilder::<MySql>::new("SELECT * FROM attachments WHERE 1 = 1");
    push_filter(&mut query, filter);
    query
        .push(" ORDER BY updated_time DESC, id DESC LIMIT ")
        .push_bind(page_size)
        .push(" OFFSET ")
        .push_bind(offset);
    let rows = query.build_query_as::<Attachment>().fetch_all(pool).await?;

    Ok((total, rows))
}

/// 按 ID 取附件。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Attachment>> {
    Ok(
        sqlx::query_as::<_, Attachment>("SELECT * FROM attachments WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 按 MD5 取附件（上传去重用）。
pub async fn get_by_md5(pool: &MySqlPool, md5: &str) -> anyhow::Result<Option<Attachment>> {
    Ok(
        sqlx::query_as::<_, Attachment>("SELECT * FROM attachments WHERE file_md5 = ? LIMIT 1")
            .bind(md5)
            .fetch_optional(pool)
            .await?,
    )
}

/// 新建附件记录，返回自增 ID。
#[allow(clippy::too_many_arguments)]
pub async fn insert(
    pool: &MySqlPool,
    file_name: &str,
    file_location: &str,
    file_size: u64,
    file_md5: &str,
    width: u32,
    height: u32,
    category_id: u32,
    is_image: i8,
    logo: &str,
) -> Result<u32, ModelError> {
    let now = chrono::Utc::now().timestamp();
    let result = sqlx::query(
        "INSERT INTO attachments (created_time, updated_time, user_id, file_name, file_location, \
            file_size, file_md5, width, height, category_id, is_image, status, watermark, \
            is_remote, logo) \
         VALUES (?, ?, 0, ?, ?, ?, ?, ?, ?, ?, ?, 1, 0, 0, ?)",
    )
    .bind(now)
    .bind(now)
    .bind(file_name.chars().take(250).collect::<String>())
    .bind(file_location)
    .bind(file_size)
    .bind(file_md5)
    .bind(width)
    .bind(height)
    .bind(category_id)
    .bind(is_image)
    .bind(logo)
    .execute(pool)
    .await?;
    Ok(result.last_insert_id() as u32)
}

/// 覆盖已有附件记录（替换上传 / 改名 / 改路径）。
pub async fn update(
    pool: &MySqlPool,
    id: u32,
    file_name: Option<&str>,
    file_location: Option<&str>,
    file_size: Option<u64>,
    file_md5: Option<&str>,
    width: Option<u32>,
    height: Option<u32>,
    logo: Option<&str>,
) -> Result<(), ModelError> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "UPDATE attachments SET updated_time = ?, \
            file_name = COALESCE(?, file_name), \
            file_location = COALESCE(?, file_location), \
            file_size = COALESCE(?, file_size), \
            file_md5 = COALESCE(?, file_md5), \
            width = COALESCE(?, width), \
            height = COALESCE(?, height), \
            logo = COALESCE(?, logo) \
         WHERE id = ?",
    )
    .bind(now)
    .bind(file_name.map(|v| v.chars().take(250).collect::<String>()))
    .bind(file_location)
    .bind(file_size)
    .bind(file_md5)
    .bind(width)
    .bind(height)
    .bind(logo)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 删除附件记录，返回是否真的删掉。
pub async fn delete(pool: &MySqlPool, id: u32) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM attachments WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// 批量改分类，返回受影响行数。
pub async fn change_category(pool: &MySqlPool, category_id: u32, ids: &[u32]) -> anyhow::Result<u64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let mut builder = QueryBuilder::<MySql>::new("UPDATE attachments SET category_id = ");
    builder.push_bind(category_id);
    builder.push(" WHERE id IN (");
    let mut list = builder.separated(", ");
    for id in ids {
        list.push_bind(*id);
    }
    list.push_unseparated(")");
    let result = builder.build().execute(pool).await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn computes_md5() {
        // 已知向量
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn normalizes_extension() {
        assert_eq!(extension_of("a.JPEG"), ".jpg");
        assert_eq!(extension_of("a.PNG"), ".png");
        assert_eq!(extension_of("noext"), "");
        assert!(is_allowed_ext(".png"));
        assert!(!is_allowed_ext(".php"));
        assert!(!is_allowed_ext(".jsp"));
        assert!(is_image_ext(".webp"));
        assert!(is_video_ext(".mp4"));
    }

    #[test]
    fn parses_image_sizes() {
        // 1x1 PNG
        let png: &[u8] = &[
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
            0, 0, 0, 1, 0, 0, 0, 1,
        ];
        assert_eq!(image_size(png), (1, 1));

        // GIF 2x3
        let gif: &[u8] = b"GIF89a\x02\x00\x03\x00\x00\x00\x00";
        assert_eq!(image_size(gif).0, 2);
        assert_eq!(image_size(gif).1, 3);

        // 未知
        assert_eq!(image_size(b"not an image at all"), (0, 0));
    }

    #[tokio::test]
    async fn reads_real_attachments() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let (total, list) = list_page(&pool, &AttachmentFilter::default(), 1, 5)
            .await
            .unwrap();
        assert!(total >= 0);
        assert!(list.len() as i64 <= 5);
        if let Some(first) = list.first() {
            let found = get(&pool, first.id).await.unwrap().expect("附件应存在");
            assert_eq!(found.file_md5, first.file_md5);
        }
        assert!(get(&pool, u32::MAX).await.unwrap().is_none());
    }
}

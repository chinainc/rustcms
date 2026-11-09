//! 数据模型层。
//!
//! 表名固定为 `categories` / `archives` / `archive_data` / `modules` …，
//! 既有数据库可以直接复用。
//!
//! 数据层先于调用方落地，部分能力（封面/看图/浏览量自增/按 token 查分类等）
//! 目前只被测试覆盖，先关闭 `dead_code` 告警，等前台与后台接上后自然消除。
#![allow(dead_code)]

pub mod admin;
pub mod anchor;
pub mod cloud;
pub mod archive;
pub mod archive_admin;
pub mod attachment;
pub mod backup;
pub mod archive_write;
pub mod category;
pub mod collector;
pub mod comment;
pub mod commission;
pub mod design;
pub mod diy;
pub mod extra;
pub mod guestbook;
pub mod link;
pub mod llms;
pub mod material;
pub mod menu;
pub mod module;
pub mod multilang;
pub mod nav;
pub mod order;
pub mod payment;
pub mod place;
pub mod plugin;
pub mod redirect;
pub mod replace;
pub mod retailer;
pub mod sensitive;
pub mod sendmail;
pub mod setting;
pub mod site_setting;
pub mod skill;
pub mod statistic;
pub mod subscriber;
pub mod tag;
pub mod titleimage;
pub mod transfer;
pub mod translate;
pub mod upload_file;
pub mod url;
pub mod user;
pub mod user_admin;
pub mod user_auth;
pub mod watermark;
pub mod wechat;
pub mod website;
pub mod withdraw;

pub mod admin_gap;
pub mod front_content;
pub mod interact;
pub mod member;
pub mod trade;

pub use category::Category;
pub use module::Module;

use serde::ser::SerializeSeq;
use serde::{Serialize, Serializer};

/// 数据层错误：业务校验失败 vs 数据库/系统错误。
///
/// 业务错误的中文文案直接回给前端（对应既有约定的 `ctx.Tr("...")`）。
#[derive(Debug)]
pub enum ModelError {
    Invalid(String),
    Db(anyhow::Error),
}

impl ModelError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }

    /// 是否是业务校验错误（前端可直接展示）。
    pub fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid(_))
    }
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(msg) => write!(f, "{msg}"),
            Self::Db(err) => write!(f, "{err:#}"),
        }
    }
}

impl std::error::Error for ModelError {}

impl From<sqlx::Error> for ModelError {
    fn from(err: sqlx::Error) -> Self {
        Self::Db(err.into())
    }
}

impl From<anyhow::Error> for ModelError {
    fn from(err: anyhow::Error) -> Self {
        Self::Db(err)
    }
}

impl From<serde_json::Error> for ModelError {
    fn from(err: serde_json::Error) -> Self {
        Self::Db(err.into())
    }
}

/// 分页结果，字段命名与既有实现 API 保持一致。
#[derive(Debug, Clone, Serialize)]
pub struct Page<T> {
    pub list: Vec<T>,
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
    pub total_page: u32,
}

impl<T> Page<T> {
    pub fn new(list: Vec<T>, total: i64, page: u32, page_size: u32) -> Self {
        let page_size = if page_size == 0 { 20 } else { page_size };
        let page = page.max(1);
        let total_page = if total <= 0 {
            0
        } else {
            ((total as f64) / (page_size as f64)).ceil() as u32
        };
        Self {
            list,
            total,
            page,
            page_size,
            total_page,
        }
    }

    /// 空结果。
    pub fn empty(page: u32, page_size: u32) -> Self {
        Self::new(Vec::new(), 0, page, page_size)
    }
}

/// 把库里的「字符串列表」列解析成 `Vec<String>`。
///
/// `images` 是 TEXT 列，历史数据存在这几种形态：
/// 空串、`{}`、`{"a"}`、`{"a","b"}`、`["a","b"]`。
/// 前两种当空列表，其余把花括号归一成方括号后按 JSON 解析，失败再退回逐项剥引号。
pub fn json_string_list(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    if raw == "{}" || raw == "[]" {
        return Vec::new();
    }

    let normalized = raw.replace('{', "[").replace('}', "]");
    if let Ok(list) = serde_json::from_str::<Vec<String>>(&normalized) {
        return list;
    }

    // 兜底：去掉首尾括号后按逗号切，逐项剥掉引号与空白。
    normalized
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|item| item.trim().trim_matches('"').trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// `images` 列序列化成字符串数组（库里是文本，既有实现 API 输出数组）。
pub(crate) fn serialize_images<S: Serializer>(
    images: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let list = json_string_list(images.as_deref());
    let mut seq = serializer.serialize_seq(Some(list.len()))?;
    for item in &list {
        seq.serialize_element(item)?;
    }
    seq.end()
}

/// 测试辅助：串行化所有依赖真实库的用例。
///
/// 写用例会新增/删除行，读用例断言总数时就可能被插队，导致「偶发」失败。
/// 同一个测试二进制内用一把锁把它们排开（`cargo test` 默认多线程并行）。
/// 用 tokio 的异步锁：`#[tokio::test]` 里可以安全地跨 await 持有。
#[cfg(test)]
pub(crate) async fn db_lock() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    LOCK.lock().await
}

/// 测试辅助：真实模板目录（仓库根的 `template/default`）。
///
/// 仓库根就是 crate 根；同样兼容 crate 曾经位于下级目录的历史布局。
#[cfg(test)]
pub(crate) fn test_template_dir() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let local = manifest.join("template/default");
    if local.is_dir() {
        local
    } else {
        manifest.join("../template/default")
    }
}

/// 测试辅助：连接测试配置文件里配置的数据库。
///
/// **必须显式用 `RUSTCMS_TEST_CONFIG` 指定配置文件才会连库**（例如
/// `RUSTCMS_TEST_CONFIG=tmp/test-config.json cargo test`）：连库用例会真的写库
/// （建/删测试归档、改设置项），默认不自动读仓库根的 `config.json`，免得
/// `cargo test` 动到开发库里的真实数据（那批用例又依赖线上配置，例如公众号
/// AppID、插件密钥，缺了会红）。
///
/// 没指定 / 连不上 / 没配 mysql 时返回 `None`，调用方直接跳过用例，
/// 这样在没有数据库的机器上 `cargo test` 也不会红（约定见 `docs/coverage-plan.md`）。
#[cfg(test)]
pub(crate) async fn test_pool() -> Option<sqlx::MySqlPool> {
    let path = std::env::var_os("RUSTCMS_TEST_CONFIG").map(std::path::PathBuf::from)?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let cfg: crate::config::ServerJson = serde_json::from_str(&raw).ok()?;
    if !cfg.mysql.is_configured() {
        return None;
    }
    match crate::db::connect(&cfg.mysql).await {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("跳过用例：{err:#}");
            None
        }
    }
}

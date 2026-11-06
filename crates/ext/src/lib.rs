//! rustcms 的外部能力层。
//!
//! 这里集中放「需要访问进程外部世界」的能力，与业务模型层分开：
//!
//! - [`http`]：HTTP 客户端（curl 子进程 + 标准库兜底两个后端）；
//! - [`smtp`]：SMTP 发信（明文会话 + 走 curl 的 TLS 后端）；
//! - [`packer`]：ZIP / tar.gz 打包解包（纯标准库实现 inflate）；
//! - [`ratelimit`]：访问频率限制与 IP 黑名单；
//! - [`tblsync`]：数据库表结构自动迁移（对齐 GORM AutoMigrate）。
//!
//! 设计约束：这一层**不引入任何新依赖**，只用主 crate 已经编译过的
//! `tokio` / `serde` / `sqlx` / `anyhow` / `chrono` / `tracing`，
//! 以便在低内存机器上也能单独编译与验证。
//!
//! 模块文件名说明：`packer` 与 `tblsync` 分别承载「打包解压」与「表结构同步」，
//! 语义与原计划中的 `zip` / `migrate` 一致。

pub mod http;
pub mod packer;
pub mod ratelimit;
pub mod smtp;
pub mod tblsync;
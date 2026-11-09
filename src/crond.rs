//! 进程内定时任务（只承担可在本地执行的周期任务）。
//!
//! 当前实现：
//! - 每分钟：发布到期的定时发布计划（`archive_drafts.status = 2`）；
//! - 每天：清理 30 天前移入回收站的内容。
//!
//! 依赖外部服务的任务（采集、AI 写作、统计上报、提现打款等）仍由各自接口按需触发。

use std::time::Duration;

use crate::state::AppState;

/// 启动后台调度器（随 HTTP 服务一起运行）。
pub fn start(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        let mut last_cleanup = chrono::Utc::now().timestamp();

        loop {
            ticker.tick().await;
            let Some(pool) = state.db().cloned() else {
                continue;
            };

            match crate::models::archive_admin::publish_planned(&pool).await {
                Ok(count) if count > 0 => tracing::info!("定时发布：已发布 {count} 篇文档"),
                Ok(_) => {}
                Err(err) => tracing::warn!("定时发布失败：{err:#}"),
            }

            let now = chrono::Utc::now().timestamp();
            if now - last_cleanup >= 24 * 3600 {
                last_cleanup = now;
                match crate::models::archive_admin::clean_recycle_bin(&pool).await {
                    Ok(count) if count > 0 => tracing::info!("回收站清理：已删除 {count} 篇文档"),
                    Ok(_) => {}
                    Err(err) => tracing::warn!("回收站清理失败：{err:#}"),
                }
            }
        }
    });
}
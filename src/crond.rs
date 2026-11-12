//! 进程内定时任务（只承担可在本地执行的周期任务）。
//!
//! 当前实现：
//! - 每分钟：发布到期的定时发布计划（`archive_drafts.status = 2`）；
//! - 每 10 分钟：把访问明细日志聚合成 `statistic_logs`（蜘蛛分析/流量分析的数据源，
//!   口径同 anqicms `calcStatistics`）；
//! - 每天 8 点后：采集搜索引擎收录量写 `spider_includes`（口径同 anqicms
//!   `daily8HourTask`），顺带清理 31 天前的明细日志；
//! - 每天：清理 30 天前移入回收站的内容。
//!
//! 其余依赖外部服务的任务（AI 写作、提现打款等）仍由各自接口按需触发。

use std::time::Duration;

use chrono::Timelike;
use crate::state::AppState;

/// 启动后台调度器（随 HTTP 服务一起运行）。
pub fn start(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        let mut last_cleanup = chrono::Utc::now().timestamp();
        let mut tick: u32 = 0;
        let mut last_include_day = String::new();
        let stat_dir = state.with_config(|config| config.data_dir()).join("statistic");

        // 启动即补一轮：目录、已有明细日志的聚合、当天收录量。
        // 否则后台数据页要等 10 分钟（聚合）或 8 点（收录）才有数据。
        crate::models::statistic::ensure_dir(&stat_dir);
        if let Some(pool) = state.db().cloned() {
            if let Err(err) = crate::models::statistic::calc_statistics(&pool, &stat_dir).await {
                tracing::warn!("访问统计聚合失败：{err:#}");
            }
            match crate::models::statistic::ensure_daily_include(&pool).await {
                Ok(true) => tracing::info!("已采集当天搜索引擎收录量"),
                Ok(false) => {}
                Err(err) => tracing::warn!("搜索引擎收录量采集失败：{err:#}"),
            }
        }

        loop {
            ticker.tick().await;
            tick += 1;
            let Some(pool) = state.db().cloned() else {
                continue;
            };

            match crate::models::archive_admin::publish_planned(&pool).await {
                Ok(count) if count > 0 => tracing::info!("定时发布：已发布 {count} 篇文档"),
                Ok(_) => {}
                Err(err) => tracing::warn!("定时发布失败：{err:#}"),
            }

            // 每 10 分钟：聚合访问日志（同 anqicms 每 10 分钟一次的 calcStatistics）
            if tick % 10 == 0 {
                crate::models::statistic::ensure_dir(&stat_dir);
                if let Err(err) = crate::models::statistic::calc_statistics(&pool, &stat_dir).await {
                    tracing::warn!("访问统计聚合失败：{err:#}");
                }
            }

            // 每天 8 点后第一个 tick：采集收录量 + 清理旧明细日志（服务 8 点后才启动也能补跑）
            let local = chrono::Local::now();
            let day = local.format("%Y-%m-%d").to_string();
            if local.hour() >= 8 && last_include_day != day {
                last_include_day = day;
                match crate::models::statistic::ensure_daily_include(&pool).await {
                    Ok(true) => tracing::info!("已采集当天搜索引擎收录量"),
                    Ok(false) => {}
                    Err(err) => tracing::warn!("搜索引擎收录量采集失败：{err:#}"),
                }
                crate::models::statistic::clean_old_logs(&stat_dir);
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

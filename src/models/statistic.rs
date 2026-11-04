//! 后台统计与版本信息。
//!
//! 对应 Go 版 `manageController/common.go` 的 `Version` /
//! `GetStatisticsSummary` / `GetStatisticsDashboard`（实体逻辑在
//! `provider/statistics.go`）。

use serde::Serialize;
use serde_json::Value;
use sqlx::mysql::MySqlPool;

/// 当前程序版本号（暂等于 crate 版本）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// 是否试用版（与 Go 的 `config.Trial` 对齐，当前恒为 false）。
pub const TRIAL: bool = false;

/// `GET /system/api/version/info` 的 data。
#[derive(Debug, Clone, Serialize)]
pub struct VersionInfo {
    pub version: &'static str,
    pub trial: bool,
}

pub fn version_info() -> VersionInfo {
    VersionInfo {
        version: VERSION,
        trial: TRIAL,
    }
}

/// 单个模型的内容数。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ModuleCount {
    pub id: u32,
    pub name: String,
    pub total: i64,
}

/// 内容数量统计。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ArchiveCount {
    pub total: i64,
    pub last_week: i64,
    pub un_release: i64,
    pub draft: i64,
    pub today: i64,
}

/// 「总量 / 今日」型统计（访问、蜘蛛）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SplitCount {
    pub total: i64,
    pub today: i64,
}

/// 统计摘要（字段与 `response.Statistics` 一致）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct Statistics {
    pub show_guide: bool,
    pub archive_counts: Vec<ModuleCount>,
    pub archive_count: ArchiveCount,
    pub category_count: i64,
    pub link_count: i64,
    pub guestbook_count: i64,
    pub traffic_count: SplitCount,
    pub spider_count: SplitCount,
    pub template_count: i64,
    pub page_count: i64,
    pub attachment_count: i64,
    pub exact: bool,
}

/// 当天 00:00 的时间戳（本地时区）。
fn start_of_today() -> i64 {
    use chrono::{Local, TimeZone};
    let now = Local::now();
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).single())
        .map(|dt| dt.timestamp())
        .unwrap_or_else(|| now.timestamp())
}

/// 本周一 00:00 的时间戳（本地时区）。
fn start_of_week() -> i64 {
    use chrono::{Datelike, Local};
    let today = start_of_today();
    let weekday = Local::now().weekday().num_days_from_monday() as i64;
    today - weekday * 86_400
}

/// 取 `visit_count` / `spider_count` JSON 的数值总和。
fn json_total(raw: Option<&str>, single_key: Option<&str>) -> i64 {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return 0;
    };
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return 0;
    };
    if let Some(key) = single_key {
        if let Some(number) = value.get(key).and_then(Value::as_i64) {
            return number;
        }
    }
    match value {
        Value::Object(map) => map.values().filter_map(Value::as_i64).sum(),
        Value::Number(number) => number.as_i64().unwrap_or(0),
        _ => 0,
    }
}

/// 统计摘要（对齐 `GetStatisticsSummary`）。
pub async fn summary(pool: &MySqlPool, exact: bool) -> anyhow::Result<Statistics> {
    let mut result = Statistics {
        exact,
        ..Default::default()
    };

    let modules = super::module::list(pool).await?;
    for module in &modules {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM archives WHERE module_id = ?")
            .bind(module.id)
            .fetch_one(pool)
            .await?;
        result.archive_counts.push(ModuleCount {
            id: module.id,
            name: module.title.clone(),
            total,
        });
        result.archive_count.total += total;
    }

    let today = start_of_today();
    let week = start_of_week();
    let now = chrono::Local::now().timestamp();

    result.archive_count.today =
        sqlx::query_scalar("SELECT COUNT(*) FROM archives WHERE created_time >= ?")
            .bind(today)
            .fetch_one(pool)
            .await?;
    result.archive_count.last_week = sqlx::query_scalar(
        "SELECT COUNT(*) FROM archives WHERE created_time >= ? AND created_time < ?",
    )
    .bind(week - 7 * 86_400)
    .bind(week)
    .fetch_one(pool)
    .await?;
    result.archive_count.un_release =
        sqlx::query_scalar("SELECT COUNT(*) FROM archive_drafts WHERE created_time > ?")
            .bind(now)
            .fetch_one(pool)
            .await?;
    result.archive_count.draft =
        sqlx::query_scalar("SELECT COUNT(*) FROM archive_drafts WHERE status = 0")
            .fetch_one(pool)
            .await?;

    result.category_count =
        sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE `type` != 3")
            .fetch_one(pool)
            .await?;
    result.page_count = sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE `type` = 3")
        .fetch_one(pool)
        .await?;
    result.link_count = sqlx::query_scalar("SELECT COUNT(*) FROM links")
        .fetch_one(pool)
        .await?;
    result.guestbook_count = sqlx::query_scalar("SELECT COUNT(*) FROM guestbooks")
        .fetch_one(pool)
        .await?;
    result.attachment_count = sqlx::query_scalar("SELECT COUNT(*) FROM attachments")
        .fetch_one(pool)
        .await?;

    // 访问 / 蜘蛛：最近 7 天的 statistic_logs
    let rows: Vec<(Option<i64>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT created_time, visit_count, spider_count FROM statistic_logs \
         WHERE created_time >= ? ORDER BY created_time ASC",
    )
    .bind(today - 7 * 86_400)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let last_index = rows.len().saturating_sub(1);
    for (index, (created_time, visit, spider)) in rows.iter().enumerate() {
        let pv = json_total(visit.as_deref(), Some("pv_count"));
        result.traffic_count.total += pv;
        result.spider_count.total += json_total(spider.as_deref(), None);
        if index == last_index && *created_time == Some(today) {
            result.traffic_count.today = pv;
        }
    }

    result.show_guide = result.category_count == 0 || result.archive_count.total == 0;
    Ok(result)
}

/// 仪表盘数据（对齐 `GetStatisticsDashboard`）。
pub async fn dashboard(pool: &MySqlPool) -> anyhow::Result<Value> {
    let logs: Vec<(Option<i64>, String)> =
        sqlx::query_as("SELECT created_time, ip FROM admin_login_logs ORDER BY id DESC LIMIT 2")
            .fetch_all(pool)
            .await
            .unwrap_or_default();

    let mut result = serde_json::Map::new();
    if logs.len() == 2 {
        result.insert(
            "last_login".into(),
            serde_json::json!({"created_time": logs[1].0, "ip": logs[1].1}),
        );
    }
    if let Some(first) = logs.first() {
        result.insert(
            "now_login".into(),
            serde_json::json!({"created_time": first.0, "ip": first.1}),
        );
    }

    let system = super::setting::get_json_or_empty(pool, "system").await?;
    result.insert("system".into(), system);
    result.insert("version".into(), Value::String(VERSION.to_string()));
    result.insert("trial".into(), Value::Bool(TRIAL));
    result.insert("memory_usage".into(), Value::from(0));

    Ok(Value::Object(result))
}

/// 图表数据点。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ChartData {
    pub date: String,
    pub label: String,
    pub value: i64,
}

/// 搜索引擎收录快照（`spider_includes`）。
#[derive(Debug, Clone, Default, sqlx::FromRow, Serialize)]
pub struct SpiderInclude {
    pub id: u32,
    pub created_time: Option<i64>,
    pub baidu_count: i32,
    pub sogou_count: i32,
    pub so_count: i32,
    pub bing_count: i32,
    pub google_count: i32,
}

fn date_of(timestamp: i64, format: &str) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|dt| dt.format(format).to_string())
        .unwrap_or_default()
}

/// 蜘蛛爬行图表（最近 30 天，按天 × 蜘蛛拆分）。
pub async fn chart_spider(pool: &MySqlPool) -> anyhow::Result<Vec<ChartData>> {
    let since = start_of_today() - 30 * 86_400;
    let rows: Vec<(Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT created_time, spider_count FROM statistic_logs \
         WHERE created_time >= ? ORDER BY created_time ASC",
    )
    .bind(since)
    .fetch_all(pool)
    .await?;

    let mut result = Vec::new();
    for (created_time, spider) in rows {
        let Some(created_time) = created_time else {
            continue;
        };
        let date = date_of(created_time, "%Y-%m-%d");
        let Some(raw) = spider.as_deref().map(str::trim).filter(|v| !v.is_empty()) else {
            continue;
        };
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        for (label, value) in map {
            result.push(ChartData {
                date: date.clone(),
                label,
                value: value.as_i64().unwrap_or(0),
            });
        }
    }
    Ok(result)
}

/// 流量图表（最近 30 天，PV / IP）。
pub async fn chart_traffic(pool: &MySqlPool) -> anyhow::Result<Vec<ChartData>> {
    let since = start_of_today() - 30 * 86_400;
    let rows: Vec<(Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT created_time, visit_count FROM statistic_logs \
         WHERE created_time >= ? ORDER BY created_time ASC",
    )
    .bind(since)
    .fetch_all(pool)
    .await?;

    let mut result = Vec::new();
    for (created_time, visit) in rows {
        let Some(created_time) = created_time else {
            continue;
        };
        let date = date_of(created_time, "%Y-%m-%d");
        let Some(raw) = visit.as_deref().map(str::trim).filter(|v| !v.is_empty()) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        result.push(ChartData {
            date: date.clone(),
            label: "PV".into(),
            value: value.get("pv_count").and_then(Value::as_i64).unwrap_or(0),
        });
        result.push(ChartData {
            date,
            label: "IP".into(),
            value: value.get("ip_count").and_then(Value::as_i64).unwrap_or(0),
        });
    }
    Ok(result)
}

/// 可用的统计日期（`data/statistic/*.log` 的文件名，倒序）。
pub fn log_dates(statistic_dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(statistic_dir) else {
        return Vec::new();
    };
    let mut dates: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(|name| name.to_string()))
        .filter_map(|name| name.strip_suffix(".log").map(|stem| stem.to_string()))
        .collect();
    dates.sort_by(|a, b| b.cmp(a));
    dates
}

/// 读某天的访问明细（`data/statistic/<date>.log`，每行一个 JSON）。
///
/// `kind`：`spider` 只看蜘蛛，`traffic` 只看普通访客；其它值不过滤。
/// 返回 `(本页记录, 总数)`，记录按时间倒序。
pub fn read_detail(
    statistic_dir: &std::path::Path,
    date: &str,
    kind: &str,
    page: i64,
    page_size: i64,
) -> (Vec<Value>, i64) {
    let date = if date.trim().is_empty() {
        chrono::Local::now().format("%Y%m%d").to_string()
    } else {
        date.trim().to_string()
    };
    let path = statistic_dir.join(format!("{date}.log"));
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return (Vec::new(), 0);
    };

    let mut records: Vec<Value> = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| match kind {
            "spider" => value
                .get("spider")
                .and_then(Value::as_str)
                .map(|spider| !spider.is_empty())
                .unwrap_or(false),
            "traffic" => value
                .get("spider")
                .and_then(Value::as_str)
                .map(|spider| spider.is_empty())
                .unwrap_or(true),
            _ => true,
        })
        .collect();
    records.reverse(); // 新的在前

    let total = records.len() as i64;
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = ((page - 1) * page_size) as usize;
    let page_items = records
        .into_iter()
        .skip(offset)
        .take(page_size as usize)
        .collect();
    (page_items, total)
}

/// 收录趋势图表（最近 30 天，同一日期只取第一条）。
pub async fn chart_include(pool: &MySqlPool) -> anyhow::Result<Vec<ChartData>> {
    let since = start_of_today() - 30 * 86_400;
    let rows: Vec<SpiderInclude> = sqlx::query_as(
        "SELECT id, created_time, baidu_count, sogou_count, so_count, bing_count, google_count \
         FROM spider_includes WHERE created_time >= ? ORDER BY created_time ASC",
    )
    .bind(since)
    .fetch_all(pool)
    .await?;

    let mut result = Vec::new();
    let mut last_date = String::new();
    for row in rows {
        let date = date_of(row.created_time.unwrap_or(0), "%m-%d");
        if date == last_date {
            continue;
        }
        last_date = date.clone();
        for (label, value) in [
            ("百度", row.baidu_count),
            ("搜狗", row.sogou_count),
            ("360", row.so_count),
            ("必应", row.bing_count),
            ("谷歌", row.google_count),
        ] {
            result.push(ChartData {
                date: date.clone(),
                label: label.to_string(),
                value: value as i64,
            });
        }
    }
    Ok(result)
}

/// 收录明细分页，返回 `(总数, 本页记录)`（按 id 倒序）。
pub async fn include_detail(
    pool: &MySqlPool,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<SpiderInclude>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM spider_includes")
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as::<_, SpiderInclude>(
        "SELECT id, created_time, baidu_count, sogou_count, so_count, bing_count, google_count \
         FROM spider_includes ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok((total, rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn parses_statistic_json() {
        assert_eq!(json_total(Some(r#"{"pv_count":12}"#), Some("pv_count")), 12);
        assert_eq!(json_total(Some(r#"{"baidu":1,"google":2}"#), None), 3);
        assert_eq!(json_total(None, None), 0);
        assert_eq!(json_total(Some("not-json"), None), 0);
    }

    #[test]
    fn version_is_stable() {
        let info = version_info();
        assert_eq!(info.version, VERSION);
        assert!(!info.trial);
    }

    #[tokio::test]
    async fn computes_summary_from_real_db() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let stats = summary(&pool, true).await.unwrap();
        // 模型数与 modules 表一致
        let module_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM modules")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stats.archive_counts.len() as i64, module_count);
        // 总数 = 各模型之和
        assert_eq!(
            stats.archive_count.total,
            stats.archive_counts.iter().map(|m| m.total).sum::<i64>()
        );
        assert!(stats.exact);
    }

    #[tokio::test]
    async fn dashboard_reads_system_setting() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let value = dashboard(&pool).await.unwrap();
        assert!(value["version"].is_string());
        assert!(value.get("system").is_some());
    }

    #[test]
    fn reads_detail_from_temp_log() {
        let dir = std::env::temp_dir().join(format!("rustcms-stat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let date = "20260101";
        let path = dir.join(format!("{date}.log"));
        let lines = [
            r#"{"spider":"","url":"/a"}"#,
            r#"{"spider":"baidu","url":"/b"}"#,
            r#"{"spider":"","url":"/c"}"#,
        ];
        std::fs::write(&path, lines.join("\n")).unwrap();

        assert_eq!(log_dates(&dir), vec![date.to_string()]);

        let (traffic, total) = read_detail(&dir, date, "traffic", 1, 10);
        assert_eq!(total, 2);
        assert_eq!(traffic[0]["url"], "/c", "应按时间倒序");

        let (spider, total) = read_detail(&dir, date, "spider", 1, 10);
        assert_eq!(total, 1);
        assert_eq!(spider[0]["url"], "/b");

        // 不存在的日期
        let (empty, total) = read_detail(&dir, "19990101", "traffic", 1, 10);
        assert!(empty.is_empty());
        assert_eq!(total, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reads_include_tables() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let (total, rows) = include_detail(&pool, 1, 5).await.unwrap();
        assert!(total >= 0);
        assert!(rows.len() as i64 <= 5);
        // 图表不报错即可（库可能为空）
        let _ = chart_include(&pool).await.unwrap();
        let _ = chart_spider(&pool).await.unwrap();
        let _ = chart_traffic(&pool).await.unwrap();
    }
}

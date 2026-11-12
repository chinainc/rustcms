//! 后台统计与版本信息。
//!
//! 版本信息与两个统计视图（概览 / 仪表盘）。

use serde::Serialize;
use serde_json::Value;
use sqlx::mysql::MySqlPool;

/// 当前程序版本号（暂等于 crate 版本）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// 是否试用版（与既有约定对齐，当前恒为 false）。
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

/// 蜘蛛爬行图表（按天 × 蜘蛛拆分，`days` 为回看天数，1~365，默认 30）。
pub async fn chart_spider(pool: &MySqlPool, days: i64) -> anyhow::Result<Vec<ChartData>> {
    let days = days.clamp(1, 365);
    let since = start_of_today() - days * 86_400;
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

/// 流量图表（PV / IP，`days` 为回看天数，1~365，默认 30）。
pub async fn chart_traffic(pool: &MySqlPool, days: i64) -> anyhow::Result<Vec<ChartData>> {
    let days = days.clamp(1, 365);
    let since = start_of_today() - days * 86_400;
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

/// 明细日志文件名是否为 `YYYYMMDD`（8 位数字）。
///
/// 目录里可能同时存在其它 `.log`（手工排查留下的文件、旧版本遗留），
/// `log_dates` 只把日期命名的文件当成可选日期；`read_detail` 也用它
/// 挡住 `../` 之类的日期参数，避免拼出目录外的路径。
pub fn is_date_stem(stem: &str) -> bool {
    stem.len() == 8 && stem.chars().all(|c| c.is_ascii_digit())
}

/// 可用的统计日期（`data/statistic/*.log` 的文件名，倒序）。
pub fn log_dates(statistic_dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(statistic_dir) else {
        return Vec::new();
    };
    let mut dates: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(|name| name.to_string()))
        .filter_map(|name| {
            let stem = name.strip_suffix(".log")?;
            is_date_stem(stem).then(|| stem.to_string())
        })
        .collect();
    dates.sort_by(|a, b| b.cmp(a));
    dates
}

/// 确保访问明细目录存在（首次运行、目录被手工删除后都能继续写入/读取）。
pub fn ensure_dir(statistic_dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(statistic_dir);
}

/// 读某天的访问明细（`data/statistic/<date>.log`，每行一个 JSON）。
///
/// `kind`：`spider` 只看蜘蛛，`traffic` 只看普通访客；其它值不过滤。
/// `keyword` 非空时按 URL/IP/域名/UA/蜘蛛/设备模糊匹配（大小写不敏感）。
/// 返回 `(本页记录, 总数)`，记录按时间倒序。
pub fn read_detail(
    statistic_dir: &std::path::Path,
    date: &str,
    kind: &str,
    keyword: &str,
    page: i64,
    page_size: i64,
) -> (Vec<Value>, i64) {
    let records = collect_detail(statistic_dir, date, kind, keyword);
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

/// 收集某天符合过滤条件的全部明细（导出 CSV 用，不分页）。
pub fn collect_detail(
    statistic_dir: &std::path::Path,
    date: &str,
    kind: &str,
    keyword: &str,
) -> Vec<Value> {
    let date = if date.trim().is_empty() {
        chrono::Local::now().format("%Y%m%d").to_string()
    } else {
        date.trim().to_string()
    };
    // 只接受 `YYYYMMDD`，避免把 query 里的 `../` 拼进文件路径
    if !is_date_stem(&date) {
        return Vec::new();
    }
    let path = statistic_dir.join(format!("{date}.log"));
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };

    let keyword = keyword.trim().to_lowercase();
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
        .filter(|value| {
            if keyword.is_empty() {
                return true;
            }
            ["url", "ip", "host", "user_agent", "spider", "device"]
                .iter()
                .any(|field| {
                    value
                        .get(field)
                        .and_then(Value::as_str)
                        .map(|v| v.to_lowercase().contains(&keyword))
                        .unwrap_or(false)
                })
        })
        .collect();
    records.reverse(); // 新的在前
    records
}

/// 访问明细的 CSV 列（顺序与后台列表列一致）。
pub const DETAIL_CSV_HEADER: &str =
    "时间,域名,访问地址,IP,设备,蜘蛛,状态码,请求UA";

/// 收录趋势图表（同一日期只取第一条，`days` 为回看天数，1~365，默认 30）。
pub async fn chart_include(pool: &MySqlPool, days: i64) -> anyhow::Result<Vec<ChartData>> {
    let days = days.clamp(1, 365);
    let since = start_of_today() - days * 86_400;
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

// ============================================================================
// 数据采集（口径同 anqicms：访问明细日志 → 日聚合 → statistic_logs；收录快照）
// ============================================================================

/// 访问明细记录（`data/statistic/<YYYYMMDD>.log` 每行一个 JSON，
/// 字段同 anqicms `provider.Statistic`，`read_detail` 直接按这些字段读）。
#[derive(Debug, Clone, Serialize)]
pub struct AccessRecord {
    pub created_time: i64,
    pub spider: String,
    pub host: String,
    pub url: String,
    pub ip: String,
    pub device: String,
    pub http_code: i64,
    pub user_agent: String,
}

/// 蜘蛛识别（口径同 anqicms `library.GetSpider`，顺序即优先级，未命中返回空串）。
pub fn detect_spider(ua: &str) -> String {
    const SPIDERS: &[(&str, &str)] = &[
        ("googlebot", "google"),
        ("bingbot", "bing"),
        ("baiduspider", "baidu"),
        ("360spider", "360"),
        ("yahoo!", "yahoo"),
        ("sogou", "sogou"),
        ("bytespider", "byte"),
        ("yisouspider", "yisou"),
        ("yandexbot", "yandex"),
        ("spider", "other"),
        ("bot", "other"),
    ];
    let ua = ua.to_lowercase();
    for (name, val) in SPIDERS {
        if ua.contains(name) {
            return (*val).to_string();
        }
    }
    String::new()
}

/// 设备识别（口径同 anqicms `library.GetDevice`，未命中返回 "proxy"）。
pub fn detect_device(ua: &str) -> String {
    const DEVICES: &[(&str, &str)] = &[
        ("ipad", "ipad"),
        ("android", "android"),
        ("iphone", "iphone"),
        ("windows", "windows"),
        ("macintosh", "mac"),
        ("linux", "linux"),
        ("mobile", "mobile"),
        ("curl", "curl"),
        ("python", "python"),
        ("client", "client"),
        ("spider", "spider"),
        ("bot", "spider"),
    ];
    let ua = ua.to_lowercase();
    for (name, val) in DEVICES {
        if ua.contains(name) {
            return (*val).to_string();
        }
    }
    "proxy".to_string()
}

/// 按字节截断（不劈开多字节字符），口径同 anqicms 的 250 字符上限。
fn truncate_bytes(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

/// 去掉 host 里的端口（口径同 anqicms LogAccess 的 `net.SplitHostPort`）。
fn strip_host_port(host: &str) -> String {
    let host = host.trim();
    match host.rsplit_once(':') {
        Some((before, port))
            if !before.is_empty()
                && !port.is_empty()
                && port.chars().all(|c| c.is_ascii_digit()) =>
        {
            before.to_string()
        }
        _ => host.to_string(),
    }
}

/// 从请求信息构造明细记录（URL/UA 截 250 字节、host 去端口、现算蜘蛛与设备）。
pub fn access_record(
    url: &str,
    ip: &str,
    host: &str,
    user_agent: &str,
    http_code: u16,
) -> AccessRecord {
    AccessRecord {
        created_time: chrono::Local::now().timestamp(),
        spider: detect_spider(user_agent),
        host: strip_host_port(host),
        url: truncate_bytes(url, 250),
        ip: ip.trim().to_string(),
        device: detect_device(user_agent),
        http_code: http_code as i64,
        user_agent: truncate_bytes(user_agent, 250),
    }
}

/// 追加一条访问明细（每天一个 `.log` 文件，JSON 行；失败静默，不打断请求）。
pub fn log_access(statistic_dir: &std::path::Path, record: &AccessRecord) {
    let date = chrono::Local::now().format("%Y%m%d").to_string();
    if std::fs::create_dir_all(statistic_dir).is_err() {
        return;
    }
    let Ok(value) = serde_json::to_string(record) else {
        return;
    };
    use std::io::Write;
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(statistic_dir.join(format!("{date}.log")))
    else {
        return;
    };
    let _ = writeln!(file, "{value}");
}

/// 本地时区下某时间戳所属日期的 0 点。
fn start_of_day(ts: i64) -> i64 {
    use chrono::{Local, TimeZone};
    let Some(dt) = Local.timestamp_opt(ts, 0).single() else {
        return ts;
    };
    dt.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).single())
        .map(|day| day.timestamp())
        .unwrap_or(ts)
}

/// 次日 0 点（跨 DST 用日历日期推进，不做 86400 秒硬加）。
fn next_day_start(ts: i64) -> i64 {
    use chrono::{Duration, Local, TimeZone};
    let Some(dt) = Local.timestamp_opt(ts, 0).single() else {
        return ts + 86_400;
    };
    (dt.date_naive() + Duration::days(1))
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).single())
        .map(|day| day.timestamp())
        .unwrap_or(ts + 86_400)
}

/// 聚合一天的明细日志（口径同 anqicms `CalcLog`）：
/// 蜘蛛按名计数（不限状态码）；非蜘蛛只算 2xx 的 PV；IP 去重（不限状态码）。
///
/// 返回 `(spider_count, visit_count)` JSON 串；当天没有日志文件返回 `None`。
fn calc_log(statistic_dir: &std::path::Path, day_start: i64) -> Option<(String, String)> {
    let date = chrono::DateTime::from_timestamp(day_start, 0)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%Y%m%d").to_string())?;
    let raw = std::fs::read_to_string(statistic_dir.join(format!("{date}.log"))).ok()?;

    let mut spider: std::collections::BTreeMap<String, i64> = Default::default();
    let mut pv: i64 = 0;
    let mut ips: std::collections::HashSet<String> = Default::default();
    for line in raw.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let spider_name = value.get("spider").and_then(Value::as_str).unwrap_or("");
        if !spider_name.is_empty() {
            *spider.entry(spider_name.to_string()).or_insert(0) += 1;
        } else {
            let code = value.get("http_code").and_then(Value::as_i64).unwrap_or(0);
            if (200..300).contains(&code) {
                pv += 1;
            }
            // IP 去重不限状态码（304/301 等正常访问也代表真实访客）
            if let Some(ip) = value.get("ip").and_then(Value::as_str) {
                ips.insert(ip.to_string());
            }
        }
    }
    let spider_json = serde_json::to_string(&spider).unwrap_or_else(|_| "{}".to_string());
    let visit_json = serde_json::json!({ "pv_count": pv, "ip_count": ips.len() }).to_string();
    Some((spider_json, visit_json))
}

async fn upsert_statistic_log(
    pool: &MySqlPool,
    created_time: i64,
    spider_json: &str,
    visit_json: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO statistic_logs (created_time, spider_count, visit_count) VALUES (?, ?, ?) \
         ON DUPLICATE KEY UPDATE spider_count = VALUES(spider_count), visit_count = VALUES(visit_count)",
    )
    .bind(created_time)
    .bind(spider_json)
    .bind(visit_json)
    .execute(pool)
    .await?;
    Ok(())
}

/// 聚合访问日志到 `statistic_logs`（口径同 anqicms `StatisticLog.Calc`，每 10 分钟一次）。
///
/// 从最后一条统计的次日开始补算到今天；最后一条不是当天时先重算它（当天日志还在追加）；
/// 没有任何统计时从 30 天前开始（与图表取数窗口一致）。
pub async fn calc_statistics(
    pool: &MySqlPool,
    statistic_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let today = start_of_today();
    let last: Option<i64> =
        sqlx::query_scalar("SELECT created_time FROM statistic_logs ORDER BY id DESC LIMIT 1")
            .fetch_optional(pool)
            .await?;
    let mut start = match last {
        Some(last_time) => {
            let last_day = start_of_day(last_time);
            if last_day != today {
                if let Some((spider, visit)) = calc_log(statistic_dir, last_day) {
                    upsert_statistic_log(pool, last_day, &spider, &visit).await?;
                }
            }
            next_day_start(last_day)
        }
        None => today - 30 * 86_400,
    };
    while start < today {
        if let Some((spider, visit)) = calc_log(statistic_dir, start) {
            upsert_statistic_log(pool, start, &spider, &visit).await?;
        }
        start = next_day_start(start);
    }
    if let Some((spider, visit)) = calc_log(statistic_dir, today) {
        upsert_statistic_log(pool, today, &spider, &visit).await?;
    }
    Ok(())
}

/// 解析收录数：抓取页面正文，按引擎各自的正则依次尝试取结果数
/// （口径同 anqicms spiderInclude.go；不同地区/语言的返回文案不一致，
/// 所以每个引擎给多个候选正则）。
async fn fetch_include(url: String, patterns: &[&str]) -> i64 {
    let regexes: Vec<regex::Regex> = patterns
        .iter()
        .filter_map(|pattern| regex::Regex::new(pattern).ok())
        .collect();
    if regexes.is_empty() {
        return 0;
    }
    let request = rustcms_ext::http::Request::get(url)
        .with_header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/94.0.4606.81 Safari/537.36",
        )
        .with_timeout(std::time::Duration::from_secs(5));
    match rustcms_ext::http::send(&request).await {
        Ok(response) => {
            let body = response.text();
            for re in regexes {
                if let Some(caps) = re.captures(&body) {
                    if let Some(m) = caps.get(1) {
                        return m.as_str().replace(',', "").parse::<i64>().unwrap_or(0);
                    }
                }
            }
            0
        }
        Err(_) => 0,
    }
}

/// 采集搜索引擎收录量并落库（口径同 anqicms `QuerySpiderInclude`，每天 8 点一次）。
///
/// 站点域名取 `front_url`（缺省回落 `base_url`）；任何一家请求失败按 0 计，不阻断其余。
pub async fn query_spider_include(pool: &MySqlPool) -> anyhow::Result<()> {
    let builder = crate::models::front_content::LinkBuilder::load(pool)
        .await
        .unwrap_or_default();
    let base = if builder.front_url.trim().is_empty() {
        builder.base_url.clone()
    } else {
        builder.front_url.clone()
    };
    let host = {
        let base = base.trim();
        let without_scheme = base
            .strip_prefix("http://")
            .or_else(|| base.strip_prefix("https://"))
            .unwrap_or(base);
        without_scheme
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("")
            .split(':')
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    };
    if host.is_empty() {
        anyhow::bail!("未配置站点域名（system.base_url / front_url），无法采集收录量");
    }

    let query = format!("site%3A{host}");
    let (baidu, sogou, so, bing, google) = tokio::join!(
        fetch_include(
            format!("https://www.baidu.com/s?wd={query}&tn=json&rn=10"),
            &[r#"(?i)"all"\s*:\s*(\d+)"#],
        ),
        fetch_include(
            format!("https://www.sogou.com/web?query={query}"),
            &[
                r#"(?i)'rn'\s*:\s*'(\d+)'"#,
                r#"(?i)找到约?\s*([0-9,]+)\s*条"#,
            ],
        ),
        fetch_include(
            format!("https://www.so.com/s?q={query}"),
            &[r#"(?i)找到相关结果约([0-9,]+)个"#],
        ),
        fetch_include(
            format!("https://www.bing.com/search?q={query}"),
            &[
                r#"(?i)([0-9,]+)\s*条结果"#,
                r#"(?i)([0-9,]+)\s*results"#,
            ],
        ),
        fetch_include(
            format!("https://www.google.com/search?q={query}"),
            &[
                r#"(?i)About\s([0-9,]+)\sresults"#,
                r#"(?i)找到约\s*([0-9,]+)\s*条"#,
                r#"(?i)([0-9,]+)\s*results"#,
            ],
        ),
    );

    sqlx::query(
        "INSERT INTO spider_includes \
         (created_time, baidu_count, sogou_count, so_count, bing_count, google_count) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(chrono::Local::now().timestamp())
    .bind(baidu)
    .bind(sogou)
    .bind(so)
    .bind(bing)
    .bind(google)
    .execute(pool)
    .await?;
    Ok(())
}

/// 当天还没有收录快照时采集一次（服务启动后、跨天 8 点后的第一个 tick 调用）。
///
/// 返回 `true` 表示本次真的发起了采集。当天已有记录时直接跳过，
/// 避免服务反复重启时重复请求搜索引擎。
pub async fn ensure_daily_include(pool: &MySqlPool) -> anyhow::Result<bool> {
    let today = start_of_today();
    let existing: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM spider_includes WHERE created_time >= ?")
            .bind(today)
            .fetch_one(pool)
            .await?;
    if existing > 0 {
        return Ok(false);
    }
    query_spider_include(pool).await?;
    Ok(true)
}

/// 清理 31 天前的明细日志文件（图表只取 30 天窗口，对齐 anqicms 的保留策略）。
pub fn clean_old_logs(statistic_dir: &std::path::Path) {
    let cutoff = chrono::Local::now().date_naive() - chrono::Duration::days(31);
    let Ok(entries) = std::fs::read_dir(statistic_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".log") else {
            continue;
        };
        // 只清理 8 位日期命名的明细日志
        if stem.len() != 8 || !stem.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Ok(day) = chrono::NaiveDate::parse_from_str(stem, "%Y%m%d") {
            if day < cutoff {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
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

    #[test]
    fn detects_spider_and_device() {
        // 蜘蛛识别（口径同 anqicms library.GetSpider）
        assert_eq!(detect_spider("Mozilla/5.0 (compatible; Baiduspider/2.0)"), "baidu");
        assert_eq!(detect_spider("Googlebot/2.1 (+http://www.google.com/bot.html)"), "google");
        assert_eq!(detect_spider("curl/8.0"), "");
        assert_eq!(detect_spider("SomeBot/1.0"), "other");
        assert_eq!(detect_spider("Sogou web spider/4.0"), "sogou");
        // 设备识别（口径同 anqicms library.GetDevice）
        assert_eq!(detect_device("Mozilla/5.0 (iPhone; CPU iPhone OS 16_0)"), "iphone");
        assert_eq!(detect_device("Mozilla/5.0 (Windows NT 10.0)"), "windows");
        assert_eq!(detect_device("curl/8.0"), "curl");
        assert_eq!(detect_device("SomeBot/1.0"), "spider");
        assert_eq!(detect_device("unknown-agent"), "proxy");
    }

    #[test]
    fn logs_access_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("rustcms-stat-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let record = access_record(
            "/abc?x=1",
            "1.2.3.4",
            "example.com:8080",
            "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0) Safari",
            200,
        );
        assert_eq!(record.host, "example.com", "应去掉端口");
        assert_eq!(record.device, "iphone");
        assert_eq!(record.spider, "");
        log_access(&dir, &record);

        let (traffic, total) = read_detail(&dir, "", "traffic", "", 1, 10);
        assert_eq!(total, 1);
        assert_eq!(traffic[0]["url"], "/abc?x=1");
        assert_eq!(traffic[0]["http_code"], 200);
        assert_eq!(traffic[0]["device"], "iphone");

        // 关键词过滤：按 URL 片段匹配
        let (hits, hits_total) = read_detail(&dir, "", "traffic", "abc", 1, 10);
        assert_eq!(hits_total, 1);
        assert_eq!(hits[0]["url"], "/abc?x=1");
        let (miss, miss_total) = read_detail(&dir, "", "traffic", "no-such", 1, 10);
        assert!(miss.is_empty());
        assert_eq!(miss_total, 0);
    }

    #[test]
    fn aggregates_log_like_anqicms_calc() {
        let dir = std::env::temp_dir().join(format!("rustcms-stat-calc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let today = start_of_today();
        let date = chrono::DateTime::from_timestamp(today, 0)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%Y%m%d")
            .to_string();
        // 蜘蛛不限状态码都计数；访客只算 2xx 的 PV；IP 去重
        let lines = [
            r#"{"created_time":1,"spider":"baidu","url":"/a","http_code":200}"#,
            r#"{"created_time":2,"spider":"baidu","url":"/b","http_code":404}"#,
            r#"{"created_time":3,"spider":"","url":"/c","ip":"1.1.1.1","http_code":200}"#,
            r#"{"created_time":4,"spider":"","url":"/d","ip":"1.1.1.1","http_code":200}"#,
            r#"{"created_time":5,"spider":"","url":"/e","ip":"2.2.2.2","http_code":304}"#,
        ];
        std::fs::write(dir.join(format!("{date}.log")), lines.join("\n")).unwrap();

        let (spider, visit) = calc_log(&dir, today).expect("当天有日志应聚合成功");
        let spider: Value = serde_json::from_str(&spider).unwrap();
        assert_eq!(spider["baidu"], 2, "蜘蛛不限状态码");
        let visit: Value = serde_json::from_str(&visit).unwrap();
        assert_eq!(visit["pv_count"], 2, "只有 2xx 计 PV");
        assert_eq!(visit["ip_count"], 2, "两个独立 IP（1.1.1.1 去重）");

        // 没有日志的日期返回 None
        assert!(calc_log(&dir, today - 40 * 86_400).is_none());
    }

    #[tokio::test]
    async fn calc_statistics_upserts_today() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        // ⚠️ calc_statistics 会重算 statistic_logs 的当天行（可能波及历史行），
        // 先整表备份、结束还原，避免测试聚合覆盖现网真实统计。
        let backup: Vec<(i64, Option<String>, Option<String>)> =
            sqlx::query_as("SELECT created_time, spider_count, visit_count FROM statistic_logs")
                .fetch_all(&pool)
                .await
                .unwrap();

        let dir = std::env::temp_dir().join(format!("rustcms-stat-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // 造一条今天的访问记录
        let record = access_record("/calc-test", "3.3.3.3", "calc.test", "curl/8.0", 200);
        log_access(&dir, &record);

        let result = calc_statistics(&pool, &dir).await;
        // 还原（先删后插，幂等）
        let _ = sqlx::query("DELETE FROM statistic_logs").execute(&pool).await;
        for (created_time, spider, visit) in &backup {
            let _ = sqlx::query(
                "INSERT INTO statistic_logs (created_time, spider_count, visit_count) VALUES (?, ?, ?)",
            )
            .bind(created_time)
            .bind(spider)
            .bind(visit)
            .execute(&pool)
            .await;
        }
        result.unwrap();

        let today = start_of_today();
        let visit: Option<String> =
            sqlx::query_scalar("SELECT visit_count FROM statistic_logs WHERE created_time = ?")
                .bind(today)
                .fetch_optional(&pool)
                .await
                .unwrap();
        let visit: Value = serde_json::from_str(&visit.unwrap_or_default()).unwrap_or_default();
        assert!(visit["pv_count"].as_i64().unwrap_or(0) >= 1, "今天的 PV 应已聚合：{visit}");
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

        let (traffic, total) = read_detail(&dir, date, "traffic", "", 1, 10);
        assert_eq!(total, 2);
        assert_eq!(traffic[0]["url"], "/c", "应按时间倒序");

        let (spider, total) = read_detail(&dir, date, "spider", "", 1, 10);
        assert_eq!(total, 1);
        assert_eq!(spider[0]["url"], "/b");

        // 不存在的日期
        let (empty, total) = read_detail(&dir, "19990101", "traffic", "", 1, 10);
        assert!(empty.is_empty());
        assert_eq!(total, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ignores_non_date_logs_and_rejects_traversal() {
        let dir = std::env::temp_dir().join(format!("rustcms-stat-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("20260101.log"), r#"{"spider":"","url":"/a"}"#).unwrap();
        // 非日期命名不应出现在可选日期里
        std::fs::write(dir.join("debug.log"), "x").unwrap();
        std::fs::write(dir.join("2026010.log"), "x").unwrap();

        assert_eq!(log_dates(&dir), vec!["20260101".to_string()]);

        // 日期参数不是 8 位数字时直接返回空，不能拼出目录外的文件
        let (items, total) = read_detail(&dir, "../20260101", "traffic", "", 1, 10);
        assert!(items.is_empty());
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
        // 图表不报错即可（库可能为空）；天数窗口取一个非法值应被收敛到 1~365
        let _ = chart_include(&pool, 30).await.unwrap();
        let _ = chart_spider(&pool, 7).await.unwrap();
        let _ = chart_traffic(&pool, 400).await.unwrap();
    }
}

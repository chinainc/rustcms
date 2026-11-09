//! 限流器与 IP 屏蔽名单。
//!
//! 令牌桶控制请求频率，
//! 滑动窗口计数触发 IP 封禁，封禁到期自动解封。
//!
//! 服务于 `src/routes.rs` 中原本返回「限流器尚未实现，没有可解除的 IP」
//! 「限流器本身未实现，因此屏蔽列表恒为空」的两个端点。
//!
//! 所有时间判断都基于 [`std::time::Instant`]，测试可用 `*_at` 系列方法
//! 注入确定时刻，不依赖 sleep。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 令牌桶配置。
#[derive(Debug, Clone, Copy)]
pub struct BucketConfig {
    /// 每秒补充的令牌数。
    pub rate_per_sec: f64,
    /// 桶容量（允许的突发量）。
    pub burst: f64,
}

impl Default for BucketConfig {
    fn default() -> Self {
        Self {
            rate_per_sec: 5.0,
            burst: 20.0,
        }
    }
}

/// 单个键的令牌桶状态。
#[derive(Debug, Clone)]
struct BucketState {
    tokens: f64,
    updated: Instant,
}

/// 按键（通常是 IP 或用户 ID）限流。
#[derive(Debug)]
pub struct RateLimiter {
    config: BucketConfig,
    buckets: Mutex<HashMap<String, BucketState>>,
}

impl RateLimiter {
    /// 新建限流器。
    pub fn new(config: BucketConfig) -> Self {
        Self {
            config,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// 用「每秒次数 + 突发量」快捷构造。
    pub fn per_second(rate: f64, burst: u32) -> Self {
        Self::new(BucketConfig {
            rate_per_sec: rate,
            burst: burst as f64,
        })
    }

    /// 取一个令牌；成功返回 `true`（允许请求）。
    pub fn allow(&self, key: &str) -> bool {
        self.allow_at(key, Instant::now())
    }

    /// 指定时刻取令牌（测试用）。
    pub fn allow_at(&self, key: &str, now: Instant) -> bool {
        let mut buckets = match self.buckets.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let bucket = buckets.entry(key.to_string()).or_insert(BucketState {
            tokens: self.config.burst,
            updated: now,
        });

        // 按经过的时间补充令牌，最多补到桶容量。
        let elapsed = now.saturating_duration_since(bucket.updated).as_secs_f64();
        if elapsed > 0.0 {
            bucket.tokens = (bucket.tokens + elapsed * self.config.rate_per_sec).min(self.config.burst);
            bucket.updated = now;
        }

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// 剩余令牌数（供 429 响应填 `Retry-After` 或提示文案）。
    pub fn remaining(&self, key: &str) -> f64 {
        let now = Instant::now();
        let mut buckets = match self.buckets.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let bucket = buckets.entry(key.to_string()).or_insert(BucketState {
            tokens: self.config.burst,
            updated: now,
        });
        let elapsed = now.saturating_duration_since(bucket.updated).as_secs_f64();
        (bucket.tokens + elapsed * self.config.rate_per_sec).min(self.config.burst)
    }

    /// 清除某个键的计数。
    pub fn forget(&self, key: &str) {
        let mut buckets = match self.buckets.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        buckets.remove(key);
    }

    /// 清空全部计数。
    pub fn reset(&self) {
        let mut buckets = match self.buckets.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        buckets.clear();
    }

    /// 当前已记录的键数量。
    pub fn tracked_keys(&self) -> usize {
        match self.buckets.lock() {
            Ok(guard) => guard.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }
}

/// IP 访问判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 放行。
    Allow,
    /// 频率超限（尚未达到封禁阈值）。
    Throttled,
    /// 已被封禁。
    Blocked,
}

impl Verdict {
    /// 是否放行。
    pub fn is_allowed(self) -> bool {
        matches!(self, Verdict::Allow)
    }
}

/// 封禁配置。
#[derive(Debug, Clone, Copy)]
pub struct BlockConfig {
    /// 窗口内允许的请求次数（超过即进入封禁）。
    pub threshold: u32,
    /// 统计窗口长度。
    pub window: Duration,
    /// 封禁持续时长。
    pub block_for: Duration,
}

impl Default for BlockConfig {
    fn default() -> Self {
        Self {
            threshold: 60,
            window: Duration::from_secs(60),
            block_for: Duration::from_secs(600),
        }
    }
}

#[derive(Debug, Clone)]
struct BlockState {
    count: u32,
    window_start: Instant,
    blocked_until: Option<Instant>,
}

/// 按 IP 统计请求次数并在超限时封禁。
#[derive(Debug)]
pub struct IpBlocker {
    config: BlockConfig,
    states: Mutex<HashMap<String, BlockState>>,
}

impl IpBlocker {
    /// 新建封禁器。
    pub fn new(config: BlockConfig) -> Self {
        Self {
            config,
            states: Mutex::new(HashMap::new()),
        }
    }

    /// 记录一次访问并给出判定。
    pub fn hit(&self, ip: &str) -> Verdict {
        self.hit_at(ip, Instant::now())
    }

    /// 指定时刻记录一次访问（测试用）。
    pub fn hit_at(&self, ip: &str, now: Instant) -> Verdict {
        let mut states = match self.states.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        // 封禁到期自动放行。
        if let Some(state) = states.get_mut(ip) {
            if let Some(until) = state.blocked_until {
                if now >= until {
                    state.blocked_until = None;
                    state.count = 0;
                    state.window_start = now;
                }
            }
            if state.blocked_until.is_some() {
                return Verdict::Blocked;
            }
        }

        let state = states.entry(ip.to_string()).or_insert(BlockState {
            count: 0,
            window_start: now,
            blocked_until: None,
        });

        // 窗口滚动：超时后重新计数。
        if now.saturating_duration_since(state.window_start) >= self.config.window {
            state.count = 0;
            state.window_start = now;
        }

        state.count += 1;
        if state.count > self.config.threshold {
            // 超过窗口内允许次数：封禁。
            state.blocked_until = Some(now + self.config.block_for);
            Verdict::Blocked
        } else if state.count == self.config.threshold {
            // 刚好到达上限：放行但提示限速。
            Verdict::Throttled
        } else {
            Verdict::Allow
        }
    }

    /// 某个 IP 当前是否处于封禁状态。
    pub fn is_blocked(&self, ip: &str) -> bool {
        let now = Instant::now();
        let states = match self.states.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        states
            .get(ip)
            .and_then(|state| state.blocked_until)
            .map(|until| now < until)
            .unwrap_or(false)
    }

    /// 解除某个 IP 的封禁，返回是否原本处于封禁状态。
    pub fn unblock(&self, ip: &str) -> bool {
        let mut states = match self.states.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        match states.get_mut(ip) {
            Some(state) => {
                let was_blocked = state.blocked_until.is_some();
                state.blocked_until = None;
                state.count = 0;
                state.window_start = Instant::now();
                was_blocked
            }
            None => false,
        }
    }

    /// 当前仍处于封禁状态的 IP 列表。
    pub fn blocked_ips(&self) -> Vec<String> {
        let now = Instant::now();
        let states = match self.states.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut list: Vec<String> = states
            .iter()
            .filter(|(_, state)| {
                state
                    .blocked_until
                    .map(|until| now < until)
                    .unwrap_or(false)
            })
            .map(|(ip, _)| ip.clone())
            .collect();
        list.sort();
        list
    }

    /// 当前封禁中的 `(IP, 解封 Unix 时间戳)` 列表，按解封时间倒序。
    ///
    /// 元素形如 `(ip, block_time)`，供后台「屏蔽 IP 列表」直接渲染。
    pub fn blocked_entries(&self) -> Vec<(String, u64)> {
        let now = Instant::now();
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        let states = match self.states.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut list: Vec<(String, u64)> = states
            .iter()
            .filter_map(|(ip, state)| {
                state.blocked_until.filter(|until| now < *until).map(|until| {
                    let remaining = until.saturating_duration_since(now).as_secs();
                    (ip.clone(), wall.saturating_add(remaining))
                })
            })
            .collect();
        list.sort_by(|left, right| right.1.cmp(&left.1));
        list
    }

    /// 清空全部记录。
    pub fn reset(&self) {
        let mut states = match self.states.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        states.clear();
    }
}

// ---------------------------------------------------------------------------
// 系统内存占用（限流器的内存阈值判定）
// ---------------------------------------------------------------------------
//
// 需要三个量：总内存、进程占用百分比、系统空闲百分比。
// 进程占用取本进程的工作集 / 常驻集，系统空闲取可用物理内存。
// Rust 标准库没有内存统计 API，这里用平台接口近似：
//
// * Windows：`GlobalMemoryStatusEx` 取物理内存 / 可用内存，`K32GetProcessMemoryInfo` 取工作集；
// * Linux：`/proc/meminfo` 取 `MemTotal` / `MemAvailable`，`/proc/self/statm` 取常驻页数；
// * 其它平台：返回 `None`（阈值判定视为不触发，不影响其余限流逻辑）。

/// 系统内存信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryInfo {
    /// 物理内存总量（字节）。
    pub total_bytes: u64,
    /// 可用物理内存（字节）。
    pub available_bytes: u64,
}

impl MemoryInfo {
    /// 系统空闲内存占总内存的百分比。
    pub fn free_percent(&self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        self.available_bytes as f64 / self.total_bytes as f64 * 100.0
    }

    /// 系统已用内存占总内存的百分比。
    pub fn used_percent(&self) -> f64 {
        100.0 - self.free_percent()
    }
}

/// 限流器视角的内存快照。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryUsage {
    pub total_bytes: u64,
    pub available_bytes: u64,
    /// 本进程占用（工作集 / 常驻集）。
    pub process_bytes: u64,
    /// 进程占用占总内存的百分比。
    pub process_percent: f64,
    /// 系统空闲占总内存的百分比。
    pub free_percent: f64,
    /// 系统已用占总内存的百分比。
    pub used_percent: f64,
}

/// 读取系统物理内存（失败返回 `None`）。
pub fn system_memory() -> Option<MemoryInfo> {
    memplatform::system_memory()
}

/// 读取本进程占用字节数（失败返回 `None`）。
pub fn process_memory_bytes() -> Option<u64> {
    memplatform::process_memory_bytes()
}

/// 一次性读取内存快照（系统 + 进程）。
pub fn memory_usage() -> Option<MemoryUsage> {
    let info = system_memory()?;
    if info.total_bytes == 0 {
        return None;
    }
    let process_bytes = process_memory_bytes().unwrap_or(0).min(info.total_bytes);
    let total = info.total_bytes as f64;
    Some(MemoryUsage {
        total_bytes: info.total_bytes,
        available_bytes: info.available_bytes,
        process_bytes,
        process_percent: process_bytes as f64 / total * 100.0,
        free_percent: info.free_percent(),
        used_percent: info.used_percent(),
    })
}

/// 是否已经超过内存阈值。
///
/// 判定条件：`进程占用百分比 > 阈值 || 系统空闲百分比 < 100 - 阈值`，
/// 且只在阈值 `> 50` 时才真正采样（低阈值下内存压力判定没有意义）。
pub fn mem_over_threshold(mem_percent: i32) -> bool {
    if mem_percent <= 50 {
        return false;
    }
    let Some(usage) = memory_usage() else {
        return false;
    };
    let limit = mem_percent as f64;
    usage.process_percent > limit || usage.free_percent < 100.0 - limit
}

#[cfg(windows)]
mod memplatform {
    use super::MemoryInfo;
    use std::ffi::c_void;

    /// `MEMORYSTATUSEX`（Win32 结构体，字段名照官方定义保留）。
    #[repr(C)]
    #[allow(non_snake_case)]
    struct MemoryStatusEx {
        dwLength: u32,
        dwMemoryLoad: u32,
        ullTotalPhys: u64,
        ullAvailPhys: u64,
        ullTotalPageFile: u64,
        ullAvailPageFile: u64,
        ullTotalVirtual: u64,
        ullAvailVirtual: u64,
        ullAvailExtendedVirtual: u64,
    }

    /// `PROCESS_MEMORY_COUNTERS`（psapi 结构体；Win7+ 由 kernel32 的 K32 前缀导出）。
    #[repr(C)]
    #[allow(non_snake_case)]
    struct ProcessMemoryCounters {
        cb: u32,
        PageFaultCount: u32,
        PeakWorkingSetSize: usize,
        WorkingSetSize: usize,
        QuotaPeakPagedPoolUsage: usize,
        QuotaPagedPoolUsage: usize,
        QuotaPeakNonPagedPoolUsage: usize,
        QuotaNonPagedPoolUsage: usize,
        PagefileUsage: usize,
        PeakPagefileUsage: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
        fn K32GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCounters,
            size: u32,
        ) -> i32;
        fn GetCurrentProcess() -> *mut c_void;
    }

    pub fn system_memory() -> Option<MemoryInfo> {
        let mut status = MemoryStatusEx {
            dwLength: std::mem::size_of::<MemoryStatusEx>() as u32,
            dwMemoryLoad: 0,
            ullTotalPhys: 0,
            ullAvailPhys: 0,
            ullTotalPageFile: 0,
            ullAvailPageFile: 0,
            ullTotalVirtual: 0,
            ullAvailVirtual: 0,
            ullAvailExtendedVirtual: 0,
        };
        // SAFETY: 传入的是本栈上大小正确的结构体，`dwLength` 已按官方要求置为 sizeof。
        let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
        if ok == 0 || status.ullTotalPhys == 0 {
            return None;
        }
        Some(MemoryInfo {
            total_bytes: status.ullTotalPhys,
            available_bytes: status.ullAvailPhys.min(status.ullTotalPhys),
        })
    }

    pub fn process_memory_bytes() -> Option<u64> {
        let mut counters = ProcessMemoryCounters {
            cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
            PageFaultCount: 0,
            PeakWorkingSetSize: 0,
            WorkingSetSize: 0,
            QuotaPeakPagedPoolUsage: 0,
            QuotaPagedPoolUsage: 0,
            QuotaPeakNonPagedPoolUsage: 0,
            QuotaNonPagedPoolUsage: 0,
            PagefileUsage: 0,
            PeakPagefileUsage: 0,
        };
        // SAFETY: 三个 API 都只读写本栈上传入的结构体 / 当前进程句柄（伪句柄无需释放）。
        let ok = unsafe {
            K32GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut counters,
                std::mem::size_of::<ProcessMemoryCounters>() as u32,
            )
        };
        if ok == 0 {
            return None;
        }
        Some(counters.WorkingSetSize as u64)
    }
}

/// 解析 `/proc/meminfo` 文本，返回 `(总内存 kB, 可用内存 kB)`。
///
/// 抽成**平台无关的纯函数**是为了让这段解析在 Windows 上也能编译并被单测覆盖
/// （本机没有 `/proc`，不抽出来就只能靠肉眼检查 cfg 块）。
/// 老内核（< 3.14）没有 `MemAvailable`，退化成 `MemFree`。
///
/// 只在 Linux 与测试构建里存在：其它平台没有调用方，留着会报 dead_code。
#[cfg(any(target_os = "linux", test))]
fn parse_meminfo_kb(text: &str) -> Option<(u64, u64)> {
    fn kb_value(rest: &str) -> Option<u64> {
        rest.split_whitespace().next()?.parse::<u64>().ok()
    }
    let mut total_kb = 0u64;
    let mut available_kb = 0u64;
    let mut free_kb = 0u64;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total_kb = kb_value(rest)?;
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            available_kb = kb_value(rest).unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("MemFree:") {
            free_kb = kb_value(rest).unwrap_or(0);
        }
    }
    if total_kb == 0 {
        return None;
    }
    if available_kb == 0 {
        available_kb = free_kb;
    }
    Some((total_kb, available_kb.min(total_kb)))
}

/// 解析 `/proc/self/statm` 的常驻页数（第二列）。
///
/// 同 `parse_meminfo_kb`：只在 Linux 与测试构建里存在。
#[cfg(any(target_os = "linux", test))]
fn parse_statm_resident_pages(text: &str) -> Option<u64> {
    text.split_whitespace().nth(1)?.parse::<u64>().ok()
}

#[cfg(target_os = "linux")]
mod memplatform {
    use super::MemoryInfo;

    /// `/proc/self/statm` 的页大小；常见架构（x86_64 / aarch64）都是 4KiB。
    const PAGE_SIZE: u64 = 4096;

    pub fn system_memory() -> Option<MemoryInfo> {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let (total_kb, available_kb) = super::parse_meminfo_kb(&text)?;
        Some(MemoryInfo {
            total_bytes: total_kb * 1024,
            available_bytes: available_kb * 1024,
        })
    }

    pub fn process_memory_bytes() -> Option<u64> {
        let text = std::fs::read_to_string("/proc/self/statm").ok()?;
        Some(super::parse_statm_resident_pages(&text)? * PAGE_SIZE)
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod memplatform {
    use super::MemoryInfo;

    pub fn system_memory() -> Option<MemoryInfo> {
        None
    }

    pub fn process_memory_bytes() -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_allows_burst_then_refuses() {
        let limiter = RateLimiter::per_second(1.0, 3);
        let now = Instant::now();
        assert!(limiter.allow_at("1.2.3.4", now));
        assert!(limiter.allow_at("1.2.3.4", now));
        assert!(limiter.allow_at("1.2.3.4", now));
        // 桶已空，立刻再取应失败。
        assert!(!limiter.allow_at("1.2.3.4", now));
    }

    #[test]
    fn bucket_refills_over_time() {
        let limiter = RateLimiter::per_second(2.0, 2);
        let start = Instant::now();
        assert!(limiter.allow_at("k", start));
        assert!(limiter.allow_at("k", start));
        assert!(!limiter.allow_at("k", start));
        // 过 1 秒补 2 个令牌。
        let later = start + Duration::from_millis(1000);
        assert!(limiter.allow_at("k", later));
        assert!(limiter.allow_at("k", later));
        assert!(!limiter.allow_at("k", later));
    }

    #[test]
    fn bucket_is_per_key() {
        let limiter = RateLimiter::per_second(1.0, 1);
        let now = Instant::now();
        assert!(limiter.allow_at("a", now));
        assert!(limiter.allow_at("b", now));
        assert!(!limiter.allow_at("a", now));
        assert_eq!(limiter.tracked_keys(), 2);
    }

    #[test]
    fn bucket_forget_resets_single_key() {
        let limiter = RateLimiter::per_second(1.0, 1);
        let now = Instant::now();
        assert!(limiter.allow_at("a", now));
        assert!(!limiter.allow_at("a", now));
        limiter.forget("a");
        assert!(limiter.allow_at("a", now));
    }

    #[test]
    fn blocker_blocks_after_threshold() {
        let blocker = IpBlocker::new(BlockConfig {
            threshold: 3,
            window: Duration::from_secs(60),
            block_for: Duration::from_secs(300),
        });
        let now = Instant::now();
        assert_eq!(blocker.hit_at("9.9.9.9", now), Verdict::Allow);
        assert_eq!(blocker.hit_at("9.9.9.9", now), Verdict::Allow);
        assert_eq!(blocker.hit_at("9.9.9.9", now), Verdict::Throttled);
        assert_eq!(blocker.hit_at("9.9.9.9", now), Verdict::Blocked);
        assert_eq!(blocker.blocked_ips(), vec!["9.9.9.9".to_string()]);
        // 封禁期内继续请求仍是 Blocked。
        assert_eq!(
            blocker.hit_at("9.9.9.9", now + Duration::from_secs(10)),
            Verdict::Blocked
        );
    }

    #[test]
    fn blocker_expires_after_block_duration() {
        let blocker = IpBlocker::new(BlockConfig {
            threshold: 1,
            window: Duration::from_secs(60),
            block_for: Duration::from_secs(30),
        });
        let now = Instant::now();
        assert_eq!(blocker.hit_at("8.8.8.8", now), Verdict::Throttled);
        assert_eq!(blocker.hit_at("8.8.8.8", now), Verdict::Blocked);
        let later = now + Duration::from_secs(31);
        assert_eq!(blocker.hit_at("8.8.8.8", later), Verdict::Throttled);
    }

    #[test]
    fn blocker_unblock_clears_state() {
        let blocker = IpBlocker::new(BlockConfig {
            threshold: 1,
            window: Duration::from_secs(60),
            block_for: Duration::from_secs(300),
        });
        let now = Instant::now();
        blocker.hit_at("7.7.7.7", now);
        blocker.hit_at("7.7.7.7", now);
        assert!(blocker.blocked_ips().contains(&"7.7.7.7".to_string()));
        assert!(blocker.is_blocked("7.7.7.7"));
        assert!(blocker.unblock("7.7.7.7"));
        assert!(!blocker.is_blocked("7.7.7.7"));
        assert!(blocker.blocked_ips().is_empty());
        assert_eq!(blocker.hit_at("7.7.7.7", now), Verdict::Throttled);
    }

    #[test]
    fn blocker_reports_unblock_deadline() {
        let blocker = IpBlocker::new(BlockConfig {
            threshold: 1,
            window: Duration::from_secs(60),
            block_for: Duration::from_secs(300),
        });
        let now = Instant::now();
        assert_eq!(blocker.hit_at("6.6.6.6", now), Verdict::Throttled);
        assert_eq!(blocker.hit_at("6.6.6.6", now), Verdict::Blocked);
        let entries = blocker.blocked_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "6.6.6.6");
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(entries[0].1 >= wall + 290, "{entries:?}");
    }

    #[test]
    fn blocker_window_rolls_over() {
        let blocker = IpBlocker::new(BlockConfig {
            threshold: 2,
            window: Duration::from_secs(10),
            block_for: Duration::from_secs(60),
        });
        let start = Instant::now();
        assert_eq!(blocker.hit_at("6.6.6.6", start), Verdict::Allow);
        assert_eq!(blocker.hit_at("6.6.6.6", start), Verdict::Throttled);
        // 窗口过后计数归零。
        let later = start + Duration::from_secs(11);
        assert_eq!(blocker.hit_at("6.6.6.6", later), Verdict::Allow);
    }

    #[test]
    fn memory_percentages_follow_total_and_available() {
        let info = MemoryInfo {
            total_bytes: 8 * 1024 * 1024 * 1024,
            available_bytes: 2 * 1024 * 1024 * 1024,
        };
        assert!((info.free_percent() - 25.0).abs() < 1e-9);
        assert!((info.used_percent() - 75.0).abs() < 1e-9);
        // 总内存为 0 时不做除法，直接给 0，避免 NaN 传进判定。
        let empty = MemoryInfo {
            total_bytes: 0,
            available_bytes: 0,
        };
        assert_eq!(empty.free_percent(), 0.0);
    }

    #[test]
    fn memory_usage_reports_sane_numbers() {
        let Some(usage) = memory_usage() else {
            // 极端受限环境拿不到系统内存信息时跳过（判定函数本身已处理 None）。
            eprintln!("跳过：当前平台未提供系统内存信息");
            return;
        };
        assert!(usage.total_bytes > 0);
        assert!(usage.available_bytes <= usage.total_bytes);
        assert!((0.0..=100.0).contains(&usage.free_percent), "{usage:?}");
        assert!((0.0..=100.0).contains(&usage.used_percent), "{usage:?}");
        assert!(usage.process_bytes <= usage.total_bytes, "{usage:?}");
        assert!((0.0..=100.0).contains(&usage.process_percent), "{usage:?}");
        // 进程占用不可能超过系统已用量太多（留 5% 余量容忍采样时刻差异）。
        assert!(
            usage.process_percent <= usage.used_percent + 5.0,
            "{usage:?}"
        );
    }

    /// `/proc/meminfo` 解析：优先 `MemAvailable`，老内核退化 `MemFree`，缺 `MemTotal` 视为失败。
    #[test]
    fn parse_meminfo_prefers_available_and_falls_back_to_free() {
        let modern = "MemTotal:       16384000 kB\nMemFree:         2000000 kB\nMemAvailable:    9000000 kB\n";
        assert_eq!(parse_meminfo_kb(modern), Some((16_384_000, 9_000_000)));

        // 老内核（< 3.14）没有 MemAvailable。
        let legacy = "MemTotal:        2048000 kB\nMemFree:          512000 kB\n";
        assert_eq!(parse_meminfo_kb(legacy), Some((2_048_000, 512_000)));

        // 脏数据里可用内存大于总量时夹回总量。
        let odd = "MemTotal: 1000 kB\nMemAvailable: 2000 kB\n";
        assert_eq!(parse_meminfo_kb(odd), Some((1000, 1000)));

        assert_eq!(parse_meminfo_kb("MemFree: 1 kB\n"), None);
        assert_eq!(parse_meminfo_kb(""), None);
    }

    /// `/proc/self/statm` 取第二列（常驻页数），格式异常返回 `None`。
    #[test]
    fn parse_statm_reads_resident_pages() {
        assert_eq!(
            parse_statm_resident_pages("2048 512 300 10 0 90 0\n"),
            Some(512)
        );
        // 只有一列、非数字、空串都要安全返回 None（不 panic）。
        assert_eq!(parse_statm_resident_pages("512"), None);
        assert_eq!(parse_statm_resident_pages("bad tokens here"), None);
        assert_eq!(parse_statm_resident_pages(""), None);
    }

    #[test]
    fn mem_threshold_ignores_limits_at_or_below_50() {
        // 阈值 ≤ 50 时直接不采样。
        assert!(!mem_over_threshold(0));
        assert!(!mem_over_threshold(50));
        assert!(!mem_over_threshold(-10));
        // 100% 阈值下两个条件都不成立，恒为 false。
        assert!(!mem_over_threshold(100));
    }
}

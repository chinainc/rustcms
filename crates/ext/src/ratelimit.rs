//! 限流器与 IP 屏蔽名单。
//!
//! 对齐 Go 版 `internal/ratelimit` 的对外语义：令牌桶控制请求频率，
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
    /// 对齐 Go `Limiter.GetBlockIPs` 返回的 `BlockIP{ip, block_time}`。
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
}

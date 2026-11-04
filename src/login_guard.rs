//! 后台登录失败次数限制。
//!
//! 对齐 Go 版 `controller/manageController/admin.go` 的 `forbidden-admin-` 计数：
//! 同一 IP / 同一账号连续失败 [`MAX_TIMES`] 次后锁定 [`LOCK_TTL`]（每次失败重新计时）。
//!
//! Go 版把计数放在站点缓存里（`currentSite.Cache`），单进程的 Rust 实现直接放进程内存：
//! 重启即清零，语义上等同于「缓存丢了也无所谓」，不引入外部依赖。

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// 连续失败多少次后锁定。
pub const MAX_TIMES: u32 = 5;

/// 锁定（以及每次失败后重新计时）的时长，与 Go 版的 `Cache.Set(..., 600)` 一致。
pub const LOCK_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy)]
struct Attempt {
    times: u32,
    expire_at: Instant,
}

/// 登录失败计数器（进程内，按 IP / 用户名分别计数）。
#[derive(Debug, Default)]
pub struct LoginGuard {
    attempts: Mutex<HashMap<String, Attempt>>,
}

impl LoginGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// IP 计数键。
    pub fn ip_key(ip: &str) -> String {
        format!("forbidden-admin-ip-{ip}")
    }

    /// 账号计数键。
    pub fn user_key(user_name: &str) -> String {
        format!("forbidden-admin-user-{user_name}")
    }

    /// 是否已被锁定（顺带清掉已过期的记录）。
    pub fn locked(&self, key: &str) -> bool {
        let mut attempts = self.lock();
        match attempts.get(key) {
            Some(attempt) if attempt.expire_at > Instant::now() => attempt.times >= MAX_TIMES,
            Some(_) => {
                attempts.remove(key);
                false
            }
            None => false,
        }
    }

    /// 记一次失败；`expire_at` 每次都重置，避免慢慢试错绕过锁定。
    pub fn fail(&self, key: &str) {
        let now = Instant::now();
        let mut attempts = self.lock();
        let entry = attempts.entry(key.to_string()).or_insert(Attempt {
            times: 0,
            expire_at: now + LOCK_TTL,
        });
        entry.times = entry.times.saturating_add(1);
        entry.expire_at = now + LOCK_TTL;
    }

    /// 清掉计数（登录成功后调用）。
    pub fn reset(&self, key: &str) {
        self.lock().remove(key);
    }

    /// 清空全部计数（手动解锁 / 测试用）。
    #[allow(dead_code)] // 暂无后台入口，留给运维手动解锁
    pub fn clear(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Attempt>> {
        self.attempts.lock().unwrap_or_else(|err| err.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_after_max_failures() {
        let guard = LoginGuard::new();
        let key = LoginGuard::user_key("lock-admin");

        assert!(!guard.locked(&key));
        for _ in 0..MAX_TIMES - 1 {
            guard.fail(&key);
            assert!(!guard.locked(&key), "未到上限不应锁定");
        }

        guard.fail(&key);
        assert!(guard.locked(&key), "第 {MAX_TIMES} 次失败后应锁定");

        // 只影响这个键，别的账号/IP 不受牵连
        assert!(!guard.locked(&LoginGuard::user_key("other-admin")));
        assert!(!guard.locked(&LoginGuard::ip_key("127.0.0.1")));
    }

    #[test]
    fn reset_unlocks_and_clear_wipes_everything() {
        let guard = LoginGuard::new();
        let key = LoginGuard::ip_key("10.0.0.1");

        for _ in 0..MAX_TIMES {
            guard.fail(&key);
        }
        assert!(guard.locked(&key));

        guard.reset(&key);
        assert!(!guard.locked(&key));

        let other = LoginGuard::user_key("admin");
        for _ in 0..MAX_TIMES {
            guard.fail(&other);
        }
        assert!(guard.locked(&other));
        guard.clear();
        assert!(!guard.locked(&other));
    }

    #[test]
    fn expired_attempts_are_forgotten() {
        let guard = LoginGuard::new();
        let key = LoginGuard::ip_key("10.0.0.2");

        for _ in 0..MAX_TIMES {
            guard.fail(&key);
        }
        assert!(guard.locked(&key));

        // 直接把过期时间拨到过去，模拟锁定窗口结束
        {
            let mut attempts = guard.lock();
            if let Some(attempt) = attempts.get_mut(&key) {
                attempt.expire_at = Instant::now() - Duration::from_secs(1);
            }
        }
        assert!(!guard.locked(&key), "过期的失败计数应立即失效");
        // 过期记录会被顺手清掉
        assert!(guard.lock().get(&key).is_none());
    }
}

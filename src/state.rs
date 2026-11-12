//! 全局应用状态（配置 + 数据库池 + 运行时元信息）。

use std::sync::{Arc, RwLock};
use std::time::Instant;

use sqlx::mysql::MySqlPool;

use crate::config::ServerJson;
use crate::login_guard::LoginGuard;
use crate::template::Engine;

struct Inner {
    config: RwLock<ServerJson>,
    db: Option<MySqlPool>,
    templates: Engine,
    login_guard: LoginGuard,
    started_at: Instant,
    version: &'static str,
}

/// 共享状态，克隆代价极低（内部是 Arc）。
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

impl AppState {
    pub fn new(config: ServerJson, db: Option<MySqlPool>) -> Self {
        let templates = Engine::new(&config.template_dir);
        // 记录模板目录：`tr` 标签按它定位语言包（locales/<lang>/default.yml）
        crate::template::set_template_dir(std::path::Path::new(&config.template_dir));
        Self {
            inner: Arc::new(Inner {
                config: RwLock::new(config),
                db,
                templates,
                login_guard: LoginGuard::new(),
                started_at: Instant::now(),
                version: env!("CARGO_PKG_VERSION"),
            }),
        }
    }

    /// 模板引擎（按目录缓存已解析模板）。
    pub fn templates(&self) -> &Engine {
        &self.inner.templates
    }

    /// 后台登录失败计数器。
    pub fn login_guard(&self) -> &LoginGuard {
        &self.inner.login_guard
    }

    /// 只读访问配置（不要跨 await 持有锁）。
    pub fn with_config<R>(&self, f: impl FnOnce(&ServerJson) -> R) -> R {
        let guard = self.inner.config.read().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }

    /// 替换配置（热更新/后台保存时使用）。
    #[allow(dead_code)] // 供后台保存配置后热更新
    pub fn replace_config(&self, config: ServerJson) {
        let mut guard = self.inner.config.write().unwrap_or_else(|e| e.into_inner());
        *guard = config;
    }

    #[allow(dead_code)] // 供数据层取连接池
    pub fn db(&self) -> Option<&MySqlPool> {
        self.inner.db.as_ref()
    }

    pub fn has_db(&self) -> bool {
        self.inner.db.is_some()
    }

    /// 已运行秒数。
    pub fn uptime_secs(&self) -> u64 {
        self.inner.started_at.elapsed().as_secs()
    }

    pub fn version(&self) -> &'static str {
        self.inner.version
    }
}

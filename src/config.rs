//! 配置文件（config.json）读写。
//!
//! 结构固定为 `mysql` / `server` 两段，
//! 同一个 config.json 可以直接复用：
//!
//! ```json
//! {
//! "mysql": { "database": "rustcms", "user": "root", "password": "", "host": "127.0.0.1", "port": 3306 },
//! "server": { "env": "production", "port": 9001, "log_level": "error" }
//! }
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 默认配置文件名。
pub const CONFIG_FILE: &str = "config.json";
/// 用环境变量覆盖配置文件路径。
pub const ENV_CONFIG_PATH: &str = "RUSTCMS_CONFIG";

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_db_port() -> u16 {
    3306
}

fn default_env() -> String {
    "production".to_string()
}

fn default_port() -> u16 {
    9001
}

fn default_log_level() -> String {
    "error".to_string()
}

fn default_template_dir() -> String {
    "./template/default".to_string()
}

fn default_public_dir() -> String {
    "./public".to_string()
}

/// MySQL 连接信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MysqlConfig {
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_db_port")]
    pub port: u16,
    /// 复用默认站点（ID=1）的数据库账号密码。
    #[serde(default)]
    pub use_default: bool,
}

impl Default for MysqlConfig {
    fn default() -> Self {
        Self {
            database: String::new(),
            user: String::new(),
            password: String::new(),
            host: default_host(),
            port: default_db_port(),
            use_default: false,
        }
    }
}

impl MysqlConfig {
    /// 是否已填写到足以尝试连接的程度。
    pub fn is_configured(&self) -> bool {
        !self.database.trim().is_empty() && !self.host.trim().is_empty()
    }
}

/// HTTP 服务配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// production / development
    #[serde(default = "default_env")]
    pub env: String,
    /// 监听端口：优先读 `config.json` 的 `server.port`；
    /// 文件里没写时自动补 9001 并写回（真正以配置文件为准）。
    #[serde(default = "default_port")]
    pub port: u16,
    /// error / warn / info / debug / trace（`release` 视作 info）
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            env: default_env(),
            port: default_port(),
            log_level: default_log_level(),
        }
    }
}

/// 根配置。未知字段会被忽略，便于直接复用既有配置文件。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerJson {
    #[serde(default)]
    pub mysql: MysqlConfig,
    #[serde(default)]
    pub server: ServerConfig,
    /// 写入接口令牌：未配置时 `/api/archives` 的 POST/PUT/DELETE 一律拒绝。
    #[serde(default)]
    pub api_token: Option<String>,
    /// 模板目录（相对工作目录或绝对路径）
    #[serde(default = "default_template_dir")]
    pub template_dir: String,
    /// 静态文件目录（uploads 落盘在这里，默认 `./public`）
    #[serde(default = "default_public_dir")]
    pub public_dir: String,
    /// 配置来源路径，不参与序列化。
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

/// 启动时的配置加载结果。
pub enum LoadOutcome {
    /// 数据库已配置，按正常模式运行。
    Ready(ServerJson),
    /// 没有 `config.json` 或没配数据库：进入安装模式。
    NeedsInstall(ServerJson),
}

/// 默认要写入的 `config.json` 路径：优先可执行文件同目录，其次当前目录。
pub fn default_config_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join(CONFIG_FILE);
        }
    }
    std::env::current_dir()
        .map(|dir| dir.join(CONFIG_FILE))
        .unwrap_or_else(|_| PathBuf::from(CONFIG_FILE))
}

impl ServerJson {
    /// 依次尝试：`RUSTCMS_CONFIG`、可执行文件同目录、当前工作目录。
    pub fn locate() -> Option<PathBuf> {
        if let Ok(custom) = std::env::var(ENV_CONFIG_PATH) {
            let custom = custom.trim();
            if !custom.is_empty() {
                return Some(PathBuf::from(custom));
            }
        }
        let mut candidates = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join(CONFIG_FILE));
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join(CONFIG_FILE));
        }
        candidates.into_iter().find(|p| p.is_file())
    }

    /// 启动用配置加载：端口以 `config.json` 为准，缺省自动补 9001 并写回。
    ///
    /// - 没有配置文件：返回 `NeedsInstall`（端口先用默认 9001），不写文件；
    /// - 有文件但 `server` 段没写 `port`（或写成 0）：补成 9001 并写回文件；
    /// - 数据库未配置（`mysql.database` 为空）：返回 `NeedsInstall`，进入安装模式。
    pub fn load_for_start() -> Result<LoadOutcome> {
        let Some(path) = Self::locate() else {
            let mut cfg = Self::default();
            cfg.server.port = default_port();
            cfg.template_dir = "./template/default".to_string();
            cfg.public_dir = "./public".to_string();
            cfg.path = Some(default_config_path());
            return Ok(LoadOutcome::NeedsInstall(cfg));
        };
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("读取配置文件失败：{}", path.display()))?;
        let mut cfg: ServerJson = serde_json::from_str(&raw)
            .with_context(|| format!("配置文件格式错误：{}", path.display()))?;
        // 写回时直接改原始 JSON，保留用户配置里的其他字段
        let mut value: Value = serde_json::from_str(&raw)
            .with_context(|| format!("配置文件格式错误：{}", path.display()))?;
        let has_port = value
            .get("server")
            .and_then(|server| server.get("port"))
            .is_some();
        if !has_port || cfg.server.port == 0 {
            cfg.server.port = default_port();
            if !value.get("server").map(Value::is_object).unwrap_or(false) {
                value["server"] = json!({});
            }
            value["server"]["port"] = json!(cfg.server.port);
            match serde_json::to_string_pretty(&value) {
                Ok(text) => match fs::write(&path, text) {
                    Ok(()) => eprintln!(
                        "[!] {} 未配置 server.port，已自动写入 {}（可在文件中修改）",
                        path.display(),
                        cfg.server.port
                    ),
                    Err(err) => eprintln!("[!] 自动补 server.port 写回失败：{err}"),
                },
                Err(err) => eprintln!("[!] 自动补 server.port 失败：{err}"),
            }
        }
        cfg.path = Some(path);
        if cfg.mysql.is_configured() {
            Ok(LoadOutcome::Ready(cfg))
        } else {
            Ok(LoadOutcome::NeedsInstall(cfg))
        }
    }

    /// 写回配置文件（保留两空格缩进风格）。
    #[allow(dead_code)] // 供后台「保存配置」使用
    pub fn save(&self) -> Result<()> {
        let path = self
            .path
            .clone()
            .or_else(|| std::env::current_dir().ok().map(|d| d.join(CONFIG_FILE)))
            .context("无法确定配置文件写入路径")?;
        let text = serde_json::to_string_pretty(self)?;
        fs::write(&path, text).with_context(|| format!("写入配置文件失败：{}", path.display()))?;
        Ok(())
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// 开发模式（development/dev/local）。
    #[allow(dead_code)] // 供调试模式下的错误页/热重载开关使用
    pub fn is_dev(&self) -> bool {
        matches!(
            self.server.env.trim().to_ascii_lowercase().as_str(),
            "development" | "dev" | "local" | "debug"
        )
    }

    /// 监听端口。
    pub fn port(&self) -> u16 {
        self.server.port
    }

    /// 静态文件目录；配置为空时退回 `./public`。
    pub fn public_dir(&self) -> PathBuf {
        let dir = self.public_dir.trim();
        if dir.is_empty() {
            PathBuf::from("./public")
        } else {
            PathBuf::from(dir)
        }
    }

    /// 运行时数据目录（`<config.json 所在目录>/data`），技能等文件放这里。
    pub fn data_dir(&self) -> PathBuf {
        match self.path.as_ref().and_then(|path| path.parent()) {
            Some(parent) => parent.join("data"),
            None => PathBuf::from("data"),
        }
    }

    /// 缓存目录（`<config.json 所在目录>/cache`）。
    pub fn cache_dir(&self) -> PathBuf {
        match self.path.as_ref().and_then(|path| path.parent()) {
            Some(parent) => parent.join("cache"),
            None => PathBuf::from("cache"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_defaults_for_missing_fields() {
        let cfg: ServerJson = serde_json::from_str(r#"{"server":{"port":9000}}"#).unwrap();
        assert_eq!(cfg.port(), 9000);
        assert_eq!(cfg.server.env, "production");
        assert_eq!(cfg.server.log_level, "error");
        assert_eq!(cfg.mysql.host, "127.0.0.1");
        assert_eq!(cfg.mysql.port, 3306);
        assert!(!cfg.mysql.is_configured());
    }

    #[test]
    fn ignores_unknown_fields_of_go_config() {
        let cfg: ServerJson = serde_json::from_str(
            r#"{"mysql":{"database":"cms","user":"root"},"server":{"env":"dev","port":9000},
                "sparks":[{"name":"any"}],"unknown":{"a":1}}"#,
        )
        .unwrap();
        assert!(cfg.mysql.is_configured());
        assert!(cfg.is_dev());
    }

    #[test]
    fn missing_port_falls_back_to_9001() {
        // 端口以 config.json 为准：server 段没写 port 时用默认 9001
        let cfg: ServerJson =
            serde_json::from_str(r#"{"server":{"env":"production","log_level":"error"}}"#).unwrap();
        assert_eq!(cfg.port(), 9001);
    }

    #[test]
    fn default_uses_port_9001() {
        let cfg = ServerJson::default();
        assert_eq!(cfg.port(), 9001);
        assert!(cfg.path().is_none());
    }

    #[test]
    fn save_roundtrip() {
        let dir = std::env::temp_dir().join(format!("rustcms-cfg-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CONFIG_FILE);

        let cfg = ServerJson {
            path: Some(path.clone()),
            server: ServerConfig {
                port: 8123,
                ..Default::default()
            },
            mysql: MysqlConfig {
                database: "cms".into(),
                ..Default::default()
            },
            api_token: None,
            ..Default::default()
        };
        cfg.save().unwrap();

        let reloaded: ServerJson =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(reloaded.port(), 8123);
        assert_eq!(reloaded.mysql.database, "cms");

        fs::remove_dir_all(&dir).ok();
    }
}

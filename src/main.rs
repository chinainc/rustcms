//! rustcms：基于 Rust 的内容管理系统。
//!
//! 启动链路：读取 config.json → 初始化日志 → 连接 MySQL → 启动 HTTP 服务。

mod api_admin_gap;
mod api_content;
mod api_interact;
mod api_member;
mod api_trade;
mod auth;
mod config;
mod crond;
mod db;
mod front;
mod install;
mod login_guard;
mod middleware;
mod models;
mod routes;
mod server;
mod state;
mod template;

use tracing_subscriber::EnvFilter;

use config::{LoadOutcome, ServerJson};

#[tokio::main]
async fn main() {
    // 命令行选项先处理：`--hash-password` 不需要配置与数据库，直接退出。
    if let Some(code) = handle_cli() {
        std::process::exit(code);
    }

    let outcome = match ServerJson::load_for_start() {
        Ok(outcome) => outcome,
        Err(err) => {
            eprintln!("启动失败：{err:#}");
            std::process::exit(1);
        }
    };

    match outcome {
        LoadOutcome::Ready(config) => run_server(config).await,
        LoadOutcome::NeedsInstall(config) => {
            init_tracing(&config.server.log_level);
            eprintln!("[!] 未检测到数据库配置，进入安装模式");
            let state = state::AppState::new(config, None);
            if let Err(err) = install::run(state).await {
                tracing::error!("{err:#}");
                eprintln!("安装模式启动失败：{err:#}");
                std::process::exit(1);
            }
            // 安装完成：重新加载配置，同一进程直接进入正式服务，无需重启。
            match ServerJson::load_for_start() {
                Ok(LoadOutcome::Ready(config)) => run_server(config).await,
                Ok(LoadOutcome::NeedsInstall(_)) => {
                    eprintln!("安装未完成：config.json 仍缺少数据库配置，请访问 /install 完成安装");
                    std::process::exit(1);
                }
                Err(err) => {
                    eprintln!("启动失败：{err:#}");
                    std::process::exit(1);
                }
            }
        }
    }
}

/// 正常模式：连库并启动 HTTP 服务。
async fn run_server(config: ServerJson) {
    init_tracing(&config.server.log_level);

    tracing::info!(
        "rustcms {} 启动中（env={}，port={}）",
        env!("CARGO_PKG_VERSION"),
        config.server.env,
        config.server.port
    );

    let pool = match db::connect(&config.mysql).await {
        Ok(pool) => pool,
        Err(err) => {
            tracing::error!("{err:#}");
            std::process::exit(1);
        }
    };

    let state = state::AppState::new(config, pool);

    if let Err(err) = server::run(state).await {
        tracing::error!("{err:#}");
        std::process::exit(1);
    }
}

/// 处理不依赖配置与数据库的命令行选项。
///
/// 返回 `Some(退出码)` 表示选项已处理完、应直接退出；`None` 表示按服务方式启动。
fn handle_cli() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    let arg = args.next()?;

    match arg.as_str() {
        "--hash-password" => {
            let Some(plain) = args.next() else {
                eprintln!("用法：rustcms --hash-password <明文密码>");
                return Some(2);
            };
            Some(match auth::hash_password(&plain) {
                Ok(hash) => {
                    println!("{hash}");
                    0
                }
                Err(err) => {
                    eprintln!("生成哈希失败：{err:#}");
                    1
                }
            })
        }
        "-h" | "--help" => {
            println!(
                "rustcms {}\n\n\
                 用法：\n  \
                 rustcms                    按 config.json 启动服务\n  \
                 rustcms --hash-password P  输出 P 的 bcrypt 哈希（写后台管理员密码用）",
                env!("CARGO_PKG_VERSION")
            );
            Some(0)
        }
        other => {
            eprintln!("未知参数：{other}（用 --help 查看用法）");
            Some(2)
        }
    }
}

/// 初始化日志。`RUST_LOG` 优先于配置里的 `log_level`。
fn init_tracing(log_level: &str) {
    let filter = std::env::var("RUST_LOG")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default_filter(log_level).to_string());

    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter))
        .with_target(false)
        .try_init();
}

/// `release` 视作 info。
fn default_filter(log_level: &str) -> &'static str {
    match log_level.trim().to_ascii_lowercase().as_str() {
        "error" => "error",
        "warn" | "warning" => "warn",
        "debug" => "debug",
        "trace" => "trace",
        _ => "info",
    }
}

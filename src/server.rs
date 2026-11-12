//! HTTP 服务启动与优雅退出。

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::middleware as mw;
use tower_http::trace::TraceLayer;

use crate::middleware::{cors, security_headers, utf8_charset};
use crate::routes;
use crate::state::AppState;

/// 端口被占用时的探测结果。
enum Occupant {
    /// 已有另一个 rustcms 实例在监听。
    RustcmsInstance,
    /// 有服务在监听，但不是 rustcms。
    OtherService,
    /// 探测不到任何服务（多为 TIME_WAIT 之类的瞬时占用）。
    Transient,
}

/// 绑定监听端口；被占用时探测原因并给出可操作提示，瞬时占用自动重试。
///
/// - 已有另一个 rustcms 实例在跑：直接失败并提示如何停止旧进程；
/// - 其他程序占用：直接失败并提示查看占用进程 / 换端口；
/// - 探测不到监听者（TIME_WAIT 等）：每秒重试，最长 75 秒后仍失败才报错。
pub async fn bind_with_hint(port: u16) -> Result<tokio::net::TcpListener> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    const RETRY_SECS: u64 = 75;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(RETRY_SECS);
    let mut hinted = false;

    loop {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => return Ok(listener),
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
                if !hinted {
                    hinted = true;
                    match probe_occupant(port).await {
                        Occupant::RustcmsInstance => anyhow::bail!(
                            "端口 {port} 已被另一个 rustcms 实例占用：请先停止旧进程（例如 `pkill rustcms`，或用 `ss -lntp | grep {port}` 找到进程后 kill）。若旧实例仍处于安装模式，也可以直接访问 http://127.0.0.1:{port}/install 完成安装。"
                        ),
                        Occupant::OtherService => anyhow::bail!(
                            "端口 {port} 已被其他程序占用：请停止占用程序或修改 config.json 的 server.port。可用 `ss -lntp | grep {port}` 查看占用进程。"
                        ),
                        Occupant::Transient => tracing::warn!(
                            "端口 {port} 暂时无法绑定（多为旧连接的 TIME_WAIT），将自动重试最多 {RETRY_SECS} 秒…"
                        ),
                    }
                }
                if tokio::time::Instant::now() >= deadline {
                    anyhow::bail!(
                        "端口 {port} 监听失败（Address already in use）：重试 {RETRY_SECS} 秒后仍无法绑定，请检查占用进程"
                    );
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(err) => {
                return Err(err).with_context(|| format!("端口 {port} 监听失败"));
            }
        }
    }
}

/// 探测端口占用者类型：向本机该端口发一次 HTTP 请求并识别响应。
async fn probe_occupant(port: u16) -> Occupant {
    let url = format!("http://127.0.0.1:{port}/");
    let request = rustcms_ext::http::Request::get(url)
        .with_timeout(Duration::from_secs(3))
        .with_follow_redirects(true);
    match rustcms_ext::http::send(&request).await {
        Ok(response) => {
            if response.text().to_ascii_lowercase().contains("rustcms") {
                Occupant::RustcmsInstance
            } else {
                Occupant::OtherService
            }
        }
        Err(_) => Occupant::Transient,
    }
}

/// 组装完整应用（路由 + 全局中间件），便于测试直接调用。
pub fn build_app(state: AppState) -> axum::Router {
    routes::router(state)
        .layer(mw::from_fn(security_headers))
        .layer(mw::from_fn(utf8_charset))
        .layer(mw::from_fn(cors))
        .layer(TraceLayer::new_for_http())
}

/// 启动 HTTP 服务，直到收到退出信号。
pub async fn run(state: AppState) -> Result<()> {
    let port = state.with_config(|c| c.port());
    if port == 0 {
        anyhow::bail!(
            "server.port 未配置：请在 config.json 的 server 段填写监听端口（例如 \"port\": 9001）"
        );
    }

    // 进程内限流器只在内存里保存状态，重启后按 settings.limiter 重新装载
    if let Some(pool) = state.db().cloned() {
        if let Err(err) = crate::models::plugin::limiter_reload(&pool).await {
            tracing::warn!("加载限流配置失败：{err:#}");
        }
    }

    let app = build_app(state.clone());

    // 进程内定时任务：定时发布、回收站清理
    crate::crond::start(state.clone());

    let listener = bind_with_hint(port).await?;

    tracing::info!("HTTP 服务已启动：http://127.0.0.1:{port}");
    tracing::info!(
        "配置来源：{}，数据库：{}",
        state.with_config(|c| c
            .path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<默认值>".to_string())),
        if state.has_db() {
            "已连接"
        } else {
            "未配置"
        }
    );

    // 带上 TCP 对端地址：后台登录的失败计数与登录日志要用它当客户端 IP。
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("HTTP 服务异常退出")?;

    tracing::info!("服务已停止");
    Ok(())
}

/// 等待 Ctrl+C（Unix 下同时处理 SIGTERM）。
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!("无法监听 Ctrl+C：{err}");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => tracing::error!("无法监听 SIGTERM：{err}"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("收到退出信号，正在关闭服务…");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::config::ServerJson;

    fn app() -> axum::Router {
        build_app(AppState::new(ServerJson::default(), None))
    }

    #[tokio::test]
    async fn applies_security_and_cors_headers() {
        let res = app()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .header("origin", "http://example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::OK);
        let headers = res.headers();
        assert_eq!(headers["x-frame-options"], "SAMEORIGIN");
        assert_eq!(
            headers["referrer-policy"],
            "strict-origin-when-cross-origin"
        );
        assert_eq!(headers["access-control-allow-origin"], "http://example.com");
        assert_eq!(headers["access-control-allow-credentials"], "true");
        assert_eq!(headers["vary"], "Origin");
    }

    #[tokio::test]
    async fn json_response_declares_utf8_charset() {
        let res = app()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.headers()["content-type"],
            "application/json; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn answers_preflight_without_hitting_route() {
        let res = app()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/health")
                    .header("origin", "http://example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            res.headers()["access-control-allow-methods"],
            "GET,POST,PUT,DELETE,PATCH,OPTIONS"
        );
    }
}

//! HTTP 服务启动与优雅退出。

use std::net::SocketAddr;

use anyhow::{Context, Result};
use axum::middleware as mw;
use tower_http::trace::TraceLayer;

use crate::middleware::{cors, security_headers, utf8_charset};
use crate::routes;
use crate::state::AppState;

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
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    // 进程内限流器只在内存里保存状态，重启后按 settings.limiter 重新装载
    if let Some(pool) = state.db().cloned() {
        if let Err(err) = crate::models::plugin::limiter_reload(&pool).await {
            tracing::warn!("加载限流配置失败：{err:#}");
        }
    }

    let app = build_app(state.clone());

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("端口 {port} 监听失败（可能已被占用）"))?;

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

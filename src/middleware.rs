//! 全局中间件：CORS 与安全响应头。
//!
//! 语义对齐 Go 版 `middleware/options.go`（回显 Origin + 允许携带凭据）
//! 与 `middleware/secure.go`（防点击劫持 / Referer / 浏览器功能权限）。

use axum::extract::Request;
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

const ALLOW_HEADERS: &str =
    "Content-Type, Api, Accept, Authorization, Version, Admin, Token, Key, \
                             Site-Id, Sub-Site-Id, X-Site-Id, X-Pjax, X-Pjax-Container";
const ALLOW_METHODS: &str = "GET,POST,PUT,DELETE,PATCH,OPTIONS";
const EXPOSE_HEADERS: &str = "Content-Disposition,Update-Token";

/// 跨域处理：回显 Origin、允许凭据，预检请求直接返回 204。
pub async fn cors(req: Request, next: Next) -> Response {
    let origin = req.headers().get(header::ORIGIN).cloned();
    let is_preflight = req.method() == Method::OPTIONS;

    let mut response = if is_preflight {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(req).await
    };

    let headers = response.headers_mut();
    if let Some(origin) = origin {
        // 响应内容随 Origin 变化，必须声明 Vary，否则中间层缓存可能串用。
        headers.insert(header::VARY, HeaderValue::from_static("Origin"));
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static(EXPOSE_HEADERS),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static(ALLOW_METHODS),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(ALLOW_HEADERS),
    );
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );

    response
}

/// 给 JSON / 文本响应补上 `charset=utf-8`。
///
/// `axum` 的 `Json` 只写 `application/json`，而 Go 版（Iris）会带 `; charset=UTF-8`；
/// Windows PowerShell 5.1 等老客户端缺了 charset 会按 Latin-1 解码，中文全成乱码。
pub async fn utf8_charset(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;

    let current = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());

    if let Some(value) = current {
        let lower = value.to_ascii_lowercase();
        let needs_charset = (lower.starts_with("application/json") || lower.starts_with("text/"))
            && !lower.contains("charset=");
        if needs_charset {
            if let Ok(header) = HeaderValue::from_str(&format!("{value}; charset=utf-8")) {
                response.headers_mut().insert(header::CONTENT_TYPE, header);
            }
        }
    }

    response
}

/// 安全响应头。
pub async fn security_headers(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("SAMEORIGIN"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("geolocation=(), microphone=(), camera=()"),
    );
    response
}

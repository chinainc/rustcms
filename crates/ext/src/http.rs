//! HTTP 客户端。
//!
//! 提供统一入口 [`send`]，内部有两个后端：
//!
//! | 后端 | 能力 | 依赖 |
//! |---|---|---|
//! | [`Backend::Curl`] | http/https、重定向、代理、gzip、任意方法与请求头 | 系统 `curl` 命令 |
//! | [`Backend::Native`] | 仅明文 `http://`，够用且零外部依赖 | 仅标准库 |
//!
//! 默认用 [`Backend::Auto`]：优先 curl（探测一次并缓存结果），不可用时退回原生。
//!
//! > 为什么不直接用 reqwest：离线构建环境拿不到新依赖，且 TLS 栈会显著抬高
//! > 编译内存峰值；这一层的设计目标就是「零新依赖、可在低内存机器上编译」。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use tokio::task::spawn_blocking;

/// curl 写响应体时附带的元信息分隔标记。
const META_MARK: &str = "\n<<<RUSTCMS_META>>>";

/// HTTP 方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Patch,
}

impl Method {
    /// 方法名（大写）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Patch => "PATCH",
        }
    }
}

/// 请求体。
#[derive(Debug, Clone, Default)]
pub enum Body {
    /// 无请求体。
    #[default]
    Empty,
    /// 原始字节。
    Bytes(Vec<u8>),
    /// `application/x-www-form-urlencoded` 表单。
    Form(Vec<(String, String)>),
    /// JSON（自动带 `Content-Type: application/json`）。
    Json(serde_json::Value),
}

impl Body {
    /// 编码成字节，并给出需要附带的 `Content-Type`。
    pub fn encode(&self) -> (Vec<u8>, Option<&'static str>) {
        match self {
            Body::Empty => (Vec::new(), None),
            Body::Bytes(raw) => (raw.clone(), None),
            Body::Form(pairs) => (
                form_encode(pairs).into_bytes(),
                Some("application/x-www-form-urlencoded"),
            ),
            Body::Json(value) => (
                serde_json::to_vec(value).unwrap_or_default(),
                Some("application/json"),
            ),
        }
    }
}

/// `application/x-www-form-urlencoded` 编码。
pub fn form_encode(pairs: &[(String, String)]) -> String {
    let mut out = String::new();
    for (key, value) in pairs {
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(&percent_encode(key));
        out.push('=');
        out.push_str(&percent_encode(value));
    }
    out
}

/// 百分号编码（`application/x-www-form-urlencoded` 口径：空格变 `+`）。
fn percent_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 一次 HTTP 请求。
#[derive(Debug, Clone)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Body,
    pub timeout: Duration,
    /// 是否跟随重定向（curl 后端交给 curl，native 后端自己实现）。
    pub follow_redirects: bool,
    /// native 后端最多跟随的跳数。
    pub max_redirects: usize,
    /// 后端选择（默认 [`Backend::Auto`]）。
    pub backend: Backend,
}

impl Request {
    /// 新建请求（默认 GET、10 秒超时、跟随重定向）。
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: Body::Empty,
            timeout: Duration::from_secs(10),
            follow_redirects: true,
            max_redirects: 5,
            backend: Backend::Auto,
        }
    }

    /// GET 请求。
    pub fn get(url: impl Into<String>) -> Self {
        Self::new(Method::Get, url)
    }

    /// POST 请求。
    pub fn post(url: impl Into<String>) -> Self {
        Self::new(Method::Post, url)
    }

    /// 附加请求头。
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    /// 设置请求体。
    pub fn with_body(mut self, body: Body) -> Self {
        self.body = body;
        self
    }

    /// 设置 JSON 请求体。
    pub fn with_json(mut self, value: serde_json::Value) -> Self {
        self.body = Body::Json(value);
        self
    }

    /// 设置表单请求体。
    pub fn with_form(mut self, pairs: Vec<(String, String)>) -> Self {
        self.body = Body::Form(pairs);
        self
    }

    /// 设置超时。
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// 是否跟随重定向。
    pub fn with_follow_redirects(mut self, follow: bool) -> Self {
        self.follow_redirects = follow;
        self
    }

    /// 指定后端。
    pub fn with_backend(mut self, backend: Backend) -> Self {
        self.backend = backend;
        self
    }
}

/// 一次 HTTP 响应。
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// 最终 URL（经过重定向后的）。
    pub final_url: String,
}

impl Response {
    /// 响应体按 UTF-8 解码（非法字节用替代字符）。
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// 响应体解析成 JSON。
    pub fn json(&self) -> Result<serde_json::Value> {
        serde_json::from_slice(&self.body).context("响应不是合法 JSON")
    }

    /// 是否是 2xx。
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// 取响应头（不区分大小写）。
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// 后端选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// 优先 curl，不可用时退回 native。
    Auto,
    /// 只用系统 curl。
    Curl,
    /// 只用标准库实现。
    Native,
}

/// 探测系统里有没有 curl（结果缓存）。
pub fn curl_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::process::Command::new("curl")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

/// 发一次请求（自动挑选后端）。
pub async fn send(request: &Request) -> Result<Response> {
    match request.backend {
        Backend::Curl => send_with(request, Backend::Curl).await,
        Backend::Native => send_with(request, Backend::Native).await,
        Backend::Auto => {
            if curl_available() {
                match send_with(request, Backend::Curl).await {
                    Ok(response) => Ok(response),
                    Err(err) => {
                        tracing::debug!(error = %err, "curl 后端失败，尝试原生后端");
                        send_with(request, Backend::Native).await
                    }
                }
            } else {
                send_with(request, Backend::Native).await
            }
        }
    }
}

/// 指定后端发请求。
pub async fn send_with(request: &Request, backend: Backend) -> Result<Response> {
    match backend {
        Backend::Curl => curl_send(request).await,
        Backend::Native => native_send(request).await,
        Backend::Auto => bail!("Auto 后端请改用 send()"),
    }
}

/// 临时响应头文件名计数器。
fn header_counter() -> &'static AtomicU64 {
    static COUNTER: OnceLock<AtomicU64> = OnceLock::new();
    COUNTER.get_or_init(|| AtomicU64::new(0))
}

/// curl 后端：调用系统 curl 子进程。
async fn curl_send(request: &Request) -> Result<Response> {
    let cloned = request.clone();
    spawn_blocking(move || curl_send_blocking(&cloned))
        .await
        .context("curl 任务异常")?
}

/// curl 后端的阻塞实现。
fn curl_send_blocking(request: &Request) -> Result<Response> {
    let header_path = std::env::temp_dir().join(format!(
        "rustcms-http-{}-{}.hdr",
        std::process::id(),
        header_counter().fetch_add(1, Ordering::Relaxed)
    ));

    let mut command = std::process::Command::new("curl");
    command.arg("-sS");
    command
        .arg("--max-time")
        .arg(request.timeout.as_secs().max(1).to_string());
    if request.follow_redirects {
        command.arg("-L");
    }
    command.arg("-X").arg(request.method.as_str());
    command.arg("-D").arg(&header_path);

    let (body_bytes, content_type) = request.body.encode();
    if !body_bytes.is_empty() {
        command.arg("--data-binary").arg("@-");
        if let Some(value) = content_type {
            command.arg("-H").arg(format!("Content-Type: {value}"));
        }
    }
    for (key, value) in &request.headers {
        command.arg("-H").arg(format!("{key}: {value}"));
    }
    command.arg("-w").arg(format!("{META_MARK}%{{http_code}}|%{{url_effective}}"));
    command.arg(&request.url);

    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command.spawn().context("启动 curl 失败")?;
    if !body_bytes.is_empty() {
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(&body_bytes)
                .context("向 curl 写入请求体失败")?;
        }
    }
    // 关掉 stdin，否则 curl 会一直等输入
    drop(child.stdin.take());

    let output = child.wait_with_output().context("等待 curl 结束")?;
    let stdout = output.stdout;
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    let header_text = std::fs::read_to_string(&header_path).unwrap_or_default();
    let _ = std::fs::remove_file(&header_path);

    if !output.status.success() {
        let detail = if stderr.trim().is_empty() {
            format!("curl 退出码 {:?}", output.status.code())
        } else {
            stderr.trim().to_string()
        };
        bail!("请求失败：{detail}");
    }

    // 拆出元信息（状态码 + 最终 URL）
    let (body, meta) = match stdout.windows(META_MARK.len()).rposition(|w| w == META_MARK.as_bytes())
    {
        Some(position) => {
            let split = position + META_MARK.len();
            (stdout[..position].to_vec(), String::from_utf8_lossy(&stdout[split..]).to_string())
        }
        None => (stdout.clone(), String::new()),
    };
    let mut meta_parts = meta.splitn(2, '|');
    let status = meta_parts
        .next()
        .unwrap_or_default()
        .trim()
        .parse::<u16>()
        .unwrap_or(0);
    let final_url = meta_parts.next().unwrap_or_default().trim().to_string();

    Ok(Response {
        status,
        headers: parse_header_block(&header_text),
        body,
        final_url: if final_url.is_empty() {
            request.url.clone()
        } else {
            final_url
        },
    })
}

/// 解析 curl `-D` 落下的响应头：取最后一个响应块。
fn parse_header_block(text: &str) -> Vec<(String, String)> {
    let blocks: Vec<&str> = text
        .split("\r\n\r\n")
        .flat_map(|part| part.split("\n\n"))
        .collect();
    let Some(last) = blocks
        .iter()
        .rev()
        .find(|block| block.trim_start().to_ascii_uppercase().starts_with("HTTP/"))
    else {
        return Vec::new();
    };
    last.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            Some((key.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// 解析后的 URL。
#[derive(Debug, Clone)]
struct ParsedUrl {
    scheme: String,
    host: String,
    port: u16,
    /// 请求行里的路径（含查询串）。
    path: String,
    /// Host 头（保留端口）。
    host_header: String,
}

/// 拆解 URL。
fn parse_url(raw: &str) -> Result<ParsedUrl> {
    let (scheme, rest) = raw
        .split_once("://")
        .ok_or_else(|| anyhow!("URL 格式错误：{raw}"))?;
    let scheme = scheme.to_ascii_lowercase();
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    // 去掉 userinfo（user:pass@host）
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = if let Some(end) = authority.strip_prefix('[') {
        // IPv6 字面量
        let (host, tail) = end
            .split_once(']')
            .ok_or_else(|| anyhow!("IPv6 地址格式错误：{raw}"))?;
        let port = tail
            .strip_prefix(':')
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(if scheme == "https" { 443 } else { 80 });
        (host.to_string(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port))
                if !port.is_empty() && port.chars().all(|item| item.is_ascii_digit()) =>
            {
                (host.to_string(), port.parse::<u16>().unwrap_or(80))
            }
            _ => (
                authority.to_string(),
                if scheme == "https" { 443 } else { 80 },
            ),
        }
    };
    if host.is_empty() {
        bail!("URL 缺少主机名：{raw}");
    }
    Ok(ParsedUrl {
        scheme,
        host,
        port,
        path: path.to_string(),
        host_header: authority.to_string(),
    })
}

/// 把相对地址补成绝对地址。
fn resolve_location(base: &str, location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.to_string();
    }
    let Ok(parsed) = parse_url(base) else {
        return location.to_string();
    };
    if location.starts_with('/') {
        format!("{}://{}{}", parsed.scheme, parsed.host_header, location)
    } else {
        let dir = match parsed.path.rfind('/') {
            Some(index) => &parsed.path[..index + 1],
            None => "/",
        };
        format!("{}://{}{}{}", parsed.scheme, parsed.host_header, dir, location)
    }
}

/// 原生后端：标准库 HTTP/1.1 实现。
async fn native_send(request: &Request) -> Result<Response> {
    let cloned = request.clone();
    spawn_blocking(move || native_send_blocking(&cloned))
        .await
        .context("原生后端任务异常")?
}

/// 原生后端的阻塞实现（含重定向跟随）。
fn native_send_blocking(request: &Request) -> Result<Response> {
    let mut current = request.clone();
    let mut hops = 0usize;
    loop {
        let (status, headers, body) = native_round_trip(&current)?;
        let location = if request.follow_redirects
            && matches!(status, 301 | 302 | 303 | 307 | 308)
        {
            headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("location"))
                .map(|(_, value)| value.clone())
        } else {
            None
        };

        match location.filter(|value| !value.is_empty()) {
            Some(target) => {
                hops += 1;
                if hops > request.max_redirects {
                    bail!("重定向次数过多（超过 {} 次）", request.max_redirects);
                }
                current.url = resolve_location(&current.url, &target);
                // 与浏览器/curl 一致：303 以及 301/302 的 POST 都降级成 GET
                if status == 303
                    || ((status == 301 || status == 302) && current.method == Method::Post)
                {
                    current.method = Method::Get;
                    current.body = Body::Empty;
                    current.headers.retain(|(key, _)| {
                        !key.eq_ignore_ascii_case("content-type")
                            && !key.eq_ignore_ascii_case("content-length")
                    });
                }
            }
            None => {
                return Ok(Response {
                    status,
                    headers,
                    body,
                    final_url: current.url,
                })
            }
        }
    }
}

type HttpResponseParts = (u16, Vec<(String, String)>, Vec<u8>);

/// 原生后端的一次请求-响应。
fn native_round_trip(request: &Request) -> Result<HttpResponseParts> {
    let parsed = parse_url(&request.url)?;
    if parsed.scheme != "http" {
        bail!("原生后端仅支持 http://（https 请安装 curl，或改用 curl 后端）");
    }
    let address = format!("{}:{}", parsed.host, parsed.port);
    let mut stream = TcpStream::connect(&address).context(format!("连接 {address} 失败"))?;
    let _ = stream.set_read_timeout(Some(request.timeout));
    let _ = stream.set_write_timeout(Some(request.timeout));

    let (body_bytes, content_type) = request.body.encode();
    let mut head = format!(
        "{} {} HTTP/1.1\r\n",
        request.method.as_str(),
        parsed.path
    );
    head.push_str(&format!("Host: {}\r\n", parsed.host_header));
    head.push_str("User-Agent: rustcms\r\n");
    head.push_str("Accept: */*\r\n");
    head.push_str("Connection: close\r\n");
    if !body_bytes.is_empty() {
        if let Some(value) = content_type {
            head.push_str(&format!("Content-Type: {value}\r\n"));
        }
        head.push_str(&format!("Content-Length: {}\r\n", body_bytes.len()));
    }
    for (key, value) in &request.headers {
        head.push_str(&format!("{key}: {value}\r\n"));
    }
    head.push_str("\r\n");

    stream
        .write_all(head.as_bytes())
        .context("写入请求头失败")?;
    if !body_bytes.is_empty() {
        stream.write_all(&body_bytes).context("写入请求体失败")?;
    }
    stream.flush().context("刷新请求失败")?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).context("读取响应失败")?;
    parse_response_bytes(&raw)
}

/// 解析原始响应字节。
fn parse_response_bytes(raw: &[u8]) -> Result<HttpResponseParts> {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4))
        .or_else(|| {
            raw.windows(2)
                .position(|window| window == b"\n\n")
                .map(|index| (index, 2))
        })
        .ok_or_else(|| anyhow!("响应格式错误（没有头部结束标记）"))?;
    let head = String::from_utf8_lossy(&raw[..split.0]).to_string();
    let mut body = raw[split.0 + split.1..].to_vec();

    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| anyhow!("状态行解析失败：{status_line}"))?;

    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            if !key.is_empty() {
                headers.push((key.to_string(), value.trim().to_string()));
            }
        }
    }

    let chunked = headers
        .iter()
        .any(|(key, value)| {
            key.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
        });
    if chunked {
        body = decode_chunked(&body)?;
    }

    Ok((status, headers, body))
}

/// 解码 chunked 传输体。
fn decode_chunked(raw: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    loop {
        let line_end = raw[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .map(|index| cursor + index)
            .ok_or_else(|| anyhow!("chunked 响应缺少块长度行"))?;
        let size_line = String::from_utf8_lossy(&raw[cursor..line_end]).to_string();
        let size_text = size_line.split(';').next().unwrap_or("").trim().to_string();
        let size = usize::from_str_radix(&size_text, 16)
            .with_context(|| format!("chunk 长度解析失败：{size_text}"))?;
        cursor = line_end + 2;
        if size == 0 {
            break;
        }
        if cursor + size > raw.len() {
            bail!("chunked 响应被截断");
        }
        out.extend_from_slice(&raw[cursor..cursor + size]);
        cursor += size + 2; // 跳过块后的 CRLF
    }
    Ok(out)
}

/// GET 并返回响应体文本。
pub async fn get_text(url: &str) -> Result<String> {
    let response = send(&Request::get(url)).await?;
    Ok(response.text())
}

/// POST JSON 并返回响应。
pub async fn post_json(url: &str, payload: &serde_json::Value) -> Result<Response> {
    send(&Request::post(url).with_json(payload.clone())).await
}

/// POST 表单并返回响应。
pub async fn post_form(url: &str, pairs: Vec<(String, String)>) -> Result<Response> {
    send(&Request::post(url).with_form(pairs)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// 起一个只应答一次的本地假服务器。
    fn spawn_fake_server(response: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地端口失败");
        let port = listener
            .local_addr()
            .expect("读取本地地址失败")
            .port();
        let handle = std::thread::spawn(move || {
            let mut received = String::new();
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 4096];
                let mut total = Vec::new();
                let mut expected_len: Option<usize> = None;
                while let Ok(count) = stream.read(&mut buffer) {
                    if count == 0 {
                        break;
                    }
                    total.extend_from_slice(&buffer[..count]);
                    if expected_len.is_none() {
                        if let Some(pos) = total.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&total[..pos]);
                            let cl = head.lines().find_map(|line| {
                                let lower = line.to_ascii_lowercase();
                                if lower.starts_with("content-length:") {
                                    lower["content-length:".len()..].trim().parse::<usize>().ok()
                                } else {
                                    None
                                }
                            }).unwrap_or(0);
                            expected_len = Some(pos + 4 + cl);
                        }
                    }
                    if let Some(exp) = expected_len {
                        if total.len() >= exp {
                            break;
                        }
                    }
                }
                received = String::from_utf8_lossy(&total).to_string();
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Write);
            }
            received
        });
        (format!("http://127.0.0.1:{port}/"), handle)
    }

    #[test]
    fn encodes_form_pairs() {
        let encoded = form_encode(&[
            ("a".to_string(), "1".to_string()),
            ("b c".to_string(), "x&y".to_string()),
        ]);
        assert_eq!(encoded, "a=1&b+c=x%26y");
    }

    #[test]
    fn parses_url_variants() {
        let plain = parse_url("http://example.com/path?x=1").unwrap();
        assert_eq!(plain.scheme, "http");
        assert_eq!(plain.host, "example.com");
        assert_eq!(plain.port, 80);
        assert_eq!(plain.path, "/path?x=1");

        let with_port = parse_url("http://example.com:8080/a").unwrap();
        assert_eq!(with_port.port, 8080);
        assert_eq!(with_port.host_header, "example.com:8080");

        let secure = parse_url("https://example.com").unwrap();
        assert_eq!(secure.port, 443);
        assert_eq!(secure.path, "/");

        let root = parse_url("http://example.com").unwrap();
        assert_eq!(root.path, "/");

        assert!(parse_url("not-a-url").is_err());
    }

    #[test]
    fn resolves_relative_location() {
        assert_eq!(
            resolve_location("http://a.com/x/y", "/z"),
            "http://a.com/z"
        );
        assert_eq!(
            resolve_location("http://a.com/x/y", "z"),
            "http://a.com/x/z"
        );
        assert_eq!(
            resolve_location("http://a.com/x/y", "http://b.com/w"),
            "http://b.com/w"
        );
    }

    #[test]
    fn parses_last_header_block() {
        let text = "HTTP/1.1 301 Moved\r\nLocation: /b\r\n\r\nHTTP/1.1 200 OK\r\nX-Test: 1\r\n\r\n";
        let headers = parse_header_block(text);
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0], ("X-Test".to_string(), "1".to_string()));
    }

    #[test]
    fn decodes_chunked_body() {
        let raw = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        assert_eq!(decode_chunked(raw).unwrap(), b"hello world");
    }

    #[tokio::test]
    async fn native_backend_round_trip() {
        let (url, handle) = spawn_fake_server(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nX-Test: yes\r\n\r\nhello rustcms",
        );
        let response = send_with(&Request::get(url.as_str()), Backend::Native)
            .await
            .unwrap();
        let request = handle.join().unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.text(), "hello rustcms");
        assert_eq!(response.header("x-test"), Some("yes"));
        assert!(request.starts_with("GET / HTTP/1.1"), "{request}");
    }

    #[tokio::test]
    async fn native_backend_posts_form_body() {
        let (url, handle) = spawn_fake_server("HTTP/1.1 201 Created\r\n\r\nok");
        let response = send_with(
            &Request::post(url.as_str()).with_form(vec![("a".into(), "1".into())]),
            Backend::Native,
        )
        .await
        .unwrap();
        let request = handle.join().unwrap();
        assert_eq!(response.status, 201);
        assert!(request.contains("Content-Length: 3"), "{request}");
        assert!(request.contains("a=1"), "{request}");
    }

    #[tokio::test]
    async fn native_backend_rejects_https() {
        let request = Request::get("https://example.com/");
        let error = send_with(&request, Backend::Native).await.unwrap_err();
        assert!(error.to_string().contains("http://"), "{error}");
    }

    #[tokio::test]
    async fn curl_backend_round_trip() {
        if !curl_available() {
            eprintln!("跳过：系统没有 curl");
            return;
        }
        let (url, handle) = spawn_fake_server("HTTP/1.1 200 OK\r\nX-Test: 1\r\n\r\ncurl-ok");
        let response = send_with(&Request::get(url.as_str()), Backend::Curl)
            .await
            .unwrap();
        handle.join().unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.text(), "curl-ok");
        assert_eq!(response.header("x-test"), Some("1"));
    }
}

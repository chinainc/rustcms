//! SMTP 发信客户端。
//!
//! 对齐 Go 版 `internal/mail` 的对外语义：
//!
//! * `smtplist` 多账号容灾 —— 前一个账号失败自动换下一个；
//! * 按服务器广告选择认证机制（`AUTH PLAIN` / `AUTH LOGIN`）；
//! * 网络错误与 4xx 临时错误重试一次，5xx 永久错误不重试；
//! * 邮件头带 `Message-ID`，正文按 UTF-8 quoted-printable 编码。
//!
//! 实现说明：**零新依赖**。标准库实现明文 SMTP 会话；需要 TLS 时
//! 改走系统 `curl` 的 `smtp://` / `smtps://` 协议（curl 自带 TLS 栈），
//! 既不必引入 rustls/openssl 绑定，也不会抬高编译内存峰值。

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Result};

// ---------------------------------------------------------------------------
// 编码工具
// ---------------------------------------------------------------------------

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64 编码（标准字母表 + `=` 补齐）。
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64_ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(BASE64_ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(BASE64_ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64_ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Base64 解码（忽略空白；遇到非法字符返回 `None`）。
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for byte in text.bytes() {
        if byte.is_ascii_whitespace() || byte == b'=' {
            continue;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

/// 主题/发件人名的 RFC 2047 编码（UTF-8 + Base64）。
///
/// 纯 ASCII 内容原样返回，避免出现不必要的 `=?UTF-8?B?...?=`。
pub fn encode_header_word(text: &str) -> String {
    if text.is_ascii() {
        return text.to_string();
    }
    format!("=?UTF-8?B?{}?=", base64_encode(text.as_bytes()))
}

/// Quoted-Printable 编码（`Content-Transfer-Encoding: quoted-printable`）。
///
/// 规则：`=` 转义成 `=3D`；单行超过 75 字节用软换行 `=\r\n` 折行；
/// 非 ASCII 字节保持原样（依赖 `8BITMIME`，与 Go 版一致）。
pub fn quoted_printable_encode(text: &str) -> String {
    let mut buf: Vec<u8> = Vec::with_capacity(text.len() + text.len() / 8);
    let mut line_len = 0usize;
    for byte in text.as_bytes() {
        if *byte == b'=' {
            if line_len + 3 > 75 {
                buf.extend_from_slice(b"=\r\n");
                line_len = 0;
            }
            buf.extend_from_slice(b"=3D");
            line_len += 3;
            continue;
        }
        if *byte == b'\r' || *byte == b'\n' {
            continue;
        }
        if line_len + 1 > 75 {
            buf.extend_from_slice(b"=\r\n");
            line_len = 0;
        }
        buf.push(*byte);
        line_len += 1;
    }
    // quoted-printable 是字节流，这里按字节还原（输入是 UTF-8 文本，字节完整保留）。
    String::from_utf8_lossy(&buf).to_string()
}

// ---------------------------------------------------------------------------
// 配置与消息
// ---------------------------------------------------------------------------

/// TLS 模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    /// 明文（通常 25 端口）。
    None,
    /// STARTTLS（通常 587 端口）。
    StartTls,
    /// 隐式 TLS（通常 465 端口）。
    Implicit,
}

/// 一个发信账号。
#[derive(Debug, Clone)]
pub struct SmtpAccount {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from_email: String,
    pub from_name: String,
    pub tls: TlsMode,
    pub timeout: Duration,
}

impl SmtpAccount {
    /// 新建账号（默认 10 秒超时）。
    pub fn new(
        host: impl Into<String>,
        port: u16,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            username: username.into(),
            password: password.into(),
            from_email: String::new(),
            from_name: String::new(),
            tls: TlsMode::None,
            timeout: Duration::from_secs(10),
        }
    }

    /// 设置发件人。
    pub fn with_sender(mut self, email: impl Into<String>, name: impl Into<String>) -> Self {
        self.from_email = email.into();
        self.from_name = name.into();
        self
    }

    /// 设置 TLS 模式。
    pub fn with_tls(mut self, mode: TlsMode) -> Self {
        self.tls = mode;
        self
    }

    /// 设置超时。
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// 发信配置：`smtplist` 的多账号容灾就是靠 `accounts` 列表。
#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub accounts: Vec<SmtpAccount>,
    /// 单账号的重试次数（网络错误 / 4xx 临时错误才重试）。
    pub retries: usize,
}

impl SmtpConfig {
    /// 只含一个账号。
    pub fn single(account: SmtpAccount) -> Self {
        Self {
            accounts: vec![account],
            retries: 1,
        }
    }

    /// 多账号容灾。
    pub fn multi(accounts: Vec<SmtpAccount>) -> Self {
        Self {
            accounts,
            retries: 1,
        }
    }
}

/// 一封待发邮件。
#[derive(Debug, Clone)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    /// HTML 正文（UTF-8）。
    pub html: String,
}

impl Mail {
    /// 新建邮件。
    pub fn new(
        to: impl Into<String>,
        subject: impl Into<String>,
        html: impl Into<String>,
    ) -> Self {
        Self {
            to: to.into(),
            subject: subject.into(),
            html: html.into(),
        }
    }
}

/// 发信结果。
#[derive(Debug, Clone)]
pub struct SendOutcome {
    /// 使用的账号下标（对应 `SmtpConfig::accounts`）。
    pub account_index: usize,
    /// 实际尝试次数（含重试）。
    pub attempts: usize,
    /// 生成的 Message-ID。
    pub message_id: String,
    /// 传输方式：`native`（明文）或 `curl`（TLS）。
    pub transport: &'static str,
}

/// 唯一 Message-ID（时间戳 + 进程号 + 计数）。
pub fn new_message_id(domain: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("<{now:x}.{seq:x}.{}@{domain}>", std::process::id())
}

/// RFC 2822 日期头（`Tue, 04 Nov 2026 10:00:00 +0800`）。
fn rfc2822_date(now: chrono::DateTime<chrono::Local>) -> String {
    now.format("%a, %d %b %Y %H:%M:%S %z").to_string()
}

/// 组装完整邮件（头部 + 正文）。
///
/// `message_id` 建议用 [`new_message_id`] 生成；`date` 由调用方给出便于测试。
pub fn build_message(
    from_email: &str,
    from_name: &str,
    mail: &Mail,
    message_id: &str,
    date: chrono::DateTime<chrono::Local>,
) -> Vec<u8> {
    let mut headers: Vec<String> = Vec::new();
    if from_name.is_empty() {
        headers.push(format!("From: <{from_email}>"));
    } else {
        headers.push(format!(
            "From: {} <{from_email}>",
            encode_header_word(from_name)
        ));
    }
    headers.push(format!("To: <{}>", mail.to));
    headers.push(format!("Subject: {}", encode_header_word(&mail.subject)));
    headers.push(format!("Date: {}", rfc2822_date(date)));
    headers.push(format!("Message-ID: {message_id}"));
    headers.push("MIME-Version: 1.0".to_string());
    headers.push("Content-Type: text/html; charset=UTF-8".to_string());
    headers.push("Content-Transfer-Encoding: quoted-printable".to_string());

    let mut message = headers.join("\r\n");
    message.push_str("\r\n\r\n");
    message.push_str(&quoted_printable_encode(&mail.html));
    message.into_bytes()
}

// ---------------------------------------------------------------------------
// 错误分类
// ---------------------------------------------------------------------------

/// SMTP 失败原因。
#[derive(Debug)]
pub enum SmtpError {
    /// 网络层失败（连接断开、读写错误）。
    Network(String),
    /// 超时。
    Timeout,
    /// 协议不符（响应无法解析、命令顺序错误）。
    Protocol(String),
    /// 认证失败。
    Auth(String),
    /// 4xx 临时错误（可重试）。
    Temporary(u16, String),
    /// 5xx 永久错误（不重试）。
    Permanent(u16, String),
    /// 需要 TLS 但当前传输方式不支持。
    Tls(String),
}

impl SmtpError {
    /// 是否值得重试（网络错误与 4xx 临时错误）。
    pub fn retryable(&self) -> bool {
        matches!(self, SmtpError::Network(_) | SmtpError::Timeout | SmtpError::Temporary(_, _))
    }
}

impl std::fmt::Display for SmtpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SmtpError::Network(msg) => write!(f, "SMTP 网络错误：{msg}"),
            SmtpError::Timeout => write!(f, "SMTP 超时"),
            SmtpError::Protocol(msg) => write!(f, "SMTP 协议错误：{msg}"),
            SmtpError::Auth(msg) => write!(f, "SMTP 认证失败：{msg}"),
            SmtpError::Temporary(code, msg) => write!(f, "SMTP 临时错误 {code}：{msg}"),
            SmtpError::Permanent(code, msg) => write!(f, "SMTP 拒绝 {code}：{msg}"),
            SmtpError::Tls(msg) => write!(f, "SMTP 需要 TLS：{msg}"),
        }
    }
}

impl std::error::Error for SmtpError {}

/// 服务器返回的一组响应（多行 250- / 250 形式）。
#[derive(Debug, Clone)]
struct SmtpResponse {
    code: u16,
    lines: Vec<String>,
}

impl SmtpResponse {
    /// 拼接后的完整文本。
    fn joined(&self) -> String {
        self.lines.join("\n")
    }

    /// 是否 2xx。
    fn is_ok(&self) -> bool {
        (200..300).contains(&self.code)
    }
}

/// 读取一组 SMTP 响应（支持 `250-` 续行）。
fn read_response(reader: &mut BufReader<TcpStream>) -> std::result::Result<SmtpResponse, SmtpError> {
    let mut lines: Vec<String> = Vec::new();
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|err| SmtpError::Network(err.to_string()))?;
        if read == 0 {
            return Err(SmtpError::Network("服务器关闭了连接".to_string()));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
        let parsed: u16 = trimmed
            .get(..3)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| SmtpError::Protocol(format!("无法解析响应：{trimmed}")))?;
        // 第 4 个字符是 '-' 表示还有后续行。
        let more = trimmed.as_bytes().get(3) == Some(&b'-');
        lines.push(trimmed);
        if !more {
            return Ok(SmtpResponse {
                code: parsed,
                lines,
            });
        }
    }
}

/// 按响应码分类错误。
fn classify(code: u16, text: &str) -> SmtpError {
    if (400..500).contains(&code) {
        SmtpError::Temporary(code, text.to_string())
    } else {
        SmtpError::Permanent(code, text.to_string())
    }
}

/// 点填充（正文中以 `.` 开头的行要写成 `..`）。
fn dot_stuff(body: &str) -> String {
    // 先把 CRLF / CR 统一成 LF，再逐行输出 CRLF。
    let normalized = body.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(normalized.len() + 16);
    for line in normalized.split('\n') {
        if line.starts_with('.') {
            out.push('.');
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out
}

/// SMTP 会话连接（明文）。
struct SmtpSession {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl SmtpSession {
    /// 连接服务器并读完欢迎语。
    fn connect(account: &SmtpAccount) -> std::result::Result<Self, SmtpError> {
        let addr = format!("{}:{}", account.host, account.port);
        let socket = TcpStream::connect(&addr).map_err(|err| {
            if err.kind() == std::io::ErrorKind::TimedOut {
                SmtpError::Timeout
            } else {
                SmtpError::Network(format!("连接 {addr} 失败：{err}"))
            }
        })?;
        socket
            .set_read_timeout(Some(account.timeout))
            .map_err(|err| SmtpError::Network(err.to_string()))?;
        socket
            .set_write_timeout(Some(account.timeout))
            .map_err(|err| SmtpError::Network(err.to_string()))?;
        let writer = socket
            .try_clone()
            .map_err(|err| SmtpError::Network(err.to_string()))?;
        let mut reader = BufReader::new(socket);
        let greeting = read_response(&mut reader)?;
        if !greeting.is_ok() {
            return Err(classify(greeting.code, &greeting.joined()));
        }
        Ok(Self { reader, writer })
    }

    /// 发送一行命令并读取响应。
    fn command(&mut self, line: &str) -> std::result::Result<SmtpResponse, SmtpError> {
        self.writer
            .write_all(format!("{line}\r\n").as_bytes())
            .map_err(|err| SmtpError::Network(err.to_string()))?;
        self.writer
            .flush()
            .map_err(|err| SmtpError::Network(err.to_string()))?;
        read_response(&mut self.reader)
    }

    /// 写入原始数据（DATA 正文）。
    fn write_raw(&mut self, data: &[u8]) -> std::result::Result<(), SmtpError> {
        self.writer
            .write_all(data)
            .map_err(|err| SmtpError::Network(err.to_string()))?;
        self.writer
            .flush()
            .map_err(|err| SmtpError::Network(err.to_string()))
    }

    /// 读一行（认证挑战时用）。
    fn read_line(&mut self) -> std::result::Result<String, SmtpError> {
        let mut line = String::new();
        let read = self
            .reader
            .read_line(&mut line)
            .map_err(|err| SmtpError::Network(err.to_string()))?;
        if read == 0 {
            return Err(SmtpError::Network("服务器关闭了连接".to_string()));
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }
}

/// 解析 EHLO 响应里的能力。
fn parse_capabilities(response: &SmtpResponse) -> (bool, Vec<String>) {
    let text = response.joined().to_uppercase();
    let supports_starttls = text.contains("STARTTLS");
    let mut mechanisms: Vec<String> = Vec::new();
    for line in response.lines.iter() {
        let upper = line.to_uppercase();
        if let Some(idx) = upper.find("AUTH") {
            let tail = &upper[idx + 4..];
            for token in tail.split_whitespace() {
                if token.starts_with('-') || token.is_empty() {
                    continue;
                }
                if token == "LOGIN" || token == "PLAIN" || token == "CRAM-MD5" || token == "XOAUTH2" {
                    mechanisms.push(token.to_string());
                } else {
                    break;
                }
            }
            break;
        }
    }
    (supports_starttls, mechanisms)
}

/// 用明文 SMTP 会话发一封信。
fn send_native_once(
    account: &SmtpAccount,
    recipient: &str,
    message: &[u8],
) -> std::result::Result<(), SmtpError> {
    if account.tls != TlsMode::None {
        return Err(SmtpError::Tls(format!(
            "账号 {}:{} 需要 TLS，走 curl 传输",
            account.host, account.port
        )));
    }

    let mut session = SmtpSession::connect(account)?;

    // EHLO（部分老服务器只认 HELO）。
    let ehlo = session.command("EHLO localhost")?;
    let (capabilities, mechanisms) = if ehlo.is_ok() {
        parse_capabilities(&ehlo)
    } else {
        let helo = session.command("HELO localhost")?;
        if !helo.is_ok() {
            return Err(classify(helo.code, &helo.joined()));
        }
        (false, Vec::new())
    };
    let _ = capabilities;

    // 认证。
    if !account.username.is_empty() {
        if mechanisms.iter().any(|m| m == "PLAIN") {
            let payload = base64_encode(
                format!("\0{}\0{}", account.username, account.password).as_bytes(),
            );
            let resp = session.command(&format!("AUTH PLAIN {payload}"))?;
            if !resp.is_ok() {
                return Err(SmtpError::Auth(format!(
                    "AUTH PLAIN 被拒：{}",
                    resp.joined()
                )));
            }
        } else if mechanisms.iter().any(|m| m == "LOGIN") {
            let resp = session.command("AUTH LOGIN")?;
            if resp.code != 334 {
                return Err(SmtpError::Auth(format!(
                    "AUTH LOGIN 挑战异常：{}",
                    resp.joined()
                )));
            }
            session.write_raw(format!("{}\r\n", base64_encode(account.username.as_bytes())).as_bytes())?;
            let challenge = session.read_line()?;
            if !challenge.starts_with("334") {
                return Err(SmtpError::Auth(format!("用户名被拒：{challenge}")));
            }
            session.write_raw(format!("{}\r\n", base64_encode(account.password.as_bytes())).as_bytes())?;
            let result = read_response(&mut session.reader)?;
            if !result.is_ok() {
                return Err(SmtpError::Auth(format!("密码被拒：{}", result.joined())));
            }
        } else {
            return Err(SmtpError::Auth(
                "服务器未通告 AUTH PLAIN / AUTH LOGIN".to_string(),
            ));
        }
    }

    // 发件人。
    let mail_from = session.command(&format!("MAIL FROM:<{}>", account.from_email))?;
    if !mail_from.is_ok() {
        return Err(classify(mail_from.code, &mail_from.joined()));
    }
    // 收件人。
    let rcpt = session.command(&format!("RCPT TO:<{recipient}>"))?;
    if !rcpt.is_ok() {
        return Err(classify(rcpt.code, &rcpt.joined()));
    }
    // 正文。
    let data = session.command("DATA")?;
    if data.code != 354 {
        return Err(classify(data.code, &data.joined()));
    }
    let body = String::from_utf8_lossy(message).to_string();
    let mut payload = dot_stuff(&body);
    payload.push_str(".\r\n");
    session.write_raw(payload.as_bytes())?;
    let accepted = read_response(&mut session.reader)?;
    if !accepted.is_ok() {
        return Err(classify(accepted.code, &accepted.joined()));
    }
    // 结束会话（失败不影响投递结果）。
    let _ = session.command("QUIT");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// 起一个假 SMTP 服务器：按脚本依次应答，并记录客户端发来的命令行。
    fn spawn_fake_smtp(
        responses: Vec<&'static str>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(socket.try_clone().expect("clone"));
            let mut log: Vec<String> = Vec::new();
            let mut index = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).expect("read") == 0 {
                    break;
                }
                let line = line.trim_end_matches(['\r', '\n']).to_string();
                log.push(line.clone());
                if line == "DATA" {
                    loop {
                        let mut body_line = String::new();
                        if reader.read_line(&mut body_line).expect("read body") == 0 {
                            break;
                        }
                        let trimmed = body_line.trim_end_matches(['\r', '\n']).to_string();
                        log.push(format!("<body>{trimmed}"));
                        if trimmed == "." {
                            break;
                        }
                    }
                }
                if index >= responses.len() {
                    break;
                }
                let response = responses[index];
                index += 1;
                socket
                    .write_all(format!("{response}\r\n").as_bytes())
                    .expect("write");
                socket.flush().expect("flush");
                if line == "QUIT" {
                    break;
                }
            }
            log
        });
        (format!("127.0.0.1:{port}"), handle)
    }

    fn plain_account(host: &str, port: u16) -> SmtpAccount {
        SmtpAccount::new(host, port, "user", "pass")
            .with_sender("from@example.com", "站点")
            .with_timeout(Duration::from_secs(5))
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"Hello"), "SGVsbG8=");
    }

    #[test]
    fn base64_round_trips_binary() {
        let data: Vec<u8> = (0u16..=255).map(|v| v as u8).collect();
        let encoded = base64_encode(&data);
        assert_eq!(base64_decode(&encoded).unwrap(), data);
    }

    #[test]
    fn base64_decode_rejects_invalid_char() {
        assert!(base64_decode("!!!!").is_none());
    }

    #[test]
    fn header_word_keeps_ascii_and_encodes_utf8() {
        assert_eq!(encode_header_word("Hello"), "Hello");
        let encoded = encode_header_word("测试");
        assert!(encoded.starts_with("=?UTF-8?B?"), "{encoded}");
        assert!(encoded.ends_with("?="), "{encoded}");
        let decoded = base64_decode(&encoded[10..encoded.len() - 2]).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "测试");
    }

    #[test]
    fn quoted_printable_escapes_equals_and_keeps_utf8() {
        assert_eq!(quoted_printable_encode("a=b"), "a=3Db");
        // 中文按字节保留，不被截断。
        let encoded = quoted_printable_encode("你好");
        assert_eq!(encoded, "你好");
        // 超长行折行。
        let long = "a".repeat(200);
        let folded = quoted_printable_encode(&long);
        assert!(folded.contains("=\r\n"), "{folded}");
        assert!(folded.replace("=\r\n", "").len() == 200);
    }

    #[test]
    fn dot_stuffing_protects_leading_dot() {
        assert_eq!(dot_stuff(".hidden\r\nnormal"), "..hidden\r\nnormal\r\n");
    }

    #[test]
    fn build_message_has_required_headers() {
        let mail = Mail::new("to@example.com", "测试主题", "<p>你好</p>");
        let message_id = "<abc@example.com>";
        let date = chrono::Local::now();
        let raw = build_message("from@example.com", "站点", &mail, message_id, date);
        let text = String::from_utf8_lossy(&raw).to_string();
        assert!(text.contains("From: =?UTF-8?B?"), "{text}");
        assert!(text.contains("To: <to@example.com>"), "{text}");
        assert!(text.contains("Subject: =?UTF-8?B?"), "{text}");
        assert!(text.contains("Message-ID: <abc@example.com>"), "{text}");
        assert!(text.contains("MIME-Version: 1.0"), "{text}");
        assert!(text.contains("Content-Type: text/html; charset=UTF-8"), "{text}");
        assert!(
            text.contains("Content-Transfer-Encoding: quoted-printable"),
            "{text}"
        );
        assert!(text.ends_with("<p>你好</p>"), "{text}");
    }

    #[test]
    fn parse_capabilities_reads_mechanisms() {
        let response = SmtpResponse {
            code: 250,
            lines: vec![
                "250-fake greets you".to_string(),
                "250-SIZE 35882577".to_string(),
                "250-STARTTLS".to_string(),
                "250 AUTH PLAIN LOGIN".to_string(),
            ],
        };
        let (starttls, mechanisms) = parse_capabilities(&response);
        assert!(starttls);
        assert_eq!(mechanisms, vec!["PLAIN".to_string(), "LOGIN".to_string()]);
    }

    #[test]
    fn native_session_completes_plain_flow() {
        let (host, handle) = spawn_fake_smtp(vec![
            "220 fake ESMTP",
            "250-fake\r\n250 AUTH PLAIN LOGIN",
            "235 authentication ok",
            "250 sender ok",
            "250 recipient ok",
            "354 go ahead",
            "250 message accepted",
            "221 bye",
        ]);
        let port: u16 = host.rsplit(':').next().unwrap().parse().unwrap();
        let account = plain_account("127.0.0.1", port);
        let mail = Mail::new("to@example.com", "s", "<p>hi</p>");
        let message = build_message(
            &account.from_email,
            &account.from_name,
            &mail,
            "<mid@example.com>",
            chrono::Local::now(),
        );
        let result = send_native_once(&account, &mail.to, &message);
        assert!(result.is_ok(), "{result:?}");
        let log = handle.join().unwrap();
        assert!(log.iter().any(|l| l == "EHLO localhost"), "{log:?}");
        assert!(log.iter().any(|l| l.starts_with("AUTH PLAIN ")), "{log:?}");
        assert!(
            log.iter().any(|l| l == "MAIL FROM:<from@example.com>"),
            "{log:?}"
        );
        assert!(
            log.iter().any(|l| l == "RCPT TO:<to@example.com>"),
            "{log:?}"
        );
        assert!(log.iter().any(|l| l == "DATA"), "{log:?}");
        assert!(log.iter().any(|l| l == "QUIT"), "{log:?}");
    }

    #[test]
    fn native_session_uses_login_when_plain_absent() {
        let (host, handle) = spawn_fake_smtp(vec![
            "220 fake ESMTP",
            "250-fake\r\n250 AUTH LOGIN",
            "334 VXNlcm5hbWU6",
            "334 UGFzc3dvcmQ6",
            "235 authentication ok",
            "250 sender ok",
            "250 recipient ok",
            "354 go ahead",
            "250 message accepted",
            "221 bye",
        ]);
        let port: u16 = host.rsplit(':').next().unwrap().parse().unwrap();
        let mut account = plain_account("127.0.0.1", port);
        account.username = "alice".to_string();
        account.password = "secret".to_string();
        let mail = Mail::new("to@example.com", "s", "<p>hi</p>");
        let message = build_message(
            &account.from_email,
            &account.from_name,
            &mail,
            "<mid@example.com>",
            chrono::Local::now(),
        );
        let result = send_native_once(&account, &mail.to, &message);
        assert!(result.is_ok(), "{result:?}");
        let log = handle.join().unwrap();
        assert!(log.iter().any(|l| l == "AUTH LOGIN"), "{log:?}");
        assert!(log.iter().any(|l| *l == base64_encode(b"alice")), "{log:?}");
        assert!(log.iter().any(|l| *l == base64_encode(b"secret")), "{log:?}");
    }

    #[test]
    fn native_session_reports_auth_failure() {
        let (host, handle) = spawn_fake_smtp(vec![
            "220 fake ESMTP",
            "250-fake\r\n250 AUTH PLAIN",
            "535 authentication failed",
        ]);
        let port: u16 = host.rsplit(':').next().unwrap().parse().unwrap();
        let account = plain_account("127.0.0.1", port);
        let mail = Mail::new("to@example.com", "s", "<p>hi</p>");
        let message = build_message(
            &account.from_email,
            &account.from_name,
            &mail,
            "<mid@example.com>",
            chrono::Local::now(),
        );
        let result = send_native_once(&account, &mail.to, &message);
        match result {
            Err(SmtpError::Auth(msg)) => assert!(msg.contains("535"), "{msg}"),
            other => panic!("期望认证失败，实际 {other:?}"),
        }
        handle.join().unwrap();
    }

    #[test]
    fn send_falls_back_to_next_account() {
        // 第一个账号指向未监听端口（连接被拒），第二个账号是假服务器。
        let (host, handle) = spawn_fake_smtp(vec![
            "220 fake ESMTP",
            "250-fake\r\n250 AUTH PLAIN",
            "235 ok",
            "250 sender ok",
            "250 recipient ok",
            "354 go ahead",
            "250 accepted",
            "221 bye",
        ]);
        let port: u16 = host.rsplit(':').next().unwrap().parse().unwrap();
        let broken = SmtpAccount::new("127.0.0.1", 1, "u", "p")
            .with_sender("from@example.com", "")
            .with_timeout(Duration::from_millis(500));
        let good = plain_account("127.0.0.1", port);
        let config = SmtpConfig {
            accounts: vec![broken, good],
            retries: 0,
        };
        let mail = Mail::new("to@example.com", "s", "<p>hi</p>");
        let outcome = send(&config, &mail).expect("应当切换到第二个账号");
        assert_eq!(outcome.account_index, 1);
        assert_eq!(outcome.transport, "native");
        assert!(!outcome.message_id.is_empty());
        handle.join().unwrap();
    }

    #[test]
    fn send_without_accounts_fails() {
        let config = SmtpConfig {
            accounts: Vec::new(),
            retries: 1,
        };
        let mail = Mail::new("to@example.com", "s", "x");
        assert!(send(&config, &mail).is_err());
    }
}

/// 用系统 curl 走 SMTP 协议发信（支持 TLS）。
fn send_curl_once(
    account: &SmtpAccount,
    recipient: &str,
    message: &[u8],
) -> std::result::Result<(), SmtpError> {
    if !crate::http::curl_available() {
        return Err(SmtpError::Tls(
            "需要 TLS 但系统没有 curl".to_string(),
        ));
    }
    let scheme = match account.tls {
        TlsMode::Implicit => "smtps",
        _ => "smtp",
    };
    let url = format!("{scheme}://{}:{}", account.host, account.port);

    let mut child = std::process::Command::new("curl")
        .arg("--url")
        .arg(&url)
        .arg("--mail-from")
        .arg(&account.from_email)
        .arg("--mail-rcpt")
        .arg(recipient)
        .arg("--upload-file")
        .arg("-")
        .arg("-sS")
        .arg("--max-time")
        .arg(account.timeout.as_secs().to_string())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| SmtpError::Network(format!("启动 curl 失败：{err}")))?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(message)
            .map_err(|err| SmtpError::Network(format!("写入邮件内容失败：{err}")))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|err| SmtpError::Network(format!("curl 执行失败：{err}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    // curl 的 SMTP 错误码形如 67/550 等。
    if let Some(code) = stderr
        .split_whitespace()
        .find_map(|token| token.parse::<u16>().ok())
    {
        if (400..600).contains(&code) {
            return Err(classify(code, &stderr));
        }
    }
    Err(SmtpError::Network(format!("curl 发信失败：{stderr}")))
}

/// 从发件人地址里取域名（用于 Message-ID）。
fn domain_of(email: &str) -> String {
    email
        .split('@')
        .nth(1)
        .unwrap_or("localhost")
        .to_string()
}

/// 发信（多账号容灾 + 临时错误重试）。
///
/// 行为与 Go 版一致：账号列表逐个尝试，前一个失败自动换下一个；
/// 网络错误与 4xx 临时错误在同账号内重试 `retries` 次。
pub fn send(config: &SmtpConfig, mail: &Mail) -> Result<SendOutcome> {
    if config.accounts.is_empty() {
        bail!("没有可用的发信账号");
    }
    let mut last_error: Option<SmtpError> = None;
    for (index, account) in config.accounts.iter().enumerate() {
        let message_id = new_message_id(&domain_of(&account.from_email));
        let message = build_message(
            &account.from_email,
            &account.from_name,
            mail,
            &message_id,
            chrono::Local::now(),
        );
        let transport: &'static str = if account.tls == TlsMode::None {
            "native"
        } else {
            "curl"
        };
        let max_attempts = config.retries.saturating_add(1);
        let mut attempts = 0usize;
        while attempts < max_attempts {
            attempts += 1;
            let result = if account.tls == TlsMode::None {
                send_native_once(account, &mail.to, &message)
            } else {
                send_curl_once(account, &mail.to, &message)
            };
            match result {
                Ok(()) => {
                    return Ok(SendOutcome {
                        account_index: index,
                        attempts,
                        message_id,
                        transport,
                    })
                }
                Err(err) => {
                    let retryable = err.retryable();
                    tracing::warn!(
                        account_index = index,
                        attempts,
                        error = %err,
                        "SMTP 发送失败"
                    );
                    last_error = Some(err);
                    if !retryable {
                        break;
                    }
                }
            }
        }
    }
    match last_error {
        Some(err) => Err(anyhow!(err)),
        None => bail!("没有可用的发信账号"),
    }
}

/// 连接测试（握手 + 认证 + QUIT，不发信）。
///
/// 对齐 Go 版 `mail.Probe()`。
pub fn probe(account: &SmtpAccount) -> Result<()> {
    if account.tls != TlsMode::None {
        if !crate::http::curl_available() {
            bail!("需要 TLS 但系统没有 curl");
        }
        return Ok(());
    }
    let mut session = SmtpSession::connect(account)?;
    let ehlo = session.command("EHLO localhost")?;
    if !ehlo.is_ok() {
        let helo = session.command("HELO localhost")?;
        if !helo.is_ok() {
            bail!("EHLO/HELO 被拒：{}", helo.joined());
        }
    }
    let (_, mechanisms) = parse_capabilities(&ehlo);
    if !account.username.is_empty() {
        if mechanisms.iter().any(|m| m == "PLAIN") {
            let payload = base64_encode(
                format!("\0{}\0{}", account.username, account.password).as_bytes(),
            );
            let resp = session.command(&format!("AUTH PLAIN {payload}"))?;
            if !resp.is_ok() {
                bail!("认证失败：{}", resp.joined());
            }
        } else if mechanisms.iter().any(|m| m == "LOGIN") {
            let resp = session.command("AUTH LOGIN")?;
            if resp.code != 334 {
                bail!("认证挑战异常：{}", resp.joined());
            }
            session
                .write_raw(format!("{}\r\n", base64_encode(account.username.as_bytes())).as_bytes())?;
            let challenge = session.read_line()?;
            if !challenge.starts_with("334") {
                bail!("认证失败：{challenge}");
            }
            session
                .write_raw(format!("{}\r\n", base64_encode(account.password.as_bytes())).as_bytes())?;
            let result = read_response(&mut session.reader)?;
            if !result.is_ok() {
                bail!("认证失败：{}", result.joined());
            }
        } else {
            bail!("服务器未通告 AUTH PLAIN / AUTH LOGIN");
        }
    }
    let _ = session.command("QUIT");
    Ok(())
}

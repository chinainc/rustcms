//! 微信支付 V2（Native 扫码 / 小程序 JSAPI）。
//!
//! 三个入口：
//! - 统一下单（Native 扫码与小程序 JSAPI 两种 `trade_type`）；
//! - 回调验签与报文体解析；
//! - 查单（本项目对外的 `GET /wechat/query`）。
//!
//! 报文是 V2 的 XML（`<xml><k>v</k></xml>`），签名规则：
//! 参数去掉 `sign`、去掉空值，按键名 ASCII 升序拼 `k=v&k=v`，末尾追 `&key=APIKey`，
//! 然后 **MD5 或 HMAC-SHA256**（`sign_type` 决定，默认 MD5）取大写十六进制。
//! 注意：与支付宝不同，`sign_type` 本身要参与签名。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use super::crypto;

/// 微信支付 V2 网关地址（沙盒是同一域名下的 `/sandboxnew` 前缀）。
pub const ENDPOINT: &str = "https://api.mch.weixin.qq.com";

/// 签名算法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignType {
    /// MD5（V2 默认）
    Md5,
    /// HMAC-SHA256
    HmacSha256,
}

impl SignType {
    /// 报文里的取值。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Md5 => "MD5",
            Self::HmacSha256 => "HMAC-SHA256",
        }
    }

    /// 解析（未知值按 MD5 处理）。
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_uppercase().as_str() {
            "HMAC-SHA256" | "HMACSHA256" => Self::HmacSha256,
            _ => Self::Md5,
        }
    }
}

/// 微信支付账户配置（`payment_accounts.pay_config`）。
///
/// 字段对应 `PayConfig`：
/// `app_id` = 公众号 / 小程序 AppID，`account` = 商户号，`api_key` = APIv2 密钥。
#[derive(Debug, Clone)]
pub struct WechatConfig {
    pub app_id: String,
    pub mch_id: String,
    pub api_key: String,
    pub sign_type: SignType,
    pub sandbox: bool,
}

impl WechatConfig {
    /// 从 `pay_config` JSON 里取配置（缺字段直接报错，避免拿空密钥去签名）。
    pub fn from_json(config: &Value) -> Result<Self, String> {
        let read = |key: &str| {
            config
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let app_id = read("app_id");
        let mch_id = read("account");
        let api_key = read("api_key");
        if app_id.is_empty() {
            return Err("微信支付账户缺少 app_id".to_string());
        }
        if mch_id.is_empty() {
            return Err("微信支付账户缺少商户号（account）".to_string());
        }
        if api_key.is_empty() {
            return Err("微信支付账户缺少 api_key".to_string());
        }
        let sign_type = SignType::parse(&read("sign_type"));
        let sandbox = match config.get("sandbox") {
            Some(Value::Bool(flag)) => *flag,
            Some(Value::Number(number)) => number.as_i64().unwrap_or(0) != 0,
            Some(Value::String(text)) => matches!(text.as_str(), "1" | "true" | "True"),
            _ => false,
        };
        Ok(Self {
            app_id,
            mch_id,
            api_key,
            sign_type,
            sandbox,
        })
    }
}

// ---------------------------------------------------------------------------
// 签名
// ---------------------------------------------------------------------------

/// 待签名字符串：去掉 `sign`、去掉空值，按键名升序拼 `k=v&…`，末尾追 `&key=APIKey`。
pub fn sign_payload(params: &BTreeMap<String, String>, api_key: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (key, value) in params {
        if key == "sign" || value.is_empty() {
            continue;
        }
        parts.push(format!("{key}={value}"));
    }
    format!("{}&key={api_key}", parts.join("&"))
}

/// 按签名算法算签名（大写十六进制）。
pub fn sign_with(params: &BTreeMap<String, String>, api_key: &str, sign_type: SignType) -> String {
    let payload = sign_payload(params, api_key);
    match sign_type {
        SignType::Md5 => crypto::md5_hex_upper(payload.as_bytes()),
        SignType::HmacSha256 => crypto::hex_upper(&crypto::hmac_sha256(api_key.as_bytes(), payload.as_bytes())),
    }
}

/// 给参数加上 `sign`（并确保 `sign_type` 在报文里）。
///
/// 注意：`sign_type` 参与签名，所以必须先写进参数表再算签名。
pub fn sign_params(
    mut params: BTreeMap<String, String>,
    api_key: &str,
    sign_type: SignType,
) -> BTreeMap<String, String> {
    params.insert("sign_type".to_string(), sign_type.as_str().to_string());
    let sign = sign_with(&params, api_key, sign_type);
    params.insert("sign".to_string(), sign);
    params
}

/// 校验回调签名（按报文里的 `sign_type` 选算法，缺省 MD5，与既有行为一致）。
pub fn verify_sign(params: &BTreeMap<String, String>, api_key: &str) -> Result<(), String> {
    let sign = params
        .get("sign")
        .map(String::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if sign.is_empty() {
        return Err("回调缺少 sign".to_string());
    }
    let sign_type = SignType::parse(
        params
            .get("sign_type")
            .map(String::as_str)
            .unwrap_or("MD5"),
    );
    let expect = sign_with(params, api_key, sign_type);
    if !sign.eq_ignore_ascii_case(&expect) {
        return Err(format!(
            "微信签名校验失败（期望 {expect}，收到 {sign}）"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// XML
// ---------------------------------------------------------------------------

/// 生成 V2 请求 XML（值全部包 CDATA，避免 XML 转义问题）。
pub fn build_xml(params: &BTreeMap<String, String>) -> String {
    let mut out = String::from("<xml>");
    for (key, value) in params {
        // CDATA 里不能出现 `]]>`，正常业务字段不会，遇到就退化成分段拼接
        let value = value.replace("]]>", "]]]]><![CDATA[>");
        out.push_str(&format!("<{key}><![CDATA[{value}]]></{key}>"));
    }
    out.push_str("</xml>");
    out
}

/// 解析 V2 的扁平 XML（`<xml><k>v</k>…</xml>`）。
///
/// 报文体解析：只取一层子元素，值可以是纯文本或 CDATA；
/// 嵌套结构（如 `<coupon_fee_0>` 这类型号后缀）按同名平铺处理。
pub fn parse_xml(xml: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let bytes = xml.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'<' {
            index += 1;
            continue;
        }
        let name_start = index + 1;
        let mut cursor = name_start;
        while cursor < bytes.len() && bytes[cursor] != b'>' {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }
        let name = &xml[name_start..cursor];
        if name.is_empty()
            || name.starts_with('/')
            || name.starts_with('?')
            || name.starts_with('!')
        {
            index = cursor + 1;
            continue;
        }
        if bytes[cursor - 1] == b'/' {
            // 自闭合标签，值为空
            out.insert(name.to_string(), String::new());
            index = cursor + 1;
            continue;
        }
        let close = format!("</{name}>");
        let value_start = cursor + 1;
        let Some(offset) = xml[value_start..].find(&close) else {
            break;
        };
        let raw = &xml[value_start..value_start + offset];
        // 值是 CDATA 时直接当文本；否则若以 `<` 开头说明是嵌套容器（如根节点 `<xml>`），
        // 进入内部继续解析。⚠️ 不能只看 `starts_with('<')`，否则 CDATA 会被误判成容器。
        let trimmed = raw.trim_start();
        if !trimmed.starts_with("<![CDATA[") && trimmed.starts_with('<') {
            index = value_start;
            continue;
        }
        out.insert(name.to_string(), decode_value(raw));
        index = value_start + offset + close.len();
    }
    out
}

/// 取值：剥 CDATA 包装 + 反转义实体。
fn decode_value(raw: &str) -> String {
    let trimmed = raw.trim();
    let inner = match trimmed
        .strip_prefix("<![CDATA[")
        .and_then(|rest| rest.strip_suffix("]]>"))
    {
        Some(value) => value.to_string(),
        None => trimmed.to_string(),
    };
    inner
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// 回调应答 XML（微信要求：成功回 `SUCCESS/OK`，失败回 `FAIL` + 原因）。
pub fn reply_xml(success: bool, message: &str) -> String {
    let (code, msg) = if success {
        ("SUCCESS", "OK")
    } else {
        ("FAIL", message)
    };
    let msg = msg.replace("]]>", "]]]]><![CDATA[>");
    format!(
        "<xml><return_code><![CDATA[{code}]]></return_code>\
         <return_msg><![CDATA[{msg}]]></return_msg></xml>"
    )
}

// ---------------------------------------------------------------------------
// 网关调用
// ---------------------------------------------------------------------------

/// 统一下单返回（只取本项目要用的字段）。
#[derive(Debug, Clone, Default)]
pub struct UnifiedOrderResult {
    pub prepay_id: String,
    pub code_url: String,
    pub nonce_str: String,
}

/// 查单结果。
#[derive(Debug, Clone, Default)]
pub struct QueryResult {
    /// `SUCCESS` / `REFUND` / `NOTPAY` / `CLOSED` / `REVOKED` / `USERPAYING` / `PAYERROR`
    pub trade_state: String,
    pub transaction_id: String,
    pub total_fee: i64,
    pub openid: String,
    pub time_end: String,
}

/// 32 位随机串（微信 `nonce_str`）。
pub fn nonce_str() -> String {
    use rand::distributions::Alphanumeric;
    use rand::Rng;
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

/// 沙盒签名密钥缓存（`mch_id → sandbox_signkey`）。
fn sandbox_keys() -> &'static Mutex<BTreeMap<String, String>> {
    static KEYS: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    KEYS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn api_url(sandbox: bool, path: &str) -> String {
    if sandbox {
        format!("{ENDPOINT}/sandboxnew{path}")
    } else {
        format!("{ENDPOINT}{path}")
    }
}

/// 沙盒环境要用「沙盒密钥」签名，先按真实 API 密钥换取并缓存。
///
/// 取沙箱签名密钥。
pub async fn sandbox_sign_key(cfg: &WechatConfig) -> Result<String, String> {
    if !cfg.sandbox {
        return Ok(cfg.api_key.clone());
    }
    if let Ok(cache) = sandbox_keys().lock() {
        if let Some(key) = cache.get(&cfg.mch_id) {
            return Ok(key.clone());
        }
    }
    let mut params = BTreeMap::new();
    params.insert("mch_id".to_string(), cfg.mch_id.clone());
    params.insert("nonce_str".to_string(), nonce_str());
    let params = sign_params(params, &cfg.api_key, SignType::Md5);
    let body = build_xml(&params);
    let request = rustcms_ext::http::Request::post(api_url(true, "/pay/getsignkey"))
        .with_header("Content-Type", "text/xml; charset=utf-8")
        .with_body(rustcms_ext::http::Body::Bytes(body.into_bytes()));
    let response = rustcms_ext::http::send(&request)
        .await
        .map_err(|err| format!("微信沙盒密钥请求失败：{err}"))?;
    let fields = parse_xml(&response.text());
    let key = fields
        .get("sandbox_signkey")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if key.is_empty() {
        return Err(format!(
            "微信沙盒密钥获取失败：{}",
            fields
                .get("return_msg")
                .map(String::as_str)
                .unwrap_or("返回内容为空")
        ));
    }
    if let Ok(mut cache) = sandbox_keys().lock() {
        cache.insert(cfg.mch_id.clone(), key.clone());
    }
    Ok(key)
}

/// 公共请求参数（`appid` / `mch_id` / `nonce_str`）。
fn base_params(cfg: &WechatConfig) -> BTreeMap<String, String> {
    let mut params = BTreeMap::new();
    params.insert("appid".to_string(), cfg.app_id.clone());
    params.insert("mch_id".to_string(), cfg.mch_id.clone());
    params.insert("nonce_str".to_string(), nonce_str());
    params
}

/// POST 一个已签名的请求，返回解析后的应答字段。
async fn post_signed(
    cfg: &WechatConfig,
    path: &str,
    params: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    let key = sandbox_sign_key(cfg).await?;
    let params = sign_params(params, &key, cfg.sign_type);
    let body = build_xml(&params);
    let request = rustcms_ext::http::Request::post(api_url(cfg.sandbox, path))
        .with_header("Content-Type", "text/xml; charset=utf-8")
        .with_body(rustcms_ext::http::Body::Bytes(body.into_bytes()));
    let response = rustcms_ext::http::send(&request)
        .await
        .map_err(|err| format!("微信支付请求失败：{err}"))?;
    let text = response.text();
    let fields = parse_xml(&text);
    if fields.is_empty() {
        return Err(format!(
            "微信支付应答无法解析（HTTP {}）：{}",
            response.status,
            text.trim()
        ));
    }
    if fields.get("return_code").map(String::as_str) != Some("SUCCESS") {
        return Err(format!(
            "微信支付通信失败：{}",
            fields
                .get("return_msg")
                .map(String::as_str)
                .unwrap_or("未知错误")
        ));
    }
    // 通信成功不代表下单成功，还要看 result_code
    if let Some(code) = fields.get("result_code") {
        if code != "SUCCESS" {
            return Err(format!(
                "微信下单失败：{} {}",
                fields
                    .get("err_code")
                    .map(String::as_str)
                    .unwrap_or_default(),
                fields
                    .get("err_code_des")
                    .map(String::as_str)
                    .unwrap_or_default()
            ));
        }
    }
    Ok(fields)
}

/// 统一下单 —— Native（扫码支付）。
///
/// 对齐 `createWechatPayment`：`trade_type=Native`，成功返回 `code_url`。
pub async fn unified_order_native(
    cfg: &WechatConfig,
    out_trade_no: &str,
    total_fee: i64,
    body: &str,
    notify_url: &str,
    client_ip: &str,
) -> Result<UnifiedOrderResult, String> {
    if total_fee <= 0 {
        return Err("支付金额必须大于 0".to_string());
    }
    let mut params = base_params(cfg);
    params.insert("body".to_string(), trim_len(body, 128));
    params.insert("out_trade_no".to_string(), out_trade_no.to_string());
    params.insert("total_fee".to_string(), total_fee.to_string());
    params.insert("trade_type".to_string(), "Native".to_string());
    params.insert("notify_url".to_string(), notify_url.to_string());
    params.insert("spbill_create_ip".to_string(), trim_len(client_ip, 64));

    let fields = post_signed(cfg, "/pay/unifiedorder", params).await?;
    let code_url = fields
        .get("code_url")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if code_url.is_empty() {
        return Err("微信下单未返回 code_url".to_string());
    }
    Ok(UnifiedOrderResult {
        prepay_id: fields
            .get("prepay_id")
            .map(|value| value.trim().to_string())
            .unwrap_or_default(),
        code_url,
        nonce_str: fields
            .get("nonce_str")
            .map(|value| value.trim().to_string())
            .unwrap_or_else(nonce_str),
    })
}

/// 统一下单 —— JSAPI（小程序支付），返回小程序端需要的 `paySign` 参数包。
///
/// 对齐 `createWeappPayment`。
pub async fn unified_order_jsapi(
    cfg: &WechatConfig,
    out_trade_no: &str,
    total_fee: i64,
    body: &str,
    notify_url: &str,
    client_ip: &str,
    openid: &str,
) -> Result<JsapiPayParams, String> {
    if total_fee <= 0 {
        return Err("支付金额必须大于 0".to_string());
    }
    if openid.trim().is_empty() {
        return Err("该会员还没有绑定小程序 openid，无法使用小程序支付".to_string());
    }
    let mut params = base_params(cfg);
    params.insert("body".to_string(), trim_len(body, 128));
    params.insert("out_trade_no".to_string(), out_trade_no.to_string());
    params.insert("total_fee".to_string(), total_fee.to_string());
    params.insert("trade_type".to_string(), "JSAPI".to_string());
    params.insert("notify_url".to_string(), notify_url.to_string());
    params.insert("spbill_create_ip".to_string(), trim_len(client_ip, 64));
    params.insert("openid".to_string(), openid.trim().to_string());

    let fields = post_signed(cfg, "/pay/unifiedorder", params).await?;
    let prepay_id = fields
        .get("prepay_id")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if prepay_id.is_empty() {
        return Err("微信下单未返回 prepay_id".to_string());
    }
    let time_stamp = chrono::Utc::now().timestamp().to_string();
    let nonce = fields
        .get("nonce_str")
        .map(|value| value.trim().to_string())
        .unwrap_or_else(nonce_str);
    let package = format!("prepay_id={prepay_id}");
    // 小程序 paySign 固定 MD5（对齐 `wechat.GetMiniPaySign(..., SignType_MD5, ...)`）
    let mut sign_src = BTreeMap::new();
    sign_src.insert("appId".to_string(), cfg.app_id.clone());
    sign_src.insert("nonceStr".to_string(), nonce.clone());
    sign_src.insert("package".to_string(), package.clone());
    sign_src.insert("signType".to_string(), "MD5".to_string());
    sign_src.insert("timeStamp".to_string(), time_stamp.clone());
    let pay_sign = sign_with(&sign_src, &cfg.api_key, SignType::Md5);

    Ok(JsapiPayParams {
        nonce_str: nonce,
        package,
        pay_sign,
        sign_type: "MD5".to_string(),
        time_stamp,
    })
}

/// 小程序支付参数包（`wx.requestPayment` 的入参）。
#[derive(Debug, Clone, Default)]
pub struct JsapiPayParams {
    pub nonce_str: String,
    pub package: String,
    pub pay_sign: String,
    pub sign_type: String,
    pub time_stamp: String,
}

/// 查单（`/pay/orderquery`）。
pub async fn order_query(cfg: &WechatConfig, out_trade_no: &str) -> Result<QueryResult, String> {
    let mut params = base_params(cfg);
    params.insert("out_trade_no".to_string(), out_trade_no.to_string());
    let fields = post_signed(cfg, "/pay/orderquery", params).await?;
    Ok(QueryResult {
        trade_state: fields
            .get("trade_state")
            .map(|value| value.trim().to_string())
            .unwrap_or_default(),
        transaction_id: fields
            .get("transaction_id")
            .map(|value| value.trim().to_string())
            .unwrap_or_default(),
        total_fee: fields
            .get("total_fee")
            .and_then(|value| value.trim().parse::<i64>().ok())
            .unwrap_or(0),
        openid: fields
            .get("openid")
            .map(|value| value.trim().to_string())
            .unwrap_or_default(),
        time_end: fields
            .get("time_end")
            .map(|value| value.trim().to_string())
            .unwrap_or_default(),
    })
}

/// 按字符（不是字节）截断，微信 `body` 上限 128 字节。
fn trim_len(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "订单支付".to_string();
    }
    let mut out = String::new();
    for ch in trimmed.chars() {
        if out.len() + ch.len_utf8() > max {
            break;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> WechatConfig {
        WechatConfig {
            app_id: "wx123".into(),
            mch_id: "1900000109".into(),
            api_key: "192006250b4c09247ec02edce69f6a2d".into(),
            sign_type: SignType::Md5,
            sandbox: false,
        }
    }

    #[test]
    fn sign_payload_filters_sign_and_empty() {
        // 微信官方文档的签名示例参数（去掉 sign 与空值后按字典序）
        let mut params = BTreeMap::new();
        params.insert("appid".to_string(), "wxd930ea5d5a258f4f".to_string());
        params.insert("mch_id".to_string(), "10000100".to_string());
        params.insert("device_info".to_string(), "1000".to_string());
        params.insert("body".to_string(), "test".to_string());
        params.insert("nonce_str".to_string(), "ibuaiVcKdpRxkhJA".to_string());
        params.insert("sign".to_string(), "IGNORED".to_string());
        params.insert("empty".to_string(), String::new());
        let payload = sign_payload(&params, "192006250b4c09247ec02edce69f6a2d");
        assert_eq!(
            payload,
            "appid=wxd930ea5d5a258f4f&body=test&device_info=1000&mch_id=10000100\
             &nonce_str=ibuaiVcKdpRxkhJA&key=192006250b4c09247ec02edce69f6a2d"
        );
        assert!(!payload.contains("IGNORED"));
        assert!(!payload.contains("empty="));
    }

    #[test]
    fn sign_params_then_verify_roundtrip() {
        let cfg = config();
        let mut params = BTreeMap::new();
        params.insert("out_trade_no".to_string(), "P20260101000001".to_string());
        params.insert("total_fee".to_string(), "100".to_string());
        let signed = sign_params(params, &cfg.api_key, cfg.sign_type);
        assert_eq!(signed.get("sign_type").map(String::as_str), Some("MD5"));
        assert_eq!(
            signed.get("sign").map(String::len),
            Some(32),
            "MD5 签名是 32 位大写十六进制"
        );
        assert!(signed
            .get("sign")
            .map(|value| value.chars().all(|ch| ch.is_ascii_hexdigit() && !ch.is_lowercase()))
            .unwrap_or(false));
        verify_sign(&signed, &cfg.api_key).unwrap();

        // 改一个参与签名的字段，验签必须失败
        let mut tampered = signed.clone();
        tampered.insert("total_fee".to_string(), "1".to_string());
        assert!(verify_sign(&tampered, &cfg.api_key).is_err());

        // 缺 sign
        let mut missing = signed.clone();
        missing.remove("sign");
        assert!(verify_sign(&missing, &cfg.api_key).is_err());
    }

    #[test]
    fn hmac_sha256_signature_is_64_hex() {
        let cfg = config();
        let mut params = BTreeMap::new();
        params.insert("out_trade_no".to_string(), "P1".to_string());
        let signed = sign_params(params.clone(), &cfg.api_key, SignType::HmacSha256);
        assert_eq!(signed.get("sign_type").map(String::as_str), Some("HMAC-SHA256"));
        assert_eq!(signed.get("sign").map(String::len), Some(64));
        verify_sign(&signed, &cfg.api_key).unwrap();

        // 用 MD5 去验 HMAC 的报文必须失败（算法要按 sign_type 选）
        let mut wrong = signed.clone();
        wrong.insert("sign_type".to_string(), "MD5".to_string());
        assert!(verify_sign(&wrong, &cfg.api_key).is_err());
    }

    #[test]
    fn xml_build_and_parse_roundtrip() {
        let mut params = BTreeMap::new();
        params.insert("return_code".to_string(), "SUCCESS".to_string());
        params.insert("out_trade_no".to_string(), "P20260101000001".to_string());
        params.insert("attach".to_string(), "a&b<c>".to_string());
        let xml = build_xml(&params);
        assert!(xml.starts_with("<xml><"));
        assert!(xml.ends_with("</xml>"));
        assert!(xml.contains("<attach><![CDATA[a&b<c>]]></attach>"));

        let parsed = parse_xml(&xml);
        assert_eq!(parsed.get("return_code").map(String::as_str), Some("SUCCESS"));
        assert_eq!(
            parsed.get("attach").map(String::as_str),
            Some("a&b<c>"),
            "CDATA 中间的 & < 不能被实体解码破坏"
        );
    }

    #[test]
    fn parse_xml_handles_wechat_notify_body() {
        let xml = "<xml><appid><![CDATA[wx123]]></appid>\
                   <mch_id>1900000109</mch_id>\
                   <total_fee>1</total_fee>\
                   <return_code>SUCCESS</return_code>\
                   <coupon_fee_0><![CDATA[10]]></coupon_fee_0>\
                   </xml>";
        let fields = parse_xml(xml);
        assert_eq!(fields.get("appid").map(String::as_str), Some("wx123"));
        assert_eq!(fields.get("mch_id").map(String::as_str), Some("1900000109"));
        assert_eq!(fields.get("total_fee").map(String::as_str), Some("1"));
        assert_eq!(fields.get("coupon_fee_0").map(String::as_str), Some("10"));
    }

    #[test]
    fn reply_xml_matches_wechat_contract() {
        assert_eq!(
            reply_xml(true, "OK"),
            "<xml><return_code><![CDATA[SUCCESS]]></return_code>\
             <return_msg><![CDATA[OK]]></return_msg></xml>"
        );
        assert!(reply_xml(false, "签名校验失败").contains("FAIL"));
    }

    #[test]
    fn config_requires_key_fields() {
        assert!(WechatConfig::from_json(&serde_json::json!({"account": "1", "api_key": "k"})).is_err());
        assert!(WechatConfig::from_json(&serde_json::json!({"app_id": "a", "api_key": "k"})).is_err());
        assert!(WechatConfig::from_json(&serde_json::json!({"app_id": "a", "account": "1"})).is_err());
        let cfg = WechatConfig::from_json(&serde_json::json!({
            "app_id": "a",
            "account": "1",
            "api_key": "k",
            "sandbox": true,
        }))
        .unwrap();
        assert!(cfg.sandbox);
    }

    #[test]
    fn body_is_truncated_by_chars() {
        assert_eq!(trim_len("   订单支付  ", 128), "订单支付");
        assert_eq!(trim_len("", 128), "订单支付");
        let long = "中".repeat(100);
        assert!(trim_len(&long, 128).len() <= 128);
        assert_eq!(trim_len(&long, 128).chars().count(), 42);
    }

    #[test]
    fn nonce_is_32_alnum() {
        let value = nonce_str();
        assert_eq!(value.len(), 32);
        assert!(value.chars().all(|ch| ch.is_ascii_alphanumeric()));
        assert_ne!(value, nonce_str());
    }
}

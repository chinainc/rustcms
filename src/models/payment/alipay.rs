//! 支付宝（电脑网站支付 + 异步通知验签）。
//!
//! 两个入口：
//! - 下单：拼接网关请求并生成待签名串（固定 `sign_type=RSA2` + 证书模式）；
//! - 回调：按开放平台规范重算签名并验签。
//!
//! 待签名字符串规则（支付宝开放平台 HTTP 接口规范）：
//! **去掉 `sign` 与 `sign_type`、去掉空值**，按键名 ASCII 升序拼 `k=v&k=v`，
//! 拼串时用**参数原值**（不做 URL 编码）；RSA2 是 SHA256withRSA，
//! MD5（老接口）是 `md5(待签串 + 密钥)` 小写十六进制。

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::crypto;
use super::rsa;

/// 生产网关。
pub const GATEWAY_PROD: &str = "https://openapi.alipay.com/gateway.do";
/// 沙箱网关（非沙箱环境走 `GATEWAY_PROD`）。
pub const GATEWAY_SANDBOX: &str = "https://openapi-sandbox.dl.alipaydev.com/gateway.do";

/// 支付账户配置（`payment_accounts.pay_config`）。
///
/// 字段对应 `PayConfig`：`app_id` = 应用 APPID，
/// `app_secret` = 应用私钥（RSA2 PEM），`public_cert_path` / `cert_path` = 支付宝公钥证书。
#[derive(Debug, Clone)]
pub struct AlipayConfig {
    pub app_id: String,
    /// 应用私钥（PKCS#1 / PKCS#8 PEM 都支持）。
    pub private_key: String,
    /// 支付宝公钥（PEM 公钥或 X.509 证书，必须是文件内容而不是路径）。
    pub public_key: String,
    /// 老接口 MD5 签名用的密钥（`pay_config.api_key`，只有 `sign_type=MD5` 才用）。
    pub md5_key: String,
    pub sandbox: bool,
}

impl AlipayConfig {
    /// 从 `pay_config` JSON 组装；`public_key` 由调用方读文件后传入。
    pub fn from_json(config: &Value, public_key: &str) -> Result<Self, String> {
        let read = |key: &str| {
            config
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let app_id = read("app_id");
        if app_id.is_empty() {
            return Err("支付宝支付账户缺少 app_id".to_string());
        }
        let sandbox = match config.get("sandbox") {
            Some(Value::Bool(flag)) => *flag,
            Some(Value::Number(number)) => number.as_i64().unwrap_or(0) != 0,
            Some(Value::String(text)) => matches!(text.as_str(), "1" | "true" | "True"),
            _ => false,
        };
        Ok(Self {
            app_id,
            private_key: read("app_secret"),
            public_key: public_key.trim().to_string(),
            md5_key: read("api_key"),
            sandbox,
        })
    }

    /// 网关地址。
    pub fn gateway(&self) -> &'static str {
        if self.sandbox {
            GATEWAY_SANDBOX
        } else {
            GATEWAY_PROD
        }
    }
}

/// 待签名字符串（去 `sign` / `sign_type` / 空值，按键名升序）。
pub fn sign_string(params: &BTreeMap<String, String>) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (key, value) in params {
        if key == "sign" || key == "sign_type" || value.is_empty() {
            continue;
        }
        parts.push(format!("{key}={value}"));
    }
    parts.join("&")
}

/// 签名算法（`sign_type`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignKind {
    /// SHA256withRSA
    Rsa2,
    /// MD5（老接口）
    Md5,
}

impl SignKind {
    /// 从报文里解析，缺省按 RSA2。
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_uppercase().as_str() {
            "MD5" => Self::Md5,
            _ => Self::Rsa2,
        }
    }
}

/// 验签（回调）。
///
/// 验签流程同 `alipay.VerifySign(publicKey, bm)`；`sign_type=MD5` 时退化成 MD5 比对。
pub fn verify_sign(params: &BTreeMap<String, String>, cfg: &AlipayConfig) -> Result<(), String> {
    let sign = params
        .get("sign")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if sign.is_empty() {
        return Err("回调缺少 sign".to_string());
    }
    let kind = SignKind::parse(
        params
            .get("sign_type")
            .map(String::as_str)
            .unwrap_or("RSA2"),
    );
    let payload = sign_string(params);
    match kind {
        SignKind::Md5 => {
            if cfg.md5_key.is_empty() {
                return Err("支付宝账户没有配置 MD5 密钥（api_key），无法校验 sign_type=MD5 的通知"
                    .to_string());
            }
            let expect = crypto::md5_hex_lower(format!("{payload}{}", cfg.md5_key).as_bytes());
            if !sign.eq_ignore_ascii_case(&expect) {
                return Err("支付宝 MD5 验签失败".to_string());
            }
        }
        SignKind::Rsa2 => {
            if cfg.public_key.is_empty() {
                return Err("支付宝账户没有配置公钥证书，无法校验 RSA2 通知".to_string());
            }
            let key = rsa::parse_public_key(&cfg.public_key)
                .map_err(|err| format!("支付宝公钥解析失败：{err}"))?;
            let signature = rustcms_ext::smtp::base64_decode(&sign)
                .ok_or_else(|| "支付宝 sign 不是合法的 base64".to_string())?;
            if !rsa::verify_sha256(&key, payload.as_bytes(), &signature) {
                return Err("支付宝 RSA2 验签失败".to_string());
            }
        }
    }
    Ok(())
}

/// RFC 3986 百分号编码（保留 `A-Za-z0-9-_.~`，空格编成 `%20`）。
pub fn url_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 请求参数签名（RSA2）→ base64 字符串。
pub fn sign_params(params: &BTreeMap<String, String>, private_key: &str) -> Result<String, String> {
    let payload = sign_string(params);
    let key = rsa::parse_private_key(private_key)?;
    let signature = rsa::sign_sha256(&key, payload.as_bytes())?;
    Ok(rustcms_ext::smtp::base64_encode(&signature))
}

/// 电脑网站支付（`alipay.trade.page.pay`）的跳转地址。
///
/// 对齐 `createAlipayPayment`：`sign_type=RSA2`、`notify_url`、`return_url`、
/// `biz_content` 里带 `out_trade_no` / `total_amount` / `subject` / `product_code`。
pub fn page_pay_url(
    cfg: &AlipayConfig,
    out_trade_no: &str,
    total_amount_fen: i64,
    subject: &str,
    notify_url: &str,
    return_url: &str,
    timestamp: &str,
) -> Result<String, String> {
    if cfg.private_key.is_empty() {
        return Err("支付宝支付账户缺少应用私钥（app_secret）".to_string());
    }
    if out_trade_no.trim().is_empty() {
        return Err("支付单号不能为空".to_string());
    }
    if total_amount_fen <= 0 {
        return Err("支付金额必须大于 0".to_string());
    }
    let subject = if subject.trim().is_empty() {
        "订单支付"
    } else {
        subject.trim()
    };

    let biz_content = json!({
        "out_trade_no": out_trade_no,
        "total_amount": format_amount(total_amount_fen),
        "subject": subject,
        "product_code": "FAST_INSTANT_TRADE_PAY",
    })
    .to_string();

    let mut params: BTreeMap<String, String> = BTreeMap::new();
    params.insert("app_id".to_string(), cfg.app_id.clone());
    params.insert("method".to_string(), "alipay.trade.page.pay".to_string());
    params.insert("format".to_string(), "JSON".to_string());
    params.insert("charset".to_string(), "utf-8".to_string());
    params.insert("sign_type".to_string(), "RSA2".to_string());
    params.insert("timestamp".to_string(), timestamp.to_string());
    params.insert("version".to_string(), "1.0".to_string());
    params.insert("notify_url".to_string(), notify_url.to_string());
    if !return_url.is_empty() {
        params.insert("return_url".to_string(), return_url.to_string());
    }
    params.insert("biz_content".to_string(), biz_content);

    let sign = sign_params(&params, &cfg.private_key)?;
    let mut query: Vec<String> = params
        .iter()
        .map(|(key, value)| format!("{}={}", url_encode(key), url_encode(value)))
        .collect();
    query.push(format!("sign={}", url_encode(&sign)));
    Ok(format!("{}?{}", cfg.gateway(), query.join("&")))
}

/// 分 → 元（两位小数字符串）。
pub fn format_amount(fen: i64) -> String {
    let negative = fen < 0;
    let value = fen.unsigned_abs();
    let yuan = value / 100;
    let cent = value % 100;
    if negative {
        format!("-{yuan}.{cent:02}")
    } else {
        format!("{yuan}.{cent:02}")
    }
}

/// 当前时间（`yyyy-MM-dd HH:mm:ss`，支付宝要求本地时间）。
pub fn timestamp_now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 回调里判断是否为退款通知（有 `refund_id` 即为退款，与既有行为一致）。
pub fn is_refund_notice(params: &BTreeMap<String, String>) -> bool {
    params
        .get("refund_id")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

/// 退款通知是否成功（`refund_status=SUCCESS`）。
pub fn refund_succeeded(params: &BTreeMap<String, String>) -> bool {
    params
        .get("refund_status")
        .map(|value| value.trim() == "SUCCESS")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn sign_string_sorts_and_filters() {
        // 键序是 ASCII 升序；sign / sign_type / 空值都不参与
        let params = map(&[
            ("total_fee", "1"),
            ("app_id", "2021000000000000"),
            ("sign", "IGNORED"),
            ("sign_type", "RSA2"),
            ("empty", ""),
            ("biz_content", "{\"a\":1}"),
        ]);
        assert_eq!(
            sign_string(&params),
            "app_id=2021000000000000&biz_content={\"a\":1}&total_fee=1"
        );
    }

    #[test]
    fn md5_verify_matches_go_rule() {
        let cfg = AlipayConfig {
            app_id: "2021000000000000".into(),
            private_key: String::new(),
            public_key: String::new(),
            md5_key: "testkey".into(),
            sandbox: false,
        };
        let payload = "app_id=2021000000000000&out_trade_no=P1&total_amount=1.00";
        let sign = crypto::md5_hex_lower(format!("{payload}{}", cfg.md5_key).as_bytes());
        let params = map(&[
            ("app_id", "2021000000000000"),
            ("out_trade_no", "P1"),
            ("total_amount", "1.00"),
            ("sign_type", "MD5"),
            ("sign", sign.as_str()),
        ]);
        verify_sign(&params, &cfg).unwrap();

        let mut tampered = params.clone();
        tampered.insert("total_amount".to_string(), "0.01".to_string());
        assert!(verify_sign(&tampered, &cfg).is_err());

        // 没有 MD5 密钥时要明确报错，而不是「验签失败」误导排查
        let no_key = AlipayConfig {
            md5_key: String::new(),
            ..cfg.clone()
        };
        let err = verify_sign(&params, &no_key).unwrap_err();
        assert!(err.contains("MD5 密钥"));
    }

    /// RSA2 正向验签：用 512 位玩具密钥（e=3）签出的真实 PKCS#1 v1.5(SHA256) 签名，
    /// 走完整的 `verify_sign`（取 sign_type → 待签串 → ASN.1 解析公钥 → 模幂 → 摘要比对）。
    #[test]
    fn rsa2_verify_accepts_fixed_vector() {
        let cfg = AlipayConfig {
            app_id: "2021000000000000".into(),
            private_key: String::new(),
            public_key: super::rsa::RSA2_PUBLIC_KEY_PEM.into(),
            md5_key: String::new(),
            sandbox: false,
        };
        let params = map(&[
            ("app_id", "2021000000000000"),
            ("out_trade_no", "P20260101000001"),
            ("total_amount", "1.00"),
            ("sign_type", "RSA2"),
            ("sign", super::rsa::RSA2_SIGNATURE_B64),
        ]);
        // 待签串必须与签名时的原文逐字节一致
        assert_eq!(sign_string(&params), super::rsa::RSA2_SIGNED_MESSAGE);
        verify_sign(&params, &cfg).unwrap();

        // 改金额 → 待签串变化 → 验签失败
        let mut tampered = params.clone();
        tampered.insert("total_amount".to_string(), "0.01".to_string());
        assert!(verify_sign(&tampered, &cfg).is_err());

        // 换一条通知（多一个参数）→ 待签串变化 → 验签失败
        let mut extra = params.clone();
        extra.insert("trade_no".to_string(), "2026010122001".to_string());
        assert!(verify_sign(&extra, &cfg).is_err());

        // 用同一把公钥去验「MD5 声明」的报文也必须失败（算法不能混用）
        let mut wrong_algo = params.clone();
        wrong_algo.insert("sign_type".to_string(), "MD5".to_string());
        assert!(verify_sign(&wrong_algo, &cfg).is_err());
    }

    #[test]
    fn rsa2_verify_rejects_missing_key_and_bad_signature() {
        let cfg = AlipayConfig {
            app_id: "2021000000000000".into(),
            private_key: String::new(),
            public_key: String::new(),
            md5_key: String::new(),
            sandbox: false,
        };
        let params = map(&[
            ("out_trade_no", "P1"),
            ("sign_type", "RSA2"),
            ("sign", "QUJD"),
        ]);
        let err = verify_sign(&params, &cfg).unwrap_err();
        assert!(err.contains("公钥证书"));

        // 公钥不是合法 PEM → 解析失败
        let cfg = AlipayConfig {
            public_key: "not-a-key".into(),
            ..cfg
        };
        let err = verify_sign(&params, &cfg).unwrap_err();
        assert!(err.contains("公钥解析失败"));
    }

    #[test]
    fn sign_type_defaults_to_rsa2() {
        assert_eq!(SignKind::parse(""), SignKind::Rsa2);
        assert_eq!(SignKind::parse("rsa2"), SignKind::Rsa2);
        assert_eq!(SignKind::parse("MD5"), SignKind::Md5);
    }

    #[test]
    fn url_encode_is_rfc3986() {
        assert_eq!(url_encode("a b"), "a%20b");
        assert_eq!(url_encode("a+b"), "a%2Bb");
        assert_eq!(url_encode("~-_.!*'()"), "~-_.%21%2A%27%28%29");
        assert_eq!(url_encode("中文"), "%E4%B8%AD%E6%96%87");
        assert_eq!(url_encode("abcXYZ0189"), "abcXYZ0189");
    }

    #[test]
    fn amount_formatting_matches_go() {
        assert_eq!(format_amount(1), "0.01");
        assert_eq!(format_amount(100), "1.00");
        assert_eq!(format_amount(123_456), "1234.56");
        assert_eq!(format_amount(-5), "-0.05");
        assert_eq!(format_amount(0), "0.00");
    }

    #[test]
    fn page_pay_url_requires_private_key() {
        let cfg = AlipayConfig {
            app_id: "2021000000000000".into(),
            private_key: String::new(),
            public_key: String::new(),
            md5_key: String::new(),
            sandbox: false,
        };
        let err = page_pay_url(&cfg, "P1", 100, "订单", "https://a/n", "https://a/r", "2026-01-01 00:00:00")
            .unwrap_err();
        assert!(err.contains("应用私钥"));

        // 私钥不是合法 PEM → 解析失败（说明确实走到了签名环节）
        let cfg = AlipayConfig {
            private_key: "-----BEGIN PRIVATE KEY-----\nQUJD\n-----END PRIVATE KEY-----".into(),
            ..cfg
        };
        let err = page_pay_url(&cfg, "P1", 100, "订单", "https://a/n", "https://a/r", "2026-01-01 00:00:00")
            .unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn refund_notice_detection() {
        let params = map(&[("refund_id", "R1"), ("refund_status", "SUCCESS")]);
        assert!(is_refund_notice(&params));
        assert!(refund_succeeded(&params));
        let params = map(&[("refund_id", "R1"), ("refund_status", "FAILED")]);
        assert!(!refund_succeeded(&params));
        let params = map(&[("out_trade_no", "P1")]);
        assert!(!is_refund_notice(&params));
    }

    #[test]
    fn gateway_switch() {
        let mut cfg = AlipayConfig {
            app_id: "a".into(),
            private_key: String::new(),
            public_key: String::new(),
            md5_key: String::new(),
            sandbox: false,
        };
        assert_eq!(cfg.gateway(), GATEWAY_PROD);
        cfg.sandbox = true;
        assert_eq!(cfg.gateway(), GATEWAY_SANDBOX);
    }
}

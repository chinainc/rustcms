//! 翻译插件（`settings.translate` + `translate_logs` / `translate_text_logs` /
//! `translate_html_logs` 表）。
//!
//! 结构：
//! - 配置存 `settings.translate`（引擎 / 语言对与各引擎凭据，共 6 个字段）；
//! - 翻译日志（整页翻译）与文本翻译日志是两张表，接口只做分页 / 修正 / 删除；
//! - **翻译引擎外呼已实现**：百度（`translate_one` + `baidu_sign` 的 appid+q 分片 MD5 签名）、
//! 有道（`youdao_sign` 的 sha256 双截断签名）、DeepL（`deepl_base` 免费/付费端点切换），
//! 语言码映射分别由 `baidu_code` / `youdao_lang` / `deepl_payload` 负责；
//! 外呼统一走 `rustcms_ext::http`，凭据缺失时返回中文错误而不是静默成功；
//! - 批量与整页：`translate_texts` 按批合并请求（行数与请求条数对不上就退回逐段翻译，
//! 防止错位），`translate_html` 只翻标签之间的文字（`split_html` / `rebuild_html`）；
//! - 修正译文后，既有实现会异步把 `cache/multiLang/<lang>/<md5(uri)>` 缓存文件里的
//! 旧译文替换掉，这里同步执行同样的替换（`ReplaceTranslateText` 的 4 组正则）。
//!
//! ⚠️ 本模块的单测只覆盖签名构造、语言映射、结果解析与 HTML 分片等纯逻辑，
//! 不发起真实网络请求（本机无引擎凭据）。

use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use sqlx::Row;

use super::{attachment, site_setting, ModelError};

/// 翻译配置键（`TranslateSettingKey`）。
pub const SETTING_KEY: &str = "translate";

/// 配置完整默认值（对应 `PluginTranslateConfig` 的零值）。
fn default_config() -> Value {
    json!({
        "engine": "",
        "baidu_app_id": "",
        "baidu_app_secret": "",
        "youdao_app_key": "",
        "youdao_app_secret": "",
        "deepl_auth_key": "",
    })
}

/// 读翻译插件配置。
pub async fn config_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let stored = site_setting::get_section(pool, SETTING_KEY).await?;
    let mut base = default_config();
    if let (Value::Object(base_map), Value::Object(stored_map)) = (&mut base, &stored) {
        for (key, value) in stored_map {
            base_map.insert(key.clone(), value.clone());
        }
    }
    Ok(base)
}

/// 保存翻译插件配置（字段级 merge）。
pub async fn config_save(pool: &MySqlPool, patch: &Value) -> anyhow::Result<Value> {
    site_setting::save_section(pool, SETTING_KEY, patch).await?;
    config_get(pool).await
}

// ─────────────────────────── 翻译引擎（外呼） ───────────────────────────
//
// 签名与端点：
// - 引擎在 `settings.translate.engine` 里选（空 = 走云端 `/translate`，见 `cloud::cloud_translate`）；
// - 百度签名 `md5(appid + q + salt + 密钥)`，服务端 QPS = 1（进程内 1 秒节流，
// 串行执行 + 每次间隔 1 秒）；
// - 有道签名 `sha256(appKey + truncate(q) + salt + curtime + 密钥)`，`signType=v3`；
// - DeepL 用 `Authorization: DeepL-Auth-Key`，`auth_key` 以 `:fx` 结尾时走免费版域名；
// - 译文按 `md5(language-to_language-text)` 缓存在 `translate_text_logs`，命中不发请求。

/// 百度翻译接口。
const BAIDU_ENDPOINT: &str = "https://fanyi-api.baidu.com/api/trans/vip/translate";
/// 有道翻译接口。
const YOUDAO_ENDPOINT: &str = "https://openapi.youdao.com/api";
/// DeepL 付费版根地址。
const DEEPL_ENDPOINT: &str = "https://api.deepl.com/v2";
/// DeepL 免费版根地址：`auth_key` 以 `:fx` 结尾时使用。
const DEEPL_FREE_ENDPOINT: &str = "https://api-free.deepl.com/v2";

/// 没有选择翻译引擎（词条 `NoAiGenerationSourceSelected`）。
pub const ERR_NO_ENGINE: &str = "没有选择AI生成来源";
/// 缺少目标语言（词条 `PleaseSelectTargetLanguage`）。
pub const ERR_NO_TO_LANGUAGE: &str = "请选择目标语言";
/// 空内容本地拦截，直接返回固定文案。
pub const ERR_EMPTY_CONTENT: &str = "翻译内容不能为空";

/// 语言表：`(语言代码, 百度语言代码)`，共 87 项。
const LANGUAGE_BAIDU: &[(&str, &str)] = &[
    ("en", "en"),
    ("zh-CN", "zh"),
    ("zh-TW", "cht"),
    ("vi", "vie"),
    ("id", "id"),
    ("hi", "hi"),
    ("it", "it"),
    ("el", "el"),
    ("es", "spa"),
    ("pt", "pt"),
    ("sr", "srp"),
    ("my", "bur"),
    ("bn", "ben"),
    ("th", "th"),
    ("tr", "tr"),
    ("ja", "jp"),
    ("lo", "lao"),
    ("ko", "kor"),
    ("ru", "ru"),
    ("fr", "fra"),
    ("de", "de"),
    ("fa", "per"),
    ("ar", "ara"),
    ("jw", "jav"),
    ("te", "tel"),
    ("ta", "tam"),
    ("mr", "mar"),
    ("ur", "urd"),
    ("pl", "pl"),
    ("uk", "ukr"),
    ("pa", "pan"),
    ("ro", "rom"),
    ("et", "est"),
    ("os", "oss"),
    ("be", "bel"),
    ("bg", "bul"),
    ("is", "ice"),
    ("bs", "bos"),
    ("bo", "tib"),
    ("da", "dan"),
    ("tl", "tgl"),
    ("fi", "fin"),
    ("sv", "swe"),
    ("ka", "geo"),
    ("kk", "kaz"),
    ("gl", "glg"),
    ("ky", "kir"),
    ("nl", "nl"),
    ("ca", "cat"),
    ("cs", "cs"),
    ("kn", "kan"),
    ("mn", "mon"),
    ("hr", "hrv"),
    ("lv", "lav"),
    ("lt", "lit"),
    ("no", "nor"),
    ("ne", "nep"),
    ("ps", "pus"),
    ("tk", "tuk"),
    ("uz", "uzb"),
    ("iw", "heb"),
    ("hu", "hu"),
    ("hy", "arm"),
    ("ht", "ht"),
    ("mg", "mg"),
    ("mk", "mac"),
    ("ml", "mal"),
    ("af", "afr"),
    ("mt", "mlt"),
    ("eu", "baq"),
    ("pt-PT", "pt"),
    ("az", "aze"),
    ("en-GB", "en"),
    ("sd", "snd"),
    ("si", "sin"),
    ("sk", "sk"),
    ("sl", "slo"),
    ("ga", "gle"),
    ("so", "som"),
    ("sq", "alb"),
    ("km", "hkm"),
    ("sw", "swa"),
    ("pt-BR", "pt"),
    ("gu", "guj"),
    ("tg", "tgk"),
    ("la", "lat"),
    ("cy", "wel"),
];

/// 语言代码 → 百度语言代码。
///
/// 对齐 `GetLanguageBaiduCode`：带 `-` 时把后半段转大写再查表，
/// 查不到一律回 `auto`（`zh-cn` → `zh-CN` → `zh`，`zh-tw` → `zh-TW` → `cht`）。
pub fn baidu_code(lang: &str) -> String {
    let normalized = match lang.split_once('-') {
        Some((head, tail)) => format!("{head}-{}", tail.to_ascii_uppercase()),
        None => lang.to_string(),
    };
    LANGUAGE_BAIDU
        .iter()
        .find(|(code, _)| *code == normalized)
        .map(|(_, baidu)| (*baidu).to_string())
        .unwrap_or_else(|| "auto".to_string())
}

/// 翻译引擎。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// 百度翻译
    Baidu,
    /// 有道翻译
    Youdao,
    /// DeepL
    Deepl,
    /// 自建 AI（`settings.ai_setting` 里的第一个可用接口）
    Ai,
    /// Google 翻译：保留引擎标识但没有实现（选中会直接报错）
    Google,
}

impl Engine {
    /// 配置里的引擎标识 → 枚举（`TranslateEngine*`）。
    pub fn from_key(key: &str) -> Option<Self> {
        match key.trim() {
            "baidu" => Some(Self::Baidu),
            "youdao" => Some(Self::Youdao),
            "deepl" => Some(Self::Deepl),
            "ai" => Some(Self::Ai),
            "google" => Some(Self::Google),
            _ => None,
        }
    }

    /// 引擎的中文名（错误提示用）。
    pub fn label(&self) -> &'static str {
        match self {
            Self::Baidu => "百度翻译",
            Self::Youdao => "有道翻译",
            Self::Deepl => "DeepL",
            Self::Ai => "AI 翻译",
            Self::Google => "Google 翻译",
        }
    }
}

/// `settings.translate` 的强类型视图。
#[derive(Debug, Clone, Default)]
pub struct EngineConfig {
    pub engine: String,
    pub baidu_app_id: String,
    pub baidu_app_secret: String,
    pub youdao_app_key: String,
    pub youdao_app_secret: String,
    pub deepl_auth_key: String,
}

impl EngineConfig {
    /// 从配置 JSON 读出引擎与凭据。
    pub fn from_value(value: &Value) -> Self {
        let read = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        Self {
            engine: read("engine"),
            baidu_app_id: read("baidu_app_id"),
            baidu_app_secret: read("baidu_app_secret"),
            youdao_app_key: read("youdao_app_key"),
            youdao_app_secret: read("youdao_app_secret"),
            deepl_auth_key: read("deepl_auth_key"),
        }
    }

    /// 已选中的引擎；`None` 表示走云端官方接口。
    pub fn engine(&self) -> Option<Engine> {
        Engine::from_key(&self.engine)
    }

    /// 凭据是否齐全；不齐时返回可直接展示的中文原因。
    pub fn missing(&self) -> Option<String> {
        let empty = |value: &str| value.trim().is_empty();
        match self.engine()? {
            Engine::Baidu if empty(&self.baidu_app_id) || empty(&self.baidu_app_secret) => {
                Some("请先填写百度翻译的 APPID 和密钥".to_string())
            }
            Engine::Youdao if empty(&self.youdao_app_key) || empty(&self.youdao_app_secret) => {
                Some("请先填写有道翻译的 AppKey 和 AppSecret".to_string())
            }
            Engine::Deepl if empty(&self.deepl_auth_key) => {
                Some("请先填写 DeepL 的 Auth Key".to_string())
            }
            _ => None,
        }
    }
}

/// 读引擎配置（读不到按零值处理，避免把配置读取失败伪装成翻译失败）。
pub async fn engine_config(pool: &MySqlPool) -> EngineConfig {
    match config_get(pool).await {
        Ok(value) => EngineConfig::from_value(&value),
        Err(err) => {
            tracing::warn!("读取翻译插件配置失败：{err:#}");
            EngineConfig::default()
        }
    }
}

/// 截断异常正文，避免把整段响应塞进错误提示。
fn short_text(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// JSON 标量转字符串（百度/有道的错误码既可能是字符串也可能是数字）。
fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        _ => String::new(),
    }
}

// ── SHA-256（有道 v3 签名用；项目未引入摘要库，这里自带一份标准实现）──

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// 标准 SHA-256。
fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let mut message = data.to_vec();
    let bit_len = (data.len() as u64).saturating_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks(64) {
        let mut w = [0u32; 64];
        for (index, slot) in w.iter_mut().take(16).enumerate() {
            let start = index * 4;
            *slot = u32::from_be_bytes([
                chunk[start],
                chunk[start + 1],
                chunk[start + 2],
                chunk[start + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h) = (
            state[0], state[1], state[2], state[3], state[4], state[5], state[6], state[7],
        );
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    let mut output = [0u8; 32];
    for (index, word) in state.iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    output
}

/// SHA-256 小写十六进制（有道签名）。
fn sha256_hex(text: &str) -> String {
    sha256(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// ── 百度 ──

/// 百度签名串：`appid + q + salt + 密钥`。
pub fn baidu_sign(app_id: &str, query: &str, salt: &str, secret: &str) -> String {
    attachment::md5_hex(format!("{app_id}{query}{salt}{secret}").as_bytes())
}

/// 百度翻译的请求参数。
pub fn baidu_form(
    app_id: &str,
    query: &str,
    salt: &str,
    secret: &str,
    from: &str,
    to: &str,
) -> Vec<(String, String)> {
    vec![
        ("appid".to_string(), app_id.to_string()),
        ("q".to_string(), query.to_string()),
        ("salt".to_string(), salt.to_string()),
        ("from".to_string(), baidu_code(from)),
        ("to".to_string(), baidu_code(to)),
        ("sign".to_string(), baidu_sign(app_id, query, salt, secret)),
    ]
}

/// 百度错误码 → 中文说明（服务端没给 `error_msg` 时兜底）。
fn baidu_error_hint(code: &str) -> &'static str {
    match code {
        "52001" => "请求超时，请重试",
        "52002" => "系统错误，请重试",
        "52003" => "未授权用户，请检查 APPID",
        "54000" => "必填参数为空",
        "54001" => "签名错误，请检查密钥",
        "54003" => "访问频率受限，请降低调用频率",
        "54004" => "账户余额不足",
        "54005" => "长文本请求频繁，请稍后重试",
        "58000" => "客户端 IP 非法，请检查百度翻译的 IP 白名单",
        "58001" => "译文语言方向不支持",
        "58002" => "服务当前已关闭",
        "90107" => "认证未通过或未生效",
        _ => "请检查翻译配置",
    }
}

/// 解析百度翻译响应（`trans_result[].dst` 依次拼接）。
pub fn parse_baidu_result(text: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|_| format!("百度翻译返回异常：{}", short_text(text, 200)))?;
    let code = value.get("error_code").map(scalar_text).unwrap_or_default();
    if !code.is_empty() {
        // 直接回服务端的 `error_msg`；为空时用错误码兜底成中文。
        let message = value
            .get("error_msg")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        return Err(if message.is_empty() {
            format!("百度翻译失败（错误码 {code}）：{}", baidu_error_hint(&code))
        } else {
            message
        });
    }
    let translated: String = value
        .get("trans_result")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("dst").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default();
    if translated.is_empty() {
        return Err("百度翻译返回空结果".to_string());
    }
    Ok(translated)
}

// ── 有道 ──

/// 有道语言代码：`zh-cn` → `zh-CHS`、`zh-tw` → `zh-CHT`，其余原样（`Translate`）。
pub fn youdao_lang(lang: &str) -> String {
    match lang.trim().to_ascii_lowercase().as_str() {
        "zh-cn" => "zh-CHS".to_string(),
        "zh-tw" => "zh-CHT".to_string(),
        _ => lang.trim().to_string(),
    }
}

/// 有道的 `truncate(q)`：≤20 个字符原样，否则「前 10 + 字符数 + 后 10」。
pub fn youdao_truncate(query: &str) -> String {
    let runes: Vec<char> = query.chars().collect();
    if runes.len() <= 20 {
        return query.to_string();
    }
    let head: String = runes[..10].iter().collect();
    let tail: String = runes[runes.len() - 10..].iter().collect();
    format!("{head}{}{tail}", runes.len())
}

/// 有道签名：`sha256(appKey + truncate(q) + salt + curtime + 密钥)`。
pub fn youdao_sign(
    app_key: &str,
    query: &str,
    salt: &str,
    curtime: &str,
    secret: &str,
) -> String {
    sha256_hex(&format!(
        "{app_key}{}{salt}{curtime}{secret}",
        youdao_truncate(query)
    ))
}

/// 有道翻译的请求参数。
pub fn youdao_form(
    app_key: &str,
    query: &str,
    salt: &str,
    curtime: &str,
    secret: &str,
    from: &str,
    to: &str,
) -> Vec<(String, String)> {
    vec![
        ("appKey".to_string(), app_key.to_string()),
        ("q".to_string(), query.to_string()),
        ("salt".to_string(), salt.to_string()),
        ("from".to_string(), youdao_lang(from)),
        ("to".to_string(), youdao_lang(to)),
        ("curtime".to_string(), curtime.to_string()),
        ("signType".to_string(), "v3".to_string()),
        (
            "sign".to_string(),
            youdao_sign(app_key, query, salt, curtime, secret),
        ),
    ]
}

/// 有道错误码 → 中文说明（`translation` 为空时兜底）。
fn youdao_error_hint(code: &str) -> &'static str {
    match code {
        "101" => "缺少必填参数",
        "102" => "不支持的语言类型",
        "103" => "翻译文本过长",
        "108" => "应用 ID 无效",
        "110" => "无相关服务的有效实例",
        "111" => "开发者账号无效",
        "112" => "请求服务无效",
        "113" => "翻译内容不能为空",
        "202" => "签名检验失败",
        "203" => "访问 IP 不在可访问 IP 列表",
        "301" => "辞典查询失败",
        "302" => "翻译查询失败",
        "401" => "账户已欠费",
        "411" => "访问频率受限",
        "412" => "长请求过于频繁",
        _ => "请检查翻译配置",
    }
}

/// 解析有道翻译响应（`translation` 数组依次拼接）。
pub fn parse_youdao_result(text: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|_| format!("有道翻译返回异常：{}", short_text(text, 200)))?;
    let translated: String = value
        .get("translation")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    if translated.is_empty() {
        let code = value.get("errorCode").map(scalar_text).unwrap_or_default();
        return Err(if code.is_empty() || code == "0" {
            "有道翻译返回空结果".to_string()
        } else {
            format!("有道翻译失败（错误码 {code}）：{}", youdao_error_hint(&code))
        });
    }
    Ok(translated)
}

// ── DeepL ──

/// DeepL 根地址：`auth_key` 以 `:fx` 结尾表示免费版。
pub fn deepl_base(auth_key: &str) -> &'static str {
    if auth_key.trim_end().ends_with(":fx") {
        DEEPL_FREE_ENDPOINT
    } else {
        DEEPL_ENDPOINT
    }
}

/// DeepL 请求体：源语言与目标语言统一转大写。
pub fn deepl_payload(text: &str, from: &str, to: &str) -> Value {
    serde_json::json!({
        "text": [text],
        "target_lang": to.trim().to_ascii_uppercase(),
        "source_lang": from.trim().to_ascii_uppercase(),
    })
}

/// DeepL 的 HTTP 状态码 → 中文说明。
fn deepl_error_hint(status: u16) -> &'static str {
    match status {
        403 => "DeepL 认证失败，请检查 Auth Key 是否有效",
        413 => "DeepL 请求文本过长",
        429 => "DeepL 请求过于频繁，请稍后重试",
        456 => "DeepL 本月额度已用尽",
        500 | 502 | 503 | 504 => "DeepL 服务暂时不可用，请稍后重试",
        _ => "DeepL 接口返回异常",
    }
}

/// 解析 DeepL 响应（`translations` 之间用换行拼接，与既有行为一致）。
pub fn parse_deepl_result(text: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|_| format!("DeepL 返回异常：{}", short_text(text, 200)))?;
    let translated = value
        .get("translations")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if translated.is_empty() {
        return Err("DeepL 返回空结果".to_string());
    }
    Ok(translated)
}

/// 百度 QPS = 1：进程内串行 + 两次调用间隔 1 秒（替代 `baiduChan` + sleep）。
fn baidu_gate() -> &'static tokio::sync::Mutex<Option<std::time::Instant>> {
    static GATE: std::sync::OnceLock<tokio::sync::Mutex<Option<std::time::Instant>>> =
        std::sync::OnceLock::new();
    GATE.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// 生成随机盐（5 位）。
fn random_salt() -> String {
    use rand::Rng;
    const POOL: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..5)
        .map(|_| POOL[rng.gen_range(0..POOL.len())] as char)
        .collect()
}

/// 调一次翻译引擎（不含缓存）。
///
/// `pool` 只在引擎为 `ai` 时需要（要读 `settings.ai_setting` 里的接口配置）。
pub async fn translate_one(
    pool: Option<&MySqlPool>,
    cfg: &EngineConfig,
    text: &str,
    from: &str,
    to: &str,
) -> Result<String, String> {
    if text.trim().is_empty() {
        // 与既有行为一致：空内容本地拦截，不发请求。
        return Err(ERR_EMPTY_CONTENT.to_string());
    }
    let engine = cfg.engine().ok_or_else(|| ERR_NO_ENGINE.to_string())?;
    if let Some(missing) = cfg.missing() {
        return Err(missing);
    }
    let from = if from.trim().is_empty() {
        "auto"
    } else {
        from.trim()
    };
    let to = to.trim();
    if to.is_empty() {
        return Err(ERR_NO_TO_LANGUAGE.to_string());
    }

    match engine {
        Engine::Baidu => {
            // 百度 QPS = 1：拿锁 → 距上次不足 1 秒先等 → 计时 → 释放后再发请求。
            let mut slot = baidu_gate().lock().await;
            let now = std::time::Instant::now();
            if let Some(last) = *slot {
                let elapsed = now.saturating_duration_since(last);
                if elapsed < std::time::Duration::from_secs(1) {
                    tokio::time::sleep(std::time::Duration::from_secs(1) - elapsed).await;
                }
            }
            *slot = Some(std::time::Instant::now());
            drop(slot);

            let salt = random_salt();
            let form = baidu_form(
                &cfg.baidu_app_id,
                text,
                &salt,
                &cfg.baidu_app_secret,
                from,
                to,
            );
            let request = rustcms_ext::http::Request::post(BAIDU_ENDPOINT)
                .with_form(form)
                .with_timeout(std::time::Duration::from_secs(30));
            let response = rustcms_ext::http::send(&request)
                .await
                .map_err(|err| format!("百度翻译请求失败：{err}"))?;
            if !response.is_success() {
                return Err(format!("百度翻译请求失败（HTTP {}）", response.status));
            }
            parse_baidu_result(&response.text())
        }
        Engine::Youdao => {
            let salt = random_salt();
            let curtime = chrono::Utc::now().timestamp().to_string();
            let form = youdao_form(
                &cfg.youdao_app_key,
                text,
                &salt,
                &curtime,
                &cfg.youdao_app_secret,
                from,
                to,
            );
            let request = rustcms_ext::http::Request::post(YOUDAO_ENDPOINT)
                .with_form(form)
                .with_timeout(std::time::Duration::from_secs(30));
            let response = rustcms_ext::http::send(&request)
                .await
                .map_err(|err| format!("有道翻译请求失败：{err}"))?;
            if !response.is_success() {
                return Err(format!("有道翻译请求失败（HTTP {}）", response.status));
            }
            parse_youdao_result(&response.text())
        }
        Engine::Deepl => {
            let link = format!("{}/translate", deepl_base(&cfg.deepl_auth_key));
            let request = rustcms_ext::http::Request::post(link)
                .with_header(
                    "Authorization",
                    format!("DeepL-Auth-Key {}", cfg.deepl_auth_key),
                )
                .with_json(deepl_payload(text, from, to))
                .with_timeout(std::time::Duration::from_secs(60));
            let response = rustcms_ext::http::send(&request)
                .await
                .map_err(|err| format!("DeepL 请求失败：{err}"))?;
            if !response.is_success() {
                // 403/456 等状态码要翻成可执行的中文提示，别只回 HTTP 码。
                let message = response
                    .json()
                    .ok()
                    .and_then(|value| {
                        value
                            .get("message")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                return Err(if message.trim().is_empty() {
                    format!("{}（HTTP {}）", deepl_error_hint(response.status), response.status)
                } else {
                    format!("{}（{}）", deepl_error_hint(response.status), message.trim())
                });
            }
            parse_deepl_result(&response.text())
        }
        Engine::Ai => {
            // 自建 AI 翻译走 `settings.ai_setting` 里的第一个可用接口（OpenAI 兼容），
            // 提示词与既有实现 `SelfAiTranslate` 同义：只回译文、不要额外内容。
            let Some(pool) = pool else {
                return Err("AI 翻译需要先配置 AI 接口".to_string());
            };
            let prompt = format!(
                "请将下列文字翻译成{to}，请直接返回翻译结果，不要返回任何其他额外的内容：\n{text}"
            );
            super::cloud::ai_complete(pool, &prompt).await
        }
        // google 引擎同样走「没有实现」的分支（回中文错误），
        // 这里给一句能照着改的提示，而不是悄悄退化成云端翻译。
        Engine::Google => Err("暂不支持 Google 翻译，请改用百度 / 有道 / DeepL".to_string()),
    }
}

/// 本地可直接处理的文本（纯数字/标点/备案号），返回 `(结果, 是否还需要翻译)`。
///
/// 逐条对齐 `localReplace`：单个中文标点换成英文标点；纯数字与点、
/// 以及备案号格式（`京ICP备12345678号-1`）原样返回。
pub fn local_replace(text: &str) -> (String, bool) {
    if text.len() < 2 {
        return (text.to_string(), false);
    }
    let runes: Vec<char> = text.chars().collect();
    if runes.len() == 1 {
        let mapped = match runes[0] {
            '，' => Some(","),
            '。' => Some("."),
            '；' => Some(";"),
            '：' => Some(":"),
            '？' => Some("?"),
            '！' => Some("!"),
            '（' => Some("("),
            '）' => Some(")"),
            '【' => Some("["),
            '】' => Some("]"),
            '《' => Some("<"),
            '》' => Some(">"),
            '“' | '”' => Some("\""),
            '‘' | '’' => Some("'"),
            '、' => Some("/"),
            '～' => Some("~"),
            _ => None,
        };
        return match mapped {
            Some(value) => (value.to_string(), false),
            None => (text.to_string(), true),
        };
    }

    // 纯数字与点/空格/冒号/横线/括号：不需要翻译
    let only_number = runes.iter().all(|ch| {
        ch.is_ascii_digit()
            || matches!(ch, '.' | ' ' | ':' | '-' | '(' | ')')
    });
    if only_number {
        return (text.to_string(), false);
    }

    // 备案号：京ICP备12345678号 / 粤ICP备12345678号-1
    if let Ok(re) = regex::Regex::new(
        r"^[京津沪渝冀豫云辽黑湘皖鲁新苏浙赣鄂桂甘晋蒙陕吉闽贵粤青藏川宁琼使领][A-Z]{2,3}备\d{6,10}号(-\d+)?$",
    ) {
        if re.is_match(text) {
            return (text.to_string(), false);
        }
    }

    (text.to_string(), true)
}

/// 写一条文本翻译缓存（命中则更新译文，等价 `FirstOrCreate`）。
async fn cache_put(
    pool: &MySqlPool,
    language: &str,
    to_language: &str,
    text: &str,
    translated: &str,
) {
    let md5 = attachment::md5_hex(format!("{language}-{to_language}-{text}").as_bytes());
    let existing: Option<u32> =
        sqlx::query_scalar("SELECT id FROM translate_text_logs WHERE `md5` = ? LIMIT 1")
            .bind(&md5)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);
    match existing {
        Some(id) => {
            if let Err(err) = sqlx::query("UPDATE translate_text_logs SET `translated` = ? WHERE `id` = ?")
                .bind(translated)
                .bind(id)
                .execute(pool)
                .await
            {
                tracing::warn!("更新翻译缓存失败：{err}");
            }
        }
        None => {
            let now = chrono::Utc::now().timestamp();
            if let Err(err) = sqlx::query(
                "INSERT INTO translate_text_logs (created_time, `md5`, language, to_language, \
                 `text`, `translated`) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(now)
            .bind(&md5)
            .bind(language)
            .bind(to_language)
            .bind(text)
            .bind(translated)
            .execute(pool)
            .await
            {
                tracing::warn!("写入翻译缓存失败：{err}");
            }
        }
    }
}

/// 批量翻译：先吃缓存，再按引擎外呼，最后按原始顺序回填。
///
/// 对齐 `TranslateTexts`：文本去重、空串原样保留、`local_replace` 命中不打接口。
pub async fn translate_texts(
    pool: &MySqlPool,
    cfg: &EngineConfig,
    texts: &[String],
    from: &str,
    to: &str,
) -> Result<Vec<String>, String> {
    let to = to.trim();
    if to.is_empty() {
        return Err(ERR_NO_TO_LANGUAGE.to_string());
    }
    if cfg.engine().is_none() {
        return Err(ERR_NO_ENGINE.to_string());
    }
    if let Some(missing) = cfg.missing() {
        return Err(missing);
    }
    let from = if from.trim().is_empty() {
        "auto"
    } else {
        from.trim()
    };

    // 1. 去重（保序）
    let mut unique: Vec<String> = Vec::new();
    let mut slots: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut positions: Vec<Vec<usize>> = Vec::new();
    for (index, raw) in texts.iter().enumerate() {
        let trimmed = raw.trim().to_string();
        if trimmed.is_empty() {
            continue; // 空文本原样保留
        }
        match slots.get(&trimmed) {
            Some(slot) => positions[*slot].push(index),
            None => {
                slots.insert(trimmed.clone(), unique.len());
                unique.push(trimmed);
                positions.push(vec![index]);
            }
        }
    }

    // 2. 逐条解析译文（本地替换 → 缓存 → 外呼）
    let mut resolved: Vec<String> = Vec::with_capacity(unique.len());
    for text in &unique {
        let (replaced, need) = local_replace(text);
        if !need {
            resolved.push(replaced);
            continue;
        }
        let md5 = attachment::md5_hex(format!("{from}-{to}-{text}").as_bytes());
        let cached: Option<String> = sqlx::query_scalar(
            "SELECT translated FROM translate_text_logs WHERE `md5` = ? LIMIT 1",
        )
        .bind(&md5)
        .fetch_optional(pool)
        .await
        .unwrap_or(None);
        if let Some(cached) = cached {
            if !cached.is_empty() {
                resolved.push(cached);
                continue;
            }
        }
        let translated = translate_one(Some(pool), cfg, text, from, to).await?;
        cache_put(pool, from, to, text, &translated).await;
        resolved.push(translated);
    }

    // 3. 回填
    let mut out: Vec<String> = texts.to_vec();
    for (slot, _) in unique.iter().enumerate() {
        for index in &positions[slot] {
            out[*index] = resolved[slot].clone();
        }
    }
    Ok(out)
}

// ── HTML 翻译（只翻文本节点，标签与属性原样保留）──

/// HTML 片段：原样保留的标签/脚本内容，或需要翻译的文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HtmlSegment {
    /// 标签、script/style/code 内部内容
    Raw(String),
    /// 标签之间的文本
    Text(String),
}

/// 把 HTML 切成「标签」与「文本」两类片段。
///
/// `script` / `style` / `code` 内部整体当原样片段（对齐 `extractTextNodes`
/// 跳过这三类节点，避免把脚本里的英文当正文翻译掉）。
pub fn split_html(html: &str) -> Vec<HtmlSegment> {
    let Ok(re) = regex::Regex::new(r"(?s)<[^>]*>|[^<]+") else {
        return vec![HtmlSegment::Raw(html.to_string())];
    };
    let mut out = Vec::new();
    let mut skip: Option<String> = None;
    for token in re.find_iter(html) {
        let raw = token.as_str();
        if raw.starts_with('<') {
            let lower = raw.to_ascii_lowercase();
            let closing = lower.starts_with("</");
            let name: String = lower
                .trim_start_matches('<')
                .trim_start_matches('/')
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric())
                .collect();
            match &skip {
                Some(tag) => {
                    if closing && name == *tag {
                        skip = None;
                    }
                }
                None => {
                    let self_closing = lower.ends_with("/>");
                    if !closing
                        && !self_closing
                        && matches!(name.as_str(), "script" | "style" | "code")
                    {
                        skip = Some(name);
                    }
                }
            }
            out.push(HtmlSegment::Raw(raw.to_string()));
        } else if skip.is_some() {
            out.push(HtmlSegment::Raw(raw.to_string()));
        } else {
            out.push(HtmlSegment::Text(raw.to_string()));
        }
    }
    out
}

/// 用译文回填 HTML 片段（译文数量不足时保留原文）。
pub fn rebuild_html(segments: &[HtmlSegment], translated: &[String]) -> String {
    let mut out = String::new();
    let mut cursor = 0usize;
    for segment in segments {
        match segment {
            HtmlSegment::Raw(raw) => out.push_str(raw),
            HtmlSegment::Text(text) => {
                match translated.get(cursor) {
                    Some(value) => out.push_str(value),
                    None => out.push_str(text),
                }
                cursor += 1;
            }
        }
    }
    out
}

/// 翻译一段 HTML：只翻文本节点，标签原样保留。
pub async fn translate_html(
    pool: &MySqlPool,
    cfg: &EngineConfig,
    html: &str,
    from: &str,
    to: &str,
) -> Result<String, String> {
    if html.trim().is_empty() {
        return Ok(html.to_string());
    }
    let segments = split_html(html);
    let texts: Vec<String> = segments
        .iter()
        .filter_map(|segment| match segment {
            HtmlSegment::Text(text) => Some(text.clone()),
            HtmlSegment::Raw(_) => None,
        })
        .collect();
    if texts.iter().all(|text| text.trim().is_empty()) {
        return Ok(html.to_string());
    }
    let translated = translate_texts(pool, cfg, &texts, from, to).await?;
    Ok(rebuild_html(&segments, &translated))
}

/// 记录整段翻译（`AddTranslateLog`）：`md5 = md5(文本数组拼接 + "-" + 目标语言)`。
///
/// 只写 `translate_logs`（整页/整段翻译记录），逐句译文由 [`translate_texts`] 写
/// `translate_text_logs`。
pub async fn add_log(pool: &MySqlPool, language: &str, to_language: &str, texts: &[String]) {
    if texts.is_empty() {
        return;
    }
    let joined = format!("[{}]", texts.join(" "));
    let md5 = attachment::md5_hex(format!("{joined}-{to_language}").as_bytes());
    let title: String = texts[0].chars().take(190).collect();
    let existing: Option<u32> =
        sqlx::query_scalar("SELECT id FROM translate_logs WHERE `md5` = ? LIMIT 1")
            .bind(&md5)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);
    let result = match existing {
        Some(id) => {
            sqlx::query(
                "UPDATE translate_logs SET origin_title = ?, language = ?, to_language = ? \
                 WHERE `id` = ?",
            )
            .bind(&title)
            .bind(language)
            .bind(to_language)
            .bind(id)
            .execute(pool)
            .await
        }
        None => {
            let now = chrono::Utc::now().timestamp();
            sqlx::query(
                "INSERT INTO translate_logs (created_time, `md5`, origin_title, origin_content, \
                 title, content, language, to_language) VALUES (?, ?, ?, '', '', '', ?, ?)",
            )
            .bind(now)
            .bind(&md5)
            .bind(&title)
            .bind(language)
            .bind(to_language)
            .execute(pool)
            .await
        }
    };
    if let Err(err) = result {
        tracing::warn!("记录翻译日志失败：{err}");
    }
}

// ─────────────────────────── 翻译日志（整页） ───────────────────────────

/// 分页取翻译日志，`id desc`（`PluginTranslateLogList`）。
pub async fn log_list(
    pool: &MySqlPool,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Value>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size };
    let offset = (page - 1) * page_size;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM translate_logs")
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query("SELECT * FROM translate_logs ORDER BY id DESC LIMIT ? OFFSET ?")
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await?;

    let items = rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<u32, _>("id"),
                "created_time": row.get::<Option<i64>, _>("created_time").unwrap_or(0),
                "md5": row.get::<Option<String>, _>("md5").unwrap_or_default(),
                "origin_title": row.get::<Option<String>, _>("origin_title").unwrap_or_default(),
                "origin_content": row.get::<Option<String>, _>("origin_content").unwrap_or_default(),
                "title": row.get::<Option<String>, _>("title").unwrap_or_default(),
                "content": row.get::<Option<String>, _>("content").unwrap_or_default(),
                "language": row.get::<Option<String>, _>("language").unwrap_or_default(),
                "to_language": row.get::<Option<String>, _>("to_language").unwrap_or_default(),
            })
        })
        .collect();
    Ok((total, items))
}

// ─────────────────────────── 文本翻译日志 ───────────────────────────

fn text_log_json(row: &sqlx::mysql::MySqlRow) -> Value {
    json!({
        "id": row.get::<u32, _>("id"),
        "created_time": row.get::<Option<i64>, _>("created_time").unwrap_or(0),
        "md5": row.get::<Option<String>, _>("md5").unwrap_or_default(),
        "language": row.get::<Option<String>, _>("language").unwrap_or_default(),
        "to_language": row.get::<Option<String>, _>("to_language").unwrap_or_default(),
        "text": row.get::<Option<String>, _>("text").unwrap_or_default(),
        "translated": row.get::<Option<String>, _>("translated").unwrap_or_default(),
    })
}

/// 分页取文本翻译日志，可按原文 / 译文前缀筛选（`PluginGetTranslateTextLog`）。
pub async fn text_log_list(
    pool: &MySqlPool,
    page: i64,
    page_size: i64,
    text: &str,
    translated: &str,
) -> anyhow::Result<(i64, Vec<Value>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size };
    let offset = (page - 1) * page_size;

    let mut where_clause = String::from("WHERE 1 = 1");
    if !text.is_empty() {
        where_clause.push_str(&format!(" AND `text` LIKE '{}%'", text.replace('\'', "''")));
    }
    if !translated.is_empty() {
        where_clause.push_str(&format!(
            " AND `translated` LIKE '{}%'",
            translated.replace('\'', "''")
        ));
    }

    let total: i64 =
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM translate_text_logs {where_clause}"))
            .fetch_one(pool)
            .await?;
    let rows = sqlx::query(&format!(
        "SELECT * FROM translate_text_logs {where_clause} ORDER BY id DESC LIMIT ? OFFSET ?"
    ))
    .bind(page_size)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok((total, rows.iter().map(text_log_json).collect()))
}

/// 删除文本翻译日志（`PluginRemoveTranslateTextLog`）：
/// - `all = true` 清空整表；
/// - `id > 0` 删单条，**先确认存在**（删不存在的 id
/// 是 0 行受影响但不报错，会让调用方误以为删掉了）；
/// - 两者都没给：明确报错，绝不静默「删除成功」。
pub async fn text_log_remove(pool: &MySqlPool, id: u32, all: bool) -> Result<(), ModelError> {
    if all {
        sqlx::query("DELETE FROM translate_text_logs WHERE `id` > 0")
            .execute(pool)
            .await?;
        return Ok(());
    }
    if id > 0 {
        let exist: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM translate_text_logs WHERE `id` = ?")
            .bind(id)
            .fetch_one(pool)
            .await?;
        if exist == 0 {
            return Err(ModelError::invalid(format!(
                "翻译记录 id={id} 不存在，未做任何修改"
            )));
        }
        sqlx::query("DELETE FROM translate_text_logs WHERE `id` = ?")
            .bind(id)
            .execute(pool)
            .await?;
        return Ok(());
    }
    Err(ModelError::invalid(
        "请提供要删除的 id，或设置 all=true 删除全部翻译记录（当前未做任何修改）",
    ))
}

/// 文本翻译保存入参（`TranslateTextLog`）。
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct TextLogInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub to_language: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub translated: String,
}

/// 添加 / 更新文本翻译，用于修正自动翻译错误（`PluginSaveTranslateTextLog`）。
///
/// 按 `id`（优先）或 `md5(language-to_language-text)` 找已有记录做更新，否则新增；
/// 译文有变化时，同步把 `cache/multiLang/<to_language>/` 下相关 HTML 缓存里的
/// 旧译文替换掉（既有实现是异步，这里同步完成）。
pub async fn text_log_save(
    pool: &MySqlPool,
    input: &TextLogInput,
    cache_dir: &std::path::Path,
) -> Result<Value, ModelError> {
    if input.text.is_empty() {
        return Err(ModelError::invalid("请填写内容"));
    }

    let mut language = input.language.trim().to_string();
    if language.is_empty() {
        let system = site_setting::get_section(pool, site_setting::SYSTEM).await?;
        language = system
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or("zh")
            .to_string();
    }
    let to_language = input.to_language.trim().to_string();
    let text_md5 = attachment::md5_hex(format!("{language}-{to_language}-{}", input.text).as_bytes());

    let mut log_id: u32 = 0;
    let mut created_time: i64 = 0;
    let mut log_language = language.clone();
    let mut log_to_language = to_language.clone();
    let mut log_text = input.text.clone();
    let mut origin_text = input.text.clone();

    if input.id > 0 {
        if let Some(row) = sqlx::query("SELECT * FROM translate_text_logs WHERE `id` = ?")
            .bind(input.id)
            .fetch_optional(pool)
            .await?
        {
            // 更新
            log_id = row.get::<u32, _>("id");
            created_time = row.get::<Option<i64>, _>("created_time").unwrap_or(0);
            log_language = row.get::<Option<String>, _>("language").unwrap_or_default();
            log_to_language = row
                .get::<Option<String>, _>("to_language")
                .unwrap_or_default();
            log_text = row.get::<Option<String>, _>("text").unwrap_or_default();
            origin_text = row
                .get::<Option<String>, _>("translated")
                .unwrap_or_default();
        }
    } else if let Some(row) =
        sqlx::query("SELECT * FROM translate_text_logs WHERE `md5` = ?").bind(&text_md5)
            .fetch_optional(pool)
            .await?
    {
        // 更新
        log_id = row.get::<u32, _>("id");
        created_time = row.get::<Option<i64>, _>("created_time").unwrap_or(0);
        log_language = row.get::<Option<String>, _>("language").unwrap_or_default();
        log_to_language = row
            .get::<Option<String>, _>("to_language")
            .unwrap_or_default();
        log_text = row.get::<Option<String>, _>("text").unwrap_or_default();
        origin_text = row
            .get::<Option<String>, _>("translated")
            .unwrap_or_default();
    }

    if log_id > 0 {
        sqlx::query("UPDATE translate_text_logs SET `translated` = ? WHERE `id` = ?")
            .bind(&input.translated)
            .bind(log_id)
            .execute(pool)
            .await?;
    } else {
        let now = chrono::Utc::now().timestamp();
        let result = sqlx::query(
            "INSERT INTO translate_text_logs (created_time, `md5`, language, to_language, \
             `text`, `translated`) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(now)
        .bind(&text_md5)
        .bind(&log_language)
        .bind(&log_to_language)
        .bind(&log_text)
        .bind(&input.translated)
        .execute(pool)
        .await?;
        log_id = result.last_insert_id() as u32;
        created_time = now;
    }

    // 译文有变化 → 全局替换缓存里的旧译文
    if !origin_text.is_empty() && origin_text != input.translated {
        replace_html_caches(pool, cache_dir, &log_to_language, &origin_text, &input.translated)
            .await?;
    }

    Ok(json!({
        "id": log_id,
        "created_time": created_time,
        "md5": text_md5,
        "language": log_language,
        "to_language": log_to_language,
        "text": log_text,
        "translated": input.translated,
    }))
}

/// 遍历 `translate_html_logs` 里目标语言的记录，替换对应缓存文件中的旧译文
/// （异步任务，这里同步执行）。
async fn replace_html_caches(
    pool: &MySqlPool,
    cache_dir: &std::path::Path,
    to_language: &str,
    old_text: &str,
    new_text: &str,
) -> anyhow::Result<()> {
    if to_language.is_empty() || old_text.is_empty() {
        return Ok(());
    }
    let root = cache_dir.join("multiLang").join(to_language);
    let mut start_id: u32 = 0;
    loop {
        let rows = sqlx::query(
            "SELECT id, uri FROM translate_html_logs WHERE id > ? AND to_language = ? \
             ORDER BY id ASC LIMIT 100",
        )
        .bind(start_id)
        .bind(to_language)
        .fetch_all(pool)
        .await?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            start_id = row.get::<u32, _>("id");
            let uri: String = row.get::<Option<String>, _>("uri").unwrap_or_default();
            let cache_path = root.join(attachment::md5_hex(uri.as_bytes()));
            if !cache_path.is_file() {
                continue;
            }
            let Ok(buf) = std::fs::read(&cache_path) else {
                continue;
            };
            let (buf, replaced) = replace_translate_text(&buf, old_text, new_text);
            if replaced {
                let _ = std::fs::write(&cache_path, buf);
            }
        }
    }
    Ok(())
}

/// 对 HTML 字节做受控替换（`ReplaceTranslateText`）：
/// 只替换标签文本（`>...<` 之间）与 `alt` / `title` / `content` 属性里的内容。
/// 返回 `(新内容, 是否命中过旧文本)`，命中与否以「字节里包含旧文本」为准（与既有行为一致）。
pub fn replace_translate_text(buf: &[u8], old_text: &str, new_text: &str) -> (Vec<u8>, bool) {
    if old_text.is_empty() {
        return (buf.to_vec(), false);
    }
    let Ok(text) = std::str::from_utf8(buf) else {
        return (buf.to_vec(), false);
    };
    if !text.contains(old_text) {
        return (buf.to_vec(), false);
    }

    let quoted = regex::escape(old_text);
    let patterns = [
        format!(r"(>[^<]*?){quoted}([^<]*<)"),
        format!(r#"(\balt\s*=\s*["'][^"']*?){quoted}([^"']*?["'])"#),
        format!(r#"(\btitle\s*=\s*["'][^"']*?){quoted}([^"']*?["'])"#),
        format!(r#"(\bcontent\s*=\s*["'][^"']*?){quoted}([^"']*?["'])"#),
    ];
    let mut out = text.to_string();
    for pattern in patterns {
        if let Ok(re) = regex::Regex::new(&pattern) {
            out = re
                .replace_all(&out, |caps: &regex::Captures| {
                    format!("{}{new_text}{}", &caps[1], &caps[2])
                })
                .into_owned();
        }
    }
    (out.into_bytes(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, setting, test_pool};

    #[test]
    fn replace_only_touches_text_and_attrs() {
        let html = b"<p>hello</p><img alt=\"hello\" src=\"hello.png\"><span>hello world</span>";
        let (out, replaced) = replace_translate_text(html, "hello", "你好");
        assert!(replaced);
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("<p>你好</p>"), "{out}");
        assert!(out.contains("alt=\"你好\""), "{out}");
        // src 属性不在替换范围内
        assert!(out.contains("src=\"hello.png\""), "{out}");
        // 标签之间的普通文本正常替换
        assert!(out.contains("<span>你好 world</span>"), "{out}");

        // 不包含时不做任何替换
        let (out, replaced) = replace_translate_text(b"<p>abc</p>", "hello", "你好");
        assert!(!replaced);
        assert_eq!(out, b"<p>abc</p>");

        // 空旧文本直接返回
        let (out, replaced) = replace_translate_text(b"<p>x</p>", "", "y");
        assert!(!replaced);
        assert_eq!(out, b"<p>x</p>");
    }

    #[tokio::test]
    async fn config_roundtrip_and_defaults() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, SETTING_KEY).await.unwrap();

        let loaded = config_get(&pool).await.unwrap();
        assert_eq!(loaded["engine"], json!(""), "默认配置补齐：{loaded}");
        assert_eq!(loaded["baidu_app_id"], json!(""));

        let saved = config_save(&pool, &json!({"engine": "baidu", "baidu_app_id": "id-1"}))
            .await
            .unwrap();
        assert_eq!(saved["engine"], json!("baidu"));
        assert_eq!(saved["baidu_app_id"], json!("id-1"));

        // 字段级 merge：只改 engine，app_id 保留
        let saved = config_save(&pool, &json!({"engine": "deepl"})).await.unwrap();
        assert_eq!(saved["engine"], json!("deepl"));
        assert_eq!(saved["baidu_app_id"], json!("id-1"));

        match backup {
            Some(raw) => setting::save_raw(&pool, SETTING_KEY, &raw).await.unwrap(),
            None => setting::remove(&pool, SETTING_KEY).await.unwrap(),
        }
    }

    #[tokio::test]
    async fn text_log_save_find_update_remove() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let cache_dir = std::env::temp_dir().join(format!("rustcms-tl-{}", std::process::id()));
        // 清掉本次测试残留
        sqlx::query("DELETE FROM translate_text_logs WHERE `text` LIKE '__rustcms_tl_%'")
            .execute(&pool)
            .await
            .unwrap();

        let input = TextLogInput {
            to_language: "en".into(),
            text: "__rustcms_tl_正文".into(),
            translated: "hello".into(),
            ..Default::default()
        };

        // 空 text 被拒
        assert!(text_log_save(&pool, &TextLogInput::default(), &cache_dir)
            .await
            .is_err());

        let saved = text_log_save(&pool, &input, &cache_dir).await.unwrap();
        assert!(saved["id"].as_u64().unwrap() > 0, "{saved}");
        assert!(saved["md5"].as_str().unwrap().len() == 32, "{saved}");
        // language 留空时取系统默认语言
        assert!(!saved["language"].as_str().unwrap().is_empty(), "{saved}");

        // 相同 (language, to_language, text) 再次保存 → 更新而不是新增
        let again = text_log_save(&pool, &input, &cache_dir).await.unwrap();
        assert_eq!(again["id"], saved["id"], "按 md5 命中同一条：{again}");
        assert_eq!(again["translated"], json!("hello"));

        // 按 id 更新译文
        let updated = text_log_save(
            &pool,
            &TextLogInput {
                id: saved["id"].as_u64().unwrap() as u32,
                text: "__rustcms_tl_正文".into(),
                translated: "hi there".into(),
                ..Default::default()
            },
            &cache_dir,
        )
        .await
        .unwrap();
        assert_eq!(updated["id"], saved["id"]);
        assert_eq!(updated["translated"], json!("hi there"));

        // 不存在的 id 走新增分支（与既有行为一致）
        let created = text_log_save(
            &pool,
            &TextLogInput {
                id: 9_999_999,
                text: "__rustcms_tl_不存在".into(),
                translated: "x".into(),
                ..Default::default()
            },
            &cache_dir,
        )
        .await
        .unwrap();
        assert_ne!(created["id"], json!(9_999_999), "不存在的 id 应新增：{created}");
        assert_ne!(created["id"], saved["id"]);

        // 列表 + 前缀筛选（正文 + 不存在 共 2 条不同记录）
        let (total, items) = text_log_list(&pool, 1, 20, "__rustcms_tl_", "").await.unwrap();
        assert!(total >= 2, "total={total}");
        assert!(items.iter().any(|item| item["text"] == json!("__rustcms_tl_正文")));
        let (miss_total, _) = text_log_list(&pool, 1, 20, "__rustcms_tl_", "no-such").await.unwrap();
        assert_eq!(miss_total, 0);

        // 删除：id 不存在要报错
        let err = text_log_remove(&pool, 9_999_999, false).await.unwrap_err();
        assert!(err.to_string().contains("不存在"), "{err}");
        // 不带 id 也不带 all 要报错
        let err = text_log_remove(&pool, 0, false).await.unwrap_err();
        assert!(err.to_string().contains("请提供"), "{err}");
        // 单删
        text_log_remove(&pool, created["id"].as_u64().unwrap() as u32, false)
            .await
            .unwrap();
        // 清理本用例数据与临时目录
        sqlx::query("DELETE FROM translate_text_logs WHERE `text` LIKE '__rustcms_tl_%'")
            .execute(&pool)
            .await
            .unwrap();
        let _ = std::fs::remove_dir_all(&cache_dir);
    }

    // ---------------------------- 翻译引擎纯逻辑 ----------------------------

    #[test]
    fn baidu_language_codes_follow_go_table() {
        // 大小写归一：zh-cn → zh-CN → zh；zh-tw → zh-TW → cht
        assert_eq!(baidu_code("zh-cn"), "zh");
        assert_eq!(baidu_code("zh-CN"), "zh");
        assert_eq!(baidu_code("zh-TW"), "cht");
        assert_eq!(baidu_code("zh-tw"), "cht");
        // 直接命中的
        assert_eq!(baidu_code("en"), "en");
        assert_eq!(baidu_code("ja"), "jp");
        assert_eq!(baidu_code("ko"), "kor");
        assert_eq!(baidu_code("es"), "spa");
        assert_eq!(baidu_code("en-GB"), "en");
        assert_eq!(baidu_code("pt-BR"), "pt");
        // 表里没有的一律 auto（`GetLanguageBaiduCode` 的默认值）
        assert_eq!(baidu_code("xx"), "auto");
        assert_eq!(baidu_code(""), "auto");
        assert_eq!(baidu_code("auto"), "auto");
    }

    #[test]
    fn baidu_sign_matches_md5_of_concatenation() {
        // 百度规则：md5(appid + q + salt + 密钥)，三段之间**没有**分隔符
        let sign = baidu_sign("2024010100", "q中文", "salt", "secret");
        let expect = attachment::md5_hex("2024010100q中文saltsecret".as_bytes());
        assert_eq!(sign, expect);
        assert_eq!(sign.len(), 32);

        let form = baidu_form("app", "你好", "abc", "sec", "zh-CN", "en");
        let get = |key: &str| {
            form.iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        assert_eq!(get("appid"), "app");
        assert_eq!(get("q"), "你好");
        assert_eq!(get("salt"), "abc");
        // 语言代码过映射表
        assert_eq!(get("from"), "zh");
        assert_eq!(get("to"), "en");
        assert_eq!(get("sign"), baidu_sign("app", "你好", "abc", "sec"));
    }

    #[test]
    fn baidu_error_codes_map_to_chinese() {
        // 服务端给了 error_msg 就原样回（的口径）
        let err = parse_baidu_result(
            r#"{"error_code":"54001","error_msg":"Invalid Sign"}"#,
        )
        .unwrap_err();
        assert_eq!(err, "Invalid Sign");

        // 没给 msg 时用错误码兜底成中文
        let err = parse_baidu_result(r#"{"error_code":"52003"}"#).unwrap_err();
        assert!(err.contains("未授权用户"), "{err}");
        let err = parse_baidu_result(r#"{"error_code":54004}"#).unwrap_err();
        assert!(err.contains("余额不足"), "{err}");

        // 正常的：多个 dst 依序拼接
        let text = parse_baidu_result(
            r#"{"from":"zh","to":"en","trans_result":[{"src":"你好","dst":"Hello"},{"src":"世界","dst":" World"}]}"#,
        )
        .unwrap();
        assert_eq!(text, "Hello World");

        // 没有 trans_result → 空结果
        let err = parse_baidu_result(r#"{"from":"zh","to":"en"}"#).unwrap_err();
        assert!(err.contains("空结果"), "{err}");
        // 坏 JSON
        let err = parse_baidu_result("not json").unwrap_err();
        assert!(err.contains("返回异常"), "{err}");
    }

    #[test]
    fn youdao_sign_uses_truncated_query_and_sha256() {
        // truncate：≤20 字符原样
        assert_eq!(youdao_truncate("短文本"), "短文本");
        let twenty = "a".repeat(20);
        assert_eq!(youdao_truncate(&twenty), twenty);
        // >20：前 10 + 字符数 + 后 10
        let long = "0123456789abcdefghij0123456789";
        let expect = format!("0123456789{}0123456789", long.chars().count());
        assert_eq!(youdao_truncate(long), expect);

        // 语言代码：zh-cn/zh-tw 特判，其余原样
        assert_eq!(youdao_lang("zh-cn"), "zh-CHS");
        assert_eq!(youdao_lang("zh-TW"), "zh-CHT");
        assert_eq!(youdao_lang("en"), "en");

        // 签名 = sha256(appKey + truncate(q) + salt + curtime + secret)
        let sign = youdao_sign("key", "你好世界", "salt", "1700000000", "secret");
        let expect = sha256_hex("key你好世界salt1700000000secret");
        assert_eq!(sign, expect);
        assert_eq!(sign.len(), 64);

        let form = youdao_form("key", "你好", "s", "1700", "sec", "zh-cn", "en");
        let get = |key: &str| {
            form.iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        assert_eq!(get("appKey"), "key");
        assert_eq!(get("signType"), "v3");
        assert_eq!(get("curtime"), "1700");
        assert_eq!(get("from"), "zh-CHS");
        assert_eq!(get("sign"), youdao_sign("key", "你好", "s", "1700", "sec"));
    }

    #[test]
    fn sha256_matches_standard_vector() {
        // 有道签名依赖这里，必须与标准实现逐字节一致
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn youdao_result_and_error_codes() {
        let text = parse_youdao_result(
            r#"{"errorCode":"0","query":"hi","translation":["你好","世界"],"l":"en2zh-CHS"}"#,
        )
        .unwrap();
        assert_eq!(text, "你好世界");

        let err = parse_youdao_result(r#"{"errorCode":"202","translation":[]}"#).unwrap_err();
        assert!(err.contains("签名检验失败"), "{err}");
        assert!(err.contains("202"), "{err}");
        let err = parse_youdao_result(r#"{"errorCode":"0","translation":[]}"#).unwrap_err();
        assert!(err.contains("空结果"), "{err}");
        let err = parse_youdao_result("{}").unwrap_err();
        assert!(err.contains("空结果"), "{err}");
    }

    #[test]
    fn deepl_payload_and_base_url() {
        // 免费版：key 以 `:fx` 结尾（用 strings.HasSuffix，区分大小写）
        assert!(deepl_base("abc:fx").ends_with("api-free.deepl.com/v2"));
        assert!(deepl_base("abc").ends_with("api.deepl.com/v2"));
        assert!(!deepl_base("abc").contains("api-free"));
        // 大写 FX 不算免费版（与 HasSuffix 口径一致）
        assert!(deepl_base("abc:FX").ends_with("api.deepl.com/v2"));

        let payload = deepl_payload("你好", "zh-cn", "en-us");
        assert_eq!(payload["text"][0], json!("你好"));
        // 与既有行为一样统一转大写
        assert_eq!(payload["target_lang"], json!("EN-US"));
        assert_eq!(payload["source_lang"], json!("ZH-CN"));

        let text = parse_deepl_result(
            r#"{"translations":[{"detected_source_language":"ZH","text":"Hello"},{"detected_source_language":"ZH","text":"World"}]}"#,
        )
        .unwrap();
        assert_eq!(text, "Hello\nWorld");
        let err = parse_deepl_result(r#"{"translations":[]}"#).unwrap_err();
        assert!(err.contains("空结果"), "{err}");
        let err = parse_deepl_result("oops").unwrap_err();
        assert!(err.contains("返回异常"), "{err}");
        // 状态码提示必须是可执行的中文，不能只回 HTTP 码
        assert!(deepl_error_hint(456).contains("额度"));
        assert!(deepl_error_hint(403).contains("Auth Key"));
        assert!(deepl_error_hint(429).contains("频繁"));
    }

    #[test]
    fn engine_config_reads_and_validates_credentials() {
        let config = EngineConfig::from_value(&json!({
            "engine": " baidu ",
            "baidu_app_id": " id ",
            "baidu_app_secret": "",
            "youdao_app_key": "k",
        }));
        assert_eq!(config.engine, "baidu");
        assert_eq!(config.baidu_app_id, "id");
        assert_eq!(config.engine(), Some(Engine::Baidu));
        // 缺密钥 → 中文提示
        let missing = config.missing().unwrap();
        assert!(missing.contains("百度"), "{missing}");

        // 未选引擎时不报缺凭据（交给调用方走云端）
        let empty = EngineConfig::from_value(&json!({}));
        assert_eq!(empty.engine(), None);
        assert!(empty.missing().is_none());

        // 凭据齐全
        let ready = EngineConfig::from_value(&json!({
            "engine": "deepl",
            "deepl_auth_key": "x:fx",
        }));
        assert!(ready.missing().is_none());
        assert_eq!(Engine::from_key("deepl"), Some(Engine::Deepl));
        assert_eq!(Engine::from_key("ai"), Some(Engine::Ai));
        // 引擎标识含 `google`（选中后走「暂不支持」分支），
        // 所以这里必须能识别出来，不能返回 None。
        assert_eq!(Engine::from_key("google"), Some(Engine::Google));
        // 非法 / 空标识一律 None（交给调用方走云端官方接口，不能瞎猜引擎）
        assert_eq!(Engine::from_key("baidu-china"), None);
        assert_eq!(Engine::from_key("GOOGLE"), None);
        assert_eq!(Engine::from_key(""), None);
        assert_eq!(Engine::from_key("   "), None);
        // 首尾空白照 `from_value` 的做法裁掉
        assert_eq!(Engine::from_key(" google "), Some(Engine::Google));
    }

    #[test]
    fn local_replace_skips_numbers_and_punctuation() {
        // 单个中文标点 → 英文标点，且不需要翻译
        for (input, expect) in [
            ("，", ","),
            ("。", "."),
            ("；", ";"),
            ("：", ":"),
            ("？", "?"),
            ("！", "!"),
            ("（", "("),
            ("）", ")"),
            ("【", "["),
            ("】", "]"),
            ("《", "<"),
            ("》", ">"),
            ("“", "\""),
            ("”", "\""),
            ("、", "/"),
            ("～", "~"),
        ] {
            let (out, need) = local_replace(input);
            assert_eq!(out, expect, "{input}");
            assert!(!need, "{input} 不该走接口");
        }
        // 单字节 ASCII 字符：`len(s) < 2` 直接返回，不翻译
        let (out, need) = local_replace("A");
        assert_eq!(out, "A");
        assert!(!need);
        // 两字节以上的普通文本要走接口
        let (_, need) = local_replace("AB");
        assert!(need);
        // 纯数字/点（价格、日期）
        assert!(!local_replace("199.00").1);
        assert!(!local_replace("2026-11-08").1);
        assert!(!local_replace("(10)").1);
        // 备案号
        assert!(!local_replace("京ICP备12345678号-1").1);
        assert!(!local_replace("粤ICP备12345678号").1);
        // 正常文本
        let (out, need) = local_replace("你好 world");
        assert_eq!(out, "你好 world");
        assert!(need);
    }

    #[test]
    fn splits_html_and_rebuilds_text_only() {
        let html = "<p>你好 <b>世界</b></p><script>var a = 'hello';</script><img alt=\"标题\">";
        let segments = split_html(html);

        // 文本片段只包含标签之间的内容，script 内部整体当原样片段
        let texts: Vec<String> = segments
            .iter()
            .filter_map(|segment| match segment {
                HtmlSegment::Text(text) => Some(text.clone()),
                HtmlSegment::Raw(_) => None,
            })
            .collect();
        assert_eq!(texts, vec!["你好 ", "世界"]);
        assert!(segments.iter().any(|segment| match segment {
            HtmlSegment::Raw(raw) => raw.contains("var a = 'hello';"),
            _ => false,
        }));

        // 回填：文本按顺序替换，标签与脚本原样保留
        let out = rebuild_html(&segments, &["Hello ".to_string(), "World".to_string()]);
        assert_eq!(
            out,
            "<p>Hello <b>World</b></p><script>var a = 'hello';</script><img alt=\"标题\">"
        );

        // 译文不足时保留原文
        let out = rebuild_html(&segments, &[]);
        assert_eq!(out, html);
    }

    #[test]
    fn html_segments_keep_entities_and_attributes() {
        let html = "<div class=\"a\">  </div><code>fn main() {}</code><style>.a{color:red}</style>";
        let segments = split_html(html);
        // 空白文本片段也在，但不会被翻译（translate_texts 对空串原样保留）
        let texts: Vec<String> = segments
            .iter()
            .filter_map(|segment| match segment {
                HtmlSegment::Text(text) => Some(text.clone()),
                HtmlSegment::Raw(_) => None,
            })
            .collect();
        assert_eq!(texts, vec!["  "]);
        assert_eq!(rebuild_html(&segments, &["  ".to_string()]), html);
    }
}

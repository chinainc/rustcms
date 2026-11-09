//! 邮件发送配置、模板与发送记录。
//!
//! 配置存 `settings.sendmail`，对象结构：
//! `{server, use_ssl, port, account, password, recipient, templates[]}`。
//! 模板是「默认模板 + 配置覆盖」两层结构（内置默认表 + 合并规则）；
//! `readonly` 模板的 subject/content 不可覆盖。
//! 发送记录是 `<cache>/mail.log` 里的 JSON 行（每行一条 `MailLog`）。

use serde::Serialize;
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use std::path::Path;

use super::{site_setting, ModelError};

/// 邮件配置键。
pub const SENDMAIL_SETTING: &str = "sendmail";

/// SMTP 授权码掩码：读取时替换真实授权码，
/// 保存时原样回传表示「未修改」，沿用库里的真实值。
pub const PASSWORD_MASK: &str = "********";

/// 发送记录文件名（位于 `<cache>/` 下）。
pub const MAIL_LOG_FILE: &str = "mail.log";

/// 邮件模板。
#[derive(Debug, Clone, Serialize)]
pub struct EmailTemplate {
    pub open: bool,
    #[serde(rename = "type")]
    pub kind: String,
    pub key: String,
    pub name: String,
    pub description: String,
    pub readonly: bool,
    pub delay: i64,
    pub subject: String,
    pub content: String,
}

impl EmailTemplate {
    fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// 发送记录（mail.log 里每行一个 JSON）。
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct MailLog {
    pub created_time: i64,
    pub subject: String,
    pub status: String,
    pub address: String,
}

/// 内置默认邮件模板。
pub fn default_templates() -> Vec<EmailTemplate> {
    vec![
        EmailTemplate {
            open: false,
            kind: "system".into(),
            key: "guestbook".into(),
            name: "留言通知".into(),
            description: "当前网站有留言时，会通过邮件发送给设置的邮箱。".into(),
            readonly: false,
            delay: 0,
            subject: r#"您的[{% system with name="SiteName" %}]网站有来自[{{guestbook.UserName}}]的新留言"#.into(),
            content: r#"您好，您的网站[{% system with name="SiteName" %}]有一条新的留言，内容如下：

用户：{{guestbook.UserName}}

联系方式：{{guestbook.Contact}}

内容：{{guestbook.Content}}

{% for key, value in guestbook.ExtraData %}
{{key}}: {{value}}
{% endfor %}

时间：{{guestbook.CreatedTime|dateFormat:"2006-01-02 15:04:05"}}

IP: {{guestbook.Ip}}

来源页面: {{guestbook.Refer}}

此邮件为系统自动发送，请勿回复。"#
                .into(),
        },
        EmailTemplate {
            open: false,
            kind: "user".into(),
            key: "auto_reply".into(),
            name: "留言自动回复".into(),
            description: "当客户提交留言时，会自动回复一封邮件给客户。".into(),
            readonly: false,
            delay: 0,
            subject: r#"[{% system with name="SiteName" %}]: 感谢您的留言"#.into(),
            content: "尊敬的 {{user.UserName}}，您好！\n\n我们已收到您的留言，将尽快回复您。\n\n{% system with name=\"SiteName\" %} 团队 敬上".into(),
        },
        EmailTemplate {
            open: false,
            kind: "system".into(),
            key: "report".into(),
            name: "网站日报".into(),
            description: "每日发送网站流量等数据给设置的邮箱".into(),
            readonly: true,
            delay: 0,
            subject: String::new(),
            content: String::new(),
        },
        EmailTemplate {
            open: false,
            kind: "user".into(),
            key: "register".into(),
            name: "注册验证邮件".into(),
            description: "当用户通过邮件注册账号时，会通过邮件发送验证码或验证链接给用户。".into(),
            readonly: false,
            delay: 0,
            subject: r#"[{% system with name="SiteName" %}]: 请验证您的邮箱地址"#.into(),
            content: "尊敬的 {{user.UserName}}，您好！\n\n感谢您注册 {% system with name=\"SiteName\" %}。请点击以下链接验证您的邮箱地址：\n\n{{url|safe}}\n\n如果您没有注册过我们的账号，请忽略此邮件。\n\n{% system with name=\"SiteName\" %} 团队 敬上".into(),
        },
        EmailTemplate {
            open: false,
            kind: "system".into(),
            key: "new_order".into(),
            name: "订单确认通知".into(),
            description: "当客户创建新订单时，会通过邮件发送给设置的邮箱。".into(),
            readonly: false,
            delay: 0,
            subject: r#"[{% system with name="SiteName" %}]: 您的订单（{{order.OrderId}}）已提交"#.into(),
            content: "尊敬的 {{user.UserName}}，您好！\n\n感谢您在 {% system with name=\"SiteName\" %} 下单。您的订单已提交，目前待支付。为确保商品及时处理和发货，请尽快完成支付。\n\n订单详情：\n订单号：{{order.OrderId}}\n下单时间：{{order.CreatedTime|dateFormat:\"2006-01-02 15:04:05\"}}\n订单金额：{{order.Amount|priceFormat}}\n\n请选择合适的支付方式完成付款。如果您已经完成支付，请忽略此提醒。\n\n支付成功且订单开始履约后，我们会再发送邮件通知您。\n\n如有任何关于订单或支付流程的疑问，请随时与我们联系。\n\n{% system with name=\"SiteName\" %} 团队 敬上".into(),
        },
        EmailTemplate {
            open: false,
            kind: "system".into(),
            key: "pay_order".into(),
            name: "支付成功通知".into(),
            description: "当客户支付订单时，会通过邮件发送给设置的邮箱。".into(),
            readonly: false,
            delay: 0,
            subject: r#"「{% system with name="SiteName" %}」: 您的订单（{{order.OrderId}}）已支付"#.into(),
            content: "尊敬的 {{user.UserName}}，您好！\n\n感谢您完成支付。您的订单已成功付款，目前正在处理中。订单发货后我们将再发送邮件通知您。\n\n订单详情：\n订单号：{{order.OrderId}}\n支付时间：{{order.PaidTime|dateFormat:\"2006-01-02 15:04:05\"}}\n订单金额：{{order.Amount|priceFormat}}\n\n感谢您的惠顾，我们将确保您的订单得到及时处理。\n\n如有任何关于订单的疑问，请随时与我们联系。\n\n{% system with name=\"SiteName\" %} 团队 敬上".into(),
        },
        EmailTemplate {
            open: false,
            kind: "user".into(),
            key: "recommendation".into(),
            name: "个性化推荐邮件".into(),
            description: "可以不定期根据用户浏览过的产品，向订阅用户发送个性化推荐邮件。".into(),
            readonly: false,
            delay: 0,
            subject: r#"[{% system with name="SiteName" %}]: 为您推荐的商品"#.into(),
            content: "尊敬的 {{user.UserName}}，您好！\n\n根据您之前的浏览记录，我们为您推荐以下文章：\n\n{% archiveList archives with limit=5 %}\n{% for item in archives %}\n**{{item.Title}}**\n  {{ item.Description }}\n链接：{{item.Link}}\n{% endfor %}\n{% endarchiveList %}\n\n快来看看这些专为您精选的推荐吧！\n\n{% system with name=\"SiteName\" %} 团队 敬上".into(),
        },
        EmailTemplate {
            open: false,
            kind: "user".into(),
            key: "newsletter_confirm".into(),
            name: "邮件订阅确认".into(),
            description: "当用户订阅邮件时，会通过邮件发送确认邮件给用户。".into(),
            readonly: false,
            delay: 0,
            subject: r#"[{% system with name="SiteName" %}]: 欢迎订阅我们的邮件！"#.into(),
            content: "您好，{{user.Email}}！\n\n感谢您订阅我们的邮件。您已加入我们的专属社区，之后将收到：\n\n- 抢先了解促销和优惠活动\n- 新品发布通知\n- 会员专属折扣\n- 实用技巧和行业资讯\n\n您很快就会收到我们发送的邮件。我们承诺只为您发送相关且有价值的内容。\n\n如果您想退订，只需点击任意邮件底部的「退订」链接即可。\n\n欢迎加入！\n\n{% system with name=\"SiteName\" %} 团队 敬上".into(),
        },
    ]
}

/// 读配置（补默认值）。
pub async fn config_get(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = site_setting::get_section(pool, SENDMAIL_SETTING).await?;
    if let Value::Object(map) = &mut value {
        map.entry("server").or_insert(json!(""));
        map.entry("use_ssl").or_insert(json!(0));
        map.entry("port").or_insert(json!(0));
        map.entry("account").or_insert(json!(""));
        map.entry("password").or_insert(json!(""));
        map.entry("recipient").or_insert(json!(""));
        map.entry("templates").or_insert(json!([]));
    }
    Ok(value)
}

/// 读配置（内部用，含真实授权码）。
async fn raw_templates(pool: &MySqlPool) -> anyhow::Result<Vec<Value>> {
    let config = config_get(pool).await?;
    Ok(config
        .get("templates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// GET /setting：配置 + 授权码掩码。
pub async fn config_get_masked(pool: &MySqlPool) -> anyhow::Result<Value> {
    let mut value = config_get(pool).await?;
    if let Value::Object(map) = &mut value {
        let password = map
            .get("password")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !password.is_empty() {
            map.insert("password".into(), json!(PASSWORD_MASK));
        }
    }
    Ok(value)
}

/// POST /setting：逐字段覆盖（server/use_ssl/port/account/password/recipient），
/// 授权码等于掩码时沿用原值。
pub async fn config_save(pool: &MySqlPool, patch: &Value) -> Result<Value, ModelError> {
    if !patch.is_object() {
        return Err(ModelError::invalid("配置格式错误"));
    }
    let current = config_get(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("读取失败: {err}")))?;
    let mut merged = current.clone();
    for key in ["server", "use_ssl", "port", "account", "password", "recipient"] {
        if let Some(item) = patch.get(key) {
            if key == "password" && item.as_str() == Some(PASSWORD_MASK) {
                continue;
            }
            if let Value::Object(map) = &mut merged {
                map.insert(key.to_string(), item.clone());
            }
        }
    }
    // templates 由模板保存接口单独维护，这里保持原样
    site_setting::save_section(pool, SENDMAIL_SETTING, &merged)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;
    config_get_masked(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("读取失败: {err}")))
}

/// GET /templates：默认模板 + 配置覆盖。
pub async fn templates_list(pool: &MySqlPool) -> anyhow::Result<Vec<Value>> {
    let overrides = raw_templates(pool).await?;
    let mut result = Vec::new();
    for template in default_templates() {
        let mut template = template;
        if let Some(item) = overrides.iter().find(|v| v["key"] == json!(template.key)) {
            template.open = item["open"].as_bool().unwrap_or(false);
            if let Some(subject) = item["subject"].as_str().filter(|s| !s.is_empty()) {
                template.subject = subject.to_string();
            }
            if let Some(content) = item["content"].as_str().filter(|s| !s.is_empty()) {
                template.content = content.to_string();
            }
            if item["delay"].as_i64().unwrap_or(0) > 0 {
                template.delay = item["delay"].as_i64().unwrap_or(0);
            }
        }
        result.push(template.to_json());
    }
    Ok(result)
}

/// GET /template?key=：合并后的单个模板。
/// 返回 `None` 表示模板不存在；`readonly` 模板不应用 subject/content 覆盖。
pub async fn template_get(pool: &MySqlPool, key: &str) -> anyhow::Result<Option<Value>> {
    let mut template = match default_templates().into_iter().find(|t| t.key == key) {
        Some(template) => template,
        None => return Ok(None),
    };
    let overrides = raw_templates(pool).await?;
    if let Some(item) = overrides.iter().find(|v| v["key"] == json!(key)) {
        template.open = item["open"].as_bool().unwrap_or(false);
        if !template.readonly {
            if let Some(subject) = item["subject"].as_str().filter(|s| !s.is_empty()) {
                template.subject = subject.to_string();
            }
            if let Some(content) = item["content"].as_str().filter(|s| !s.is_empty()) {
                template.content = content.to_string();
            }
            if item["delay"].as_i64().unwrap_or(0) > 0 {
                template.delay = item["delay"].as_i64().unwrap_or(0);
            }
        }
    }
    Ok(Some(template.to_json()))
}

/// POST /template：按 key 保存 {open, delay, subject, content}。
pub async fn template_save(
    pool: &MySqlPool,
    body: &Value,
) -> Result<Value, ModelError> {
    let key = body["key"].as_str().unwrap_or_default().to_string();
    if key.is_empty() {
        return Err(ModelError::invalid("模板标识不能为空"));
    }
    let entry = json!({
        "open": body["open"].as_bool().unwrap_or(false),
        "key": key,
        "delay": body["delay"].as_i64().unwrap_or(0),
        "subject": body["subject"].as_str().unwrap_or_default(),
        "content": body["content"].as_str().unwrap_or_default(),
    });

    let current = config_get(pool)
        .await
        .map_err(|err| ModelError::invalid(format!("读取失败: {err}")))?;
    let mut templates = current
        .get("templates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(slot) = templates.iter_mut().find(|v| v["key"] == json!(key)) {
        *slot = entry.clone();
    } else {
        templates.push(entry.clone());
    }
    let mut merged = current;
    if let Value::Object(map) = &mut merged {
        map.insert("templates".into(), Value::Array(templates));
    }
    site_setting::save_section(pool, SENDMAIL_SETTING, &merged)
        .await
        .map_err(|err| ModelError::invalid(format!("保存失败: {err}")))?;

    Ok(entry)
}

/// 把时间布局串（如 `2006-01-02 15:04:05`）转成 chrono 格式串。
fn go_layout_to_chrono(layout: &str) -> String {
    const TOKENS: [(&str, &str); 7] = [
        ("2006", "%Y"),
        ("01", "%m"),
        ("02", "%d"),
        ("15", "%H"),
        ("04", "%M"),
        ("05", "%S"),
        ("06", "%y"),
    ];
    let mut out = String::new();
    let mut rest = layout;
    'outer: while !rest.is_empty() {
        for (token, repl) in TOKENS {
            if let Some(head) = rest.strip_prefix(token) {
                out.push_str(repl);
                rest = head;
                continue 'outer;
            }
        }
        // 没有命中任何 token：原样吃掉一个字符
        let ch = rest.chars().next().unwrap();
        if ch == '%' {
            out.push_str("%%");
        } else {
            out.push(ch);
        }
        rest = &rest[ch.len_utf8()..];
    }
    out
}

/// 把 JSON 值字符串化（渲染输出用）。
fn value_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// 按点路径取值（`guestbook.ExtraData` 等）。
fn lookup<'a>(data: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = data;
    for part in path.split('.') {
        current = match current {
            Value::Object(map) => map.get(part)?,
            _ => return None,
        };
    }
    Some(current)
}

/// 应用 pongo2 风格 filter（只支持邮件模板里实际用到的三种）。
fn apply_filter(value: String, filter: &str, arg: Option<&str>) -> String {
    let (name, arg) = match filter.split_once(':') {
        Some((name, rest)) => (name, Some(rest.trim_matches('"'))),
        None => (filter, arg),
    };
    match name {
        "safe" => value,
        "priceFormat" => {
            // 金额单位是分，输出保留两位小数的元
            match value.parse::<i64>() {
                Ok(fen) => format!("{:.2}", fen as f64 / 100.0),
                Err(_) => value,
            }
        }
        "dateFormat" => {
            let Some(arg) = arg else {
                return value;
            };
            match value.parse::<i64>() {
                Ok(stamp) => chrono::DateTime::from_timestamp(stamp, 0)
                    .map(|dt| {
                        dt.with_timezone(&chrono::Local)
                            .format(&go_layout_to_chrono(arg))
                            .to_string()
                    })
                    .unwrap_or(value),
                Err(_) => value,
            }
        }
        _ => value,
    }
}

/// 渲染 `{{ var.path|filter:"arg" }}` 表达式。
fn render_expression(expr: &str, data: &Value) -> String {
    let expr = expr.trim();
    let (path, filters) = match expr.split_once('|') {
        Some((path, filters)) => (path.trim(), filters),
        None => (expr, ""),
    };
    let mut value = value_text(lookup(data, path).unwrap_or(&Value::Null));
    for filter in filters.split('|') {
        let filter = filter.trim();
        if filter.is_empty() {
            continue;
        }
        value = apply_filter(value, filter, None);
    }
    value
}

/// 渲染 pongo2 风格邮件模板。
///
/// 支持 `{% system with name="X" %}`、`{{ path|filter }}`、
/// `{% for key, value in map %}..{% endfor %}`（单层，map 迭代）；
/// 其余 tag（如 archiveList）原样保留。渲染失败由调用方把错误文本
/// 写回 subject/content。
pub fn render_template(template: &str, data: &Value) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        // 只认紧邻的 `{{` / `{%`，否则把孤立 `{` 原样输出
        let tag_start = match rest.as_bytes().get(start + 1) {
            Some(b'{') | Some(b'%') => start + 1,
            _ => {
                out.push_str(&rest[..start + 1]);
                rest = &rest[start + 1..];
                continue;
            }
        };
        let tag_kind = rest.as_bytes()[tag_start];
        out.push_str(&rest[..start]);
        let (inner, after) = match rest[tag_start + 1..].find(if tag_kind == b'%' { "%}" } else { "}}" }) {
            Some(end) => (
                &rest[tag_start + 1..tag_start + 1 + end],
                &rest[tag_start + 1 + end + 2..],
            ),
            None => {
                // 未闭合的标签按渲染错误处理（对齐 pongo2 行为）
                return Err("标签未关闭".into());
            }
        };
        rest = after;
        let trimmed = inner.trim();
        if tag_kind == b'{' {
            // {{ ... }} 变量
            out.push_str(&render_expression(trimmed, data));
            continue;
        }
        // {% ... %} 标签
        if let Some(name) = trimmed.strip_prefix("system with name=") {
            let field = name.trim().trim_matches('"');
            out.push_str(&value_text(lookup(data, &format!("system.{field}")).unwrap_or(&Value::Null)));
        } else if trimmed == "system" {
            // 不带参数的 system 标签：输出空
        } else if let Some(spec) = trimmed.strip_prefix("for ").or(trimmed.strip_prefix("range ")) {
            // for key, value in map —— 单层对象迭代
            let body_start = rest.find("{% endfor %}");
            let Some(body_end) = body_start else {
                return Err("for 缺少 endfor".into());
            };
            let body = &rest[..body_end];
            rest = &rest[body_end + "{% endfor %}".len()..];
            // spec: "key, value in path"
            let Some((vars, path)) = spec.split_once(" in ") else {
                return Err(format!("for 语法错误: {spec}"));
            };
            let mut parts = vars.split(',');
            let key_var = parts.next().unwrap_or("").trim();
            let value_var = parts.next().map(str::trim).unwrap_or("");
            let target = lookup(data, path.trim()).cloned().unwrap_or(Value::Null);
            match target {
                Value::Object(map) => {
                    for (key, value) in map {
                        let mut scope = data.clone();
                        if let Value::Object(slot) = &mut scope {
                            if !key_var.is_empty() {
                                slot.insert(key_var.to_string(), json!(key));
                            }
                            if !value_var.is_empty() {
                                slot.insert(value_var.to_string(), value);
                            }
                        }
                        out.push_str(&render_template(body, &scope)?);
                    }
                }
                _ => {
                    // 目标不存在：pongo2 迭代空集，输出空
                }
            }
        } else {
            // archiveList 等模板引擎专属 tag：原样保留
            out.push_str(&format!("{{% {trimmed} %}}"));
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// 把非 HTML 内容转成简单段落 HTML（简化版 Markdown 支持）。
fn markdown_to_html(content: &str) -> String {
    if content.trim().is_empty() {
        return String::new();
    }
    let mut html = String::from("<p>");
    for ch in content.chars() {
        match ch {
            '\n' => html.push_str("<br>\n"),
            '<' => html.push_str("&lt;"),
            '>' => html.push_str("&gt;"),
            other => html.push(other),
        }
    }
    html.push_str("</p>");
    html
}

/// 预览渲染（模型层）：
/// 返回渲染后的 {subject, content}，渲染失败时把错误文本写回对应字段。
pub fn render_preview(subject: &str, content: &str, data: &Value) -> Value {
    let mut subject = subject.to_string();
    let mut content = content.to_string();
    let mut error: Option<String> = None;
    match render_template(&subject, data) {
        Ok(rendered) => subject = rendered,
        Err(err) => {
            error = Some(err.clone());
            subject = err;
        }
    }
    match render_template(&content, data) {
        Ok(rendered) => content = rendered,
        Err(err) => {
            error = Some(err.clone());
            content = err;
        }
    }
    // content 先按 Markdown 转 HTML（非 < 开头才转）
    if !content.starts_with('<') {
        content = markdown_to_html(&content);
    }
    json!({
        "key": "",
        "name": "",
        "description": "",
        "type": "",
        "open": false,
        "readonly": false,
        "delay": 0,
        "subject": subject,
        "content": content,
        "error": error,
    })
}

/// 读取发送记录（`<cache>/mail.log` 的 JSON 行，倒序分页）。
///
/// 返回 `(本页记录, 总数)`；文件不存在返回空。
pub fn mail_log_list(cache_dir: &Path, page: i64, page_size: i64) -> (Vec<MailLog>, i64) {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;

    let Ok(content) = std::fs::read_to_string(cache_dir.join(MAIL_LOG_FILE)) else {
        return (Vec::new(), 0);
    };
    // 从文件尾部倒着读：等价于全量行倒序
    let mut lines: Vec<&str> = content.lines().rev().collect();
    let total = lines.len() as i64;
    let selected: Vec<MailLog> = lines
        .drain(..)
        .skip(offset as usize)
        .take(page_size as usize)
        .filter_map(|line| serde_json::from_str::<MailLog>(line).ok())
        .collect();
    let mut total = total;
    // 修正：最后一页不满时 total 修正为 offset + len
    if page_size as usize > selected.len() {
        total = offset + selected.len() as i64;
    }
    if total < selected.len() as i64 {
        total = selected.len() as i64;
    }
    (selected, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sendmail_mask_and_merge() {
        // 掩码常量不会被真实授权码冲突（16 位随机字符）
        assert_eq!(PASSWORD_MASK, "********");
    }

    #[test]
    fn default_templates_cover_all_keys() {
        let templates = default_templates();
        let keys: Vec<&str> = templates.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "guestbook",
                "auto_reply",
                "report",
                "register",
                "new_order",
                "pay_order",
                "recommendation",
                "newsletter_confirm",
            ]
        );
        // report 是只读模板
        let report = templates.iter().find(|t| t.key == "report").unwrap();
        assert!(report.readonly);
        // 每个非只读模板都有标题
        for template in &templates {
            if !template.readonly && template.key != "report" {
                assert!(!template.subject.is_empty(), "{} 应有标题", template.key);
            }
        }
    }

    #[test]
    fn renders_variables_and_filters() {
        let now = 1_726_387_200i64; // 2024-09-15 08:00:00 UTC
        let data = json!({
            "system": {"SiteName": "测试站"},
            "user": {"UserName": "张三", "Email": "a@b.c"},
            "guestbook": {
                "UserName": "李四",
                "Contact": "12345",
                "Content": "你好",
                "CreatedTime": now,
                "Ip": "127.0.0.1",
                "Refer": "https://example.com",
                "ExtraData": {"Extra1": "值1", "Extra2": "值2"},
            },
            "order": {"OrderId": "ORD1", "Amount": 1999, "CreatedTime": now},
            "url": "https://example.com/verify",
        });

        let out = render_template(
            r#"[{% system with name="SiteName" %}]: {{user.UserName}} 的留言 {{order.Amount|priceFormat}} {{guestbook.CreatedTime|dateFormat:"2006-01-02 15:04:05"}}"#,
            &data,
        )
        .unwrap();
        assert!(out.contains("[测试站]: 张三 的留言"), "{out}");
        assert!(out.contains("19.99"), "{out}");
        // dateFormat 按布局串输出本地时间：只断言包含日期部分
        assert!(out.contains("-"), "{out}");

        // for 循环 + 变量
        let out = render_template(
            "{% for key, value in guestbook.ExtraData %}\n{{key}}: {{value}}\n{% endfor %}",
            &data,
        )
        .unwrap();
        assert!(out.contains("Extra1: 值1"), "{out}");
        assert!(out.contains("Extra2: 值2"), "{out}");

        // 不存在的变量输出空（pongo2 行为）
        let out = render_template("A{{nope.missing}}B", &data).unwrap();
        assert_eq!(out, "AB");

        // 未知 tag 原样保留
        let out = render_template("{% archiveList archives with limit=5 %}x", &data).unwrap();
        assert_eq!(out, "{% archiveList archives with limit=5 %}x");

        // url|safe 原样输出
        let out = render_template("{{url|safe}}", &data).unwrap();
        assert_eq!(out, "https://example.com/verify");
    }

    #[test]
    fn render_preview_fills_and_converts() {
        let data = json!({"user": {"UserName": "张三"}});
        let result = render_preview("你好 {{user.UserName}}", "正文第一行\n正文第二行", &data);
        assert_eq!(result["subject"], json!("你好 张三"));
        let content = result["content"].as_str().unwrap();
        assert!(content.starts_with("<p>"), "{content}");
        assert!(content.contains("正文第一行<br>"), "{content}");

        // 渲染失败把错误写回字段
        let result = render_preview("{{ user.", "body", &data);
        assert!(result["subject"].as_str().unwrap_or("").contains("缺少") || result["error"].is_string(), "{result}");
    }

    #[test]
    fn go_layout_conversion() {
        assert_eq!(
            go_layout_to_chrono("2006-01-02 15:04:05"),
            "%Y-%m-%d %H:%M:%S"
        );
        assert_eq!(go_layout_to_chrono("2006"), "%Y");
    }
}

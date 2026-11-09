//! 小程序消息推送（`NotifyWeappMsg`）的报文解析。
//!
//! 小程序回调有明文与安全（AES）两种模式：安全模式下 POST body 是一层
//! `{"Encrypt": ...}` 信封 XML，真正的报文在解密之后；解出来的报文与公众号
//! 一样是扁平 XML（`MsgType` / `Event` / `FromUserName` …）。
//!
//! 这里只做**纯解析**（不碰数据库、不发请求），动作由调用方执行。

use std::collections::HashMap;

/// 订阅消息事件（`subscribe_msg_popup_event` / `subscribe_msg_change_event`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscribeEvent {
    /// 订阅消息模板 ID。
    pub template_id: String,
    /// `accept` 表示订阅，其余（`reject` / `ban` / `filter`）视为拒绝。
    pub status: String,
}

impl SubscribeEvent {
    /// 是否是「同意订阅」。
    pub fn accepted(&self) -> bool {
        self.status.eq_ignore_ascii_case("accept")
    }
}

/// 取信封里的 `<Encrypt>` 密文（不是安全模式时返回 `None`）。
pub fn envelope_encrypt(xml: &str) -> Option<String> {
    xml_fields(xml)
        .get("Encrypt")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 解析扁平 XML（微信回调都是单层标签 + CDATA）。
///
/// 嵌套容器（值以 `<` 开头的非 CDATA 内容）不记录自身，交给调用方按需处理，
/// 例如订阅消息的 `<SubscribeMsgPopupEvent><List>…</List></SubscribeMsgPopupEvent>`。
pub fn xml_fields(xml: &str) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
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
        if name.starts_with('/')
            || name.starts_with('?')
            || name.starts_with('!')
            || name.is_empty()
        {
            index = cursor + 1;
            continue;
        }
        // 自闭合标签没有内容
        if cursor > name_start && bytes[cursor - 1] == b'/' {
            index = cursor + 1;
            continue;
        }
        let close = format!("</{name}>");
        let value_start = cursor + 1;
        let Some(end) = xml[value_start..].find(&close) else {
            break;
        };
        let raw_value = &xml[value_start..value_start + end];
        let value = if let Some(inner) = raw_value
            .strip_prefix("<![CDATA[")
            .and_then(|rest| rest.strip_suffix("]]>"))
        {
            inner
        } else if raw_value.starts_with('<') {
            // 嵌套容器：进去解析内部，不记录自身
            index = value_start;
            continue;
        } else {
            raw_value
        };
        out.insert(name.to_string(), xml_decode(value));
        index = value_start + end + close.len();
    }
    out
}

/// XML 实体解码。
fn xml_decode(raw: &str) -> String {
    raw.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// 取某个标签的全部内层内容（非嵌套，按出现顺序）。
fn inner_blocks<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(found) = xml[cursor..].find(&open) {
        let start = cursor + found + open.len();
        let Some(end) = xml[start..].find(&close) else {
            break;
        };
        out.push(&xml[start..start + end]);
        cursor = start + end + close.len();
    }
    out
}

/// 从解密后的报文里提取订阅消息事件列表。
///
/// 微信的报文形如：
///
/// ```xml
/// <xml>
/// <Event><![CDATA[subscribe_msg_popup_event]]></Event>
/// <SubscribeMsgPopupEvent>
/// <List><TemplateId>..</TemplateId><SubscribeStatusString>accept</SubscribeStatusString></List>
/// <List>…</List>
/// </SubscribeMsgPopupEvent>
/// </xml>
/// ```
pub fn subscribe_events(xml: &str) -> Vec<SubscribeEvent> {
    let mut out = Vec::new();
    for block in inner_blocks(xml, "List") {
        let fields = xml_fields(&format!("<List>{block}</List>"));
        let template_id = fields
            .get("TemplateId")
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        if template_id.is_empty() {
            continue;
        }
        let status = fields
            .get("SubscribeStatusString")
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        out.push(SubscribeEvent {
            template_id,
            status,
        });
    }
    if !out.is_empty() {
        return out;
    }
    // 兜底：没有 <List> 包装时按单条解析（部分事件直接给一个条目）
    let fields = xml_fields(xml);
    if let Some(template_id) = fields.get("TemplateId").map(|value| value.trim()) {
        if !template_id.is_empty() {
            out.push(SubscribeEvent {
                template_id: template_id.to_string(),
                status: fields
                    .get("SubscribeStatusString")
                    .map(|value| value.trim().to_string())
                    .unwrap_or_default(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flat_fields_with_cdata() {
        let xml = "<xml><ToUserName><![CDATA[gh_1]]></ToUserName>\
                   <FromUserName><![CDATA[o_user]]></FromUserName>\
                   <CreateTime>1700000000</CreateTime>\
                   <MsgType><![CDATA[event]]></MsgType>\
                   <Event><![CDATA[subscribe_msg_popup_event]]></Event>\
                   <Empty/>\
                   <Content><![CDATA[a < b &amp; c]]></Content></xml>";
        let fields = xml_fields(xml);
        assert_eq!(fields.get("ToUserName").map(String::as_str), Some("gh_1"));
        assert_eq!(
            fields.get("Event").map(String::as_str),
            Some("subscribe_msg_popup_event")
        );
        assert_eq!(
            fields.get("Content").map(String::as_str),
            Some("a < b & c")
        );
        assert!(!fields.contains_key("Empty"));
    }

    #[test]
    fn extracts_encrypt_envelope() {
        let xml = "<xml><ToUserName><![CDATA[gh_1]]></ToUserName>\
                   <Encrypt><![CDATA[abc==]]></Encrypt></xml>";
        assert_eq!(envelope_encrypt(xml).as_deref(), Some("abc=="));

        let plain = "<xml><Content><![CDATA[hi]]></Content></xml>";
        assert!(envelope_encrypt(plain).is_none());
    }

    #[test]
    fn collects_subscribe_events() {
        let xml = "<xml>\
<ToUserName><![CDATA[gh_1]]></ToUserName>\
<FromUserName><![CDATA[o_user]]></FromUserName>\
<MsgType><![CDATA[event]]></MsgType>\
<Event><![CDATA[subscribe_msg_popup_event]]></Event>\
<SubscribeMsgPopupEvent>\
  <List><TemplateId><![CDATA[tpl_a]]></TemplateId>\
  <SubscribeStatusString><![CDATA[accept]]></SubscribeStatusString>\
  <PopupScene>0</PopupScene></List>\
  <List><TemplateId><![CDATA[tpl_b]]></TemplateId>\
  <SubscribeStatusString><![CDATA[reject]]></SubscribeStatusString></List>\
</SubscribeMsgPopupEvent></xml>";
        let events = subscribe_events(xml);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].template_id, "tpl_a");
        assert!(events[0].accepted());
        assert_eq!(events[1].template_id, "tpl_b");
        assert!(!events[1].accepted());
    }

    #[test]
    fn collects_single_event_without_list() {
        let xml = "<xml><Event><![CDATA[subscribe_msg_change_event]]></Event>\
                   <TemplateId><![CDATA[tpl_c]]></TemplateId>\
                   <SubscribeStatusString><![CDATA[accept]]></SubscribeStatusString></xml>";
        let events = subscribe_events(xml);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].template_id, "tpl_c");
        assert!(events[0].accepted());
    }
}

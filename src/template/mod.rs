//! 模板引擎（类 Django 模板语法）。
//!
//! 覆盖类 Django 模板语法里最常用的部分：
//!
//! - `{{ expr }}`，支持 `a.b.c`、函数调用、`|escape` 之类的过滤器
//! - `{% if %}` / `{% else %}` / `{% elif %}` / `{% endif %}`
//! - `{% for x in xs %}` / `{% endfor %}`
//! - `{% extends 'base.html' %}` + `{% block name %}`（子模板覆盖父模板）
//! - `{% include "partial/x.html" %}`
//! - 自定义标签：`{% archiveList articles with ... %}…{% endarchiveList %}`（块标签）
//! 与 `{% categoryDetail with name="Title" %}`（行内标签）
//! - `{%-` / `-%}` 空白裁剪、`{# 注释 #}`
//!
//! 标签的数据由 [`Provider`] 提供，这样模板层不需要知道数据库。

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use anyhow::{anyhow, bail, Context as _, Result};
use serde_json::{Map, Value};

pub mod site_provider;

/// 模板上下文（变量表）。
pub type Scope = Map<String, Value>;

// ---------------------------- 多语言（语言包 / 语言切换） ---------------------

/// 运行中的模板目录（`state` 初始化时写入，供 `tr` 标签定位语言包）。
static TEMPLATE_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// 记录运行中的模板目录（应用启动时调用一次）。
pub fn set_template_dir(dir: &Path) {
    let _ = TEMPLATE_DIR.set(dir.to_path_buf());
}

/// 运行中的模板目录；未初始化时回退 `template/default`。
fn template_dir() -> PathBuf {
    TEMPLATE_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("template/default"))
}

/// 语言代码归一：`zh` / `zh_CN` / `zh-CN` → `zh-cn`，`en` / `en_US` → `en-us`。
pub fn normalize_language(lang: &str) -> String {
    let lowered = lang.trim().to_lowercase().replace('_', "-");
    match lowered.as_str() {
        "zh" | "zh-hans" => "zh-cn".to_string(),
        "en" | "en-gb" => "en-us".to_string(),
        other => other.to_string(),
    }
}

/// 语言显示名（语言切换器里的 `LanguageName`）。
pub fn language_display_name(lang: &str) -> String {
    match normalize_language(lang).as_str() {
        "zh-cn" => "简体中文".to_string(),
        "en-us" => "English".to_string(),
        other => other.to_string(),
    }
}

/// 语言国旗 emoji（语言切换器里的 `LanguageEmoji`）。
pub fn language_emoji(lang: &str) -> String {
    match normalize_language(lang).as_str() {
        "zh-cn" => "🇨🇳".to_string(),
        "en-us" => "🇬🇧".to_string(),
        _ => String::new(),
    }
}

/// 语言包缓存：（模板目录, 语言代码）→ 词条表。
static LANGUAGE_PACKS: std::sync::OnceLock<RwLock<HashMap<(PathBuf, String), Arc<HashMap<String, String>>>>> =
    std::sync::OnceLock::new();

/// 取当前站点语言对应的语言包（`locales/<lang>/default.yml`，进程内缓存）。
pub fn language_pack(lang: &str) -> Arc<HashMap<String, String>> {
    let dir = template_dir();
    let lang = normalize_language(lang);
    let caches = LANGUAGE_PACKS.get_or_init(|| RwLock::new(HashMap::new()));
    if let Some(hit) = caches
        .read()
        .unwrap_or_else(|err| err.into_inner())
        .get(&(dir.clone(), lang.clone()))
    {
        return hit.clone();
    }
    let path = dir.join("locales").join(&lang).join("default.yml");
    let pack = std::fs::read_to_string(&path)
        .map(|source| parse_language_pack(&source))
        .unwrap_or_default();
    let pack = Arc::new(pack);
    caches
        .write()
        .unwrap_or_else(|err| err.into_inner())
        .insert((dir, lang), pack.clone());
    pack
}

/// 解析语言包（`"词条": "译文"` 行式 YAML，不支持的行忽略）。
fn parse_language_pack(source: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = unquote_yaml(key.trim());
        let value = unquote_yaml(value.trim());
        if !key.is_empty() {
            map.insert(key, value);
        }
    }
    map
}

fn unquote_yaml(text: &str) -> String {
    let text = text.trim();
    if text.len() >= 2
        && ((text.starts_with('"') && text.ends_with('"'))
            || (text.starts_with('\'') && text.ends_with('\'')))
    {
        text[1..text.len() - 1].to_string()
    } else {
        text.to_string()
    }
}

/// 异步数据提供者：按标签名取数据，供模板标签使用。
pub trait Provider: Send + Sync {
    /// `tag` 是标签名（如 `archiveList`），`args` 是解析后的参数。
    ///
    /// 返回值约定：
    /// - 块标签（有 `end` 的标签）返回 `Value::Array` 或 `Value::Object` 绑定给变量；
    /// - 行内标签返回标量，直接输出（或绑定给 `{% tag var with ... %}` 里的 var）。
    fn data<'a>(
        &'a self,
        tag: &'a str,
        args: &'a Map<String, Value>,
    ) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + 'a>>;
}

/// 什么都不返回的 Provider（单测用）。
#[cfg(test)]
pub struct EmptyProvider;

#[cfg(test)]
impl Provider for EmptyProvider {
    fn data<'a>(
        &'a self,
        _tag: &'a str,
        _args: &'a Map<String, Value>,
    ) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + 'a>> {
        Box::pin(async { Ok(Value::Null) })
    }
}

// ---------------------------------------------------------------------------
// 词法
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Text(String),
    /// `{{ expr }}`
    Output(String),
    /// `{% content %}`
    Tag(String),
}

fn tokenize(source: &str) -> Result<Vec<Token>> {
    let bytes: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut text = String::new();
    let mut index = 0usize;
    let mut trim_next = false;

    while index < bytes.len() {
        let ch = bytes[index];
        if ch == '{' && index + 1 < bytes.len() {
            let next = bytes[index + 1];
            if next == '%' || next == '{' || next == '#' {
                // 取出到对应的结束标记：{# #} / {% %} / {{ }}
                let close: [char; 2] = match next {
                    '#' => ['#', '}'],
                    '{' => ['}', '}'],
                    _ => ['%', '}'],
                };
                let mut cursor = index + 2;
                // 空白裁剪：{%- / {{-
                let strip_before = bytes.get(cursor) == Some(&'-');
                if strip_before {
                    cursor += 1;
                }
                let body_start = cursor;
                let mut body_end = None;
                while cursor + 1 < bytes.len() {
                    if bytes[cursor] == close[0] && bytes[cursor + 1] == close[1] {
                        body_end = Some(cursor);
                        break;
                    }
                    cursor += 1;
                }
                let Some(body_end) = body_end else {
                    // 没有闭合，当普通文本
                    text.push(ch);
                    index += 1;
                    continue;
                };
                let mut body: String = bytes[body_start..body_end].iter().collect();
                // 空白裁剪：-%} / -}}
                let strip_after = body.trim_end().ends_with('-');
                if strip_after {
                    let trimmed = body.trim_end();
                    body = trimmed[..trimmed.len() - 1].to_string();
                }

                if strip_before {
                    while text.ends_with(|c: char| c.is_whitespace()) {
                        text.pop();
                    }
                }
                let _ = trim_next;

                if !text.is_empty() {
                    tokens.push(Token::Text(std::mem::take(&mut text)));
                }

                let body = body.trim().to_string();
                if next != '#' {
                    if next == '{' {
                        tokens.push(Token::Output(body));
                    } else {
                        tokens.push(Token::Tag(body));
                    }
                }

                trim_next = strip_after;
                index = body_end + 2;
                continue;
            }
        }

        if trim_next && ch.is_whitespace() {
            index += 1;
            continue;
        }
        trim_next = false;
        text.push(ch);
        index += 1;
    }

    if !text.is_empty() {
        tokens.push(Token::Text(text));
    }
    Ok(tokens)
}

// ---------------------------------------------------------------------------
// 表达式
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Expr {
    Literal(Value),
    Path(Vec<String>),
    Call(String, Vec<Expr>),
    Filter(Box<Expr>, String, Vec<Expr>),
    Not(Box<Expr>),
    Binary(Box<Expr>, String, Box<Expr>),
}

impl Expr {
    fn eval(&self, scope: &Scope) -> Value {
        match self {
            Expr::Literal(value) => value.clone(),
            Expr::Path(parts) => lookup(scope, parts),
            Expr::Call(name, args) => {
                let values: Vec<Value> = args.iter().map(|arg| arg.eval(scope)).collect();
                call_function(name, &values)
            }
            Expr::Filter(inner, name, args) => {
                let value = inner.eval(scope);
                let values: Vec<Value> = args.iter().map(|arg| arg.eval(scope)).collect();
                apply_filter(value, name, &values)
            }
            Expr::Not(inner) => Value::Bool(!truthy(&inner.eval(scope))),
            Expr::Binary(left, op, right) => {
                let l = left.eval(scope);
                let r = right.eval(scope);
                compare(&l, op, &r)
            }
        }
    }
}

fn lookup(scope: &Scope, parts: &[String]) -> Value {
    let Some(first) = parts.first() else {
        return Value::Null;
    };
    let mut current = scope.get(first).cloned().unwrap_or(Value::Null);
    for part in &parts[1..] {
        current = match current {
            Value::Object(map) => map.get(part).cloned().unwrap_or(Value::Null),
            Value::Array(items) => part
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get(index).cloned())
                .unwrap_or(Value::Null),
            _ => Value::Null,
        };
    }
    current
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().unwrap_or(0.0) != 0.0,
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

fn compare(left: &Value, op: &str, right: &Value) -> Value {
    let equal = match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        _ => as_text(left) == as_text(right),
    };
    let ordering = match (left.as_f64(), right.as_f64()) {
        (Some(a), Some(b)) => a.partial_cmp(&b),
        _ => Some(as_text(left).cmp(&as_text(right))),
    };
    let result = match op {
        "==" => equal,
        "!=" => !equal,
        ">" => matches!(ordering, Some(std::cmp::Ordering::Greater)),
        "<" => matches!(ordering, Some(std::cmp::Ordering::Less)),
        ">=" => matches!(
            ordering,
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        ),
        "<=" => matches!(
            ordering,
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ),
        "in" => match right {
            Value::Array(items) => items.iter().any(|item| as_text(item) == as_text(left)),
            Value::String(text) => text.contains(&as_text(left)),
            _ => false,
        },
        _ => false,
    };
    Value::Bool(result)
}

/// 值转文本（模板输出用）。
pub fn as_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn apply_filter(value: Value, name: &str, _args: &[Value]) -> Value {
    match name {
        "escape" | "e" => Value::String(html_escape(&as_text(&value))),
        "safe" => value,
        "length" => Value::from(match &value {
            Value::Array(items) => items.len(),
            Value::String(text) => text.chars().count(),
            Value::Object(map) => map.len(),
            _ => 0,
        }),
        "default" => {
            if truthy(&value) {
                value
            } else {
                _args.first().cloned().unwrap_or(Value::Null)
            }
        }
        "upper" => Value::String(as_text(&value).to_uppercase()),
        "lower" => Value::String(as_text(&value).to_lowercase()),
        "trim" => Value::String(as_text(&value).trim().to_string()),
        _ => value,
    }
}

/// 把 `2006-01-02 15:04:05` 这类参考格式串转成 chrono 的 strftime。
pub(crate) fn go_layout_to_chrono(layout: &str) -> String {
    layout
        .replace("2006", "%Y")
        .replace("01", "%m")
        .replace("02", "%d")
        .replace("15", "%H")
        .replace("04", "%M")
        .replace("05", "%S")
}

fn call_function(name: &str, args: &[Value]) -> Value {
    match name {
        "stampToDate" | "date" => {
            let timestamp = args.first().map(|v| v.as_i64().unwrap_or(0)).unwrap_or(0);
            let layout = args
                .get(1)
                .map(as_text)
                .unwrap_or_else(|| "2006-01-02".into());
            match chrono::DateTime::from_timestamp(timestamp, 0) {
                Some(dt) => Value::String(dt.format(&go_layout_to_chrono(&layout)).to_string()),
                None => Value::String(String::new()),
            }
        }
        "now" => Value::from(chrono::Utc::now().timestamp()),
        "len" => Value::from(match args.first() {
            Some(Value::Array(items)) => items.len(),
            Some(Value::String(text)) => text.chars().count(),
            _ => 0,
        }),
        _ => Value::Null,
    }
}

struct ExprParser<'a> {
    tokens: Vec<String>,
    pos: usize,
    source: &'a str,
}

impl<'a> ExprParser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            tokens: split_tokens(source),
            pos: 0,
            source,
        }
    }

    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(|s| s.as_str())
    }

    fn next(&mut self) -> Option<String> {
        let item = self.tokens.get(self.pos).cloned();
        if item.is_some() {
            self.pos += 1;
        }
        item
    }

    fn eat(&mut self, expected: &str) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            return true;
        }
        false
    }

    fn parse(&mut self) -> Result<Expr> {
        let expr = self.parse_or()?;
        if self.pos < self.tokens.len() {
            bail!(
                "表达式多余内容：{}（{}）",
                self.tokens[self.pos],
                self.source
            );
        }
        Ok(expr)
    }

    fn parse_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_and()?;
        while self.eat("or") {
            let right = self.parse_and()?;
            left = Expr::Binary(Box::new(left), "or".into(), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_not()?;
        while self.eat("and") {
            let right = self.parse_not()?;
            left = Expr::Binary(Box::new(left), "and".into(), Box::new(right));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr> {
        if self.eat("not") {
            return Ok(Expr::Not(Box::new(self.parse_not()?)));
        }
        self.parse_compare()
    }

    fn parse_compare(&mut self) -> Result<Expr> {
        let left = self.parse_postfix()?;
        for op in ["==", "!=", ">=", "<=", ">", "<", "in"] {
            if self.eat(op) {
                let right = self.parse_postfix()?;
                return Ok(Expr::Binary(
                    Box::new(left),
                    op.to_string(),
                    Box::new(right),
                ));
            }
        }
        Ok(left)
    }

    fn parse_postfix(&mut self) -> Result<Expr> {
        let mut expr = self.parse_primary()?;
        while self.peek() == Some("|") {
            self.pos += 1;
            let name = self
                .next()
                .ok_or_else(|| anyhow!("过滤器缺少名字：{}", self.source))?;
            let mut args = Vec::new();
            if self.eat("(") {
                while !self.eat(")") {
                    args.push(self.parse_or()?);
                    self.eat(",");
                }
            } else if self.eat(":") {
                // 模板过滤器风格：`{{ x|default:'/search' }}`
                args.push(self.parse_primary()?);
            }
            expr = Expr::Filter(Box::new(expr), name, args);
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        let token = self
            .next()
            .ok_or_else(|| anyhow!("表达式为空：{}", self.source))?;
        if token == "(" {
            let expr = self.parse_or()?;
            self.eat(")");
            return Ok(expr);
        }
        if (token.starts_with('"') && token.ends_with('"'))
            || (token.starts_with('\'') && token.ends_with('\''))
        {
            let inner = token[1..token.len() - 1].to_string();
            return Ok(Expr::Literal(Value::String(inner)));
        }
        if token == "true" {
            return Ok(Expr::Literal(Value::Bool(true)));
        }
        if token == "false" {
            return Ok(Expr::Literal(Value::Bool(false)));
        }
        if token == "nil" || token == "null" {
            return Ok(Expr::Literal(Value::Null));
        }
        if let Ok(number) = token.parse::<f64>() {
            return Ok(Expr::Literal(Value::Number(
                serde_json::Number::from_f64(number).unwrap_or_else(|| 0.into()),
            )));
        }
        // 函数调用
        if self.peek() == Some("(") {
            self.pos += 1;
            let mut args = Vec::new();
            while !self.eat(")") {
                args.push(self.parse_or()?);
                self.eat(",");
            }
            return Ok(Expr::Call(token, args));
        }
        // 变量路径
        let parts: Vec<String> = token.split('.').map(|part| part.to_string()).collect();
        Ok(Expr::Path(parts))
    }
}

/// 把标签/表达式内容切成 token：保留引号字符串、运算符、括号、管道。
fn split_tokens(source: &str) -> Vec<String> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];
        if ch.is_whitespace() {
            index += 1;
            continue;
        }
        if ch == '"' || ch == '\'' {
            let quote = ch;
            let start = index;
            index += 1;
            while index < chars.len() && chars[index] != quote {
                index += 1;
            }
            index = (index + 1).min(chars.len());
            tokens.push(chars[start..index].iter().collect());
            continue;
        }
        if "()|,:".contains(ch) {
            tokens.push(ch.to_string());
            index += 1;
            continue;
        }
        if "=!<>".contains(ch) {
            let mut op = ch.to_string();
            if index + 1 < chars.len() && chars[index + 1] == '=' {
                op.push('=');
                index += 1;
            }
            tokens.push(op);
            index += 1;
            continue;
        }
        let start = index;
        while index < chars.len()
            && !chars[index].is_whitespace()
            && !"()|,=!<>:".contains(chars[index])
            && chars[index] != '"'
            && chars[index] != '\''
        {
            index += 1;
        }
        tokens.push(chars[start..index].iter().collect());
    }
    tokens
}

// ---------------------------------------------------------------------------
// 语法树
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Node {
    Text(String),
    Output(Expr),
    If {
        branches: Vec<(Option<Expr>, Vec<Node>)>,
    },
    For {
        var: String,
        iter: Expr,
        body: Vec<Node>,
    },
    Include(String),
    Extends(String),
    /// `{% block name %}…{% endblock %}`：既是默认内容，也是可被覆盖的槽位
    BlockDef(String, Vec<Node>),
    /// 自定义标签：块标签（body 非空且存在 end 标签）
    Tag {
        name: String,
        var: Option<String>,
        args: Vec<(String, Expr)>,
        body: Option<Vec<Node>>,
    },
}

/// 块名 → 块内容（父子模板合并后使用）
type BlockMap = HashMap<String, Vec<Node>>;

#[derive(Debug, Default)]
struct Template {
    extends: Option<String>,
    blocks: BlockMap,
    nodes: Vec<Node>,
}

fn parse_template(source: &str) -> Result<Template> {
    let tokens = tokenize(source)?;

    // 先收集哪些标签是块标签（存在 `{% endX %}`）
    let mut block_names: HashSet<String> = HashSet::new();
    for token in &tokens {
        if let Token::Tag(content) = token {
            let name = first_word(content);
            if let Some(rest) = name.strip_prefix("end") {
                if !rest.is_empty() {
                    block_names.insert(rest.to_lowercase());
                }
            }
        }
    }

    let mut cursor = 0usize;
    let (nodes, _) = parse_nodes(&tokens, &mut cursor, &block_names, None)?;

    let mut template = Template::default();
    for node in nodes {
        match node {
            Node::Extends(path) => template.extends = Some(path),
            Node::BlockDef(name, body) => {
                // 默认内容留一份给「没被子模板覆盖」的情况
                template.blocks.insert(name.clone(), body.clone());
                template.nodes.push(Node::BlockDef(name, body));
            }
            other => template.nodes.push(other),
        }
    }

    Ok(template)
}

fn first_word(content: &str) -> String {
    content.split_whitespace().next().unwrap_or("").to_string()
}

fn parse_nodes(
    tokens: &[Token],
    cursor: &mut usize,
    block_names: &HashSet<String>,
    stop: Option<&[&str]>,
) -> Result<(Vec<Node>, Option<String>)> {
    let mut nodes = Vec::new();
    while *cursor < tokens.len() {
        match &tokens[*cursor] {
            Token::Text(text) => {
                nodes.push(Node::Text(text.clone()));
                *cursor += 1;
            }
            Token::Output(expr) => {
                let expr = ExprParser::new(expr).parse()?;
                nodes.push(Node::Output(expr));
                *cursor += 1;
            }
            Token::Tag(content) => {
                let name = first_word(content);
                let lower = name.to_lowercase();
                if let Some(stops) = stop {
                    if stops.contains(&lower.as_str()) {
                        *cursor += 1;
                        return Ok((nodes, Some(lower)));
                    }
                }
                *cursor += 1;

                match lower.as_str() {
                    "if" => {
                        let rest = content[name.len()..].trim().to_string();
                        let mut branches = Vec::new();
                        let mut current_cond = Some(ExprParser::new(&rest).parse()?);
                        loop {
                            let (body, closed) = parse_nodes(
                                tokens,
                                cursor,
                                block_names,
                                Some(&["else", "elif", "endif"]),
                            )?;
                            branches.push((current_cond.take(), body));
                            match closed.as_deref() {
                                Some("endif") => break,
                                Some("elif") => {
                                    let cond_src = match tokens.get(*cursor - 1) {
                                        Some(Token::Tag(inner)) => {
                                            inner[first_word(inner).len()..].trim().to_string()
                                        }
                                        _ => String::new(),
                                    };
                                    current_cond = Some(ExprParser::new(&cond_src).parse()?);
                                }
                                Some("else") => {
                                    let (body, _) =
                                        parse_nodes(tokens, cursor, block_names, Some(&["endif"]))?;
                                    branches.push((None, body));
                                    break;
                                }
                                _ => bail!("if 缺少 endif"),
                            }
                        }
                        nodes.push(Node::If { branches });
                    }
                    "for" => {
                        let rest = content[name.len()..].trim().to_string();
                        let tokens_inner = split_tokens(&rest);
                        // for x in xs
                        let var = tokens_inner.first().cloned().unwrap_or_default();
                        let iter_src = tokens_inner
                            .iter()
                            .skip_while(|token| *token != "in")
                            .skip(1)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" ");
                        let iter = ExprParser::new(&iter_src).parse()?;
                        let (body, _) =
                            parse_nodes(tokens, cursor, block_names, Some(&["endfor"]))?;
                        nodes.push(Node::For { var, iter, body });
                    }
                    "block" => {
                        let rest = content[name.len()..].trim();
                        let block_name = split_tokens(rest).first().cloned().unwrap_or_default();
                        let (body, _) =
                            parse_nodes(tokens, cursor, block_names, Some(&["endblock"]))?;
                        nodes.push(Node::BlockDef(block_name, body));
                    }
                    "include" => {
                        let rest = content[name.len()..].trim();
                        let path = match ExprParser::new(rest).parse()? {
                            Expr::Literal(Value::String(path)) => path,
                            _ => bail!("include 需要字符串路径"),
                        };
                        nodes.push(Node::Include(path));
                    }
                    "extends" => {
                        let rest = content[name.len()..].trim();
                        let path = match ExprParser::new(rest).parse()? {
                            Expr::Literal(Value::String(path)) => path,
                            _ => bail!("extends 需要字符串路径"),
                        };
                        nodes.push(Node::Extends(path));
                    }
                    _ if lower.starts_with("end") => {
                        // 多余/未匹配的 end 标签：忽略
                    }
                    _ => {
                        let (name, var, args) = parse_tag_call(content)?;
                        if block_names.contains(&lower) {
                            let end_name = format!("end{lower}");
                            let (body, _) = parse_nodes(
                                tokens,
                                cursor,
                                block_names,
                                Some(&[end_name.as_str()]),
                            )?;
                            nodes.push(Node::Tag {
                                name,
                                var,
                                args,
                                body: Some(body),
                            });
                        } else {
                            nodes.push(Node::Tag {
                                name,
                                var,
                                args,
                                body: None,
                            });
                        }
                    }
                }
            }
        }
    }
    Ok((nodes, None))
}

/// 解析出的标签调用：(标签名, 绑定的变量名, 参数列表)
type TagCall = (String, Option<String>, Vec<(String, Expr)>);

/// 解析 `tagName [var] with key=value key2=value2 ...`
///
/// 注意 `split_tokens` 会把 `=` 单独切出来，这里按 `key`、`=`、`value` 三连处理。
fn parse_tag_call(content: &str) -> Result<TagCall> {
    let tokens = split_tokens(content);
    let name = tokens.first().cloned().unwrap_or_default();
    let mut var = None;
    let mut args = Vec::new();
    let mut index = 1usize;
    // 只有「第一个位置参数是标识符」才当作输出变量；
    // 例如 `{% tr 'welcome%s' siteName %}` 里的 siteName 是参数而不是变量。
    let mut first_positional = true;

    while index < tokens.len() {
        let token = tokens[index].clone();
        if token == "with" {
            index += 1;
            continue;
        }
        // key = value
        if index + 2 < tokens.len() && tokens[index + 1] == "=" {
            let expr = ExprParser::new(&tokens[index + 2]).parse()?;
            args.push((token, expr));
            index += 3;
            first_positional = false;
            continue;
        }
        let is_ident = !token.is_empty()
            && token
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
            && !token.chars().next().unwrap_or('0').is_ascii_digit();
        if first_positional && var.is_none() && is_ident {
            var = Some(token);
        } else {
            args.push((
                format!("__arg{}", args.len()),
                ExprParser::new(&token).parse()?,
            ));
        }
        first_positional = false;
        index += 1;
    }

    Ok((name, var, args))
}

// ---------------------------------------------------------------------------
// 渲染
// ---------------------------------------------------------------------------

/// 模板引擎：按名字加载文件、解析并缓存。
pub struct Engine {
    dir: PathBuf,
    cache: RwLock<HashMap<String, Arc<Template>>>,
}

impl Engine {
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_path_buf(),
            cache: RwLock::new(HashMap::new()),
        }
    }

    #[allow(dead_code)] // 预留给模板热重载/后台预览
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn load(&self, name: &str) -> Result<Arc<Template>> {
        if let Some(cached) = self
            .cache
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .get(name)
            .cloned()
        {
            return Ok(cached);
        }
        let path = self.dir.join(name);
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("读取模板失败：{}", path.display()))?;
        let mut template = parse_template(&source)?;
        template.extends = template.extends.take();
        let template = Arc::new(template);
        self.cache
            .write()
            .unwrap_or_else(|err| err.into_inner())
            .insert(name.to_string(), template.clone());
        Ok(template)
    }

    pub async fn render(
        &self,
        name: &str,
        scope: &Scope,
        provider: &dyn Provider,
    ) -> Result<String> {
        let mut out = String::new();
        let mut blocks: BlockMap = HashMap::new();
        self.render_into(name, scope.clone(), provider, &mut blocks, &mut out)
            .await?;
        Ok(out)
    }

    fn render_into<'a>(
        &'a self,
        name: &'a str,
        scope: Scope,
        provider: &'a dyn Provider,
        blocks: &'a mut BlockMap,
        out: &'a mut String,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let template = self.load(name)?;
            let mut merged = blocks.clone();
            for (key, value) in &template.blocks {
                merged.entry(key.clone()).or_insert_with(|| value.clone());
            }
            // 子模板：把合并后的 block 交给父模板渲染
            if let Some(parent) = &template.extends {
                let parent = parent.clone();
                return self
                    .render_into(&parent, scope, provider, &mut merged, out)
                    .await;
            }
            let mut scope = scope;
            self.render_nodes(&template.nodes, &mut scope, provider, &merged, out)
                .await
        })
    }

    fn render_nodes<'a>(
        &'a self,
        nodes: &'a [Node],
        scope: &'a mut Scope,
        provider: &'a dyn Provider,
        blocks: &'a BlockMap,
        out: &'a mut String,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            for node in nodes {
                match node {
                    Node::Text(text) => out.push_str(text),
                    Node::Output(expr) => {
                        let value = expr.eval(scope);
                        out.push_str(&as_text(&value));
                    }
                    Node::Include(path) => {
                        let mut nested = scope.clone();
                        let fragment = self
                            .render_snippet(path, &mut nested, provider, blocks)
                            .await?;
                        out.push_str(&fragment);
                    }
                    // extends 已在 parse_template 阶段被提走，这里不会出现
                    Node::Extends(_) => {}
                    Node::BlockDef(name, default_body) => {
                        // 有覆盖用覆盖，否则用本模板的默认内容
                        let body = blocks
                            .get(name)
                            .cloned()
                            .unwrap_or_else(|| default_body.clone());
                        let mut nested = scope.clone();
                        self.render_nodes(&body, &mut nested, provider, blocks, out)
                            .await?;
                    }
                    Node::If { branches } => {
                        for (cond, body) in branches {
                            let matched = match cond {
                                Some(expr) => truthy(&expr.eval(scope)),
                                None => true,
                            };
                            if matched {
                                let mut nested = scope.clone();
                                self.render_nodes(body, &mut nested, provider, blocks, out)
                                    .await?;
                                break;
                            }
                        }
                    }
                    Node::For { var, iter, body } => {
                        let items = iter.eval(scope);
                        // 模板允许直接遍历字符串（按字符逐个迭代）
                        let list: Vec<Value> = match items {
                            Value::Array(items) => items,
                            Value::Object(map) => map.values().cloned().collect(),
                            Value::String(text) => text
                                .chars()
                                .map(|ch| Value::String(ch.to_string()))
                                .collect(),
                            _ => Vec::new(),
                        };
                        let total = list.len();
                        for (index, item) in list.into_iter().enumerate() {
                            let mut nested = scope.clone();
                            nested.insert(var.clone(), item);
                            // forloop 对象：Counter/First/Last
                            let mut info = Map::new();
                            info.insert("Counter".into(), Value::from(index + 1));
                            info.insert("First".into(), Value::Bool(index == 0));
                            info.insert("Last".into(), Value::Bool(index + 1 == total));
                            nested.insert("forloop".into(), Value::Object(info));
                            self.render_nodes(body, &mut nested, provider, blocks, out)
                                .await?;
                        }
                    }
                    Node::Tag {
                        name,
                        var,
                        args,
                        body,
                    } => {
                        let mut values = Map::new();
                        for (key, expr) in args {
                            values.insert(key.clone(), expr.eval(scope));
                        }

                        let data = match provider.data(name, &values).await {
                            Ok(data) => data,
                            Err(err) => {
                                // 标签数据出错不影响整页渲染，留空并记日志
                                tracing::warn!("模板标签 {name} 取数失败：{err:#}");
                                Value::Null
                            }
                        };

                        match body {
                            Some(body) => {
                                let mut nested = scope.clone();
                                if let Some(var) = var {
                                    nested.insert(var.clone(), data.clone());
                                }
                                self.render_nodes(body, &mut nested, provider, blocks, out)
                                    .await?;
                            }
                            None => {
                                if let Some(var) = var {
                                    scope.insert(var.clone(), data);
                                } else {
                                    out.push_str(&as_text(&data));
                                }
                            }
                        }
                    }
                }
            }
            Ok(())
        })
    }

    fn render_snippet<'a>(
        &'a self,
        name: &'a str,
        scope: &'a mut Scope,
        provider: &'a dyn Provider,
        blocks: &'a BlockMap,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let template = self.load(name)?;
            let mut out = String::new();
            self.render_nodes(&template.nodes, scope, provider, blocks, &mut out)
                .await?;
            Ok(out)
        })
    }
}

/// 简单模板（字符串）渲染，单测用。
#[cfg(test)]
pub async fn render_str(source: &str, scope: Scope, provider: &dyn Provider) -> Result<String> {
    let tokens = tokenize(source)?;
    let mut block_names = HashSet::new();
    for token in &tokens {
        if let Token::Tag(content) = token {
            if let Some(rest) = first_word(content).strip_prefix("end") {
                block_names.insert(rest.to_lowercase());
            }
        }
    }
    let mut cursor = 0usize;
    let (nodes, _) = parse_nodes(&tokens, &mut cursor, &block_names, None)?;
    let engine = Engine::new(".");
    let mut scope = scope;
    let mut out = String::new();
    engine
        .render_nodes(&nodes, &mut scope, provider, &HashMap::new(), &mut out)
        .await?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn normalizes_language_codes() {
        assert_eq!(normalize_language("zh"), "zh-cn");
        assert_eq!(normalize_language("zh_CN"), "zh-cn");
        assert_eq!(normalize_language("zh-CN"), "zh-cn");
        assert_eq!(normalize_language("en"), "en-us");
        assert_eq!(normalize_language("en_US"), "en-us");
        assert_eq!(normalize_language("ja-JP"), "ja-jp");
    }

    #[test]
    fn language_display_and_emoji() {
        assert_eq!(language_display_name("zh"), "简体中文");
        assert_eq!(language_display_name("en-us"), "English");
        assert_eq!(language_emoji("zh-cn"), "🇨🇳");
        assert_eq!(language_emoji("en"), "🇬🇧");
        assert_eq!(language_emoji("ja-jp"), "");
    }

    #[test]
    fn parses_language_pack_yaml() {
        let pack = parse_language_pack(
            "# 注释行\n\"welcome%s\": \"欢迎来到 %s 官方网站!\"\n\"total-items%d\": \"共 %d 条\"\nplain: value\n\nbroken line without colon",
        );
        assert_eq!(pack.get("welcome%s").map(String::as_str), Some("欢迎来到 %s 官方网站!"));
        assert_eq!(pack.get("total-items%d").map(String::as_str), Some("共 %d 条"));
        assert_eq!(pack.get("plain").map(String::as_str), Some("value"));
        assert_eq!(pack.len(), 3);
    }

    #[test]
    fn loads_real_language_pack_from_template_dir() {
        // 测试进程未调 set_template_dir：language_pack 回退 template/default（仓库内模板）
        let pack = language_pack("zh");
        assert_eq!(pack.get("welcome%s").map(String::as_str), Some("欢迎来到 %s 官方网站!"));
        assert_eq!(pack.get("SearchPlaceholder").map(String::as_str), Some("搜索产品、新闻…"));
        // 语言包缺失时是空表（tr 回退词条名）
        assert!(language_pack("fr-fr").is_empty());
    }

    struct StubProvider {
        archives: Vec<Value>,
        calls: Mutex<Vec<String>>,
    }

    impl Provider for StubProvider {
        fn data<'a>(
            &'a self,
            tag: &'a str,
            args: &'a Map<String, Value>,
        ) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + 'a>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .push(format!("{tag}({:?})", args));
                match tag {
                    "archiveList" => Ok(Value::Array(self.archives.clone())),
                    "categoryDetail" => Ok(match args.get("name").map(as_text).as_deref() {
                        Some("Title") => Value::String("新闻资讯".into()),
                        Some("Link") => Value::String("/news/".into()),
                        _ => Value::Null,
                    }),
                    "system" => Ok(match args.get("name").map(as_text).as_deref() {
                        Some("SiteName") => Value::String("淘码网".into()),
                        Some("TemplateUrl") => Value::String("/static/default/".into()),
                        _ => Value::Null,
                    }),
                    "tr" => Ok(Value::String(
                        args.get("__arg0").map(as_text).unwrap_or_default(),
                    )),
                    "unknownTag" => Ok(Value::Null),
                    _ => Ok(Value::Null),
                }
            })
        }
    }

    fn stub() -> StubProvider {
        StubProvider {
            archives: vec![
                Value::Object(Map::from_iter([
                    ("Title".to_string(), Value::String("第一篇".into())),
                    ("Link".to_string(), Value::String("/a/1.html".into())),
                    ("CreatedTime".to_string(), Value::from(1_694_745_839i64)),
                ])),
                Value::Object(Map::from_iter([
                    ("Title".to_string(), Value::String("第二篇".into())),
                    ("Link".to_string(), Value::String("/a/2.html".into())),
                    ("CreatedTime".to_string(), Value::from(1_694_745_900i64)),
                ])),
            ],
            calls: Mutex::new(Vec::new()),
        }
    }

    #[tokio::test]
    async fn renders_text_variables_and_filters() {
        let provider = stub();
        let mut scope = Scope::new();
        scope.insert("name".into(), Value::String("<b>淘码</b>".into()));
        scope.insert(
            "item".into(),
            Value::Object(Map::from_iter([("Title".to_string(), Value::from("标题"))])),
        );

        let html = render_str(
            "你好 {{ name }} / {{ name|escape }} / {{ item.Title }} / {{ missing }}",
            scope,
            &provider,
        )
        .await
        .unwrap();
        assert_eq!(html, "你好 <b>淘码</b> / &lt;b&gt;淘码&lt;/b&gt; / 标题 / ");
    }

    #[tokio::test]
    async fn renders_if_else_and_for() {
        let provider = stub();
        let mut scope = Scope::new();
        scope.insert("user".into(), Value::String("root".into()));
        scope.insert(
            "list".into(),
            Value::Array(vec![Value::from(1), Value::from(2), Value::from(3)]),
        );

        let html = render_str(
            "{% if user %}有用户{% else %}无用户{% endif %}|{% for n in list %}{{ n }},{% endfor %}",
            scope,
            &provider,
        )
        .await
        .unwrap();
        assert_eq!(html, "有用户|1,2,3,");
    }

    #[tokio::test]
    async fn iterates_string_chars_in_for() {
        let html = render_str(
            "{% for ch in \"ABC\" %}[{{ ch }}]{% endfor %}",
            Scope::new(),
            &EmptyProvider,
        )
        .await
        .unwrap();
        assert_eq!(html, "[A][B][C]");
    }

    #[tokio::test]
    async fn renders_block_and_inline_tags() {
        let provider = stub();
        let html = render_str(
            "{% archiveList articles with type=\"page\" limit=\"10\" %}\
             {% for item in articles %}{{ item.Title }}@{{ item.CreatedTime|length }};{% endfor %}\
             {% endarchiveList %}",
            Scope::new(),
            &provider,
        )
        .await
        .unwrap();
        assert!(html.contains("第一篇"), "{html}");
        assert!(html.contains("第二篇"), "{html}");

        // 行内标签：categoryDetail / system / tdk 绑定变量
        let html = render_str(
            "{% categoryDetail with name=\"Title\" %} / \
             {% system siteName with name=\"SiteName\" %}{{ siteName }} / \
             {% tr 'welcome' %}",
            Scope::new(),
            &provider,
        )
        .await
        .unwrap();
        assert_eq!(html, "新闻资讯 / 淘码网 / welcome");
    }

    #[tokio::test]
    async fn renders_extends_and_include() {
        let dir = std::env::temp_dir().join(format!("rustcms-tpl-{}", std::process::id()));
        let partial = dir.join("partial");
        std::fs::create_dir_all(&partial).unwrap();
        std::fs::write(
            dir.join("base.html"),
            "<html><head>{% block title %}默认标题{% endblock %}</head><body>{% block main %}主体{% endblock %}</body></html>",
        )
        .unwrap();
        std::fs::write(
            dir.join("child.html"),
            "{% extends 'base.html' %}{% block main %}{% include \"partial/x.html\" %}{% endblock %}",
        )
        .unwrap();
        std::fs::write(partial.join("x.html"), "片段:{{ name }}").unwrap();

        let engine = Engine::new(&dir);
        let mut scope = Scope::new();
        scope.insert("name".into(), Value::from("淘码"));
        let html = engine
            .render("child.html", &scope, &EmptyProvider)
            .await
            .unwrap();
        assert_eq!(
            html,
            "<html><head>默认标题</head><body>片段:淘码</body></html>"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn trims_whitespace_and_skips_unknown_tags() {
        let provider = stub();
        let html = render_str(
            "a\n   {%- if true -%}   b   {%- endif -%}\n   c{% unknownTag %}d{# 注释 #}",
            Scope::new(),
            &provider,
        )
        .await
        .unwrap();
        assert_eq!(html, "abcd");

        let calls = provider.calls.lock().unwrap().clone();
        assert!(calls.iter().any(|call| call.starts_with("unknownTag")));
    }

    #[tokio::test]
    async fn formats_timestamps_like_go_layouts() {
        let html = render_str(
            "{{ stampToDate(1694745839, \"2006-01-02\") }}",
            Scope::new(),
            &EmptyProvider,
        )
        .await
        .unwrap();
        // 2023-09-15 按东八区/UTC 都可能是 15 号，这里只断言格式
        assert_eq!(html.len(), 10);
        assert_eq!(&html[4..5], "-");
    }
}

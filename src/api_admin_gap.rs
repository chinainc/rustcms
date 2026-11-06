//! 后台 API 缺口补齐（批次 E）。
//!
//! 对应 Go 版 `route/manage.go` 里 Rust 侧尚未覆盖的后台端点：
//! 找回密码三步、内容导入（上传 / 状态 / Excel 模板）、
//! `archive` 的删除图片 / 补 alt / 定时计划 / 发布 / 标签、
//! 附件远程地址登记、锚文本导入、`htmlcache` 证书上传、`setting/convert/webp`。
//!
//! 路由使用**绝对路径**（`/system/api/...`），由主线在 `routes.rs` 里
//! `merge` 进来；本文件不改动 `routes.rs`。
//!
//! 鉴权口径：
//! - `/system/api/password/*`（找回密码）在 Go 版里**没有**挂后台令牌中间件，
//!   因为它是「还没登录的后台管理员」用来重置账号密码的，所以这里也不鉴权；
//! - 其余端点一律先 `require_admin`，失败直接返回那个错误响应。
//!
//! 实现口径：
//! - `archive/import`：zip 取文本条目、xlsx 读首个工作表（不处理图片与分片上传）；
//! - `setting/convert/webp`：依赖外置 `cwebp` 工具，缺失时明确提示替代方案；
//! - `password/verify` 的 DNS 方式依赖系统解析器（`nslookup -type=TXT`）。

use axum::extract::{ConnectInfo, Multipart, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::models::archive_write::{self, ArchiveInput};
use crate::models::{
    admin, admin_gap, anchor, archive_admin, attachment, category, module, plugin, site_setting,
};
use crate::routes::{fail, failed, model_error, ok, ok_msg, require_admin, NO_DB};
use crate::state::AppState;

/// 本批路由表。
pub fn router() -> Router<AppState> {
    Router::new()
        // 找回密码三步
        .route("/system/api/password/choose", post(password_choose))
        .route(
            "/system/api/password/verify",
            get(password_verify).post(password_verify),
        )
        .route("/system/api/password/reset", post(password_reset))
        // 内容批量导入
        .route("/system/api/archive/import", post(archive_import))
        .route("/system/api/archive/import/status", get(archive_import_status))
        .route(
            "/system/api/archive/import/exceltemplate",
            post(archive_import_excel_template),
        )
        // 文档批量操作
        .route("/system/api/archive/delete/image", post(archive_delete_image))
        .route("/system/api/archive/fill-img-alt", post(archive_fill_img_alt))
        .route("/system/api/archive/plan", post(archive_plan))
        .route("/system/api/archive/release", post(archive_release))
        .route("/system/api/archive/tags", post(archive_tags))
        // 附件 / 锚文本 / 静态缓存 / 图片转码
        .route("/system/api/attachment/addurl", post(attachment_addurl))
        .route("/system/api/anchor/addfromtitle", post(anchor_add_from_title))
        .route(
            "/system/api/plugin/anchor/addfromtitle",
            post(anchor_add_from_title),
        )
        .route("/system/api/htmlcache/upload", post(htmlcache_upload))
        .route("/system/api/plugin/htmlcache/upload", post(htmlcache_upload))
        .route("/system/api/setting/convert/webp", post(setting_convert_webp))
}

/// 带提示语的成功信封（Go 版不少接口是 `code:0` + `msg` + `data`）。
fn ok_data_msg(data: impl serde::Serialize, msg: impl Into<String>) -> Response {
    axum::Json(json!({ "code": 0, "msg": msg.into(), "data": data })).into_response()
}

// ---------------------------------------------------------------------------
// 找回密码三步
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct PasswordChooseBody {
    /// 验证方式：`file`（上传令牌文件）/ `dns`（域名 TXT 解析）。
    #[serde(default)]
    way: String,
}

/// `POST /system/api/password/choose`
///
/// 选定验证方式并生成一次性令牌（Go：`FindPasswordChooseWay`）。
async fn password_choose(
    State(state): State<AppState>,
    Json(body): Json<PasswordChooseBody>,
) -> Response {
    let way = body.way.trim().to_string();
    if way != admin_gap::FIND_WAY_FILE && way != admin_gap::FIND_WAY_DNS {
        return fail("无效的验证方式");
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let secret = admin_gap::site_token_secret(pool).await;
    let base_url = admin_gap::site_base_url(pool).await;
    match admin_gap::find_password_choose(&secret, &way, &base_url).await {
        Ok(info) => ok(info),
        Err(err) => model_error(err),
    }
}

/// `GET|POST /system/api/password/verify`
///
/// 校验找回密码的验证结果（Go：`FindPasswordVerify`）。
/// - 文件方式：读 `<public>/<token>.txt`，内容必须等于令牌；
/// - DNS 方式：用系统 `nslookup` 查询 `_rustcms.<域名>` 的 TXT 记录并比对令牌。
async fn password_verify(State(state): State<AppState>) -> Response {
    if !state.has_db() {
        return fail(NO_DB);
    }
    let Some(info) = admin_gap::find_password_state().await else {
        return fail("验证已失效");
    };

    if info.way == admin_gap::FIND_WAY_FILE {
        let public_dir = state.with_config(|config| config.public_dir());
        let path = public_dir.join(format!("{}.txt", info.token));
        let content = tokio::fs::read_to_string(&path).await.unwrap_or_default();
        if content.trim() != info.token {
            return fail("文件不存在或内容不正确");
        }
    } else {
        match admin_gap::dns_txt_matches(&info.host, &info.token).await {
            Ok(true) => {}
            Ok(false) => return fail("DNS 解析不存在或内容不正确"),
            Err(msg) => return fail(msg),
        }
    }

    admin_gap::find_password_mark_verified().await;
    let Some(info) = admin_gap::find_password_state().await else {
        return fail("验证已失效");
    };
    ok_data_msg(info, "验证成功")
}

#[derive(Debug, Deserialize, Default)]
struct PasswordResetBody {
    /// 管理员账号。
    #[serde(default)]
    user_name: String,
    /// 新密码（服务端要求至少 6 位）。
    #[serde(default)]
    password: String,
}

/// `POST /system/api/password/reset`
///
/// 验证通过后重置超级管理员（`admins.id = 1`）的账号与密码（Go：`FindPasswordReset`）。
async fn password_reset(
    State(state): State<AppState>,
    Json(body): Json<PasswordResetBody>,
) -> Response {
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let Some(info) = admin_gap::find_password_state().await else {
        return fail("验证已失效");
    };
    if !info.verified {
        return fail("权限验证失败");
    }

    let user_name = body.user_name.trim().to_string();
    if user_name.is_empty() || body.password.chars().count() < 6 {
        return fail("请填写管理员账号和6位以上的密码");
    }

    let hash = match crate::auth::hash_password(&body.password) {
        Ok(value) => value,
        Err(err) => return failed(err),
    };

    match admin::get(pool, 1).await {
        Ok(Some(existing)) => {
            if let Err(err) = admin::update_profile(
                pool,
                existing.id,
                &user_name,
                existing.status,
                existing.group_id,
                Some(&hash),
            )
            .await
            {
                return failed(err);
            }
        }
        Ok(None) => {
            // Go 版：管理员不存在时按 id = 1 直接保存（等价于新建）
            if let Err(err) = admin::create(pool, &user_name, &hash, 1, 1).await {
                return failed(err);
            }
        }
        Err(err) => return failed(err),
    }

    admin_gap::find_password_clear().await;
    ok_msg("管理员账号和密码已重置")
}

// ---------------------------------------------------------------------------
// 内容批量导入
// ---------------------------------------------------------------------------

/// `POST /system/api/archive/import`
///
/// 上传 zip / xlsx 批量导入文档（对齐 Go：`QuickImportArchive`）。
/// - zip：逐个读取 `.html/.htm/.txt/.md` 文本条目，文件名作标题（`title_type=1`
///   时改用 `<title>` / 首行），正文原样写入；
/// - xlsx：读取第一个工作表，首行为表头（支持中文 / 英文字段名），逐行建文档。
///
/// 说明：zip 里的图片与分片上传暂不处理；导入同步执行并返回成功条数。
async fn archive_import(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };

    let mut file_name = String::new();
    let mut file_size = 0i64;
    let mut category_id = 0u32;
    let mut title_type = 0u32;
    let mut chunks = 0u32;
    let mut has_file = false;
    let mut bytes: Option<Vec<u8>> = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" | "file1" | "upfile" => {
                file_name = field.file_name().unwrap_or_default().to_string();
                match field.bytes().await {
                    Ok(value) => {
                        file_size = value.len() as i64;
                        has_file = true;
                        bytes = Some(value.to_vec());
                    }
                    Err(err) => return failed(err.into()),
                }
            }
            "category_id" => {
                category_id = field
                    .text()
                    .await
                    .ok()
                    .and_then(|value| value.trim().parse::<u32>().ok())
                    .unwrap_or(0);
            }
            "title_type" => {
                title_type = field
                    .text()
                    .await
                    .ok()
                    .and_then(|value| value.trim().parse::<u32>().ok())
                    .unwrap_or(0);
            }
            "chunks" => {
                chunks = field
                    .text()
                    .await
                    .ok()
                    .and_then(|value| value.trim().parse::<u32>().ok())
                    .unwrap_or(0);
            }
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    if !has_file {
        return fail("无法获取上传文件");
    }
    if file_size <= 0 {
        return fail("文件内容为空");
    }
    if file_name.is_empty() {
        return fail("无法获取上传文件名");
    }
    if chunks > 0 {
        return fail("暂不支持分片上传，请关闭分片后重新导入");
    }
    if category_id == 0 {
        return fail("请选择导入的分类");
    }
    match category::get(pool, category_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return fail("分类不存在"),
        Err(err) => return failed(err),
    }

    let Some(bytes) = bytes else {
        return fail("无法获取上传文件");
    };
    let ext = Path::new(&file_name)
        .extension()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let inputs = match ext.as_str() {
        "xlsx" => match xlsx_inputs(&bytes, category_id) {
            Some(inputs) => inputs,
            None => return fail("无法解析上传的 xlsx 文件（需包含「标题」列）"),
        },
        _ => match zip_inputs(&bytes, category_id, title_type) {
            Some(inputs) => inputs,
            None => return fail("无法解析上传的压缩包（仅支持 zip / xlsx）"),
        },
    };

    let mut succeed = 0u32;
    let mut failed_rows = 0u32;
    for input in inputs {
        let title = input.title.clone();
        match archive_write::create(pool, &input).await {
            Ok(_) => succeed += 1,
            Err(err) => {
                failed_rows += 1;
                tracing::warn!("导入文档失败（{title}）：{err}");
            }
        }
    }

    ok(json!({
        "total": succeed + failed_rows,
        "succeed": succeed,
        "failed": failed_rows,
    }))
}

/// 从 zip 中取出文本条目（支持 html / htm / txt / md）并转成文档参数。
fn zip_inputs(data: &[u8], category_id: u32, title_type: u32) -> Option<Vec<ArchiveInput>> {
    let archive = rustcms_ext::packer::ZipArchive::open(data.to_vec()).ok()?;
    let mut inputs = Vec::new();
    for entry in archive.entries() {
        if entry.is_dir {
            continue;
        }
        let name = entry.name.replace('\\', "/");
        if name.starts_with('.') || name.split('/').any(|segment| segment == "..") {
            continue;
        }
        let ext = Path::new(&name)
            .extension()
            .map(|value| value.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if !matches!(ext.as_str(), "html" | "htm" | "txt" | "md") {
            continue;
        }
        if let Ok(content) = archive.read_entry(entry) {
            if let Some(input) = build_import_input(&name, &content, category_id, title_type) {
                inputs.push(input);
            }
        }
    }
    Some(inputs)
}

/// 文件名 / 正文 → 文档写入参数；正文为空时返回 `None`。
fn build_import_input(
    name: &str,
    content: &[u8],
    category_id: u32,
    title_type: u32,
) -> Option<ArchiveInput> {
    let text = String::from_utf8(content.to_vec()).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    let ext = Path::new(name)
        .extension()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let stem = Path::new(name)
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());

    let mut title = stem;
    let mut body = text;
    if matches!(ext.as_str(), "html" | "htm") {
        if title_type == 1 {
            if let Some(found) = html_title(&body) {
                title = found;
            }
        }
    } else if body.starts_with('#') || title_type == 1 {
        if let Some((first, rest)) = body.split_once('\n') {
            let first = first.trim().trim_matches(|ch| ch == '#' || ch == '*' || ch == ' ');
            if !first.is_empty() {
                title = first.to_string();
                body = rest.to_string();
            }
        }
    }

    if title.trim().is_empty() && body.trim().is_empty() {
        return None;
    }
    Some(ArchiveInput {
        title,
        content: body,
        category_id,
        ..Default::default()
    })
}

/// 取 `<title>` 内容。
fn html_title(html: &str) -> Option<String> {
    let re = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").ok()?;
    let captured = re.captures(html)?;
    let value = captured.get(1)?.as_str().trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// 读取 xlsx 第一个工作表，首行表头，逐行映射为文档参数。
fn xlsx_inputs(data: &[u8], category_id: u32) -> Option<Vec<ArchiveInput>> {
    let archive = rustcms_ext::packer::ZipArchive::open(data.to_vec()).ok()?;
    let shared = archive
        .read_by_name("xl/sharedStrings.xml")
        .ok()
        .map(|raw| xlsx_shared_strings(&String::from_utf8_lossy(&raw)))
        .unwrap_or_default();
    let sheet = archive.read_by_name("xl/worksheets/sheet1.xml").ok()?;
    let table = xlsx_table(&String::from_utf8_lossy(&sheet), &shared);
    let headers = table.first()?;
    let columns = import_columns(headers);
    if !columns.contains_key("title") {
        return None;
    }

    let mut inputs = Vec::new();
    for row in table.iter().skip(1) {
        let cell = |key: &str| -> String {
            columns
                .get(key)
                .and_then(|index| row.get(*index))
                .map(|value| value.trim().to_string())
                .unwrap_or_default()
        };
        let title = cell("title");
        if title.is_empty() {
            continue;
        }
        inputs.push(ArchiveInput {
            title,
            content: cell("content"),
            category_id,
            seo_title: cell("seo_title"),
            keywords: cell("keywords"),
            description: cell("description"),
            url_token: Some(cell("url_token")).filter(|value| !value.is_empty()),
            origin_url: cell("origin_url"),
            ..Default::default()
        });
    }
    Some(inputs)
}

/// 表头别名 → 统一字段名。
fn import_columns(headers: &[String]) -> HashMap<String, usize> {
    let aliases: [(&str, &[&str]); 7] = [
        ("title", &["title", "标题"]),
        ("content", &["content", "内容", "正文"]),
        ("seo_title", &["seo_title", "seo标题"]),
        ("keywords", &["keywords", "关键词"]),
        ("description", &["description", "描述", "简介"]),
        ("url_token", &["url_token", "slug", "自定义链接"]),
        ("origin_url", &["origin_url", "原文地址"]),
    ];
    let mut map = HashMap::new();
    for (index, header) in headers.iter().enumerate() {
        let header = header.trim().to_ascii_lowercase();
        for (key, names) in aliases {
            if names.contains(&header.as_str()) && !map.contains_key(key) {
                map.insert(key.to_string(), index);
            }
        }
    }
    map
}

/// xlsx 共享字符串表。
fn xlsx_shared_strings(xml: &str) -> Vec<String> {
    let si = Regex::new(r"(?is)<si[^>]*>(.*?)</si>").expect("si 正则");
    let text = Regex::new(r"(?is)<t[^>]*>(.*?)</t>").expect("t 正则");
    si.captures_iter(xml)
        .map(|captured| {
            text.captures_iter(&captured[1])
                .map(|value| unescape_xml(&value[1]))
                .collect::<Vec<_>>()
                .join("")
        })
        .collect()
}

/// xlsx 工作表 → 二维单元格文本（按 `r` 列号对齐）。
fn xlsx_table(xml: &str, shared: &[String]) -> Vec<Vec<String>> {
    let row_re = Regex::new(r"(?is)<row[^>]*>(.*?)</row>").expect("row 正则");
    let cell_re = Regex::new(r"(?is)<c([^>]*)>(.*?)</c>").expect("c 正则");
    let value_re = Regex::new(r"(?is)<v[^>]*>(.*?)</v>").expect("v 正则");
    let inline_re = Regex::new(r"(?is)<t[^>]*>(.*?)</t>").expect("inline 正则");

    let mut table = Vec::new();
    for row in row_re.captures_iter(xml) {
        let mut cells: Vec<String> = Vec::new();
        for cell in cell_re.captures_iter(&row[1]) {
            let attrs = &cell[1];
            let body = &cell[2];
            let index = cell_column(attrs).unwrap_or(cells.len());
            let value = if attrs.contains("t=\"s\"") {
                value_re
                    .captures(body)
                    .and_then(|value| value[1].trim().parse::<usize>().ok())
                    .and_then(|index| shared.get(index).cloned())
                    .unwrap_or_default()
            } else if attrs.contains("t=\"inlineStr\"") {
                inline_re
                    .captures(body)
                    .map(|value| unescape_xml(&value[1]))
                    .unwrap_or_default()
            } else {
                value_re
                    .captures(body)
                    .map(|value| unescape_xml(value[1].trim()))
                    .unwrap_or_default()
            };
            if cells.len() < index {
                cells.resize(index, String::new());
            }
            if index < cells.len() {
                cells[index] = value;
            } else {
                cells.push(value);
            }
        }
        table.push(cells);
    }
    table
}

/// 单元格引用（`A1` / `AB12`）→ 0 基列号。
fn cell_column(attrs: &str) -> Option<usize> {
    let re = Regex::new(r#"r="([A-Za-z]+)\d+""#).ok()?;
    let letters = re.captures(attrs)?.get(1)?.as_str().to_ascii_uppercase();
    let mut index = 0usize;
    for ch in letters.chars() {
        if !ch.is_ascii_uppercase() {
            return None;
        }
        index = index * 26 + (ch as usize - 'A' as usize + 1);
    }
    Some(index.saturating_sub(1))
}

fn unescape_xml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// `GET /system/api/archive/import/status`
///
/// 查询批量导入任务进度（Go：`GetQuickImportArchiveStatus`）。
/// 当前导入是同步完成的，这里固定返回一个可读的空闲态，避免 `data: null`
/// 让前端无法展示状态。
async fn archive_import_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    ok(json!({
        "status": 0,
        "running": false,
        "total": 0,
        "finished": 0,
        "failed": 0,
        "percent": 100,
        "message": "当前没有进行中的导入任务",
    }))
}

#[derive(Debug, Deserialize, Default)]
struct ExcelTemplateBody {
    /// 目标分类 ID（必填）。
    #[serde(default)]
    category_id: u32,
}

/// `POST /system/api/archive/import/exceltemplate`
///
/// 生成批量导入用的 Excel 模板（Go：`GetQuickImportExcelTemplate`）。
/// 返回的是 xlsx 二进制（`Content-Disposition: attachment`），不是 JSON。
async fn archive_import_excel_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ExcelTemplateBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if body.category_id == 0 {
        return fail("category_id is required");
    }
    let category = match category::get(pool, body.category_id).await {
        Ok(Some(item)) => item,
        Ok(None) => return fail("category is empty"),
        Err(err) => return failed(err),
    };
    let module = match module::get(pool, category.module_id).await {
        Ok(Some(item)) => item,
        Ok(None) => return fail("module is empty"),
        Err(err) => return failed(err),
    };

    let mut columns: Vec<(String, String)> = vec![
        ("title".to_string(), "示例标题".to_string()),
        ("content".to_string(), "示例内容".to_string()),
        ("seo_title".to_string(), "示例SEO标题".to_string()),
        (
            "logo".to_string(),
            "https://example.com/logo.png".to_string(),
        ),
        ("keywords".to_string(), "示例关键词".to_string()),
        ("description".to_string(), "示例介绍".to_string()),
        ("price".to_string(), "9980".to_string()),
        ("stock".to_string(), "9999".to_string()),
    ];
    if let Some(raw) = module.fields.as_deref() {
        if let Ok(fields) = serde_json::from_str::<Vec<module::CustomField>>(raw) {
            for field in fields {
                if field.field_name.trim().is_empty() {
                    continue;
                }
                columns.push((field.field_name.clone(), field.name.clone()));
            }
        }
    }

    let bytes = admin_gap::build_excel_template(&columns);
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"import-template.xlsx\""),
    );
    response
}

// ---------------------------------------------------------------------------
// 文档批量操作
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct ArchiveImageDeleteBody {
    /// 文档 ID。
    #[serde(default)]
    id: i64,
    /// 要删的图片下标（从 0 开始）。
    #[serde(default)]
    image_index: usize,
}

/// `POST /system/api/archive/delete/image`
///
/// 删除文档相册里的指定图片（Go：`ArchiveDeleteImage`）。
async fn archive_delete_image(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ArchiveImageDeleteBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if body.id <= 0 {
        return fail("文档不存在");
    }
    match admin_gap::delete_archive_image(pool, body.id, body.image_index).await {
        Ok(true) => ok_msg("文章图片已删除"),
        Ok(false) => fail("图片不存在"),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/archive/fill-img-alt`
///
/// 全站回填图片 alt（Go：`ArchiveFillImgAlt`）：只补空 alt，不覆盖已有值。
async fn archive_fill_img_alt(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match admin_gap::fill_all_archive_img_alt(pool).await {
        Ok((docs, imgs)) => ok_data_msg(
            json!({ "docs": docs, "imgs": imgs }),
            format!("回填图片Alt：处理文档 {docs} 篇，补充图片 {imgs} 张"),
        ),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct ArchivePlanBody {
    /// 文档 ID 列表。
    #[serde(default)]
    ids: Vec<i64>,
    /// 每天发布数量（<= 0 时用文档总数）。
    #[serde(default)]
    daily_limit: i64,
    /// 每天开始时间（时，0-23）。
    #[serde(default)]
    start_hour: i64,
    /// 每天结束时间（时，0-23）。
    #[serde(default)]
    end_hour: i64,
}

/// `POST /system/api/archive/plan`
///
/// 批量设置定时发布计划（Go：`UpdateArchiveReleasePlan`）。
async fn archive_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ArchivePlanBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match admin_gap::set_release_plan(
        pool,
        &body.ids,
        body.daily_limit,
        body.start_hour,
        body.end_hour,
    )
    .await
    {
        Ok(count) => ok_data_msg(json!({ "count": count }), "文章已更新"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct ArchiveIdBody {
    /// 文档 ID。
    #[serde(default)]
    id: i64,
}

/// `POST /system/api/archive/release`
///
/// 发布指定的草稿文档（Go：`ArchiveRelease`）：草稿 → 正式表，发布时间取当前时间。
async fn archive_release(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ArchiveIdBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if body.id <= 0 {
        return fail("文档不存在");
    }
    // Go 版只发布草稿：正式表里的内容没有草稿行，直接报「文档不存在」
    let draft: Option<i64> = sqlx::query_scalar("SELECT id FROM archive_drafts WHERE id = ?")
        .bind(body.id)
        .fetch_optional(pool)
        .await
        .unwrap_or(None);
    if draft.is_none() {
        return fail("文档不存在");
    }
    match archive_admin::set_status(pool, &[body.id], 1).await {
        Ok(_) => ok_msg("文章已发布"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct ArchiveTagsBody {
    /// 文档 ID 列表。
    #[serde(default)]
    ids: Vec<i64>,
    /// 标签名列表（不存在会自动创建）。
    #[serde(default)]
    tags: Vec<String>,
}

/// `POST /system/api/archive/tags`
///
/// 批量修改文档标签（Go：`UpdateArchiveTags`）。
async fn archive_tags(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ArchiveTagsBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    match admin_gap::set_archive_tags(pool, &body.ids, &body.tags).await {
        Ok(count) => ok_data_msg(json!({ "count": count }), "文章已更新"),
        Err(err) => model_error(err),
    }
}

// ---------------------------------------------------------------------------
// 附件 / 锚文本 / 静态缓存 / 图片转码
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct AttachmentAddUrlBody {
    /// 分类 ID。
    #[serde(default)]
    category_id: u32,
    /// 远程地址列表（支持 `地址|文件名`）。
    #[serde(default)]
    urls: Vec<String>,
}

/// `POST /system/api/attachment/addurl`
///
/// 按远程 URL 批量登记附件（Go：`AttachmentAddRemoteUrl`）。
async fn attachment_addurl(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AttachmentAddUrlBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if body.urls.is_empty() {
        return fail("请输入远程图片地址");
    }
    match admin_gap::add_remote_urls(pool, &body.urls, body.category_id).await {
        Ok(count) => ok_data_msg(json!({ "count": count }), "远程地址添加成功"),
        Err(err) => model_error(err),
    }
}

#[derive(Debug, Deserialize, Default)]
struct AnchorFromTitleBody {
    /// 导入类型：`category` / `archive` / `tag`。
    #[serde(default, rename = "type")]
    kind: String,
    /// 对应的 ID 列表。
    #[serde(default)]
    ids: Vec<u32>,
}

/// `POST /system/api/anchor/addfromtitle`（Go 版真实路径是
/// `/system/api/plugin/anchor/addfromtitle`，两个路径都注册）。
///
/// 批量把标题导入锚文本（Go：`PluginAnchorAddFromTitle`）。
async fn anchor_add_from_title(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AnchorFromTitleBody>,
) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if body.ids.is_empty() {
        return fail("请选择要导入的数据");
    }
    let front_url = site_front_url(pool).await;
    match anchor::add_from_titles(pool, body.kind.trim(), &body.ids, &front_url).await {
        Ok(count) => ok_data_msg(json!({ "count": count }), "上传完毕"),
        Err(err) => model_error(err),
    }
}

/// `POST /system/api/plugin/htmlcache/upload`（Go 版真实路径，路由表里另有
/// `/system/api/htmlcache/upload` 兼容别名，两者行为一致）。
///
/// 上传 SSH 私钥证书，用于 SSH 方式推送静态缓存（Go：`PluginHtmlCacheUploadFile`）。
/// Go 版在写盘之后还会把私钥文件名回写进 `html_cache` 设置并记一条后台操作日志，
/// 这里保持同样口径。
async fn htmlcache_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut multipart: Multipart,
) -> Response {
    let current = match require_admin(&state, &headers).await {
        Ok(found) => found,
        Err(res) => return *res,
    };

    let mut bytes: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => return failed(err.into()),
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" || name == "file1" {
            match field.bytes().await {
                Ok(value) => bytes = Some(value.to_vec()),
                Err(err) => return failed(err.into()),
            }
        } else {
            let _ = field.bytes().await;
        }
    }

    let Some(bytes) = bytes else {
        return fail("请选择要上传的文件");
    };
    if bytes.is_empty() {
        return fail("文件内容为空");
    }

    let cert_dir = state.with_config(|config| config.data_dir()).join("cert");
    if let Err(err) = tokio::fs::create_dir_all(&cert_dir).await {
        return failed(err.into());
    }
    let file_name = "htmlcache_ssh_private_key.key";
    if let Err(err) = tokio::fs::write(cert_dir.join(file_name), &bytes).await {
        return failed(err.into());
    }

    // 回写 `html_cache.ssh_private_key`（Go 版 `SaveSettingValue(HtmlCacheSettingKey, ...)`），
    // 并记一条后台操作日志；两者失败都不影响上传结果。
    if let Some(pool) = state.db() {
        if let Err(err) =
            site_setting::save_section(pool, plugin::HTML_CACHE, &json!({ "ssh_private_key": file_name }))
                .await
        {
            tracing::warn!("回写 html_cache.ssh_private_key 失败：{err:#}");
        }
        if let Err(err) = admin::record_action(
            pool,
            current.id,
            &current.user_name,
            &client_ip_of(&headers, peer),
            &format!("上传SSH证书文件：{file_name}"),
        )
        .await
        {
            tracing::warn!("写后台操作日志失败：{err:#}");
        }
    }

    ok_data_msg(json!(file_name), "文件已上传完成")
}

/// 取客户端 IP：优先反向代理头，其次 TCP 对端地址（与 `routes.rs` 的 `client_ip` 同口径）。
fn client_ip_of(headers: &HeaderMap, peer: SocketAddr) -> String {
    let header_ip = headers
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .or_else(|| {
            headers
                .get("x-forwarded-for")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(',').next())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match header_ip {
        Some(ip) => ip.to_string(),
        None => peer.ip().to_string(),
    }
}

/// `POST /system/api/setting/convert/webp`
///
/// 批量把附件图片转换成 WebP（对齐 Go：`ConvertImageToWebp`）。
/// Go 版同样依赖外置 `cwebp` 命令；本项目的内置图片引擎只支持 PNG/BMP 解码、
/// 不具备 WebP 编码能力，因此这里复用系统 `cwebp`，不引入新依赖。
async fn setting_convert_webp(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    if find_cwebp().is_none() {
        return fail(
            "未找到 cwebp：内置图片引擎不支持 WebP 编码。请安装 WebP 命令行工具\
             （https://developers.google.com/speed/webp/download），把 cwebp(.exe) 放到程序目录\
             或加入 PATH 后重试；也可改在前台/对象存储侧转换。",
        );
    }
    let public_dir = state.with_config(|config| config.public_dir());
    let pool = pool.clone();
    tokio::spawn(async move {
        if let Err(err) = convert_attachments_to_webp(&pool, &public_dir).await {
            tracing::warn!("批量转换 WebP 失败：{err:#}");
        }
    });
    ok_msg("WebP 转换任务已提交至后台运行")
}

/// 找 `cwebp`：优先程序目录，其次 PATH。
fn find_cwebp() -> Option<PathBuf> {
    let names = ["cwebp.exe", "cwebp"];
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in names {
                candidates.push(dir.join(name));
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in names {
                candidates.push(dir.join(name));
            }
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// 遍历附件表，把 jpg/png/gif/bmp 转成同名 `.webp` 并同步替换引用。
async fn convert_attachments_to_webp(
    pool: &sqlx::MySqlPool,
    public_dir: &Path,
) -> anyhow::Result<()> {
    let Some(cwebp) = find_cwebp() else {
        return Ok(());
    };
    let rows = sqlx::query(
        "SELECT id, file_location FROM attachments \
         WHERE is_image = 1 AND file_location <> '' AND is_remote = 0",
    )
    .fetch_all(pool)
    .await?;

    for row in rows {
        let id: u32 = sqlx::Row::try_get(&row, "id")?;
        let location: String = sqlx::Row::try_get(&row, "file_location")?;
        let ext = Path::new(&location)
            .extension()
            .map(|value| value.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "bmp") {
            continue;
        }
        let relative = location.trim_start_matches('/');
        let source = public_dir.join(relative);
        if !source.is_file() {
            continue;
        }
        let target_relative = format!(
            "{}.webp",
            relative
                .rsplit_once('.')
                .map(|(stem, _)| stem)
                .unwrap_or(relative)
        );
        let target = public_dir.join(&target_relative);
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let output = std::process::Command::new(&cwebp)
            .arg("-q")
            .arg("90")
            .arg(&source)
            .arg("-o")
            .arg(&target)
            .output();
        if !matches!(&output, Ok(out) if out.status.success()) {
            tracing::warn!("cwebp 转换失败：{}", source.display());
            continue;
        }
        let Ok(bytes) = std::fs::read(&target) else {
            continue;
        };
        let (width, height) = attachment::image_size(&bytes);
        let md5 = attachment::md5_hex(&bytes);
        let size = bytes.len() as u64;
        if let Err(err) = attachment::update(
            pool,
            id,
            None,
            Some(&target_relative),
            Some(size),
            Some(&md5),
            Some(width),
            Some(height),
            None,
        )
        .await
        {
            tracing::warn!("更新附件记录失败（{id}）：{err}");
            continue;
        }
        replace_attachment_refs(pool, &location, &target_relative).await;
    }
    Ok(())
}

/// 把各表里对旧图片路径的引用替换为新路径。
async fn replace_attachment_refs(pool: &sqlx::MySqlPool, old: &str, new: &str) {
    let targets: [(&str, &[&str]); 5] = [
        ("archives", &["logo", "images"]),
        ("archive_data", &["content"]),
        ("categories", &["logo", "images", "content"]),
        ("comments", &["content"]),
        ("materials", &["content"]),
    ];
    for (table, columns) in targets {
        for column in columns {
            let sql = format!(
                "UPDATE {table} SET {column} = REPLACE({column}, ?, ?) WHERE {column} LIKE ?"
            );
            if let Err(err) = sqlx::query(&sql)
                .bind(old)
                .bind(new)
                .bind(format!("%{old}%"))
                .execute(pool)
                .await
            {
                tracing::warn!("替换 {table}.{column} 引用失败：{err}");
            }
        }
    }
}

/// 站点前台地址（`settings.system` 的 `front_url` 优先，回退 `base_url`）。
async fn site_front_url(pool: &sqlx::MySqlPool) -> String {
    let system = site_setting::get_section(pool, site_setting::SYSTEM)
        .await
        .unwrap_or_else(|_| json!({}));
    system
        .get("front_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| system.get("base_url").and_then(Value::as_str))
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_body_defaults() {
        let body: PasswordChooseBody = serde_json::from_str("{}").unwrap();
        assert!(body.way.is_empty());
        let body: PasswordResetBody = serde_json::from_str(r#"{"user_name":"admin"}"#).unwrap();
        assert_eq!(body.user_name, "admin");
        assert!(body.password.is_empty());
    }

    #[test]
    fn archive_bodies_default() {
        let body: ArchiveImageDeleteBody = serde_json::from_str("{}").unwrap();
        assert_eq!(body.id, 0);
        assert_eq!(body.image_index, 0);

        let body: ArchivePlanBody = serde_json::from_str(r#"{"ids":[1,2],"daily_limit":5}"#).unwrap();
        assert_eq!(body.ids.len(), 2);
        assert_eq!(body.daily_limit, 5);

        let body: ArchiveTagsBody =
            serde_json::from_str(r#"{"ids":[1],"tags":["a","b"]}"#).unwrap();
        assert_eq!(body.tags.len(), 2);
    }

    #[test]
    fn anchor_body_parses_type_alias() {
        let body: AnchorFromTitleBody =
            serde_json::from_str(r#"{"type":"category","ids":[3]}"#).unwrap();
        assert_eq!(body.kind, "category");
        assert_eq!(body.ids, vec![3]);
    }

    #[test]
    fn attachment_body_parses_urls() {
        let body: AttachmentAddUrlBody =
            serde_json::from_str(r#"{"category_id":2,"urls":["https://a.com/1.png"]}"#).unwrap();
        assert_eq!(body.category_id, 2);
        assert_eq!(body.urls.len(), 1);
    }

    #[test]
    fn router_builds() {
        let _router: Router<AppState> = router();
    }
}

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
//! 未实现项按项目既有口径「接口保留 + 参数校验 + 明确未实现提示」处理：
//! - `archive/import`（zip / xlsx 解析）
//! - `setting/convert/webp`（图片转码）
//! - `password/verify` 的 DNS 方式（TXT 解析需要 DNS 客户端）

use axum::extract::{ConnectInfo, Multipart, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;

use crate::models::{admin, admin_gap, anchor, archive_admin, category, module, plugin, site_setting};
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
/// - DNS 方式：需要 TXT 解析，Rust 侧暂未接入，明确提示改用文件方式。
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
        return fail("DNS 解析验证尚未实现，请改用文件验证方式");
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
/// 上传 zip / xlsx 批量导入文档（Go：`QuickImportArchive`）。
/// 文件解析依赖 zip / xlsx 解析组件，Rust 侧暂未接入：
/// 这里保留接口、校验参数，并返回明确的未实现提示。
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
    let mut has_file = false;

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
                    Ok(bytes) => {
                        file_size = bytes.len() as i64;
                        has_file = true;
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
    if category_id == 0 {
        return fail("请选择导入的分类");
    }
    match category::get(pool, category_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return fail("分类不存在"),
        Err(err) => return failed(err),
    }

    fail("批量导入文档需要 zip/xlsx 解析组件，尚未实现")
}

/// `GET /system/api/archive/import/status`
///
/// 查询批量导入任务进度（Go：`GetQuickImportArchiveStatus`）。
/// 导入本身未实现，这里恒为「没有任务」（`data` 为 `null`，与 Go 版一致）。
async fn archive_import_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    ok(Value::Null)
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
/// 批量把图片转换成 WebP（Go：`ConvertImageToWebp`）。
/// 图片转码需要图片处理组件，Rust 侧暂未接入：保留接口并返回明确提示。
async fn setting_convert_webp(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(res) = require_admin(&state, &headers).await {
        return *res;
    }
    let Some(pool) = state.db() else {
        return fail(NO_DB);
    };
    let _ = pool;
    fail("批量转换图片为 WebP 需要图片处理组件，尚未实现")
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

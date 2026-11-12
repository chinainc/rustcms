//! 无配置安装模式。
//!
//! 没有 `config.json` 或 `mysql.database` 为空时，主程序不再直接退出，
//! 而是监听端口并只提供 `/install`：
//! 填数据库信息 → 测试连接 →（空库时）导入 `deploy/sql/rustcms-install.sql`
//! → 写入 `config.json` → 安装模式服务优雅关闭，由 main 自动进入正式服务。

use std::path::PathBuf;

use anyhow::{Context, Result};
use axum::extract::{Extension, Form, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use tokio::sync::watch;

use crate::config::{default_config_path, MysqlConfig, ServerConfig, ServerJson};
use crate::state::AppState;

/// 安装表单字段。
#[derive(Debug, Deserialize)]
struct InstallForm {
    #[serde(default)]
    host: String,
    #[serde(default = "default_db_port")]
    db_port: u16,
    #[serde(default)]
    database: String,
    #[serde(default)]
    user: String,
    #[serde(default)]
    password: String,
    #[serde(default = "default_server_port")]
    port: u16,
}

fn default_db_port() -> u16 {
    3306
}

fn default_server_port() -> u16 {
    9001
}

fn default_form() -> InstallForm {
    InstallForm {
        host: "127.0.0.1".to_string(),
        db_port: default_db_port(),
        database: "rustcms".to_string(),
        user: "root".to_string(),
        password: String::new(),
        port: default_server_port(),
    }
}

/// 安装模式入口：只暴露 `/install`，其它路径一律跳过去。
///
/// 安装提交成功后发送关闭信号，`run` 返回，由 main 重新加载配置进入正式服务。
pub async fn run(state: AppState) -> Result<()> {
    let port = state.with_config(|cfg| cfg.port());
    let port = if port == 0 { default_server_port() } else { port };
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let app = Router::new()
        .route("/install", get(page).post(submit))
        .fallback(|| async { Redirect::to("/install") })
        .layer(Extension(shutdown_tx))
        .with_state(state);
    let listener = crate::server::bind_with_hint(port).await?;
    eprintln!("[!] 未配置数据库，已进入安装模式：http://127.0.0.1:{port}/install");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.changed().await;
        })
        .await
        .context("安装模式服务异常退出")?;
    Ok(())
}

async fn page(State(state): State<AppState>) -> Html<String> {
    let port = state.with_config(|cfg| cfg.port());
    let mut form = default_form();
    form.port = if port == 0 { default_server_port() } else { port };
    Html(render_form(Some(&form), None))
}

async fn submit(
    State(state): State<AppState>,
    Extension(shutdown): Extension<watch::Sender<bool>>,
    Form(form): Form<InstallForm>,
) -> Response {
    match install(&state, &form).await {
        Ok(message) => {
            // 安装成功：通知安装模式服务优雅关闭，main 随后自动进入正式服务。
            let _ = shutdown.send(true);
            Html(render_done(&message, form.port)).into_response()
        }
        Err(err) => {
            let text = format!("{err:#}");
            (
                StatusCode::OK,
                Html(render_form(Some(&form), Some(&text))),
            )
                .into_response()
        }
    }
}

/// 测试连接 → 空库导表 → 写 `config.json`。
async fn install(state: &AppState, form: &InstallForm) -> Result<String> {
    let host = form.host.trim();
    let database = form.database.trim();
    if host.is_empty() || database.is_empty() {
        anyhow::bail!("请填写数据库地址和数据库名");
    }
    if form.port == 0 {
        anyhow::bail!("本站监听端口不能为 0");
    }

    let mysql = MysqlConfig {
        database: database.to_string(),
        user: form.user.trim().to_string(),
        password: form.password.clone(),
        host: host.to_string(),
        port: form.db_port,
        use_default: false,
    };
    let pool = crate::db::connect(&mysql)
        .await
        .map_err(|err| anyhow::anyhow!("连接数据库失败（请先在宝塔/MySQL 创建数据库 {database}）：{err:#}"))?
        .context("数据库配置不完整")?;

    let tables: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = ?")
            .bind(database)
            .fetch_one(&pool)
            .await
            .map_err(|err| anyhow::anyhow!("检查数据库表失败：{err:#}"))?;

    let imported = if tables == 0 {
        let sql = load_install_sql()?;
        sqlx::raw_sql(&sql)
            .execute(&pool)
            .await
            .map_err(|err| anyhow::anyhow!("导入表结构失败：{err:#}"))?;
        true
    } else {
        false
    };

    let path = state
        .with_config(|cfg| cfg.path().map(|path| path.to_path_buf()))
        .unwrap_or_else(default_config_path);
    let config = ServerJson {
        mysql,
        server: ServerConfig {
            env: "production".to_string(),
            port: form.port,
            log_level: "error".to_string(),
        },
        api_token: Some(String::new()),
        template_dir: "./template/default".to_string(),
        public_dir: "./public".to_string(),
        path: Some(path.clone()),
    };
    config
        .save()
        .with_context(|| format!("写入配置文件失败：{}", path.display()))?;

    Ok(format!(
        "安装完成，配置已写入 {}。{}",
        path.display(),
        if imported {
            "已导入表结构。"
        } else {
            "数据库已有表，已跳过导入。"
        }
    ))
}

/// 依次在程序目录 / 当前目录及其上级目录查找安装 SQL。
fn load_install_sql() -> Result<String> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    for root in roots {
        let mut current = Some(root.as_path());
        for _ in 0..4 {
            let Some(dir) = current else { break };
            for name in ["deploy/sql/rustcms-install.sql", "rustcms-install.sql"] {
                let path = dir.join(name);
                if path.is_file() {
                    return std::fs::read_to_string(&path)
                        .with_context(|| format!("读取安装 SQL 失败：{}", path.display()));
                }
            }
            current = dir.parent();
        }
    }
    anyhow::bail!("安装包内未找到 deploy/sql/rustcms-install.sql，请先导入数据库表结构")
}

fn esc(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const PAGE: &str = r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>RustCMS 安装</title>
<style>
body{margin:0;background:#f5f6f8;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,"Helvetica Neue",Arial,"Microsoft YaHei",sans-serif;color:#333}
.wrap{max-width:640px;margin:56px auto;padding:0 16px}
.card{background:#fff;border:1px solid #e5e6eb;border-radius:8px;padding:32px}
h1{margin:0 0 8px;font-size:22px}
.sub{color:#86909c;font-size:13px;margin:0 0 20px}
label{display:block;font-size:13px;margin:14px 0 6px}
input{width:100%;box-sizing:border-box;padding:9px 12px;border:1px solid #d9dce2;border-radius:6px;font-size:14px}
.row{display:flex;gap:12px}.row>div{flex:1}
button{margin-top:24px;width:100%;padding:11px;border:0;border-radius:6px;background:#1677ff;color:#fff;font-size:15px;cursor:pointer}
.err{background:#fff2f0;border:1px solid #ffccc7;color:#cf1322;padding:10px 12px;border-radius:6px;font-size:13px;margin-bottom:10px;white-space:pre-wrap}
.ok{background:#f6ffed;border:1px solid #b7eb8f;color:#389e0d;padding:12px;border-radius:6px;font-size:14px;white-space:pre-wrap;line-height:1.7}
.tip{color:#86909c;font-size:12px;margin-top:14px;line-height:1.7}
</style>
</head>
<body>
<div class="wrap">
  <div class="card">
    <h1>RustCMS 安装</h1>
    <p class="sub">首次运行需要配置数据库，完成后会写入 config.json，重启程序即可使用。</p>
    {{ERROR}}
    <form method="post" action="/install">
      <label>数据库地址</label>
      <input name="host" value="{{HOST}}" placeholder="127.0.0.1">
      <div class="row">
        <div><label>数据库端口</label><input name="db_port" value="{{DBPORT}}"></div>
        <div><label>数据库名</label><input name="database" value="{{DATABASE}}" placeholder="rustcms"></div>
      </div>
      <div class="row">
        <div><label>数据库用户名</label><input name="user" value="{{USER}}" placeholder="root"></div>
        <div><label>数据库密码</label><input name="password" type="password" value="{{PASSWORD}}"></div>
      </div>
      <label>本站监听端口</label>
      <input name="port" value="{{PORT}}">
      <button type="submit">开始安装</button>
    </form>
    <p class="tip">需要先在宝塔 / MySQL 里创建好数据库（utf8mb4）。空库会自动导入安装包内的 deploy/sql/rustcms-install.sql；已有表则跳过导入。</p>
  </div>
</div>
</body>
</html>"#;

fn render_form(form: Option<&InstallForm>, error: Option<&str>) -> String {
    let default = default_form();
    let form = form.unwrap_or(&default);
    let error = error
        .map(|text| format!("<div class=\"err\">{}</div>", esc(text)))
        .unwrap_or_default();
    PAGE.replace("{{ERROR}}", &error)
        .replace("{{HOST}}", &esc(&form.host))
        .replace("{{DBPORT}}", &form.db_port.to_string())
        .replace("{{DATABASE}}", &esc(&form.database))
        .replace("{{USER}}", &esc(&form.user))
        .replace("{{PASSWORD}}", &esc(&form.password))
        .replace("{{PORT}}", &form.port.to_string())
}

const DONE: &str = r#"<!DOCTYPE html>
<html lang="zh-CN">
<head><meta charset="utf-8"><title>RustCMS 安装完成</title>
<style>
body{margin:0;background:#f5f6f8;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,"Helvetica Neue",Arial,"Microsoft YaHei",sans-serif;color:#333}
.wrap{max-width:640px;margin:56px auto;padding:0 16px}
.card{background:#fff;border:1px solid #e5e6eb;border-radius:8px;padding:32px}
h1{margin:0 0 12px;font-size:22px}
.ok{background:#f6ffed;border:1px solid #b7eb8f;color:#389e0d;padding:12px;border-radius:6px;font-size:14px;white-space:pre-wrap;line-height:1.7}
.tip{color:#86909c;font-size:13px;margin-top:16px;line-height:1.8}
</style></head>
<body><div class="wrap"><div class="card">
<h1>安装完成</h1>
<div class="ok">{{MSG}}</div>
<p class="tip">本进程将自动切换到正式服务（无需手动重启），稍候即可访问 <b>http://服务器IP:{{PORT}}/</b>（后台 /system/）。</p>
</div></div></body></html>"#;

fn render_done(message: &str, port: u16) -> String {
    DONE
        .replace("{{MSG}}", &esc(message))
        .replace("{{PORT}}", &port.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    fn app() -> Router {
        let state = AppState::new(ServerJson::default(), None);
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        Router::new()
            .route("/install", get(page).post(submit))
            .fallback(|| async { Redirect::to("/install") })
            .layer(Extension(shutdown_tx))
            .with_state(state)
    }

    async fn body_text(res: Response) -> String {
        let bytes = to_bytes(res.into_body(), 1024 * 1024).await.unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    }

    #[tokio::test]
    async fn install_page_renders_form() {
        let res = app()
            .oneshot(Request::builder().uri("/install").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let html = body_text(res).await;
        assert!(html.contains("RustCMS 安装"), "{html}");
        assert!(html.contains("name=\"database\""), "{html}");
    }

    #[tokio::test]
    async fn other_paths_redirect_to_install() {
        let res = app()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(res.headers().get("location").unwrap(), "/install");
    }

    #[tokio::test]
    async fn empty_form_reports_error_without_db() {
        let res = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/install")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("host=&database="))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let html = body_text(res).await;
        assert!(html.contains("请填写数据库地址和数据库名"), "{html}");
    }

    #[test]
    fn finds_packaged_install_sql() {
        let sql = load_install_sql().unwrap();
        assert!(sql.contains("CREATE TABLE"), "安装 SQL 内容异常");
    }

    #[test]
    fn escapes_form_values() {
        let form = InstallForm {
            host: "<x>".to_string(),
            ..default_form()
        };
        let html = render_form(Some(&form), Some("<err>"));
        assert!(html.contains("&lt;x&gt;"));
        assert!(html.contains("&lt;err&gt;"));
    }
}
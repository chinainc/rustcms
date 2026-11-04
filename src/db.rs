//! MySQL 连接池（sqlx）。

use std::time::Duration;

use anyhow::{Context, Result};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use sqlx::mysql::{MySqlPool, MySqlPoolOptions};

use crate::config::MysqlConfig;

/// URL 的 userinfo 段需要转义的字符集。
const USERINFO: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'\'')
    .add(b'/')
    .add(b':')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// 拼接 sqlx 的 MySQL 连接串。
pub fn url(cfg: &MysqlConfig) -> String {
    let user = utf8_percent_encode(&cfg.user, USERINFO);
    let password = utf8_percent_encode(&cfg.password, USERINFO);
    format!(
        "mysql://{}:{}@{}:{}/{}",
        user, password, cfg.host, cfg.port, cfg.database
    )
}

/// 建立连接池；未配置数据库时返回 `Ok(None)`。
///
/// 只要配置了数据库就要求连通，连不上直接失败（不带病启动）。
pub async fn connect(cfg: &MysqlConfig) -> Result<Option<MySqlPool>> {
    if !cfg.is_configured() {
        return Ok(None);
    }

    let pool = MySqlPoolOptions::new()
        .max_connections(20)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&url(cfg))
        .await
        .with_context(|| {
            format!(
                "连接 MySQL 失败：{}@{}:{}/{}",
                cfg.user, cfg.host, cfg.port, cfg.database
            )
        })?;

    sqlx::query("SELECT 1")
        .execute(&pool)
        .await
        .context("MySQL 连通性检查失败")?;

    Ok(Some(pool))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(password: &str) -> MysqlConfig {
        MysqlConfig {
            database: "cms".into(),
            user: "root".into(),
            password: password.into(),
            host: "127.0.0.1".into(),
            port: 3307,
            use_default: false,
        }
    }

    #[test]
    fn url_escapes_credentials() {
        assert_eq!(
            url(&cfg("p@ss:w/rd")),
            "mysql://root:p%40ss%3Aw%2Frd@127.0.0.1:3307/cms"
        );
    }

    #[tokio::test]
    async fn connect_skips_when_not_configured() {
        let pool = connect(&MysqlConfig::default()).await.unwrap();
        assert!(pool.is_none());
    }
}

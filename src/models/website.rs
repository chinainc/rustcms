//! 多站点（表 `websites`）。
//!
//! Go 版每个站点有**独立的 MySQL 连接**（`websites.mysql` 存 JSON 连接串），
//! Rust 侧目前是「单连接池」架构，因此这里只实现站点记录的读取与维护，
//! 真正的多站点数据隔离（切换连接池）尚未实现。
//!
//! 对齐 Go 版 `controller/manageController/website.go` 中的
//! `GetWebsiteList` / `GetWebsiteInfo` / `DeleteWebsite` / `GetCurrentSiteInfo`。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::mysql::{MySqlPool, MySqlPoolOptions};
use sqlx::FromRow;

use super::{site_setting, ModelError};

/// 站点记录。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Website {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub root_path: String,
    pub name: String,
    /// JSON 形式的 MySQL 连接串（Go 版用它建独立连接池）
    pub mysql: Option<String>,
    pub token_secret: String,
    pub status: u8,
    pub parent_id: i32,
    pub sync_time: Option<i64>,
    pub language_icon: String,
}

/// 保存入参。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WebsiteInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub root_path: String,
    #[serde(default)]
    pub status: Option<u8>,
    #[serde(default)]
    pub parent_id: Option<i32>,
    /// 直接给 JSON 字符串；不传则沿用原值
    #[serde(default)]
    pub mysql: Option<serde_json::Value>,
}

/// 分页查站点，返回 `(总数, 本页记录)`。
///
/// 注意：`websites` 表**没有 `base_url` 列**（域名存在各站点自己的
/// `settings.system` 里），所以 `base_url` 过滤参数暂不生效。
pub async fn list(
    pool: &MySqlPool,
    name: &str,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Website>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 { 20 } else { page_size.min(200) };
    let offset = (page - 1) * page_size;
    let keyword = name.trim();

    let (total, rows) = if keyword.is_empty() {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM websites")
            .fetch_one(pool)
            .await?;
        let rows = sqlx::query_as::<_, Website>(
            "SELECT * FROM websites ORDER BY id ASC LIMIT ? OFFSET ?",
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        (total, rows)
    } else {
        let like = format!("%{keyword}%");
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM websites WHERE name LIKE ?")
            .bind(&like)
            .fetch_one(pool)
            .await?;
        let rows = sqlx::query_as::<_, Website>(
            "SELECT * FROM websites WHERE name LIKE ? ORDER BY id ASC LIMIT ? OFFSET ?",
        )
        .bind(&like)
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        (total, rows)
    };

    Ok((total, rows))
}

/// 按 ID 取站点。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Website>> {
    Ok(
        sqlx::query_as::<_, Website>("SELECT * FROM websites WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 新建 / 更新站点记录。
pub async fn save(pool: &MySqlPool, input: &WebsiteInput) -> Result<Website, ModelError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(ModelError::invalid("请填写站点名称"));
    }
    let now = chrono::Utc::now().timestamp();
    let mysql_json = match input.mysql.as_ref() {
        Some(value) if value.is_object() => Some(serde_json::to_string(value)?),
        Some(value) if value.is_string() => Some(value.as_str().unwrap_or_default().to_string()),
        _ => None,
    };

    if input.id > 0 && get(pool, input.id).await?.is_some() {
        sqlx::query(
            "UPDATE websites SET updated_time = ?, name = ?, root_path = COALESCE(?, root_path), \
                status = COALESCE(?, status), parent_id = COALESCE(?, parent_id), \
                mysql = COALESCE(?, mysql) WHERE id = ?",
        )
        .bind(now)
        .bind(name)
        .bind(if input.root_path.trim().is_empty() {
            None
        } else {
            Some(input.root_path.trim().to_string())
        })
        .bind(input.status)
        .bind(input.parent_id)
        .bind(mysql_json)
        .bind(input.id)
        .execute(pool)
        .await?;
        return get(pool, input.id)
            .await?
            .ok_or_else(|| ModelError::invalid("站点不存在"));
    }

    // 新增：token_secret 复用 32 位随机串
    let secret = crate::auth::generate_secret(32);
    let result = sqlx::query(
        "INSERT INTO websites (created_time, updated_time, root_path, name, mysql, token_secret, \
            status, parent_id, sync_time, language_icon) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, '')",
    )
    .bind(now)
    .bind(now)
    .bind(input.root_path.trim())
    .bind(name)
    .bind(mysql_json)
    .bind(&secret)
    .bind(input.status.unwrap_or(1))
    .bind(input.parent_id.unwrap_or(0))
    .execute(pool)
    .await?;
    let id = result.last_insert_id() as u32;
    get(pool, id)
        .await?
        .ok_or_else(|| ModelError::invalid("站点创建失败"))
}

/// 删除站点记录（默认站点 1 不允许删除）。
pub async fn delete(pool: &MySqlPool, id: u32) -> Result<(), ModelError> {
    if id == 1 {
        return Err(ModelError::invalid("默认站点不能删除"));
    }
    if get(pool, id).await?.is_none() {
        return Err(ModelError::invalid("站点不存在"));
    }
    sqlx::query("DELETE FROM websites WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 子站点切换登录（对齐 Go `LoginSubWebsite` + `provider.SignAdminSSO`）
// ---------------------------------------------------------------------------

/// SSO 票据有效期（秒），与 Go 版 `provider.AdminSSOTTL` 一致。
const ADMIN_SSO_TTL_SECS: i64 = 120;

/// 生成一次性票据：`<过期时间戳>:<16 字节随机十六进制>`。
pub fn mint_admin_sso_nonce(ttl_secs: i64) -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..16).map(|_| rng.gen::<u8>()).collect();
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{}:{}", chrono::Utc::now().timestamp() + ttl_secs, hex)
}

/// 用目标站点密钥签发登录签名：`HMAC-SHA256(secret, "admin-sso|user_name|nonce")`。
pub fn sign_admin_sso(token_secret: &str, user_name: &str, nonce: &str) -> String {
    let message = format!("admin-sso|{user_name}|{nonce}");
    hmac_sha256_hex(token_secret.as_bytes(), message.as_bytes())
}

/// 登录链接里的查询参数转义（对齐 Go 的 `url.QueryEscape` 语义）。
fn query_escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(byte as char)
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

/// 去掉登录地址里的目录部分（Go 版：`path` 长度大于 1 时清空 path）。
fn clear_url_path(raw: &str) -> String {
    let mut value = raw.trim().to_string();
    if let Some(hash) = value.find('#') {
        value.truncate(hash);
    }
    if let Some(query) = value.find('?') {
        value.truncate(query);
    }
    if let Some(scheme) = value.find("://") {
        let authority_start = scheme + 3;
        if let Some(slash) = value[authority_start..].find('/') {
            let path = &value[authority_start + slash..];
            if path.len() > 1 {
                value.truncate(authority_start + slash);
            }
        }
    }
    value
}

/// 把子站点的 `websites.mysql` JSON 转成连接配置；`use_default` 时复用默认站点账号密码。
fn sub_site_mysql_config(
    raw: Option<&str>,
    default: &crate::config::MysqlConfig,
) -> Result<crate::config::MysqlConfig, ModelError> {
    let mut config: crate::config::MysqlConfig = match raw {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(text)
            .map_err(|err| ModelError::invalid(format!("站点数据库配置格式错误：{err}")))?,
        _ => crate::config::MysqlConfig::default(),
    };
    if config.use_default {
        config.user = default.user.clone();
        config.password = default.password.clone();
        config.host = default.host.clone();
        config.port = default.port;
    }
    Ok(config)
}

/// 从站点所在数据库读取登录所需信息：`(base_url, admin_url, 首个管理员账号及状态)`。
async fn read_site_login_meta(
    pool: &MySqlPool,
) -> Result<(String, String, Option<(String, u8)>), ModelError> {
    let system = site_setting::get_section(pool, site_setting::SYSTEM).await?;
    let base_url = system
        .get("base_url")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let admin_url = system
        .get("admin_url")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();

    let admin: Option<(String, u8)> = sqlx::query_as(
        "SELECT user_name, status FROM admins ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok((base_url, admin_url, admin))
}

/// 生成以子站点身份登录后台的一次性链接（Go：`LoginSubWebsite`）。
pub async fn login_sub_site(
    pool: &MySqlPool,
    config: &crate::config::ServerJson,
    site_id: u32,
) -> Result<String, ModelError> {
    if site_id == 0 {
        return Err(ModelError::invalid("站点不存在"));
    }
    let site = get(pool, site_id)
        .await?
        .ok_or_else(|| ModelError::invalid("站点不存在"))?;

    let (base_url, admin_url, admin) = if site_id == 1 {
        read_site_login_meta(pool).await?
    } else {
        let mysql = sub_site_mysql_config(site.mysql.as_deref(), &config.mysql)?;
        if !mysql.is_configured() {
            return Err(ModelError::invalid("站点未配置数据库连接"));
        }
        let sub_pool = MySqlPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&crate::db::url(&mysql))
            .await
            .map_err(|err| ModelError::invalid(format!("站点数据库连接失败：{err}")))?;
        let meta = read_site_login_meta(&sub_pool).await;
        sub_pool.close().await;
        meta?
    };

    let Some((user_name, status)) = admin else {
        return Err(ModelError::invalid("用户不存在"));
    };
    if status != 1 {
        return Err(ModelError::invalid("权限不足"));
    }

    let nonce = mint_admin_sso_nonce(ADMIN_SSO_TTL_SECS);
    let sign = sign_admin_sso(&site.token_secret, &user_name, &nonce);

    let login_url = if admin_url.is_empty() {
        base_url
    } else {
        admin_url
    };
    let login_url = clear_url_path(&login_url);

    Ok(format!(
        "{login_url}/system/login?admin-login=true&site_id={}&user_name={}&sign={}&nonce={}",
        site.id,
        query_escape(&user_name),
        sign,
        query_escape(&nonce)
    ))
}

// ---------------------------------------------------------------------------
// HMAC-SHA256（Go 版用 HMAC-SHA256 签发 SSO，签名必须与标准实现逐字节一致）
// ---------------------------------------------------------------------------

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

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
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
        for (index, word) in w.iter_mut().take(16).enumerate() {
            let offset = index * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
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

fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    const BLOCK: usize = 64;
    let mut block_key = if key.len() > BLOCK {
        sha256(key).to_vec()
    } else {
        key.to_vec()
    };
    block_key.resize(BLOCK, 0);
    let mut inner_pad = vec![0x36u8; BLOCK];
    let mut outer_pad = vec![0x5cu8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] ^= block_key[index];
        outer_pad[index] ^= block_key[index];
    }

    let mut inner = inner_pad;
    inner.extend_from_slice(message);
    let inner_hash = sha256(&inner);

    let mut outer = outer_pad;
    outer.extend_from_slice(&inner_hash);
    sha256(&outer)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[tokio::test]
    async fn reads_default_site() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let (total, rows) = list(&pool, "", 1, 20).await.unwrap();
        assert!(total >= 1, "至少应有默认站点");
        assert!(rows.iter().any(|item| item.id == 1));

        let site = get(&pool, 1).await.unwrap().expect("默认站点应存在");
        assert!(!site.token_secret.is_empty());

        // 名称搜索
        let keyword = site.name.chars().take(2).collect::<String>();
        let (found, _) = list(&pool, &keyword, 1, 20).await.unwrap();
        assert!(found >= 1);

        assert!(get(&pool, u32::MAX).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn refuses_to_delete_default_site() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let err = delete(&pool, 1).await.unwrap_err();
        assert_eq!(err.to_string(), "默认站点不能删除");
        assert!(get(&pool, 1).await.unwrap().is_some());
    }

    #[test]
    fn sha256_matches_standard_vectors() {
        let empty: String = sha256(b"")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(
            empty,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        let abc: String = sha256(b"abc")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(
            abc,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hmac_sha256_matches_rfc4231() {
        // RFC 4231 Test Case 1
        let key = vec![0x0bu8; 20];
        assert_eq!(
            hmac_sha256_hex(&key, b"Hi There"),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // RFC 4231 Test Case 2
        assert_eq!(
            hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn builds_sso_nonce_and_signature() {
        let nonce = mint_admin_sso_nonce(ADMIN_SSO_TTL_SECS);
        let (expire, random) = nonce.split_once(':').expect("票据格式应为 过期时间:随机串");
        assert!(expire.parse::<i64>().unwrap() > chrono::Utc::now().timestamp());
        assert_eq!(random.len(), 32, "16 字节随机串应为 32 位十六进制");

        let sign = sign_admin_sso("secret", "admin", "1700000000:abcd");
        assert_eq!(sign.len(), 64, "HMAC-SHA256 十六进制输出应为 64 位");
        assert_eq!(sign, sign.to_ascii_lowercase());
    }

    #[test]
    fn clears_admin_login_directory() {
        assert_eq!(clear_url_path("https://a.com"), "https://a.com");
        assert_eq!(clear_url_path("https://a.com/"), "https://a.com/");
        assert_eq!(clear_url_path("https://a.com/admin"), "https://a.com");
        assert_eq!(
            clear_url_path("https://a.com/admin/index?x=1"),
            "https://a.com"
        );
    }

    #[test]
    fn sub_site_mysql_can_reuse_default_account() {
        let default = crate::config::MysqlConfig {
            database: "main".to_string(),
            user: "root".to_string(),
            password: "pwd".to_string(),
            host: "127.0.0.1".to_string(),
            port: 3306,
            use_default: false,
        };
        let raw = r#"{"database":"sub","use_default":true,"host":"badhost","port":1234}"#;
        let cfg = sub_site_mysql_config(Some(raw), &default).unwrap();
        assert_eq!(cfg.database, "sub");
        assert_eq!(cfg.user, "root");
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.port, 3306);
        assert!(cfg.is_configured());
    }
}

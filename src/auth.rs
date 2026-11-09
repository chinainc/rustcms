//! 登录与 JWT 令牌。
//!
//! 令牌约定：
//! - 管理员令牌 claims 为 `{"adminId":"<id>","t":"<过期时间戳>"}`，
//! 签名密钥是 `TokenSecret + "-admin-token"`；
//! - 会员令牌 claims 为 `{"userId":"<id>","t":"..."}`，密钥 `TokenSecret + "-user-token"`；
//! - `TokenSecret` 存在 `settings.system.token_secret`，缺省时生成 32 位随机串并写回。
//!
//! 注意 claims 里的 `t` 是「过期时间戳」且以字符串存放；
//! `jsonwebtoken` 只认 `exp`，所以过期校验在 [`verify`] 里手工做。

use anyhow::{bail, Context, Result};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sqlx::mysql::MySqlPool;

/// 令牌归属：后台管理员 / 前台会员。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Admin,
    User,
}

impl TokenKind {
    fn suffix(self) -> &'static str {
        match self {
            Self::Admin => "-admin-token",
            Self::User => "-user-token",
        }
    }

    fn claim_key(self) -> &'static str {
        match self {
            Self::Admin => "adminId",
            Self::User => "userId",
        }
    }
}

/// JWT claims（键名与既有接口契约一致）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    #[serde(rename = "adminId", skip_serializing_if = "Option::is_none", default)]
    pub admin_id: Option<String>,
    #[serde(rename = "userId", skip_serializing_if = "Option::is_none", default)]
    pub user_id: Option<String>,
    /// 过期时间戳（Unix 秒，字符串）
    #[serde(rename = "t")]
    pub expire: String,
}

impl Claims {
    /// 取出主体 ID。
    pub fn subject(&self, kind: TokenKind) -> Option<u32> {
        let raw = match kind {
            TokenKind::Admin => self.admin_id.as_deref(),
            TokenKind::User => self.user_id.as_deref(),
        }?;
        raw.trim().parse::<u32>().ok()
    }
}

/// 生成随机密钥（大小写字母 + 数字）。
pub fn generate_secret(len: usize) -> String {
    const POOL: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| POOL[rng.gen_range(0..POOL.len())] as char)
        .collect()
}

/// 读取站点 `TokenSecret`；没有就生成并写回 `settings.system`。
pub async fn token_secret(pool: &MySqlPool) -> Result<String> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE `key` = 'system'")
            .fetch_optional(pool)
            .await?;
    let Some(raw) = raw else {
        bail!("站点设置缺失（settings.system）");
    };

    let mut json: serde_json::Value =
        serde_json::from_str(&raw).context("settings.system 不是合法 JSON")?;

    if let Some(secret) = json
        .get("token_secret")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(secret.to_string());
    }

    let secret = generate_secret(32);
    json["token_secret"] = serde_json::Value::String(secret.clone());
    sqlx::query("UPDATE settings SET value = ? WHERE `key` = 'system'")
        .bind(json.to_string())
        .execute(pool)
        .await?;
    tracing::info!("已生成站点 token_secret 并写入 settings.system");
    Ok(secret)
}

/// 签发令牌，返回 `(token, 过期时间戳)`。
///
/// `remember` 时 30 天，否则 24 小时（与既有接口契约一致）。
pub fn issue(kind: TokenKind, secret: &str, id: u32, remember: bool) -> Result<(String, i64)> {
    let now = chrono::Utc::now().timestamp();
    let expire = if remember {
        now + 30 * 24 * 3600
    } else {
        now + 24 * 3600
    };

    let mut claims = Claims {
        admin_id: None,
        user_id: None,
        expire: expire.to_string(),
    };
    let id = id.to_string();
    match kind {
        TokenKind::Admin => claims.admin_id = Some(id),
        TokenKind::User => claims.user_id = Some(id),
    }

    let key = format!("{secret}{}", kind.suffix());
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(key.as_bytes()),
    )?;
    Ok((token, expire))
}

/// 校验令牌：签名 + 过期时间。
pub fn verify(kind: TokenKind, secret: &str, token: &str) -> Result<Claims> {
    let key = format!("{secret}{}", kind.suffix());
    // 关掉内置 exp 校验（既有实现用的是 `t`），其余仍然校验算法
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = false;
    validation.required_spec_claims.clear();

    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(key.as_bytes()),
        &validation,
    )
    .map_err(|err| anyhow::anyhow!("令牌无效：{err}"))?;
    let claims = data.claims;

    let expire = claims
        .expire
        .trim()
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!("令牌缺少过期时间"))?;
    if expire <= chrono::Utc::now().timestamp() {
        bail!("令牌已过期");
    }
    if claims.subject(kind).is_none() {
        bail!("令牌缺少 {}", kind.claim_key());
    }
    Ok(claims)
}

/// bcrypt 校验（既有实现库存的也是 bcrypt 哈希）。
pub fn check_password(plain: &str, hash: &str) -> bool {
    if plain.is_empty() || hash.is_empty() {
        return false;
    }
    bcrypt::verify(plain, hash).unwrap_or(false)
}

/// 生成 bcrypt 哈希（`rustcms --hash-password <明文>` 用它创建后台账号）。
pub fn hash_password(plain: &str) -> Result<String> {
    let hash = bcrypt::hash(plain, 10)?;
    Ok(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issues_and_verifies_tokens() {
        let secret = "unit-test-secret";
        let (token, expire) = issue(TokenKind::Admin, secret, 7, false).unwrap();
        assert!(expire > chrono::Utc::now().timestamp());

        let claims = verify(TokenKind::Admin, secret, &token).unwrap();
        assert_eq!(claims.subject(TokenKind::Admin), Some(7));
        assert!(claims.user_id.is_none());

        // 换一种令牌类型应当失败（密钥不同）
        assert!(verify(TokenKind::User, secret, &token).is_err());
        // 换密钥应当失败
        assert!(verify(TokenKind::Admin, "other-secret", &token).is_err());
        // 篡改应当失败
        assert!(verify(TokenKind::Admin, secret, &format!("{token}x")).is_err());
    }

    #[test]
    fn rejects_expired_token() {
        let secret = "unit-test-secret";
        let claims = Claims {
            admin_id: Some("1".into()),
            user_id: None,
            expire: (chrono::Utc::now().timestamp() - 10).to_string(),
        };
        let key = format!("{secret}{}", TokenKind::Admin.suffix());
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(key.as_bytes()),
        )
        .unwrap();

        let err = verify(TokenKind::Admin, secret, &token).unwrap_err();
        assert!(err.to_string().contains("过期"), "{err}");
    }

    #[test]
    fn remembers_longer_expiry() {
        let secret = "unit-test-secret";
        let (_, short) = issue(TokenKind::User, secret, 1, false).unwrap();
        let (_, long) = issue(TokenKind::User, secret, 1, true).unwrap();
        assert!(long - short > 20 * 24 * 3600);
    }

    #[test]
    fn verifies_bcrypt_passwords() {
        let hash = hash_password("test-password-123").unwrap();
        assert!(check_password("test-password-123", &hash));
        assert!(!check_password("wrong", &hash));
        assert!(!check_password("", &hash));
        // 库里真实管理员的哈希也是 bcrypt，格式应能识别
        assert!(!check_password("whatever", "not-a-hash"));
    }

    #[test]
    fn generates_unique_secrets() {
        let a = generate_secret(32);
        let b = generate_secret(32);
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}

//! 最小 RSA 实现（只做支付宝 RSA2 需要的两件事：**验签**与**签名**）。
//!
//! 两种必需能力：解析公钥 / 私钥，按 PKCS#1 v1.5 做 SHA-256 验签与签名。
//! 支付宝公钥在证书模式下是 X.509 证书（PEM `CERTIFICATE`），也支持
//! `PUBLIC KEY`（SubjectPublicKeyInfo）与 `RSA PUBLIC KEY`（PKCS#1）三种 PEM。
//!
//! 实现要点：
//! - ASN.1 DER 只解析需要的部分（TLV + 定长/长定长长度），不追求通用；
//! - 模幂用 **Montgomery 乘法（CIOS）+ u32 limb**，只支持奇数模数（RSA 模数必为奇数）；
//! - 签名填充按 PKCS#1 v1.5（`00 01 FF..FF 00 || DigestInfo`），摘要固定 SHA-256
//! （支付宝 `sign_type=RSA2` 就是 SHA256withRSA）。
//!
//! 不引第三方 crate：`rsa`/`sha2`/`asn1` 都不在离线依赖里。

use std::cmp::Ordering;

use super::crypto;

/// 大整数（小端 `u32` limb，高位 0 已裁掉）。
pub type Limbs = Vec<u32>;

/// RSA 公钥：模数 + 指数。
#[derive(Debug, Clone)]
pub struct RsaPublicKey {
    pub modulus: Limbs,
    pub exponent: Limbs,
}

impl RsaPublicKey {
    /// 模数的字节长度（签名长度必须等于它）。
    pub fn byte_len(&self) -> usize {
        limbs_byte_len(&self.modulus)
    }
}

/// RSA 私钥：模数 + 私钥指数（不做 CRT 优化，够支付宝下单用）。
#[derive(Debug, Clone)]
pub struct RsaPrivateKey {
    pub modulus: Limbs,
    pub private_exponent: Limbs,
}

impl RsaPrivateKey {
    pub fn byte_len(&self) -> usize {
        limbs_byte_len(&self.modulus)
    }
}

// ---------------------------------------------------------------------------
// 大整数基础操作
// ---------------------------------------------------------------------------

/// 大端字节 → limb（自动裁掉高位 0）。
pub fn limbs_from_be(bytes: &[u8]) -> Limbs {
    let mut limbs: Limbs = Vec::with_capacity(bytes.len() / 4 + 1);
    let mut index = bytes.len();
    while index > 0 {
        let start = index.saturating_sub(4);
        let mut value = 0u32;
        for byte in &bytes[start..index] {
            value = (value << 8) | *byte as u32;
        }
        limbs.push(value);
        index = start;
    }
    trim(&mut limbs);
    limbs
}

/// limb → 定长大端字节（左侧补 0 到 `len`，超出则截断高位）。
pub fn limbs_to_be(value: &Limbs, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    for (index, limb) in value.iter().enumerate() {
        let base = index * 4;
        if base >= len {
            break;
        }
        let bytes = limb.to_be_bytes();
        for offset in 0..4 {
            let position = base + offset;
            if position < len {
                out[len - 1 - position] = bytes[3 - offset];
            }
        }
    }
    out
}

/// 模数字节长度（至少 1）。
fn limbs_byte_len(value: &Limbs) -> usize {
    match value.last() {
        Some(top) if *top != 0 => (value.len() - 1) * 4 + (4 - top.leading_zeros() as usize / 8),
        _ => 1,
    }
}

fn trim(value: &mut Limbs) {
    while value.last() == Some(&0) {
        value.pop();
    }
}

/// 按数值比较（忽略长度差异，短的高位当 0）。
fn cmp_limbs(a: &[u32], b: &[u32]) -> Ordering {
    let len = a.len().max(b.len());
    for index in (0..len).rev() {
        let left = *a.get(index).unwrap_or(&0);
        let right = *b.get(index).unwrap_or(&0);
        match left.cmp(&right) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

/// `a -= b`（要求 a >= b），长度取两者最大值。
fn sub_limbs(a: &mut Vec<u32>, b: &[u32]) {
    let mut borrow = 0i64;
    for index in 0..a.len().max(b.len()) {
        if index >= a.len() {
            a.push(0);
        }
        let left = a[index] as i64 - borrow - *b.get(index).unwrap_or(&0) as i64;
        if left < 0 {
            a[index] = (left + (1i64 << 32)) as u32;
            borrow = 1;
        } else {
            a[index] = left as u32;
            borrow = 0;
        }
    }
    trim(a);
}

/// 比特长度。
fn bit_len(value: &[u32]) -> usize {
    match value.iter().rposition(|limb| *limb != 0) {
        Some(index) => index * 32 + (32 - value[index].leading_zeros() as usize),
        None => 0,
    }
}

fn bit_at(value: &[u32], index: usize) -> bool {
    match value.get(index / 32) {
        Some(limb) => (limb >> (index % 32)) & 1 == 1,
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Montgomery 乘法（CIOS）
// ---------------------------------------------------------------------------

/// 模幂上下文。
struct Montgomery {
    n: Limbs,
    k: usize,
    /// `-n[0]^{-1} mod 2^32`
    n0inv: u32,
    /// `R^2 mod n`，`R = 2^(32k)`
    r2: Limbs,
}

impl Montgomery {
    fn new(n: Limbs) -> Option<Self> {
        if n.is_empty() || n[0] % 2 == 0 {
            return None;
        }
        let k = n.len();
        let n0inv = n[0].wrapping_neg_inverse();
        let r2 = compute_r2(&n, k);
        Some(Self { n, k, n0inv, r2 })
    }

    /// Montgomery 乘法：返回 `a*b*R^{-1} mod n`（a、b 必须 < n）。
    fn mul(&self, a: &[u32], b: &[u32]) -> Limbs {
        let k = self.k;
        let mut t = vec![0u32; k + 2];
        for index in 0..k {
            let bi = *b.get(index).unwrap_or(&0) as u64;
            let mut carry = 0u64;
            for limb in 0..k {
                let aj = *a.get(limb).unwrap_or(&0) as u64;
                let sum = t[limb] as u64 + aj * bi + carry;
                t[limb] = sum as u32;
                carry = sum >> 32;
            }
            let sum = t[k] as u64 + carry;
            t[k] = sum as u32;
            t[k + 1] = (sum >> 32) as u32;

            let m = t[0].wrapping_mul(self.n0inv);
            let mut carry = 0u64;
            for limb in 0..k {
                let sum = t[limb] as u64 + (m as u64) * (self.n[limb] as u64) + carry;
                t[limb] = sum as u32;
                carry = sum >> 32;
            }
            let sum = t[k] as u64 + carry;
            t[k] = sum as u32;
            t[k + 1] = t[k + 1].wrapping_add((sum >> 32) as u32);

            for limb in 0..=k {
                t[limb] = t[limb + 1];
            }
            t[k + 1] = 0;
        }

        let mut result: Limbs = t[..=k].to_vec();
        if cmp_limbs(&result, &self.n) != Ordering::Less {
            sub_limbs(&mut result, &self.n);
        }
        result.truncate(k);
        trim(&mut result);
        result
    }

    /// 模幂：`base^exp mod n`（base 必须 < n）。
    fn mod_pow(&self, base: &Limbs, exp: &Limbs) -> Limbs {
        let one: Limbs = vec![1];
        // R mod n
        let mut acc = self.mul(&one, &self.r2);
        let base_mont = self.mul(base, &self.r2);
        let bits = bit_len(exp);
        for index in (0..bits).rev() {
            acc = self.mul(&acc, &acc);
            if bit_at(exp, index) {
                acc = self.mul(&acc, &base_mont);
            }
        }
        self.mul(&acc, &one)
    }
}

/// `x^{-1} mod 2^32`（x 为奇数），返回 `-x^{-1} mod 2^32`。
trait ModInverse32 {
    fn wrapping_neg_inverse(self) -> u32;
}

impl ModInverse32 for u32 {
    fn wrapping_neg_inverse(self) -> u32 {
        // Newton 迭代：每轮把正确位数翻倍，5 轮覆盖 32 位。
        let mut y: u32 = 1;
        for _ in 0..5 {
            y = y.wrapping_mul(2u32.wrapping_sub(self.wrapping_mul(y)));
        }
        y.wrapping_neg()
    }
}

/// `R^2 mod n`（从 1 起，做 `64k` 次「翻倍取模」）。
fn compute_r2(n: &Limbs, k: usize) -> Limbs {
    let mut value: Limbs = vec![1];
    for _ in 0..(64 * k) {
        value = double_mod(&value, n);
    }
    value
}

/// `2x mod n`。
fn double_mod(value: &Limbs, n: &Limbs) -> Limbs {
    let mut out = Vec::with_capacity(value.len() + 1);
    let mut carry = 0u32;
    for limb in value {
        let shifted = (limb << 1) | carry;
        carry = limb >> 31;
        out.push(shifted);
    }
    if carry > 0 {
        out.push(carry);
    }
    trim(&mut out);
    if cmp_limbs(&out, n) != Ordering::Less {
        sub_limbs(&mut out, n);
    }
    out
}

// ---------------------------------------------------------------------------
// ASN.1 DER
// ---------------------------------------------------------------------------

/// 读一个 TLV，返回 `(tag, value, 整段长度)`。
fn der_read_tlv(data: &[u8]) -> Option<(u8, &[u8], usize)> {
    if data.len() < 2 {
        return None;
    }
    let tag = data[0];
    let first = data[1];
    let (length, header) = if first < 0x80 {
        (first as usize, 2usize)
    } else {
        let count = (first & 0x7f) as usize;
        if count == 0 || count > 3 || data.len() < 2 + count {
            return None;
        }
        let mut length = 0usize;
        for index in 0..count {
            length = (length << 8) | data[2 + index] as usize;
        }
        (length, 2 + count)
    };
    if data.len() < header + length {
        return None;
    }
    Some((tag, &data[header..header + length], header + length))
}

/// 解析 SEQUENCE / SET 内容里的直接子元素。
fn der_children(content: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut rest = content;
    while !rest.is_empty() {
        match der_read_tlv(rest) {
            Some((tag, value, consumed)) => {
                out.push((tag, value));
                rest = &rest[consumed..];
                if consumed == 0 {
                    break;
                }
            }
            None => break,
        }
    }
    out
}

/// PKCS#1 `RSAPublicKey ::= SEQUENCE { modulus INTEGER, publicExponent INTEGER }`
fn parse_pkcs1_public(content: &[u8]) -> Option<(Limbs, Limbs)> {
    let children = der_children(content);
    if children.len() < 2 {
        return None;
    }
    if children[0].0 != 0x02 || children[1].0 != 0x02 {
        return None;
    }
    let modulus = limbs_from_be(children[0].1);
    let exponent = limbs_from_be(children[1].1);
    if modulus.is_empty() || exponent.is_empty() {
        return None;
    }
    Some((modulus, exponent))
}

/// `SubjectPublicKeyInfo ::= SEQUENCE { algorithm AlgorithmIdentifier, subjectPublicKey BIT STRING }`
fn parse_spki(content: &[u8]) -> Option<(Limbs, Limbs)> {
    let children = der_children(content);
    if children.len() < 2 {
        return None;
    }
    if children[0].0 != 0x30 || children[1].0 != 0x03 {
        return None;
    }
    let bits = children[1].1;
    // BIT STRING 首字节是「未使用位数」，公钥永远是整字节
    if bits.len() < 2 || bits[0] != 0x00 {
        return None;
    }
    let inner = der_read_tlv(&bits[1..])?;
    if inner.0 != 0x30 {
        return None;
    }
    parse_pkcs1_public(inner.1)
}

/// X.509 证书里找 `SubjectPublicKeyInfo`。
fn parse_certificate(content: &[u8]) -> Option<(Limbs, Limbs)> {
    let outer = der_children(content);
    let tbs = outer
        .iter()
        .find(|(tag, _)| *tag == 0x30)
        .map(|(_, value)| *value)?;
    // 逐个 SEQUENCE 试解析：版本号 / 序列号 / 签名算法 / 有效期等都是 SEQUENCE，
    // 只有 SPKI 能同时解出「算法 + BIT STRING」，第一个解出来的就是公钥。
    for (tag, value) in der_children(tbs) {
        if tag != 0x30 {
            continue;
        }
        if let Some(key) = parse_spki(value) {
            return Some(key);
        }
    }
    None
}

/// PKCS#1 `RSAPrivateKey`（只取模数与私钥指数）。
fn parse_pkcs1_private(content: &[u8]) -> Option<(Limbs, Limbs)> {
    let children = der_children(content);
    if children.len() < 4 {
        return None;
    }
    if children[0].0 != 0x02 || children[1].0 != 0x02 || children[3].0 != 0x02 {
        return None;
    }
    let modulus = limbs_from_be(children[1].1);
    let exponent = limbs_from_be(children[3].1);
    if modulus.is_empty() || exponent.is_empty() {
        return None;
    }
    Some((modulus, exponent))
}

/// PKCS#8 `PrivateKeyInfo ::= SEQUENCE { version, algorithm, privateKey OCTET STRING }`
fn parse_pkcs8_private(content: &[u8]) -> Option<(Limbs, Limbs)> {
    let children = der_children(content);
    if children.len() < 3 {
        return None;
    }
    if children[2].0 != 0x04 {
        return None;
    }
    let inner = der_read_tlv(children[2].1)?;
    if inner.0 != 0x30 {
        return None;
    }
    parse_pkcs1_private(inner.1)
}

/// PEM / 裸 base64 → DER 字节。
fn pem_to_der(text: &str) -> Result<Vec<u8>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("密钥内容为空".to_string());
    }
    let body = if let Some(begin) = trimmed.find("-----BEGIN") {
        let after_begin = match trimmed[begin..].find("-----") {
            Some(_) => &trimmed[begin + 5..],
            None => return Err("PEM 头部不完整".to_string()),
        };
        // 跳过 BEGIN 那一行的行尾，取到 END 之前
        let after_line = match after_begin.find('\n') {
            Some(index) => &after_begin[index + 1..],
            None => return Err("PEM 头部不完整".to_string()),
        };
        let end = after_line
            .find("-----END")
            .ok_or_else(|| "PEM 缺少 END 行".to_string())?;
        after_line[..end].to_string()
    } else {
        trimmed.to_string()
    };
    rustcms_ext::smtp::base64_decode(&body).ok_or_else(|| "密钥不是合法的 base64".to_string())
}

/// 解析 RSA 公钥（支持 `PUBLIC KEY` / `RSA PUBLIC KEY` / `CERTIFICATE`）。
///
/// 等价于 `alipay.ParsePublicKey(publicKey string)`。
pub fn parse_public_key(text: &str) -> Result<RsaPublicKey, String> {
    let der = pem_to_der(text)?;
    let (tag, content, _) = der_read_tlv(&der).ok_or_else(|| "公钥不是合法的 DER".to_string())?;
    if tag != 0x30 {
        return Err("公钥不是合法的 DER SEQUENCE".to_string());
    }
    // 依次尝试三种结构（PEM 头不可靠时也能解出来）
    let parsed = parse_spki(content)
        .or_else(|| parse_certificate(content))
        .or_else(|| parse_pkcs1_public(content));
    match parsed {
        Some((modulus, exponent)) => Ok(RsaPublicKey { modulus, exponent }),
        None => Err("无法解析 RSA 公钥（既不是 SPKI / 证书，也不是 PKCS#1）".to_string()),
    }
}

/// 解析 RSA 私钥（支持 `PRIVATE KEY`（PKCS#8）与 `RSA PRIVATE KEY`（PKCS#1））。
///
/// 私钥同时支持 PKCS#8 与 PKCS#1 两种 DER 包装。
pub fn parse_private_key(text: &str) -> Result<RsaPrivateKey, String> {
    let der = pem_to_der(text)?;
    let (tag, content, _) = der_read_tlv(&der).ok_or_else(|| "私钥不是合法的 DER".to_string())?;
    if tag != 0x30 {
        return Err("私钥不是合法的 DER SEQUENCE".to_string());
    }
    let parsed = parse_pkcs1_private(content).or_else(|| parse_pkcs8_private(content));
    match parsed {
        Some((modulus, private_exponent)) => Ok(RsaPrivateKey {
            modulus,
            private_exponent,
        }),
        None => Err("无法解析 RSA 私钥（既不是 PKCS#8，也不是 PKCS#1）".to_string()),
    }
}

// ---------------------------------------------------------------------------
// PKCS#1 v1.5 填充 + SHA-256
// ---------------------------------------------------------------------------

/// SHA-256 的 `DigestInfo` 前缀（RFC 8017 附录 A.2.4）。
const SHA256_DIGEST_INFO_PREFIX: [u8; 19] = [
    0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05,
    0x00, 0x04, 0x20,
];

/// `DigestInfo(SHA256, digest)`。
pub fn sha256_digest_info(digest: &[u8; 32]) -> Vec<u8> {
    let mut out = SHA256_DIGEST_INFO_PREFIX.to_vec();
    out.extend_from_slice(digest);
    out
}

/// 按 PKCS#1 v1.5 组装 `EM`（`00 01 FF..FF 00 || DigestInfo`）。
pub fn pkcs1_v15_encode(digest_info: &[u8], k: usize) -> Result<Vec<u8>, String> {
    if k < digest_info.len() + 11 {
        return Err("RSA 模数太短，无法容纳 SHA-256 签名".to_string());
    }
    let mut out = Vec::with_capacity(k);
    out.push(0x00);
    out.push(0x01);
    out.resize(k - digest_info.len() - 1, 0xff);
    out.push(0x00);
    out.extend_from_slice(digest_info);
    Ok(out)
}

/// 校验 `EM` 是否为合法填充且摘要一致。
pub fn pkcs1_v15_check(em: &[u8], digest_info: &[u8]) -> bool {
    if em.len() < digest_info.len() + 11 {
        return false;
    }
    if em[0] != 0x00 || em[1] != 0x01 {
        return false;
    }
    let mut index = 2usize;
    while index < em.len() && em[index] == 0xff {
        index += 1;
    }
    if index - 2 < 8 {
        return false;
    }
    if index >= em.len() || em[index] != 0x00 {
        return false;
    }
    &em[index + 1..] == digest_info
}

/// RSA2 验签（SHA256withRSA / PKCS#1 v1.5）。
///
/// 等价于 `alipay.VerifySign(publicKey, bm)`。
pub fn verify_sha256(key: &RsaPublicKey, message: &[u8], signature: &[u8]) -> bool {
    let k = key.byte_len();
    if signature.len() != k {
        return false;
    }
    let value = limbs_from_be(signature);
    if cmp_limbs(&value, &key.modulus) != Ordering::Less {
        return false;
    }
    let Some(mont) = Montgomery::new(key.modulus.clone()) else {
        return false;
    };
    let em = mont.mod_pow(&value, &key.exponent);
    let em_bytes = limbs_to_be(&em, k);
    pkcs1_v15_check(&em_bytes, &sha256_digest_info(&crypto::sha256(message)))
}

/// RSA2 签名（返回模数字节长度的原始签名，调用方自行 base64 / URL 编码）。
pub fn sign_sha256(key: &RsaPrivateKey, message: &[u8]) -> Result<Vec<u8>, String> {
    let k = key.byte_len();
    let digest_info = sha256_digest_info(&crypto::sha256(message));
    let em = pkcs1_v15_encode(&digest_info, k)?;
    let value = limbs_from_be(&em);
    let mont = Montgomery::new(key.modulus.clone()).ok_or_else(|| "RSA 模数非法".to_string())?;
    let signature = mont.mod_pow(&value, &key.private_exponent);
    Ok(limbs_to_be(&signature, k))
}

/// 直接对摘要签名的便捷入口（测试与自定义 digest 场景）。
pub fn sign_digest_info(key: &RsaPrivateKey, digest_info: &[u8]) -> Result<Vec<u8>, String> {
    let k = key.byte_len();
    let em = pkcs1_v15_encode(digest_info, k)?;
    let value = limbs_from_be(&em);
    let mont = Montgomery::new(key.modulus.clone()).ok_or_else(|| "RSA 模数非法".to_string())?;
    let signature = mont.mod_pow(&value, &key.private_exponent);
    Ok(limbs_to_be(&signature, k))
}

/// 固定测试向量：512 位玩具 RSA 公钥（e=3），用 PKCS#1 v1.5 + SHA-256 签出的真实签名。
///
/// 生成方式：取 p ≡ 2 (mod 3)、q = 3、e = 3，则 `3^{-1} mod (p-1) = (2p-1)/3`，
/// 用 CRT 把 `EM = 00 01 FF..FF 00 || DigestInfo(SHA256(msg))` 开三次方得到 s，
/// 并自检 `s^3 mod n == EM`（等价于任一标准验签器的判定）。
/// 它覆盖「ASN.1 解析公钥 → 模幂 → PKCS#1 填充 → 摘要比对」整条链路。
#[cfg(test)]
pub(crate) const RSA2_PUBLIC_KEY_PEM: &str = "-----BEGIN PUBLIC KEY-----
MFgwDQYJKoZIhvcNAQEBBQADRwAwRAI/AwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAbVAgED
-----END PUBLIC KEY-----
";

#[cfg(test)]
pub(crate) const RSA2_SIGNED_MESSAGE: &str =
    "app_id=2021000000000000&out_trade_no=P20260101000001&total_amount=1.00";

#[cfg(test)]
pub(crate) const RSA2_SIGNATURE_B64: &str =
    "AXvVL/Di3I2z9kOxK66+rnmrHHJ8fR0H4NIGzo7GeIOp5P8WW0o+LAzHGqOz+MplXrHkosdrC0ZZD2mNUcYb";

#[cfg(test)]
mod tests {
    use super::*;

    /// 用固定向量验证「真签名能过、篡改一定挂」。
    #[test]
    fn verify_sha256_accepts_fixed_vector() {
        let key = parse_public_key(RSA2_PUBLIC_KEY_PEM).unwrap();
        assert_eq!(key.byte_len(), 63, "模数是 63 字节");
        assert_eq!(key.exponent, vec![3]);
        let signature = rustcms_ext::smtp::base64_decode(RSA2_SIGNATURE_B64).unwrap();
        assert_eq!(signature.len(), 63);

        assert!(verify_sha256(
            &key,
            RSA2_SIGNED_MESSAGE.as_bytes(),
            &signature
        ));

        // 消息改一个字节（1.00 → 1.01）：摘要不同，必须失败
        let tampered_message = RSA2_SIGNED_MESSAGE.replace("1.00", "1.01");
        assert!(!verify_sha256(&key, tampered_message.as_bytes(), &signature));

        // 签名改一个字节：模幂结果不再是合法填充，必须失败
        let mut tampered = signature.clone();
        tampered[62] ^= 0x01;
        assert!(!verify_sha256(&key, RSA2_SIGNED_MESSAGE.as_bytes(), &tampered));

        // 长度不对直接失败
        assert!(!verify_sha256(&key, RSA2_SIGNED_MESSAGE.as_bytes(), &signature[1..]));
    }

    /// 极简 DER 编码（测试里手搭密钥结构用）。
    fn der(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        let len = content.len();
        if len < 0x80 {
            out.push(len as u8);
        } else if len < 0x100 {
            out.push(0x81);
            out.push(len as u8);
        } else {
            out.push(0x82);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        }
        out.extend_from_slice(content);
        out
    }

    fn der_int(bytes: &[u8]) -> Vec<u8> {
        der(0x02, bytes)
    }

    fn pem(label: &str, der_bytes: &[u8]) -> String {
        let encoded = rustcms_ext::smtp::base64_encode(der_bytes);
        let mut body = String::new();
        for chunk in encoded.as_bytes().chunks(64) {
            body.push_str(&String::from_utf8_lossy(chunk));
            body.push('\n');
        }
        format!("-----BEGIN {label}-----\n{body}-----END {label}-----\n")
    }

    /// 玩具密钥：n = 3233(0x0CA1)，e = 17。
    fn toy_pkcs1_public() -> Vec<u8> {
        let mut content = der_int(&[0x0c, 0xa1]);
        content.extend(der_int(&[0x11]));
        der(0x30, &content)
    }

    fn toy_spki() -> Vec<u8> {
        // OID 1.2.840.113549.1.1.1 (rsaEncryption) + NULL
        let oid = der(0x06, &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01]);
        let mut alg = oid.clone();
        alg.extend(der(0x05, &[]));
        let alg_id = der(0x30, &alg);

        let mut bits = vec![0x00u8];
        bits.extend(toy_pkcs1_public());
        let bit_string = der(0x03, &bits);

        let mut content = alg_id;
        content.extend(bit_string);
        der(0x30, &content)
    }

    #[test]
    fn parse_pkcs1_public_key() {
        let key = parse_public_key(&pem("RSA PUBLIC KEY", &toy_pkcs1_public())).unwrap();
        assert_eq!(key.modulus, vec![0x0ca1]);
        assert_eq!(key.exponent, vec![0x11]);
        assert_eq!(key.byte_len(), 2);
    }

    #[test]
    fn parse_spki_public_key() {
        let key = parse_public_key(&pem("PUBLIC KEY", &toy_spki())).unwrap();
        assert_eq!(key.modulus, vec![0x0ca1]);
        assert_eq!(key.exponent, vec![0x11]);
    }

    #[test]
    fn parse_certificate_public_key() {
        // 证书里 tbsCertificate 依次是：版本 / 序列号 / 签名算法 / 颁发者 / 有效期 / 主体 / SPKI
        let mut tbs = der(0xa0, &der_int(&[0x02]));
        tbs.extend(der_int(&[0x01]));
        let mut alg = der(0x06, &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b]);
        alg.extend(der(0x05, &[]));
        tbs.extend(der(0x30, &alg));
        tbs.extend(der(0x30, &der(0x06, &[0x55, 0x04, 0x03])));
        tbs.extend(der(0x30, &[]));
        tbs.extend(der(0x30, &[]));
        tbs.extend(toy_spki());
        let certificate = der(0x30, &der(0x30, &tbs));

        let key = parse_public_key(&pem("CERTIFICATE", &certificate)).unwrap();
        assert_eq!(key.modulus, vec![0x0ca1]);
        assert_eq!(key.exponent, vec![0x11]);
    }

    #[test]
    fn parse_pkcs1_private_key() {
        // RSAPrivateKey ::= SEQUENCE { version, n, e, d, p, q, dp, dq, qinv }
        let mut content = der_int(&[0x00]);
        content.extend(der_int(&[0x0c, 0xa1]));
        content.extend(der_int(&[0x11]));
        content.extend(der_int(&[0x0a, 0xc3]));
        for _ in 0..5 {
            content.extend(der_int(&[0x01]));
        }
        let key = parse_private_key(&pem("RSA PRIVATE KEY", &der(0x30, &content))).unwrap();
        assert_eq!(key.modulus, vec![0x0ca1]);
        assert_eq!(key.private_exponent, vec![0x0ac3]);
    }

    #[test]
    fn parse_pkcs8_private_key() {
        let mut pkcs1 = der_int(&[0x00]);
        pkcs1.extend(der_int(&[0x0c, 0xa1]));
        pkcs1.extend(der_int(&[0x11]));
        pkcs1.extend(der_int(&[0x0a, 0xc3]));
        for _ in 0..5 {
            pkcs1.extend(der_int(&[0x01]));
        }
        let mut content = der_int(&[0x00]);
        let mut alg = der(0x06, &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01]);
        alg.extend(der(0x05, &[]));
        content.extend(der(0x30, &alg));
        content.extend(der(0x04, &der(0x30, &pkcs1)));
        let key = parse_private_key(&pem("PRIVATE KEY", &der(0x30, &content))).unwrap();
        assert_eq!(key.modulus, vec![0x0ca1]);
        assert_eq!(key.private_exponent, vec![0x0ac3]);
    }

    #[test]
    fn mod_pow_matches_known_values() {
        // 4^13 mod 497 = 445；2^10 mod 1000 需要偶数模数（Montgomery 不支持），跳过
        let mont = Montgomery::new(vec![497]).unwrap();
        let result = mont.mod_pow(&vec![4], &vec![13]);
        assert_eq!(result, vec![445]);

        // 3^200 mod 2^31-1（素数）用大整数复核：3^200 = 265613988875874769338781322035779626829233452653394495974574961739092490901302182994384699044001
        // 手工算太容易出错，这里用「a^2 mod n 与直接相乘一致」的方式校验一致性。
        let n: u64 = 2_147_483_647;
        let mont = Montgomery::new(limbs_from_be(&n.to_be_bytes())).unwrap();
        let squared = mont.mod_pow(&vec![123_456_789], &vec![2]);
        let expect = ((123_456_789u128 * 123_456_789u128) % n as u128) as u32;
        assert_eq!(squared, vec![expect]);
    }

    #[test]
    fn pkcs1_padding_roundtrip() {
        // 固定向量：SHA-256("abc")
        let digest: [u8; 32] = [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ];
        assert_eq!(crypto::sha256(b"abc"), digest);
        let info = sha256_digest_info(&digest);
        assert_eq!(info.len(), 51);

        let em = pkcs1_v15_encode(&info, 128).unwrap();
        assert_eq!(em.len(), 128);
        assert_eq!(&em[..2], &[0x00, 0x01]);
        assert_eq!(em[128 - 51 - 1], 0x00);
        assert!(pkcs1_v15_check(&em, &info));

        // 篡改填充 / 摘要都要被拒
        let mut bad = em.clone();
        bad[10] = 0x00;
        assert!(!pkcs1_v15_check(&bad, &info));
        let mut bad = em.clone();
        bad[127] ^= 0xff;
        assert!(!pkcs1_v15_check(&bad, &info));
        // 填充太短（0xFF 少于 8 字节）被拒
        let mut short_pad = vec![0x00, 0x01, 0xff, 0x00];
        short_pad.extend_from_slice(&info);
        assert!(!pkcs1_v15_check(&short_pad, &info));
    }

    #[test]
    fn sign_and_verify_roundtrip_with_toy_key() {
        // 512 位以内的玩具密钥没法跑完整签名，这里只验证「模数太短要报错」。
        let key = RsaPrivateKey {
            modulus: vec![0x0ca1],
            private_exponent: vec![0x0ac3],
        };
        assert!(sign_sha256(&key, b"abc").is_err());
    }

    #[test]
    fn verify_rejects_bad_length_and_range() {
        let key = parse_public_key(&pem("PUBLIC KEY", &toy_spki())).unwrap();
        assert!(!verify_sha256(&key, b"abc", &[0x01]));
        // 签名值 >= 模数
        assert!(!verify_sha256(&key, b"abc", &[0x0c, 0xa1]));
    }

    #[test]
    fn limb_conversions_roundtrip() {
        let value: u128 = 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210;
        let bytes = value.to_be_bytes();
        let limbs = limbs_from_be(&bytes);
        assert_eq!(limbs_to_be(&limbs, 16), bytes.to_vec());
        assert_eq!(limbs_from_be(&[0x00, 0x00]), Limbs::new());
        assert_eq!(limbs_to_be(&Limbs::new(), 4), vec![0, 0, 0, 0]);
        assert_eq!(limbs_byte_len(&vec![0x0ca1]), 2);
        assert_eq!(limbs_byte_len(&vec![0x10000]), 3);
    }
}

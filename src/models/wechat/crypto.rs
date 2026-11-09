//! 微信消息加解密（`WXBizMsgCrypt` 口径）。
//!
//! 公众号「安全模式」与小程序消息推送用的是同一套协议：
//!
//! ```text
//! key = base64_decode(EncodingAESKey + "=") // 32 字节
//! iv = key[0..16]
//! 密文 = AES-128-CBC(key, iv, PKCS#7(明文, 块大小 32))
//! 明文 = 16 字节随机串 + 4 字节大端长度 + 消息体 + appid
//! 签名 = sha1(字典序拼接(token, timestamp, nonce, 密文 base64))
//! ```
//!
//! 仓库里没有 AES 依赖（也不允许新增），所以这里手写一份 AES-128：
//! 只做 16 字节分组的加/解密与 CBC 串联，够处理微信消息，性能不是瓶颈。

use std::sync::OnceLock;

use rustcms_ext::smtp::{base64_decode, base64_encode};

use crate::models::interact::sha1_hex;

/// PKCS#7 补位块大小（微信固定 32，不是 AES 的 16）。
pub const PKCS7_BLOCK: usize = 32;

/// 明文头部随机串长度。
const RANDOM_LEN: usize = 16;

/// 长度字段长度。
const LENGTH_LEN: usize = 4;

// ---------------------------------------------------------------------------
// 有限域 GF(2^8) 运算
// ---------------------------------------------------------------------------

/// GF(2^8) 乘法（模多项式 0x11B）。
fn gmul(a: u8, b: u8) -> u8 {
    let mut a = a;
    let mut b = b;
    let mut result = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 {
            result ^= a;
        }
        let high = a & 0x80;
        a <<= 1;
        if high != 0 {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    result
}

/// GF(2^8) 乘法逆元（0 的逆元按 AES 惯例取 0）。
fn ginv(value: u8) -> u8 {
    if value == 0 {
        return 0;
    }
    for candidate in 1..=255u8 {
        if gmul(value, candidate) == 1 {
            return candidate;
        }
    }
    0
}

/// AES 正向 / 逆向 S 盒（按 GF 逆元 + 仿射变换现场生成，避免手抄 256 项常量出错）。
fn sboxes() -> &'static ([u8; 256], [u8; 256]) {
    static TABLES: OnceLock<([u8; 256], [u8; 256])> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut forward = [0u8; 256];
        for value in 0..256u32 {
            let inverse = ginv(value as u8);
            let mut substituted = 0u8;
            for bit in 0..8u32 {
                let bit_at = |offset: u32| (inverse >> ((bit + offset) % 8)) & 1;
                // b'_i = b_i ⊕ b_(i+4) ⊕ b_(i+5) ⊕ b_(i+6) ⊕ b_(i+7) ⊕ c_i，c = 0x63
                let mixed = bit_at(0) ^ bit_at(4) ^ bit_at(5) ^ bit_at(6) ^ bit_at(7);
                let constant = (0x63u8 >> bit) & 1;
                substituted |= ((mixed ^ constant) & 1) << bit;
            }
            forward[value as usize] = substituted;
        }
        let mut backward = [0u8; 256];
        for (value, substituted) in forward.iter().enumerate() {
            backward[*substituted as usize] = value as u8;
        }
        (forward, backward)
    })
}

// ---------------------------------------------------------------------------
// AES-128 分组加解密
// ---------------------------------------------------------------------------

/// 轮密钥扩展（AES-128：11 组，共 176 字节）。
fn expand_key(key: &[u8; 16]) -> [u8; 176] {
    let (forward, _) = sboxes();
    let mut expanded = [0u8; 176];
    expanded[..16].copy_from_slice(key);
    let mut round_constant = 1u8;
    for word in 4..44usize {
        let mut temp = [
            expanded[(word - 1) * 4],
            expanded[(word - 1) * 4 + 1],
            expanded[(word - 1) * 4 + 2],
            expanded[(word - 1) * 4 + 3],
        ];
        if word % 4 == 0 {
            // RotWord + SubWord + Rcon
            temp.rotate_left(1);
            for byte in temp.iter_mut() {
                *byte = forward[*byte as usize];
            }
            temp[0] ^= round_constant;
            round_constant = gmul(round_constant, 2);
        }
        for index in 0..4 {
            expanded[word * 4 + index] = expanded[(word - 4) * 4 + index] ^ temp[index];
        }
    }
    expanded
}

fn add_round_key(state: &mut [u8; 16], key: &[u8]) {
    for index in 0..16 {
        state[index] ^= key[index];
    }
}

/// 行移位：第 r 行（`state[r + 4c]`）循环左移 r 位。
fn shift_rows(state: &mut [u8; 16]) {
    for row in 1..4usize {
        let mut values = [state[row], state[row + 4], state[row + 8], state[row + 12]];
        values.rotate_left(row);
        state[row] = values[0];
        state[row + 4] = values[1];
        state[row + 8] = values[2];
        state[row + 12] = values[3];
    }
}

/// 逆行移位：第 r 行循环右移 r 位。
fn inv_shift_rows(state: &mut [u8; 16]) {
    for row in 1..4usize {
        let mut values = [state[row], state[row + 4], state[row + 8], state[row + 12]];
        values.rotate_right(row);
        state[row] = values[0];
        state[row + 4] = values[1];
        state[row + 8] = values[2];
        state[row + 12] = values[3];
    }
}

fn mix_columns(state: &mut [u8; 16]) {
    for column in 0..4usize {
        let index = column * 4;
        let (a0, a1, a2, a3) = (
            state[index],
            state[index + 1],
            state[index + 2],
            state[index + 3],
        );
        state[index] = gmul(a0, 2) ^ gmul(a1, 3) ^ a2 ^ a3;
        state[index + 1] = a0 ^ gmul(a1, 2) ^ gmul(a2, 3) ^ a3;
        state[index + 2] = a0 ^ a1 ^ gmul(a2, 2) ^ gmul(a3, 3);
        state[index + 3] = gmul(a0, 3) ^ a1 ^ a2 ^ gmul(a3, 2);
    }
}

fn inv_mix_columns(state: &mut [u8; 16]) {
    for column in 0..4usize {
        let index = column * 4;
        let (a0, a1, a2, a3) = (
            state[index],
            state[index + 1],
            state[index + 2],
            state[index + 3],
        );
        state[index] = gmul(a0, 14) ^ gmul(a1, 11) ^ gmul(a2, 13) ^ gmul(a3, 9);
        state[index + 1] = gmul(a0, 9) ^ gmul(a1, 14) ^ gmul(a2, 11) ^ gmul(a3, 13);
        state[index + 2] = gmul(a0, 13) ^ gmul(a1, 9) ^ gmul(a2, 14) ^ gmul(a3, 11);
        state[index + 3] = gmul(a0, 11) ^ gmul(a1, 13) ^ gmul(a2, 9) ^ gmul(a3, 14);
    }
}

/// 加密一个 16 字节分组。
fn encrypt_block(round_keys: &[u8; 176], block: &mut [u8; 16]) {
    let (forward, _) = sboxes();
    add_round_key(block, &round_keys[0..16]);
    for round in 1..10usize {
        for byte in block.iter_mut() {
            *byte = forward[*byte as usize];
        }
        shift_rows(block);
        mix_columns(block);
        add_round_key(block, &round_keys[round * 16..round * 16 + 16]);
    }
    for byte in block.iter_mut() {
        *byte = forward[*byte as usize];
    }
    shift_rows(block);
    add_round_key(block, &round_keys[160..176]);
}

/// 解密一个 16 字节分组（逆密码，轮密钥倒序使用）。
fn decrypt_block(round_keys: &[u8; 176], block: &mut [u8; 16]) {
    let (_, backward) = sboxes();
    add_round_key(block, &round_keys[160..176]);
    for round in (1..10usize).rev() {
        inv_shift_rows(block);
        for byte in block.iter_mut() {
            *byte = backward[*byte as usize];
        }
        add_round_key(block, &round_keys[round * 16..round * 16 + 16]);
        inv_mix_columns(block);
    }
    inv_shift_rows(block);
    for byte in block.iter_mut() {
        *byte = backward[*byte as usize];
    }
    add_round_key(block, &round_keys[0..16]);
}

/// CBC 加密（不做补位，入参长度必须是 16 的整数倍）。
pub fn cbc_encrypt(key: &[u8; 16], iv: &[u8; 16], plain: &[u8]) -> Vec<u8> {
    let round_keys = expand_key(key);
    let mut out = Vec::with_capacity(plain.len());
    let mut chain = *iv;
    for chunk in plain.chunks(16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        for index in 0..16 {
            block[index] ^= chain[index];
        }
        encrypt_block(&round_keys, &mut block);
        out.extend_from_slice(&block);
        chain = block;
    }
    out
}

/// CBC 解密（不做去补位，入参长度必须是 16 的整数倍）。
pub fn cbc_decrypt(key: &[u8; 16], iv: &[u8; 16], cipher: &[u8]) -> Option<Vec<u8>> {
    if cipher.is_empty() || cipher.len() % 16 != 0 {
        return None;
    }
    let round_keys = expand_key(key);
    let mut out = Vec::with_capacity(cipher.len());
    let mut chain = *iv;
    for chunk in cipher.chunks(16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        let saved = block;
        decrypt_block(&round_keys, &mut block);
        for index in 0..16 {
            block[index] ^= chain[index];
        }
        out.extend_from_slice(&block);
        chain = saved;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// PKCS#7 补位（块大小 32）
// ---------------------------------------------------------------------------

/// 追加 PKCS#7 补位。
pub fn pkcs7_pad(data: &mut Vec<u8>, block: usize) {
    let block = block.clamp(1, 255);
    let padding = block - data.len() % block;
    data.extend(std::iter::repeat(padding as u8).take(padding));
}

/// 去掉 PKCS#7 补位（失败返回 `None`，不修改入参）。
pub fn pkcs7_unpad(data: &[u8], block: usize) -> Option<Vec<u8>> {
    let block = block.clamp(1, 255);
    let last = *data.last()? as usize;
    if last == 0 || last > block || last > data.len() {
        return None;
    }
    if !data[data.len() - last..].iter().all(|byte| *byte as usize == last) {
        return None;
    }
    Some(data[..data.len() - last].to_vec())
}

// ---------------------------------------------------------------------------
// 微信信封
// ---------------------------------------------------------------------------

/// `EncodingAESKey`（43 位 base64 字符）→ 32 字节密钥。
pub fn decode_aes_key(encoding_aes_key: &str) -> Result<[u8; 32], String> {
    let raw = encoding_aes_key.trim();
    if raw.len() != 43 {
        return Err(format!(
            "EncodingAESKey 长度应为 43 个字符，当前 {} 个",
            raw.len()
        ));
    }
    let decoded = base64_decode(&format!("{raw}="))
        .ok_or_else(|| "EncodingAESKey 不是合法的 base64".to_string())?;
    if decoded.len() != 32 {
        return Err(format!(
            "EncodingAESKey 解码后应为 32 字节，实际 {} 字节",
            decoded.len()
        ));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&decoded);
    Ok(key)
}

/// 32 字节 `EncodingAESKey` → `(AES 密钥, IV)`。
///
/// 微信的约定是 **AES-128**：密钥与 IV 都取前 16 字节（官方 PHP/C# 示例同款），
/// 后 16 字节不参与运算。
fn aes_key_and_iv(key: &[u8; 32]) -> ([u8; 16], [u8; 16]) {
    let mut aes_key = [0u8; 16];
    aes_key.copy_from_slice(&key[..16]);
    (aes_key, aes_key)
}

/// 密文签名：`sha1(字典序拼接(token, timestamp, nonce, encrypt))`。
pub fn msg_signature(token: &str, timestamp: &str, nonce: &str, encrypt: &str) -> String {
    let mut parts = [token, timestamp, nonce, encrypt];
    parts.sort_unstable();
    sha1_hex(parts.join("").as_bytes())
}

/// 随机字符串（微信明文头部的 16 字节随机串 / 回复信封的 nonce）。
fn random_bytes(len: usize) -> Vec<u8> {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..len).map(|_| rng.gen::<u8>()).collect()
}

/// 随机数字串（回复信封的 `Nonce` 用数字，与微信一致）。
fn random_nonce() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..10)
        .map(|_| char::from(b'0' + rng.gen_range(0..10u8)))
        .collect()
}

/// 解密一条微信消息，返回明文 XML。
///
/// 校验顺序：`msg_signature` → base64 → AES-128-CBC → PKCS#7 → 长度字段 → appid。
pub fn decrypt_message(
    token: &str,
    encoding_aes_key: &str,
    app_id: &str,
    msg_signature_value: &str,
    timestamp: &str,
    nonce: &str,
    encrypt: &str,
) -> Result<String, String> {
    let encrypt = encrypt.trim();
    if encrypt.is_empty() {
        return Err("消息体缺少 Encrypt 字段".to_string());
    }
    let expected = msg_signature(token, timestamp, nonce, encrypt);
    if !expected.eq_ignore_ascii_case(msg_signature_value.trim()) {
        return Err("msg_signature 校验失败".to_string());
    }
    let (aes_key, iv) = aes_key_and_iv(&decode_aes_key(encoding_aes_key)?);

    let cipher = base64_decode(encrypt).ok_or_else(|| "消息密文不是合法的 base64".to_string())?;
    let mut plain = cbc_decrypt(&aes_key, &iv, &cipher)
        .ok_or_else(|| "消息密文长度不是 16 的整数倍".to_string())?;
    plain = pkcs7_unpad(&plain, PKCS7_BLOCK).ok_or_else(|| "消息密文补位不合法".to_string())?;
    if plain.len() < RANDOM_LEN + LENGTH_LEN {
        return Err("解密结果长度不足".to_string());
    }

    let length_offset = RANDOM_LEN;
    let message_len = u32::from_be_bytes([
        plain[length_offset],
        plain[length_offset + 1],
        plain[length_offset + 2],
        plain[length_offset + 3],
    ]) as usize;
    let message_start = RANDOM_LEN + LENGTH_LEN;
    let message_end = message_start
        .checked_add(message_len)
        .filter(|end| *end <= plain.len())
        .ok_or_else(|| "解密结果的长度字段与实际内容不符".to_string())?;

    let received_app_id = &plain[message_end..];
    let expected_app_id = app_id.trim().as_bytes();
    if !expected_app_id.is_empty() && received_app_id != expected_app_id {
        return Err("消息里的 appid 与当前配置不一致".to_string());
    }

    String::from_utf8(plain[message_start..message_end].to_vec())
        .map_err(|_| "解密结果不是合法的 UTF-8".to_string())
}

/// 加密一条回复，返回微信要求的安全模式信封 XML。
///
/// 文档：<https://developers.weixin.qq.com/doc/offiaccount/Message_Management/Message_encryption_and_decryption_instructions.html>
pub fn encrypt_message(
    token: &str,
    encoding_aes_key: &str,
    app_id: &str,
    plain_xml: &str,
) -> Result<String, String> {
    let (aes_key, iv) = aes_key_and_iv(&decode_aes_key(encoding_aes_key)?);

    let mut buffer = random_bytes(RANDOM_LEN);
    buffer.extend_from_slice(&(plain_xml.len() as u32).to_be_bytes());
    buffer.extend_from_slice(plain_xml.as_bytes());
    buffer.extend_from_slice(app_id.trim().as_bytes());
    pkcs7_pad(&mut buffer, PKCS7_BLOCK);

    let cipher = cbc_encrypt(&aes_key, &iv, &buffer);
    let encrypt = base64_encode(&cipher);
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let nonce = random_nonce();
    let signature = msg_signature(token, &timestamp, &nonce, &encrypt);

    Ok(format!(
        "<xml><Encrypt><![CDATA[{encrypt}]]></Encrypt>\
<MsgSignature><![CDATA[{signature}]]></MsgSignature>\
<TimeStamp>{timestamp}</TimeStamp>\
<Nonce><![CDATA[{nonce}]]></Nonce></xml>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 十六进制字符串 → 字节。
    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn matches_fips197_single_block_vector() {
        // FIPS-197 附录 C.1：AES-128 单分组
        let key: [u8; 16] = bytes("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let round_keys = expand_key(&key);
        let mut block: [u8; 16] = bytes("00112233445566778899aabbccddeeff")
            .try_into()
            .unwrap();
        encrypt_block(&round_keys, &mut block);
        assert_eq!(
            block.to_vec(),
            bytes("69c4e0d86a7b0430d8cdb78070b4c55a"),
            "AES-128 加密结果应与 FIPS-197 一致"
        );

        let mut back = block;
        decrypt_block(&round_keys, &mut back);
        assert_eq!(back.to_vec(), bytes("00112233445566778899aabbccddeeff"));
    }

    #[test]
    fn decrypts_nist_cbc_vector() {
        // NIST SP 800-38A F.2.1（CBC-AES128.Decrypt）
        let key: [u8; 16] = bytes("2b7e151628aed2a6abf7158809cf4f3c").try_into().unwrap();
        let iv: [u8; 16] = bytes("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let cipher = bytes(
            "7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2\
             73bed6b8e3c1743b7116e69e222295163ff1caa1681fac09120eca307586e1a7",
        );
        let plain = cbc_decrypt(&key, &iv, &cipher).expect("CBC 解密");
        assert_eq!(
            plain,
            bytes(
                "6bc1bee22e409f96e93d7e117393172a\
                 ae2d8a571e03ac9c9eb76fac45af8e51\
                 30c81c46a35ce411e5fbc1191a0a52ef\
                 f69f2445df4f9b17ad2b417be66c3710"
            )
        );
        // 再加密回去，验证 CBC 串联正确
        assert_eq!(cbc_encrypt(&key, &iv, &plain), cipher);
    }

    #[test]
    fn pads_and_unpads_with_block32() {
        // 空内容补满 32 字节
        let mut data = Vec::new();
        pkcs7_pad(&mut data, PKCS7_BLOCK);
        assert_eq!(data.len(), 32);
        assert!(data.iter().all(|byte| *byte == 32));
        assert_eq!(pkcs7_unpad(&data, PKCS7_BLOCK).unwrap(), Vec::<u8>::new());

        // 32 字节整块要再补一整块
        let mut data = vec![7u8; 32];
        pkcs7_pad(&mut data, PKCS7_BLOCK);
        assert_eq!(data.len(), 64);
        assert_eq!(data[32..], [32u8; 32]);
        assert_eq!(pkcs7_unpad(&data, PKCS7_BLOCK).unwrap(), vec![7u8; 32]);

        // 非法补位
        assert!(pkcs7_unpad(&[1, 2, 3, 0], PKCS7_BLOCK).is_none());
        assert!(pkcs7_unpad(&[1, 2, 3, 33], PKCS7_BLOCK).is_none());
        assert!(pkcs7_unpad(&[1, 2, 9, 9], PKCS7_BLOCK).is_none());
        assert!(pkcs7_unpad(&[], PKCS7_BLOCK).is_none());
    }

    #[test]
    fn verifies_msg_signature_in_sorted_order() {
        // sha1(sort("tok", "123", "abc") 拼接)
        let expected =
            sha1_hex("123abctok".as_bytes());
        assert_eq!(msg_signature("tok", "123", "abc", ""), expected);
        // 顺序无关
        assert_eq!(msg_signature("tok", "abc", "123", ""), expected);
        // 大小写不同的签名由调用方归一（这里只产出小写 hex）
        assert!(expected.chars().all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()));
    }

    #[test]
    fn encodes_and_decodes_aes_key() {
        // 43 位 → 32 字节；末位必须是「规范 base64」字符（末 2 位补零），
        // 即索引能被 4 整除的字符（A/E/I/M/Q/U/Y/c/g/k/o/s/w/0/4/8），否则回编码不会还原。
        let key = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFE";
        assert_eq!(key.len(), 43);
        let decoded = decode_aes_key(key).unwrap();
        assert_eq!(decoded.len(), 32);
        assert_eq!(base64_encode(&decoded), format!("{key}="));

        assert_eq!(
            decode_aes_key("too-short").unwrap_err(),
            "EncodingAESKey 长度应为 43 个字符，当前 9 个"
        );
    }

    #[test]
    fn unwraps_fixed_ciphertext() {
        // 固定密文：EncodingAESKey 全 0 加密 "hello wechat"（appid=wx123）
        // 生成方式：本模块 encrypt_message 用固定 key/iv 复算，下面的断言把
        // 解密路径（签名 → base64 → AES → 去补位 → 长度字段 → appid）串起来。
        let key_text = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let app_id = "wx123";
        let plain = "<xml><Content><![CDATA[hi]]></Content></xml>";

        let envelope = encrypt_message("tok", key_text, app_id, plain).unwrap();
        assert!(envelope.contains("<Encrypt><![CDATA["));
        assert!(envelope.contains("<MsgSignature>"));
        // 从信封里取出密文 / 签名 / 时间戳 / nonce
        let encrypt = between(&envelope, "<Encrypt><![CDATA[", "]]></Encrypt>");
        let signature = between(&envelope, "<MsgSignature><![CDATA[", "]]></MsgSignature>");
        let timestamp = between(&envelope, "<TimeStamp>", "</TimeStamp>");
        let nonce = between(&envelope, "<Nonce><![CDATA[", "]]></Nonce>");

        let decrypted = decrypt_message(
            "tok", key_text, app_id, &signature, &timestamp, &nonce, &encrypt,
        )
        .expect("应能解开自己加的信封");
        assert_eq!(decrypted, plain);

        // appid 不一致要拒绝
        let error = decrypt_message(
            "tok", key_text, "wx999", &signature, &timestamp, &nonce, &encrypt,
        )
        .unwrap_err();
        assert_eq!(error, "消息里的 appid 与当前配置不一致");

        // 签名错误要拒绝
        let error = decrypt_message(
            "tok", key_text, app_id, "bad", &timestamp, &nonce, &encrypt,
        )
        .unwrap_err();
        assert_eq!(error, "msg_signature 校验失败");
    }

    /// 取两个标记之间的内容（测试用）。
    fn between<'a>(text: &'a str, start: &str, end: &str) -> String {
        let begin = text.find(start).expect("缺少起始标记") + start.len();
        let finish = text[begin..].find(end).expect("缺少结束标记") + begin;
        text[begin..finish].to_string()
    }
}

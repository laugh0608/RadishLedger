//! M0-C 固定候选原语；运行参数和公开算法向量的入口严格分开。
use crate::{CoreError, Result};
use argon2::{Algorithm, Argon2, Block, Params, Version};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305,
    aead::{Aead, Payload},
};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Mutex;
use zeroize::Zeroizing;

pub type Secret = Zeroizing<[u8; 32]>;
static KDF: Mutex<()> = Mutex::new(());

pub fn error() -> CoreError {
    CoreError::new(
        "authentication_failed",
        "解锁材料错误或封装已损坏，原文件保留",
    )
}

pub fn random<const N: usize>() -> Result<Zeroizing<[u8; N]>> {
    let mut bytes = Zeroizing::new([0; N]);
    getrandom::fill(bytes.as_mut())
        .map_err(|_| CoreError::new("random_unavailable", "系统随机源不可用"))?;
    Ok(bytes)
}

pub fn secret(bytes: &[u8]) -> Result<Secret> {
    if bytes.len() != 32 {
        return Err(error());
    }
    let mut key = Zeroizing::new([0; 32]);
    key.copy_from_slice(bytes);
    Ok(key)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub memory_kib: u32,
    pub iterations: u32,
    pub lanes: u32,
    pub version: u32,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            memory_kib: 65536,
            iterations: 3,
            lanes: 1,
            version: 19,
        }
    }
}
impl Profile {
    pub fn validate(&self) -> Result<()> {
        if self.memory_kib != 65536 || self.iterations != 3 || self.lanes != 1 || self.version != 19
        {
            return Err(CoreError::new(
                "unsupported_kdf",
                "KDF 参数不在已允许的实验 profile 内",
            ));
        }
        Ok(())
    }
    pub fn encode(&self, output: &mut Vec<u8>) {
        for n in [self.memory_kib, self.iterations, self.lanes, self.version] {
            output.extend_from_slice(&n.to_be_bytes());
        }
    }
}

pub fn password_key(password: &[u8], salt: &[u8; 16], profile: &Profile) -> Result<Secret> {
    profile.validate()?;
    if password.is_empty() || password.len() > 1024 || std::str::from_utf8(password).is_err() {
        return Err(CoreError::new(
            "invalid_password",
            "口令必须是 1 至 1024 字节的 UTF-8",
        ));
    }
    // 不排队积压昂贵 KDF；所有 bridge / isolate 共用一把有界执行锁。
    let _guard = KDF
        .try_lock()
        .map_err(|_| CoreError::new("kdf_busy", "已有口令派生任务尚未结束"))?;
    let params = Params::new(
        profile.memory_kib,
        profile.iterations,
        profile.lanes,
        Some(32),
    )
    .map_err(|_| error())?;
    let count = params.block_count();
    let mut blocks = Vec::new();
    blocks
        .try_reserve_exact(count)
        .map_err(|_| CoreError::new("resource_unavailable", "KDF 工作内存不可用"))?;
    blocks.resize(count, Block::default());
    let mut blocks = Zeroizing::new(blocks);
    let mut output = Zeroizing::new([0; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into_with_memory(password, salt, output.as_mut(), blocks.as_mut_slice())
        .map_err(|_| error())?;
    derive(output.as_ref(), salt, b"radishledger/m0c/v1/password-wrap")
}

pub fn derive(key: &[u8], salt: &[u8], purpose: &[u8]) -> Result<Secret> {
    let mut output = Zeroizing::new([0; 32]);
    Hkdf::<Sha256>::new(Some(salt), key)
        .expand(purpose, output.as_mut())
        .map_err(|_| error())?;
    Ok(output)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
}
impl Envelope {
    pub fn validate(&self) -> Result<()> {
        if self.ciphertext.len() != 48 {
            return Err(error());
        }
        Ok(())
    }
}

pub fn seal(key: &[u8], input: &[u8; 32], aad: &[u8]) -> Result<Envelope> {
    let nonce = *random::<24>()?;
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| error())?;
    let ciphertext = cipher
        .encrypt((&nonce).into(), Payload { msg: input, aad })
        .map_err(|_| error())?;
    Ok(Envelope { nonce, ciphertext })
}

pub fn unseal(key: &[u8], envelope: &Envelope, aad: &[u8]) -> Result<Secret> {
    envelope.validate()?;
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| error())?;
    let clear = Zeroizing::new(
        cipher
            .decrypt(
                (&envelope.nonce).into(),
                Payload {
                    msg: &envelope.ciphertext,
                    aad,
                },
            )
            .map_err(|_| error())?,
    );
    secret(&clear)
}

pub fn hex(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(ALPHABET[(byte >> 4) as usize] as char);
        output.push(ALPHABET[(byte & 15) as usize] as char);
    }
    output
}

pub fn fingerprint(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// 可打印恢复材料是显式显示动作；本返回值不得写日志或默认复制。
pub fn recovery_text(key: &[u8]) -> Result<Zeroizing<String>> {
    if key.len() != 32 {
        return Err(error());
    }
    let mut text = Zeroizing::new(String::from("RLM0C1-"));
    // 避免在临时普通 String 中复制完整秘密。
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for byte in key {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 15) as usize] as char);
    }
    let checksum = fingerprint(text.as_bytes());
    text.push('-');
    text.push_str(&checksum[..8]);
    Ok(text)
}

pub fn parse_recovery(text: &[u8]) -> Result<Secret> {
    if text.len() != 80 || &text[..7] != b"RLM0C1-" || text[71] != b'-' {
        return Err(CoreError::new(
            "invalid_recovery",
            "恢复材料的版本、长度或校验无效",
        ));
    }
    let checksum = fingerprint(&text[..71]);
    if text[72..] != checksum.as_bytes()[..8] {
        return Err(CoreError::new("invalid_recovery", "恢复材料校验失败"));
    }
    let mut output = Zeroizing::new([0; 32]);
    for (i, pair) in text[7..71].chunks_exact(2).enumerate() {
        let digit = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        output[i] = (digit(pair[0]).ok_or_else(error)? << 4) | digit(pair[1]).ok_or_else(error)?;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn unhex(text: &str) -> Vec<u8> {
        text.as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }
    #[test]
    fn independent_vectors_and_authentication() {
        // RFC 5869 Appendix A.1；预期直接来自 RFC，不由本实现生成。
        let mut output = [0; 42];
        Hkdf::<Sha256>::new(Some(&unhex("000102030405060708090a0b0c")), &[0x0b; 22])
            .expand(&unhex("f0f1f2f3f4f5f6f7f8f9"), &mut output)
            .unwrap();
        assert_eq!(
            output.as_slice(),
            unhex(
                "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
            )
        );
        // RFC 9106 §5.3；仅此测试接受 32 KiB / 4 lanes。
        let mut builder = argon2::ParamsBuilder::new();
        builder
            .m_cost(32)
            .t_cost(3)
            .p_cost(4)
            .output_len(32)
            .data(argon2::AssociatedData::new(&[4; 12]).unwrap());
        let argon = Argon2::new_with_secret(
            &[3; 8],
            Algorithm::Argon2id,
            Version::V0x13,
            builder.build().unwrap(),
        )
        .unwrap();
        let mut output = [0; 32];
        argon
            .hash_password_into(&[1; 32], &[2; 16], &mut output)
            .unwrap();
        assert_eq!(
            output.as_slice(),
            unhex("0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659")
        );
        // draft-irtf-cfrg-xchacha-03 Appendix A.1（公开算法材料）。
        let key: Vec<u8> = (0x80..=0x9f).collect();
        let nonce: [u8; 24] = (0x40..=0x57).collect::<Vec<_>>().try_into().unwrap();
        let aad = unhex("50515253c0c1c2c3c4c5c6c7");
        let plaintext = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let expected = unhex(concat!(
            "bd6d179d3e83d43b9576579493c0e939572a1700252bfaccbed2902c21396cbb",
            "731c7f1b0b4aa6440bf3a82f4eda7e39ae64c6708c54c216cb96b72e1213b452",
            "2f8c9ba40db5d945b11b69b982c1bb9e3f3fac2bc369488f76b2383565d3fff9",
            "21f9664c97637da9768812f615c68b13b52ec0875924c1c7987947deafd8780acf49"
        ));
        let cipher = XChaCha20Poly1305::new_from_slice(&key).unwrap();
        assert_eq!(
            cipher
                .encrypt(
                    (&nonce).into(),
                    Payload {
                        msg: plaintext,
                        aad: &aad
                    }
                )
                .unwrap(),
            expected
        );
        assert_eq!(
            cipher
                .decrypt(
                    (&nonce).into(),
                    Payload {
                        msg: &expected,
                        aad: &aad
                    }
                )
                .unwrap(),
            plaintext
        );
        for index in 0..expected.len() {
            let mut damaged = expected.clone();
            damaged[index] ^= 1;
            assert!(
                cipher
                    .decrypt(
                        (&nonce).into(),
                        Payload {
                            msg: &damaged,
                            aad: &aad
                        }
                    )
                    .is_err()
            );
        }
        assert!(
            cipher
                .decrypt(
                    (&nonce).into(),
                    Payload {
                        msg: &expected,
                        aad: b"wrong"
                    }
                )
                .is_err()
        );
        assert_ne!(
            *derive(&key, b"salt", b"recovery").unwrap(),
            *derive(&key, b"salt", b"database").unwrap()
        );
    }
    #[test]
    fn recovery_checksum_and_resource_bounds() {
        let key = random::<32>().unwrap();
        let mut text = recovery_text(key.as_ref()).unwrap();
        assert!(*parse_recovery(text.as_bytes()).unwrap() == *key);
        let replacement = if &text[79..] == "0" { "1" } else { "0" };
        text.replace_range(79..80, replacement);
        assert!(parse_recovery(text.as_bytes()).is_err());
        for memory_kib in [0, 8, 32, 65535, 65537, u32::MAX] {
            assert_eq!(
                password_key(
                    b"test",
                    &[0; 16],
                    &Profile {
                        memory_kib,
                        ..Profile::default()
                    }
                )
                .err()
                .unwrap()
                .code,
                "unsupported_kdf"
            );
        }
    }
}

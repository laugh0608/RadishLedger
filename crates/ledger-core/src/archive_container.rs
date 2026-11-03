//! M0-E 已批准的整体认证宿主实验；只处理内存，不写文件或激活恢复账库。
use crate::crypto::{self, KdfPoint, Profile, Secret};
use crate::logical_snapshot::{Contents, Facts, LogicalSnapshot};
use crate::{CoreError, Result};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, aead::AeadInOut};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use zeroize::Zeroizing;

#[path = "archive_json.rs"]
mod json;
#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;

pub const MAX_PAYLOAD: usize = 16 * 1024 * 1024;
const HEADER: usize = 176;
const MANIFEST: usize = 116;
const BODY_OVERHEAD: usize = 136;
const OVERHEAD: usize = 328;
const WRAP_INFO: &[u8] = b"radishledger/archive/review1/password-wrap";
const BODY_INFO: &[u8] = b"radishledger/archive/review1/content-key";
const WRAP_AAD: &[u8] = b"radishledger/archive/review1/wrap\0";
const BODY_AAD: &[u8] = b"radishledger/archive/review1/content\0";

#[derive(Clone, Copy)]
pub struct Limits {
    pub payload_bytes: usize,
    pub records: usize,
    pub json_depth: usize,
    pub string_bytes: usize,
    pub decoded_string_bytes: usize,
    pub collection_entries: usize,
}

pub const CANDIDATE_LIMITS: Limits = Limits {
    payload_bytes: MAX_PAYLOAD,
    records: 100_000,
    json_depth: 64,
    string_bytes: 65_536,
    decoded_string_bytes: MAX_PAYLOAD,
    collection_entries: 1_000_000,
};

impl Limits {
    fn validate(self) -> Result<()> {
        if !(1..=MAX_PAYLOAD).contains(&self.payload_bytes)
            || self.records > 100_000
            || !(1..=64).contains(&self.json_depth)
            || !(1..=65_536).contains(&self.string_bytes)
            || !(1..=MAX_PAYLOAD).contains(&self.decoded_string_bytes)
            || !(1..=1_000_000).contains(&self.collection_entries)
        {
            return Err(limit());
        }
        Ok(())
    }
}

/// 密文仅通过显式借用交付；不提供普通 Debug / Serialize。
pub struct SealedArchive(Zeroizing<Vec<u8>>);
impl SealedArchive {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// 只包含完整验证后的逻辑快照，没有旧域运行状态或恢复写入能力。
pub struct OpenedArchive {
    id: [u8; 16],
    contents: Contents,
}
impl OpenedArchive {
    pub fn archive_id(&self) -> &[u8; 16] {
        &self.id
    }
    pub fn contents(&self) -> &Contents {
        &self.contents
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Point {
    BeforeKdf,
    KdfAcquired,
    KdfHashed,
    AfterWrap,
    AfterContent,
    AfterJson,
    Complete,
}

fn invalid() -> CoreError {
    CoreError::new("invalid_archive", "归档结构或长度无效，原数据保留")
}
pub(super) fn payload_error() -> CoreError {
    CoreError::new(
        "invalid_archive_payload",
        "归档已认证，但载荷或业务关系无效",
    )
}
pub(super) fn limit() -> CoreError {
    CoreError::new("archive_limit", "归档超过本次实验资源预算")
}
fn resource() -> CoreError {
    CoreError::new("resource_unavailable", "归档工作内存不可用")
}
fn unlock_error() -> CoreError {
    CoreError::new("archive_unlock_failed", "口令不正确或归档损坏")
}
fn content_error() -> CoreError {
    CoreError::new(
        "archive_content_auth_failed",
        "归档正文认证失败，未返回明文",
    )
}
fn cancelled(flag: &AtomicBool) -> Result<()> {
    if flag.load(Ordering::Relaxed) {
        Err(CoreError::new("cancelled", "归档处理已取消，未发布结果"))
    } else {
        Ok(())
    }
}

fn buffer(size: usize) -> Result<Zeroizing<Vec<u8>>> {
    let mut output = Vec::new();
    output.try_reserve_exact(size).map_err(|_| resource())?;
    Ok(Zeroizing::new(output))
}

fn copy(input: &[u8], extra: usize) -> Result<Zeroizing<Vec<u8>>> {
    let mut output = buffer(input.len().checked_add(extra).ok_or_else(limit)?)?;
    output.extend_from_slice(input);
    Ok(output)
}

fn aad(label: &[u8], header: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let mut output = copy(label, header.len())?;
    output.extend_from_slice(header);
    Ok(output)
}

fn encrypt(key: &Secret, nonce: &[u8; 24], clear: &mut Vec<u8>, associated: &[u8]) -> Result<()> {
    clear.try_reserve_exact(16).map_err(|_| resource())?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).map_err(|_| invalid())?;
    let tag = cipher
        .encrypt_inout_detached(nonce.into(), associated, clear.as_mut_slice().into())
        .map_err(|_| invalid())?;
    clear.extend_from_slice(&tag);
    Ok(())
}

fn decrypt(
    key: &Secret,
    nonce: &[u8; 24],
    data: &mut Vec<u8>,
    associated: &[u8],
    failure: CoreError,
) -> Result<()> {
    let split = data.len().checked_sub(16).ok_or_else(|| failure.clone())?;
    let tag: [u8; 16] = data[split..].try_into().map_err(|_| failure.clone())?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).map_err(|_| failure.clone())?;
    cipher
        .decrypt_inout_detached(
            nonce.into(),
            associated,
            (&mut data[..split]).into(),
            (&tag).into(),
        )
        .map_err(|_| failure)?;
    data.truncate(split);
    Ok(())
}

struct Header {
    prefix: [u8; 128],
}
impl Header {
    fn new(n: usize, entropy: &Entropy) -> Self {
        let mut p = [0_u8; 128];
        p[..8].copy_from_slice(b"RLM0E001");
        p[10..12].copy_from_slice(&1_u16.to_be_bytes());
        p[12..16].copy_from_slice(&(HEADER as u32).to_be_bytes());
        p[16..18].copy_from_slice(&1_u16.to_be_bytes());
        p[18..20].copy_from_slice(&1_u16.to_be_bytes());
        p[24..40].copy_from_slice(&entropy.id);
        p[40..56].copy_from_slice(&entropy.salt);
        for (at, value) in [(56, 19_u32), (60, 65536), (64, 3), (68, 1)] {
            p[at..at + 4].copy_from_slice(&value.to_be_bytes());
        }
        p[72..96].copy_from_slice(&entropy.wrap_nonce);
        p[96..120].copy_from_slice(&entropy.body_nonce);
        p[120..128].copy_from_slice(&((n + 152) as u64).to_be_bytes());
        Self { prefix: p }
    }

    fn read(input: &[u8], limits: Limits) -> Result<Self> {
        if input.len() < HEADER || &input[..8] != b"RLM0E001" {
            return Err(invalid());
        }
        let p: [u8; 128] = input[..128].try_into().map_err(|_| invalid())?;
        if u16::from_be_bytes(p[8..10].try_into().map_err(|_| invalid())?) != 0
            || u16::from_be_bytes(p[10..12].try_into().map_err(|_| invalid())?) != 1
            || u16::from_be_bytes(p[16..18].try_into().map_err(|_| invalid())?) != 1
            || p[20..24] != [0; 4]
        {
            return Err(CoreError::new(
                "unsupported_archive",
                "归档版本、算法或 flags 不受支持",
            ));
        }
        if u32::from_be_bytes(p[12..16].try_into().map_err(|_| invalid())?) != HEADER as u32 {
            return Err(invalid());
        }
        if p[18..20] != 1_u16.to_be_bytes() {
            return Err(CoreError::new(
                "unsupported_kdf",
                "归档 KDF profile 不受支持",
            ));
        }
        let profile = Profile {
            version: read_u32(&p, 56)?,
            memory_kib: read_u32(&p, 60)?,
            iterations: read_u32(&p, 64)?,
            lanes: read_u32(&p, 68)?,
        };
        profile.validate()?;
        let length = read_u64(&p, 120)?;
        if length < 153 || length > limits.payload_bytes as u64 + 152 {
            return Err(limit());
        }
        let total = length.checked_add(HEADER as u64).ok_or_else(limit)?;
        if total != input.len() as u64 {
            return Err(invalid());
        }
        Ok(Self { prefix: p })
    }
    fn id(&self) -> [u8; 16] {
        self.prefix[24..40].try_into().expect("fixed header field")
    }
    fn salt(&self) -> [u8; 16] {
        self.prefix[40..56].try_into().expect("fixed header field")
    }
    fn wrap_nonce(&self) -> [u8; 24] {
        self.prefix[72..96].try_into().expect("fixed header field")
    }
    fn body_nonce(&self) -> [u8; 24] {
        self.prefix[96..120].try_into().expect("fixed header field")
    }
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(
        bytes
            .get(at..at + 4)
            .ok_or_else(invalid)?
            .try_into()
            .map_err(|_| invalid())?,
    ))
}
fn read_u64(bytes: &[u8], at: usize) -> Result<u64> {
    Ok(u64::from_be_bytes(
        bytes
            .get(at..at + 8)
            .ok_or_else(invalid)?
            .try_into()
            .map_err(|_| invalid())?,
    ))
}

struct Entropy {
    id: [u8; 16],
    salt: [u8; 16],
    wrap_nonce: [u8; 24],
    body_nonce: [u8; 24],
    key: Secret,
}
impl Entropy {
    fn random() -> Result<Self> {
        Ok(Self {
            id: *crypto::random::<16>()?,
            salt: *crypto::random::<16>()?,
            wrap_nonce: *crypto::random::<24>()?,
            body_nonce: *crypto::random::<24>()?,
            key: crypto::random::<32>()?,
        })
    }
}

fn counts(f: &Facts, limits: Limits) -> Result<[u32; 13]> {
    let mut sizes = [
        f.accounts.len(),
        f.categories.len(),
        f.revisions.len(),
        f.selected.len(),
        f.candidates.len(),
        f.intent_history.len(),
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    if let Some(i) = &f.imports {
        sizes[6..].copy_from_slice(&[
            i.tasks.len(),
            i.rows.len(),
            i.sources.len(),
            i.batches.len(),
            i.undo_tasks.len(),
            i.undo_rows.len(),
            i.undo_batches.len(),
        ]);
    }
    let total = sizes
        .iter()
        .try_fold(0_usize, |n, value| n.checked_add(*value))
        .ok_or_else(limit)?;
    if total > limits.records {
        return Err(limit());
    }
    let mut output = [0_u32; 13];
    for (out, value) in output.iter_mut().zip(sizes) {
        *out = u32::try_from(value).map_err(|_| limit())?;
    }
    Ok(output)
}

fn manifest(facts: &Facts, payload: &[u8], limits: Limits) -> Result<[u8; MANIFEST]> {
    let mut output = [0_u8; MANIFEST];
    output[..8].copy_from_slice(b"RLM0MAN1");
    output[8..10].copy_from_slice(&1_u16.to_be_bytes());
    output[10..12].copy_from_slice(&1_u16.to_be_bytes());
    output[12..16].copy_from_slice(&(facts.source_schema as u32).to_be_bytes());
    output[16..24].copy_from_slice(&(facts.sequence as u64).to_be_bytes());
    for (n, value) in counts(facts, limits)?.iter().enumerate() {
        output[24 + 4 * n..28 + 4 * n].copy_from_slice(&value.to_be_bytes());
    }
    output[76..84].copy_from_slice(&(payload.len() as u64).to_be_bytes());
    output[84..].copy_from_slice(&Sha256::digest(payload));
    Ok(output)
}

fn check_manifest(bytes: &[u8], payload: &[u8], limits: Limits) -> Result<()> {
    if bytes.len() != MANIFEST
        || &bytes[..8] != b"RLM0MAN1"
        || bytes[8..10] != 1_u16.to_be_bytes()
        || bytes[10..12] != 1_u16.to_be_bytes()
        || !matches!(read_u32(bytes, 12)?, 1..=3)
        || read_u64(bytes, 16)? > i64::MAX as u64
        || read_u64(bytes, 76)? != payload.len() as u64
        || bytes[84..] != Sha256::digest(payload)[..]
    {
        return Err(payload_error());
    }
    let mut total = 0_usize;
    for n in 0..13 {
        total = total
            .checked_add(read_u32(bytes, 24 + 4 * n)? as usize)
            .ok_or_else(limit)?;
    }
    if total > limits.records {
        return Err(limit());
    }
    Ok(())
}

fn key(
    password: &[u8],
    header: &Header,
    cancel: &AtomicBool,
    observe: &mut impl FnMut(Point) -> Result<()>,
) -> Result<Secret> {
    observe(Point::BeforeKdf)?;
    cancelled(cancel)?;
    let raw = crypto::password_material(password, &header.salt(), &Profile::default(), |point| {
        observe(match point {
            KdfPoint::Acquired => Point::KdfAcquired,
            KdfPoint::Hashed => Point::KdfHashed,
        })?;
        cancelled(cancel)
    })?;
    crypto::derive(raw.as_ref(), &header.id(), WRAP_INFO)
}

pub fn seal(
    snapshot: &LogicalSnapshot,
    password: &[u8],
    limits: Limits,
    cancel: &AtomicBool,
) -> Result<SealedArchive> {
    limits.validate()?;
    cancelled(cancel)?;
    seal_with(
        snapshot,
        password,
        limits,
        cancel,
        &Entropy::random()?,
        |_| Ok(()),
    )
}

fn seal_with(
    snapshot: &LogicalSnapshot,
    password: &[u8],
    limits: Limits,
    cancel: &AtomicBool,
    entropy: &Entropy,
    mut observe: impl FnMut(Point) -> Result<()>,
) -> Result<SealedArchive> {
    limits.validate()?;
    cancelled(cancel)?;
    crypto::validate_password(password)?;
    let payload = Zeroizing::new(snapshot.encode(limits.payload_bytes, cancel).map_err(
        |error| {
            if error.code == "snapshot_limit" {
                limit()
            } else {
                error
            }
        },
    )?);
    json::scan(&payload, limits, cancel)?;
    let m = manifest(&snapshot.contents().facts, &payload, limits)?;
    let mut body = buffer(payload.len() + BODY_OVERHEAD + 16)?;
    body.extend_from_slice(&(MANIFEST as u32).to_be_bytes());
    body.extend_from_slice(&m);
    body.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    body.extend_from_slice(&payload);
    body.extend_from_slice(b"RLM0END1");
    let header = Header::new(payload.len(), entropy);
    let wrap_key = key(password, &header, cancel, &mut observe)?;
    let mut wrapped = copy(entropy.key.as_ref(), 16)?;
    encrypt(
        &wrap_key,
        &header.wrap_nonce(),
        &mut wrapped,
        &aad(WRAP_AAD, &header.prefix)?,
    )?;
    let mut output = copy(&header.prefix, wrapped.len() + body.len() + 16)?;
    output.extend_from_slice(&wrapped);
    observe(Point::AfterWrap)?;
    cancelled(cancel)?;
    let body_key = crypto::derive(entropy.key.as_ref(), &header.id(), BODY_INFO)?;
    encrypt(
        &body_key,
        &header.body_nonce(),
        &mut body,
        &aad(BODY_AAD, &output)?,
    )?;
    observe(Point::AfterContent)?;
    cancelled(cancel)?;
    output.extend_from_slice(&body);
    observe(Point::Complete)?;
    cancelled(cancel)?;
    Ok(SealedArchive(output))
}

pub fn open(
    input: &[u8],
    password: &[u8],
    limits: Limits,
    cancel: &AtomicBool,
) -> Result<OpenedArchive> {
    open_observed(input, password, limits, cancel, |_| Ok(()))
}

fn open_observed(
    input: &[u8],
    password: &[u8],
    limits: Limits,
    cancel: &AtomicBool,
    mut observe: impl FnMut(Point) -> Result<()>,
) -> Result<OpenedArchive> {
    limits.validate()?;
    cancelled(cancel)?;
    if input.len() > limits.payload_bytes + OVERHEAD {
        return Err(limit());
    }
    let header = Header::read(input, limits)?;
    let wrap_key = key(password, &header, cancel, &mut observe)?;
    let mut wrapped = copy(&input[128..HEADER], 0)?;
    decrypt(
        &wrap_key,
        &header.wrap_nonce(),
        &mut wrapped,
        &aad(WRAP_AAD, &header.prefix)?,
        unlock_error(),
    )?;
    let archive_key = crypto::secret(&wrapped).map_err(|_| unlock_error())?;
    observe(Point::AfterWrap)?;
    cancelled(cancel)?;
    let body_key = crypto::derive(archive_key.as_ref(), &header.id(), BODY_INFO)?;
    let mut body = copy(&input[HEADER..], 0)?;
    decrypt(
        &body_key,
        &header.body_nonce(),
        &mut body,
        &aad(BODY_AAD, &input[..HEADER])?,
        content_error(),
    )?;
    observe(Point::AfterContent)?;
    cancelled(cancel)?;
    let n = body
        .len()
        .checked_sub(BODY_OVERHEAD)
        .ok_or_else(payload_error)?;
    if n == 0
        || n > limits.payload_bytes
        || read_u32(&body, 0)? != MANIFEST as u32
        || read_u64(&body, 120)? != n as u64
        || &body[128 + n..] != b"RLM0END1"
    {
        return Err(payload_error());
    }
    let payload = &body[128..128 + n];
    check_manifest(&body[4..120], payload, limits)?;
    let contents = json::decode(payload, limits, cancel)?;
    if body[4..120] != manifest(&contents.facts, payload, limits)? {
        return Err(payload_error());
    }
    observe(Point::AfterJson)?;
    cancelled(cancel)?;
    observe(Point::Complete)?;
    cancelled(cancel)?;
    Ok(OpenedArchive {
        id: header.id(),
        contents,
    })
}

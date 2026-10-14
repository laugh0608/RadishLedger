//! M0-C 的合成账库生命周期。所有解密先认证；失效、取消和失败不重建旧库。
use crate::bootstrap_files::{Files, identity, io_error};
use crate::crypto::{self, Envelope, Profile, Secret};
use crate::store::{DbKey, Store};
use crate::{CoreError, Result, random_id};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    format: u32,
    vault: String,
    device: String,
    salt: [u8; 16],
    profile: Profile,
    password: Envelope,
    recovery: Envelope,
    database: Envelope,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalProtection {
    generation: String,
    root: Envelope,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum Unlock {
    Password,
    Recovery,
    System,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Point {
    EnvelopesSaved,
    DatabaseStarted,
    DatabaseReady,
    ReadyPublished,
    ProtectionPrepared,
    ProtectionPublished,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub abi: u32,
    pub directory: PathBuf,
    pub run: String,
    pub action: Action,
    #[serde(default)]
    pub pause_at: Option<Point>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Action {
    NewRecovery,
    Create,
    Resume { unlock: Unlock },
    Confirm,
    ReissueRecovery,
    Seed { unlock: Unlock },
    Inspect { unlock: Unlock },
    PrepareProtection { unlock: Unlock },
    CommitProtection { generation: String },
    Status,
    Cleanup,
}

/// 秘密单独返回，不能序列化到普通报告或日志。桥接层负责有界转交和清理。
pub struct Reply {
    pub report: Value,
    pub secret: Zeroizing<Vec<u8>>,
}
impl Reply {
    fn public(report: Value) -> Self {
        Self {
            report,
            secret: Zeroizing::new(Vec::new()),
        }
    }
    fn secret(report: Value, bytes: &[u8]) -> Self {
        Self {
            report,
            secret: Zeroizing::new(bytes.to_vec()),
        }
    }
}

fn format_error() -> CoreError {
    CoreError::new("invalid_bootstrap", "bootstrap 格式缺失或损坏，原文件保留")
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| format_error())
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|_| format_error())
}

fn aad(
    vault: &str,
    device: &str,
    salt: &[u8; 16],
    profile: &Profile,
    purpose: &str,
    generation: &str,
) -> Vec<u8> {
    let mut output = b"RadishLedger/M0C/XChaCha20Poly1305/v1".to_vec();
    for field in [vault, device, purpose, generation] {
        output.extend_from_slice(&(field.len() as u32).to_be_bytes());
        output.extend_from_slice(field.as_bytes());
    }
    output.extend_from_slice(salt);
    profile.encode(&mut output);
    output
}
impl Bootstrap {
    fn aad(&self, purpose: &str, generation: &str) -> Vec<u8> {
        aad(
            &self.vault,
            &self.device,
            &self.salt,
            &self.profile,
            purpose,
            generation,
        )
    }
    fn load(files: &Files) -> Result<Self> {
        let b: Self = decode(&files.read("bootstrap.json")?)?;
        if b.format != 1 {
            return Err(CoreError::new(
                "unsupported_bootstrap",
                "bootstrap 版本不受支持",
            ));
        }
        identity(&b.vault)?;
        identity(&b.device)?;
        b.profile.validate()?;
        b.password.validate()?;
        b.recovery.validate()?;
        b.database.validate()?;
        Ok(b)
    }
    fn root(&self, files: &Files, unlock: Unlock, input: &[u8]) -> Result<Secret> {
        match unlock {
            Unlock::Password => {
                let key = crypto::password_key(input, &self.salt, &self.profile)?;
                crypto::unseal(key.as_ref(), &self.password, &self.aad("password-root", ""))
            }
            Unlock::Recovery => {
                let recovery = crypto::parse_recovery(input)?;
                let key = crypto::derive(
                    recovery.as_ref(),
                    &self.salt,
                    b"radishledger/m0c/v1/recovery-wrap",
                )?;
                crypto::unseal(key.as_ref(), &self.recovery, &self.aad("recovery-root", ""))
            }
            Unlock::System => {
                let generation = active_generation(files)?
                    .ok_or_else(|| CoreError::new("protection_missing", "尚无已完成的日常保护"))?;
                self.local_root(files, &generation, input)
            }
        }
    }
    fn local_root(&self, files: &Files, generation: &str, input: &[u8]) -> Result<Secret> {
        identity(generation)?;
        let local: LocalProtection = decode(&files.read(&format!("local-{generation}.json"))?)?;
        if local.generation != generation {
            return Err(crypto::error());
        }
        let key = crypto::derive(
            crypto::secret(input)?.as_ref(),
            &self.salt,
            b"radishledger/m0c/v1/local-wrap",
        )?;
        crypto::unseal(
            key.as_ref(),
            &local.root,
            &self.aad("local-root", generation),
        )
    }
    fn db_key(&self, root: &Secret) -> Result<DbKey> {
        let key = crypto::derive(
            root.as_ref(),
            &self.salt,
            b"radishledger/m0c/v1/database-wrap",
        )?;
        let bytes = crypto::unseal(key.as_ref(), &self.database, &self.aad("database", ""))?;
        DbKey::from_secret(bytes.as_ref())
    }
    fn open(&self, files: &Files, root: &Secret, writable: bool) -> Result<Store> {
        let path = files.path.join("ledger.db");
        if !files.exists("ledger.db")? {
            return Err(CoreError::new(
                "missing_database",
                "原账库文件不存在，材料保留",
            ));
        }
        crate::bootstrap_files::regular(&path)?;
        let key = self.db_key(root)?;
        // 写入入口也必须先完成只读身份核对。
        let store = Store::open_readonly(&path, &key)?;
        store.verify_bootstrap(&self.vault, &self.device)?;
        if !writable {
            return Ok(store);
        }
        drop(store);
        Store::open(&path, &key)
    }
    fn ready(&self, files: &Files, root: &Secret) -> Result<bool> {
        if !files.exists("ready.json")? {
            return Ok(false);
        }
        let envelope = decode(&files.read("ready.json")?)?;
        let key = crypto::derive(root.as_ref(), &self.salt, b"radishledger/m0c/v1/ready")?;
        if *crypto::unseal(key.as_ref(), &envelope, &self.aad("ready", ""))? != [0x52; 32] {
            return Err(crypto::error());
        }
        Ok(true)
    }
    fn inspect(&self, files: &Files, root: &Secret) -> Result<Value> {
        let ready = self.ready(files, root)?;
        let store = self.open(files, root, false)?;
        let digest = store.content_digest()?;
        let expected = store.fixture_digest()?;
        if expected
            .as_ref()
            .is_some_and(|expected| expected != &digest)
        {
            return Err(CoreError::new(
                "fixture_mismatch",
                "恢复后的完整合成账务内容与已核对基线不符",
            ));
        }
        Ok(
            json!({"vault":self.vault,"device":self.device,"ready":ready,
            "fixtureComplete":expected.is_some(),"contentDigest":digest,
            "audit":store.validation_audit()?,"cipherVersion":store.cipher_version()?,
            "activeProtection":active_generation(files)?,"pid":std::process::id()}),
        )
    }
}

fn active_generation(files: &Files) -> Result<Option<String>> {
    if !files.exists("active-protection.json")? {
        return Ok(None);
    }
    let generation: String = decode(&files.read("active-protection.json")?)?;
    identity(&generation)?;
    Ok(Some(generation))
}

fn point(files: &Files, actual: Point, requested: Option<Point>) -> Result<()> {
    if Some(actual) != requested {
        return Ok(());
    }
    files.publish(
        "paused.json",
        &encode(&json!({"point":actual,"pid":std::process::id()}))?,
        true,
    )?;
    // 仅 M0 合成强杀入口。失败也有有界退出，不留无限后台工作。
    let deadline = Instant::now() + Duration::from_secs(45);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(CoreError::new(
        "fault_timeout",
        "强杀实验未在限定时间内结束进程",
    ))
}

fn initialize_database(
    files: &Files,
    b: &Bootstrap,
    root: &Secret,
    pause: Option<Point>,
) -> Result<()> {
    if files.exists("database-started")? {
        // 有开始证据但缺 DB / schema / identity 时，不猜测可以覆盖。
        b.open(files, root, false)?;
        return Ok(());
    }
    if files.exists("ledger.db")? {
        return Err(CoreError::new(
            "unexpected_database",
            "未登记的 DB 已存在，不能覆盖",
        ));
    }
    files.publish("database-started", b"1\n", false)?;
    point(files, Point::DatabaseStarted, pause)?;
    let store = Store::create(&files.path.join("ledger.db"), &b.db_key(root)?)?;
    store.bind_bootstrap(&b.vault, &b.device)?;
    store.verify_bootstrap(&b.vault, &b.device)?;
    drop(store);
    std::fs::File::open(files.path.join("ledger.db"))
        .map_err(io_error)?
        .sync_all()
        .map_err(io_error)?;
    crate::bootstrap_files::sync(&files.path)?;
    point(files, Point::DatabaseReady, pause)
}

fn create(request: &Request, input: &[u8]) -> Result<Reply> {
    // 输入末尾是本次提前生成并由调用方持有的 80-byte 恢复文本。
    // 这样创建过程被杀后，控制器仍有独立材料；不把秘密留在 DB 旁。
    if input.len() <= 80 || input.len() > 1104 {
        return Err(CoreError::new("invalid_argument", "创建材料长度无效"));
    }
    let (password, recovery) = input.split_at(input.len() - 80);
    let recovery = crypto::parse_recovery(recovery)?;
    let salt = *crypto::random::<16>()?;
    let profile = Profile::default();
    let password_key = crypto::password_key(password, &salt, &profile)?;
    let recovery_key = crypto::derive(
        recovery.as_ref(),
        &salt,
        b"radishledger/m0c/v1/recovery-wrap",
    )?;
    let root = crypto::random::<32>()?;
    let database_key = crypto::random::<32>()?;
    let vault = random_id()?;
    let device = random_id()?;
    let database_wrap = crypto::derive(root.as_ref(), &salt, b"radishledger/m0c/v1/database-wrap")?;
    let b = Bootstrap {
        format: 1,
        password: crypto::seal(
            password_key.as_ref(),
            &root,
            &aad(&vault, &device, &salt, &profile, "password-root", ""),
        )?,
        recovery: crypto::seal(
            recovery_key.as_ref(),
            &root,
            &aad(&vault, &device, &salt, &profile, "recovery-root", ""),
        )?,
        database: crypto::seal(
            database_wrap.as_ref(),
            &database_key,
            &aad(&vault, &device, &salt, &profile, "database", ""),
        )?,
        vault,
        device,
        salt,
        profile,
    };
    let files = Files::acquire(&request.directory, &request.run, true)?;
    files.publish("bootstrap.json", &encode(&b)?, false)?;
    point(&files, Point::EnvelopesSaved, request.pause_at)?;
    initialize_database(&files, &b, &root, request.pause_at)?;
    Ok(Reply::public(b.inspect(&files, &root)?))
}

pub fn run(request: Request, input: &[u8]) -> Result<Reply> {
    if request.abi != 1 {
        return Err(CoreError::new("abi_mismatch", "bootstrap 实验 ABI 不兼容"));
    }
    if input.len() > 1104 {
        return Err(CoreError::new("invalid_argument", "秘密输入超过允许长度"));
    }
    identity(&request.run)?;
    if let Action::NewRecovery = request.action {
        let recovery = crypto::random::<32>()?;
        let text = crypto::recovery_text(recovery.as_ref())?;
        return Ok(Reply::secret(
            json!({"kind":"recoveryText"}),
            text.as_bytes(),
        ));
    }
    if let Action::Create = request.action {
        return create(&request, input);
    }
    let files = Files::acquire(&request.directory, &request.run, false)?;
    if let Action::Cleanup = request.action {
        files.cleanup()?;
        return Ok(Reply::public(json!({"cleaned":true})));
    }
    let mut b = Bootstrap::load(&files)?;
    if let Action::Status = request.action {
        return Ok(Reply::public(
            json!({"vault":b.vault,"device":b.device,"activeProtection":active_generation(&files)?,
            "readyMarkerPresent":files.exists("ready.json")?,"candidates":files.local_generations()?,"authenticated":false}),
        ));
    }
    let unlock = match request.action {
        Action::Resume { unlock }
        | Action::Inspect { unlock }
        | Action::Seed { unlock }
        | Action::PrepareProtection { unlock } => unlock,
        Action::Confirm => Unlock::Recovery,
        Action::ReissueRecovery => Unlock::Password,
        Action::CommitProtection { ref generation } => {
            let root = b.local_root(&files, generation, input)?;
            let mut report = b.inspect(&files, &root)?;
            if report["ready"] != true || report["fixtureComplete"] != true {
                return Err(CoreError::new("not_ready", "先完成原账库及未同步事实核对"));
            }
            let previous = active_generation(&files)?;
            files.publish("active-protection.json", &encode(generation)?, true)?;
            point(&files, Point::ProtectionPublished, request.pause_at)?;
            report["previousProtection"] = json!(previous.filter(|value| value != generation));
            report["activeProtection"] = json!(generation);
            return Ok(Reply::public(report));
        }
        _ => return Err(CoreError::new("invalid_action", "实验动作不可用")),
    };
    let root = b.root(&files, unlock, input)?;
    match request.action {
        Action::Resume { .. } => initialize_database(&files, &b, &root, request.pause_at)?,
        Action::ReissueRecovery => {
            let store = b.open(&files, &root, false)?;
            if b.ready(&files, &root)? || store.sequence()? != 0 {
                return Err(CoreError::new(
                    "already_ready",
                    "只允许重新生成尚未确认且没有账务的恢复材料",
                ));
            }
            drop(store);
            let recovery = crypto::random::<32>()?;
            let key = crypto::derive(
                recovery.as_ref(),
                &b.salt,
                b"radishledger/m0c/v1/recovery-wrap",
            )?;
            b.recovery = crypto::seal(key.as_ref(), &root, &b.aad("recovery-root", ""))?;
            files.publish("bootstrap.json", &encode(&b)?, true)?;
            let text = crypto::recovery_text(recovery.as_ref())?;
            return Ok(Reply::secret(b.inspect(&files, &root)?, text.as_bytes()));
        }
        Action::Confirm => {
            b.open(&files, &root, false)?;
            if !b.ready(&files, &root)? {
                let key = crypto::derive(root.as_ref(), &b.salt, b"radishledger/m0c/v1/ready")?;
                let ready = crypto::seal(key.as_ref(), &[0x52; 32], &b.aad("ready", ""))?;
                files.publish("ready.json", &encode(&ready)?, false)?;
            }
            point(&files, Point::ReadyPublished, request.pause_at)?;
        }
        Action::Seed { .. } => {
            if !b.ready(&files, &root)? {
                return Err(CoreError::new(
                    "recovery_unconfirmed",
                    "先确认独立恢复材料，再保存合成账务",
                ));
            }
            let mut store = b.open(&files, &root, true)?;
            if store.fixture_digest()?.is_none() {
                if store.sequence()? != 0 {
                    return Err(CoreError::new(
                        "fixture_incomplete",
                        "合成样例写入中断，已有事实保留供核对",
                    ));
                }
                crate::probe::populate_bootstrap_fixture(&mut store)?;
                store.save_fixture_digest(&store.content_digest()?)?;
            }
        }
        Action::PrepareProtection { .. } => {
            let mut report = b.inspect(&files, &root)?;
            if report["ready"] != true || report["fixtureComplete"] != true {
                return Err(CoreError::new("not_ready", "先完成原账库及未同步事实核对"));
            }
            let generation = random_id()?;
            let local_key = crypto::random::<32>()?;
            let key = crypto::derive(
                local_key.as_ref(),
                &b.salt,
                b"radishledger/m0c/v1/local-wrap",
            )?;
            let local = LocalProtection {
                root: crypto::seal(key.as_ref(), &root, &b.aad("local-root", &generation))?,
                generation: generation.clone(),
            };
            files.publish(&format!("local-{generation}.json"), &encode(&local)?, false)?;
            point(&files, Point::ProtectionPrepared, request.pause_at)?;
            report["candidateProtection"] = json!(generation);
            return Ok(Reply::secret(report, local_key.as_ref()));
        }
        Action::Inspect { .. } => {}
        _ => return Err(CoreError::new("invalid_action", "实验动作不可用")),
    }
    Ok(Reply::public(b.inspect(&files, &root)?))
}

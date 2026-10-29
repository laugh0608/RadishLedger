//! M0 合成故障实验。恢复 fixture 使用公开测试 key，不是生产 bootstrap / 解锁。
use crate::domain::{Event, EventKind};
use crate::store::{DbKey, Snapshot, Store, Write, WriteEffect};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

pub const ABI: u32 = 1;
const CASH: &str = "00000000000000000000000000000001";
const BANK: &str = "00000000000000000000000000000002";
const OWNER: &str = "RadishLedger M0 public synthetic recovery fixture v1\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommitPoint {
    BeforeOutbox,
    BeforeCommit,
    AfterCommit,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub abi: u32,
    pub directory: PathBuf,
    pub action: Action,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Action {
    NativeError,
    #[cfg(unix)]
    FileBoundary {
        run: String,
        #[serde(rename = "fileControl")]
        file_control: bool,
    },
    FileBoundaryCleanup {
        run: String,
    },
    Panic,
    Concurrent,
    RecoveryStart {
        run: String,
        point: CommitPoint,
    },
    RecoveryInspect {
        run: String,
        point: CommitPoint,
    },
    RecoveryCleanup {
        run: String,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Audit {
    pub snapshot: Snapshot,
    pub sequence: i64,
    pub revisions: i64,
    pub operations: Vec<String>,
}

fn failure(message: &'static str) -> CoreError {
    CoreError::new("validation_failed", message)
}
fn io_error(_: std::io::Error) -> CoreError {
    CoreError::new("probe_io", "实验文件操作失败，保留已有材料")
}
fn ensure(ok: bool, message: &'static str) -> Result<()> {
    if ok { Ok(()) } else { Err(failure(message)) }
}
fn id(n: usize) -> String {
    format!("{n:032x}")
}
fn opening(n: usize, account: &str, amount: &str) -> Write {
    Write {
        operation_id: id(n),
        event_id: id(n + 10000),
        revision_id: id(n + 20000),
        previous_revision_id: None,
        event: Some(Event {
            note: String::new(),
            kind: EventKind::Opening,
            business_date: "2026-10-14".into(),
            account_id: Some(account.into()),
            category_id: None,
            from_account_id: None,
            to_account_id: None,
            amount_minor: amount.into(),
        }),
        expected_sequence: None,
        observed_balance_minor: None,
        target_balance_minor: None,
        resolve_candidates: BTreeSet::new(),
    }
}
fn transfer(n: usize, amount: &str) -> Write {
    let mut write = opening(n, CASH, amount);
    write.event = Some(Event {
        note: String::new(),
        kind: EventKind::Transfer,
        business_date: "2026-10-14".into(),
        account_id: None,
        category_id: None,
        from_account_id: Some(CASH.into()),
        to_account_id: Some(BANK.into()),
        amount_minor: amount.into(),
    });
    write
}
fn seed(path: &Path, key: &DbKey) -> Result<Store> {
    let mut store = Store::create(path, key)?;
    store.add_account(CASH, "cash", "合成现金")?;
    store.add_account(BANK, "bank", "合成银行")?;
    store.commit(&opening(10, CASH, "10000"))?;
    store.commit(&opening(11, BANK, "20000"))?;
    Ok(store)
}
fn verify(
    audit: &Audit,
    transfers: usize,
    amount: i64,
    expected_ops: &BTreeSet<String>,
) -> Result<()> {
    ensure(
        audit.sequence == (2 + transfers) as i64
            && audit.revisions == (2 + transfers) as i64
            && audit.operations.len() == 2 + transfers
            && audit.operations.iter().cloned().collect::<BTreeSet<_>>() == *expected_ops
            && audit.snapshot.active_event_ids.len() == 2 + transfers
            && audit.snapshot.projection.balances_minor.get(CASH)
                == Some(&(10000 - amount).to_string())
            && audit.snapshot.projection.balances_minor.get(BANK)
                == Some(&(20000 + amount).to_string()),
        "事实、转账两侧、序号或 outbox 与独立预期不符",
    )
}

pub fn run(request: Request) -> Result<Value> {
    if request.abi != ABI {
        return Err(CoreError::new(
            "abi_mismatch",
            "实验 ABI 不兼容，未执行操作",
        ));
    }
    if !request.directory.is_absolute() || !request.directory.is_dir() {
        return Err(CoreError::new("invalid_directory", "实验临时目录不可用"));
    }
    match request.action {
        #[cfg(unix)]
        Action::FileBoundary { run, file_control } => {
            crate::store::boundary::run(&request.directory, &run, file_control)
        }
        Action::FileBoundaryCleanup { run } => {
            crate::bootstrap_files::Files::acquire(&request.directory, &run, false)?.cleanup()?;
            Ok(json!({"cleaned":true}))
        }
        Action::NativeError => Err(CoreError::new("synthetic_native_error", "合成 native 拒绝")),
        Action::Panic => panic!("M0 synthetic panic; no user data"),
        Action::Concurrent => concurrent(&request.directory),
        Action::RecoveryStart { run, point } => recovery_start(&request.directory, &run, point),
        Action::RecoveryInspect { run, point } => recovery_inspect(&request.directory, &run, point),
        Action::RecoveryCleanup { run } => {
            let root = owned_root(&request.directory, &run)?;
            std::fs::remove_dir_all(root).map_err(io_error)?;
            Ok(json!({"cleaned":true}))
        }
    }
}

fn concurrent(directory: &Path) -> Result<Value> {
    const WRITERS: usize = 4;
    const PER_WRITER: usize = 16;
    let temp = tempfile::Builder::new()
        .prefix("radishledger-concurrent-")
        .tempdir_in(directory)
        .map_err(io_error)?;
    let path = temp.path().join("shared.db");
    let key = DbKey::random()?;
    drop(seed(&path, &key)?);
    // 所有连接先打开再出发，避免线程准备失败让 barrier 永久等待。
    let mut writers = Vec::new();
    for _ in 0..WRITERS {
        writers.push(Store::open(&path, &key)?);
    }
    let reader = Store::open(&path, &key)?;
    let start = Arc::new(Barrier::new(WRITERS + 1));
    let remaining = Arc::new(AtomicUsize::new(WRITERS));
    let (committed, replayed, samples) = std::thread::scope(|scope| -> Result<_> {
        let mut jobs = Vec::new();
        for (worker, mut store) in writers.into_iter().enumerate() {
            let start = start.clone();
            let remaining = remaining.clone();
            jobs.push(scope.spawn(move || {
                start.wait();
                struct Finished(Arc<AtomicUsize>);
                impl Drop for Finished {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                let _finished = Finished(remaining);
                (|| -> Result<(usize, usize)> {
                    let mut commits = 0;
                    let mut replays = 0;
                    let mut writes = vec![transfer(100, "7")];
                    writes.extend(
                        (0..PER_WRITER).map(|n| transfer(1000 + worker * PER_WRITER + n, "1")),
                    );
                    for write in writes {
                        match store.commit(&write)? {
                            WriteEffect::Committed => commits += 1,
                            WriteEffect::Replayed => replays += 1,
                        }
                        ensure(
                            store.commit(&write)? == WriteEffect::Replayed,
                            "重复回调产生新写入",
                        )?;
                        replays += 1;
                    }
                    Ok((commits, replays))
                })()
            }));
        }
        start.wait();
        let mut samples = 0;
        let observed = (|| -> Result<()> {
            while remaining.load(Ordering::SeqCst) != 0 {
                let audit = reader.validation_audit()?;
                let cash: i64 = audit.snapshot.projection.balances_minor[CASH]
                    .parse()
                    .map_err(|_| failure("余额编码无效"))?;
                let bank: i64 = audit.snapshot.projection.balances_minor[BANK]
                    .parse()
                    .map_err(|_| failure("余额编码无效"))?;
                ensure(
                    cash + bank == 30000
                        && audit.sequence == audit.revisions
                        && audit.operations.len() == audit.revisions as usize,
                    "并发读取看见部分事务",
                )?;
                samples += 1;
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        })();
        let mut committed = 0;
        let mut replayed = 0;
        for job in jobs {
            let (c, r) = job.join().map_err(|_| failure("并发线程 panic"))??;
            committed += c;
            replayed += r;
        }
        observed?;
        ensure(samples > 0, "没有取得并发读样本")?;
        Ok((committed, replayed, samples))
    })?;
    let transfers = WRITERS * PER_WRITER + 1;
    ensure(
        committed == transfers && replayed == WRITERS * (PER_WRITER + 1) + WRITERS - 1,
        "并发去重计数不符",
    )?;
    let mut ops = BTreeSet::from([id(10), id(11), id(100)]);
    ops.extend((1000..1000 + WRITERS * PER_WRITER).map(id));
    let audit = reader.validation_audit()?;
    verify(&audit, transfers, (WRITERS * PER_WRITER + 7) as i64, &ops)?;
    drop(reader);
    let report = json!({"writers":WRITERS,"writesPerWriter":PER_WRITER,"committed":committed,
        "replayed":replayed,"readSamples":samples,"audit":audit});
    temp.close().map_err(io_error)?;
    Ok(report)
}

fn root_path(directory: &Path, run: &str) -> Result<PathBuf> {
    ensure(
        run.len() == 32 && run.bytes().all(|b| b.is_ascii_hexdigit()),
        "实验 run 标识无效",
    )?;
    Ok(directory.join(format!("radishledger-recovery-{run}")))
}
fn owned_root(directory: &Path, run: &str) -> Result<PathBuf> {
    let root = root_path(directory, run)?;
    let meta = std::fs::symlink_metadata(&root).map_err(io_error)?;
    ensure(
        meta.is_dir() && !meta.file_type().is_symlink(),
        "实验目录身份无效",
    )?;
    ensure(
        std::fs::read_to_string(root.join("owner.txt")).map_err(io_error)? == OWNER,
        "实验目录归属不符",
    )?;
    Ok(root)
}
fn save_json(path: &Path, value: &Value) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(value.to_string().as_bytes())
        .map_err(io_error)?;
    file.sync_all().map_err(io_error)
}
fn recovery_start(directory: &Path, run: &str, point: CommitPoint) -> Result<Value> {
    let root = root_path(directory, run)?;
    std::fs::create_dir(&root).map_err(io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .map_err(io_error)?;
    }
    std::fs::write(root.join("owner.txt"), OWNER).map_err(io_error)?;
    save_json(
        &root.join("scenario.json"),
        &json!({"point":point,"run":run}),
    )?;
    let mut store = seed(&root.join("fixture.db"), &DbKey::public_recovery_fixture())?;
    store.commit_observed(&transfer(100, "1700"), |current| {
        if current == point {
            save_json(
                &root.join("ready.json"),
                &json!({"run":run,"point":point,"pid":std::process::id()}),
            )?;
            // 外部 harness 必须在窗口内确认 PID 后强制终止；超时不冒充故障证据。
            let deadline = Instant::now() + Duration::from_secs(45);
            while Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            return Err(CoreError::new(
                "interruption_timeout",
                "未在窗口内取得外部进程中断",
            ));
        }
        Ok(())
    })?;
    Err(failure("没有到达指定中断点"))
}
fn recovery_inspect(directory: &Path, run: &str, point: CommitPoint) -> Result<Value> {
    let root = owned_root(directory, run)?;
    let scenario: Value =
        serde_json::from_slice(&std::fs::read(root.join("scenario.json")).map_err(io_error)?)
            .map_err(|_| failure("实验场景文件损坏"))?;
    ensure(
        scenario == json!({"point":point,"run":run}),
        "恢复场景不匹配",
    )?;
    let mut store = Store::open(&root.join("fixture.db"), &DbKey::public_recovery_fixture())?;
    let before = store.validation_audit()?;
    let committed = point == CommitPoint::AfterCommit;
    let mut ops = BTreeSet::from([id(10), id(11)]);
    if committed {
        ops.insert(id(100));
    }
    verify(
        &before,
        usize::from(committed),
        if committed { 1700 } else { 0 },
        &ops,
    )?;
    let write = transfer(100, "1700");
    ensure(
        store.commit(&write)?
            == if committed {
                WriteEffect::Replayed
            } else {
                WriteEffect::Committed
            },
        "恢复后的操作身份核对失败",
    )?;
    ensure(
        store.commit(&write)? == WriteEffect::Replayed,
        "恢复后重复提交产生新事实",
    )?;
    let wrong = transfer(100, "1701");
    ensure(
        store
            .commit(&wrong)
            .err()
            .is_some_and(|e| e.code == "operation_reused"),
        "复用身份的不同意图未拒绝",
    )?;
    let after = store.validation_audit()?;
    ops.insert(id(100));
    verify(&after, 1, 1700, &ops)?;
    Ok(
        json!({"point":point,"pid":std::process::id(),"before":before,"after":after,
        "alreadyCommitted":committed,"duplicateVerified":true,"changedIntentRejected":true}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_rejects_foreign_directories_and_missing_databases() {
        let dir = tempfile::tempdir().unwrap();
        assert!(root_path(dir.path(), "../foreign").is_err());
        let identity = crate::random_id().unwrap();
        let root = root_path(dir.path(), &identity).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("owner.txt"), "foreign").unwrap();
        assert!(owned_root(dir.path(), &identity).is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("owner.txt")).unwrap(),
            "foreign"
        );
        std::fs::write(root.join("owner.txt"), OWNER).unwrap();
        save_json(
            &root.join("scenario.json"),
            &json!({"point":"beforeCommit", "run":identity}),
        )
        .unwrap();
        assert_eq!(
            recovery_inspect(dir.path(), &identity, CommitPoint::BeforeCommit)
                .unwrap_err()
                .code,
            "missing_database"
        );
        assert!(!root.join("fixture.db").exists());
    }
    #[test]
    fn concurrent_shared_database_preserves_atomicity_and_identity() {
        let dir = tempfile::tempdir().unwrap();
        concurrent(dir.path()).unwrap();
        assert_eq!(dir.path().read_dir().unwrap().count(), 0);
    }
    #[test]
    fn abi_mismatch_cannot_execute_or_create_a_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let request = Request {
            abi: 99,
            directory: dir.path().into(),
            action: Action::Panic,
        };
        assert_eq!(run(request).unwrap_err().code, "abi_mismatch");
        assert_eq!(dir.path().read_dir().unwrap().count(), 0);
    }
}

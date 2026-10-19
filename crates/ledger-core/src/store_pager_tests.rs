//! 宿主 pager 故障及临时文件边界。所有 VFS 全局变更只发生在独占子进程。
use super::*;
use rusqlite::ffi;
use serde_json::{Value, json};
use std::fs::File;
use std::io::{Read, Write as IoWrite};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[path = "store_test_vfs.rs"]
mod vfs;
use vfs::{Failure, Fault, Kind, Observation, Operation, Phase, Registration};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum Scenario {
    WalCommit,
    DeleteCommit,
    Checkpoint,
    MemoryTemp,
    FileTempControl,
}

#[derive(Serialize, Deserialize)]
struct Request {
    directory: PathBuf,
    scenario: Scenario,
    write: Write,
    fault: Option<Fault>,
}

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn run_worker(request: &Request, key: &DbKey, kill_after_report: bool) -> Value {
    let mut child = Worker(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "store::pager_tests::pager_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("RLM0_PAGER_WORKER", "1")
            // 只影响本次子进程；即使原 VFS 自选匿名临时名也落在本次独占目录。
            .env("TMPDIR", &request.directory)
            .env("SQLITE_TMPDIR", &request.directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(
                File::create(request.directory.join("worker.stdout")).unwrap(),
            ))
            .stderr(Stdio::from(
                File::create(request.directory.join("worker.stderr")).unwrap(),
            ))
            .spawn()
            .unwrap(),
    );
    let mut input = child.0.stdin.take().unwrap();
    let metadata = serde_json::to_vec(request).unwrap();
    input
        .write_all(&(metadata.len() as u32).to_be_bytes())
        .unwrap();
    input.write_all(&metadata).unwrap();
    input.write_all(key.0.as_ref()).unwrap();
    drop(input);
    let report_path = request.directory.join("report.json");
    let deadline = Instant::now() + Duration::from_secs(45);
    while !report_path.exists() {
        if let Some(status) = child.0.try_wait().unwrap() {
            panic!(
                "worker exited {status}: {}",
                std::fs::read_to_string(request.directory.join("worker.stderr")).unwrap()
            );
        }
        assert!(Instant::now() < deadline, "pager worker timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
    let report: Value = serde_json::from_slice(&std::fs::read(report_path).unwrap()).unwrap();
    assert_eq!(report["pid"], child.0.id());
    assert_ne!(child.0.id(), std::process::id());
    if kill_after_report {
        child.0.kill().unwrap();
        assert_eq!(child.0.wait().unwrap().signal(), Some(9));
    } else {
        while child.0.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "worker did not close its files");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(child.0.wait().unwrap().success());
    }
    report
}

fn save_evidence(name: &str, value: &Value) {
    if let Some(directory) = std::env::var_os("M0_PAGER_EVIDENCE_DIR") {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let directory = PathBuf::from(directory).canonicalize().unwrap();
        assert!(directory.starts_with(root.join("local-evidence/m0-c")));
        std::fs::write(
            directory.join(name),
            serde_json::to_string_pretty(value).unwrap() + "\n",
        )
        .unwrap();
    }
}

fn audit(store: &Store) -> Value {
    serde_json::to_value(store.validation_audit().unwrap()).unwrap()
}

fn check_a11(value: &Value, added: bool) {
    let extra = i64::from(added);
    assert_eq!(value["sequence"], 8 + extra);
    assert_eq!(value["revisions"], 8 + extra);
    assert_eq!(
        value["operations"].as_array().unwrap().len(),
        7 + extra as usize
    );
    let snapshot = &value["snapshot"];
    assert_eq!(
        snapshot["unresolvedCandidateRevisionIds"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(snapshot["deletedEventIds"].as_array().unwrap().len(), 1);
    assert_eq!(
        snapshot["activeEventIds"].as_array().unwrap().len(),
        3 + extra as usize
    );
    assert_eq!(
        snapshot["selectedRevisionIds"].as_object().unwrap().len(),
        4 + extra as usize
    );
    assert_eq!(snapshot["netAssetsMinor"], "18000");
    let mut balances: Vec<_> = snapshot["balancesMinor"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| v.as_str().unwrap().parse::<i64>().unwrap())
        .collect();
    balances.sort();
    assert_eq!(balances, if added { [8123, 9877] } else { [8000, 10000] });
}

fn seed(path: &Path, key: &DbKey) -> Write {
    let mut store = Store::create(path, key).unwrap();
    crate::probe::populate_bootstrap_fixture(&mut store).unwrap();
    check_a11(&audit(&store), false);
    let account = |kind: &str| {
        store
            .connection
            .query_row("SELECT id FROM accounts WHERE kind=?1", [kind], |row| {
                row.get(0)
            })
            .unwrap()
    };
    Write {
        operation_id: crate::random_id().unwrap(),
        event_id: crate::random_id().unwrap(),
        revision_id: crate::random_id().unwrap(),
        previous_revision_id: None,
        event: Some(Event {
            kind: EventKind::Transfer,
            business_date: "2026-10-19".into(),
            account_id: None,
            category_id: None,
            from_account_id: Some(account("bank")),
            to_account_id: Some(account("cash")),
            amount_minor: "123".into(),
        }),
        expected_sequence: Some(8),
        observed_balance_minor: None,
        target_balance_minor: None,
        resolve_candidates: BTreeSet::new(),
    }
}

#[test]
fn pager_partial_writes_and_sync_errors_recover_atomic_facts() {
    let root = tempfile::Builder::new()
        .prefix("radishledger-pager-")
        .tempdir()
        .unwrap();
    let key = DbKey::random().unwrap();
    let original = root.path().join("original.db");
    let write = seed(&original, &key);
    let before = Store::open_readonly(&original, &key).unwrap();
    let before_digest = before.content_digest().unwrap();
    let before_audit = audit(&before);
    drop(before);
    let expected_path = root.path().join("expected.db");
    std::fs::copy(&original, &expected_path).unwrap();
    let mut expected = Store::open(&expected_path, &key).unwrap();
    assert_eq!(expected.commit(&write).unwrap(), WriteEffect::Committed);
    check_a11(&audit(&expected), true);
    let after_digest = expected.content_digest().unwrap();
    let after_audit = audit(&expected);
    drop(expected);
    let mut reports = Vec::new();
    let mut controls = Vec::new();
    for scenario in [
        Scenario::WalCommit,
        Scenario::DeleteCommit,
        Scenario::Checkpoint,
    ] {
        let control = tempfile::tempdir_in(root.path()).unwrap();
        std::fs::copy(&original, control.path().join("ledger.db")).unwrap();
        let request = Request {
            directory: control.path().into(),
            scenario,
            write: write.clone(),
            fault: None,
        };
        let baseline = run_worker(&request, &key, true);
        assert!(baseline["error"].is_null());
        let trace: Observation = serde_json::from_value(baseline["io"].clone()).unwrap();
        assert!(!trace.events.is_empty());
        let successful = Store::open(&control.path().join("ledger.db"), &key).unwrap();
        assert_eq!(successful.content_digest().unwrap(), after_digest);
        assert_eq!(audit(&successful), after_audit);
        drop(successful);
        controls.push(baseline);
        let mut counts = [0, 0];
        for (index, event) in trace.events.iter().enumerate() {
            assert!(matches!(
                event.kind,
                Kind::Database | Kind::Wal | Kind::Journal
            ));
            let failures = match event.operation {
                Operation::Write => {
                    counts[0] += 1;
                    vec![Failure::PartialWrite]
                }
                Operation::Sync => {
                    counts[1] += 1;
                    vec![Failure::SyncBefore, Failure::SyncAfter]
                }
            };
            for failure in failures {
                let directory = tempfile::tempdir_in(root.path()).unwrap();
                let path = directory.path().join("ledger.db");
                std::fs::copy(&original, &path).unwrap();
                let request = Request {
                    directory: directory.path().into(),
                    scenario,
                    write: write.clone(),
                    fault: Some(Fault {
                        event: index + 1,
                        failure,
                    }),
                };
                let result = run_worker(&request, &key, true);
                let observed: Observation = serde_json::from_value(result["io"].clone()).unwrap();
                assert_eq!(observed.injected, 1, "fault must hit an actual IO call");
                assert_eq!(observed.events[index].kind, event.kind);
                assert_eq!(observed.events[index].operation, event.operation);
                assert_eq!(observed.events[index].amount, event.amount);
                assert_eq!(observed.events[index].offset, event.offset);
                let code = match failure {
                    Failure::PartialWrite => {
                        assert!(
                            observed.partial_bytes > 0 && observed.partial_bytes < event.amount
                        );
                        ffi::SQLITE_IOERR_WRITE
                    }
                    Failure::SyncBefore => ffi::SQLITE_IOERR_FSYNC,
                    Failure::SyncAfter => {
                        assert_eq!(observed.delegated_syncs_before_error, 1);
                        ffi::SQLITE_IOERR_FSYNC
                    }
                };
                assert_eq!(
                    result["error"]["sqliteCode"], code,
                    "{scenario:?} {index} {failure:?}: {result}"
                );
                assert_eq!(result["error"]["code"], "storage_error");
                // 父进程没有注册 shim；强杀后由普通生产入口重新打开和恢复。
                let mut recovered = Store::open(&path, &key).unwrap();
                let recovered_digest = recovered.content_digest().unwrap();
                let recovered_audit = audit(&recovered);
                let applied = recovered_digest == after_digest;
                if matches!(scenario, Scenario::Checkpoint) {
                    assert!(applied, "checkpoint must retain committed WAL facts");
                }
                assert_eq!(
                    recovered_digest,
                    if applied {
                        &after_digest
                    } else {
                        &before_digest
                    }
                    .to_owned()
                );
                assert_eq!(
                    recovered_audit,
                    if applied {
                        after_audit.clone()
                    } else {
                        before_audit.clone()
                    }
                );
                check_a11(&audit(&recovered), applied);
                let integrity: String = recovered
                    .connection
                    .query_row("PRAGMA integrity_check", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(integrity, "ok");
                assert_eq!(
                    recovered.commit(&write).unwrap(),
                    if applied {
                        WriteEffect::Replayed
                    } else {
                        WriteEffect::Committed
                    }
                );
                assert_eq!(recovered.commit(&write).unwrap(), WriteEffect::Replayed);
                assert_eq!(recovered.content_digest().unwrap(), after_digest);
                assert_eq!(audit(&recovered), after_audit);
                drop(recovered);
                println!(
                    "pager {scenario:?} event={} {:?}/{:?} {failure:?}: error={code}, recovered={}, retry/replay=passed",
                    index + 1,
                    event.kind,
                    event.operation,
                    if applied { "complete-new" } else { "original" }
                );
                reports.push(json!({"scenario": scenario, "event": index + 1, "failure": failure,
                    "worker": result, "recovered": if applied { "complete-new" } else { "original" },
                    "recoveryPid": std::process::id(), "recoveredDigest": recovered_digest,
                    "recoveredAudit": recovered_audit, "afterRetryDigest": after_digest,
                    "retryAndReplay": true, "integrity": integrity}));
                directory.close().unwrap();
            }
        }
        assert!(counts[0] > 0 && counts[1] > 0);
        control.close().unwrap();
    }
    root.close().unwrap();
    save_evidence(
        "pager-report.json",
        &json!({"cases": reports, "controls": controls, "allPassed": true, "cleaned": true,
        "scope": "host test VFS partial writes and sync errors; no power-loss claim"}),
    );
}

fn temp_workload(
    store: &mut Store,
    file_control: bool,
    observer: &Registration,
    directory: &Path,
    key: &DbKey,
) -> Value {
    let connection = &mut store.connection;
    if file_control {
        connection.execute_batch("PRAGMA temp_store=FILE;").unwrap();
    }
    let temp_store: i64 = connection
        .pragma_query_value(None, "temp_store", |r| r.get(0))
        .unwrap();
    assert_eq!(temp_store, if file_control { 1 } else { 2 });
    connection
        .execute_batch(
            "PRAGMA cache_size=8; PRAGMA temp.cache_size=8;
        CREATE TABLE boundary_source(i INTEGER PRIMARY KEY, value TEXT NOT NULL UNIQUE);",
        )
        .unwrap();
    let marker = std::str::from_utf8(vfs::MARKER).unwrap();
    let tx = connection.transaction().unwrap();
    for i in 1..=8192 {
        tx.execute(
            "INSERT INTO boundary_source VALUES (?1, ?2)",
            params![i, format!("{marker}{:0256}-{i:05}", 0)],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    observer.phase(Phase::TempTable);
    connection
        .execute_batch("CREATE TEMP TABLE boundary_temp AS SELECT * FROM boundary_source;")
        .unwrap();
    let sql = "SELECT value FROM boundary_source ORDER BY substr(value, -5) DESC";
    let plan: Vec<String> = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap()
        .query_map([], |r| r.get(3))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert!(plan.iter().any(|s| s.contains("TEMP B-TREE")));
    observer.phase(Phase::OrderBy);
    let mut sort = connection.prepare(sql).unwrap();
    let values: Vec<String> = sort
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(values.len(), 8192);
    assert!(values[0].ends_with("08192") && values[8191].ends_with("00001"));
    let sorts = sort.get_status(rusqlite::StatementStatus::Sort);
    assert!(sorts > 0);
    drop(sort);
    observer.phase(Phase::AutoIndex);
    let mut join = connection
        .prepare(
            "SELECT sum(length(b.value)) FROM boundary_temp a JOIN boundary_temp b ON a.i=b.i WHERE a.i<=64",
        )
        .unwrap();
    assert_eq!(
        join.query_row([], |r| r.get::<_, i64>(0)).unwrap(),
        64 * values[0].len() as i64
    );
    let autoindex = join.get_status(rusqlite::StatementStatus::AutoIndex);
    assert!(autoindex > 0);
    drop(join);
    observer.phase(Phase::Materialization);
    let materialized: i64 = connection
        .query_row(
            "WITH m AS MATERIALIZED (SELECT * FROM boundary_source) SELECT count(*) FROM m",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(materialized, 8192);
    observer.phase(Phase::StatementRollback);
    // ABORT 在多行更新最后一行触发，验证 statement journal 的回滚语义。
    let constraint = connection.execute_batch("BEGIN; UPDATE boundary_source SET value=CASE WHEN i=8192 THEN (SELECT value FROM boundary_source WHERE i=1) ELSE value||'changed' END;").unwrap_err();
    assert_eq!(
        constraint.sqlite_error_code(),
        Some(ffi::ErrorCode::ConstraintViolation)
    );
    connection.execute_batch("COMMIT;").unwrap();
    let changed: i64 = connection
        .query_row(
            "SELECT count(*) FROM boundary_source WHERE value LIKE '%changed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(changed, 0);
    observer.phase(Phase::TempIndex);
    connection
        .execute_batch(
            "CREATE INDEX temp.boundary_sort ON boundary_temp(value); DROP TABLE boundary_temp;",
        )
        .unwrap();
    observer.phase(Phase::Vacuum);
    connection.execute_batch("VACUUM;").unwrap();
    // SHM 不经 xOpen，而由父 VFS 的 xShmMap 管理；在 WAL 尚打开时检查真实文件。
    let shm = std::fs::read(directory.join("ledger.db-shm")).unwrap();
    assert!(!shm.is_empty() && !shm.windows(vfs::MARKER.len()).any(|w| w == vfs::MARKER));
    observer.phase(Phase::MultiDatabaseCommit);
    connection
        .execute_batch("PRAGMA journal_mode=DELETE;")
        .unwrap();
    // 只为覆盖 super-journal；附加库与 key 均为本次合成材料，参数绑定不写日志。
    // Store 的既有连接不带 SQLITE_OPEN_CREATE，附加的新文件也必须先排他创建。
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join("auxiliary.db"))
        .unwrap();
    connection
        .execute(
            "ATTACH DATABASE ?1 AS auxiliary KEY ?2",
            params![
                directory.join("auxiliary.db").to_str().unwrap(),
                key.0.as_ref()
            ],
        )
        .unwrap();
    connection
        .execute_batch(
            "PRAGMA auxiliary.journal_mode=DELETE; PRAGMA auxiliary.synchronous=FULL;
        CREATE TABLE auxiliary.sample(value TEXT NOT NULL); BEGIN;
        UPDATE boundary_source SET value=value||'-multidb' WHERE i=1;",
        )
        .unwrap();
    connection
        .execute("INSERT INTO auxiliary.sample VALUES (?1)", [marker])
        .unwrap();
    connection
        .execute_batch("COMMIT; DETACH auxiliary; PRAGMA journal_mode=WAL;")
        .unwrap();
    let auxiliary = Store::connect(&directory.join("auxiliary.db"), key, true).unwrap();
    assert_eq!(
        auxiliary
            .connection
            .query_row("SELECT value FROM sample", [], |r| r.get::<_, String>(0))
            .unwrap(),
        marker
    );
    drop(auxiliary);
    json!({"tempStore": temp_store, "rows": materialized, "sortOperations": sorts,
        "automaticIndexRows": autoindex, "statementRollback": true, "vacuum": true,
        "multiDatabaseCommit": true, "shmBytes": shm.len(), "shmMarkerFound": false,
        "queryPlan": plan})
}

#[test]
fn temporary_files_observed_before_unlink_and_memory_policy_checked() {
    let root = tempfile::Builder::new()
        .prefix("radishledger-temp-boundary-")
        .tempdir()
        .unwrap();
    let key = DbKey::random().unwrap();
    let original = root.path().join("original.db");
    let write = seed(&original, &key);
    let before = Store::open(&original, &key).unwrap();
    let before_digest = before.content_digest().unwrap();
    let before_audit = audit(&before);
    drop(before);
    let mut reports = Vec::new();
    for scenario in [Scenario::MemoryTemp, Scenario::FileTempControl] {
        let directory = tempfile::tempdir_in(root.path()).unwrap();
        std::fs::copy(&original, directory.path().join("ledger.db")).unwrap();
        let result = run_worker(
            &Request {
                directory: directory.path().into(),
                scenario,
                write: write.clone(),
                fault: None,
            },
            &key,
            false,
        );
        let observation: Observation = serde_json::from_value(result["io"].clone()).unwrap();
        assert!(
            observation
                .files
                .iter()
                .all(|f| f.closed && f.scan_code == ffi::SQLITE_OK)
        );
        assert!(observation.files.iter().all(|f| f.kind != Kind::Other));
        assert!(observation.files.iter().any(|f| f.shm_maps > 0));
        assert!(
            observation
                .files
                .iter()
                .any(|f| f.kind == Kind::SuperJournal && f.scanned_bytes > 0)
        );
        assert!(
            observation
                .files
                .iter()
                .any(|f| f.kind == Kind::Journal && f.scanned_bytes > 0)
        );
        let temporary: Vec<_> = observation
            .files
            .iter()
            .filter(|f| f.kind.temporary())
            .collect();
        if matches!(scenario, Scenario::MemoryTemp) {
            assert!(
                temporary.is_empty(),
                "MEMORY must not open disk temporary files: {temporary:?}"
            );
            assert!(
                observation
                    .files
                    .iter()
                    .all(|f| !f.disk_contains_marker && !f.write_contains_marker)
            );
        } else {
            assert!(!temporary.is_empty());
            for kind in [
                Kind::TempDatabase,
                Kind::TempJournal,
                Kind::SubJournal,
                Kind::Transient,
            ] {
                assert!(
                    temporary
                        .iter()
                        .any(|f| f.kind == kind && f.scanned_bytes > 0),
                    "missing positive control for {kind:?}"
                );
            }
            for phase in [
                Phase::TempTable,
                Phase::OrderBy,
                Phase::AutoIndex,
                Phase::Materialization,
                Phase::StatementRollback,
                Phase::TempIndex,
                Phase::Vacuum,
            ] {
                assert!(
                    temporary.iter().any(|f| f.written_during.contains(&phase)),
                    "missing disk positive control for {phase:?}"
                );
            }
            assert!(
                temporary
                    .iter()
                    .any(|f| f.anonymous && f.scanned_bytes > 0 && f.disk_contains_marker)
            );
        }
        // 普通可写入口在最后关闭时清理 WAL / SHM；只读连接可能合法地留下 sidecar。
        let store = Store::open(&directory.path().join("ledger.db"), &key).unwrap();
        check_a11(&audit(&store), false);
        assert_eq!(store.content_digest().unwrap(), before_digest);
        assert_eq!(audit(&store), before_audit);
        drop(store);
        let mut files: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        files.sort();
        assert_eq!(
            files,
            [
                "auxiliary.db",
                "ledger.db",
                "report.json",
                "worker.stderr",
                "worker.stdout"
            ]
        );
        println!(
            "temporary {scenario:?}: {} actual VFS files, {} temporary, all closed and scanned",
            observation.files.len(),
            temporary.len()
        );
        reports.push(json!({"worker": result, "a11Digest": before_digest,
            "a11Audit": before_audit, "fullFactsPreserved": true, "filesAfterClose": files}));
        directory.close().unwrap();
    }
    root.close().unwrap();
    save_evidence(
        "temporary-files-report.json",
        &json!({"cases": reports, "allPassed": true, "cleaned": true,
        "scope": "bounded host workload and FILE positive control; no exhaustive memory/cache claim"}),
    );
}

#[test]
#[ignore = "仅由上方父测试在独占子进程调用；stdin 传递随机合成 key"]
fn pager_worker() {
    assert_eq!(std::env::var("RLM0_PAGER_WORKER").as_deref(), Ok("1"));
    let mut input = std::io::stdin().lock();
    let mut header = [0; 4];
    input.read_exact(&mut header).unwrap();
    let length = u32::from_be_bytes(header) as usize;
    assert!(length <= 65536);
    let mut metadata = vec![0; length];
    input.read_exact(&mut metadata).unwrap();
    let request: Request = serde_json::from_slice(&metadata).unwrap();
    let mut secret = Zeroizing::new([0_u8; 32]);
    input.read_exact(secret.as_mut()).unwrap();
    let key = DbKey::from_secret(secret.as_ref()).unwrap();
    drop(secret);
    drop(input);
    let observer = Registration::new();
    let mut store = Store::open(&request.directory.join("ledger.db"), &key).unwrap();
    store
        .connection
        .execute_batch("PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    if matches!(request.scenario, Scenario::DeleteCommit) {
        store
            .connection
            .execute_batch("PRAGMA journal_mode=DELETE;")
            .unwrap();
    }
    if matches!(request.scenario, Scenario::Checkpoint) {
        store.commit(&request.write).unwrap();
    }
    let cipher: String = store
        .connection
        .pragma_query_value(None, "cipher_version", |r| r.get(0))
        .unwrap();
    let compile_options: Vec<String> = store
        .connection
        .prepare("PRAGMA compile_options")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .filter(|s| s.starts_with("TEMP_STORE="))
        .collect();
    observer.arm(request.fault);
    observer.phase(if matches!(request.scenario, Scenario::Checkpoint) {
        Phase::Checkpoint
    } else {
        Phase::Commit
    });
    let mut workload = Value::Null;
    let error = match request.scenario {
        Scenario::WalCommit | Scenario::DeleteCommit => store.commit(&request.write).err(),
        Scenario::Checkpoint => store
            .connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(db_error)
            .err(),
        Scenario::MemoryTemp | Scenario::FileTempControl => {
            workload = temp_workload(
                &mut store,
                matches!(request.scenario, Scenario::FileTempControl),
                &observer,
                &request.directory,
                &key,
            );
            None
        }
    };
    let temporary = matches!(
        request.scenario,
        Scenario::MemoryTemp | Scenario::FileTempControl
    );
    // pager 实验不关闭连接，交给父进程 SIGKILL；临时文件实验必须验证 xClose 扫描。
    if temporary {
        observer.phase(Phase::Close);
        drop(store);
    } else {
        std::mem::forget(store);
    }
    let result = json!({"pid": std::process::id(), "scenario": request.scenario, "error": error,
        "sqlite": vfs::sqlite_version(), "sqlcipher": cipher, "compileOptions": compile_options,
        "io": observer.report(), "workload": workload});
    let pending = request.directory.join("report.pending");
    std::fs::write(&pending, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    std::fs::rename(pending, request.directory.join("report.json")).unwrap();
    if temporary {
        drop(observer);
    } else {
        loop {
            std::thread::park();
        }
    }
}

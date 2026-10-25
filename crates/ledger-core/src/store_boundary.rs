//! M0 文件边界观测：同一工作负载供宿主 / 原生 harness 与真实 App 调用。
//! App 只用命名 VFS，不切换进程默认 VFS；不是生产账库接口。
use super::*;
use rusqlite::ffi;
use serde_json::{Value, json};

#[path = "store_test_vfs.rs"]
pub(super) mod vfs;
use vfs::{Kind, Phase, Registration};

pub(super) fn configuration(connection: &Connection) -> Value {
    let options: Vec<String> = connection
        .prepare("PRAGMA compile_options")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    let cipher: String = connection
        .pragma_query_value(None, "cipher_version", |r| r.get(0))
        .unwrap();
    let parent = unsafe { ffi::sqlite3_vfs_find(std::ptr::null()) };
    assert!(!parent.is_null());
    let name = unsafe { std::ffi::CStr::from_ptr((*parent).zName).to_str().unwrap() };
    json!({"os": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "sqlite": vfs::sqlite_version(), "sqlcipher": cipher, "compileOptions": options,
        "defaultVfs": name, "vfsVersion": unsafe { (*parent).iVersion }})
}

pub(super) fn temp_workload(
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
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
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

fn verify_observation(observation: &vfs::Observation, memory: bool, directory: &Path) {
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
    for file in &observation.files {
        if let Some(path) = &file.path {
            assert!(Path::new(path).starts_with(directory));
            // SQLite super-journal uses 0644 subject to the process umask;
            // Android App umask removes group/other bits. The parent stays 0700.
            assert!(
                file.mode == Some(0o600)
                    || (file.kind == Kind::SuperJournal && file.mode == Some(0o644)),
                "named synthetic file mode: {:?} {:?}",
                file.kind,
                file.mode
            );
            assert!(file.owner_uid.is_some());
        }
    }
    let temporary: Vec<_> = observation
        .files
        .iter()
        .filter(|f| f.kind.temporary())
        .collect();
    if memory {
        assert!(temporary.is_empty());
        assert!(
            observation
                .files
                .iter()
                .all(|f| !f.disk_contains_marker && !f.write_contains_marker)
        );
    } else {
        for kind in [
            Kind::TempDatabase,
            Kind::TempJournal,
            Kind::SubJournal,
            Kind::Transient,
        ] {
            assert!(
                temporary
                    .iter()
                    .any(|f| f.kind == kind && f.scanned_bytes > 0)
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
            assert!(temporary.iter().any(|f| f.written_during.contains(&phase)));
        }
        assert!(
            temporary
                .iter()
                .any(|f| f.anonymous && f.disk_contains_marker)
        );
    }
}

/// 专用 App 实验留下非秘密元数据待平台读取；调用方必须用同一 run 精确清理。
/// 不接受已有账库，随机 DB key 只存在于本次调用的内存。
pub(crate) fn run(directory: &Path, run: &str, file_control: bool) -> Result<Value> {
    use crate::bootstrap_files::{Files, io_error};
    use std::os::unix::fs::OpenOptionsExt;
    let files = Files::acquire(directory, run, true)?;
    let path = files.path.join("ledger.db");
    let key = DbKey::random()?;
    let mut store = Store::create(&path, &key)?;
    crate::probe::populate_bootstrap_fixture(&mut store)?;
    let before_digest = store.content_digest()?;
    let before_audit = serde_json::to_value(store.validation_audit()?).unwrap();
    let config = configuration(&store.connection);
    drop(store);
    let forced_memory = config["compileOptions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|option| option == "TEMP_STORE=3");
    let observer = Registration::named(&format!("radishledger-boundary-{run}"));
    assert_eq!(
        configuration(&Connection::open_in_memory().map_err(db_error)?),
        config
    );
    let connection = Connection::open_with_flags_and_vfs(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        observer.name(),
    )
    .map_err(db_error)?;
    let mut store = Store::configure(connection, &key, false)?;
    observer.arm(None);
    let workload = temp_workload(&mut store, file_control, &observer, &files.path, &key);
    // 留下真实、非空 WAL 与 SHM 给平台读取属性；只读事务阻止最后 writer 把它们删除。
    store
        .connection
        .execute_batch("UPDATE boundary_source SET value=value||'-wal-evidence' WHERE i=1;")
        .map_err(db_error)?;
    let after_digest = store.content_digest()?;
    let after_audit = serde_json::to_value(store.validation_audit()?).unwrap();
    assert_eq!(before_digest, after_digest);
    assert_eq!(before_audit, after_audit);
    let observed_before_default = observer.report().files.len();
    let readonly = Store::open_readonly(&path, &key)?;
    assert_eq!(readonly.content_digest()?, before_digest);
    readonly
        .connection
        .execute_batch("BEGIN; SELECT count(*) FROM revisions;")
        .map_err(db_error)?;
    assert_eq!(
        observer.report().files.len(),
        observed_before_default,
        "ordinary Store must not use the named observer"
    );
    observer.phase(Phase::Close);
    drop(store);
    let observation = observer.report();
    verify_observation(&observation, !file_control || forced_memory, &files.path);

    // 有界的独立明文扫描对照；文件属性也可由平台 API 实测，不能混入加密 IO 判据。
    let control = files.path.join("scanner-control.db");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&control)
        .map_err(io_error)?;
    let plain = Connection::open_with_flags_and_vfs(
        &control,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        observer.name(),
    )
    .map_err(db_error)?;
    plain
        .execute_batch("CREATE TABLE scanner(value TEXT NOT NULL);")
        .map_err(db_error)?;
    plain
        .execute(
            "INSERT INTO scanner VALUES (?1)",
            [std::str::from_utf8(vfs::MARKER).unwrap()],
        )
        .map_err(db_error)?;
    drop(plain);
    let report = observer.report();
    let scanner = report
        .files
        .iter()
        .skip(observation.files.len())
        .find(|file| file.kind == Kind::Database)
        .unwrap();
    assert!(
        scanner.closed
            && scanner.scan_code == ffi::SQLITE_OK
            && scanner.scanned_bytes > 0
            && scanner.disk_contains_marker
    );
    assert!(
        report
            .files
            .iter()
            .all(|f| f.closed && f.scan_code == ffi::SQLITE_OK)
    );
    drop(observer);
    drop(readonly);
    assert_eq!(
        configuration(&Connection::open_in_memory().map_err(db_error)?),
        config
    );
    let wal_bytes = std::fs::metadata(files.path.join("ledger.db-wal"))
        .map_err(io_error)?
        .len();
    let shm_bytes = std::fs::metadata(files.path.join("ledger.db-shm"))
        .map_err(io_error)?
        .len();
    assert!(wal_bytes > 32 && shm_bytes > 0);
    let io_event_count = observation.events.len();
    let mut io = serde_json::to_value(&observation).unwrap();
    io.as_object_mut().unwrap().remove("events");
    // App 只返回文件级元数据和实际 IO 总数；完整逐调用 trace 由 pager harness 保存。
    Ok(
        json!({"configuration":config,"directory":files.path,"pid":std::process::id(),
        "fileControl":file_control,"forcedMemory":forced_memory,"defaultVfsUnchanged":true,
        "io":io,"ioEventCount":io_event_count,"scannerControl":scanner,"workload":workload,
        "a11Digest":after_digest,"a11Audit":after_audit,"fullFactsPreserved":true,
        "walBytesForPlatformAudit":wal_bytes,"shmBytesForPlatformAudit":shm_bytes,
        "cleanupRequired":true}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap_files::Files;

    #[test]
    fn configuration_error_closes_connection_before_unregistering_named_vfs() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("ledger.db");
        let key = DbKey::random().unwrap();
        drop(Store::create(&path, &key).unwrap());
        let before = configuration(&Connection::open_in_memory().unwrap());
        let name = format!("radishledger-config-error-{}", crate::random_id().unwrap());
        let lookup = std::ffi::CString::new(name.clone()).unwrap();
        let result = (|| -> Result<()> {
            let observer = Registration::named(&name);
            let connection = Connection::open_with_flags_and_vfs(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
                observer.name(),
            )
            .map_err(db_error)?;
            let wrong_key = DbKey::random()?;
            let _store = Store::configure(connection, &wrong_key, false)?;
            Ok(())
        })();
        assert_eq!(result.unwrap_err().code, "storage_error");
        // SAFETY: 名称由本测试创建；只检查注册表，不解引用返回指针。
        assert!(unsafe { ffi::sqlite3_vfs_find(lookup.as_ptr()) }.is_null());
        assert_eq!(
            configuration(&Connection::open_in_memory().unwrap()),
            before
        );
        assert_eq!(
            Store::open(&path, &key)
                .unwrap()
                .snapshot(&[])
                .unwrap()
                .projection
                .net_assets_minor,
            "0"
        );
    }

    #[test]
    fn named_app_observer_keeps_default_vfs_and_exact_cleanup() {
        let root = tempfile::tempdir().unwrap();
        for file_control in [false, true] {
            let id = crate::random_id().unwrap();
            let report = run(root.path(), &id, file_control).unwrap();
            assert!(report["defaultVfsUnchanged"].as_bool().unwrap());
            assert!(report["fullFactsPreserved"].as_bool().unwrap());
            assert_eq!(report["a11Audit"]["snapshot"]["netAssetsMinor"], "18000");
            assert_eq!(
                report["a11Audit"]["operations"].as_array().unwrap().len(),
                7
            );
            assert!(run(root.path(), &id, false).is_err());
            Files::acquire(root.path(), &id, false)
                .unwrap()
                .cleanup()
                .unwrap();
            assert!(!Path::new(report["directory"].as_str().unwrap()).exists());
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

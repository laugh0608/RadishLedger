//! 实际生成文件的宿主证据；不把标记未检出外推为完整密码或移动设备结论。
use super::*;

const MARKER: &str = "RadishLedger synthetic plaintext boundary marker 2026-10-19";

fn contains_marker(path: &Path, minimum_size: usize) -> bool {
    let bytes = std::fs::read(path).unwrap();
    assert!(bytes.len() > minimum_size, "必须实际生成非空文件");
    bytes
        .windows(MARKER.len())
        .any(|chunk| chunk == MARKER.as_bytes())
}

#[test]
fn encrypted_wal_checkpoint_and_rollback_journal_preserve_nonempty_facts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("encrypted.db");
    let key = DbKey::random().unwrap();
    let mut store = Store::create(&path, &key).unwrap();
    store
        .connection
        .pragma_update(None, "wal_autocheckpoint", 0)
        .unwrap();
    crate::probe::populate_bootstrap_fixture(&mut store).unwrap();
    let marker_id = crate::random_id().unwrap();
    store.add_account(&marker_id, "cash", MARKER).unwrap();
    let digest = store.content_digest().unwrap();
    let before = serde_json::to_value(store.validation_audit().unwrap()).unwrap();
    assert_eq!(
        before["snapshot"]["unresolvedCandidateRevisionIds"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        before["snapshot"]["deletedEventIds"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(before["operations"].as_array().unwrap().len(), 7);
    assert!(!contains_marker(
        &directory.path().join("encrypted.db-wal"),
        32
    ));
    assert!(!contains_marker(
        &directory.path().join("encrypted.db-shm"),
        0
    ));
    store
        .connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    assert!(!contains_marker(&path, 4096));
    assert_eq!(
        std::fs::metadata(directory.path().join("encrypted.db-wal"))
            .unwrap()
            .len(),
        0
    );

    // 同一 SQLCipher 构建不传 key 的连接不能读取真实加密库。
    let plain =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert!(
        plain
            .query_row("SELECT count(*) FROM accounts", [], |row| row
                .get::<_, i64>(0))
            .is_err()
    );
    drop(plain);
    assert!(Store::open_readonly(&path, &DbKey::random().unwrap()).is_err());

    // 生产实验默认 WAL；这里显式切到 DELETE，观察真实回滚日志，不改变默认配置。
    let mode: String = store
        .connection
        .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "delete");
    let tx = store.connection.transaction().unwrap();
    tx.execute(
        "UPDATE accounts SET name='synthetic pending change' WHERE id=?1",
        [&marker_id],
    )
    .unwrap();
    tx.cache_flush().unwrap();
    assert!(!contains_marker(
        &directory.path().join("encrypted.db-journal"),
        512
    ));
    tx.rollback().unwrap();
    assert!(!directory.path().join("encrypted.db-journal").exists());
    assert_eq!(store.content_digest().unwrap(), digest);
    assert_eq!(
        serde_json::to_value(store.validation_audit().unwrap()).unwrap(),
        before
    );
    let temp_store: i64 = store
        .connection
        .pragma_query_value(None, "temp_store", |row| row.get(0))
        .unwrap();
    assert_eq!(temp_store, 2);
    let names = store
        .connection
        .prepare("SELECT name FROM accounts ORDER BY name DESC")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert!(names.iter().any(|name| name == MARKER));
    drop(store);
    let reopened = Store::open_readonly(&path, &key).unwrap();
    assert_eq!(reopened.content_digest().unwrap(), digest);
    drop(reopened);

    // 独立正向对照：无 key 的合成库必须在相同文件检查中检出标记。
    let control_path = directory.path().join("plaintext-control.db");
    let control = Connection::open(&control_path).unwrap();
    control.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE sample(value TEXT);").unwrap();
    control
        .execute("INSERT INTO sample VALUES (?1)", [MARKER])
        .unwrap();
    assert!(contains_marker(
        &directory.path().join("plaintext-control.db-wal"),
        32
    ));
    control
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")
        .unwrap();
    assert!(contains_marker(&control_path, 4096));
    control
        .execute_batch("BEGIN IMMEDIATE; UPDATE sample SET value='synthetic pending change';")
        .unwrap();
    control.cache_flush().unwrap();
    assert!(contains_marker(
        &directory.path().join("plaintext-control.db-journal"),
        512
    ));
    control.execute_batch("ROLLBACK;").unwrap();
    drop(control);
    // 已关闭所有连接后只有两份本次生成的 DB；临时查询按 MEMORY 执行。
    let mut files: Vec<_> = directory
        .path()
        .read_dir()
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    files.sort();
    assert_eq!(files, ["encrypted.db", "plaintext-control.db"]);
    directory.close().unwrap();
}

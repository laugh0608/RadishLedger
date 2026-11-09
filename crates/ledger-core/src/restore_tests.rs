//! 合成 SQLCipher、原人工材料与进程中断；不接收真实账单或系统密钥。
use super::*;
use crate::archive_container::{CANDIDATE_LIMITS, seal};
use crate::logical_snapshot::{self, LogicalSnapshot};
use crate::store::Store;
use serde_json::Value;
use std::fs;
use std::sync::atomic::Ordering;

const PASSWORD: &[u8] = b"public-restore-stage-test-password";
const POINTS: [Point; 10] = [
    Point::Owned,
    Point::Schema,
    Point::References,
    Point::Revisions,
    Point::Selections,
    Point::History,
    Point::BeforeCommit,
    Point::Written,
    Point::Verified,
    Point::Marked,
];
fn flag() -> AtomicBool {
    AtomicBool::new(false)
}
fn capture(store: &Store) -> LogicalSnapshot {
    store
        .logical_snapshot(
            logical_snapshot::Limits {
                rows: 100_000,
                bytes: 16 * 1024 * 1024,
            },
            &["2026-01".into()],
            &flag(),
        )
        .unwrap()
}
fn path(parent: &Path, ticket: &Ticket) -> std::path::PathBuf {
    parent
        .join(format!("restore-{}", ticket.task_id))
        .join("stage.db")
}
fn sql(path: &Path, key: &DbKey, statement: &str) {
    // 测试专用连接不暴露到产品入口；与 Store 使用相同 SQLCipher 配置。
    crate::store::restore_store::test_sql(path, key, statement).unwrap();
}
fn source(case: &str, step: &str) -> (tempfile::TempDir, DbKey, Store, LogicalSnapshot) {
    let dir = tempfile::tempdir().unwrap();
    let key = DbKey::random().unwrap();
    let mut store = Store::create(&dir.path().join("original.db"), &key).unwrap();
    crate::probe::populate_snapshot_fixture(&mut store, case, step).unwrap();
    let snapshot = capture(&store);
    (dir, key, store, snapshot)
}
fn assert_contents(actual: &Contents, expected: &Contents) {
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}
fn code<T>(result: Result<T>) -> &'static str {
    result.err().unwrap().code
}
fn public_archive() -> Vec<u8> {
    let material: Value = serde_json::from_str(include_str!(
        "../../../docs/validation/archive-container-review-inputs.json"
    ))
    .unwrap();
    material["cryptoExpected"]["archiveHex"]
        .as_str()
        .unwrap()
        .as_bytes()
        .chunks_exact(2)
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect()
}
const PUBLIC_PASSWORD: &[u8] = b"public-review-password";

#[test]
fn a11_new_key_reopened_staging_preserves_facts_but_cannot_be_a_runtime_store() {
    let _guard = crate::crypto::KDF_TESTS.lock().unwrap();
    let (dir, old_key, store, snapshot) = source("A11", "conflict");
    let old_digest = store.content_digest().unwrap();
    let archive = seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &flag()).unwrap();
    let task = Task::new(archive.as_bytes()).unwrap();
    let other = Task::new(archive.as_bytes()).unwrap();
    assert_ne!(task.ticket.target_id, other.ticket.target_id);
    let result = task
        .prepare(
            dir.path(),
            archive.as_bytes(),
            PASSWORD,
            CANDIDATE_LIMITS,
            &flag(),
        )
        .unwrap();
    assert_contents(result.contents(), snapshot.contents());
    let facts = &result.contents().facts;
    assert_eq!(
        (
            facts.revisions.len(),
            facts.candidates.len(),
            facts.intent_history.len()
        ),
        (8, 2, 7)
    );
    assert_eq!(result.contents().verification.deleted_event_ids.len(), 1);
    assert_eq!(store.content_digest().unwrap(), old_digest);
    assert_eq!(
        code(Store::open(&path(dir.path(), task.ticket()), &task.key)),
        "unsupported_schema"
    );
    assert_eq!(
        code(StageDb::open_readonly(
            &path(dir.path(), task.ticket()),
            &old_key,
            task.ticket()
        )),
        "storage_error"
    );
    drop(store);
    drop(old_key);
    let (ticket, key) = task.into_parts();
    let task = Task::resume(ticket, key).unwrap();
    assert_contents(
        task.inspect(
            dir.path(),
            archive.as_bytes(),
            PASSWORD,
            CANDIDATE_LIMITS,
            &flag(),
        )
        .unwrap()
        .contents(),
        snapshot.contents(),
    );
    task.abandon(dir.path()).unwrap();
    assert!(!path(dir.path(), task.ticket()).parent().unwrap().exists());
    assert!(dir.path().join("original.db").exists());
}

#[test]
fn authenticated_empty_go_archive_creates_a_real_encrypted_target() {
    let _guard = crate::crypto::KDF_TESTS.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let input = public_archive();
    let task = Task::new(&input).unwrap();
    let result = task
        .prepare(
            dir.path(),
            &input,
            PUBLIC_PASSWORD,
            CANDIDATE_LIMITS,
            &flag(),
        )
        .unwrap();
    assert!(result.contents().facts.revisions.is_empty());
    assert!(
        !fs::read(path(dir.path(), task.ticket()))
            .unwrap()
            .starts_with(b"SQLite format 3")
    );
    crate::store::restore_store::test_integrity(&path(dir.path(), task.ticket()), &task.key)
        .unwrap();
    task.abandon(dir.path()).unwrap();
}

#[test]
fn wrong_password_changed_input_cancel_and_missing_resume_have_no_target_side_effects() {
    let _guard = crate::crypto::KDF_TESTS.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let input = public_archive();
    let task = Task::new(&input).unwrap();
    assert_eq!(
        code(task.prepare(
            dir.path(),
            &input,
            b"wrong-password",
            CANDIDATE_LIMITS,
            &flag()
        )),
        "archive_unlock_failed"
    );
    let mut changed = input.clone();
    changed[200] ^= 1;
    assert_eq!(
        code(task.prepare(
            dir.path(),
            &changed,
            PUBLIC_PASSWORD,
            CANDIDATE_LIMITS,
            &flag()
        )),
        "restore_mismatch"
    );
    assert_eq!(
        code(task.prepare(
            dir.path(),
            &input,
            PUBLIC_PASSWORD,
            CANDIDATE_LIMITS,
            &AtomicBool::new(true)
        )),
        "cancelled"
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    let (ticket, key) = task.into_parts();
    let resumed = Task::resume(ticket, key).unwrap();
    assert_eq!(
        code(resumed.prepare(
            dir.path(),
            &input,
            PUBLIC_PASSWORD,
            CANDIDATE_LIMITS,
            &flag()
        )),
        "restore_io"
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn cancellation_and_stage_failures_keep_committed_range_and_allow_exact_retry() {
    let _guard = crate::crypto::KDF_TESTS.lock().unwrap();
    let (dir, _old_key, store, snapshot) = source("A11", "conflict");
    let before = store.content_digest().unwrap();
    let archive = seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &flag()).unwrap();
    for point in POINTS {
        for fail in [false, true] {
            let task = Task::new(archive.as_bytes()).unwrap();
            let cancel = flag();
            let result = task.prepare_observed(
                dir.path(),
                archive.as_bytes(),
                PASSWORD,
                CANDIDATE_LIMITS,
                &cancel,
                |p| {
                    if p == point {
                        if fail {
                            return Err(CoreError::new("injected_failure", "合成暂存阶段失败"));
                        }
                        cancel.store(true, Ordering::Relaxed);
                    }
                    Ok(())
                },
            );
            assert_eq!(
                code(result),
                if fail {
                    "injected_failure"
                } else {
                    "cancelled"
                }
            );
            assert_eq!(store.content_digest().unwrap(), before);
            let (ticket, key) = task.into_parts();
            let task = Task::resume(ticket, key).unwrap();
            let result = task
                .prepare(
                    dir.path(),
                    archive.as_bytes(),
                    PASSWORD,
                    CANDIDATE_LIMITS,
                    &flag(),
                )
                .unwrap();
            assert_contents(result.contents(), snapshot.contents());
            task.abandon(dir.path()).unwrap();
        }
    }
}

#[test]
fn archived_references_and_original_changes_after_capture_remain_separate() {
    let _guard = crate::crypto::KDF_TESTS.lock().unwrap();
    for (case, step) in [("A06", "renamed"), ("A04", "delete")] {
        let (dir, _key, mut store, snapshot) = source(case, step);
        let archive = seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &flag()).unwrap();
        let task = Task::new(archive.as_bytes()).unwrap();
        task.prepare(
            dir.path(),
            archive.as_bytes(),
            PASSWORD,
            CANDIDATE_LIMITS,
            &flag(),
        )
        .unwrap();
        // 原库在捕获后产生新事实；暂存不会覆盖或切换它。
        store
            .add_account(
                &crate::random_id().unwrap(),
                "cash",
                "synthetic-later-original-account",
            )
            .unwrap();
        let changed = store.content_digest().unwrap();
        let result = task
            .inspect(
                dir.path(),
                archive.as_bytes(),
                PASSWORD,
                CANDIDATE_LIMITS,
                &flag(),
            )
            .unwrap();
        assert_contents(result.contents(), snapshot.contents());
        assert_eq!(store.content_digest().unwrap(), changed);
        task.abandon(dir.path()).unwrap();
    }
}

#[test]
fn sql_rejection_rolls_back_all_facts_and_verified_flags_cannot_hide_tampering() {
    let _guard = crate::crypto::KDF_TESTS.lock().unwrap();
    let (dir, _key, store, snapshot) = source("A11", "conflict");
    let archive = seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &flag()).unwrap();
    let task = Task::new(archive.as_bytes()).unwrap();
    let dbpath = path(dir.path(), task.ticket());
    let result=task.prepare_observed(dir.path(),archive.as_bytes(),PASSWORD,CANDIDATE_LIMITS,&flag(),|point| {
        if point==Point::Schema {
            sql(&dbpath,&task.key,"CREATE TRIGGER reject_restore BEFORE INSERT ON restore_revisions BEGIN SELECT RAISE(ABORT,'synthetic SQL rejection'); END;");
        }
        Ok(())
    });
    let error = result.err().unwrap();
    assert_eq!(error.code, "storage_error");
    assert!(error.sqlite_code.is_some());
    sql(&dbpath, &task.key, "DROP TRIGGER reject_restore;");
    assert_eq!(
        crate::store::restore_store::test_count(
            &dbpath,
            &task.key,
            "SELECT count(*) FROM restore_accounts"
        )
        .unwrap(),
        0
    );
    assert_eq!(
        crate::store::restore_store::test_count(
            &dbpath,
            &task.key,
            "SELECT state FROM restore_control"
        )
        .unwrap(),
        0
    );
    task.prepare(
        dir.path(),
        archive.as_bytes(),
        PASSWORD,
        CANDIDATE_LIMITS,
        &flag(),
    )
    .unwrap();
    sql(
        &dbpath,
        &task.key,
        "UPDATE restore_accounts SET name='synthetic unexpected changed name' WHERE position=0; UPDATE restore_control SET state=2;",
    );
    assert_eq!(
        code(task.inspect(
            dir.path(),
            archive.as_bytes(),
            PASSWORD,
            CANDIDATE_LIMITS,
            &flag()
        )),
        "restore_mismatch"
    );
    assert_eq!(
        code(task.prepare(
            dir.path(),
            archive.as_bytes(),
            PASSWORD,
            CANDIDATE_LIMITS,
            &flag()
        )),
        "restore_mismatch"
    );
    // 归属仍正确，可显式放弃损坏暂存；不自动重建掩盖差异。
    task.abandon(dir.path()).unwrap();
    assert_contents(capture(&store).contents(), snapshot.contents());
}

#[test]
fn owner_wrong_key_unknown_schema_busy_links_and_unknown_files_are_rejected() {
    let _guard = crate::crypto::KDF_TESTS.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let input = public_archive();
    let task = Task::new(&input).unwrap();
    task.prepare(
        dir.path(),
        &input,
        PUBLIC_PASSWORD,
        CANDIDATE_LIMITS,
        &flag(),
    )
    .unwrap();
    let dbpath = path(dir.path(), task.ticket());
    let folder = dbpath.parent().unwrap();
    let held = files::Files::acquire(dir.path(), task.ticket(), false).unwrap();
    assert_eq!(
        code(files::Files::acquire(dir.path(), task.ticket(), false)),
        "restore_busy"
    );
    drop(held);
    let ticket = task.ticket.clone();
    let wrong = Task::resume(ticket, DbKey::random().unwrap()).unwrap();
    assert_eq!(
        code(wrong.inspect(
            dir.path(),
            &input,
            PUBLIC_PASSWORD,
            CANDIDATE_LIMITS,
            &flag()
        )),
        "storage_error"
    );
    assert_eq!(code(wrong.abandon(dir.path())), "storage_error");
    let owner = fs::read(folder.join("owner")).unwrap();
    fs::write(folder.join("owner"), b"foreign owner").unwrap();
    assert_eq!(code(task.abandon(dir.path())), "restore_mismatch");
    fs::write(folder.join("owner"), owner).unwrap();
    fs::write(folder.join("unrelated.keep"), b"synthetic unrelated file").unwrap();
    assert_eq!(code(task.abandon(dir.path())), "restore_mismatch");
    assert!(dbpath.exists());
    fs::remove_file(folder.join("unrelated.keep")).unwrap();
    sql(
        &dbpath,
        &task.key,
        "CREATE TABLE unknown_stage_fact(value TEXT);",
    );
    assert_eq!(
        code(task.inspect(
            dir.path(),
            &input,
            PUBLIC_PASSWORD,
            CANDIDATE_LIMITS,
            &flag()
        )),
        "restore_mismatch"
    );
    sql(&dbpath, &task.key, "DROP TABLE unknown_stage_fact;");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = dir.path().join("unrelated.db");
        fs::write(&outside, b"synthetic unrelated database").unwrap();
        let saved = dir.path().join("saved-stage.db");
        fs::rename(&dbpath, &saved).unwrap();
        symlink(&outside, &dbpath).unwrap();
        assert_eq!(code(task.abandon(dir.path())), "restore_mismatch");
        fs::remove_file(&dbpath).unwrap();
        fs::hard_link(&outside, &dbpath).unwrap();
        assert_eq!(code(task.abandon(dir.path())), "restore_mismatch");
        fs::remove_file(&dbpath).unwrap();
        fs::rename(saved, &dbpath).unwrap();
        assert_eq!(fs::read(&outside).unwrap(), b"synthetic unrelated database");
    }
    task.abandon(dir.path()).unwrap();
}

#[test]
fn discard_failure_retains_exact_permit_and_retry_cannot_resurrect_target() {
    let _guard = crate::crypto::KDF_TESTS.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let input = public_archive();
    let task = Task::new(&input).unwrap();
    task.prepare(
        dir.path(),
        &input,
        PUBLIC_PASSWORD,
        CANDIDATE_LIMITS,
        &flag(),
    )
    .unwrap();
    let result = task.abandon_observed(dir.path(), |name| {
        if name == "stage.db" {
            Err(CoreError::new("injected_cleanup", "合成删除后失败"))
        } else {
            Ok(())
        }
    });
    assert_eq!(code(result), "injected_cleanup");
    let folder = path(dir.path(), task.ticket()).parent().unwrap().to_owned();
    assert!(folder.join("owner").exists() && folder.join("discard").exists());
    assert!(!folder.join("stage.db").exists());
    assert_eq!(
        code(task.prepare(
            dir.path(),
            &input,
            PUBLIC_PASSWORD,
            CANDIDATE_LIMITS,
            &flag()
        )),
        "restore_discarding"
    );
    let (ticket, key) = task.into_parts();
    let resumed = Task::resume(ticket, key).unwrap();
    resumed.abandon(dir.path()).unwrap();
    assert!(!folder.exists());
}

#[test]
fn v2_v3_history_is_preserved_without_executable_queues_or_raw_csv() {
    use crate::csv_import::{
        Decision, prepare,
        test_support::{HEADER, ONE, id, mapping},
    };
    let _guard = crate::crypto::KDF_TESTS.lock().unwrap();
    for version in [2, 3] {
        let dir = tempfile::tempdir().unwrap();
        let key = DbKey::random().unwrap();
        let mut store = if version == 2 {
            Store::create_csv_experiment(&dir.path().join("original.db"), &key)
        } else {
            Store::create_csv_undo_experiment(&dir.path().join("original.db"), &key)
        }
        .unwrap();
        store
            .add_account(
                &id(1),
                "cash",
                "synthetic-protected-restore-plaintext-marker-v1",
            )
            .unwrap();
        store.add_category(&id(5), "expense", "合成餐饮").unwrap();
        let mut mapping = mapping();
        mapping.fields.push(crate::csv_import::Field::Ignore);
        let text = HEADER.trim_end().to_owned()
            + ",ignored\n"
            + ONE.trim_end()
            + ",excluded-raw-csv-marker\n"
            + &ONE.trim_end().replace("src-1", "src-2")
            + ",excluded-raw-csv-marker\n"
            + &ONE.trim_end().replace("src-1", "src-3")
            + ",excluded-raw-csv-marker\n";
        let plan = prepare(&store, text.as_bytes(), &mapping, &flag()).unwrap();
        let import = store
            .save_csv_task(
                &id(900),
                &plan,
                &[Decision::Import, Decision::Import, Decision::Import],
                2,
                &flag(),
            )
            .unwrap();
        store
            .publish_csv_batch(&import.target_id, &import.id, 1, &flag())
            .unwrap();
        store
            .set_csv_cancelled(&import.target_id, &import.id, true)
            .unwrap();
        if version == 3 {
            let preview = store
                .preview_csv_undo(&import.target_id, &import.id, &flag())
                .unwrap();
            store
                .save_csv_undo(&id(800), &preview, &[1, 2], 1, &flag())
                .unwrap();
            store
                .publish_csv_undo_batch(&import.target_id, &id(800), 1, &flag())
                .unwrap();
            store
                .set_csv_undo_cancelled(&import.target_id, &id(800), true)
                .unwrap();
        }
        let snapshot = capture(&store);
        let original = store.content_digest().unwrap();
        let archive = seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &flag()).unwrap();
        let task = Task::new(archive.as_bytes()).unwrap();
        let result = task
            .prepare(
                dir.path(),
                archive.as_bytes(),
                PASSWORD,
                CANDIDATE_LIMITS,
                &flag(),
            )
            .unwrap();
        assert_contents(result.contents(), snapshot.contents());
        let h = result.contents().facts.imports.as_ref().unwrap();
        assert_eq!(h.tasks[0].next_record, 3);
        assert_eq!(h.tasks[0].state, crate::csv_import::TaskState::Cancelled);
        assert_ne!(h.source_target_id, task.ticket.target_id);
        assert_eq!(h.undo_tasks.len(), if version == 3 { 1 } else { 0 });
        if version == 3 {
            assert_eq!(h.undo_tasks[0].next_ordinal, 2);
            assert_eq!(h.undo_tasks[0].state, crate::csv_undo::State::Cancelled);
        }
        let dbpath = path(dir.path(), task.ticket());
        assert_eq!(crate::store::restore_store::test_count(&dbpath,&task.key,"SELECT count(*) FROM sqlite_schema WHERE name IN ('outbox','csv_tasks','csv_undo_tasks','bootstrap_identity')").unwrap(),0);
        for entry in fs::read_dir(dbpath.parent().unwrap()).unwrap() {
            let bytes = fs::read(entry.unwrap().path()).unwrap();
            for marker in [
                b"synthetic-protected-restore-plaintext-marker-v1".as_slice(),
                b"excluded-raw-csv-marker",
            ] {
                assert!(!bytes.windows(marker.len()).any(|b| b == marker));
            }
        }
        let control = b"synthetic-protected-restore-plaintext-marker-v1";
        let control_path = dir.path().join("plaintext-control");
        fs::write(&control_path, control).unwrap();
        assert!(
            fs::read(&control_path)
                .unwrap()
                .windows(control.len())
                .any(|b| b == control)
        );
        assert_eq!(store.content_digest().unwrap(), original);
        task.abandon(dir.path()).unwrap();
    }
}

#[cfg(unix)]
#[path = "restore_crash_tests.rs"]
mod crash;

#[test]
fn staged_read_budgets_and_duplicate_history_are_checked_before_trusting_verified_state() {
    let _guard = crate::crypto::KDF_TESTS.lock().unwrap();
    let (dir, _key, _store, snapshot) = source("A11", "conflict");
    let archive = seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &flag()).unwrap();
    let task = Task::new(archive.as_bytes()).unwrap();
    task.prepare(
        dir.path(),
        archive.as_bytes(),
        PASSWORD,
        CANDIDATE_LIMITS,
        &flag(),
    )
    .unwrap();
    let dbpath = path(dir.path(), task.ticket());
    let db = StageDb::open_readonly(&dbpath, &task.key, task.ticket()).unwrap();
    for records in [23, 24, 25] {
        let result = db.verify(
            snapshot.contents(),
            Limits {
                records,
                ..CANDIDATE_LIMITS
            },
            &flag(),
        );
        assert_eq!(result.is_ok(), records >= 24);
        if records < 24 {
            assert_eq!(code(result), "archive_limit");
        }
    }
    let size = snapshot.encode(16 * 1024 * 1024, &flag()).unwrap().len();
    for payload_bytes in [size - 1, size, size + 1] {
        let result = db.verify(
            snapshot.contents(),
            Limits {
                payload_bytes,
                ..CANDIDATE_LIMITS
            },
            &flag(),
        );
        assert_eq!(result.is_ok(), payload_bytes >= size);
        if payload_bytes < size {
            assert_eq!(code(result), "archive_limit");
        }
    }
    drop(db);
    sql(
        &dbpath,
        &task.key,
        "UPDATE restore_intents SET payload='{\"operationId\":\"x\",\"operationId\":\"y\"}' WHERE position=0;",
    );
    assert_eq!(
        code(task.inspect(
            dir.path(),
            archive.as_bytes(),
            PASSWORD,
            CANDIDATE_LIMITS,
            &flag()
        )),
        "invalid_archive_payload"
    );
    task.abandon(dir.path()).unwrap();
}

#[test]
fn unregistered_orphan_and_unknown_publication_file_are_preserved_for_review() {
    let dir = tempfile::tempdir().unwrap();
    let input = public_archive();
    let task = Task::new(&input).unwrap();
    let folder = path(dir.path(), task.ticket()).parent().unwrap().to_owned();
    fs::create_dir(&folder).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
    }
    assert_eq!(code(task.abandon(dir.path())), "restore_io");
    assert!(folder.exists());
    // 缺 owner 不按路径名推断归属；原任务材料重新建立后仍拒绝未知临时文件。
    fs::remove_dir(&folder).unwrap();
    drop(files::Files::acquire(dir.path(), task.ticket(), true).unwrap());
    fs::write(
        folder.join("unknown-publication.tmp"),
        b"synthetic unfinished publication",
    )
    .unwrap();
    assert_eq!(code(task.abandon(dir.path())), "restore_mismatch");
    assert!(folder.join("unknown-publication.tmp").exists());
}

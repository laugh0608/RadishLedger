use super::*;
use crate::csv_import::{prepare, test_support::*};
use serde_json::Value;

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

fn database_v2() -> (tempfile::TempDir, DbKey, Store) {
    let directory = tempfile::tempdir().unwrap();
    let key = DbKey::random().unwrap();
    let mut store =
        Store::create_csv_experiment(&directory.path().join("import.db"), &key).unwrap();
    for (n, kind) in [(1, "cash"), (2, "bank"), (3, "credit_card")] {
        store.add_account(&id(n), kind, "合成账户").unwrap();
    }
    for (n, kind) in [(4, "income"), (5, "expense"), (6, "expense")] {
        store.add_category(&id(n), kind, "合成分类").unwrap();
    }
    (directory, key, store)
}

fn plan(store: &Store, text: &str) -> Prepared {
    prepare(store, text.as_bytes(), &mapping(), &no_cancel()).unwrap()
}

fn save(store: &mut Store, text: &str, task: usize, batch: usize) -> Task {
    let plan = plan(store, text);
    let choices = vec![Decision::Import; plan.preview().rows.len()];
    store
        .save_csv_task(&id(task), &plan, &choices, batch, &no_cancel())
        .unwrap()
}

fn counts(store: &Store) -> (u32, u32, u32, u32, i64) {
    let count = |table| {
        store
            .connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    (
        count("revisions"),
        count("outbox"),
        count("csv_sources"),
        count("csv_batches"),
        store.sequence().unwrap(),
    )
}

#[test]
fn a01_batches_preserve_notes_and_match_independent_projection_after_reopen() {
    let (directory, key, mut store) = database_v2();
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let accounting: Value = serde_json::from_str(ACCOUNTING).unwrap();
    let a01 = &accounting["cases"][0];
    for (n, step) in a01["openingEvents"].as_array().unwrap().iter().enumerate() {
        store
            .commit(&Write {
                operation_id: id(100 + n),
                event_id: id(200 + n),
                revision_id: id(300 + n),
                previous_revision_id: None,
                event: Some(event_from_step(step, true)),
                expected_sequence: None,
                observed_balance_minor: None,
                target_balance_minor: None,
                resolve_candidates: BTreeSet::new(),
            })
            .unwrap();
    }
    let task = save(&mut store, fixture["a01Input"].as_str().unwrap(), 900, 3);
    let rows = store.csv_task_rows(&task.target_id, &task.id).unwrap();
    assert_eq!(counts(&store), (3, 3, 0, 0, 3));
    for first in [1, 4] {
        let batch = store
            .publish_csv_batch(&task.target_id, &task.id, first, &no_cancel())
            .unwrap();
        assert_eq!(
            (batch.imported, batch.skipped, batch.last_record),
            (3, 0, first + 2)
        );
        let snapshot = store.snapshot(&["2026-01".into()]).unwrap();
        let step = &a01["steps"][first + 2];
        for (key, value) in normalize(serde_json::to_value(snapshot.projection).unwrap())
            .as_object()
            .unwrap()
        {
            assert_eq!(value, &step["expected"][key], "{first} {key}");
        }
        drop(store);
        store = Store::open(&directory.path().join("import.db"), &key).unwrap();
        assert!(
            store
                .publish_csv_batch(&task.target_id, &task.id, first, &AtomicBool::new(true))
                .unwrap()
                .replayed
        );
    }
    for (n, row) in rows.iter().enumerate() {
        let event = store
            .revision_event(row.revision_id.as_ref().unwrap())
            .unwrap()
            .unwrap();
        let mut expected = event_from_step(&a01["steps"][n + 1], false);
        expected.note = fixture["a01Notes"][n].as_str().unwrap().into();
        assert_eq!(event, expected);
        let intent: String = store
            .connection
            .query_row(
                "SELECT intent FROM outbox WHERE operation_id=?1",
                [row.operation_id.as_ref().unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Write>(&intent).unwrap().event,
            Some(event)
        );
    }
    assert_eq!(counts(&store), (9, 9, 6, 2, 9));
    let done = store.csv_task(&task.target_id, &task.id).unwrap();
    assert_eq!(
        (done.state, done.imported, done.skipped, done.next_record),
        (TaskState::Complete, 6, 0, 7)
    );
    let reopened_rows = store.csv_task_rows(&task.target_id, &task.id).unwrap();
    for (before, after) in rows.iter().zip(reopened_rows) {
        assert_eq!(before.row_id, after.row_id);
        assert_eq!(before.operation_id, after.operation_id);
        assert_eq!(after.outcome, RowOutcome::Imported);
    }
}

#[test]
fn plan_identity_is_immutable_target_bound_and_rechecks_stale_preview() {
    let (_directory, _key, mut store) = database_v2();
    let text = HEADER.to_owned() + ONE;
    let plan = plan(&store, &text);
    let before = store.content_digest().unwrap();
    for batch in [0, 101] {
        assert_eq!(
            store
                .save_csv_task(&id(901), &plan, &[Decision::Import], batch, &no_cancel())
                .unwrap_err()
                .code,
            "csv_plan"
        );
    }
    assert_eq!(
        store
            .save_csv_task(&id(901), &plan, &[], 1, &no_cancel())
            .unwrap_err()
            .code,
        "csv_plan"
    );
    assert_eq!(
        store
            .save_csv_task(
                &id(901),
                &plan,
                &[Decision::Import],
                1,
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .code,
        "cancelled"
    );
    assert_eq!(store.content_digest().unwrap(), before);
    let (_other_dir, _other_key, mut other) = database_v2();
    assert_eq!(
        other
            .save_csv_task(&id(901), &plan, &[Decision::Import], 1, &no_cancel())
            .unwrap_err()
            .code,
        "csv_target_changed"
    );
    let task = store
        .save_csv_task(&id(901), &plan, &[Decision::Import], 1, &no_cancel())
        .unwrap();
    let rows = store.csv_task_rows(&task.target_id, &task.id).unwrap();
    assert_eq!(
        store
            .save_csv_task(&task.id, &plan, &[Decision::Import], 1, &no_cancel())
            .unwrap(),
        task
    );
    assert_eq!(
        store.csv_task_rows(&task.target_id, &task.id).unwrap(),
        rows
    );
    assert_eq!(
        store
            .save_csv_task(&task.id, &plan, &[Decision::Skip], 1, &no_cancel())
            .unwrap_err()
            .code,
        "operation_reused"
    );
    let changed = prepare(
        &store,
        (HEADER.to_owned() + &ONE.replace("1.00", "2.00")).as_bytes(),
        &mapping(),
        &no_cancel(),
    )
    .unwrap();
    assert_eq!(
        store
            .save_csv_task(&task.id, &changed, &[Decision::Import], 1, &no_cancel())
            .unwrap_err()
            .code,
        "operation_reused"
    );
    store.archive(&[id(1)], &[]).unwrap();
    assert_eq!(
        store
            .save_csv_task(&id(902), &plan, &[Decision::Import], 1, &no_cancel())
            .unwrap_err()
            .code,
        "stale_observation"
    );
    assert_eq!(
        store
            .publish_csv_batch(&task.target_id, &task.id, 1, &no_cancel())
            .unwrap_err()
            .code,
        "archived_reference"
    );
    assert_eq!(counts(&store), (0, 0, 0, 0, 1));
}

#[test]
fn explicit_skip_cancel_resume_and_missing_source_rows_keep_fixed_identities() {
    let (directory, key, mut store) = database_v2();
    let row = ONE.replace("src-1", "");
    let text = HEADER.to_owned() + &row + &row + &ONE.replace("1.00", "bad");
    let prepared = plan(&store, &text);
    assert_eq!(
        store
            .save_csv_task(&id(900), &prepared, &[Decision::Import; 3], 2, &no_cancel())
            .unwrap_err()
            .code,
        "csv_invalid_row"
    );
    assert!(store.csv_tasks().unwrap().is_empty());
    let task = store
        .save_csv_task(
            &id(900),
            &prepared,
            &[Decision::Import, Decision::Import, Decision::Skip],
            2,
            &no_cancel(),
        )
        .unwrap();
    assert_eq!(
        store
            .publish_csv_batch(&task.target_id, &task.id, 3, &no_cancel())
            .unwrap_err()
            .code,
        "csv_batch_order"
    );
    store
        .publish_csv_batch(&task.target_id, &task.id, 1, &no_cancel())
        .unwrap();
    let stopped = store
        .set_csv_cancelled(&task.target_id, &task.id, true)
        .unwrap();
    assert_eq!(
        (stopped.state, stopped.imported, stopped.next_record),
        (TaskState::Cancelled, 2, 3)
    );
    drop(store);
    let mut store = Store::open(&directory.path().join("import.db"), &key).unwrap();
    assert_eq!(store.csv_tasks().unwrap(), vec![stopped]);
    assert_eq!(
        store
            .publish_csv_batch(&task.target_id, &task.id, 3, &no_cancel())
            .unwrap_err()
            .code,
        "csv_task_stopped"
    );
    assert!(
        store
            .publish_csv_batch(&task.target_id, &task.id, 1, &no_cancel())
            .unwrap()
            .replayed
    );
    store
        .set_csv_cancelled(&task.target_id, &task.id, false)
        .unwrap();
    let batch = store
        .publish_csv_batch(&task.target_id, &task.id, 3, &no_cancel())
        .unwrap();
    assert_eq!((batch.imported, batch.skipped), (0, 1));
    assert_eq!(counts(&store), (2, 2, 0, 2, 2));
    assert_eq!(
        store.snapshot(&[]).unwrap().projection.net_assets_minor,
        "-200"
    );
    assert_eq!(
        plan(&store, &text).previous_tasks(),
        std::slice::from_ref(&task.id)
    );
    assert_eq!(
        store
            .set_csv_cancelled(&task.target_id, &task.id, true)
            .unwrap()
            .state,
        TaskState::Complete
    );
}

#[test]
fn historical_sources_survive_edit_delete_and_reject_conflicting_payloads() {
    let (_directory, _key, mut store) = database_v2();
    let text = HEADER.to_owned() + ONE + ONE;
    let prepared = plan(&store, &text);
    assert_eq!(
        store
            .save_csv_task(&id(900), &prepared, &[Decision::Import; 2], 2, &no_cancel())
            .unwrap_err()
            .code,
        "csv_duplicate_source"
    );
    let task = store
        .save_csv_task(
            &id(900),
            &prepared,
            &[Decision::Import, Decision::Skip],
            2,
            &no_cancel(),
        )
        .unwrap();
    store
        .publish_csv_batch(&task.target_id, &task.id, 1, &no_cancel())
        .unwrap();
    let row = store
        .csv_task_rows(&task.target_id, &task.id)
        .unwrap()
        .remove(0);
    let mut edited = store
        .revision_event(row.revision_id.as_ref().unwrap())
        .unwrap()
        .unwrap();
    edited.note = "后续编辑".into();
    let mut write = Write {
        operation_id: id(800),
        event_id: row.event_id.unwrap(),
        revision_id: id(801),
        previous_revision_id: row.revision_id,
        event: Some(edited),
        expected_sequence: None,
        observed_balance_minor: None,
        target_balance_minor: None,
        resolve_candidates: BTreeSet::new(),
    };
    store.commit(&write).unwrap();
    write.previous_revision_id = Some(write.revision_id.clone());
    write.revision_id = id(802);
    write.operation_id = id(803);
    write.event = None;
    store.commit(&write).unwrap();
    let prepared = plan(&store, &text);
    assert!(matches!(
        prepared.preview().rows[0].source_match,
        SourceMatch::PreviouslyImported { .. }
    ));
    assert_eq!(
        store
            .save_csv_task(
                &id(901),
                &prepared,
                &[Decision::Import, Decision::Skip],
                2,
                &no_cancel()
            )
            .unwrap_err()
            .code,
        "csv_duplicate_source"
    );
    let changed = plan(&store, &text.replace("合成备注", "不同备注"));
    assert!(
        changed
            .preview()
            .rows
            .iter()
            .all(|r| r.errors.iter().any(|e| e.code == "csv_source_conflict"))
    );
    assert!(
        store
            .snapshot(&[])
            .unwrap()
            .deleted_event_ids
            .contains(&write.event_id)
    );
    assert_eq!(counts(&store), (3, 3, 1, 1, 3));
}

#[test]
fn concurrent_source_registration_and_reference_changes_reject_whole_batch() {
    let (directory, key, mut store) = database_v2();
    let text = HEADER.to_owned() + ONE + &ONE.replace("src-1", "src-2").replace("现金", "银行");
    let one = save(&mut store, &text, 900, 2);
    let mut other = Store::open(&directory.path().join("import.db"), &key).unwrap();
    let two = save(&mut other, &text, 901, 2);
    // 在第二行才拒绝，第一行的事实 / 来源 / 队列也必须回滚。
    other.archive(&[id(2)], &[]).unwrap();
    let before = store.content_digest().unwrap();
    assert_eq!(
        store
            .publish_csv_batch(&one.target_id, &one.id, 1, &no_cancel())
            .unwrap_err()
            .code,
        "archived_reference"
    );
    assert_eq!(store.content_digest().unwrap(), before);
    // 仅为合成调度解除归档；正式 API 不提供自动解归档 fallback。
    other
        .connection
        .execute("UPDATE accounts SET archived=0 WHERE id=?1", [id(2)])
        .unwrap();
    store
        .publish_csv_observed(&one.target_id, &one.id, 1, &no_cancel(), |point| {
            if point == ImportPoint::AfterFact || point == ImportPoint::BeforeCommit {
                assert_eq!(counts(&other), (0, 0, 0, 0, 1));
                assert_eq!(
                    other.csv_task(&one.target_id, &one.id).unwrap().next_record,
                    1
                );
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(counts(&other), (2, 2, 2, 1, 3));
    assert_eq!(
        other
            .publish_csv_batch(&two.target_id, &two.id, 1, &no_cancel())
            .unwrap_err()
            .code,
        "csv_duplicate_source"
    );
    let mut mapping = mapping();
    mapping.source_namespace = "independent-source".into();
    let different = prepare(&other, text.as_bytes(), &mapping, &no_cancel()).unwrap();
    other
        .save_csv_task(
            &id(902),
            &different,
            &[Decision::Import; 2],
            2,
            &no_cancel(),
        )
        .unwrap();
    other
        .publish_csv_batch(&two.target_id, &id(902), 1, &no_cancel())
        .unwrap();
    assert_eq!(counts(&store), (4, 4, 4, 2, 5));
}

#[test]
fn transaction_failure_cancellation_and_lost_ack_have_exact_persistent_outcomes() {
    let (_directory, _key, mut store) = database_v2();
    let task = save(
        &mut store,
        &(HEADER.to_owned() + ONE + &ONE.replace("src-1", "src-2")),
        900,
        2,
    );
    let before = store.content_digest().unwrap();
    for fail in [
        ImportPoint::AfterFact,
        ImportPoint::AfterSource,
        ImportPoint::BeforeProgress,
        ImportPoint::BeforeCommit,
    ] {
        let error = store
            .publish_csv_observed(&task.target_id, &task.id, 1, &no_cancel(), |point| {
                if point == fail {
                    Err(CoreError::new("injected_failure", "合成事务中断"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert_eq!(error.code, "injected_failure");
        assert_eq!(store.content_digest().unwrap(), before);
        let cancel = no_cancel();
        assert_eq!(
            store
                .publish_csv_observed(&task.target_id, &task.id, 1, &cancel, |point| {
                    if point == fail {
                        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    Ok(())
                })
                .unwrap_err()
                .code,
            "cancelled"
        );
        assert_eq!(store.content_digest().unwrap(), before);
    }
    store.connection.execute_batch("CREATE TRIGGER fail_source BEFORE INSERT ON csv_sources BEGIN SELECT RAISE(ABORT, 'synthetic'); END;").unwrap();
    assert_eq!(
        store
            .publish_csv_batch(&task.target_id, &task.id, 1, &no_cancel())
            .unwrap_err()
            .code,
        "storage_error"
    );
    assert_eq!(store.content_digest().unwrap(), before);
    store
        .connection
        .execute_batch("DROP TRIGGER fail_source;")
        .unwrap();
    assert_eq!(
        store
            .publish_csv_observed(&task.target_id, &task.id, 1, &no_cancel(), |point| {
                if point == ImportPoint::AfterCommit {
                    Err(CoreError::new("lost_ack", "合成丢失确认"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err()
            .code,
        "lost_ack"
    );
    assert_eq!(counts(&store), (2, 2, 2, 1, 2));
    assert!(
        store
            .publish_csv_batch(&task.target_id, &task.id, 1, &no_cancel())
            .unwrap()
            .replayed
    );
    assert_eq!(
        store.csv_task(&task.target_id, &task.id).unwrap().state,
        TaskState::Complete
    );
}

#[test]
fn v1_remains_unchanged_and_notes_require_v2_with_exact_limits() {
    let (_directory, _key, mut v1) = database();
    let before = v1.content_digest().unwrap();
    assert_eq!(
        prepare(
            &v1,
            (HEADER.to_owned() + ONE).as_bytes(),
            &mapping(),
            &no_cancel()
        )
        .err()
        .unwrap()
        .code,
        "unsupported_schema"
    );
    let candidate = show(&v1, &(HEADER.to_owned() + ONE), &mapping())
        .rows
        .remove(0)
        .candidate
        .unwrap();
    let mut write = Write {
        operation_id: id(900),
        event_id: id(901),
        revision_id: id(902),
        previous_revision_id: None,
        event: Some(candidate.fact()),
        expected_sequence: None,
        observed_balance_minor: None,
        target_balance_minor: None,
        resolve_candidates: BTreeSet::new(),
    };
    assert_eq!(v1.commit(&write).unwrap_err().code, "unsupported_schema");
    assert_eq!(v1.content_digest().unwrap(), before);
    write.event.as_mut().unwrap().note.clear();
    assert!(
        !encode(write.event.as_ref().unwrap())
            .unwrap()
            .contains("note")
    );
    v1.commit(&write).unwrap();
    let (directory, key, mut v2) = database_v2();
    for (n, len) in [4095, 4096, 4097].into_iter().enumerate() {
        write.operation_id = id(100 + n);
        write.event_id = id(200 + n);
        write.revision_id = id(300 + n);
        write.event.as_mut().unwrap().note = "a".repeat(len);
        let result = v2.commit(&write);
        if len <= 4096 {
            assert_eq!(result.unwrap(), WriteEffect::Committed);
        } else {
            assert_eq!(result.unwrap_err().code, "invalid_note");
        }
    }
    write.event.as_mut().unwrap().note = "x\0y".into();
    assert_eq!(v2.commit(&write).unwrap_err().code, "invalid_note");
    drop(v2);
    let readonly = Store::open_readonly(&directory.path().join("import.db"), &key).unwrap();
    assert_eq!(
        readonly
            .revision_event(&id(301))
            .unwrap()
            .unwrap()
            .note
            .len(),
        4096
    );
}

#[test]
fn batch_budget_accepts_both_edges_and_cancellation_after_commit_is_success() {
    for size in [1, 99, 100] {
        let (_directory, _key, mut store) = database_v2();
        let text = HEADER.to_owned() + &ONE.replace("src-1", "").repeat(100);
        let task = save(&mut store, &text, 900, size);
        let cancel = no_cancel();
        let batch = store
            .publish_csv_observed(&task.target_id, &task.id, 1, &cancel, |point| {
                if point == ImportPoint::AfterCommit {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(batch.imported, size);
        assert_eq!(
            store
                .csv_task(&task.target_id, &task.id)
                .unwrap()
                .next_record,
            size + 1
        );
    }
}

#[cfg(unix)]
#[path = "csv_crash_tests.rs"]
mod crash;

#[test]
fn task_registration_sql_failure_rolls_back_and_corrupt_progress_or_profile_is_rejected() {
    let (_directory, _key, mut store) = database_v2();
    let prepared = plan(
        &store,
        &(HEADER.to_owned() + ONE + &ONE.replace("src-1", "src-2")),
    );
    let before = store.content_digest().unwrap();
    store.connection.execute_batch("CREATE TRIGGER fail_task_row BEFORE INSERT ON csv_rows WHEN NEW.record=2 BEGIN SELECT RAISE(ABORT, 'synthetic'); END;").unwrap();
    assert_eq!(
        store
            .save_csv_task(&id(900), &prepared, &[Decision::Import; 2], 2, &no_cancel())
            .unwrap_err()
            .code,
        "storage_error"
    );
    assert_eq!(store.content_digest().unwrap(), before);
    assert!(store.csv_tasks().unwrap().is_empty());
    store
        .connection
        .execute_batch("DROP TRIGGER fail_task_row;")
        .unwrap();
    let task = store
        .save_csv_task(&id(900), &prepared, &[Decision::Import; 2], 2, &no_cancel())
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE csv_tasks SET profile='unknown' WHERE id=?1",
            [&task.id],
        )
        .unwrap();
    assert_eq!(
        store
            .publish_csv_batch(&task.target_id, &task.id, 1, &no_cancel())
            .unwrap_err()
            .code,
        "csv_profile"
    );
    store
        .connection
        .execute(
            "UPDATE csv_tasks SET profile=?2,next_record=2 WHERE id=?1",
            params![task.id, PROFILE],
        )
        .unwrap();
    assert_eq!(
        store
            .publish_csv_batch(&task.target_id, &task.id, 2, &no_cancel())
            .unwrap_err()
            .code,
        "csv_corrupt_task"
    );
    assert_eq!(counts(&store), (0, 0, 0, 0, 0));
    store
        .connection
        .execute("UPDATE csv_tasks SET next_record=1 WHERE id=?1", [&task.id])
        .unwrap();
    let intent: String = store
        .connection
        .query_row(
            "SELECT intent FROM csv_rows WHERE task_id=?1 AND record=1",
            [&task.id],
            |r| r.get(0),
        )
        .unwrap();
    let mut invalid: Write = serde_json::from_str(&intent).unwrap();
    invalid.previous_revision_id = Some(id(700));
    store
        .connection
        .execute(
            "UPDATE csv_rows SET intent=?2 WHERE task_id=?1 AND record=1",
            params![task.id, encode(&invalid).unwrap()],
        )
        .unwrap();
    assert_eq!(
        store
            .publish_csv_batch(&task.target_id, &task.id, 1, &no_cancel())
            .unwrap_err()
            .code,
        "csv_corrupt_task"
    );
    assert_eq!(counts(&store), (0, 0, 0, 0, 0));
}

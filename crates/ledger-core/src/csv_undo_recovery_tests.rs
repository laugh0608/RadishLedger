//! 持久撤销的失败、取消、恢复、格式与资源边界。
use super::*;

#[test]
fn cancellation_failure_and_lost_response_preserve_exact_committed_range() {
    let (directory, key, mut store) = database_v3();
    let import = imported(
        &mut store,
        &(HEADER.to_owned() + &ONE.replace("src-1", "").repeat(4)),
        4,
    );
    let task = confirm(&mut store, &import, 800, &[1, 2, 3, 4], 2);
    let before = store.content_digest().unwrap();
    for failure in [
        UndoPoint::AfterDelete,
        UndoPoint::AfterRow,
        UndoPoint::BeforeProgress,
        UndoPoint::BeforeCommit,
    ] {
        assert_eq!(
            store
                .publish_csv_undo_observed(&import.target_id, &task.id, 1, &no_cancel(), |point| {
                    if point == failure {
                        Err(CoreError::new("injected_failure", "合成撤销中断"))
                    } else {
                        Ok(())
                    }
                })
                .unwrap_err()
                .code,
            "injected_failure"
        );
        assert_eq!(store.content_digest().unwrap(), before);
        let cancel = no_cancel();
        assert_eq!(
            store
                .publish_csv_undo_observed(&import.target_id, &task.id, 1, &cancel, |point| {
                    if point == failure {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    Ok(())
                })
                .unwrap_err()
                .code,
            "cancelled"
        );
        assert_eq!(store.content_digest().unwrap(), before);
    }
    let readonly = Store::open_readonly(&directory.path().join("undo.db"), &key).unwrap();
    assert_eq!(
        store
            .publish_csv_undo_observed(&import.target_id, &task.id, 1, &no_cancel(), |point| {
                if point == UndoPoint::AfterDelete || point == UndoPoint::BeforeCommit {
                    assert_eq!(readonly.content_digest().unwrap(), before);
                }
                if point == UndoPoint::AfterCommit {
                    Err(CoreError::new("lost_ack", "合成确认丢失"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err()
            .code,
        "lost_ack"
    );
    assert_eq!(
        store
            .csv_undo_task(&import.target_id, &task.id)
            .unwrap()
            .deleted,
        2
    );
    assert_eq!(
        readonly
            .csv_undo_task(&import.target_id, &task.id)
            .unwrap()
            .deleted,
        2
    );
    let after_first = store.content_digest().unwrap();
    assert!(
        store
            .publish_csv_undo_batch(&import.target_id, &task.id, 1, &AtomicBool::new(true))
            .unwrap()
            .replayed
    );
    assert_eq!(store.content_digest().unwrap(), after_first);
    assert_eq!(
        store
            .set_csv_undo_cancelled(&import.target_id, &task.id, true)
            .unwrap()
            .state,
        State::Cancelled
    );
    drop(store);
    let mut store = Store::open(&directory.path().join("undo.db"), &key).unwrap();
    assert_eq!(
        store.csv_undo_tasks(&import.target_id, &import.id).unwrap()[0].state,
        State::Cancelled
    );
    assert_eq!(
        store
            .publish_csv_undo_batch(&import.target_id, &task.id, 3, &no_cancel())
            .unwrap_err()
            .code,
        "csv_undo_stopped"
    );
    store
        .set_csv_undo_cancelled(&import.target_id, &task.id, false)
        .unwrap();
    let cancel = no_cancel();
    assert_eq!(
        store
            .publish_csv_undo_observed(&import.target_id, &task.id, 3, &cancel, |point| {
                if point == UndoPoint::AfterCommit {
                    cancel.store(true, Ordering::Relaxed);
                }
                Ok(())
            })
            .unwrap()
            .deleted,
        2
    );
    assert_eq!(
        store
            .set_csv_undo_cancelled(&import.target_id, &task.id, true)
            .unwrap()
            .state,
        State::Complete
    );
    assert_eq!(
        store
            .csv_undo_rows(&import.target_id, &task.id)
            .unwrap()
            .iter()
            .filter(|r| r.deleted)
            .count(),
        4
    );
}

#[test]
fn sql_refusals_rollback_registration_facts_outbox_and_progress() {
    let (_dir, _key, mut store) = database_v3();
    let import = imported(
        &mut store,
        &(HEADER.to_owned() + &ONE.replace("src-1", "").repeat(2)),
        2,
    );
    let preview = store
        .preview_csv_undo(&import.target_id, &import.id, &no_cancel())
        .unwrap();
    let before = store.content_digest().unwrap();
    store.connection.execute_batch("CREATE TRIGGER fail_registration BEFORE INSERT ON csv_undo_rows WHEN NEW.ordinal=2 BEGIN SELECT RAISE(ABORT,'synthetic'); END;").unwrap();
    assert_eq!(
        store
            .save_csv_undo(&id(800), &preview, &[1, 2], 2, &no_cancel())
            .unwrap_err()
            .code,
        "storage_error"
    );
    assert_eq!(store.content_digest().unwrap(), before);
    store
        .connection
        .execute_batch("DROP TRIGGER fail_registration;")
        .unwrap();
    let task = store
        .save_csv_undo(&id(800), &preview, &[1, 2], 2, &no_cancel())
        .unwrap();
    let before = store.content_digest().unwrap();
    for clause in [
        "BEFORE INSERT ON outbox",
        "BEFORE UPDATE ON csv_undo_rows WHEN NEW.ordinal=2",
        "BEFORE INSERT ON csv_undo_batches",
        "BEFORE UPDATE ON csv_undo_tasks",
    ] {
        store
            .connection
            .execute_batch(&format!(
                "CREATE TRIGGER fail_undo {clause} BEGIN SELECT RAISE(ABORT,'synthetic'); END;"
            ))
            .unwrap();
        assert_eq!(
            store
                .publish_csv_undo_batch(&import.target_id, &task.id, 1, &no_cancel())
                .unwrap_err()
                .code,
            "storage_error"
        );
        assert_eq!(store.content_digest().unwrap(), before);
        store
            .connection
            .execute_batch("DROP TRIGGER fail_undo;")
            .unwrap();
    }
    store
        .publish_csv_undo_batch(&import.target_id, &task.id, 1, &no_cancel())
        .unwrap();
    assert_eq!(
        (count(&store, "revisions"), count(&store, "outbox")),
        (4, 4)
    );
}

#[test]
fn legacy_schemas_are_unchanged_and_unknown_relations_or_corrupt_intents_stop() {
    for version in [1, 2] {
        let directory = tempfile::tempdir().unwrap();
        let key = DbKey::random().unwrap();
        let path = directory.path().join("old.db");
        let old = if version == 1 {
            Store::create(&path, &key).unwrap()
        } else {
            Store::create_csv_experiment(&path, &key).unwrap()
        };
        let before = old.content_digest().unwrap();
        assert_eq!(
            old.preview_csv_undo(&id(1), &id(900), &no_cancel())
                .err()
                .unwrap()
                .code,
            "unsupported_schema"
        );
        assert_eq!(old.content_digest().unwrap(), before);
        drop(old);
        let old = Store::open(&path, &key).unwrap();
        assert_eq!(
            old.connection
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            version
        );
        assert_eq!(old.content_digest().unwrap(), before);
        assert_eq!(
            Store::create_csv_undo_experiment(&path, &key)
                .err()
                .unwrap()
                .code,
            "create_failed"
        );
    }
    let (_dir, _key, mut store) = database_v3();
    let import = imported(&mut store, &(HEADER.to_owned() + ONE), 1);
    let task = confirm(&mut store, &import, 800, &[1], 1);
    let rows = store.csv_undo_rows(&import.target_id, &task.id).unwrap();
    let mut invalid = deletion(&rows[0]);
    invalid.event = Some(original_event(&store, &import, 1));
    store
        .connection
        .execute(
            "UPDATE csv_undo_rows SET intent=?1",
            [encode(&invalid).unwrap()],
        )
        .unwrap();
    let before = store.content_digest().unwrap();
    assert_eq!(
        store
            .publish_csv_undo_batch(&import.target_id, &task.id, 1, &no_cancel())
            .unwrap_err()
            .code,
        "csv_corrupt_undo"
    );
    assert_eq!(store.content_digest().unwrap(), before);
    let mut original = serde_json::to_value(original_event(&store, &import, 1)).unwrap();
    original["linkedEventId"] = Value::String(id(999));
    store
        .connection
        .execute(
            "UPDATE revisions SET payload=?1 WHERE id=?2",
            params![original.to_string(), rows[0].original_revision_id],
        )
        .unwrap();
    let before = store.content_digest().unwrap();
    assert_eq!(
        store
            .preview_csv_undo(&import.target_id, &import.id, &no_cancel())
            .err()
            .unwrap()
            .code,
        "csv_corrupt_undo"
    );
    assert_eq!(store.content_digest().unwrap(), before);
}

#[test]
fn batch_limits_and_full_selection_budget_are_bounded() {
    for size in [1, 99, 100] {
        let (_dir, _key, mut store) = database_v3();
        let import = imported(
            &mut store,
            &(HEADER.to_owned() + &ONE.replace("src-1", "").repeat(1000)),
            100,
        );
        let records: Vec<_> = (1..=1000).collect();
        let task = confirm(&mut store, &import, 800, &records, size);
        let batch = store
            .publish_csv_undo_batch(&import.target_id, &task.id, 1, &no_cancel())
            .unwrap();
        assert_eq!(batch.deleted, size);
        assert_eq!(
            store
                .csv_undo_task(&import.target_id, &task.id)
                .unwrap()
                .next_ordinal,
            size + 1
        );
        assert_eq!(store.snapshot(&[]).unwrap().deleted_event_ids.len(), size);
    }
}

#[test]
fn preview_is_one_read_snapshot_and_cancelled_preview_does_not_write() {
    let (directory, key, mut store) = database_v3();
    let import = imported(
        &mut store,
        &(HEADER.to_owned() + &ONE.replace("src-1", "").repeat(2)),
        2,
    );
    let mut other = Store::open(&directory.path().join("undo.db"), &key).unwrap();
    let mut changed = original_event(&other, &import, 2);
    changed.note = "合成并发备注".into();
    let preview = store
        .preview_csv_undo_observed(&import.target_id, &import.id, &no_cancel(), |record| {
            if record == 1 {
                edit(&mut other, &import, 2, 1000, Some(changed.clone()));
            }
            Ok(())
        })
        .unwrap();
    assert!(preview.rows().iter().all(|r| r.restriction.is_none()));
    let after_edit = other.content_digest().unwrap();
    assert_eq!(
        store
            .save_csv_undo(&id(800), &preview, &[1, 2], 2, &no_cancel())
            .unwrap_err()
            .code,
        "csv_undo_stale"
    );
    assert_eq!(store.content_digest().unwrap(), after_edit);
    let current = store
        .preview_csv_undo(&import.target_id, &import.id, &no_cancel())
        .unwrap();
    assert_eq!(
        current.rows()[1].restriction,
        Some(Restriction::ChangedRevision)
    );
    let cancel = no_cancel();
    assert_eq!(
        store
            .preview_csv_undo_observed(&import.target_id, &import.id, &cancel, |_| {
                cancel.store(true, Ordering::Relaxed);
                Ok(())
            })
            .err()
            .unwrap()
            .code,
        "cancelled"
    );
    assert_eq!(store.content_digest().unwrap(), after_edit);
}

#[test]
fn corrupt_progress_or_source_binding_is_rejected_without_deleting() {
    let (_dir, _key, mut store) = database_v3();
    let import = imported(&mut store, &(HEADER.to_owned() + ONE), 1);
    let task = confirm(&mut store, &import, 800, &[1], 1);
    store
        .connection
        .execute(
            "UPDATE csv_undo_tasks SET next_ordinal=2,state='complete'",
            [],
        )
        .unwrap();
    let before = store.content_digest().unwrap();
    assert_eq!(
        store
            .csv_undo_task(&import.target_id, &task.id)
            .unwrap_err()
            .code,
        "csv_corrupt_undo"
    );
    assert_eq!(
        store
            .publish_csv_undo_batch(&import.target_id, &task.id, 1, &no_cancel())
            .unwrap_err()
            .code,
        "csv_corrupt_undo"
    );
    assert_eq!(store.content_digest().unwrap(), before);
    store
        .connection
        .execute("UPDATE csv_undo_tasks SET next_ordinal=1,state='ready'", [])
        .unwrap();
    store
        .connection
        .execute("UPDATE csv_sources SET payload='{}'", [])
        .unwrap();
    let before = store.content_digest().unwrap();
    assert_eq!(
        store
            .preview_csv_undo(&import.target_id, &import.id, &no_cancel())
            .err()
            .unwrap()
            .code,
        "csv_corrupt_undo"
    );
    assert_eq!(
        store
            .publish_csv_undo_batch(&import.target_id, &task.id, 1, &no_cancel())
            .unwrap_err()
            .code,
        "csv_corrupt_undo"
    );
    assert_eq!(store.content_digest().unwrap(), before);
    assert_eq!(store.snapshot(&[]).unwrap().deleted_event_ids.len(), 0);
}

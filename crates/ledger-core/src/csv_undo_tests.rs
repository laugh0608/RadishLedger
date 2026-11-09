use super::*;
use crate::csv_import::{Decision, SourceMatch, prepare, test_support::*};
use serde_json::Value;
use std::sync::atomic::Ordering;

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

fn database_v3() -> (tempfile::TempDir, DbKey, Store) {
    let directory = tempfile::tempdir().unwrap();
    let key = DbKey::random().unwrap();
    let mut store =
        Store::create_csv_undo_experiment(&directory.path().join("undo.db"), &key).unwrap();
    for (n, kind) in [(1, "cash"), (2, "bank"), (3, "credit_card")] {
        store.add_account(&id(n), kind, "合成账户").unwrap();
    }
    for (n, kind) in [(4, "income"), (5, "expense"), (6, "expense")] {
        store.add_category(&id(n), kind, "合成分类").unwrap();
    }
    (directory, key, store)
}

fn imported(store: &mut Store, text: &str, batch_size: usize) -> crate::csv_import::Task {
    let plan = prepare(store, text.as_bytes(), &mapping(), &no_cancel()).unwrap();
    let mut task = store
        .save_csv_task(
            &id(900),
            &plan,
            &vec![Decision::Import; plan.preview().rows.len()],
            batch_size,
            &no_cancel(),
        )
        .unwrap();
    while task.next_record <= task.total {
        store
            .publish_csv_batch(&task.target_id, &task.id, task.next_record, &no_cancel())
            .unwrap();
        task = store.csv_task(&task.target_id, &task.id).unwrap();
    }
    task
}

fn confirm(
    store: &mut Store,
    import: &crate::csv_import::Task,
    id_number: usize,
    records: &[usize],
    batch_size: usize,
) -> Task {
    let preview = store
        .preview_csv_undo(&import.target_id, &import.id, &no_cancel())
        .unwrap();
    store
        .save_csv_undo(&id(id_number), &preview, records, batch_size, &no_cancel())
        .unwrap()
}

fn edit(
    store: &mut Store,
    import: &crate::csv_import::Task,
    record: usize,
    n: usize,
    event: Option<Event>,
) -> String {
    let row = store
        .csv_task_rows(&import.target_id, &import.id)
        .unwrap()
        .remove(record - 1);
    let event_id = row.event_id.unwrap();
    let parent: String = store
        .connection
        .query_row(
            "SELECT revision_id FROM selected WHERE event_id=?1",
            [&event_id],
            |r| r.get(0),
        )
        .unwrap();
    store
        .commit(&Write {
            operation_id: id(n),
            event_id,
            revision_id: id(n + 1),
            previous_revision_id: Some(parent),
            event,
            expected_sequence: None,
            observed_balance_minor: None,
            target_balance_minor: None,
            resolve_candidates: BTreeSet::new(),
        })
        .unwrap();
    id(n + 1)
}

fn original_event(store: &Store, task: &crate::csv_import::Task, record: usize) -> Event {
    let row = store
        .csv_task_rows(&task.target_id, &task.id)
        .unwrap()
        .remove(record - 1);
    store
        .revision_event(row.revision_id.as_ref().unwrap())
        .unwrap()
        .unwrap()
}

fn count(store: &Store, table: &str) -> usize {
    store
        .connection
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
            index(r, 0)
        })
        .unwrap()
}

fn check_projection(store: &Store, expected: &Value) {
    let projection = normalize(
        serde_json::to_value(store.snapshot(&["2026-01".into()]).unwrap().projection).unwrap(),
    );
    for (key, value) in projection.as_object().unwrap() {
        assert_eq!(value, &expected[key], "projection {key}");
    }
}

#[test]
fn e07_note_race_rejects_whole_batch_then_explicit_subset_matches_a01() {
    let (directory, key, mut store) = database_v3();
    assert!(store.cipher_version().unwrap().starts_with("4.19.0"));
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
    let text = fixture["a01Input"].as_str().unwrap();
    let import = imported(&mut store, text, 3);
    let import_rows = store.csv_task_rows(&import.target_id, &import.id).unwrap();
    let mut first = original_event(&store, &import, 1);
    first.note = "合成后续备注一".into();
    let edited_first = edit(&mut store, &import, 1, 1000, Some(first.clone()));
    let before_preview = store.content_digest().unwrap();
    let preview = store
        .preview_csv_undo(&import.target_id, &import.id, &no_cancel())
        .unwrap();
    assert_eq!(store.content_digest().unwrap(), before_preview);
    assert_eq!(
        preview.rows()[0].restriction,
        Some(Restriction::ChangedRevision)
    );
    assert!(preview.rows()[1..].iter().all(|r| r.restriction.is_none()));
    let stale_plan = store
        .save_csv_undo(&id(800), &preview, &[2, 3, 4, 5, 6], 5, &no_cancel())
        .unwrap();
    let mut second = original_event(&store, &import, 2);
    second.note = "合成后续备注二".into();
    let edited_second = edit(&mut store, &import, 2, 1010, Some(second.clone()));
    let before = store.content_digest().unwrap();
    assert_eq!(
        store
            .publish_csv_undo_batch(&import.target_id, &stale_plan.id, 1, &no_cancel())
            .unwrap_err()
            .code,
        "csv_undo_stale"
    );
    assert_eq!(store.content_digest().unwrap(), before);
    check_projection(&store, &a01["steps"][6]["expected"]);
    assert_eq!(
        store
            .csv_undo_task(&import.target_id, &stale_plan.id)
            .unwrap()
            .deleted,
        0
    );
    let preview = store
        .preview_csv_undo(&import.target_id, &import.id, &no_cancel())
        .unwrap();
    assert!(
        preview.rows()[..2]
            .iter()
            .all(|r| r.restriction == Some(Restriction::ChangedRevision))
    );
    let undo = store
        .save_csv_undo(&id(801), &preview, &[3, 4, 5, 6], 4, &no_cancel())
        .unwrap();
    let identities = store.csv_undo_rows(&import.target_id, &undo.id).unwrap();
    assert_eq!(
        store
            .publish_csv_undo_batch(&import.target_id, &undo.id, 1, &no_cancel())
            .unwrap()
            .deleted,
        4
    );
    drop(store);
    let mut store = Store::open(&directory.path().join("undo.db"), &key).unwrap();
    check_projection(&store, &a01["steps"][2]["expected"]);
    let snapshot = store.snapshot(&[]).unwrap();
    let mut active: BTreeSet<String> = [id(200), id(201), id(202)].into();
    active.extend(import_rows[..2].iter().map(|r| r.event_id.clone().unwrap()));
    assert_eq!(snapshot.active_event_ids, active);
    assert_eq!(
        snapshot.deleted_event_ids,
        import_rows[2..]
            .iter()
            .map(|r| r.event_id.clone().unwrap())
            .collect()
    );
    assert!(snapshot.unresolved_candidate_revision_ids.is_empty());
    assert_eq!(
        snapshot.selected_revision_ids[import_rows[0].event_id.as_ref().unwrap()],
        edited_first
    );
    assert_eq!(
        snapshot.selected_revision_ids[import_rows[1].event_id.as_ref().unwrap()],
        edited_second
    );
    assert_eq!(store.revision_event(&edited_first).unwrap(), Some(first));
    assert_eq!(store.revision_event(&edited_second).unwrap(), Some(second));
    assert_eq!(
        store.csv_task(&import.target_id, &import.id).unwrap(),
        import
    );
    assert_eq!(
        store.csv_task_rows(&import.target_id, &import.id).unwrap(),
        import_rows
    );
    assert_eq!(
        (
            count(&store, "revisions"),
            count(&store, "outbox"),
            count(&store, "csv_sources"),
            count(&store, "csv_batches")
        ),
        (15, 15, 6, 2)
    );
    for row in &identities {
        let evidence: (String, String, Option<String>) = store
            .connection
            .query_row(
                "SELECT event_id,predecessor,payload FROM revisions WHERE id=?1",
                [&row.revision_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            evidence,
            (row.event_id.clone(), row.original_revision_id.clone(), None)
        );
        let intent: String = store
            .connection
            .query_row(
                "SELECT intent FROM outbox WHERE operation_id=?1",
                [&row.operation_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(intent, encode(&deletion(row)).unwrap());
        let source_revision: String = store
            .connection
            .query_row(
                "SELECT revision_id FROM csv_sources WHERE source_id=?1",
                [format!("src-{}", row.record)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(source_revision, row.original_revision_id);
    }
    let after = store.content_digest().unwrap();
    assert!(
        store
            .publish_csv_undo_batch(&import.target_id, &undo.id, 1, &AtomicBool::new(true))
            .unwrap()
            .replayed
    );
    assert_eq!(
        store
            .save_csv_undo(&undo.id, &preview, &[3, 4, 5, 6], 4, &no_cancel())
            .unwrap()
            .state,
        State::Complete
    );
    assert_eq!(store.content_digest().unwrap(), after);
    let again = store
        .preview_csv_undo(&import.target_id, &import.id, &no_cancel())
        .unwrap();
    assert!(
        again.rows()[2..]
            .iter()
            .all(|r| r.restriction == Some(Restriction::AlreadyUndone))
    );
    let reimport = prepare(&store, text.as_bytes(), &mapping(), &no_cancel()).unwrap();
    assert!(
        reimport
            .preview()
            .rows
            .iter()
            .all(|r| matches!(r.source_match, SourceMatch::PreviouslyImported { .. }))
    );
    assert_eq!(
        store
            .save_csv_task(&id(901), &reimport, &[Decision::Import; 6], 3, &no_cancel())
            .unwrap_err()
            .code,
        "csv_duplicate_source"
    );
    assert!(
        store
            .publish_csv_batch(&import.target_id, &import.id, 1, &no_cancel())
            .unwrap()
            .replayed
    );
    assert_eq!(store.content_digest().unwrap(), after);
}

#[test]
fn confirmed_set_is_immutable_target_bound_and_checked_again_at_registration() {
    let (_dir, _key, mut store) = database_v3();
    let import = imported(
        &mut store,
        &(HEADER.to_owned() + &ONE.replace("src-1", "").repeat(3)),
        3,
    );
    let preview = store
        .preview_csv_undo(&import.target_id, &import.id, &no_cancel())
        .unwrap();
    let before = store.content_digest().unwrap();
    for records in [
        vec![],
        vec![0],
        vec![4],
        vec![1, 1],
        vec![2, 1],
        vec![usize::MAX],
        vec![1; 1001],
    ] {
        assert_eq!(
            store
                .save_csv_undo(&id(800), &preview, &records, 1, &no_cancel())
                .unwrap_err()
                .code,
            "csv_undo_plan"
        );
    }
    for size in [0, 101, usize::MAX] {
        assert_eq!(
            store
                .save_csv_undo(&id(800), &preview, &[1], size, &no_cancel())
                .unwrap_err()
                .code,
            "csv_undo_plan"
        );
    }
    assert_eq!(
        store
            .save_csv_undo(&id(800), &preview, &[1], 1, &AtomicBool::new(true))
            .unwrap_err()
            .code,
        "cancelled"
    );
    let (_other_dir, _other_key, mut other) = database_v3();
    assert_eq!(
        other
            .save_csv_undo(&id(800), &preview, &[1], 1, &no_cancel())
            .unwrap_err()
            .code,
        "csv_target_changed"
    );
    assert_eq!(store.content_digest().unwrap(), before);
    let task = store
        .save_csv_undo(&id(800), &preview, &[1, 3], 1, &no_cancel())
        .unwrap();
    let rows = store.csv_undo_rows(&import.target_id, &task.id).unwrap();
    assert_eq!(
        store
            .save_csv_undo(&id(800), &preview, &[1, 3], 1, &no_cancel())
            .unwrap(),
        task
    );
    for (records, size) in [(vec![1], 1), (vec![1, 3], 2)] {
        assert_eq!(
            store
                .save_csv_undo(&id(800), &preview, &records, size, &no_cancel())
                .unwrap_err()
                .code,
            "operation_reused"
        );
    }
    assert_eq!(
        store.csv_undo_rows(&import.target_id, &task.id).unwrap(),
        rows
    );
    let original = original_event(&store, &import, 2);
    edit(&mut store, &import, 2, 1000, Some(original));
    assert_eq!(
        store
            .save_csv_undo(&id(801), &preview, &[2], 1, &no_cancel())
            .unwrap_err()
            .code,
        "csv_undo_stale"
    );
    let restricted = store
        .preview_csv_undo(&import.target_id, &import.id, &no_cancel())
        .unwrap();
    assert_eq!(
        store
            .save_csv_undo(&id(801), &restricted, &[2], 1, &no_cancel())
            .unwrap_err()
            .code,
        "csv_undo_plan"
    );
    assert_eq!(
        store
            .csv_undo_task(&import.target_id, &id(999))
            .unwrap_err()
            .code,
        "csv_missing_undo"
    );
    for first in [0, 2, 1001, usize::MAX] {
        assert_eq!(
            store
                .publish_csv_undo_batch(&import.target_id, &task.id, first, &no_cancel())
                .unwrap_err()
                .code,
            "csv_undo_order"
        );
    }
}

#[test]
fn conflicts_deleted_restored_and_unselected_history_are_restricted() {
    let (_dir, _key, mut store) = database_v3();
    let import = imported(
        &mut store,
        &(HEADER.to_owned() + &ONE.replace("src-1", "").repeat(4)),
        4,
    );
    let rows = store.csv_task_rows(&import.target_id, &import.id).unwrap();
    let original = original_event(&store, &import, 1);
    edit(&mut store, &import, 1, 1000, Some(original.clone()));
    store
        .receive_candidate(
            rows[0].event_id.as_ref().unwrap(),
            &id(1100),
            rows[0].revision_id.as_ref().unwrap(),
            &original,
        )
        .unwrap();
    edit(&mut store, &import, 2, 1010, None);
    edit(&mut store, &import, 2, 1020, Some(original.clone()));
    edit(&mut store, &import, 3, 1030, None);
    // 无候选登记的额外历史也不能被当作“没有依赖”；保守停止该行。
    store
        .connection
        .execute(
            "INSERT INTO revisions VALUES (?1,?2,?3,?4)",
            params![
                id(1200),
                rows[3].event_id,
                rows[3].revision_id,
                encode(&original).unwrap()
            ],
        )
        .unwrap();
    let before = store.content_digest().unwrap();
    let preview = store
        .preview_csv_undo(&import.target_id, &import.id, &no_cancel())
        .unwrap();
    assert_eq!(
        preview
            .rows()
            .iter()
            .map(|r| r.restriction)
            .collect::<Vec<_>>(),
        vec![
            Some(Restriction::ConflictCandidates),
            Some(Restriction::ChangedRevision),
            Some(Restriction::Deleted),
            Some(Restriction::DependentHistory)
        ]
    );
    assert_eq!(store.content_digest().unwrap(), before);
}

#[test]
fn candidate_race_and_two_confirmed_writers_cannot_delete_changed_facts() {
    let (directory, key, mut store) = database_v3();
    let import = imported(&mut store, &(HEADER.to_owned() + ONE), 1);
    let one = confirm(&mut store, &import, 800, &[1], 1);
    let mut other = Store::open(&directory.path().join("undo.db"), &key).unwrap();
    // 当前核心允许接收某个历史分支的并发候选，候选集合变化也要阻止删除。
    let original = original_event(&store, &import, 1);
    let row = store
        .csv_task_rows(&import.target_id, &import.id)
        .unwrap()
        .remove(0);
    store
        .connection
        .execute(
            "INSERT INTO revisions VALUES (?1,?2,?3,?4)",
            params![
                id(1100),
                row.event_id,
                row.revision_id,
                encode(&original).unwrap()
            ],
        )
        .unwrap();
    other
        .receive_candidate(
            row.event_id.as_ref().unwrap(),
            &id(1101),
            &id(1100),
            &original,
        )
        .unwrap();
    let before = store.content_digest().unwrap();
    assert_eq!(
        store
            .publish_csv_undo_batch(&import.target_id, &one.id, 1, &no_cancel())
            .unwrap_err()
            .code,
        "csv_undo_stale"
    );
    assert_eq!(store.content_digest().unwrap(), before);
    // 独立干净分支验证两个已登记撤销竞争；不清除原冲突制造通过。
    let (directory, key, mut clean) = database_v3();
    let import = imported(&mut clean, &(HEADER.to_owned() + ONE), 1);
    let first = confirm(&mut clean, &import, 800, &[1], 1);
    let second = confirm(&mut clean, &import, 801, &[1], 1);
    let mut other = Store::open(&directory.path().join("undo.db"), &key).unwrap();
    clean
        .publish_csv_undo_batch(&import.target_id, &first.id, 1, &no_cancel())
        .unwrap();
    let before = other.content_digest().unwrap();
    assert_eq!(
        other
            .publish_csv_undo_batch(&import.target_id, &second.id, 1, &no_cancel())
            .unwrap_err()
            .code,
        "csv_undo_stale"
    );
    assert_eq!(other.content_digest().unwrap(), before);
}

#[test]
fn pending_and_skipped_rows_are_excluded_and_shared_references_are_retained() {
    let (_dir, _key, mut store) = database_v3();
    let text = HEADER.to_owned() + &ONE.replace("src-1", "").repeat(3);
    let plan = prepare(&store, text.as_bytes(), &mapping(), &no_cancel()).unwrap();
    let import = store
        .save_csv_task(
            &id(900),
            &plan,
            &[Decision::Import, Decision::Skip, Decision::Import],
            1,
            &no_cancel(),
        )
        .unwrap();
    for first in [1, 2] {
        store
            .publish_csv_batch(&import.target_id, &import.id, first, &no_cancel())
            .unwrap();
    }
    let preview = store
        .preview_csv_undo(&import.target_id, &import.id, &no_cancel())
        .unwrap();
    assert_eq!(
        preview
            .rows()
            .iter()
            .map(|r| r.restriction)
            .collect::<Vec<_>>(),
        vec![None, Some(Restriction::Skipped), Some(Restriction::Pending)]
    );
    let undo = store
        .save_csv_undo(&id(800), &preview, &[1], 1, &no_cancel())
        .unwrap();
    store
        .publish_csv_batch(&import.target_id, &import.id, 3, &no_cancel())
        .unwrap();
    store.archive(&[id(1)], &[id(5)]).unwrap();
    store
        .publish_csv_undo_batch(&import.target_id, &undo.id, 1, &no_cancel())
        .unwrap();
    let snapshot = store.snapshot(&[]).unwrap();
    assert_eq!(snapshot.active_event_ids.len(), 1);
    assert_eq!(snapshot.deleted_event_ids.len(), 1);
    assert_eq!(snapshot.archived_account_ids, [id(1)].into());
    assert_eq!(snapshot.archived_category_ids, [id(5)].into());
    assert_eq!(
        (count(&store, "accounts"), count(&store, "categories")),
        (3, 3)
    );
    assert!(
        store
            .publish_csv_batch(&import.target_id, &import.id, 1, &no_cancel())
            .unwrap()
            .replayed
    );
    assert_eq!(store.snapshot(&[]).unwrap().deleted_event_ids.len(), 1);
}

#[test]
fn original_receipt_never_redeletes_explicitly_restored_revision() {
    let (_dir, _key, mut store) = database_v3();
    let import = imported(&mut store, &(HEADER.to_owned() + ONE), 1);
    let original = original_event(&store, &import, 1);
    let task = confirm(&mut store, &import, 800, &[1], 1);
    store
        .publish_csv_undo_batch(&import.target_id, &task.id, 1, &no_cancel())
        .unwrap();
    let restored = edit(&mut store, &import, 1, 1000, Some(original));
    let before = store.content_digest().unwrap();
    assert!(
        store
            .publish_csv_undo_batch(&import.target_id, &task.id, 1, &no_cancel())
            .unwrap()
            .replayed
    );
    assert_eq!(store.content_digest().unwrap(), before);
    let preview = store
        .preview_csv_undo(&import.target_id, &import.id, &no_cancel())
        .unwrap();
    assert_eq!(
        preview.rows()[0].restriction,
        Some(Restriction::AlreadyUndone)
    );
    assert_eq!(
        store
            .snapshot(&[])
            .unwrap()
            .selected_revision_ids
            .values()
            .collect::<Vec<_>>(),
        vec![&restored]
    );
}

#[path = "csv_undo_recovery_tests.rs"]
mod recovery;

#[cfg(unix)]
#[path = "csv_undo_crash_tests.rs"]
mod crash;

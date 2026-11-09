//! 使用原 A04 / A06 / A11 人工预期，另核对完整事实与任务集合。
use super::*;
use crate::csv_import::{prepare, test_support::*};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}
fn limits() -> Limits {
    Limits {
        rows: MAX_ROWS,
        bytes: MAX_BYTES,
    }
}
fn months() -> Vec<String> {
    vec!["2026-01".into()]
}
fn capture(store: &Store) -> LogicalSnapshot {
    store
        .logical_snapshot(limits(), &months(), &no_cancel())
        .unwrap()
}

fn fixture(
    case_id: &str,
    step: &str,
) -> (tempfile::TempDir, DbKey, Store, BTreeMap<String, String>) {
    let directory = tempfile::tempdir().unwrap();
    let key = DbKey::random().unwrap();
    let mut store = Store::create(&directory.path().join("snapshot.db"), &key).unwrap();
    let aliases = crate::probe::populate_snapshot_fixture(&mut store, case_id, step).unwrap();
    (directory, key, store, aliases)
}

fn label_id(aliases: &BTreeMap<String, String>, label: &str) -> String {
    aliases
        .iter()
        .find(|(_, v)| v.as_str() == label)
        .unwrap()
        .0
        .clone()
}

fn normalized(value: Value, aliases: &BTreeMap<String, String>) -> Value {
    match value {
        Value::String(s) => Value::String(aliases.get(&s).cloned().unwrap_or(s)),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    (
                        aliases.get(&k).cloned().unwrap_or(k),
                        normalized(v, aliases),
                    )
                })
                .collect(),
        ),
        Value::Array(values) => {
            let mut values = values
                .into_iter()
                .map(|v| normalized(v, aliases))
                .collect::<Vec<_>>();
            if values.iter().all(Value::is_string) {
                values.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
            }
            Value::Array(values)
        }
        other => other,
    }
}

fn expected(case_id: &str, step_id: &str) -> Value {
    let cases: Value = serde_json::from_str(ACCOUNTING).unwrap();
    cases["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == case_id)
        .unwrap()["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == step_id)
        .unwrap()["expected"]
        .clone()
}

pub(crate) fn assert_expected(
    contents: &Contents,
    aliases: &BTreeMap<String, String>,
    case: &str,
    step: &str,
) {
    let actual = normalized(
        serde_json::to_value(&contents.verification).unwrap(),
        aliases,
    );
    let expected = normalized(expected(case, step), &BTreeMap::new());
    for (key, value) in expected.as_object().unwrap() {
        if key == "selectedRevisionIds" {
            for (event, revision) in value.as_object().unwrap() {
                assert_eq!(&actual[key][event], revision);
            }
        } else {
            assert_eq!(&actual[key], value, "{case}/{step}/{key}");
        }
    }
}

#[test]
fn a11_preserves_every_revision_candidate_tombstone_and_original_intent() {
    let (directory, key, store, aliases) = fixture("A11", "conflict");
    let before = store.content_digest().unwrap();
    let snapshot = capture(&store);
    assert_expected(snapshot.contents(), &aliases, "A11", "conflict");
    let facts = &snapshot.contents().facts;
    assert_eq!(
        (
            facts.accounts.len(),
            facts.categories.len(),
            facts.revisions.len(),
            facts.selected.len(),
            facts.candidates.len(),
            facts.intent_history.len()
        ),
        (2, 1, 8, 4, 2, 7)
    );
    assert_eq!((facts.source_schema, facts.sequence), (1, 8));
    assert!(facts.imports.is_none());
    let actual = facts
        .revisions
        .iter()
        .map(|r| {
            (
                aliases[&r.id].clone(),
                r.predecessor.as_ref().map(|p| aliases[p].clone()),
                r.event.as_ref().map(|e| e.amount_minor.clone()),
            )
        })
        .collect::<BTreeSet<_>>();
    let cases: Value = serde_json::from_str(ACCOUNTING).unwrap();
    let case = cases["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "A11")
        .unwrap();
    let mut independent = BTreeSet::new();
    for row in case["openingEvents"]
        .as_array()
        .unwrap()
        .iter()
        .chain(case["steps"].as_array().unwrap())
    {
        independent.insert((
            row["revisionId"].as_str().unwrap().to_owned(),
            row["previousRevisionId"].as_str().map(str::to_owned),
            row["amountMinor"]
                .as_str()
                .or_else(|| row["deltaMinor"].as_str())
                .map(str::to_owned),
        ));
    }
    assert_eq!(actual, independent);
    let remote = label_id(&aliases, "expense-v2-remote");
    assert!(!facts.intent_history.iter().any(|w| w.revision_id == remote));
    for write in &facts.intent_history {
        let raw: String = store
            .connection
            .query_row(
                "SELECT intent FROM outbox WHERE operation_id=?1",
                [&write.operation_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<Write>(&raw).unwrap(), *write);
    }
    assert_eq!(store.content_digest().unwrap(), before);
    let encoded = snapshot.encode(MAX_BYTES, &no_cancel()).unwrap();
    drop(store);
    let readonly = Store::open_readonly(&directory.path().join("snapshot.db"), &key).unwrap();
    assert_eq!(
        capture(&readonly).encode(MAX_BYTES, &no_cancel()).unwrap(),
        encoded
    );
    assert_eq!(readonly.content_digest().unwrap(), before);
}

#[test]
fn e09_a04_concurrent_restore_never_mixes_old_facts_with_new_projection() {
    for point in [
        CapturePoint::Header,
        CapturePoint::References,
        CapturePoint::Revisions,
        CapturePoint::Selections,
        CapturePoint::Intents,
        CapturePoint::Imports,
        CapturePoint::BeforeFinish,
    ] {
        let (directory, key, store, aliases) = fixture("A04", "delete");
        let mut writer = Store::open(&directory.path().join("snapshot.db"), &key).unwrap();
        let prior = writer
            .revision_event(&label_id(&aliases, "transfer-v2"))
            .unwrap()
            .unwrap();
        let original_sequence = store.sequence().unwrap();
        let mut restored = false;
        let old = store
            .logical_snapshot_observed(limits(), &months(), &no_cancel(), |observed| {
                if observed == point {
                    writer
                        .commit(&Write {
                            operation_id: id(1000),
                            event_id: label_id(&aliases, "transfer"),
                            revision_id: id(1001),
                            previous_revision_id: Some(label_id(&aliases, "transfer-v3")),
                            event: Some(prior.clone()),
                            expected_sequence: None,
                            observed_balance_minor: None,
                            target_balance_minor: None,
                            resolve_candidates: BTreeSet::new(),
                        })
                        .unwrap();
                    restored = true;
                }
                Ok(())
            })
            .unwrap();
        assert!(restored);
        assert_expected(old.contents(), &aliases, "A04", "delete");
        assert_eq!(old.contents().facts.sequence, original_sequence);
        assert_eq!(
            (
                old.contents().facts.revisions.len(),
                old.contents().facts.intent_history.len()
            ),
            (5, 5)
        );
        let mut new_aliases = aliases.clone();
        new_aliases.insert(id(1001), "transfer-v4".into());
        let current = capture(&writer);
        assert_expected(current.contents(), &new_aliases, "A04", "restore");
        assert_eq!(
            (
                current.contents().facts.revisions.len(),
                current.contents().facts.sequence
            ),
            (6, original_sequence + 1)
        );
    }
}

#[test]
fn a06_archived_names_and_references_remain_valid_historical_content() {
    let (_dir, _key, store, aliases) = fixture("A06", "renamed");
    let snapshot = capture(&store);
    assert_expected(snapshot.contents(), &aliases, "A06", "renamed");
    assert_eq!(snapshot.contents().facts.accounts[0].name, "合成现金账户");
    assert_eq!(snapshot.contents().facts.categories[0].name, "合成餐饮分类");
    assert!(snapshot.contents().facts.accounts[0].archived);
    assert!(snapshot.contents().facts.categories[0].archived);
    assert_eq!(snapshot.contents().facts.revisions.len(), 2);
}

fn import_database(version: i64) -> (tempfile::TempDir, DbKey, Store) {
    let dir = tempfile::tempdir().unwrap();
    let key = DbKey::random().unwrap();
    let mut store = if version == 2 {
        Store::create_csv_experiment(&dir.path().join("import.db"), &key).unwrap()
    } else {
        Store::create_csv_undo_experiment(&dir.path().join("import.db"), &key).unwrap()
    };
    for (n, kind) in [(1, "cash"), (2, "bank"), (3, "credit_card")] {
        store.add_account(&id(n), kind, "合成账户").unwrap();
    }
    for (n, kind) in [(4, "income"), (5, "expense"), (6, "expense")] {
        store.add_category(&id(n), kind, "合成分类").unwrap();
    }
    (dir, key, store)
}

fn pending_imports_and_undo(store: &mut Store) -> (String, String) {
    let mut map = mapping();
    map.fields.push(crate::csv_import::Field::Ignore);
    let header = HEADER.trim_end().to_owned() + ",忽略列\n";
    let mut text = header;
    for n in 1..=6 {
        text.push_str(
            &(ONE.trim_end().replace("src-1", &format!("src-{n}")) + ",excluded-raw-csv-marker\n"),
        );
    }
    let prepared = prepare(store, text.as_bytes(), &map, &no_cancel()).unwrap();
    let task = store
        .save_csv_task(
            &id(900),
            &prepared,
            &[
                Decision::Import,
                Decision::Import,
                Decision::Skip,
                Decision::Import,
                Decision::Import,
                Decision::Skip,
            ],
            2,
            &no_cancel(),
        )
        .unwrap();
    for first in [1, 3] {
        store
            .publish_csv_batch(&task.target_id, &task.id, first, &no_cancel())
            .unwrap();
    }
    store
        .set_csv_cancelled(&task.target_id, &task.id, true)
        .unwrap();
    let preview = store
        .preview_csv_undo(&task.target_id, &task.id, &no_cancel())
        .unwrap();
    store
        .save_csv_undo(&id(800), &preview, &[1, 2, 4], 1, &no_cancel())
        .unwrap();
    store
        .publish_csv_undo_batch(&task.target_id, &id(800), 1, &no_cancel())
        .unwrap();
    store
        .set_csv_undo_cancelled(&task.target_id, &id(800), true)
        .unwrap();
    (task.target_id, task.id)
}

#[test]
fn v3_preserves_partial_import_skip_cancel_and_undo_identities_without_raw_input() {
    let (dir, key, mut store) = import_database(3);
    let (target, task) = pending_imports_and_undo(&mut store);
    store.bind_bootstrap(&id(700), &id(701)).unwrap();
    store
        .connection
        .execute(
            "INSERT INTO bootstrap_fixture VALUES (1,'excluded-bootstrap-marker')",
            [],
        )
        .unwrap();
    store.archive(&[id(1)], &[id(5)]).unwrap();
    let before = store.content_digest().unwrap();
    let snapshot = capture(&store);
    let history = snapshot.contents().facts.imports.as_ref().unwrap();
    assert_eq!(history.source_target_id, target);
    assert_eq!(
        (
            history.tasks.len(),
            history.rows.len(),
            history.sources.len(),
            history.batches.len()
        ),
        (1, 6, 3, 2)
    );
    assert_eq!(
        (
            history.undo_tasks.len(),
            history.undo_rows.len(),
            history.undo_batches.len()
        ),
        (1, 3, 1)
    );
    assert_eq!(
        (
            history.tasks[0].state,
            history.tasks[0].next_record,
            history.tasks[0].batch_size
        ),
        (TaskState::Cancelled, 5, 2)
    );
    assert_eq!(
        (
            history.undo_tasks[0].state,
            history.undo_tasks[0].next_ordinal
        ),
        (State::Cancelled, 2)
    );
    assert_eq!(
        history
            .undo_rows
            .iter()
            .map(|r| r.deleted)
            .collect::<Vec<_>>(),
        vec![true, false, false]
    );
    let original = store.csv_task_rows(&target, &task).unwrap();
    for (n, row) in history.rows.iter().enumerate() {
        assert_eq!(row.row_id, original[n].row_id);
        assert_eq!(
            row.intent.as_ref().map(|w| w.operation_id.clone()),
            original[n].operation_id
        );
    }
    let undo = store.csv_undo_rows(&target, &id(800)).unwrap();
    for (a, b) in history.undo_rows.iter().zip(undo) {
        assert_eq!(
            (&a.row_id, &a.intent.operation_id, &a.intent.revision_id),
            (&b.row_id, &b.operation_id, &b.revision_id)
        );
    }
    assert_eq!(
        (
            snapshot.contents().facts.revisions.len(),
            snapshot.contents().facts.intent_history.len()
        ),
        (4, 4)
    );
    let encoded = snapshot.encode(MAX_BYTES, &no_cancel()).unwrap();
    let value: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["facts"]["remoteKnowledge"], "unknown");
    assert!(value["facts"]["imports"]["tasks"][0].get("input").is_none());
    for excluded in [
        "excluded-raw-csv-marker",
        "excluded-bootstrap-marker",
        "bootstrap_identity",
        "bootstrap_fixture",
    ] {
        assert!(!std::str::from_utf8(&encoded).unwrap().contains(excluded));
    }
    assert_eq!(store.content_digest().unwrap(), before);
    drop(store);
    let reopened = Store::open(&dir.path().join("import.db"), &key).unwrap();
    assert_eq!(
        capture(&reopened).encode(MAX_BYTES, &no_cancel()).unwrap(),
        encoded
    );
    assert_eq!(reopened.csv_task(&target, &task).unwrap().next_record, 5);
    assert_eq!(
        reopened
            .csv_undo_task(&target, &id(800))
            .unwrap()
            .next_ordinal,
        2
    );
}

#[test]
fn v2_history_is_captured_without_inventing_undo_or_upgrading_schema() {
    let (_dir, _key, mut store) = import_database(2);
    let prepared = prepare(
        &store,
        (HEADER.to_owned() + ONE).as_bytes(),
        &mapping(),
        &no_cancel(),
    )
    .unwrap();
    let task = store
        .save_csv_task(&id(900), &prepared, &[Decision::Import], 1, &no_cancel())
        .unwrap();
    store
        .publish_csv_batch(&task.target_id, &task.id, 1, &no_cancel())
        .unwrap();
    let before = store.content_digest().unwrap();
    let snapshot = capture(&store);
    assert_eq!(snapshot.contents().facts.source_schema, 2);
    let history = snapshot.contents().facts.imports.as_ref().unwrap();
    assert_eq!(
        (
            history.tasks.len(),
            history.sources.len(),
            history.batches.len()
        ),
        (1, 1, 1)
    );
    assert!(
        history.undo_tasks.is_empty()
            && history.undo_rows.is_empty()
            && history.undo_batches.is_empty()
    );
    assert_eq!(store.content_digest().unwrap(), before);
}

#[test]
fn exact_row_read_byte_and_encoding_limits_reject_without_partial_results() {
    let (_dir, _key, mut store) = import_database(3);
    pending_imports_and_undo(&mut store);
    let full = capture(&store);
    let rows = full.captured_rows();
    let bytes = full.captured_bytes();
    let before = store.content_digest().unwrap();
    for delta in [-1_isize, 0, 1] {
        let actual = store.logical_snapshot(
            Limits {
                rows: rows.checked_add_signed(delta).unwrap(),
                bytes: MAX_BYTES,
            },
            &months(),
            &no_cancel(),
        );
        if delta < 0 {
            assert_eq!(actual.err().unwrap().code, "snapshot_limit");
        } else {
            assert!(actual.is_ok());
        }
        let actual = store.logical_snapshot(
            Limits {
                rows: MAX_ROWS,
                bytes: bytes.checked_add_signed(delta).unwrap(),
            },
            &months(),
            &no_cancel(),
        );
        if delta < 0 {
            assert_eq!(actual.err().unwrap().code, "snapshot_limit");
        } else {
            assert!(actual.is_ok());
        }
    }
    for limits in [
        Limits { rows: 0, bytes: 1 },
        Limits {
            rows: MAX_ROWS + 1,
            bytes: MAX_BYTES,
        },
        Limits { rows: 1, bytes: 0 },
        Limits {
            rows: MAX_ROWS,
            bytes: MAX_BYTES + 1,
        },
    ] {
        assert_eq!(
            store
                .logical_snapshot(limits, &months(), &no_cancel())
                .err()
                .unwrap()
                .code,
            "snapshot_limit"
        );
    }
    let length = full.encode(MAX_BYTES, &no_cancel()).unwrap().len();
    for budget in [0, length - 1, MAX_BYTES + 1] {
        assert_eq!(
            full.encode(budget, &no_cancel()).unwrap_err().code,
            "snapshot_limit"
        );
    }
    assert_eq!(full.encode(length, &no_cancel()).unwrap().len(), length);
    assert_eq!(full.encode(length + 1, &no_cancel()).unwrap().len(), length);
    assert_eq!(
        full.encode(MAX_BYTES, &AtomicBool::new(true))
            .unwrap_err()
            .code,
        "cancelled"
    );
    assert_eq!(store.content_digest().unwrap(), before);
}

#[test]
fn cancellation_and_observer_failure_at_every_read_boundary_release_the_transaction() {
    let (_dir, _key, store, _aliases) = fixture("A11", "conflict");
    let before = store.content_digest().unwrap();
    for point in [
        CapturePoint::Header,
        CapturePoint::References,
        CapturePoint::Revisions,
        CapturePoint::Selections,
        CapturePoint::Intents,
        CapturePoint::Imports,
        CapturePoint::BeforeFinish,
    ] {
        let cancel = no_cancel();
        assert_eq!(
            store
                .logical_snapshot_observed(limits(), &months(), &cancel, |p| {
                    if p == point {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    Ok(())
                })
                .err()
                .unwrap()
                .code,
            "cancelled"
        );
        assert_eq!(
            store
                .logical_snapshot_observed(limits(), &months(), &no_cancel(), |p| if p == point {
                    Err(CoreError::new("injected_failure", "合成读取中断"))
                } else {
                    Ok(())
                })
                .err()
                .unwrap()
                .code,
            "injected_failure"
        );
        assert_eq!(store.content_digest().unwrap(), before);
        assert!(
            store
                .logical_snapshot(limits(), &months(), &no_cancel())
                .is_ok()
        );
    }
    assert_eq!(
        store
            .logical_snapshot(limits(), &vec!["2026-01".into(); 121], &no_cancel())
            .err()
            .unwrap()
            .code,
        "snapshot_limit"
    );
    assert_eq!(
        store
            .logical_snapshot(limits(), &["2026-13".into()], &no_cancel())
            .err()
            .unwrap()
            .code,
        "invalid_calendar_date"
    );
}

#[test]
fn logical_encoding_ignores_physical_layout_and_json_key_order() {
    let (_dir, _key, store, _aliases) = fixture("A11", "conflict");
    let before = capture(&store).encode(MAX_BYTES, &no_cancel()).unwrap();
    let rows = store
        .connection
        .prepare("SELECT operation_id,intent FROM outbox")
        .unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    for (id, text) in rows {
        let value: Value = serde_json::from_str(&text).unwrap();
        store
            .connection
            .execute(
                "UPDATE outbox SET intent=?1 WHERE operation_id=?2",
                params![serde_json::to_string_pretty(&value).unwrap(), id],
            )
            .unwrap();
    }
    store.connection.execute_batch("VACUUM;").unwrap();
    assert_eq!(
        capture(&store).encode(MAX_BYTES, &no_cancel()).unwrap(),
        before
    );
}

#[test]
fn malformed_graph_missing_reference_candidate_or_unknown_fact_never_yields_a_snapshot() {
    for corruption in 0..7 {
        let (_dir, _key, store, aliases) = fixture("A11", "conflict");
        let local = label_id(&aliases, "expense-v2-local");
        let remote = label_id(&aliases, "expense-v2-remote");
        match corruption {
            0 => {
                store
                    .connection
                    .execute(
                        "UPDATE revisions SET predecessor=?1 WHERE id=?2",
                        params![id(999), remote],
                    )
                    .unwrap();
            }
            1 => {
                store
                    .connection
                    .execute(
                        "UPDATE revisions SET predecessor=?1 WHERE id=?2",
                        params![&remote, &local],
                    )
                    .unwrap();
                store
                    .connection
                    .execute(
                        "UPDATE revisions SET predecessor=?1 WHERE id=?2",
                        params![&local, &remote],
                    )
                    .unwrap();
            }
            2 => {
                store
                    .connection
                    .execute(
                        "DELETE FROM accounts WHERE id=?1",
                        [label_id(&aliases, "cash")],
                    )
                    .unwrap();
            }
            3 => {
                store
                    .connection
                    .execute(
                        "UPDATE selected SET revision_id=?1 WHERE event_id=?2",
                        params![local, label_id(&aliases, "transfer")],
                    )
                    .unwrap();
            }
            4 => {
                store
                    .connection
                    .execute("DELETE FROM candidates WHERE revision_id=?1", [local])
                    .unwrap();
                store
                    .connection
                    .execute(
                        "INSERT INTO candidates VALUES (?1,?2)",
                        params![
                            label_id(&aliases, "expense-v1"),
                            label_id(&aliases, "expense")
                        ],
                    )
                    .unwrap();
            }
            5 | 6 => {
                let mut value =
                    serde_json::to_value(store.revision_event(&remote).unwrap().unwrap()).unwrap();
                if corruption == 5 {
                    value["amountMinor"] = json!("1e3");
                } else {
                    value["currency"] = json!("unknown");
                }
                store
                    .connection
                    .execute(
                        "UPDATE revisions SET payload=?1 WHERE id=?2",
                        params![value.to_string(), remote],
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before = store.content_digest().unwrap();
        assert_eq!(
            store
                .logical_snapshot(limits(), &months(), &no_cancel())
                .err()
                .unwrap()
                .code,
            "snapshot_corrupt"
        );
        assert_eq!(store.content_digest().unwrap(), before);
    }
}

#[test]
fn broken_import_source_progress_undo_or_unknown_intent_is_not_silently_omitted() {
    for sql in [
        "UPDATE csv_sources SET payload='{}'",
        "UPDATE csv_tasks SET next_record=1",
        "DELETE FROM csv_batches",
        "UPDATE csv_undo_rows SET outcome='pending' WHERE ordinal=1",
        "DELETE FROM csv_undo_batches",
        "UPDATE csv_undo_tasks SET request='[]'",
        "UPDATE outbox SET intent=json_set(intent,'$.unexpected',1)",
    ] {
        let (_dir, _key, mut store) = import_database(3);
        pending_imports_and_undo(&mut store);
        store.connection.execute_batch(sql).unwrap();
        let before = store.content_digest().unwrap();
        assert_eq!(
            store
                .logical_snapshot(limits(), &months(), &no_cancel())
                .err()
                .unwrap()
                .code,
            "snapshot_corrupt",
            "{sql}"
        );
        assert_eq!(store.content_digest().unwrap(), before);
    }
}

#[test]
fn unsupported_schema_or_undeclared_table_stops_without_rebuilding() {
    for sql in [
        "PRAGMA user_version=4",
        "CREATE TABLE undeclared_business (id TEXT)",
        "ALTER TABLE revisions ADD COLUMN undeclared_fact TEXT",
    ] {
        let (_dir, _key, store, _aliases) = fixture("A11", "conflict");
        let before = store.sequence().unwrap();
        store.connection.execute_batch(sql).unwrap();
        assert_eq!(
            store
                .logical_snapshot(limits(), &months(), &no_cancel())
                .err()
                .unwrap()
                .code,
            "unsupported_snapshot_schema"
        );
        assert_eq!(store.sequence().unwrap(), before);
        assert_eq!(store.outbox_count().unwrap(), 7);
    }
}

#[test]
fn all_common_accounting_cases_are_readable_with_their_original_expected_values() {
    let input: Value = serde_json::from_str(ACCOUNTING).unwrap();
    for case in input["cases"].as_array().unwrap() {
        let case_id = case["id"].as_str().unwrap();
        let step = case["steps"].as_array().unwrap().last().unwrap()["id"]
            .as_str()
            .unwrap();
        let (_dir, _key, store, aliases) = fixture(case_id, step);
        let requested = expected(case_id, step)["monthlyReports"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let snapshot = store
            .logical_snapshot(limits(), &requested, &no_cancel())
            .unwrap();
        assert_expected(snapshot.contents(), &aliases, case_id, step);
    }
}

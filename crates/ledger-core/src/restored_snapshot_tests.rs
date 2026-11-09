use super::*;
use materialized::CapturePoint;

fn select(host: &Host<'_>, candidate: &Candidate) -> Selection {
    let plan = host.confirm(target(candidate), &flag()).unwrap();
    host.publish(plan, &flag(), Faults::default(), |_| Ok(()))
        .unwrap()
}

fn snapshot(
    host: &Host<'_>,
    selected: &Selection,
) -> Result<crate::archive_container::restored::RestoredContents> {
    host.capture_restored(
        selected,
        &["2026-01".into()],
        CANDIDATE_LIMITS,
        &flag(),
        |_| Ok(()),
    )
}

#[test]
fn restored_snapshot_keeps_independent_observations_under_a_concurrent_sql_commit() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let mut cash = String::new();
    let fixture = Fixture::new(1, |store| {
        let aliases = crate::probe::populate_snapshot_fixture(store, "A11", "conflict").unwrap();
        assert_expected(capture(store).contents(), &aliases, "A11", "conflict");
        cash = aliases
            .iter()
            .find(|(_, alias)| *alias == "cash")
            .unwrap()
            .0
            .clone();
        let mut inherited = new_write(cash.clone());
        inherited.operation_id = id(711);
        inherited.event_id = id(712);
        inherited.revision_id = id(713);
        inherited.expected_sequence = Some(8);
        let event = inherited.event.as_mut().unwrap();
        event.note.clear();
        event.amount_minor = "5".into();
        store.commit(&inherited).unwrap();
    });
    let mut candidate = Candidate::new(&fixture.stage).unwrap();
    fixture.materialize(&mut candidate);
    let host = fixture.host(&candidate);
    let selected = select(&host, &candidate);
    assert_eq!(code(snapshot(&host, &fixture.old)), "selection_stale");
    let first = new_write(cash.clone());
    host.write(&selected, |store| store.commit(&first)).unwrap();
    let mut second = new_write(cash.clone());
    second.operation_id = id(721);
    second.event_id = id(722);
    second.revision_id = id(723);
    second.expected_sequence = Some(1);
    second.event.as_mut().unwrap().amount_minor = "11".into();

    // 单独 SQL 连接专门验证读事务，受控写入口仍须遵守共同锁。
    let mut other = materialized::open(
        &host.database(&selected.target),
        &candidate.key,
        &candidate.ticket,
        &candidate.device,
        false,
    )
    .unwrap();
    let before = host
        .capture_restored(
            &selected,
            &["2026-01".into()],
            CANDIDATE_LIMITS,
            &flag(),
            |point| {
                if point == CapturePoint::Established {
                    assert_eq!(
                        code(host.write(&selected, |store| store.commit(&second))),
                        "vault_busy"
                    );
                    other.commit(&second)?;
                    other.add_account(&id(725), "cash", "并发新增的合成账户")?;
                }
                Ok(())
            },
        )
        .unwrap();
    drop(other);
    assert_eq!(before.facts.history[0].view().source_schema, Some(1));
    assert_eq!(before.facts.history[0].view().sequence, 9);
    assert_eq!(
        before.facts.history[0].view().intents,
        fixture.snapshot.contents().facts.intent_history
    );
    assert!(
        before.facts.history[0]
            .view()
            .intents
            .iter()
            .any(|write| write.expected_sequence == Some(8))
    );
    assert_eq!(before.facts.history.last().unwrap().view().sequence, 1);
    assert_eq!(
        before.facts.history.last().unwrap().view().intents,
        std::slice::from_ref(&first)
    );
    assert_eq!(
        (
            before.facts.ledger.revisions.len(),
            before.facts.ledger.selected.len(),
            before.facts.ledger.candidates.len()
        ),
        (10, 6, 2)
    );
    assert!(
        !before
            .facts
            .ledger
            .accounts
            .iter()
            .any(|account| account.id == id(725))
    );
    assert_eq!(before.verification.projection.balances_minor[&cash], "8042");
    assert_eq!(before.verification.projection.net_assets_minor, "18042");
    assert_eq!(before.verification.deleted_event_ids.len(), 1);
    assert_eq!(
        before.verification.projection.monthly_reports["2026-01"].gross_expense_minor,
        "2000"
    );

    let after = snapshot(&host, &selected).unwrap();
    assert_eq!(after.facts.history.last().unwrap().view().sequence, 2);
    assert_eq!(
        after.facts.history.last().unwrap().view().intents,
        [first, second]
    );
    assert_eq!(after.verification.projection.balances_minor[&cash], "8053");
    assert_eq!(after.verification.projection.balances_minor[&id(725)], "0");
    assert!(
        after
            .facts
            .ledger
            .accounts
            .iter()
            .any(|account| account.id == id(725))
    );
    // 8 在继承段是有效观察点，在本地段则超出范围。
    restore_store::test_sql(
        &host.database(&selected.target),
        &candidate.key,
        "UPDATE outbox SET intent=json_set(intent,'$.expectedSequence',8)",
    )
    .unwrap();
    assert_eq!(code(snapshot(&host, &selected)), "materialization_mismatch");
}

#[test]
fn restored_snapshot_keeps_the_a04_delete_revision_after_a_local_restore() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let mut aliases = BTreeMap::new();
    let fixture = Fixture::new(1, |store| {
        aliases = crate::probe::populate_snapshot_fixture(store, "A04", "delete").unwrap();
    });
    let label = |name| aliases.iter().find(|(_, v)| *v == name).unwrap().0.clone();
    let mut candidate = Candidate::new(&fixture.stage).unwrap();
    fixture.materialize(&mut candidate);
    let host = fixture.host(&candidate);
    let selected = select(&host, &candidate);
    let delete_revision = label("transfer-v3");
    let event_id = label("transfer");
    let mut restore = new_write(label("cash"));
    restore.event_id = event_id.clone();
    restore.previous_revision_id = Some(delete_revision.clone());
    host.write(&selected, |store| {
        restore.event = store.revision_event(&label("transfer-v2"))?;
        store.commit(&restore)
    })
    .unwrap();
    let restored = snapshot(&host, &selected).unwrap();
    let tombstone = restored
        .facts
        .ledger
        .revisions
        .iter()
        .find(|r| r.id == delete_revision)
        .unwrap();
    assert!(tombstone.event.is_none());
    assert_eq!(
        restored.facts.history.last().unwrap().view().intents,
        std::slice::from_ref(&restore)
    );
    assert_eq!(
        restored.verification.selected_revision_ids[&event_id],
        restore.revision_id
    );
    assert!(!restored.verification.deleted_event_ids.contains(&event_id));
    assert_eq!(
        restored.verification.projection.balances_minor[&label("bank")],
        "7000"
    );
    assert_eq!(
        restored.verification.projection.balances_minor[&label("cash")],
        "3000"
    );
}

#[test]
fn restored_snapshot_checks_csv_history_and_reserved_ids_after_new_writes() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let bytes = format!(
        "{HEADER}{ONE}{}{}",
        ONE.replace("src-1", "src-2"),
        ONE.replace("src-1", "src-3")
    );
    for version in [2, 3] {
        let fixture = Fixture::new(version, |store| seed_csv(store, version, bytes.as_bytes()));
        let mut candidate = Candidate::new(&fixture.stage).unwrap();
        let ready = fixture.materialize(&mut candidate);
        let imports = ready.facts.imports.as_ref().unwrap();
        let pending = imports
            .rows
            .iter()
            .find(|r| r.record == 3)
            .unwrap()
            .intent
            .as_ref()
            .unwrap();
        let mut write = new_write(id(1));
        let host = fixture.host(&candidate);
        let selected = select(&host, &candidate);
        host.write(&selected, |store| {
            let mut collision = write.clone();
            collision.event_id.clone_from(&pending.event_id);
            assert_eq!(code(store.commit(&collision)), "archived_plan_identity");
            collision.event_id.clone_from(&write.event_id);
            collision.revision_id.clone_from(&pending.revision_id);
            assert_eq!(code(store.commit(&collision)), "archived_plan_identity");
            if let Some(undo) = imports.undo_rows.iter().find(|row| !row.deleted) {
                collision.revision_id.clone_from(&undo.intent.revision_id);
                assert_eq!(code(store.commit(&collision)), "archived_plan_identity");
            }
            let source = imports
                .sources
                .iter()
                .find(|source| source.source_id == "src-2")
                .unwrap();
            write.event_id.clone_from(&source.event_id);
            write.previous_revision_id = Some(source.revision_id.clone());
            write.event = Some(source.event.clone());
            write.event.as_mut().unwrap().note = "本机编辑，原来源不变".into();
            store.commit(&write)
        })
        .unwrap();
        let captured = snapshot(&host, &selected).unwrap();
        assert_eq!(
            captured.facts.history.last().unwrap().view().intents,
            [write]
        );
        assert_eq!(captured.facts.history.last().unwrap().view().sequence, 1);
        assert_eq!(
            serde_json::to_value(captured.facts.history[0].view().imports.unwrap()).unwrap(),
            serde_json::to_value(imports).unwrap(),
        );
        assert_eq!(
            captured.facts.history[0]
                .view()
                .imports
                .unwrap()
                .undo_tasks
                .len(),
            usize::from(version == 3)
        );
        let archive =
            crate::archive_container::restored::seal(captured, PASSWORD, CANDIDATE_LIMITS, &flag())
                .unwrap();
        let crate::archive_container::restored::Opened::Restored(opened) =
            crate::archive_container::restored::open(
                archive.as_bytes(),
                PASSWORD,
                CANDIDATE_LIMITS,
                &flag(),
            )
            .unwrap()
        else {
            panic!("expected 0.2 archive");
        };
        let [
            crate::archive_container::restored::History::Legacy {
                imports: Some(archived),
                ..
            },
            ..,
        ] = opened.contents.facts.history.as_slice()
        else {
            panic!("missing inherited CSV history");
        };
        assert_eq!(
            serde_json::to_value(archived).unwrap(),
            serde_json::to_value(imports).unwrap()
        );
        assert_eq!(
            opened.contents.facts.ledger.revisions.len(),
            ready.facts.revisions.len() + 1
        );
        let path = host.database(&selected.target);
        if version == 2 {
            restore_store::test_sql(&path, &candidate.key, "DELETE FROM archived_revisions")
                .unwrap();
        } else {
            restore_store::test_sql(&path, &candidate.key, "DELETE FROM csv_sources").unwrap();
        }
        assert_eq!(code(snapshot(&host, &selected)), "materialization_mismatch");
    }
}

#[test]
fn restored_snapshot_checks_limits_cancellation_and_history_without_reusing_the_ready_digest() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let fixture = Fixture::new(1, |store| {
        store.add_account(&id(1), "cash", "合成现金").unwrap();
    });
    let mut candidate = Candidate::new(&fixture.stage).unwrap();
    fixture.materialize(&mut candidate);
    let host = fixture.host(&candidate);
    let selected = select(&host, &candidate);
    host.write(&selected, |store| {
        store.add_account(&id(2), "cash", "新增账户")
    })
    .unwrap();
    let before = snapshot(&host, &selected).unwrap();
    assert_eq!(before.facts.history.last().unwrap().view().sequence, 0);
    assert_eq!(before.facts.ledger.accounts.len(), 2);
    for limits in [
        crate::archive_container::Limits {
            records: 1,
            ..CANDIDATE_LIMITS
        },
        crate::archive_container::Limits {
            payload_bytes: 80,
            ..CANDIDATE_LIMITS
        },
    ] {
        assert_eq!(
            code(host.capture_restored(&selected, &[], limits, &flag(), |_| Ok(()))),
            "snapshot_limit"
        );
    }
    let cancelled = flag();
    assert_eq!(
        code(
            host.capture_restored(&selected, &[], CANDIDATE_LIMITS, &cancelled, |point| {
                if point == CapturePoint::BeforeFinish {
                    cancelled.store(true, Ordering::Relaxed);
                }
                Ok(())
            })
        ),
        "cancelled",
    );
    assert_eq!(
        snapshot(&host, &selected)
            .unwrap()
            .facts
            .ledger
            .accounts
            .len(),
        2
    );
    let path = host.database(&selected.target);
    restore_store::test_sql(
        &path,
        &candidate.key,
        "ALTER TABLE accounts ADD COLUMN unknown TEXT GENERATED ALWAYS AS (name) VIRTUAL",
    )
    .unwrap();
    assert_eq!(
        code(snapshot(&host, &selected)),
        "unsupported_snapshot_schema"
    );
    restore_store::test_sql(
        &path,
        &candidate.key,
        "ALTER TABLE accounts DROP COLUMN unknown",
    )
    .unwrap();
    restore_store::test_sql(
        &path,
        &candidate.key,
        "UPDATE materialization SET history=replace(history,'\"sequence\":0','\"sequence\":1')",
    )
    .unwrap();
    assert_eq!(code(snapshot(&host, &selected)), "materialization_mismatch");
}

#[test]
fn restored_snapshot_rejects_unsettled_selection_and_preserves_the_legacy_layout() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let fixture = Fixture::new(1, |store| {
        store.add_account(&id(1), "cash", "合成现金").unwrap();
    });
    let mut candidate = Candidate::new(&fixture.stage).unwrap();
    fixture.materialize(&mut candidate);
    let host = fixture.host(&candidate);
    let target = target(&candidate);
    restore_store::test_sql(
        &host.database(&target),
        &candidate.key,
        "DROP TABLE archived_revisions; DROP TABLE archived_events;
            ALTER TABLE materialization DROP COLUMN history_digest;
            PRAGMA user_version=1002;",
    )
    .unwrap();
    let plan = host.confirm(target, &flag()).unwrap();
    let selected = plan.pending.new.clone();
    let fault = Faults {
        pending: None,
        selection: Some(FaultStage::DirectorySync),
    };
    assert_eq!(
        code(host.publish(plan, &flag(), fault, |_| Ok(()))),
        "selection_uncertain"
    );
    assert_eq!(code(snapshot(&host, &selected)), "selection_uncertain");
    assert_eq!(host.recover().unwrap(), selected);
    assert_eq!(
        code(snapshot(&host, &selected)),
        "unsupported_snapshot_schema"
    );
    let write = new_write(id(1));
    assert_eq!(
        host.write(&selected, |store| store.commit(&write))
            .unwrap()
            .1,
        WriteEffect::Committed
    );
    assert_eq!(
        host.read_store(&selected).unwrap().outbox_count().unwrap(),
        1
    );
}

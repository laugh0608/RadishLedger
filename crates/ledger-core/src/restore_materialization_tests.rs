use super::super::{
    Task,
    materialization::{Candidate, Point as MaterializePoint, StageInput},
};
use super::*;
use crate::archive_container::{SealedArchive, seal};
use crate::csv_import::{
    Decision, SourceMatch, prepare,
    test_support::{HEADER, ONE, Worker, id, mapping},
};
use crate::domain::{Event, EventKind};
use crate::logical_snapshot::{Contents, LogicalSnapshot};
use crate::store::{Write, WriteEffect, restore_store, snapshot_store::tests::assert_expected};
use std::collections::BTreeMap;
use std::io::{Read, Write as _};
use std::os::unix::{fs::PermissionsExt, process::ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const PASSWORD: &[u8] = b"public-materialization-test-password";

fn flag() -> AtomicBool {
    AtomicBool::new(false)
}

fn code<T>(result: Result<T>) -> &'static str {
    result
        .err()
        .expect("expected materialization rejection")
        .code
}

fn capture(store: &Store) -> LogicalSnapshot {
    store
        .logical_snapshot(SNAPSHOT_LIMITS, &["2026-01".into()], &flag())
        .unwrap()
}

fn target(candidate: &Candidate) -> Target {
    Target {
        slot: Slot::Candidate,
        vault: candidate.ticket.target_id().into(),
        device: candidate.device.clone(),
        materialization: Some(candidate.ticket.clone()),
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    old_key: DbKey,
    old: Selection,
    snapshot: LogicalSnapshot,
    archive: SealedArchive,
    stage: Task,
    stage_secret: Zeroizing<[u8; 32]>,
}

impl Fixture {
    fn new(version: u8, seed: impl FnOnce(&mut Store)) -> Self {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = directory.path();
        let old_key = DbKey::random().unwrap();
        let old = Selection::new(Target {
            slot: Slot::Original,
            vault: random_id().unwrap(),
            device: random_id().unwrap(),
            materialization: None,
        })
        .unwrap();
        let path = root.join("original.db");
        let mut store = match version {
            1 => Store::create(&path, &old_key),
            2 => Store::create_csv_experiment(&path, &old_key),
            3 => Store::create_csv_undo_experiment(&path, &old_key),
            _ => unreachable!(),
        }
        .unwrap();
        store
            .bind_bootstrap(&old.target.vault, &old.target.device)
            .unwrap();
        seed(&mut store);
        let snapshot = capture(&store);
        drop(store);
        let archive = seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &flag()).unwrap();
        let mut stage = Task::new(archive.as_bytes()).unwrap();
        let stage_secret = crate::crypto::random::<32>().unwrap();
        stage.key = DbKey::from_secret(stage_secret.as_ref()).unwrap();
        stage
            .prepare(
                root,
                archive.as_bytes(),
                PASSWORD,
                CANDIDATE_LIMITS,
                &flag(),
            )
            .unwrap();
        files::publish(&root.join(LOCK), b"", false).unwrap();
        files::publish(
            &root.join(ACTIVE),
            &serde_json::to_vec(&old).unwrap(),
            false,
        )
        .unwrap();
        Self {
            directory,
            old_key,
            old,
            snapshot,
            archive,
            stage,
            stage_secret,
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn input(&self) -> StageInput<'_> {
        StageInput {
            task: &self.stage,
            parent: self.root(),
            archive: self.archive.as_bytes(),
            password: PASSWORD,
        }
    }

    fn host<'a>(&'a self, candidate: &'a Candidate) -> Host<'a> {
        Host {
            root: self.root(),
            original_key: &self.old_key,
            candidate_key: Some(&candidate.key),
        }
    }

    fn materialize(&self, candidate: &mut Candidate) -> Contents {
        let ready = candidate
            .prepare(
                &self.input(),
                self.root(),
                CANDIDATE_LIMITS,
                &flag(),
                |_| Ok(()),
            )
            .unwrap();
        assert_eq!(
            materialized::digest(&ready, CANDIDATE_LIMITS, &flag()).unwrap(),
            materialized::digest(self.snapshot.contents(), CANDIDATE_LIMITS, &flag()).unwrap(),
        );
        ready
    }
}

fn new_write(account: String) -> Write {
    Write {
        operation_id: id(701),
        event_id: id(702),
        revision_id: id(703),
        previous_revision_id: None,
        event: Some(Event {
            kind: EventKind::Opening,
            business_date: "2026-01-01".into(),
            account_id: Some(account),
            category_id: None,
            from_account_id: None,
            to_account_id: None,
            amount_minor: "37".into(),
            note: "合成新写备注".into(),
        }),
        expected_sequence: Some(0),
        observed_balance_minor: None,
        target_balance_minor: None,
        resolve_candidates: Default::default(),
    }
}

#[test]
fn materialization_reopens_real_facts_and_preserves_the_later_original() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    for (case, step) in [("A11", "conflict"), ("A06", "renamed"), ("A04", "delete")] {
        let mut aliases = BTreeMap::new();
        let fixture = Fixture::new(1, |store| {
            aliases = crate::probe::populate_snapshot_fixture(store, case, step).unwrap();
        });
        let mut candidate = Candidate::new(&fixture.stage).unwrap();
        if case == "A04" {
            let label = |value| aliases.iter().find(|(_, v)| *v == value).unwrap().0.clone();
            let host = fixture.host(&candidate);
            host.write(&fixture.old, |store| {
                let prior = store.revision_event(&label("transfer-v2"))?.unwrap();
                store.commit(&Write {
                    operation_id: id(910),
                    event_id: label("transfer"),
                    revision_id: id(911),
                    previous_revision_id: Some(label("transfer-v3")),
                    event: Some(prior),
                    expected_sequence: None,
                    observed_balance_minor: None,
                    target_balance_minor: None,
                    resolve_candidates: Default::default(),
                })
            })
            .unwrap();
            let original = capture(&host.read_store(&fixture.old).unwrap());
            let mut later_aliases = aliases.clone();
            later_aliases.insert(id(911), "transfer-v4".into());
            assert_expected(original.contents(), &later_aliases, "A04", "restore");
        }
        let before = fixture
            .host(&candidate)
            .read_store(&fixture.old)
            .unwrap()
            .content_digest()
            .unwrap();
        let result = fixture.materialize(&mut candidate);
        assert_expected(&result, &aliases, case, step);
        let host = fixture.host(&candidate);
        let selection = Selection::new(target(&candidate)).unwrap();
        let path = host.database(&selection.target);
        let store = host.read_store(&selection).unwrap();
        assert_eq!(store.sequence().unwrap(), 0);
        assert_eq!(store.outbox_count().unwrap(), 0);
        assert_eq!(
            code(Store::open_readonly(&path, &candidate.key)),
            "unsupported_schema"
        );
        assert_eq!(
            code(Store::open_readonly(&path, &fixture.old_key)),
            "storage_error"
        );
        assert_eq!(
            code(Store::open_readonly(&path, &fixture.stage.key)),
            "storage_error"
        );
        assert_eq!(
            code(store.logical_snapshot(SNAPSHOT_LIMITS, &[], &flag())),
            "unsupported_snapshot_schema"
        );
        assert_eq!(
            host.read_store(&fixture.old)
                .unwrap()
                .content_digest()
                .unwrap(),
            before
        );
        assert_eq!(host.recover().unwrap(), fixture.old);
    }
}

#[test]
fn materialization_keeps_csv_dedupe_even_after_delete_and_history_cannot_run() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    for version in [2, 3] {
        let mapping = mapping();
        let text = format!(
            "{HEADER}{ONE}{}{}",
            ONE.replace("src-1", "src-2"),
            ONE.replace("src-1", "src-3")
        );
        let fixture = Fixture::new(version, |store| {
            store.add_account(&id(1), "cash", "合成现金").unwrap();
            store.add_category(&id(5), "expense", "合成餐饮").unwrap();
            let plan = prepare(store, text.as_bytes(), &mapping, &flag()).unwrap();
            let task = store
                .save_csv_task(&id(900), &plan, &[Decision::Import; 3], 2, &flag())
                .unwrap();
            store
                .publish_csv_batch(&task.target_id, &task.id, 1, &flag())
                .unwrap();
            store
                .set_csv_cancelled(&task.target_id, &task.id, true)
                .unwrap();
            if version == 3 {
                let undo = store
                    .preview_csv_undo(&task.target_id, &task.id, &flag())
                    .unwrap();
                store
                    .save_csv_undo(&id(800), &undo, &[1, 2], 1, &flag())
                    .unwrap();
                store
                    .publish_csv_undo_batch(&task.target_id, &id(800), 1, &flag())
                    .unwrap();
                store
                    .set_csv_undo_cancelled(&task.target_id, &id(800), true)
                    .unwrap();
            }
        });
        let mut candidate = Candidate::new(&fixture.stage).unwrap();
        let ready = fixture.materialize(&mut candidate);
        let history = ready.facts.imports.as_ref().unwrap();
        assert_eq!(history.tasks[0].next_record, 3);
        assert_eq!(history.undo_tasks.len(), usize::from(version == 3));
        let source = history
            .sources
            .iter()
            .find(|source| source.source_id == "src-1")
            .unwrap();
        assert_eq!(
            ready
                .verification
                .deleted_event_ids
                .contains(&source.event_id),
            version == 3
        );
        let host = fixture.host(&candidate);
        let plan = host.confirm(target(&candidate), &flag()).unwrap();
        let selected = host
            .publish(plan, &flag(), Faults::default(), |_| Ok(()))
            .unwrap();
        host.write(&selected, |store| {
            let matched = materialized::preview(
                store,
                format!("{HEADER}{ONE}").as_bytes(),
                &mapping,
                &flag(),
            )?;
            assert_eq!(
                matched.rows[0].source_match,
                SourceMatch::PreviouslyImported {
                    event_id: source.event_id.clone(),
                    revision_id: source.revision_id.clone(),
                }
            );
            let changed = materialized::preview(
                store,
                format!("{HEADER}{}", ONE.replace("1.00", "2.00")).as_bytes(),
                &mapping,
                &flag(),
            )?;
            assert!(
                changed.rows[0]
                    .errors
                    .iter()
                    .any(|e| e.code == "csv_source_conflict")
            );
            assert_eq!(
                code(store.set_csv_cancelled(
                    &history.source_target_id,
                    &history.tasks[0].id,
                    false
                )),
                "unsupported_schema"
            );
            let pending = history
                .rows
                .iter()
                .find(|row| row.record == 3)
                .unwrap()
                .intent
                .as_ref()
                .unwrap();
            assert_eq!(code(store.commit(pending)), "archived_operation");
            assert_eq!(store.outbox_count()?, 0);
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn materialized_candidate_enters_conditional_publication_and_accepts_a_new_write() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let fixture = Fixture::new(1, |store| {
        crate::probe::populate_bootstrap_fixture(store).unwrap();
    });
    let mut candidate = Candidate::new(&fixture.stage).unwrap();
    let ready = fixture.materialize(&mut candidate);
    let host = fixture.host(&candidate);
    let target = target(&candidate);
    let stale = host.confirm(target.clone(), &flag()).unwrap();
    host.write(&fixture.old, |store| {
        store.add_account(&id(905), "cash", "合成旧库新增账户")
    })
    .unwrap();
    let original = host
        .read_store(&fixture.old)
        .unwrap()
        .content_digest()
        .unwrap();
    assert_eq!(
        code(host.publish(stale, &flag(), Faults::default(), |_| Ok(()))),
        "selection_stale"
    );
    let late = host.write(&fixture.old, |_| Ok("旧响应")).unwrap();
    let plan = host.confirm(target, &flag()).unwrap();
    let selected = host
        .publish(plan, &flag(), Faults::default(), |_| Ok(()))
        .unwrap();
    assert_eq!(host.accept_result(late).unwrap(), None);
    let account = ready
        .facts
        .accounts
        .iter()
        .find(|r| !r.archived && r.kind == "cash")
        .unwrap()
        .id
        .clone();
    let write = new_write(account);
    assert_eq!(
        code(host.write(&fixture.old, |store| store.commit(&write))),
        "selection_stale"
    );
    assert_eq!(
        host.write(&selected, |store| store.commit(&write))
            .unwrap()
            .1,
        WriteEffect::Committed
    );
    assert_eq!(host.recover().unwrap(), selected);
    let reopened = host.read_store(&selected).unwrap();
    assert_eq!(reopened.sequence().unwrap(), 1);
    assert_eq!(reopened.outbox_count().unwrap(), 1);
    assert_eq!(
        reopened.revision_event(&write.revision_id).unwrap(),
        write.event
    );
    let path = host.database(&selected.target);
    assert_eq!(
        restore_store::test_count(&path, &candidate.key, "SELECT count(*) FROM revisions").unwrap(),
        9
    );
    assert_eq!(
        restore_store::test_count(
            &path,
            &candidate.key,
            "SELECT count(*) FROM archived_operations"
        )
        .unwrap(),
        7
    );
    assert_eq!(
        host.read_store(&fixture.old)
            .unwrap()
            .content_digest()
            .unwrap(),
        original
    );
}

#[test]
fn materialization_retries_same_identity_after_cancellation_and_rejects_mismatch() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let fixture = Fixture::new(1, |store| {
        crate::probe::populate_bootstrap_fixture(store).unwrap();
    });
    let mut candidate = Candidate::new(&fixture.stage).unwrap();
    let cancel = flag();
    let wrong = Task::new(fixture.archive.as_bytes()).unwrap();
    let wrong_input = StageInput {
        task: &wrong,
        ..fixture.input()
    };
    assert_eq!(
        code(candidate.prepare(
            &wrong_input,
            fixture.root(),
            CANDIDATE_LIMITS,
            &cancel,
            |_| Ok(())
        )),
        "materialization_mismatch"
    );
    let mut changed_archive = fixture.archive.as_bytes().to_vec();
    *changed_archive.last_mut().unwrap() ^= 1;
    assert_eq!(
        code(candidate.prepare(
            &StageInput {
                archive: &changed_archive,
                ..fixture.input()
            },
            fixture.root(),
            CANDIDATE_LIMITS,
            &cancel,
            |_| Ok(())
        )),
        "restore_mismatch"
    );
    assert!(!materialization::directory(fixture.root(), &candidate.ticket).exists());
    assert_eq!(
        code(candidate.prepare(
            &fixture.input(),
            fixture.root(),
            CANDIDATE_LIMITS,
            &cancel,
            |point| {
                if point == MaterializePoint::Registered {
                    assert_eq!(code(fixture.stage.abandon(fixture.root())), "restore_busy");
                }
                if point == MaterializePoint::BeforeCommit {
                    cancel.store(true, Ordering::Relaxed);
                }
                Ok(())
            }
        )),
        "cancelled"
    );
    let directory = materialization::directory(fixture.root(), &candidate.ticket);
    let database = directory.join("stage.db");
    assert_eq!(
        restore_store::test_count(&database, &candidate.key, "SELECT count(*) FROM revisions")
            .unwrap(),
        0
    );
    let mut wrong_key = Candidate::resume(
        candidate.ticket.clone(),
        candidate.device.clone(),
        DbKey::random().unwrap(),
    )
    .unwrap();
    assert_eq!(
        code(wrong_key.prepare(
            &fixture.input(),
            fixture.root(),
            CANDIDATE_LIMITS,
            &flag(),
            |_| Ok(())
        )),
        "storage_error"
    );
    let mut candidate =
        Candidate::resume(candidate.ticket, candidate.device, candidate.key).unwrap();
    fixture.materialize(&mut candidate);
    restore_store::test_sql(
        &database,
        &candidate.key,
        "CREATE TRIGGER unexpected BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT, 'unexpected'); END",
    )
    .unwrap();
    assert_eq!(
        code(candidate.prepare(
            &fixture.input(),
            fixture.root(),
            CANDIDATE_LIMITS,
            &flag(),
            |_| Ok(())
        )),
        "materialization_mismatch"
    );
    restore_store::test_sql(&database, &candidate.key, "DROP TRIGGER unexpected").unwrap();
    restore_store::test_sql(&database, &candidate.key, "DELETE FROM archived_operations").unwrap();
    assert_eq!(
        code(candidate.prepare(
            &fixture.input(),
            fixture.root(),
            CANDIDATE_LIMITS,
            &flag(),
            |_| Ok(())
        )),
        "materialization_mismatch"
    );
    fs::rename(&database, fixture.root().join("detached.db")).unwrap();
    assert_eq!(
        code(candidate.prepare(
            &fixture.input(),
            fixture.root(),
            CANDIDATE_LIMITS,
            &flag(),
            |_| Ok(())
        )),
        "restore_stage_missing"
    );
    assert!(!database.exists());
    assert_eq!(fixture.host(&candidate).recover().unwrap(), fixture.old);
}

#[test]
fn materialized_cleanup_is_guarded_by_selection_and_exact_ownership() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let fixture = Fixture::new(1, |store| {
        crate::probe::populate_bootstrap_fixture(store).unwrap();
    });
    let mut discardable = Candidate::new(&fixture.stage).unwrap();
    fixture.materialize(&mut discardable);
    let host = fixture.host(&discardable);
    let discard_target = target(&discardable);
    let directory = materialization::directory(fixture.root(), &discardable.ticket);
    let owner = directory.join("owner");
    fs::rename(&owner, fixture.root().join("saved-owner")).unwrap();
    assert_eq!(
        code(host.discard_materialized(&discard_target)),
        "restore_owner_missing"
    );
    assert!(directory.join("stage.db").exists());
    fs::rename(fixture.root().join("saved-owner"), &owner).unwrap();
    host.discard_materialized(&discard_target).unwrap();
    host.discard_materialized(&discard_target).unwrap();
    assert!(!directory.exists());

    let mut candidate = Candidate::new(&fixture.stage).unwrap();
    fixture.materialize(&mut candidate);
    let host = fixture.host(&candidate);
    let target = target(&candidate);
    let plan = host.confirm(target.clone(), &flag()).unwrap();
    let faults = Faults {
        pending: None,
        selection: Some(FaultStage::DirectorySync),
    };
    assert_eq!(
        code(host.publish(plan, &flag(), faults, |point| {
            if point == Point::BeforePending {
                assert_eq!(code(host.discard_materialized(&target)), "vault_busy");
            }
            Ok(())
        })),
        "selection_uncertain"
    );
    assert_eq!(
        code(host.discard_materialized(&target)),
        "selection_uncertain"
    );
    let unavailable = Host {
        candidate_key: None,
        ..fixture.host(&candidate)
    };
    assert_eq!(code(unavailable.recover()), "selection_key_unavailable");
    assert_eq!(unavailable.active().unwrap().target, target);
    assert!(unavailable.pending().unwrap().is_some());
    assert_eq!(host.recover().unwrap().target, target);
    assert_eq!(code(host.discard_materialized(&target)), "selection_in_use");
    assert!(host.database(&target).exists());
    assert!(fixture.root().join("original.db").exists());
    fixture
        .stage
        .inspect(
            fixture.root(),
            fixture.archive.as_bytes(),
            PASSWORD,
            CANDIDATE_LIMITS,
            &flag(),
        )
        .unwrap();
}

const CRASH_POINTS: [MaterializePoint; 4] = [
    MaterializePoint::Registered,
    MaterializePoint::BeforeCommit,
    MaterializePoint::Written,
    MaterializePoint::Reopened,
];

#[test]
#[ignore = "只由父测试供给本批合成钥与专属目录"]
fn materialization_worker() {
    let root = PathBuf::from(std::env::var_os("RLM0_MATERIALIZATION_ROOT").unwrap());
    let point: usize = std::env::var("RLM0_MATERIALIZATION_POINT")
        .unwrap()
        .parse()
        .unwrap();
    let mut keys = Zeroizing::new([0_u8; 64]);
    std::io::stdin().read_exact(keys.as_mut()).unwrap();
    let stage: Ticket =
        serde_json::from_slice(&files::read(&root.join("stage-ticket.json"), 4096).unwrap())
            .unwrap();
    let target: Target =
        serde_json::from_slice(&files::read(&root.join("materialized-target.json"), 4096).unwrap())
            .unwrap();
    let archive = files::read(
        &root.join("synthetic.archive"),
        crate::archive_container::MAX_PAYLOAD + 328,
    )
    .unwrap();
    let stage = Task::resume(stage, DbKey::from_secret(&keys[..32]).unwrap()).unwrap();
    let mut candidate = Candidate::resume(
        target.materialization.unwrap(),
        target.device,
        DbKey::from_secret(&keys[32..]).unwrap(),
    )
    .unwrap();
    let input = StageInput {
        task: &stage,
        parent: &root,
        archive: &archive,
        password: PASSWORD,
    };
    candidate
        .prepare(&input, &root, CANDIDATE_LIMITS, &flag(), |observed| {
            if observed == CRASH_POINTS[point] {
                files::publish(&root.join("materialization-reached"), b"ready", false).unwrap();
                loop {
                    std::thread::park_timeout(Duration::from_secs(1));
                }
            }
            Ok(())
        })
        .unwrap();
    panic!("materialization worker missed its crash boundary");
}

#[test]
fn materialization_survives_process_kill_without_partial_rows_or_new_identity() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let fixture = Fixture::new(1, |store| {
        crate::probe::populate_bootstrap_fixture(store).unwrap();
    });
    let root = fixture.root();
    // 测试归档沿正式容器生成，独占目录内写入完整密文。
    fs::write(root.join("synthetic.archive"), fixture.archive.as_bytes()).unwrap();
    files::publish(
        &root.join("stage-ticket.json"),
        &serde_json::to_vec(fixture.stage.ticket()).unwrap(),
        false,
    )
    .unwrap();
    for (index, point) in CRASH_POINTS.into_iter().enumerate() {
        let mut candidate = Candidate::new(&fixture.stage).unwrap();
        let secret = crate::crypto::random::<32>().unwrap();
        candidate.key = DbKey::from_secret(secret.as_ref()).unwrap();
        let ticket = candidate.ticket.clone();
        drop(restore_files::Files::acquire(root, &ticket, true).unwrap());
        files::publish(
            &root.join("materialized-target.json"),
            &serde_json::to_vec(&target(&candidate)).unwrap(),
            true,
        )
        .unwrap();
        let mut worker = Worker(Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "restore_stage::restore_selection::restore_materialization_tests::materialization_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("RLM0_MATERIALIZATION_ROOT", root).env("RLM0_MATERIALIZATION_POINT", index.to_string())
            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
        let mut pipe = worker.0.stdin.take().unwrap();
        pipe.write_all(fixture.stage_secret.as_ref()).unwrap();
        pipe.write_all(secret.as_ref()).unwrap();
        drop(pipe);
        let reached = root.join("materialization-reached");
        let deadline = Instant::now() + Duration::from_secs(20);
        while !reached.exists() {
            assert!(
                worker.0.try_wait().unwrap().is_none(),
                "materialization worker exited early"
            );
            assert!(Instant::now() < deadline, "materialization worker deadline");
            std::thread::sleep(Duration::from_millis(10));
        }
        worker.0.kill().unwrap();
        assert_eq!(worker.0.wait().unwrap().signal(), Some(9));
        fs::remove_file(reached).unwrap();
        let path = materialization::directory(root, &ticket).join("stage.db");
        let visible =
            restore_store::test_count(&path, &candidate.key, "SELECT count(*) FROM revisions")
                .unwrap();
        assert_eq!(
            visible,
            if matches!(
                point,
                MaterializePoint::Written | MaterializePoint::Reopened
            ) {
                8
            } else {
                0
            }
        );
        let mut resumed =
            Candidate::resume(candidate.ticket, candidate.device, candidate.key).unwrap();
        fixture.materialize(&mut resumed);
        assert_eq!(resumed.ticket, ticket);
        assert_eq!(fixture.host(&resumed).recover().unwrap(), fixture.old);
        fixture
            .host(&resumed)
            .discard_materialized(&target(&resumed))
            .unwrap();
    }
}

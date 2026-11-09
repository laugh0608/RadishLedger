use super::*;
use crate::csv_import::{
    Decision, prepare,
    test_support::{HEADER, ONE, Worker, id, mapping},
};
use crate::domain::{Event, EventKind};
use crate::store::{Write, WriteEffect};
use std::io::{Read, Write as _};
use std::os::unix::{
    fs::{OpenOptionsExt, PermissionsExt},
    process::ExitStatusExt,
};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn flag() -> AtomicBool {
    AtomicBool::new(false)
}

fn code<T>(result: Result<T>) -> &'static str {
    result.err().expect("expected selection rejection").code
}

struct Fixture {
    directory: tempfile::TempDir,
    secrets: Zeroizing<[u8; 64]>,
    original_key: DbKey,
    candidate_key: DbKey,
    old: Selection,
    candidate: Target,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let secrets = crate::crypto::random::<64>().unwrap();
        let original_key = DbKey::from_secret(&secrets[..32]).unwrap();
        let candidate_key = DbKey::from_secret(&secrets[32..]).unwrap();
        let target = |slot| Target {
            slot,
            vault: random_id().unwrap(),
            device: random_id().unwrap(),
        };
        let old = Selection::new(target(Slot::Original)).unwrap();
        let candidate = target(Slot::Candidate);
        let root = directory.path();
        let mut original =
            Store::create_csv_experiment(&root.join(Slot::Original.database()), &original_key)
                .unwrap();
        original
            .bind_bootstrap(&old.target.vault, &old.target.device)
            .unwrap();
        crate::probe::populate_bootstrap_fixture(&mut original).unwrap();
        let mut next =
            Store::create(&root.join(Slot::Candidate.database()), &candidate_key).unwrap();
        next.bind_bootstrap(&candidate.vault, &candidate.device)
            .unwrap();
        crate::probe::populate_bootstrap_fixture(&mut next).unwrap();
        let snapshot = next
            .logical_snapshot(SNAPSHOT_LIMITS, &[], &flag())
            .unwrap();
        let facts = &snapshot.contents().facts;
        assert_eq!(
            (
                facts.revisions.len(),
                facts.candidates.len(),
                facts.intent_history.len()
            ),
            (8, 2, 7)
        );
        assert_eq!(snapshot.contents().verification.deleted_event_ids.len(), 1);
        next.save_fixture_digest(&crate::crypto::hex(&checkpoint(&next, &flag()).unwrap()))
            .unwrap();
        drop(original);
        drop(next);
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join(LOCK))
            .unwrap()
            .sync_all()
            .unwrap();
        let fixture = Self {
            directory,
            secrets,
            original_key,
            candidate_key,
            old,
            candidate,
        };
        fixture
            .host()
            .persist(ACTIVE, &fixture.old, false, None)
            .unwrap();
        fixture
            .host()
            .persist("fixture-target.json", &fixture.candidate, false, None)
            .unwrap();
        fixture
    }

    fn host(&self) -> Host<'_> {
        Host {
            root: self.directory.path(),
            original_key: &self.original_key,
            candidate_key: Some(&self.candidate_key),
        }
    }

    fn confirm(&self) -> Confirmed {
        self.host()
            .confirm(self.candidate.clone(), &flag())
            .unwrap()
    }

    fn books(&self) -> (String, String) {
        let host = self.host();
        let candidate = Selection {
            target: self.candidate.clone(),
            ..self.old.clone()
        };
        (
            host.read_store(&self.old)
                .unwrap()
                .content_digest()
                .unwrap(),
            host.read_store(&candidate)
                .unwrap()
                .content_digest()
                .unwrap(),
        )
    }
}

fn add_account(store: &mut Store) -> Result<()> {
    store.add_account(&id(712), "cash", "合成晚到账户")
}

fn add_transaction(store: &mut Store) -> Result<()> {
    store.add_account(&id(701), "cash", "合成未同步账户")?;
    let effect = store.commit(&Write {
        operation_id: id(702),
        event_id: id(703),
        revision_id: id(704),
        previous_revision_id: None,
        event: Some(Event {
            kind: EventKind::Opening,
            business_date: "2026-01-01".into(),
            account_id: Some(id(701)),
            category_id: None,
            from_account_id: None,
            to_account_id: None,
            amount_minor: "37".into(),
            note: String::new(),
        }),
        expected_sequence: None,
        observed_balance_minor: None,
        target_balance_minor: None,
        resolve_candidates: Default::default(),
    })?;
    assert_eq!(effect, WriteEffect::Committed);
    Ok(())
}

#[test]
fn old_checkpoint_rejects_transactions_accounts_and_csv_progress() {
    for change in 0..3 {
        let fixture = Fixture::new();
        let host = fixture.host();
        let csv_task = if change == 2 {
            Some(
                host.write(&fixture.old, |store| {
                    store.add_account(&id(1), "cash", "合成现金")?;
                    store.add_category(&id(5), "expense", "合成餐饮")?;
                    let prepared = prepare(
                        store,
                        format!("{HEADER}{ONE}").as_bytes(),
                        &mapping(),
                        &flag(),
                    )?;
                    store.save_csv_task(&id(900), &prepared, &[Decision::Import], 1, &flag())
                })
                .unwrap()
                .1,
            )
        } else {
            None
        };
        let plan = fixture.confirm();
        let sequence = host.read_store(&fixture.old).unwrap().sequence().unwrap();
        let before = fixture.books();
        host.write(&fixture.old, |store| match change {
            0 => add_transaction(store),
            1 => add_account(store),
            _ => {
                let task = csv_task.as_ref().unwrap();
                store.set_csv_cancelled(&task.target_id, &task.id, true)?;
                Ok(())
            }
        })
        .unwrap();
        let after = fixture.books();
        assert_ne!(before.0, after.0);
        assert_eq!(before.1, after.1);
        assert_eq!(
            host.read_store(&fixture.old).unwrap().sequence().unwrap(),
            sequence + i64::from(change == 0)
        );
        assert_eq!(
            code(host.publish(plan, &flag(), Faults::default(), |_| Ok(()))),
            "selection_stale"
        );
        assert!(host.pending().unwrap().is_none());
        assert_eq!(host.recover().unwrap(), fixture.old);
        assert_eq!(fixture.books(), after);
    }
}

#[test]
fn cancellation_before_replacement_and_after_replacement_reports_actual_choice() {
    for (point, applied) in [
        (Point::BeforePending, false),
        (Point::Pending, false),
        (Point::Selected, true),
    ] {
        let fixture = Fixture::new();
        let host = fixture.host();
        let plan = fixture.confirm();
        let new = plan.pending.new.clone();
        let cancel = flag();
        let before = fixture.books();
        let result = host.publish(plan, &cancel, Faults::default(), |at| {
            if at == point {
                cancel.store(true, Ordering::Relaxed);
            }
            Ok(())
        });
        if applied {
            assert_eq!(result.unwrap(), new);
        } else {
            assert_eq!(code(result), "cancelled");
        }
        assert_eq!(
            host.recover().unwrap(),
            if applied { new } else { fixture.old.clone() }
        );
        assert!(host.pending().unwrap().is_none());
        assert_eq!(fixture.books(), before);
    }
}

#[test]
fn file_publication_faults_resolve_exact_old_or_new_selection() {
    for pending_file in [true, false] {
        for stage in [
            FaultStage::WriteZero,
            FaultStage::WriteError,
            FaultStage::FileSync,
            FaultStage::Publish,
            FaultStage::DirectorySync,
        ] {
            let fixture = Fixture::new();
            let host = fixture.host();
            let before = fixture.books();
            let plan = fixture.confirm();
            let new = plan.pending.new.clone();
            let dir_sync_failed = stage == FaultStage::DirectorySync;
            let pending_visible = !pending_file || dir_sync_failed;
            let new_visible = !pending_file && dir_sync_failed;
            let faults = if pending_file {
                Faults {
                    pending: Some(stage),
                    selection: None,
                }
            } else {
                Faults {
                    pending: None,
                    selection: Some(stage),
                }
            };
            assert_eq!(
                code(host.publish(plan, &flag(), faults, |_| Ok(()))),
                if pending_visible {
                    "selection_uncertain"
                } else {
                    "bootstrap_io"
                }
            );
            assert_eq!(host.pending().unwrap().is_some(), pending_visible);
            if pending_visible {
                assert_eq!(
                    code(host.write::<()>(&fixture.old, |_| panic!("pending write"))),
                    "selection_uncertain"
                );
            }
            assert_eq!(
                host.recover().unwrap(),
                if new_visible {
                    new
                } else {
                    fixture.old.clone()
                }
            );
            assert!(host.pending().unwrap().is_none());
            assert_eq!(fixture.books(), before);
        }
    }
}

#[test]
fn missing_key_or_changed_binding_preserves_both_libraries() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let unavailable = Host {
        candidate_key: None,
        ..fixture.host()
    };
    let before = fixture.books();
    assert_eq!(
        code(Store::open_readonly(
            &host.database(&fixture.candidate),
            &fixture.original_key
        )),
        "storage_error"
    );
    let mut wrong_binding = fixture.candidate.clone();
    wrong_binding.vault = random_id().unwrap();
    assert_eq!(
        code(host.confirm(wrong_binding, &flag())),
        "identity_mismatch"
    );
    let plan = fixture.confirm();
    assert_eq!(
        code(unavailable.publish(plan, &flag(), Faults::default(), |_| Ok(()))),
        "selection_key_unavailable"
    );
    assert_eq!(host.recover().unwrap(), fixture.old);
    let plan = fixture.confirm();
    let new = plan.pending.new.clone();
    let faults = Faults {
        pending: None,
        selection: Some(FaultStage::DirectorySync),
    };
    assert_eq!(
        code(host.publish(plan, &flag(), faults, |_| Ok(()))),
        "selection_uncertain"
    );
    assert_eq!(code(unavailable.recover()), "selection_key_unavailable");
    assert_eq!(host.active().unwrap(), new);
    assert!(host.pending().unwrap().is_some());
    assert_eq!(fixture.books(), before);
    assert_eq!(host.recover().unwrap(), new);
}

#[test]
fn changed_candidate_cannot_pass_the_preparation_check_or_recovery() {
    for selected in [false, true] {
        let fixture = Fixture::new();
        let host = fixture.host();
        let before = fixture.books();
        let plan = fixture.confirm();
        let new = plan.pending.new.clone();
        let mut plan = Some(plan);
        if selected {
            let faults = Faults {
                pending: None,
                selection: Some(FaultStage::DirectorySync),
            };
            assert_eq!(
                code(host.publish(plan.take().unwrap(), &flag(), faults, |_| Ok(()))),
                "selection_uncertain"
            );
        }
        // 模拟候选文件在核对后损坏，绕过实验的正常写入入口。
        let mut damaged =
            Store::open(&host.database(&fixture.candidate), &fixture.candidate_key).unwrap();
        add_account(&mut damaged).unwrap();
        drop(damaged);
        let after = fixture.books();
        assert_eq!(before.0, after.0);
        assert_ne!(before.1, after.1);
        if let Some(plan) = plan {
            assert_eq!(
                code(host.publish(plan, &flag(), Faults::default(), |_| Ok(()))),
                "selection_mismatch"
            );
            assert_eq!(host.recover().unwrap(), fixture.old);
            assert!(host.pending().unwrap().is_none());
        } else {
            assert_eq!(code(host.recover()), "selection_mismatch");
            assert_eq!(host.active().unwrap(), new);
            assert!(host.pending().unwrap().is_some());
        }
        assert_eq!(fixture.books(), after);
    }
}

#[test]
fn failure_during_finalization_is_rechecked_without_rolling_back_selection() {
    for stop in [Point::Reopened, Point::PendingRemoved] {
        let fixture = Fixture::new();
        let host = fixture.host();
        let before = fixture.books();
        let plan = fixture.confirm();
        let new = plan.pending.new.clone();
        let result = host.publish(plan, &flag(), Faults::default(), |point| {
            if point == stop {
                return Err(CoreError::new("injected_failure", "合成终结失败"));
            }
            Ok(())
        });
        assert_eq!(code(result), "selection_uncertain");
        assert_eq!(host.pending().unwrap().is_some(), stop == Point::Reopened);
        assert_eq!(host.recover().unwrap(), new);
        assert_eq!(fixture.books(), before);
    }
}

#[test]
fn absent_corrupt_or_unrelated_selection_blocks_recovery_and_writes() {
    for damage in 0..3 {
        let fixture = Fixture::new();
        let host = fixture.host();
        let before = fixture.books();
        let plan = fixture.confirm();
        let faults = Faults {
            pending: None,
            selection: Some(FaultStage::DirectorySync),
        };
        assert_eq!(
            code(host.publish(plan, &flag(), faults, |_| Ok(()))),
            "selection_uncertain"
        );
        let path = host.root.join(ACTIVE);
        match damage {
            0 => fs::remove_file(&path).unwrap(),
            1 => fs::write(&path, b"corrupt control").unwrap(),
            _ => host
                .persist(
                    ACTIVE,
                    &Selection::new(fixture.old.target.clone()).unwrap(),
                    true,
                    None,
                )
                .unwrap(),
        }
        assert_eq!(
            code(host.recover()),
            if damage == 0 {
                "selection_io"
            } else {
                "selection_mismatch"
            }
        );
        assert_eq!(
            code(host.write::<()>(&fixture.old, |_| panic!("unresolved write"))),
            "selection_uncertain"
        );
        assert!(host.pending().unwrap().is_some());
        assert_eq!(fixture.books(), before);
    }
}

const CRASH_POINTS: [Point; 6] = [
    Point::BeforePending,
    Point::Pending,
    Point::Selected,
    Point::Reopened,
    Point::PendingRemoved,
    Point::Settled,
];

#[derive(Serialize, Deserialize)]
enum WorkerAction {
    HoldWrite,
    KillPublish(usize),
}

fn reached(root: &Path) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join("reached"))
        .unwrap();
    file.write_all(b"ready").unwrap();
    file.sync_all().unwrap();
}

#[test]
#[ignore = "由活动选择父测试提供合成密钥和专属 tempfile"]
fn selection_worker() {
    let root = PathBuf::from(std::env::var_os("RLM0_SELECTION_ROOT").unwrap());
    let action: WorkerAction =
        serde_json::from_str(&std::env::var("RLM0_SELECTION_ACTION").unwrap()).unwrap();
    let mut input = std::io::stdin().lock();
    let mut secrets = Zeroizing::new([0_u8; 64]);
    input.read_exact(secrets.as_mut()).unwrap();
    let original_key = DbKey::from_secret(&secrets[..32]).unwrap();
    let candidate_key = DbKey::from_secret(&secrets[32..]).unwrap();
    let host = Host {
        root: &root,
        original_key: &original_key,
        candidate_key: Some(&candidate_key),
    };
    match action {
        WorkerAction::HoldWrite => {
            let origin = host.recover().unwrap();
            host.write(&origin, |store| {
                reached(&root);
                input.read_exact(&mut [0_u8; 1]).unwrap();
                add_account(store)
            })
            .unwrap();
        }
        WorkerAction::KillPublish(index) => {
            let candidate = host.read("fixture-target.json").unwrap();
            let plan = host.confirm(candidate, &flag()).unwrap();
            host.persist("attempt.json", &plan.pending.new, false, None)
                .unwrap();
            host.publish(plan, &flag(), Faults::default(), |at| {
                if at == CRASH_POINTS[index] {
                    reached(&root);
                    loop {
                        std::thread::park_timeout(Duration::from_secs(1));
                    }
                }
                Ok(())
            })
            .unwrap();
            panic!("selection worker missed its crash boundary");
        }
    }
}

fn spawn(fixture: &Fixture, action: WorkerAction) -> Worker {
    let mut worker = Worker(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "restore_selection::tests::selection_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("RLM0_SELECTION_ROOT", fixture.directory.path())
            .env(
                "RLM0_SELECTION_ACTION",
                serde_json::to_string(&action).unwrap(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    worker
        .0
        .stdin
        .as_mut()
        .unwrap()
        .write_all(fixture.secrets.as_slice())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !fixture.directory.path().join("reached").exists() {
        assert!(
            worker.0.try_wait().unwrap().is_none(),
            "selection worker exited early"
        );
        assert!(Instant::now() < deadline, "selection worker deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
    worker
}

#[test]
fn killed_publication_recovers_exact_selection_and_does_not_replay() {
    for (index, point) in CRASH_POINTS.into_iter().enumerate() {
        let fixture = Fixture::new();
        let host = fixture.host();
        let before = fixture.books();
        let mut worker = spawn(&fixture, WorkerAction::KillPublish(index));
        worker.0.kill().unwrap();
        assert_eq!(worker.0.wait().unwrap().signal(), Some(9));
        let applied = !matches!(point, Point::BeforePending | Point::Pending);
        let new = host.read::<Selection>("attempt.json").unwrap();
        let expected = if applied { &new } else { &fixture.old };
        assert_eq!(
            host.pending().unwrap().is_some(),
            matches!(point, Point::Pending | Point::Selected | Point::Reopened)
        );
        assert_eq!(&host.recover().unwrap(), expected);
        assert_eq!(&host.recover().unwrap(), expected);
        assert!(host.pending().unwrap().is_none());
        assert_eq!(fixture.books(), before);
        if applied {
            host.write(&new, add_account).unwrap();
            let after = fixture.books();
            assert_eq!(after.0, before.0);
            assert_ne!(after.1, before.1);
            assert_eq!(host.recover().unwrap(), new);
            assert_eq!(fixture.books(), after);
        }
    }
}

#[test]
fn cross_process_writer_and_switch_share_gate_and_reject_late_old_work() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let before = fixture.books();
    let busy_plan = fixture.confirm();
    let stale_plan = fixture.confirm();
    let mut worker = spawn(&fixture, WorkerAction::HoldWrite);
    assert_eq!(
        code(host.publish(busy_plan, &flag(), Faults::default(), |_| Ok(()))),
        "vault_busy"
    );
    worker.0.stdin.take().unwrap().write_all(b"1").unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = worker.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "selection writer deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        code(host.publish(stale_plan, &flag(), Faults::default(), |_| Ok(()))),
        "selection_stale"
    );
    let old_success = host.write(&fixture.old, |_| Ok("合成旧响应")).unwrap();
    let plan = fixture.confirm();
    let concurrent = fixture.confirm();
    let selected = host
        .publish(plan, &flag(), Faults::default(), |_| Ok(()))
        .unwrap();
    assert_eq!(
        code(host.publish(concurrent, &flag(), Faults::default(), |_| Ok(()))),
        "selection_stale"
    );
    assert_eq!(
        code(host.write::<()>(&fixture.old, |_| panic!("late old write"))),
        "selection_stale"
    );
    assert_eq!(host.accept_result(old_success).unwrap(), None);
    let current_success = host.write(&selected, |_| Ok("合成新响应")).unwrap();
    assert_eq!(
        host.accept_result(current_success).unwrap(),
        Some("合成新响应")
    );
    let after = fixture.books();
    assert_ne!(before.0, after.0);
    assert_eq!(before.1, after.1);
}

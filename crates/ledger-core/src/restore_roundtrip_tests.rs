use super::*;
use crate::archive_container::restored::{self, RestoredContents};

struct BlankHost {
    directory: tempfile::TempDir,
    key: DbKey,
    original: Selection,
}

impl BlankHost {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let key = DbKey::random().unwrap();
        let original = Selection::new(Target {
            slot: Slot::Original,
            vault: random_id().unwrap(),
            device: random_id().unwrap(),
            materialization: None,
        })
        .unwrap();
        let store = Store::create(&directory.path().join("original.db"), &key).unwrap();
        store
            .bind_bootstrap(&original.target.vault, &original.target.device)
            .unwrap();
        files::publish(&directory.path().join(LOCK), b"", false).unwrap();
        files::publish(
            &directory.path().join(ACTIVE),
            &serde_json::to_vec(&original).unwrap(),
            false,
        )
        .unwrap();
        Self {
            directory,
            key,
            original,
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn host<'a>(&'a self, candidate: &'a Candidate) -> Host<'a> {
        Host {
            root: self.root(),
            original_key: &self.key,
            candidate_key: Some(&candidate.key),
        }
    }

    fn restore(&self, archive: &SealedArchive) -> (Candidate, Selection, RestoredContents) {
        let bytes = archive.as_bytes();
        let stage = Task::new(bytes).unwrap();
        stage
            .prepare_restored(
                self.root(),
                bytes,
                PASSWORD,
                CANDIDATE_LIMITS,
                &flag(),
                |_| Ok(()),
            )
            .unwrap();
        let mut candidate = Candidate::new(&stage).unwrap();
        let input = StageInput {
            task: &stage,
            parent: self.root(),
            archive: bytes,
            password: PASSWORD,
        };
        let ready = candidate
            .prepare_restored(&input, self.root(), CANDIDATE_LIMITS, &flag(), |_| Ok(()))
            .unwrap();
        let host = self.host(&candidate);
        let path = host.database(&target(&candidate));
        assert!(Store::open_readonly(&path, &self.key).is_err());
        assert!(Store::open_readonly(&path, &stage.key).is_err());
        assert_eq!(
            code(Store::open_readonly(&path, &candidate.key)),
            "unsupported_schema"
        );
        stage.abandon_restored(self.root()).unwrap();
        assert_eq!(host.recover().unwrap(), self.original);
        let selected = select(&host, &candidate);
        let store = host.read_store(&selected).unwrap();
        assert_eq!(store.sequence().unwrap(), 0);
        assert_eq!(store.outbox_count().unwrap(), 0);
        (candidate, selected, ready)
    }
}

fn select(host: &Host<'_>, candidate: &Candidate) -> Selection {
    let plan = host.confirm(target(candidate), &flag()).unwrap();
    host.publish(plan, &flag(), Faults::default(), |_| Ok(()))
        .unwrap()
}

fn reseal(host: &Host<'_>, selected: &Selection) -> SealedArchive {
    let contents = host
        .capture_restored(
            selected,
            &["2026-01".into()],
            CANDIDATE_LIMITS,
            &flag(),
            |_| Ok(()),
        )
        .unwrap();
    restored::seal(contents, PASSWORD, CANDIDATE_LIMITS, &flag()).unwrap()
}

#[test]
fn restore_roundtrip_preserves_history_and_csv_across_independent_generations() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let first = new_write(id(1));
    let archive = {
        let bytes = format!(
            "{HEADER}{ONE}{}{}",
            ONE.replace("src-1", "src-2"),
            ONE.replace("src-1", "src-3")
        );
        let fixture = Fixture::new(3, |store| seed_csv(store, 3, bytes.as_bytes()));
        let mut candidate = Candidate::new(&fixture.stage).unwrap();
        fixture.materialize(&mut candidate);
        let host = fixture.host(&candidate);
        let selected = select(&host, &candidate);
        host.write(&selected, |store| store.commit(&first)).unwrap();
        reseal(&host, &selected)
    };

    let (next_archive, prior_id) = {
        let blank = BlankHost::new();
        let (candidate, selected, ready) = blank.restore(&archive);
        assert_eq!(ready.verification.projection.balances_minor[&id(1)], "-63");
        assert_eq!(ready.facts.history.len(), 2);
        assert_eq!(
            ready.facts.history[1].view().intents,
            std::slice::from_ref(&first)
        );
        let imports = ready.facts.history[0].view().imports.unwrap();
        assert_eq!(imports.batches.len(), 1);
        assert_eq!(imports.undo_batches.len(), 1);
        let source = imports
            .sources
            .iter()
            .find(|source| source.source_id == "src-1")
            .unwrap();
        assert!(
            ready
                .verification
                .deleted_event_ids
                .contains(&source.event_id)
        );
        let pending = imports
            .rows
            .iter()
            .find(|row| row.record == 3)
            .unwrap()
            .intent
            .as_ref()
            .unwrap();
        let mut second = new_write(id(1));
        second.operation_id = id(731);
        second.event_id = id(732);
        second.revision_id = id(733);
        second.event.as_mut().unwrap().amount_minor = "11".into();
        let host = blank.host(&candidate);
        host.write(&selected, |store| {
            assert_eq!(code(store.commit(&first)), "archived_operation");
            let mut collision = second.clone();
            collision.event_id.clone_from(&pending.event_id);
            assert_eq!(code(store.commit(&collision)), "archived_plan_identity");
            let preview = materialized::preview(
                store,
                format!("{HEADER}{ONE}").as_bytes(),
                &mapping(),
                &flag(),
            )?;
            assert!(matches!(&preview.rows[0].source_match,
                SourceMatch::PreviouslyImported { event_id, revision_id }
                    if event_id == &source.event_id && revision_id == &source.revision_id));
            store.commit(&second)
        })
        .unwrap();
        (
            reseal(&host, &selected),
            candidate.ticket.target_id().to_owned(),
        )
    };
    drop(archive);

    let empty_tail_archive = {
        let blank = BlankHost::new();
        let (candidate, selected, ready) = blank.restore(&next_archive);
        assert_ne!(candidate.ticket.target_id(), prior_id);
        assert_eq!(ready.verification.projection.balances_minor[&id(1)], "-52");
        assert_eq!(ready.facts.history.len(), 3);
        assert_eq!(
            ready.facts.history[2].view().intents[0].operation_id,
            id(731)
        );
        let host = blank.host(&candidate);
        host.write(&selected, |store| {
            store.add_account(&id(739), "cash", "新账户")
        })
        .unwrap();
        reseal(&host, &selected)
    };

    let blank = BlankHost::new();
    let (candidate, selected, ready) = blank.restore(&empty_tail_archive);
    assert_eq!(ready.facts.history.len(), 4);
    assert_eq!(ready.verification.projection.balances_minor[&id(739)], "0");
    let repeated = reseal(&blank.host(&candidate), &selected);
    let restored::Opened::Restored(opened) =
        restored::open(repeated.as_bytes(), PASSWORD, CANDIDATE_LIMITS, &flag()).unwrap()
    else {
        panic!("expected 0.2 archive")
    };
    assert_eq!(opened.contents.facts.history.len(), 4);
    assert!(opened.contents.facts.history[3].view().intents.is_empty());
    assert_eq!(
        opened.contents.verification.projection.balances_minor[&id(739)],
        "0"
    );
}

#[test]
fn restore_roundtrip_rolls_back_and_resumes_owned_stage_and_candidate() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let artifact: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/validation/archive-restored-review-vector.json"
    ))
    .unwrap();
    let archive: Vec<_> = artifact["output"]["archiveHex"]
        .as_str()
        .unwrap()
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    let password = b"public-restored-review-password";
    let blank = BlankHost::new();
    let stage = Task::new(&archive).unwrap();
    let stage_dir = materialization::directory(blank.root(), stage.ticket());
    assert_eq!(
        code(stage.prepare_restored(
            blank.root(),
            &archive,
            b"wrong password",
            CANDIDATE_LIMITS,
            &flag(),
            |_| Ok(())
        )),
        "archive_unlock_failed"
    );
    assert!(!stage_dir.exists());
    let cancel = flag();
    assert_eq!(
        code(stage.prepare_restored(
            blank.root(),
            &archive,
            password,
            CANDIDATE_LIMITS,
            &cancel,
            |point| {
                if point == super::super::super::Point::BeforeCommit {
                    cancel.store(true, Ordering::Relaxed);
                }
                Ok(())
            }
        )),
        "cancelled"
    );
    assert_eq!(
        restore_store::test_count(
            &stage_dir.join("stage.db"),
            &stage.key,
            "SELECT count(*) FROM restore_bundle WHERE payload IS NOT NULL"
        )
        .unwrap(),
        0
    );
    let (ticket, key) = stage.into_parts();
    let stage = Task::resume(ticket, key).unwrap();
    stage
        .prepare_restored(
            blank.root(),
            &archive,
            password,
            CANDIDATE_LIMITS,
            &flag(),
            |_| Ok(()),
        )
        .unwrap();
    let mut candidate = Candidate::new(&stage).unwrap();
    let input = StageInput {
        task: &stage,
        parent: blank.root(),
        archive: &archive,
        password,
    };
    let cancel = flag();
    assert_eq!(
        code(candidate.prepare_restored(
            &input,
            blank.root(),
            CANDIDATE_LIMITS,
            &cancel,
            |point| {
                if point == MaterializePoint::BeforeCommit {
                    cancel.store(true, Ordering::Relaxed);
                }
                Ok(())
            }
        )),
        "cancelled"
    );
    let path = blank.host(&candidate).database(&target(&candidate));
    assert_eq!(
        restore_store::test_count(&path, &candidate.key, "SELECT count(*) FROM revisions").unwrap(),
        0
    );
    let mut candidate =
        Candidate::resume(candidate.ticket, candidate.device, candidate.key).unwrap();
    let ready = candidate
        .prepare_restored(&input, blank.root(), CANDIDATE_LIMITS, &flag(), |_| Ok(()))
        .unwrap();
    assert_eq!(ready.verification.projection.net_assets_minor, "137");
    assert_eq!(blank.host(&candidate).recover().unwrap(), blank.original);
    restore_store::test_sql(
        &stage_dir.join("stage.db"),
        &stage.key,
        "UPDATE restore_bundle SET payload=x'00'",
    )
    .unwrap();
    assert_eq!(
        code(
            candidate.prepare_restored(&input, blank.root(), CANDIDATE_LIMITS, &flag(), |_| Ok(()))
        ),
        "restore_mismatch"
    );
    assert_eq!(
        restore_store::test_count(&path, &candidate.key, "SELECT count(*) FROM revisions").unwrap(),
        2
    );
}

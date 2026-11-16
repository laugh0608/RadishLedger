use super::super::{CANDIDATE_LIMITS, MAX_PAYLOAD, seal};
use super::*;
use crate::csv_import::test_support::{Worker, id};
use crate::restore_stage::Task;
use crate::store::{DbKey, Store, snapshot_store::tests::assert_expected};
use std::os::unix::{fs::symlink, process::ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const PASSWORD: &[u8] = b"public-archive-file-test-password";
const OLD: &str = "old.archive";
const NEW: &str = "new.archive";

struct Fixture {
    directory: tempfile::TempDir,
    store: Store,
    archive: SealedArchive,
    aliases: std::collections::BTreeMap<String, String>,
}

impl Fixture {
    fn new() -> Self {
        let directory = private_temp();
        let mut store = Store::create(
            &directory.path().join("source.db"),
            &DbKey::random().unwrap(),
        )
        .unwrap();
        let aliases =
            crate::probe::populate_snapshot_fixture(&mut store, "A11", "conflict").unwrap();
        let archive = seal_store(&store);
        Self {
            directory,
            store,
            archive,
            aliases,
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }
}

fn private_temp() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

fn seal_store(store: &Store) -> SealedArchive {
    let cancel = AtomicBool::new(false);
    let snapshot = store
        .logical_snapshot(
            crate::logical_snapshot::Limits {
                rows: 100_000,
                bytes: MAX_PAYLOAD,
            },
            &["2026-01".into()],
            &cancel,
        )
        .unwrap();
    seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &cancel).unwrap()
}

fn files<'a>(root: &Path, cancel: &'a AtomicBool) -> ArchiveFiles<'a> {
    ArchiveFiles::open(root, PASSWORD, CANDIDATE_LIMITS, cancel).unwrap()
}

fn code<T>(result: Result<T>) -> &'static str {
    result.err().expect("expected archive file rejection").code
}

fn temporaries(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .as_encoded_bytes()
                .starts_with(TEMP_PREFIX.as_bytes())
        })
        .collect()
}

#[test]
fn e15_reopens_saved_legacy_archive_in_an_independent_sqlcipher_stage() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let cancel = AtomicBool::new(false);
    let output = private_temp();
    let files = files(output.path(), &cancel);
    let aliases = {
        let fixture = Fixture::new();
        files.save_new(OsStr::new(NEW), &fixture.archive).unwrap();
        fixture.aliases
    };
    let path = output.path().join(NEW);
    let metadata = fs::metadata(path).unwrap();
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert_eq!(metadata.nlink(), 1);
    assert!(temporaries(output.path()).is_empty());

    let saved = files.read_saved(OsStr::new(NEW)).unwrap();
    let target = private_temp();
    let input = saved.as_bytes();
    let task = Task::new(input).unwrap();
    task.prepare(target.path(), input, PASSWORD, CANDIDATE_LIMITS, &cancel)
        .unwrap();
    let stage = task
        .inspect(target.path(), input, PASSWORD, CANDIDATE_LIMITS, &cancel)
        .unwrap();
    let contents = stage.contents();
    assert_expected(contents, &aliases, "A11", "conflict");
    assert_eq!(
        (
            contents.facts.revisions.len(),
            contents.facts.candidates.len(),
            contents.facts.intent_history.len(),
            contents.verification.deleted_event_ids.len(),
        ),
        (8, 2, 7, 1)
    );
}

#[test]
fn e15_reuses_archive_capacity_and_rejects_over_budget_files_before_authentication() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let mut fixture = Fixture::new();
    for n in 1000..1300 {
        fixture
            .store
            .add_account(&id(n), "cash", "合成归档资源测试")
            .unwrap();
    }
    let archive = seal_store(&fixture.store);
    assert!(archive.as_bytes().len() > crate::bootstrap_files::MAX_BOOTSTRAP);
    let cancel = AtomicBool::new(false);
    let files = files(fixture.root(), &cancel);
    files.save_new(OsStr::new(NEW), &archive).unwrap();
    assert_eq!(
        files.read_saved(OsStr::new(NEW)).unwrap().as_bytes(),
        archive.as_bytes()
    );
    let limits = Limits {
        payload_bytes: 1000,
        ..CANDIDATE_LIMITS
    };
    let tighter = ArchiveFiles::open(fixture.root(), PASSWORD, limits, &cancel).unwrap();
    assert_eq!(
        code(tighter.save_new(OsStr::new(OLD), &archive)),
        "archive_limit"
    );
    assert_eq!(code(tighter.read_saved(OsStr::new(NEW))), "archive_limit");
    let oversized = File::create(fixture.root().join(OLD)).unwrap();
    oversized
        .set_len((MAX_PAYLOAD + OVERHEAD + 1) as u64)
        .unwrap();
    assert_eq!(code(files.read_saved(OsStr::new(OLD))), "archive_limit");
}

#[test]
fn e15_prepublication_errors_and_cancellation_preserve_old_archive_and_ledger() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let fixture = Fixture::new();
    let cancel = AtomicBool::new(false);
    let files = files(fixture.root(), &cancel);
    files.save_new(OsStr::new(OLD), &fixture.archive).unwrap();
    let old_bytes = fs::read(fixture.root().join(OLD)).unwrap();
    let before = fixture.store.content_digest().unwrap();
    let next = seal_store(&fixture.store);
    assert_ne!(
        next.ciphertext_sha256(),
        fixture.archive.ciphertext_sha256()
    );

    for fault in [Point::PartialWrite, Point::FileSync, Point::Publish] {
        let result = files.save_observed(OsStr::new(NEW), &next, |point, temporary| {
            if point == fault {
                let size = fs::metadata(temporary)?.len();
                if point == Point::PartialWrite {
                    assert_eq!(size, super::super::HEADER as u64);
                } else {
                    assert_eq!(size, next.as_bytes().len() as u64);
                }
                return Err(io::Error::other("synthetic archive save failure"));
            }
            Ok(())
        });
        assert_eq!(code(result), "archive_file_io");
        assert!(!fixture.root().join(NEW).exists());
        assert!(temporaries(fixture.root()).is_empty());
        assert_eq!(fs::read(fixture.root().join(OLD)).unwrap(), old_bytes);
    }
    for point_to_cancel in [Point::PartialWrite, Point::Publish] {
        let result = files.save_observed(OsStr::new(NEW), &next, |point, _| {
            if point == point_to_cancel {
                cancel.store(true, Ordering::Relaxed);
            }
            Ok(())
        });
        assert_eq!(code(result), "cancelled");
        assert!(!fixture.root().join(NEW).exists());
        assert!(temporaries(fixture.root()).is_empty());
        assert_eq!(fs::read(fixture.root().join(OLD)).unwrap(), old_bytes);
        cancel.store(false, Ordering::Relaxed);
    }
    assert_eq!(
        code(
            files.save_observed(OsStr::new(NEW), &next, |point, temporary| {
                if point == Point::ReadBack {
                    let mut file = fs::OpenOptions::new().write(true).open(temporary)?;
                    file.seek(io::SeekFrom::End(-1))?;
                    file.write_all(&[next.as_bytes().last().unwrap() ^ 1])?;
                }
                Ok(())
            })
        ),
        "archive_content_auth_failed"
    );
    assert!(!fixture.root().join(NEW).exists());
    assert!(temporaries(fixture.root()).is_empty());
    assert_eq!(
        files.read_saved(OsStr::new(OLD)).unwrap().as_bytes(),
        old_bytes
    );
    assert_eq!(fixture.store.content_digest().unwrap(), before);
    files.save_new(OsStr::new(NEW), &next).unwrap();
}

#[test]
fn e15_existing_targets_and_publication_race_never_overwrite() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let fixture = Fixture::new();
    let cancel = AtomicBool::new(false);
    let files = files(fixture.root(), &cancel);
    files.save_new(OsStr::new(OLD), &fixture.archive).unwrap();
    let old = fixture.root().join(OLD);
    let next = seal_store(&fixture.store);
    let before = fixture.store.content_digest().unwrap();
    assert_eq!(
        code(files.save_new(OsStr::new(OLD), &next)),
        "archive_exists"
    );
    let linked = fixture.root().join("linked.archive");
    symlink(&old, &linked).unwrap();
    assert_eq!(
        code(files.save_new(OsStr::new("linked.archive"), &next)),
        "archive_exists"
    );
    assert_eq!(
        code(files.read_saved(OsStr::new("linked.archive"))),
        "invalid_archive"
    );
    assert_eq!(fs::read_link(linked).unwrap(), old);
    let competing = fixture.root().join(NEW);
    assert_eq!(
        code(files.save_observed(OsStr::new(NEW), &next, |point, _| {
            if point == Point::Publish {
                fs::hard_link(&old, &competing)?;
            }
            Ok(())
        })),
        "archive_exists"
    );
    assert!(temporaries(fixture.root()).is_empty());
    assert_eq!(fs::read(competing).unwrap(), fixture.archive.as_bytes());
    assert_eq!(
        files.read_saved(OsStr::new(OLD)).unwrap().as_bytes(),
        fixture.archive.as_bytes()
    );
    assert_eq!(fixture.store.content_digest().unwrap(), before);
}

#[test]
fn e15_post_publication_uncertainty_rechecks_only_the_expected_authenticated_file() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let fixture = Fixture::new();
    let cancel = AtomicBool::new(false);
    let files = files(fixture.root(), &cancel);
    files.save_new(OsStr::new(OLD), &fixture.archive).unwrap();
    let before = fixture.store.content_digest().unwrap();
    let next = seal_store(&fixture.store);
    let digest = next.ciphertext_sha256();
    assert_eq!(
        code(files.recheck_saved(OsStr::new(NEW), &digest)),
        "archive_not_found"
    );
    assert_eq!(
        code(files.save_observed(OsStr::new(NEW), &next, |point, _| {
            if point == Point::Published {
                cancel.store(true, Ordering::Relaxed);
            }
            if point == Point::DirectorySync {
                return Err(io::Error::other("synthetic parent sync failure"));
            }
            Ok(())
        })),
        "archive_save_uncertain"
    );
    assert_eq!(fs::read(fixture.root().join(NEW)).unwrap(), next.as_bytes());
    assert!(temporaries(fixture.root()).is_empty());
    cancel.store(false, Ordering::Relaxed);
    assert_eq!(
        code(files.save_new(OsStr::new(NEW), &next)),
        "archive_exists"
    );
    assert_eq!(
        code(files.recheck_saved(OsStr::new(OLD), &digest)),
        "archive_file_mismatch"
    );
    let wrong =
        ArchiveFiles::open(fixture.root(), b"wrong password", CANDIDATE_LIMITS, &cancel).unwrap();
    assert_eq!(
        code(wrong.recheck_saved(OsStr::new(NEW), &digest)),
        "archive_unlock_failed"
    );
    files.recheck_saved(OsStr::new(NEW), &digest).unwrap();
    assert_eq!(
        files.read_saved(OsStr::new(OLD)).unwrap().as_bytes(),
        fixture.archive.as_bytes()
    );
    assert_eq!(fixture.store.content_digest().unwrap(), before);

    files
        .save_observed(OsStr::new("late.archive"), &next, |point, _| {
            if point == Point::Published {
                cancel.store(true, Ordering::Relaxed);
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(
        fs::read(fixture.root().join("late.archive")).unwrap(),
        next.as_bytes()
    );
}

#[test]
fn e15_requires_a_stable_private_directory_and_single_non_temporary_name() {
    let root = private_temp();
    let cancel = AtomicBool::new(false);
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        code(ArchiveFiles::open(
            &shared,
            PASSWORD,
            CANDIDATE_LIMITS,
            &cancel
        )),
        "invalid_archive_directory"
    );
    let files = files(root.path(), &cancel);
    for invalid in [
        "",
        ".",
        "..",
        "../x",
        "/x",
        "x/y",
        "x/",
        ".radishledger-archive-stale",
    ] {
        assert_eq!(
            code(files.read_saved(OsStr::new(invalid))),
            "invalid_archive_name"
        );
    }
    let directory = private_temp();
    let original = directory.path().join("original");
    fs::create_dir(&original).unwrap();
    fs::set_permissions(&original, fs::Permissions::from_mode(0o700)).unwrap();
    let checked = ArchiveFiles::open(&original, PASSWORD, CANDIDATE_LIMITS, &cancel).unwrap();
    fs::rename(&original, directory.path().join("relocated")).unwrap();
    fs::create_dir(&original).unwrap();
    fs::set_permissions(original, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        code(checked.read_saved(OsStr::new(OLD))),
        "invalid_archive_directory"
    );
}

#[test]
#[ignore = "由 E15 父测试提供专属合成目录和密文"]
fn e15_save_worker() {
    let root = PathBuf::from(std::env::var_os("RADISH_E15_WORKER_ROOT").unwrap());
    let stop = match std::env::var("RADISH_E15_WORKER_POINT").unwrap().as_str() {
        "before" => Point::Publish,
        "after" => Point::Published,
        _ => panic!("unexpected E15 interruption point"),
    };
    let mut bytes = buffer(0).unwrap();
    let mut input = io::stdin().lock();
    let mut length = [0; 4];
    input.read_exact(&mut length).unwrap();
    let length = u32::from_be_bytes(length) as usize;
    assert!(length <= MAX_PAYLOAD + OVERHEAD);
    bytes.resize(length, 0);
    input.read_exact(&mut bytes).unwrap();
    let cancel = AtomicBool::new(false);
    files(&root, &cancel)
        .save_observed(OsStr::new(NEW), &SealedArchive(bytes), |point, _| {
            if point == stop {
                fs::write(root.join("reached"), b"ready")?;
                input.read_exact(&mut [0])?;
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn e15_sigkill_before_and_after_publication_preserves_old_and_rechecks_new() {
    let _kdf = crate::crypto::KDF_TESTS.lock().unwrap();
    let fixture = Fixture::new();
    let before = fixture.store.content_digest().unwrap();
    let next = seal_store(&fixture.store);
    let cancel = AtomicBool::new(false);
    for point in ["before", "after"] {
        let root = private_temp();
        let files = files(root.path(), &cancel);
        files.save_new(OsStr::new(OLD), &fixture.archive).unwrap();
        let mut worker = Worker(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    concat!(module_path!(), "::e15_save_worker")
                        .split_once("::")
                        .unwrap()
                        .1,
                ])
                .env("RADISH_E15_WORKER_ROOT", root.path())
                .env("RADISH_E15_WORKER_POINT", point)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut input = worker.0.stdin.take().unwrap();
        let bytes = next.as_bytes();
        input
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .unwrap();
        input.write_all(bytes).unwrap();
        input.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !root.path().join("reached").is_file() {
            assert!(
                worker.0.try_wait().unwrap().is_none(),
                "E15 worker exited early"
            );
            assert!(Instant::now() < deadline, "E15 worker timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        worker.0.kill().unwrap();
        assert_eq!(worker.0.wait().unwrap().signal(), Some(9));
        let orphan = temporaries(root.path());
        assert_eq!(orphan.len(), 1);
        assert_eq!(fs::read(&orphan[0]).unwrap(), next.as_bytes());
        let digest = next.ciphertext_sha256();
        if point == "before" {
            assert_eq!(
                code(files.recheck_saved(OsStr::new(NEW), &digest)),
                "archive_not_found"
            );
            let replacement = seal_store(&fixture.store);
            assert_ne!(replacement.ciphertext_sha256(), digest);
            files.save_new(OsStr::new(NEW), &replacement).unwrap();
            files
                .recheck_saved(OsStr::new(NEW), &replacement.ciphertext_sha256())
                .unwrap();
        } else {
            files.recheck_saved(OsStr::new(NEW), &digest).unwrap();
            assert_eq!(
                files.read_saved(OsStr::new(NEW)).unwrap().as_bytes(),
                next.as_bytes()
            );
        }
        assert_eq!(temporaries(root.path()), orphan);
        assert_eq!(
            files.read_saved(OsStr::new(OLD)).unwrap().as_bytes(),
            fixture.archive.as_bytes()
        );
    }
    assert_eq!(fixture.store.content_digest().unwrap(), before);
}

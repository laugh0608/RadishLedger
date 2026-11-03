//! 只强杀本次子进程；随机暂存钥通过匿名 stdin，参数 / 日志不含密钥。
use super::*;
use std::io::{Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn reached(directory: &Path) {
    let mut marker = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join("reached"))
        .unwrap();
    marker.write_all(b"ready").unwrap();
    marker.sync_all().unwrap();
    loop {
        std::thread::park_timeout(Duration::from_secs(1));
    }
}
#[test]
#[ignore = "由父测试以匿名管道提供合成暂存钥并在实际写入 / 清理点强杀"]
fn restore_crash_worker() {
    let directory = std::path::PathBuf::from(std::env::var_os("RLM0_RESTORE_DIRECTORY").unwrap());
    let point: usize = std::env::var("RLM0_RESTORE_POINT")
        .unwrap()
        .parse()
        .unwrap();
    let mut secret = zeroize::Zeroizing::new([0_u8; 32]);
    std::io::stdin().read_exact(secret.as_mut()).unwrap();
    let key = DbKey::from_secret(secret.as_ref()).unwrap();
    let ticket: Ticket =
        serde_json::from_slice(&fs::read(directory.join("ticket.json")).unwrap()).unwrap();
    let task = Task::resume(ticket, key).unwrap();
    if point == POINTS.len() {
        task.abandon_observed(&directory, |name| {
            if name == "stage.db" {
                reached(&directory);
            }
            Ok(())
        })
        .unwrap();
    } else {
        let archive = fs::read(directory.join("input.archive")).unwrap();
        task.prepare_observed(
            &directory,
            &archive,
            PASSWORD,
            CANDIDATE_LIMITS,
            &flag(),
            |p| {
                if p == POINTS[point] {
                    reached(&directory);
                }
                Ok(())
            },
        )
        .unwrap();
    }
    panic!("requested stage point was not reached");
}
struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn spawn(directory: &Path, point: usize, secret: &[u8]) -> Worker {
    let mut worker = Worker(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "restore_stage::tests::crash::restore_crash_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("RLM0_RESTORE_DIRECTORY", directory)
            .env("RLM0_RESTORE_POINT", point.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    worker.0.stdin.take().unwrap().write_all(secret).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !directory.join("reached").exists() {
        assert!(
            worker.0.try_wait().unwrap().is_none(),
            "restore worker exited before marker"
        );
        assert!(Instant::now() < deadline, "restore worker deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
    worker
}
fn kill(worker: &mut Worker) {
    worker.0.kill().unwrap();
    assert_eq!(worker.0.wait().unwrap().signal(), Some(9));
}

#[test]
fn killed_staging_recovers_only_registered_or_whole_facts_and_retries_same_identity() {
    let _guard = crate::crypto::KDF_TESTS.lock().unwrap();
    let (source_dir, _old_key, store, snapshot) = source("A11", "conflict");
    let original = store.content_digest().unwrap();
    let archive = seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &flag()).unwrap();
    for (n, point) in POINTS.iter().enumerate() {
        let dir = tempfile::tempdir_in(source_dir.path()).unwrap();
        let mut task = Task::new(archive.as_bytes()).unwrap();
        let secret = crate::crypto::random::<32>().unwrap();
        task.key = DbKey::from_secret(secret.as_ref()).unwrap();
        // 父测试先登记 owner，子进程只重开本任务；不保留对原 DB Key 的访问通道。
        drop(files::Files::acquire(dir.path(), task.ticket(), true).unwrap());
        fs::write(
            dir.path().join("ticket.json"),
            serde_json::to_vec(task.ticket()).unwrap(),
        )
        .unwrap();
        fs::write(dir.path().join("input.archive"), archive.as_bytes()).unwrap();
        let mut worker = spawn(dir.path(), n, secret.as_ref());
        if *point != Point::Owned {
            let visible = crate::store::restore_store::test_count(
                &path(dir.path(), task.ticket()),
                &task.key,
                "SELECT count(*) FROM restore_revisions",
            )
            .unwrap();
            assert_eq!(visible, if n >= 7 { 8 } else { 0 });
        }
        kill(&mut worker);
        let target_id = task.ticket.target_id.clone();
        let result = task
            .prepare(
                dir.path(),
                archive.as_bytes(),
                PASSWORD,
                CANDIDATE_LIMITS,
                &flag(),
            )
            .unwrap();
        assert_eq!(result.ticket().target_id, target_id);
        assert_contents(result.contents(), snapshot.contents());
        assert_eq!(store.content_digest().unwrap(), original);
        task.abandon(dir.path()).unwrap();
    }
}

#[test]
fn killed_discard_keeps_permit_and_finishes_without_reopening_deleted_database() {
    let _guard = crate::crypto::KDF_TESTS.lock().unwrap();
    let (dir, _key, _store, snapshot) = source("A11", "conflict");
    let archive = seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &flag()).unwrap();
    let mut task = Task::new(archive.as_bytes()).unwrap();
    let secret = crate::crypto::random::<32>().unwrap();
    task.key = DbKey::from_secret(secret.as_ref()).unwrap();
    task.prepare(
        dir.path(),
        archive.as_bytes(),
        PASSWORD,
        CANDIDATE_LIMITS,
        &flag(),
    )
    .unwrap();
    fs::write(
        dir.path().join("ticket.json"),
        serde_json::to_vec(task.ticket()).unwrap(),
    )
    .unwrap();
    let mut worker = spawn(dir.path(), POINTS.len(), secret.as_ref());
    kill(&mut worker);
    assert!(!path(dir.path(), task.ticket()).exists());
    task.abandon(dir.path()).unwrap();
    task.abandon(dir.path()).unwrap();
    assert_eq!(
        code(task.prepare(
            dir.path(),
            archive.as_bytes(),
            PASSWORD,
            CANDIDATE_LIMITS,
            &flag()
        )),
        "restore_io"
    );
}

//! 独立进程 SIGKILL；仅使用父测试创建的合成目录，密钥走匿名管道。
use super::*;
use std::io::{Read, Write as IoWrite};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const POINTS: [ImportPoint; 5] = [
    ImportPoint::AfterFact,
    ImportPoint::AfterSource,
    ImportPoint::BeforeProgress,
    ImportPoint::BeforeCommit,
    ImportPoint::AfterCommit,
];

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "由父测试以匿名管道提供合成材料并在写入点强杀"]
fn csv_crash_worker() {
    let directory =
        std::path::PathBuf::from(std::env::var_os("RLM0_CSV_DIRECTORY").expect("parent required"));
    let point: usize = std::env::var("RLM0_CSV_POINT").unwrap().parse().unwrap();
    let mut bytes = zeroize::Zeroizing::new([0_u8; 32]);
    std::io::stdin().read_exact(bytes.as_mut()).unwrap();
    let key = DbKey::from_secret(bytes.as_ref()).unwrap();
    let mut store = Store::open(&directory.join("import.db"), &key).unwrap();
    let task = store.csv_tasks().unwrap().remove(0);
    store
        .publish_csv_observed(&task.target_id, &task.id, 1, &no_cancel(), |observed| {
            if observed == POINTS[point] {
                let mut marker = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(directory.join("reached"))
                    .unwrap();
                marker.write_all(b"ready").unwrap();
                marker.sync_all().unwrap();
                // 父测试有 deadline 和 Drop 清理，不能让失败轮残留子进程。
                loop {
                    std::thread::park_timeout(Duration::from_secs(1));
                }
            }
            Ok(())
        })
        .unwrap();
    panic!("requested commit point was not observed");
}

#[test]
fn killed_process_recovers_whole_batch_and_retries_original_ids() {
    for (n, point) in POINTS.iter().enumerate() {
        let (directory, key, mut store) = database_v2();
        let text = HEADER.to_owned() + ONE + &ONE.replace("src-1", "src-2");
        let task = save(&mut store, &text, 900, 2);
        let identities = store.csv_task_rows(&task.target_id, &task.id).unwrap();
        let before = store.content_digest().unwrap();
        drop(store);
        // 原库关闭后复制；仅用于独立完整提交对照，绝不复制活动 WAL 主文件。
        std::fs::copy(
            directory.path().join("import.db"),
            directory.path().join("expected.db"),
        )
        .unwrap();
        let mut expected = Store::open(&directory.path().join("expected.db"), &key).unwrap();
        expected
            .publish_csv_batch(&task.target_id, &task.id, 1, &no_cancel())
            .unwrap();
        let after = expected.content_digest().unwrap();
        drop(expected);
        assert_ne!(before, after);
        let mut worker = Worker(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "store::csv_store::tests::crash::csv_crash_worker",
                    "--ignored",
                    "--nocapture",
                ])
                .env("RLM0_CSV_DIRECTORY", directory.path())
                .env("RLM0_CSV_POINT", n.to_string())
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::from(
                    std::fs::File::create(directory.path().join("worker.stderr")).unwrap(),
                ))
                .spawn()
                .unwrap(),
        );
        let mut input = worker.0.stdin.take().unwrap();
        input.write_all(key.0.as_ref()).unwrap();
        drop(input);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !directory.path().join("reached").exists() {
            if let Some(status) = worker.0.try_wait().unwrap() {
                panic!(
                    "CSV worker exited {status}: {}",
                    std::fs::read_to_string(directory.path().join("worker.stderr")).unwrap()
                );
            }
            assert!(Instant::now() < deadline, "CSV worker deadline");
            std::thread::sleep(Duration::from_millis(10));
        }
        let committed = *point == ImportPoint::AfterCommit;
        let visible = Store::open_readonly(&directory.path().join("import.db"), &key).unwrap();
        assert_eq!(
            visible.content_digest().unwrap(),
            if committed { &after } else { &before }.as_str()
        );
        drop(visible);
        worker.0.kill().unwrap();
        assert_eq!(worker.0.wait().unwrap().signal(), Some(9));
        let mut recovered = Store::open(&directory.path().join("import.db"), &key).unwrap();
        assert_eq!(
            recovered.content_digest().unwrap(),
            if committed { &after } else { &before }.as_str()
        );
        let result = recovered
            .publish_csv_batch(&task.target_id, &task.id, 1, &no_cancel())
            .unwrap();
        assert_eq!(result.replayed, committed);
        assert_eq!(recovered.content_digest().unwrap(), after);
        let rows = recovered.csv_task_rows(&task.target_id, &task.id).unwrap();
        for (prior, current) in identities.iter().zip(rows) {
            assert_eq!(prior.row_id, current.row_id);
            assert_eq!(prior.operation_id, current.operation_id);
            assert_eq!(current.outcome, RowOutcome::Imported);
        }
        assert_eq!(counts(&recovered), (2, 2, 2, 1, 2));
    }
}

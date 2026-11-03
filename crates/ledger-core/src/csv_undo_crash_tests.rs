//! 父测试仅强杀本次创建的专属 worker；密钥走匿名 stdin，不进入参数或日志。
use super::*;
use std::io::{Read, Write as IoWrite};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const POINTS: [UndoPoint; 5] = [
    UndoPoint::AfterDelete,
    UndoPoint::AfterRow,
    UndoPoint::BeforeProgress,
    UndoPoint::BeforeCommit,
    UndoPoint::AfterCommit,
];

#[test]
#[ignore = "由父测试以匿名管道提供合成材料并在撤销写入点强杀"]
fn csv_undo_crash_worker() {
    let directory =
        std::path::PathBuf::from(std::env::var_os("RLM0_UNDO_DIRECTORY").expect("parent required"));
    let point: usize = std::env::var("RLM0_UNDO_POINT").unwrap().parse().unwrap();
    let mut bytes = zeroize::Zeroizing::new([0_u8; 32]);
    std::io::stdin().read_exact(bytes.as_mut()).unwrap();
    let key = DbKey::from_secret(bytes.as_ref()).unwrap();
    let mut store = Store::open(&directory.join("undo.db"), &key).unwrap();
    let import = store.csv_tasks().unwrap().remove(0);
    let task = store
        .csv_undo_tasks(&import.target_id, &import.id)
        .unwrap()
        .remove(0);
    assert_eq!((task.deleted, task.next_ordinal), (2, 3));
    store
        .publish_csv_undo_observed(&task.target_id, &task.id, 3, &no_cancel(), |observed| {
            if observed == POINTS[point] {
                let mut marker = std::fs::OpenOptions::new()
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
            Ok(())
        })
        .unwrap();
    panic!("requested undo point was not observed");
}

#[test]
fn killed_undo_preserves_prior_batch_and_recovers_original_delete_identities() {
    for (n, point) in POINTS.iter().enumerate() {
        let (directory, key, mut store) = database_v3();
        let import = imported(
            &mut store,
            &(HEADER.to_owned() + &ONE.replace("src-1", "").repeat(4)),
            4,
        );
        let task = confirm(&mut store, &import, 800, &[1, 2, 3, 4], 2);
        let identities = store.csv_undo_rows(&import.target_id, &task.id).unwrap();
        store
            .publish_csv_undo_batch(&import.target_id, &task.id, 1, &no_cancel())
            .unwrap();
        let before = store.content_digest().unwrap();
        drop(store);
        // 关闭原库后建立完整提交对照；不复制带有活动 WAL 的主文件。
        std::fs::copy(
            directory.path().join("undo.db"),
            directory.path().join("expected.db"),
        )
        .unwrap();
        let mut expected = Store::open(&directory.path().join("expected.db"), &key).unwrap();
        expected
            .publish_csv_undo_batch(&import.target_id, &task.id, 3, &no_cancel())
            .unwrap();
        let after = expected.content_digest().unwrap();
        drop(expected);
        assert_ne!(before, after);
        let mut worker = Worker(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "store::csv_undo_store::tests::crash::csv_undo_crash_worker",
                    "--ignored",
                    "--nocapture",
                ])
                .env("RLM0_UNDO_DIRECTORY", directory.path())
                .env("RLM0_UNDO_POINT", n.to_string())
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
                    "undo worker exited {status}: {}",
                    std::fs::read_to_string(directory.path().join("worker.stderr")).unwrap()
                );
            }
            assert!(Instant::now() < deadline, "undo worker deadline");
            std::thread::sleep(Duration::from_millis(10));
        }
        let committed = *point == UndoPoint::AfterCommit;
        let visible = Store::open_readonly(&directory.path().join("undo.db"), &key).unwrap();
        assert_eq!(
            visible.content_digest().unwrap(),
            if committed { &after } else { &before }.as_str()
        );
        drop(visible);
        worker.0.kill().unwrap();
        assert_eq!(worker.0.wait().unwrap().signal(), Some(9));
        let mut recovered = Store::open(&directory.path().join("undo.db"), &key).unwrap();
        assert_eq!(
            recovered.content_digest().unwrap(),
            if committed { &after } else { &before }.as_str()
        );
        let progress = recovered
            .csv_undo_task(&import.target_id, &task.id)
            .unwrap();
        assert_eq!(progress.deleted, if committed { 4 } else { 2 });
        let result = recovered
            .publish_csv_undo_batch(&import.target_id, &task.id, 3, &no_cancel())
            .unwrap();
        assert_eq!(result.replayed, committed);
        assert_eq!(recovered.content_digest().unwrap(), after);
        let rows = recovered
            .csv_undo_rows(&import.target_id, &task.id)
            .unwrap();
        for (before, after) in identities.iter().zip(rows) {
            assert_eq!(
                (
                    &before.row_id,
                    &before.operation_id,
                    &before.revision_id,
                    &before.original_revision_id
                ),
                (
                    &after.row_id,
                    &after.operation_id,
                    &after.revision_id,
                    &after.original_revision_id
                )
            );
            assert!(after.deleted);
            assert_eq!(recovered.revision_event(&after.revision_id).unwrap(), None);
        }
        assert_eq!(
            (
                count(&recovered, "revisions"),
                count(&recovered, "outbox"),
                count(&recovered, "csv_undo_batches")
            ),
            (8, 8, 2)
        );
        assert_eq!(recovered.snapshot(&[]).unwrap().deleted_event_ids.len(), 4);
    }
}

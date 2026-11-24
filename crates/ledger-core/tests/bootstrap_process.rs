use radishledger_core::crypto;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

fn start(directory: &Path, run: &str, action: Value, pause: Option<&str>, secret: &[u8]) -> Child {
    start_request(directory, run, action, pause, None, secret)
}
fn start_request(
    directory: &Path,
    run: &str,
    action: Value,
    pause: Option<&str>,
    fault: Option<Value>,
    secret: &[u8],
) -> Child {
    start_request_with_worker(
        Path::new(env!("CARGO_BIN_EXE_m0-bootstrap-worker")),
        directory,
        run,
        action,
        pause,
        fault,
        secret,
    )
}
fn start_request_with_worker(
    worker: &Path,
    directory: &Path,
    run: &str,
    action: Value,
    pause: Option<&str>,
    fault: Option<Value>,
    secret: &[u8],
) -> Child {
    spawn_request(
        worker,
        json!({"abi":1,"directory":directory,"run":run,"action":action,"pause_at":pause,"file_fault":fault}),
        secret,
    )
}

fn spawn_request(worker: &Path, request: Value, secret: &[u8]) -> Child {
    let metadata = serde_json::to_vec(&request).unwrap();
    let mut child = Command::new(worker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(&(metadata.len() as u32).to_be_bytes())
        .unwrap();
    stdin
        .write_all(&(secret.len() as u32).to_be_bytes())
        .unwrap();
    stdin.write_all(&metadata).unwrap();
    stdin.write_all(secret).unwrap();
    child
}
fn finish(mut child: Child) -> (Value, Zeroizing<Vec<u8>>) {
    let deadline = Instant::now() + Duration::from_secs(40);
    // 输出有界（本切片 < 8 KiB），先有界等待；超时只终止本函数创建的 PID。
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("bootstrap worker exceeded deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut output = Zeroizing::new(Vec::new());
    child
        .stdout
        .take()
        .unwrap()
        .take(32768)
        .read_to_end(&mut output)
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert!(output.len() >= 8);
    let metadata_len = u32::from_be_bytes(output[..4].try_into().unwrap()) as usize;
    let secret_len = u32::from_be_bytes(output[4..8].try_into().unwrap()) as usize;
    assert!(
        metadata_len <= 16384 && secret_len <= 128 && output.len() == 8 + metadata_len + secret_len
    );
    let value = serde_json::from_slice(&output[8..8 + metadata_len]).unwrap();
    (value, Zeroizing::new(output[8 + metadata_len..].to_vec()))
}
fn call(directory: &Path, run: &str, action: Value, secret: &[u8]) -> (Value, Zeroizing<Vec<u8>>) {
    finish(start(directory, run, action, None, secret))
}
fn ok(response: (Value, Zeroizing<Vec<u8>>)) -> (Value, Zeroizing<Vec<u8>>) {
    assert_eq!(
        response.0["ok"], true,
        "non-secret response: {}",
        response.0
    );
    (response.0["report"].clone(), response.1)
}

fn assert_nonempty(report: &Value) {
    let audit = &report["audit"];
    let snapshot = &audit["snapshot"];
    // A11：两笔期初 + 三个转账修订 + 两个本机支出修订 + 一个远端候选。
    assert_eq!(audit["sequence"], 8);
    assert_eq!(audit["revisions"], 8);
    assert_eq!(audit["operations"].as_array().unwrap().len(), 7);
    assert_eq!(snapshot["activeEventIds"].as_array().unwrap().len(), 3);
    assert_eq!(
        snapshot["selectedRevisionIds"].as_object().unwrap().len(),
        4
    );
    assert_eq!(snapshot["deletedEventIds"].as_array().unwrap().len(), 1);
    assert_eq!(
        snapshot["unresolvedCandidateRevisionIds"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(snapshot["netAssetsMinor"], "18000");
    let mut balances: Vec<_> = snapshot["balancesMinor"]
        .as_object()
        .unwrap()
        .values()
        .map(|n| n.as_str().unwrap())
        .collect();
    balances.sort();
    assert_eq!(balances, ["10000", "8000"]);
}

fn assert_preserved(before: &Value, after: &Value) {
    assert_nonempty(after);
    for field in [
        "vault",
        "device",
        "ready",
        "fixtureComplete",
        "audit",
        "contentDigest",
    ] {
        assert_eq!(before[field], after[field], "{field}");
    }
    assert_ne!(before["pid"], after["pid"]);
}

fn hashes(path: &Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_str().unwrap().to_owned(),
                crypto::fingerprint(&std::fs::read(entry.path()).unwrap()),
            )
        })
        .collect()
}
fn kill_at(directory: &Path, run: &str, mut child: Child, point: &str) {
    let marker = directory.join(format!("m0c-{run}/paused.json"));
    let deadline = Instant::now() + Duration::from_secs(35);
    loop {
        if let Ok(bytes) = std::fs::read(&marker)
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            && value["pid"] == child.id()
            && value["point"] == point
        {
            break;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("worker exited before fault marker: {status}");
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("fault marker timeout");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
}

#[test]
fn local_book_reopens_receipts_transactions_and_balances_across_processes() {
    let dir = tempfile::tempdir().unwrap();
    let directory = dir.path();
    let run = "00000000000000000000000000000001";
    let password = b"synthetic local ledger process test";
    let (_, recovery) = ok(call(directory, run, json!({"kind":"newRecovery"}), &[]));
    let mut create_input = Zeroizing::new(password.to_vec());
    create_input.extend_from_slice(&recovery);
    let (created, _) = ok(call(
        directory,
        run,
        json!({"kind":"create"}),
        &create_input,
    ));
    let identity = json!([created["vault"], created["device"]]);
    let process = |action, pause| {
        spawn_request(
            Path::new(env!("CARGO_BIN_EXE_m0-bootstrap-worker")),
            json!({"abi":1,"directory":directory,"run":run,"action":action,
                   "pause_at":pause,"expected_identity":identity,"include_ledger":true}),
            password,
        )
    };
    let save = |command, pause| {
        process(
            json!({"kind":"saveLocal","unlock":"password","command":command}),
            pause,
        )
    };
    let reopen = || {
        ok(finish(process(
            json!({"kind":"inspect","unlock":"password"}),
            None,
        )))
        .0
    };
    let id = |n: usize| format!("{n:032x}");
    let write_identity = |n| json!({"operationId":id(n),"eventId":id(n+1),"revisionId":id(n+2)});
    // A01 的银行期初和收入，另在同一账户录入 30 元支出以核对借贷方向。
    let account = json!({"kind":"account","id":id(1),"accountKind":"bank","name":"合成银行",
        "amountYuan":"1000.00","businessDate":"2026-01-01","identity":write_identity(10)});
    let income_category =
        json!({"kind":"category","id":id(2),"categoryKind":"income","name":"合成工资"});
    let expense_category =
        json!({"kind":"category","id":id(3),"categoryKind":"expense","name":"合成餐饮"});
    let income = json!({"kind":"entry","direction":"income","amountYuan":"2000.00",
        "businessDate":"2026-01-05","accountId":id(1),"categoryId":id(2),"identity":write_identity(20)});
    let expense = json!({"kind":"entry","direction":"expense","amountYuan":"30.00",
        "businessDate":"2026-01-07","accountId":id(1),"categoryId":id(3),"identity":write_identity(30)});

    assert_eq!(
        finish(save(account.clone(), None)).0["error"]["code"],
        "recovery_unconfirmed"
    );
    ok(call(directory, run, json!({"kind":"confirm"}), &recovery));
    let mut bad_opening = account.clone();
    bad_opening["businessDate"] = json!("2026-02-29");
    assert_eq!(
        finish(save(bad_opening, None)).0["error"]["code"],
        "invalid_calendar_date"
    );
    assert_eq!(reopen()["book"]["accounts"], json!([]));

    let pending = save(account.clone(), Some("localSavePrepared"));
    let pending_pid = pending.id();
    kill_at(directory, run, pending, "localSavePrepared");
    let rolled_back = reopen();
    assert_ne!(rolled_back["pid"], pending_pid);
    assert_eq!(rolled_back["lastSaveCommitted"], false);
    assert_eq!(rolled_back["book"]["accounts"], json!([]));
    let (opened, _) = ok(finish(save(account.clone(), None)));
    let (replayed, _) = ok(finish(save(account, None)));
    assert_eq!(opened["book"], replayed["book"]);
    assert_eq!(opened["book"]["balancesMinor"][id(1)], "100000");
    assert_eq!(opened["book"]["recentEntries"], json!([]));

    for category in [income_category, expense_category] {
        let (registered, _) = ok(finish(save(category.clone(), None)));
        assert_eq!(
            registered["book"],
            ok(finish(save(category, None))).0["book"]
        );
    }
    let mut wrong_category = income.clone();
    wrong_category["categoryId"] = json!(id(3));
    assert_eq!(
        finish(save(wrong_category, None)).0["error"]["code"],
        "category_type_mismatch"
    );
    let (recorded, _) = ok(finish(save(income, None)));
    assert_eq!(recorded["book"]["balancesMinor"][id(1)], "300000");

    let pending = save(expense.clone(), Some("localSaveCommitted"));
    let pending_pid = pending.id();
    kill_at(directory, run, pending, "localSaveCommitted");
    let reopened = reopen();
    assert_ne!(reopened["pid"], pending_pid);
    assert_eq!(reopened["lastSaveCommitted"], true);
    assert_eq!(reopened["book"]["balancesMinor"][id(1)], "297000");
    let rows = reopened["book"]["recentEntries"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    for (row, event, kind, amount, date, category) in [
        (&rows[0], id(31), "expense", "3000", "2026-01-07", id(3)),
        (&rows[1], id(21), "income", "200000", "2026-01-05", id(2)),
    ] {
        assert_eq!(row["id"], event);
        assert_eq!(row["kind"], kind);
        assert_eq!(row["amountMinor"], amount);
        assert_eq!(row["businessDate"], date);
        assert_eq!(row["accountId"], id(1));
        assert_eq!(row["categoryId"], category);
    }
    assert_eq!(ok(finish(save(expense, None))).0["book"], reopened["book"]);
    let (audit, _) = ok(call(
        directory,
        run,
        json!({"kind":"inspect","unlock":"password"}),
        password,
    ));
    assert_eq!(audit["audit"]["sequence"], 3);
    assert_eq!(audit["audit"]["operations"].as_array().unwrap().len(), 3);
    let edit = json!({"kind":"reviseEntry","amountYuan":"25.00","businessDate":"2026-02-01",
        "previousRevisionId":rows[0]["revisionId"],
        "identity":{"eventId":rows[0]["id"],"operationId":id(40),"revisionId":id(41)}});
    kill_at(
        directory,
        run,
        save(edit.clone(), Some("localSavePrepared")),
        "localSavePrepared",
    );
    let uncommitted = reopen();
    assert_eq!(uncommitted["lastSaveCommitted"], false);
    assert_eq!(uncommitted["book"], reopened["book"]);
    kill_at(
        directory,
        run,
        save(edit.clone(), Some("localSaveCommitted")),
        "localSaveCommitted",
    );
    let committed = reopen();
    assert_eq!(committed["lastSaveCommitted"], true);
    assert_eq!(committed["book"]["balancesMinor"][id(1)], "297500");
    let revised = committed["book"]["recentEntries"].as_array().unwrap();
    assert_eq!(revised.len(), 2);
    assert_eq!(revised[0]["id"], rows[0]["id"]);
    assert_eq!(revised[0]["revisionId"], id(41));
    assert_eq!(revised[0]["businessDate"], "2026-02-01");
    assert_eq!(revised[1], rows[1]);
    assert_eq!(ok(finish(save(edit, None))).0["book"], committed["book"]);
    let delete = json!({"kind":"deleteEntry","previousRevisionId":id(41),
        "identity":{"eventId":id(31),"operationId":id(50),"revisionId":id(51)}});
    kill_at(
        directory,
        run,
        save(delete.clone(), Some("localSavePrepared")),
        "localSavePrepared",
    );
    let pending_delete = reopen();
    assert_eq!(pending_delete["lastSaveCommitted"], false);
    assert_eq!(pending_delete["book"], committed["book"]);
    kill_at(
        directory,
        run,
        save(delete.clone(), Some("localSaveCommitted")),
        "localSaveCommitted",
    );
    let deleted = reopen();
    assert_eq!(deleted["lastSaveCommitted"], true);
    assert_eq!(deleted["book"]["balancesMinor"][id(1)], "300000");
    assert_eq!(deleted["book"]["recentEntries"], json!([rows[1]]));
    assert_eq!(ok(finish(save(delete, None))).0["book"], deleted["book"]);
    let (audit, _) = ok(call(
        directory,
        run,
        json!({"kind":"inspect","unlock":"password"}),
        password,
    ));
    let snapshot = &audit["audit"]["snapshot"];
    assert_eq!(snapshot["deletedEventIds"], json!([id(31)]));
    assert_eq!(snapshot["selectedRevisionIds"][id(31)], id(51));
    assert_eq!(audit["audit"]["revisions"], 5);
    assert_eq!(
        audit["audit"]["operations"],
        json!([id(10), id(20), id(30), id(40), id(50)])
    );
    let (status, _) = ok(call(directory, run, json!({"kind":"status"}), &[]));
    assert_eq!(status["candidates"], json!([]));
}

#[test]
#[ignore = "需显式提供升级前 SQLCipher 4.14.0 的独立 worker，日常检查不冒充跨版本验证"]
fn previous_sqlcipher_preserves_closed_and_interrupted_vaults() {
    previous_native_preserves_vaults("M0_PREVIOUS_BOOTSTRAP_WORKER", "4.14.0");
}

#[test]
#[ignore = "需非 Apple 目标及独立保存的 SQLCipher 4.19.0 / OpenSSL 3.6.3 worker"]
fn previous_openssl_preserves_closed_and_interrupted_vaults() {
    if cfg!(target_vendor = "apple") {
        panic!("CommonCrypto 目标不能冒充 OpenSSL 迁移验证");
    }
    previous_native_preserves_vaults("M0_PREVIOUS_OPENSSL_WORKER", "4.19.0");
}

fn previous_native_preserves_vaults(worker_variable: &str, previous_cipher: &str) {
    let previous_worker = std::path::PathBuf::from(
        std::env::var_os(worker_variable).expect("必须提供升级前构建并单独保存的 worker"),
    );
    assert!(previous_worker.is_absolute() && previous_worker.is_file());
    assert_ne!(
        previous_worker.canonicalize().unwrap(),
        Path::new(env!("CARGO_BIN_EXE_m0-bootstrap-worker"))
            .canonicalize()
            .unwrap()
    );
    let previous = |directory: &Path, run: &str, action: Value, secret: &[u8]| {
        finish(start_request_with_worker(
            &previous_worker,
            directory,
            run,
            action,
            None,
            None,
            secret,
        ))
    };
    for interrupted in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let reference = tempfile::tempdir().unwrap();
        let run = radishledger_core::random_id().unwrap();
        let (_, recovery) = ok(previous(
            directory.path(),
            &run,
            json!({"kind":"newRecovery"}),
            &[],
        ));
        let password = Zeroizing::new(crypto::hex(crypto::random::<32>().unwrap().as_ref()));
        let mut input = Zeroizing::new(password.as_bytes().to_vec());
        input.extend_from_slice(&recovery);
        let (created, _) = ok(previous(
            directory.path(),
            &run,
            json!({"kind":"create"}),
            &input,
        ));
        assert!(
            created["cipherVersion"]
                .as_str()
                .unwrap()
                .starts_with(previous_cipher)
        );
        ok(previous(
            directory.path(),
            &run,
            json!({"kind":"confirm"}),
            &recovery,
        ));
        let path = directory.path().join(format!("m0c-{run}"));
        let baseline = if interrupted {
            let child = start_request_with_worker(
                &previous_worker,
                directory.path(),
                &run,
                json!({"kind":"seed","unlock":"recovery"}),
                Some("fixtureSaved"),
                None,
                &recovery,
            );
            kill_at(directory.path(), &run, child, "fixtureSaved");
            assert!(std::fs::metadata(path.join("ledger.db-wal")).unwrap().len() > 32);
            let original = hashes(&path);
            // 只让旧版读取副本取得独立完整事实；原待恢复 DB/WAL 留给新版首次打开。
            let copy = reference.path().join(format!("m0c-{run}"));
            std::fs::create_dir(&copy).unwrap();
            for entry in std::fs::read_dir(&path).unwrap() {
                let entry = entry.unwrap();
                assert!(entry.file_type().unwrap().is_file());
                std::fs::copy(entry.path(), copy.join(entry.file_name())).unwrap();
            }
            let (report, _) = ok(previous(
                reference.path(),
                &run,
                json!({"kind":"inspect","unlock":"recovery"}),
                &recovery,
            ));
            assert_eq!(hashes(&path), original);
            ok(previous(
                reference.path(),
                &run,
                json!({"kind":"cleanup"}),
                &[],
            ));
            report
        } else {
            let (report, _) = ok(previous(
                directory.path(),
                &run,
                json!({"kind":"seed","unlock":"recovery"}),
                &recovery,
            ));
            assert!(
                !path.join("ledger.db-wal").exists()
                    || std::fs::metadata(path.join("ledger.db-wal")).unwrap().len() == 0
            );
            report
        };
        assert_nonempty(&baseline);
        assert!(
            baseline["cipherVersion"]
                .as_str()
                .unwrap()
                .starts_with(previous_cipher)
        );
        let original = hashes(&path);
        let wrong_recovery =
            crypto::recovery_text(crypto::random::<32>().unwrap().as_ref()).unwrap();
        for (unlock, secret) in [
            ("password", b"wrong synthetic password".as_slice()),
            ("recovery", wrong_recovery.as_bytes()),
        ] {
            let rejected = call(
                directory.path(),
                &run,
                json!({"kind":"inspect","unlock":unlock}),
                secret,
            );
            assert_eq!(rejected.0["error"]["code"], "authentication_failed");
            assert_eq!(hashes(&path), original);
        }
        for (kind, unlock, secret) in [
            ("inspect", "recovery", recovery.as_slice()),
            ("inspect", "password", password.as_bytes()),
            ("replay", "recovery", recovery.as_slice()),
            ("inspect", "recovery", recovery.as_slice()),
        ] {
            let (after, _) = ok(call(
                directory.path(),
                &run,
                json!({"kind":kind,"unlock":unlock}),
                secret,
            ));
            assert_preserved(&baseline, &after);
            assert!(
                after["cipherVersion"]
                    .as_str()
                    .unwrap()
                    .starts_with("4.19.0")
            );
        }
        ok(call(directory.path(), &run, json!({"kind":"cleanup"}), &[]));
        assert_eq!(directory.path().read_dir().unwrap().count(), 0);
        assert_eq!(reference.path().read_dir().unwrap().count(), 0);
        eprintln!(
            "{worker_variable}: SQLCipher {previous_cipher} -> 4.19.0; interrupted={interrupted}; A11 facts, wrong-material preservation, replay and cleanup passed"
        );
    }
}

#[test]
fn real_process_bootstrap_interruptions_and_independent_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let recovery = crypto::random::<32>().unwrap();
    let recovery = crypto::recovery_text(recovery.as_ref()).unwrap();
    let password = Zeroizing::new(crypto::hex(crypto::random::<32>().unwrap().as_ref()));
    let mut input = Zeroizing::new(password.as_bytes().to_vec());
    input.extend_from_slice(recovery.as_bytes());
    for point in [
        "envelopesSaved",
        "databaseStarted",
        "databaseReady",
        "readyPublished",
        "protectionPrepared",
        "protectionPublished",
    ] {
        let run = radishledger_core::random_id().unwrap();
        let (empty, _) = ok(call(directory.path(), &run, json!({"kind":"locate"}), &[]));
        assert_eq!(empty, json!({"state":"empty"}));
        assert!(!directory.path().join(format!("m0c-{run}")).exists());
        if ["envelopesSaved", "databaseStarted", "databaseReady"].contains(&point) {
            kill_at(
                directory.path(),
                &run,
                start(
                    directory.path(),
                    &run,
                    json!({"kind":"create"}),
                    Some(point),
                    &input,
                ),
                point,
            );
            let located = call(directory.path(), &run, json!({"kind":"locate"}), &[]);
            let resumed = call(
                directory.path(),
                &run,
                json!({"kind":"resume","unlock":"recovery"}),
                recovery.as_bytes(),
            );
            if point == "databaseStarted" {
                assert_eq!(located.0["error"]["code"], "missing_database");
                assert_eq!(resumed.0["error"]["code"], "missing_database");
                assert!(
                    !directory
                        .path()
                        .join(format!("m0c-{run}/ledger.db"))
                        .exists()
                );
                ok(call(directory.path(), &run, json!({"kind":"cleanup"}), &[]));
                continue;
            }
            let (located, _) = ok(located);
            assert_eq!(
                located["state"],
                if point == "envelopesSaved" {
                    "initializing"
                } else {
                    "registered"
                }
            );
            let (resumed, _) = ok(resumed);
            assert_eq!(located["vault"], resumed["vault"]);
            assert_eq!(located["device"], resumed["device"]);
        } else {
            ok(call(
                directory.path(),
                &run,
                json!({"kind":"create"}),
                &input,
            ));
        }
        if point == "readyPublished" {
            kill_at(
                directory.path(),
                &run,
                start(
                    directory.path(),
                    &run,
                    json!({"kind":"confirm"}),
                    Some(point),
                    recovery.as_bytes(),
                ),
                point,
            );
        }
        ok(call(
            directory.path(),
            &run,
            json!({"kind":"confirm"}),
            recovery.as_bytes(),
        ));
        let (before, _) = ok(call(
            directory.path(),
            &run,
            json!({"kind":"seed","unlock":"recovery"}),
            recovery.as_bytes(),
        ));
        assert_nonempty(&before);
        if point == "protectionPrepared" {
            kill_at(
                directory.path(),
                &run,
                start(
                    directory.path(),
                    &run,
                    json!({"kind":"prepareProtection","unlock":"recovery"}),
                    Some(point),
                    recovery.as_bytes(),
                ),
                point,
            );
            let (status, _) = ok(call(directory.path(), &run, json!({"kind":"status"}), &[]));
            assert!(status["activeProtection"].is_null());
        }
        let (candidate, local) = ok(call(
            directory.path(),
            &run,
            json!({"kind":"prepareProtection","unlock":"recovery"}),
            recovery.as_bytes(),
        ));
        let generation = candidate["candidateProtection"].as_str().unwrap();
        if point == "protectionPublished" {
            kill_at(
                directory.path(),
                &run,
                start(
                    directory.path(),
                    &run,
                    json!({"kind":"commitProtection","generation":generation}),
                    Some(point),
                    &local,
                ),
                point,
            );
        }
        ok(call(
            directory.path(),
            &run,
            json!({"kind":"commitProtection","generation":generation}),
            &local,
        ));
        for (unlock, secret) in [
            ("password", password.as_bytes()),
            ("recovery", recovery.as_bytes()),
            ("system", local.as_slice()),
        ] {
            let (after, _) = ok(call(
                directory.path(),
                &run,
                json!({"kind":"inspect","unlock":unlock}),
                secret,
            ));
            assert_eq!(after["contentDigest"], before["contentDigest"]);
            assert_eq!(after["audit"], before["audit"]);
            assert_eq!(after["vault"], before["vault"]);
            assert_ne!(after["pid"], before["pid"]);
        }
        ok(call(directory.path(), &run, json!({"kind":"cleanup"}), &[]));
    }
}

#[test]
fn nonempty_wal_survives_kill_wrong_material_and_original_intent_replay() {
    let directory = tempfile::tempdir().unwrap();
    let run = radishledger_core::random_id().unwrap();
    let recovery = crypto::recovery_text(crypto::random::<32>().unwrap().as_ref()).unwrap();
    let password = Zeroizing::new(crypto::hex(crypto::random::<32>().unwrap().as_ref()));
    let mut input = Zeroizing::new(password.as_bytes().to_vec());
    input.extend_from_slice(recovery.as_bytes());
    ok(call(
        directory.path(),
        &run,
        json!({"kind":"create"}),
        &input,
    ));
    ok(call(
        directory.path(),
        &run,
        json!({"kind":"confirm"}),
        recovery.as_bytes(),
    ));
    let child = start(
        directory.path(),
        &run,
        json!({"kind":"seed","unlock":"recovery"}),
        Some("fixtureSaved"),
        recovery.as_bytes(),
    );
    let killed_pid = child.id();
    kill_at(directory.path(), &run, child, "fixtureSaved");
    let path = directory.path().join(format!("m0c-{run}"));
    assert!(std::fs::metadata(path.join("ledger.db-wal")).unwrap().len() > 32);
    let original_files = hashes(&path);
    let (located, _) = ok(call(directory.path(), &run, json!({"kind":"locate"}), &[]));
    assert_eq!(located["state"], "registered");
    assert_eq!(hashes(&path), original_files);
    for (unlock, secret) in [
        (
            "password",
            Zeroizing::new(b"wrong synthetic password".to_vec()),
        ),
        (
            "recovery",
            Zeroizing::new(
                crypto::recovery_text(crypto::random::<32>().unwrap().as_ref())
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
            ),
        ),
    ] {
        let response = call(
            directory.path(),
            &run,
            json!({"kind":"inspect","unlock":unlock}),
            &secret,
        );
        assert_eq!(response.0["error"]["code"], "authentication_failed");
        assert_eq!(hashes(&path), original_files);
    }
    let (before, _) = ok(call(
        directory.path(),
        &run,
        json!({"kind":"inspect","unlock":"recovery"}),
        recovery.as_bytes(),
    ));
    assert_nonempty(&before);
    assert_eq!(located["vault"], before["vault"]);
    assert_eq!(located["device"], before["device"]);
    assert_ne!(before["pid"], killed_pid);
    let (password_opened, _) = ok(call(
        directory.path(),
        &run,
        json!({"kind":"inspect","unlock":"password"}),
        password.as_bytes(),
    ));
    assert_preserved(&before, &password_opened);
    let (replayed, _) = ok(call(
        directory.path(),
        &run,
        json!({"kind":"replay","unlock":"recovery"}),
        recovery.as_bytes(),
    ));
    assert_preserved(&before, &replayed);
    // 正确材料也不能把损坏封装当空账库；错误读取不改写任何现存文件。
    let bootstrap_path = path.join("bootstrap.json");
    let bootstrap = std::fs::read(&bootstrap_path).unwrap();
    std::fs::write(&bootstrap_path, b"{").unwrap();
    let corrupted_files = hashes(&path);
    let located = call(directory.path(), &run, json!({"kind":"locate"}), &[]);
    assert_eq!(located.0["error"]["code"], "invalid_bootstrap");
    assert_eq!(hashes(&path), corrupted_files);
    let rejected = call(
        directory.path(),
        &run,
        json!({"kind":"inspect","unlock":"recovery"}),
        recovery.as_bytes(),
    );
    assert_eq!(rejected.0["error"]["code"], "invalid_bootstrap");
    assert_eq!(hashes(&path), corrupted_files);
    std::fs::write(bootstrap_path, bootstrap).unwrap();
    let (recovered, _) = ok(call(
        directory.path(),
        &run,
        json!({"kind":"inspect","unlock":"recovery"}),
        recovery.as_bytes(),
    ));
    assert_preserved(&before, &recovered);
    ok(call(directory.path(), &run, json!({"kind":"cleanup"}), &[]));
    assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

#[test]
fn publication_faults_preserve_nonempty_vault_and_recover_in_new_process() {
    let directory = tempfile::tempdir().unwrap();
    let run = radishledger_core::random_id().unwrap();
    let recovery = crypto::recovery_text(crypto::random::<32>().unwrap().as_ref()).unwrap();
    let password = Zeroizing::new(crypto::hex(crypto::random::<32>().unwrap().as_ref()));
    let mut input = Zeroizing::new(password.as_bytes().to_vec());
    input.extend_from_slice(recovery.as_bytes());
    ok(call(
        directory.path(),
        &run,
        json!({"kind":"create"}),
        &input,
    ));
    ok(call(
        directory.path(),
        &run,
        json!({"kind":"confirm"}),
        recovery.as_bytes(),
    ));
    let (baseline, _) = ok(call(
        directory.path(),
        &run,
        json!({"kind":"seed","unlock":"recovery"}),
        recovery.as_bytes(),
    ));
    assert_nonempty(&baseline);
    let path = directory.path().join(format!("m0c-{run}"));
    let (initial, initial_key) = ok(call(
        directory.path(),
        &run,
        json!({"kind":"prepareProtection","unlock":"recovery"}),
        recovery.as_bytes(),
    ));
    ok(call(
        directory.path(),
        &run,
        json!({"kind":"commitProtection","generation":initial["candidateProtection"]}),
        &initial_key,
    ));
    for file in ["localProtection", "activeProtection"] {
        for stage in [
            "shortWrite",
            "writeZero",
            "writeError",
            "fileSync",
            "publish",
            "directorySync",
        ] {
            let (candidate, local) = ok(call(
                directory.path(),
                &run,
                json!({"kind":"prepareProtection","unlock":"recovery"}),
                recovery.as_bytes(),
            ));
            let action = if file == "localProtection" {
                json!({"kind":"prepareProtection","unlock":"recovery"})
            } else {
                json!({"kind":"commitProtection","generation":candidate["candidateProtection"]})
            };
            let secret = if file == "localProtection" {
                recovery.as_bytes()
            } else {
                local.as_slice()
            };
            let before_files = hashes(&path);
            let response = finish(start_request(
                directory.path(),
                &run,
                action,
                None,
                Some(json!({"file":file,"stage":stage})),
                secret,
            ));
            if stage == "shortWrite" {
                let (written, key) = ok(response);
                if file == "localProtection" {
                    ok(call(
                        directory.path(),
                        &run,
                        json!({"kind":"commitProtection","generation":written["candidateProtection"]}),
                        &key,
                    ));
                    let (opened, _) = ok(call(
                        directory.path(),
                        &run,
                        json!({"kind":"inspect","unlock":"system"}),
                        &key,
                    ));
                    assert_preserved(&baseline, &opened);
                }
            } else {
                assert_eq!(response.0["ok"], false, "{file}/{stage}");
                let code = if stage == "directorySync" {
                    "bootstrap_publish_uncertain"
                } else {
                    "bootstrap_io"
                };
                assert_eq!(response.0["error"]["code"], code, "{file}/{stage}");
                if stage != "directorySync" {
                    assert_eq!(hashes(&path), before_files, "{file}/{stage}");
                }
            }
            let (after, _) = ok(call(
                directory.path(),
                &run,
                json!({"kind":"inspect","unlock":"recovery"}),
                recovery.as_bytes(),
            ));
            assert_preserved(&baseline, &after);
            if file == "activeProtection" && matches!(stage, "shortWrite" | "directorySync") {
                assert_eq!(after["activeProtection"], candidate["candidateProtection"]);
                let (system, _) = ok(call(
                    directory.path(),
                    &run,
                    json!({"kind":"inspect","unlock":"system"}),
                    &local,
                ));
                assert_preserved(&baseline, &system);
            }
            // 无论失败位于发布前还是发布后，都可从独立材料重试并切换到可读的新保护。
            ok(call(
                directory.path(),
                &run,
                json!({"kind":"commitProtection","generation":candidate["candidateProtection"]}),
                &local,
            ));
            let (reopened, _) = ok(call(
                directory.path(),
                &run,
                json!({"kind":"inspect","unlock":"system"}),
                &local,
            ));
            assert_preserved(&baseline, &reopened);
        }
    }
    ok(call(directory.path(), &run, json!({"kind":"cleanup"}), &[]));
    assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

#[test]
fn incomplete_bootstrap_and_confirmation_never_report_false_success() {
    let directory = tempfile::tempdir().unwrap();
    let recovery = crypto::recovery_text(crypto::random::<32>().unwrap().as_ref()).unwrap();
    let password = Zeroizing::new(crypto::hex(crypto::random::<32>().unwrap().as_ref()));
    let mut input = Zeroizing::new(password.as_bytes().to_vec());
    input.extend_from_slice(recovery.as_bytes());
    for file in ["bootstrap", "ready"] {
        for stage in [
            "shortWrite",
            "writeZero",
            "writeError",
            "fileSync",
            "publish",
            "directorySync",
        ] {
            let run = radishledger_core::random_id().unwrap();
            let path = directory.path().join(format!("m0c-{run}"));
            if file == "ready" {
                ok(call(
                    directory.path(),
                    &run,
                    json!({"kind":"create"}),
                    &input,
                ));
            }
            let action = if file == "bootstrap" {
                json!({"kind":"create"})
            } else {
                json!({"kind":"confirm"})
            };
            let secret = if file == "bootstrap" {
                input.as_slice()
            } else {
                recovery.as_bytes()
            };
            let response = finish(start_request(
                directory.path(),
                &run,
                action,
                None,
                Some(json!({"file":file,"stage":stage})),
                secret,
            ));
            if stage == "shortWrite" {
                ok(response);
            } else {
                assert_eq!(response.0["ok"], false);
                assert_eq!(
                    response.0["error"]["code"],
                    if stage == "directorySync" {
                        "bootstrap_publish_uncertain"
                    } else {
                        "bootstrap_io"
                    }
                );
            }
            if file == "bootstrap" && !matches!(stage, "shortWrite" | "directorySync") {
                assert!(!path.join("bootstrap.json").exists());
                assert!(!path.join("ledger.db").exists());
                let files = hashes(&path);
                let resume = call(
                    directory.path(),
                    &run,
                    json!({"kind":"resume","unlock":"recovery"}),
                    recovery.as_bytes(),
                );
                assert_eq!(resume.0["ok"], false);
                assert_eq!(hashes(&path), files);
                let recreate = call(directory.path(), &run, json!({"kind":"create"}), &input);
                assert_eq!(recreate.0["error"]["code"], "already_exists");
                assert_eq!(hashes(&path), files);
            } else {
                if file == "bootstrap" && stage == "directorySync" {
                    assert!(!path.join("ledger.db").exists());
                    ok(call(
                        directory.path(),
                        &run,
                        json!({"kind":"resume","unlock":"recovery"}),
                        recovery.as_bytes(),
                    ));
                }
                if file == "ready" {
                    let (inspected, _) = ok(call(
                        directory.path(),
                        &run,
                        json!({"kind":"inspect","unlock":"recovery"}),
                        recovery.as_bytes(),
                    ));
                    assert_eq!(
                        inspected["ready"],
                        matches!(stage, "shortWrite" | "directorySync")
                    );
                    assert_eq!(inspected["audit"]["sequence"], 0);
                }
                ok(call(
                    directory.path(),
                    &run,
                    json!({"kind":"confirm"}),
                    recovery.as_bytes(),
                ));
                let (seeded, _) = ok(call(
                    directory.path(),
                    &run,
                    json!({"kind":"seed","unlock":"recovery"}),
                    recovery.as_bytes(),
                ));
                assert_nonempty(&seeded);
            }
            ok(call(directory.path(), &run, json!({"kind":"cleanup"}), &[]));
        }
    }
    assert_eq!(directory.path().read_dir().unwrap().count(), 0);
}

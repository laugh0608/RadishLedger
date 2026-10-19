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
    let metadata = serde_json::to_vec(
        &json!({"abi":1,"directory":directory,"run":run,"action":action,"pause_at":pause,"file_fault":fault}),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_m0-bootstrap-worker"))
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
            let resumed = call(
                directory.path(),
                &run,
                json!({"kind":"resume","unlock":"recovery"}),
                recovery.as_bytes(),
            );
            if point == "databaseStarted" {
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
            ok(resumed);
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

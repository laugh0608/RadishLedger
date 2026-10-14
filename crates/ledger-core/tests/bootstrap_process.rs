use radishledger_core::crypto;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

fn start(directory: &Path, run: &str, action: Value, pause: Option<&str>, secret: &[u8]) -> Child {
    let metadata = serde_json::to_vec(
        &json!({"abi":1,"directory":directory,"run":run,"action":action,"pause_at":pause}),
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

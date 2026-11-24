use crate::bootstrap::{self, Reply, Request};
use crate::crypto::{self, Secret};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use zeroize::Zeroizing;

fn command(directory: &Path, run: &str, action: Value, input: &[u8]) -> crate::Result<Reply> {
    let request: Request =
        serde_json::from_value(json!({"abi":1,"directory":directory,"run":run,"action":action}))
            .unwrap();
    bootstrap::run(request, input)
}
fn hashes(path: &Path) -> BTreeMap<String, String> {
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
fn create_input(password: &[u8], recovery: &Secret) -> Zeroizing<Vec<u8>> {
    let mut input = Zeroizing::new(password.to_vec());
    input.extend_from_slice(crypto::recovery_text(recovery.as_ref()).unwrap().as_bytes());
    input
}

#[test]
fn bootstrap_preserves_identity_and_unsynced_facts_through_recovery() {
    let _test_guard = crypto::KDF_TESTS.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let run = crate::random_id().unwrap();
    let path = directory.path().join(format!("m0c-{run}"));
    let password = Zeroizing::new(crypto::hex(crypto::random::<32>().unwrap().as_ref()));
    let recovery = crypto::random::<32>().unwrap();
    let mut recovery_text = crypto::recovery_text(recovery.as_ref()).unwrap();
    let call = |action, input: &[u8]| command(directory.path(), &run, action, input);
    let created = call(
        json!({"kind":"create"}),
        &create_input(password.as_bytes(), &recovery),
    )
    .unwrap();
    assert_eq!(created.report["ready"], false);
    assert_eq!(created.report["audit"]["sequence"], 0);
    assert_eq!(
        call(
            json!({"kind":"seed","unlock":"recovery"}),
            recovery_text.as_bytes()
        )
        .err()
        .unwrap()
        .code,
        "recovery_unconfirmed"
    );
    let before = hashes(&path);
    assert_eq!(
        call(
            json!({"kind":"inspect","unlock":"password"}),
            b"wrong test material"
        )
        .err()
        .unwrap()
        .code,
        "authentication_failed"
    );
    assert_eq!(
        call(json!({"kind":"confirm"}), b"wrong")
            .err()
            .unwrap()
            .code,
        "invalid_recovery"
    );
    assert_eq!(hashes(&path), before);
    assert_eq!(
        call(
            json!({"kind":"create"}),
            &create_input(password.as_bytes(), &recovery)
        )
        .err()
        .unwrap()
        .code,
        "already_exists"
    );
    assert_eq!(hashes(&path), before);
    let reissued = call(json!({"kind":"reissueRecovery"}), password.as_bytes()).unwrap();
    assert!(
        call(
            json!({"kind":"inspect","unlock":"recovery"}),
            recovery_text.as_bytes()
        )
        .is_err()
    );
    recovery_text = Zeroizing::new(std::str::from_utf8(&reissued.secret).unwrap().to_owned());
    assert_eq!(
        call(json!({"kind":"confirm"}), recovery_text.as_bytes())
            .unwrap()
            .report["ready"],
        true
    );
    assert_eq!(
        call(json!({"kind":"reissueRecovery"}), password.as_bytes())
            .err()
            .unwrap()
            .code,
        "already_ready"
    );
    let seeded = call(
        json!({"kind":"seed","unlock":"recovery"}),
        recovery_text.as_bytes(),
    )
    .unwrap()
    .report;
    assert_eq!(seeded["fixtureComplete"], true);
    assert!(seeded["audit"]["operations"].as_array().unwrap().len() > 5);
    for action in [
        json!({"kind":"seed","unlock":"recovery"}),
        json!({"kind":"inspect","unlock":"recovery"}),
    ] {
        assert_eq!(
            call(action, recovery_text.as_bytes()).unwrap().report,
            seeded
        );
    }
    assert_eq!(
        call(
            json!({"kind":"inspect","unlock":"password"}),
            password.as_bytes()
        )
        .unwrap()
        .report,
        seeded
    );

    let candidate = call(
        json!({"kind":"prepareProtection","unlock":"recovery"}),
        recovery_text.as_bytes(),
    )
    .unwrap();
    let generation = candidate.report["candidateProtection"].as_str().unwrap();
    let before = hashes(&path);
    assert_eq!(
        call(
            json!({"kind":"commitProtection","generation":generation}),
            &[0; 32]
        )
        .err()
        .unwrap()
        .code,
        "authentication_failed"
    );
    assert_eq!(hashes(&path), before);
    assert!(call(json!({"kind":"status"}), &[]).unwrap().report["activeProtection"].is_null());
    call(
        json!({"kind":"commitProtection","generation":generation}),
        &candidate.secret,
    )
    .unwrap();
    let opened = call(
        json!({"kind":"inspect","unlock":"system"}),
        &candidate.secret,
    )
    .unwrap()
    .report;
    assert_eq!(opened["contentDigest"], seeded["contentDigest"]);
    assert_eq!(opened["audit"], seeded["audit"]);
    // 内核层模拟日常材料不可取；真实平台项缺失由移动实验另验。
    assert!(call(json!({"kind":"inspect","unlock":"system"}), &[]).is_err());
    let recovered = call(
        json!({"kind":"inspect","unlock":"recovery"}),
        recovery_text.as_bytes(),
    )
    .unwrap()
    .report;
    assert_eq!(recovered["audit"], seeded["audit"]);
    let replacement = call(
        json!({"kind":"prepareProtection","unlock":"recovery"}),
        recovery_text.as_bytes(),
    )
    .unwrap();
    let replacement_id = replacement.report["candidateProtection"].as_str().unwrap();
    let committed = call(
        json!({"kind":"commitProtection","generation":replacement_id}),
        &replacement.secret,
    )
    .unwrap()
    .report;
    assert_eq!(committed["previousProtection"], generation);
    assert_eq!(committed["audit"], seeded["audit"]);
    assert!(
        call(
            json!({"kind":"inspect","unlock":"system"}),
            &candidate.secret
        )
        .is_err()
    );
    assert_eq!(
        call(
            json!({"kind":"inspect","unlock":"system"}),
            &replacement.secret
        )
        .unwrap()
        .report["audit"],
        seeded["audit"]
    );

    let original = std::fs::read(path.join("bootstrap.json")).unwrap();
    let parsed: Value = serde_json::from_slice(&original).unwrap();
    let mut mutations = Vec::new();
    for field in ["vault", "device"] {
        let mut x = parsed.clone();
        x[field] = json!(crate::random_id().unwrap());
        mutations.push(x);
    }
    for field in ["recovery", "database"] {
        for child in ["nonce", "ciphertext"] {
            let mut x = parsed.clone();
            x[field][child][0] = json!(x[field][child][0].as_u64().unwrap() ^ 1);
            mutations.push(x);
        }
    }
    let mut x = parsed.clone();
    x["salt"][0] = json!(x["salt"][0].as_u64().unwrap() ^ 1);
    mutations.push(x);
    let mut x = parsed.clone();
    x["profile"]["memory_kib"] = json!(u32::MAX);
    mutations.push(x);
    let mut x = parsed.clone();
    x["format"] = json!(2);
    mutations.push(x);
    let mut x = parsed.clone();
    x["unexpected"] = json!(true);
    mutations.push(x);
    let mut x = parsed.clone();
    x["recovery"]["ciphertext"] = json!([]);
    mutations.push(x);
    for mutation in mutations {
        std::fs::write(
            path.join("bootstrap.json"),
            serde_json::to_vec(&mutation).unwrap(),
        )
        .unwrap();
        let before = hashes(&path);
        assert!(
            call(
                json!({"kind":"inspect","unlock":"recovery"}),
                recovery_text.as_bytes()
            )
            .is_err()
        );
        assert_eq!(hashes(&path), before);
    }
    for malformed in [b"{".to_vec(), vec![0; 16385]] {
        std::fs::write(path.join("bootstrap.json"), malformed).unwrap();
        let before = hashes(&path);
        assert!(
            call(
                json!({"kind":"inspect","unlock":"recovery"}),
                recovery_text.as_bytes()
            )
            .is_err()
        );
        assert_eq!(hashes(&path), before);
    }
    std::fs::write(path.join("bootstrap.json"), &original).unwrap();
    // 错误文件类型不能通过读取或恢复去覆盖。
    let ready = std::fs::read(path.join("ready.json")).unwrap();
    std::fs::remove_file(path.join("ready.json")).unwrap();
    std::fs::create_dir(path.join("ready.json")).unwrap();
    assert_eq!(
        call(json!({"kind":"confirm"}), recovery_text.as_bytes())
            .err()
            .unwrap()
            .code,
        "invalid_file"
    );
    std::fs::remove_dir(path.join("ready.json")).unwrap();
    std::fs::write(path.join("ready.json"), ready).unwrap();
    assert_eq!(
        call(
            json!({"kind":"inspect","unlock":"recovery"}),
            recovery_text.as_bytes()
        )
        .unwrap()
        .report["audit"],
        seeded["audit"]
    );
    call(json!({"kind":"cleanup"}), &[]).unwrap();
    assert!(!path.exists());
}

#[test]
fn archive_captures_authenticated_history_and_requires_the_written_bytes() {
    use crate::archive_container::{self, CANDIDATE_LIMITS};
    use std::sync::atomic::AtomicBool;

    let _test_guard = crypto::KDF_TESTS.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let run = crate::random_id().unwrap();
    let local = "synthetic local";
    let pass = b"synthetic archive";
    let recovery = crypto::random::<32>().unwrap();
    let created = command(
        base.path(),
        &run,
        json!({"kind":"create"}),
        &create_input(local.as_bytes(), &recovery),
    )
    .unwrap();
    let identity = json!([created.report["vault"], created.report["device"]]);
    let call = |action: Value, secret: &[u8]| {
        let request = serde_json::from_value(json!({"abi":1,"directory":base.path(),"run":run,"action":action,"expected_identity":identity})).unwrap();
        bootstrap::run(request, secret)
    };
    let material = crypto::recovery_text(recovery.as_ref()).unwrap();
    call(json!({"kind":"confirm"}), material.as_bytes()).unwrap();
    let seeded = call(
        json!({"kind":"seed","unlock":"recovery"}),
        material.as_bytes(),
    )
    .unwrap();
    let stage = "archive-host01";
    let dir = base.path().join(stage);
    std::fs::create_dir(&dir).unwrap();
    let original = hashes(&base.path().join(format!("m0c-{run}")));
    let prepare = json!({"kind":"prepareArchive","unlock":"password","stage":stage,"archive_bytes":pass.len()});
    let combined = Zeroizing::new([local.as_bytes(), pass].concat());
    let sealed = call(prepare.clone(), &combined).unwrap();
    assert!(call(prepare, &combined).is_err());
    let bytes = std::fs::read(dir.join("sealed")).unwrap();
    let archive =
        archive_container::open(&bytes, pass, CANDIDATE_LIMITS, &AtomicBool::new(false)).unwrap();
    let facts = &archive.contents().facts;
    assert_eq!(json!(facts.sequence), seeded.report["audit"]["sequence"]);
    assert!(!facts.candidates.is_empty() && facts.revisions.iter().any(|r| r.event.is_none()));
    assert!(facts.revisions.len() > facts.selected.len());
    assert_eq!(
        facts.intent_history.len(),
        seeded.report["audit"]["operations"]
            .as_array()
            .unwrap()
            .len()
    );
    let verify = json!({"kind":"verifyArchive","stage":stage,"digest":sealed.report["digest"]});
    std::fs::write(dir.join("readback"), &bytes).unwrap();
    assert_eq!(
        call(verify.clone(), local.as_bytes()).err().unwrap().code,
        "archive_unlock_failed"
    );
    assert_eq!(call(verify.clone(), pass).unwrap().report["verified"], true);
    std::fs::write(dir.join("readback"), &bytes[..bytes.len() - 1]).unwrap();
    assert_eq!(
        call(verify, pass).err().unwrap().code,
        "archive_file_mismatch"
    );
    assert_eq!(std::fs::read(dir.join("sealed")).unwrap(), bytes);
    let destination = tempfile::tempdir().unwrap();
    let stage = "archive-resume01";
    let target = destination.path().join(stage);
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("source.rla"), &bytes).unwrap();
    let restore = |action, input: &[u8]| command(destination.path(), &run, action, input);
    let prepare = json!({"kind":"prepareRestore","stage":stage});
    assert_eq!(
        restore(prepare.clone(), local.as_bytes())
            .err()
            .unwrap()
            .code,
        "archive_unlock_failed"
    );
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 1);
    let restored = restore(prepare.clone(), pass).unwrap();
    let report = &restored.report;
    assert_eq!(
        (
            report["activeEvents"].as_u64(),
            report["deleted"].as_u64(),
            report["candidates"].as_u64()
        ),
        (Some(3), Some(1), Some(2))
    );
    assert_eq!(report["netAssetsMinor"], "18000");
    let mut balances: Vec<_> = report["balances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["minor"].as_str().unwrap())
        .collect();
    balances.sort();
    assert_eq!(balances, ["10000", "8000"]);
    assert_eq!(report["revisions"], 8);
    assert_eq!(report["intents"], 7);
    assert_eq!(
        restore(prepare, pass).err().unwrap().code,
        "restore_target_not_empty"
    );
    let inspect = json!({"kind":"inspectRestore","stage":stage,"ticket":report["ticket"]});
    let credentials = Zeroizing::new([&restored.secret[..], pass].concat());
    assert_eq!(
        restore(inspect.clone(), &credentials).unwrap().report,
        *report
    );
    let discard = json!({"kind":"discardRestore","stage":stage,"ticket":report["ticket"]});
    assert!(restore(discard.clone(), &[0; 32]).is_err());
    std::fs::write(target.join("source.rla"), &bytes[..bytes.len() - 1]).unwrap();
    assert_eq!(
        restore(inspect, &credentials).err().unwrap().code,
        "restore_mismatch"
    );
    assert_eq!(
        restore(discard, &restored.secret).unwrap().report["discarded"],
        true
    );
    assert_eq!(std::fs::read_dir(target).unwrap().count(), 1);
    assert_eq!(
        restore(json!({"kind":"locate"}), &[]).unwrap().report["state"],
        "empty"
    );
    assert_eq!(hashes(&base.path().join(format!("m0c-{run}"))), original);
}

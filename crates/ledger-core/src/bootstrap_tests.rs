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

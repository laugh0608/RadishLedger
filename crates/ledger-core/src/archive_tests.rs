use super::*;
use crate::logical_snapshot;
use crate::store::{DbKey, Store};
use serde_json::Value;

const MATERIAL: &str =
    include_str!("../../../docs/validation/archive-container-review-inputs.json");
const PASSWORD: &[u8] = b"public-review-password";
fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}
fn fixture() -> Value {
    serde_json::from_str(MATERIAL).unwrap()
}
fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect()
}
fn oracle() -> Vec<u8> {
    unhex(fixture()["cryptoExpected"]["archiveHex"].as_str().unwrap())
}
fn entropy() -> Entropy {
    let material = fixture();
    let base = &material["baseInput"];
    Entropy {
        id: unhex(base["archiveIdHex"].as_str().unwrap())
            .try_into()
            .unwrap(),
        salt: unhex(base["saltHex"].as_str().unwrap()).try_into().unwrap(),
        wrap_nonce: unhex(base["wrapNonceHex"].as_str().unwrap())
            .try_into()
            .unwrap(),
        body_nonce: unhex(base["contentNonceHex"].as_str().unwrap())
            .try_into()
            .unwrap(),
        key: crypto::secret(&unhex(base["archiveKeyHex"].as_str().unwrap())).unwrap(),
    }
}
fn blank() -> (tempfile::TempDir, DbKey, Store, LogicalSnapshot) {
    let directory = tempfile::tempdir().unwrap();
    let key = DbKey::random().unwrap();
    let store = Store::create(&directory.path().join("source.db"), &key).unwrap();
    let snapshot = store
        .logical_snapshot(
            logical_snapshot::Limits {
                rows: 100_000,
                bytes: MAX_PAYLOAD,
            },
            &[],
            &no_cancel(),
        )
        .unwrap();
    (directory, key, store, snapshot)
}

#[test]
fn independent_go_vector_matches_keys_full_ciphertext_and_m0c_compatibility() {
    let _guard = crypto::KDF_TESTS.lock().unwrap();
    let material = fixture();
    let expected = &material["cryptoExpected"];
    let entropy = entropy();
    let raw = crypto::password_material(PASSWORD, &entropy.salt, &Profile::default(), |_| Ok(()))
        .unwrap();
    assert_eq!(crypto::hex(raw.as_ref()), expected["argon2idOutputHex"]);
    assert_eq!(
        crypto::hex(
            crypto::derive(raw.as_ref(), &entropy.id, WRAP_INFO)
                .unwrap()
                .as_ref()
        ),
        expected["wrapKeyHex"]
    );
    assert_eq!(
        crypto::hex(
            crypto::derive(entropy.key.as_ref(), &entropy.id, BODY_INFO)
                .unwrap()
                .as_ref()
        ),
        expected["contentKeyHex"]
    );
    assert_eq!(
        crypto::hex(
            crypto::password_key(PASSWORD, &entropy.salt, &Profile::default())
                .unwrap()
                .as_ref()
        ),
        expected["m0cPasswordKeyHex"]
    );
    let (_dir, _key, _store, snapshot) = blank();
    assert_eq!(
        snapshot.encode(MAX_PAYLOAD, &no_cancel()).unwrap(),
        material["baseInput"]["payloadUtf8"]
            .as_str()
            .unwrap()
            .as_bytes()
    );
    let sealed = seal_with(
        &snapshot,
        PASSWORD,
        CANDIDATE_LIMITS,
        &no_cancel(),
        &entropy,
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(sealed.as_bytes(), oracle());
    assert_eq!(
        crypto::fingerprint(sealed.as_bytes()),
        expected["archiveSha256"]
    );
    assert_eq!(
        crypto::hex(&sealed.as_bytes()[128..HEADER]),
        expected["wrappedArchiveKeyHex"]
    );
    assert_eq!(
        crypto::hex(&sealed.as_bytes()[HEADER..]),
        expected["contentCiphertextHex"]
    );
    let opened = open(&oracle(), PASSWORD, CANDIDATE_LIMITS, &no_cancel()).unwrap();
    assert_eq!(opened.archive_id(), &entropy.id);
    assert_eq!(
        serde_json::to_value(opened.contents()).unwrap(),
        serde_json::to_value(snapshot.contents()).unwrap()
    );
}

#[test]
fn unsupported_header_length_flags_profile_and_input_size_are_rejected_before_kdf() {
    let _guard = crypto::KDF_TESTS.lock().unwrap();
    // 占住真实名额，任何意外进入 KDF 的路径都会得到 kdf_busy 并使断言失败。
    crypto::password_material(PASSWORD, &entropy().salt, &Profile::default(), |point| {
        if point != KdfPoint::Acquired {
            return Ok(());
        }
        let original = oracle();
        let mut mutations = Vec::new();
        for (at, value, code) in [
            (0, 0_u8, "invalid_archive"),
            (9, 1, "unsupported_archive"),
            (11, 2, "unsupported_archive"),
            (17, 2, "unsupported_archive"),
            (19, 2, "unsupported_kdf"),
            (23, 1, "unsupported_archive"),
            (59, 20, "unsupported_kdf"),
            (63, 1, "unsupported_kdf"),
            (67, 4, "unsupported_kdf"),
            (71, 4, "unsupported_kdf"),
        ] {
            let mut data = original.clone();
            data[at] = value;
            mutations.push((data, code));
        }
        for value in [0, u32::MAX] {
            let mut data = original.clone();
            data[12..16].copy_from_slice(&value.to_be_bytes());
            mutations.push((data, "invalid_archive"));
        }
        for (at, values) in [
            (56, [0, 18, 20, u32::MAX]),
            (60, [0, 65535, 65537, u32::MAX]),
            (64, [0, 2, 4, u32::MAX]),
            (68, [0, 2, 4, u32::MAX]),
        ] {
            for value in values {
                let mut data = original.clone();
                data[at..at + 4].copy_from_slice(&value.to_be_bytes());
                mutations.push((data, "unsupported_kdf"));
            }
        }
        for value in [0, 153 - 1, (MAX_PAYLOAD + 153) as u64, u64::MAX] {
            let mut data = original.clone();
            data[120..128].copy_from_slice(&value.to_be_bytes());
            mutations.push((data, "archive_limit"));
        }
        for len in [0, 1, 127, 175, 176, 328, original.len() - 1] {
            mutations.push((original[..len].to_vec(), "invalid_archive"));
        }
        let mut extra = original.clone();
        extra.push(0);
        mutations.push((extra, "invalid_archive"));
        for (input, expected) in mutations {
            assert_eq!(
                open(&input, PASSWORD, CANDIDATE_LIMITS, &no_cancel())
                    .err()
                    .unwrap()
                    .code,
                expected
            );
        }
        assert_eq!(
            open(&original, PASSWORD, CANDIDATE_LIMITS, &no_cancel())
                .err()
                .unwrap()
                .code,
            "kdf_busy"
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn wrong_password_wrap_body_and_cross_archive_splice_have_distinct_failures() {
    let _guard = crypto::KDF_TESTS.lock().unwrap();
    let original = oracle();
    assert_eq!(
        open(
            &original,
            b"wrong-public-review-password",
            CANDIDATE_LIMITS,
            &no_cancel()
        )
        .err()
        .unwrap()
        .code,
        "archive_unlock_failed"
    );
    for at in [24, 40, 72, 96, 128, 175] {
        let mut data = original.clone();
        data[at] ^= 1;
        assert_eq!(
            open(&data, PASSWORD, CANDIDATE_LIMITS, &no_cancel())
                .err()
                .unwrap()
                .code,
            "archive_unlock_failed"
        );
    }
    for at in [HEADER, original.len() - 17, original.len() - 1] {
        let mut data = original.clone();
        data[at] ^= 1;
        assert_eq!(
            open(&data, PASSWORD, CANDIDATE_LIMITS, &no_cancel())
                .err()
                .unwrap()
                .code,
            "archive_content_auth_failed"
        );
    }
    let (_dir, _key, _store, snapshot) = blank();
    let second = seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &no_cancel()).unwrap();
    assert_eq!(second.as_bytes().len(), original.len());
    let mut splice = original.clone();
    splice[HEADER..].copy_from_slice(&second.as_bytes()[HEADER..]);
    assert_eq!(
        open(&splice, PASSWORD, CANDIDATE_LIMITS, &no_cancel())
            .err()
            .unwrap()
            .code,
        "archive_content_auth_failed"
    );
    assert_eq!(
        open(&original, PASSWORD, CANDIDATE_LIMITS, &no_cancel())
            .unwrap()
            .contents()
            .facts
            .revisions
            .len(),
        0
    );
}

#[test]
fn body_cipher_authentication_rejects_every_changed_byte_without_exposing_cleartext() {
    let material = fixture();
    let key = crypto::secret(&unhex(
        material["cryptoExpected"]["contentKeyHex"]
            .as_str()
            .unwrap(),
    ))
    .unwrap();
    let data = oracle();
    let e = entropy();
    for at in HEADER..data.len() {
        let mut body = copy(&data[HEADER..], 0).unwrap();
        body[at - HEADER] ^= 1;
        assert_eq!(
            decrypt(
                &key,
                &e.body_nonce,
                &mut body,
                &aad(BODY_AAD, &data[..HEADER]).unwrap(),
                content_error()
            )
            .unwrap_err()
            .code,
            "archive_content_auth_failed"
        );
    }
}

#[test]
fn retry_uses_new_random_archive_material_and_does_not_mutate_source_sqlcipher() {
    let _guard = crypto::KDF_TESTS.lock().unwrap();
    let (_dir, _key, store, snapshot) = blank();
    let before = store.content_digest().unwrap();
    let one = seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &no_cancel()).unwrap();
    let two = seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &no_cancel()).unwrap();
    for range in [24..40, 40..56, 72..96, 96..120, 128..176] {
        assert_ne!(&one.as_bytes()[range.clone()], &two.as_bytes()[range]);
    }
    assert_eq!(
        serde_json::to_value(
            open(one.as_bytes(), PASSWORD, CANDIDATE_LIMITS, &no_cancel())
                .unwrap()
                .contents()
        )
        .unwrap(),
        serde_json::to_value(snapshot.contents()).unwrap()
    );
    assert_eq!(store.content_digest().unwrap(), before);
}

#[test]
fn raw_utf8_password_identity_matches_independent_nfc_and_nfd_vectors() {
    let _guard = crypto::KDF_TESTS.lock().unwrap();
    let material = fixture();
    let e = entropy();
    for (password, field) in [
        ("\u{e9}", "unicodeNfcArgon2Hex"),
        ("e\u{301}", "unicodeNfdArgon2Hex"),
    ] {
        let raw = crypto::password_material(
            password.as_bytes(),
            &e.salt,
            &Profile::default(),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(crypto::hex(raw.as_ref()), material["cryptoExpected"][field]);
    }
    assert_ne!(
        material["cryptoExpected"]["unicodeNfcArgon2Hex"],
        material["cryptoExpected"]["unicodeNfdArgon2Hex"]
    );
    let (_dir, _key, _store, snapshot) = blank();
    for password in [Vec::new(), vec![b'x'; 1025], vec![0xff]] {
        assert_eq!(
            seal(&snapshot, &password, CANDIDATE_LIMITS, &no_cancel())
                .err()
                .unwrap()
                .code,
            "invalid_password"
        );
        assert_eq!(
            open(&oracle(), &password, CANDIDATE_LIMITS, &no_cancel())
                .err()
                .unwrap()
                .code,
            "invalid_password"
        );
    }
}

#[test]
fn cancellation_failure_and_shared_gate_keep_original_source_and_allow_retry() {
    let _guard = crypto::KDF_TESTS.lock().unwrap();
    let (_dir, _key, store, snapshot) = blank();
    let before = store.content_digest().unwrap();
    for point in [
        Point::BeforeKdf,
        Point::KdfAcquired,
        Point::KdfHashed,
        Point::AfterWrap,
        Point::AfterContent,
        Point::AfterJson,
        Point::Complete,
    ] {
        let cancel = no_cancel();
        assert_eq!(
            open_observed(&oracle(), PASSWORD, CANDIDATE_LIMITS, &cancel, |observed| {
                if observed == point {
                    cancel.store(true, Ordering::Relaxed);
                }
                if matches!(observed, Point::KdfAcquired | Point::KdfHashed) {
                    assert_eq!(
                        crypto::password_key(PASSWORD, &entropy().salt, &Profile::default())
                            .err()
                            .unwrap()
                            .code,
                        "kdf_busy"
                    );
                }
                Ok(())
            })
            .err()
            .unwrap()
            .code,
            "cancelled"
        );
        assert_eq!(
            open_observed(
                &oracle(),
                PASSWORD,
                CANDIDATE_LIMITS,
                &no_cancel(),
                |observed| if observed == point {
                    Err(CoreError::new("injected_failure", "合成容器阶段失败"))
                } else {
                    Ok(())
                }
            )
            .err()
            .unwrap()
            .code,
            "injected_failure"
        );
        assert_eq!(store.content_digest().unwrap(), before);
    }
    for point in [
        Point::BeforeKdf,
        Point::KdfAcquired,
        Point::KdfHashed,
        Point::AfterWrap,
        Point::AfterContent,
        Point::Complete,
    ] {
        let cancel = no_cancel();
        assert_eq!(
            seal_with(
                &snapshot,
                PASSWORD,
                CANDIDATE_LIMITS,
                &cancel,
                &entropy(),
                |p| {
                    if p == point {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    Ok(())
                }
            )
            .err()
            .unwrap()
            .code,
            "cancelled"
        );
    }
    assert!(open(&oracle(), PASSWORD, CANDIDATE_LIMITS, &no_cancel()).is_ok());
    assert_eq!(store.content_digest().unwrap(), before);
}

#[path = "archive_payload_tests.rs"]
mod payload;

//! 认证后的非法载荷与真实来源保全，和未认证篡改分开构造。
use super::*;
use crate::csv_import::{
    Decision, prepare,
    test_support::{HEADER as CSV_HEADER, ONE, id, mapping},
};

fn forged(facts: &Facts, payload: &[u8], mutate: impl FnOnce(&mut [u8])) -> Vec<u8> {
    let mut e = Entropy::random().unwrap();
    e.salt = entropy().salt;
    let ikm = crypto::secret(&unhex(
        fixture()["cryptoExpected"]["argon2idOutputHex"]
            .as_str()
            .unwrap(),
    ))
    .unwrap();
    let h = Header::new(payload.len(), &e);
    let mut body = buffer(payload.len() + BODY_OVERHEAD + 16).unwrap();
    body.extend_from_slice(&(MANIFEST as u32).to_be_bytes());
    body.extend_from_slice(&manifest(facts, payload, CANDIDATE_LIMITS).unwrap());
    body.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    body.extend_from_slice(payload);
    body.extend_from_slice(b"RLM0END1");
    mutate(&mut body);
    let wrap_key = crypto::derive(ikm.as_ref(), &e.id, WRAP_INFO).unwrap();
    let mut wrapped = copy(e.key.as_ref(), 16).unwrap();
    encrypt(
        &wrap_key,
        &e.wrap_nonce,
        &mut wrapped,
        &aad(WRAP_AAD, &h.prefix).unwrap(),
    )
    .unwrap();
    let mut out = copy(&h.prefix, wrapped.len() + body.len() + 16).unwrap();
    out.extend_from_slice(&wrapped);
    let body_key = crypto::derive(e.key.as_ref(), &e.id, BODY_INFO).unwrap();
    encrypt(
        &body_key,
        &e.body_nonce,
        &mut body,
        &aad(BODY_AAD, &out).unwrap(),
    )
    .unwrap();
    out.extend_from_slice(&body);
    out.to_vec()
}

#[test]
fn authenticated_bad_manifest_terminator_json_and_summary_are_rejected_after_aead() {
    let _guard = crypto::KDF_TESTS.lock().unwrap();
    let (_dir, _key, _store, snapshot) = blank();
    let baseline = snapshot.encode(MAX_PAYLOAD, &no_cancel()).unwrap();
    for mutation in 0..6 {
        let input = forged(
            &snapshot.contents().facts,
            &baseline,
            |body| match mutation {
                0 => body[4 + 84] ^= 1,
                1 => {
                    let last = body.len() - 1;
                    body[last] ^= 1;
                }
                2 => body[28..32].copy_from_slice(&1_u32.to_be_bytes()),
                3 => body[..4].copy_from_slice(&115_u32.to_be_bytes()),
                4 => body[120..128].copy_from_slice(&1_u64.to_be_bytes()),
                5 => body[14..16].copy_from_slice(&2_u16.to_be_bytes()),
                _ => unreachable!(),
            },
        );
        let mut authenticated = false;
        assert_eq!(
            open_observed(&input, PASSWORD, CANDIDATE_LIMITS, &no_cancel(), |p| {
                if p == Point::AfterContent {
                    authenticated = true;
                }
                Ok(())
            })
            .err()
            .unwrap()
            .code,
            "invalid_archive_payload"
        );
        assert!(authenticated);
    }
    let mut values = Vec::new();
    let original: Value = serde_json::from_slice(&baseline).unwrap();
    for change in 0..6 {
        let mut value = original.clone();
        match change {
            0 => value["verification"]["netAssetsMinor"] = Value::String("1".into()),
            1 => value["facts"]["unexpected"] = Value::Bool(true),
            2 => value["facts"]["profile"] = Value::String("future-profile".into()),
            3 => value["facts"]["remoteKnowledge"] = Value::String("confirmed".into()),
            4 => value["facts"]["sourceSchema"] = Value::from(3),
            5 => value["verification"]["unexpected"] = Value::Null,
            _ => unreachable!(),
        }
        values.push(serde_json::to_vec(&value).unwrap());
    }
    let text = std::str::from_utf8(&baseline).unwrap();
    values.push(
        text.replacen(
            "\"sourceSchema\":1",
            "\"sourceSchema\":1,\"sourceSchema\":1",
            1,
        )
        .into_bytes(),
    );
    values.push(
        text.replacen(
            "\"balancesMinor\":{}",
            "\"balancesMinor\":{\"x\":\"0\",\"\\u0078\":\"0\"}",
            1,
        )
        .into_bytes(),
    );
    let mut bom = vec![0xef, 0xbb, 0xbf];
    bom.extend_from_slice(&baseline);
    values.push(bom);
    values.push([baseline.as_slice(), b" {}"].concat());
    values.push(vec![0xff]);
    for payload in values {
        let input = forged(&snapshot.contents().facts, &payload, |_| {});
        let mut authenticated = false;
        assert_eq!(
            open_observed(&input, PASSWORD, CANDIDATE_LIMITS, &no_cancel(), |p| {
                if p == Point::AfterContent {
                    authenticated = true;
                }
                Ok(())
            })
            .err()
            .unwrap()
            .code,
            "invalid_archive_payload"
        );
        assert!(authenticated);
    }
}

#[test]
fn json_budgets_include_nested_records_keys_decoded_text_depth_and_members() {
    for max in [1, 2, 3] {
        let l = Limits {
            records: max,
            ..CANDIDATE_LIMITS
        };
        let result = json::scan(br#"{"facts":{"accounts":[{},{}]}}"#, l, &no_cancel());
        assert_eq!(result.is_ok(), max >= 2);
        if max < 2 {
            assert_eq!(result.unwrap_err().code, "archive_limit");
        }
    }
    for n in [99_999, 100_000, 100_001] {
        let text = "{\"facts\":{\"accounts\":[".to_owned() + &"{},".repeat(n - 1) + "{}]}}";
        assert_eq!(
            json::scan(text.as_bytes(), CANDIDATE_LIMITS, &no_cancel()).is_ok(),
            n <= 100_000
        );
    }
    for depth in [1, 2, 3] {
        let text = "[".repeat(depth) + "0" + &"]".repeat(depth);
        let result = json::scan(
            text.as_bytes(),
            Limits {
                json_depth: 2,
                ..CANDIDATE_LIMITS
            },
            &no_cancel(),
        );
        assert_eq!(result.is_ok(), depth <= 2);
    }
    for n in [63, 64, 65] {
        let text = "[".repeat(n) + "0" + &"]".repeat(n);
        assert_eq!(
            json::scan(text.as_bytes(), CANDIDATE_LIMITS, &no_cancel()).is_ok(),
            n <= 64
        );
    }
    for n in [65_535, 65_536, 65_537] {
        let text = "\"".to_owned() + &"a".repeat(n) + "\"";
        assert_eq!(
            json::scan(text.as_bytes(), CANDIDATE_LIMITS, &no_cancel()).is_ok(),
            n <= 65_536
        );
        let escaped = "\"".to_owned() + &"\\u0061".repeat(n) + "\"";
        assert_eq!(
            json::scan(escaped.as_bytes(), CANDIDATE_LIMITS, &no_cancel()).is_ok(),
            n <= 65_536
        );
    }
    for max in [4, 5, 6] {
        let result = json::scan(
            br#"["aa","bbb"]"#,
            Limits {
                decoded_string_bytes: max,
                ..CANDIDATE_LIMITS
            },
            &no_cancel(),
        );
        assert_eq!(result.is_ok(), max >= 5);
    }
    for n in [999_999, 1_000_000, 1_000_001] {
        let text = "[".to_owned() + &"0,".repeat(n - 1) + "0]";
        let result = json::scan(text.as_bytes(), CANDIDATE_LIMITS, &no_cancel());
        assert_eq!(result.is_ok(), n <= 1_000_000);
    }
    assert_eq!(
        json::scan(b"{}", CANDIDATE_LIMITS, &AtomicBool::new(true))
            .unwrap_err()
            .code,
        "cancelled"
    );
}

#[test]
fn authenticated_payload_limit_accepts_both_edges_and_rejects_one_more_before_kdf() {
    let _guard = crypto::KDF_TESTS.lock().unwrap();
    let (_dir, _key, _store, snapshot) = blank();
    let base = snapshot.encode(MAX_PAYLOAD, &no_cancel()).unwrap();
    for length in [MAX_PAYLOAD - 1, MAX_PAYLOAD] {
        let mut payload = base.clone();
        payload.resize(length, b' ');
        let input = forged(&snapshot.contents().facts, &payload, |_| {});
        let output = open(&input, PASSWORD, CANDIDATE_LIMITS, &no_cancel()).unwrap();
        assert_eq!(output.contents().facts.sequence, 0);
        assert_eq!(input.len(), length + OVERHEAD);
    }
    let mut large = oracle();
    large.resize(MAX_PAYLOAD + OVERHEAD + 1, 0);
    assert_eq!(
        open(&large, PASSWORD, CANDIDATE_LIMITS, &no_cancel())
            .err()
            .unwrap()
            .code,
        "archive_limit"
    );
    for size in [517, 518, 519] {
        let result = open(
            &oracle(),
            PASSWORD,
            Limits {
                payload_bytes: size,
                ..CANDIDATE_LIMITS
            },
            &no_cancel(),
        );
        assert_eq!(result.is_ok(), size >= 518);
    }
    for l in [
        Limits {
            payload_bytes: 0,
            ..CANDIDATE_LIMITS
        },
        Limits {
            payload_bytes: MAX_PAYLOAD + 1,
            ..CANDIDATE_LIMITS
        },
        Limits {
            records: 100_001,
            ..CANDIDATE_LIMITS
        },
        Limits {
            json_depth: 65,
            ..CANDIDATE_LIMITS
        },
        Limits {
            string_bytes: 65_537,
            ..CANDIDATE_LIMITS
        },
        Limits {
            decoded_string_bytes: MAX_PAYLOAD + 1,
            ..CANDIDATE_LIMITS
        },
        Limits {
            collection_entries: 1_000_001,
            ..CANDIDATE_LIMITS
        },
    ] {
        assert_eq!(
            open(&oracle(), PASSWORD, l, &no_cancel())
                .err()
                .unwrap()
                .code,
            "archive_limit"
        );
        assert_eq!(
            seal(&snapshot, PASSWORD, l, &no_cancel())
                .err()
                .unwrap()
                .code,
            "archive_limit"
        );
    }
}

#[test]
fn a11_sqlcipher_facts_are_preserved_and_open_needs_no_original_database_key() {
    let _guard = crypto::KDF_TESTS.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let key = DbKey::random().unwrap();
    let mut store = Store::create(&directory.path().join("source.db"), &key).unwrap();
    crate::probe::populate_snapshot_fixture(&mut store, "A11", "conflict").unwrap();
    let snapshot = store
        .logical_snapshot(
            logical_snapshot::Limits {
                rows: 100_000,
                bytes: MAX_PAYLOAD,
            },
            &["2026-01".into()],
            &no_cancel(),
        )
        .unwrap();
    let before = store.content_digest().unwrap();
    let input = seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &no_cancel()).unwrap();
    let opened = open(input.as_bytes(), PASSWORD, CANDIDATE_LIMITS, &no_cancel()).unwrap();
    assert_eq!(
        serde_json::to_value(opened.contents()).unwrap(),
        serde_json::to_value(snapshot.contents()).unwrap()
    );
    let facts = &opened.contents().facts;
    assert_eq!(
        (
            facts.revisions.len(),
            facts.intent_history.len(),
            facts.candidates.len()
        ),
        (8, 7, 2)
    );
    assert_eq!(opened.contents().verification.deleted_event_ids.len(), 1);
    for change in 0..2 {
        let mut invalid = serde_json::to_value(snapshot.contents()).unwrap();
        if change == 0 {
            let revision = invalid["facts"]["revisions"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|r| r["predecessor"].is_string())
                .unwrap();
            revision["predecessor"] = Value::String(id(99999));
        } else {
            let revision = invalid["facts"]["revisions"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|r| r["event"]["accountId"].is_string())
                .unwrap();
            revision["event"]["accountId"] = Value::String(id(99999));
        }
        let malformed = forged(
            &snapshot.contents().facts,
            &serde_json::to_vec(&invalid).unwrap(),
            |_| {},
        );
        assert_eq!(
            open(&malformed, PASSWORD, CANDIDATE_LIMITS, &no_cancel())
                .err()
                .unwrap()
                .code,
            "invalid_archive_payload"
        );
    }
    let total = counts(facts, CANDIDATE_LIMITS)
        .unwrap()
        .iter()
        .map(|n| *n as usize)
        .sum::<usize>();
    assert_eq!(total, 24);
    assert_eq!(
        seal(
            &snapshot,
            PASSWORD,
            Limits {
                records: 23,
                ..CANDIDATE_LIMITS
            },
            &no_cancel()
        )
        .err()
        .unwrap()
        .code,
        "archive_limit"
    );
    assert!(
        json::scan(
            &snapshot.encode(MAX_PAYLOAD, &no_cancel()).unwrap(),
            Limits {
                records: 24,
                ..CANDIDATE_LIMITS
            },
            &no_cancel()
        )
        .is_ok()
    );
    assert!(
        json::scan(
            &snapshot.encode(MAX_PAYLOAD, &no_cancel()).unwrap(),
            Limits {
                records: 25,
                ..CANDIDATE_LIMITS
            },
            &no_cancel()
        )
        .is_ok()
    );
    assert_eq!(
        open(
            input.as_bytes(),
            b"wrong-public-password",
            CANDIDATE_LIMITS,
            &no_cancel()
        )
        .err()
        .unwrap()
        .code,
        "archive_unlock_failed"
    );
    assert_eq!(store.content_digest().unwrap(), before);
    assert_eq!(
        store.commit(&facts.intent_history[0]).unwrap(),
        crate::store::WriteEffect::Replayed
    );
    assert_eq!(store.content_digest().unwrap(), before);
    drop(store);
    drop(key);
    let independent = open(input.as_bytes(), PASSWORD, CANDIDATE_LIMITS, &no_cancel()).unwrap();
    assert_eq!(independent.contents().facts.revisions.len(), 8);
}

#[test]
fn v2_import_history_is_accepted_without_upgrading_or_inventing_undo_state() {
    let _guard = crypto::KDF_TESTS.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let key = DbKey::random().unwrap();
    let mut store =
        Store::create_csv_experiment(&directory.path().join("source.db"), &key).unwrap();
    store.add_account(&id(1), "cash", "合成现金").unwrap();
    store.add_category(&id(5), "expense", "合成餐饮").unwrap();
    let plan = prepare(
        &store,
        (CSV_HEADER.to_owned() + ONE).as_bytes(),
        &mapping(),
        &no_cancel(),
    )
    .unwrap();
    let task = store
        .save_csv_task(&id(900), &plan, &[Decision::Import], 1, &no_cancel())
        .unwrap();
    store
        .publish_csv_batch(&task.target_id, &task.id, 1, &no_cancel())
        .unwrap();
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
    let before = store.content_digest().unwrap();
    let archive = seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &no_cancel()).unwrap();
    let opened = open(archive.as_bytes(), PASSWORD, CANDIDATE_LIMITS, &no_cancel()).unwrap();
    assert_eq!(opened.contents().facts.source_schema, 2);
    let imports = opened.contents().facts.imports.as_ref().unwrap();
    assert_eq!(
        (
            imports.rows.len(),
            imports.sources.len(),
            imports.batches.len()
        ),
        (1, 1, 1)
    );
    assert!(
        imports.undo_tasks.is_empty()
            && imports.undo_rows.is_empty()
            && imports.undo_batches.is_empty()
    );
    assert_eq!(store.content_digest().unwrap(), before);
}

#[test]
fn v3_cancelled_import_and_partial_undo_roundtrip_only_as_inert_history() {
    let _guard = crypto::KDF_TESTS.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let key = DbKey::random().unwrap();
    let mut store =
        Store::create_csv_undo_experiment(&directory.path().join("source.db"), &key).unwrap();
    for (n, kind) in [(1, "cash"), (2, "bank"), (3, "credit_card")] {
        store.add_account(&id(n), kind, "合成账户").unwrap();
    }
    for (n, kind) in [(4, "income"), (5, "expense"), (6, "expense")] {
        store.add_category(&id(n), kind, "合成分类").unwrap();
    }
    let mut text = CSV_HEADER.to_owned();
    for n in 1..=4 {
        text.push_str(&ONE.replace("src-1", &format!("src-{n}")));
    }
    let prepared = prepare(&store, text.as_bytes(), &mapping(), &no_cancel()).unwrap();
    let task = store
        .save_csv_task(
            &id(900),
            &prepared,
            &[
                Decision::Import,
                Decision::Import,
                Decision::Skip,
                Decision::Import,
            ],
            2,
            &no_cancel(),
        )
        .unwrap();
    store
        .publish_csv_batch(&task.target_id, &task.id, 1, &no_cancel())
        .unwrap();
    store
        .set_csv_cancelled(&task.target_id, &task.id, true)
        .unwrap();
    let preview = store
        .preview_csv_undo(&task.target_id, &task.id, &no_cancel())
        .unwrap();
    store
        .save_csv_undo(&id(800), &preview, &[1, 2], 1, &no_cancel())
        .unwrap();
    store
        .publish_csv_undo_batch(&task.target_id, &id(800), 1, &no_cancel())
        .unwrap();
    store
        .set_csv_undo_cancelled(&task.target_id, &id(800), true)
        .unwrap();
    let snapshot = store
        .logical_snapshot(
            logical_snapshot::Limits {
                rows: 100_000,
                bytes: MAX_PAYLOAD,
            },
            &["2026-01".into()],
            &no_cancel(),
        )
        .unwrap();
    let before = store.content_digest().unwrap();
    let archive = seal(&snapshot, PASSWORD, CANDIDATE_LIMITS, &no_cancel()).unwrap();
    let opened = open(archive.as_bytes(), PASSWORD, CANDIDATE_LIMITS, &no_cancel()).unwrap();
    assert_eq!(
        serde_json::to_value(opened.contents()).unwrap(),
        serde_json::to_value(snapshot.contents()).unwrap()
    );
    let history = opened.contents().facts.imports.as_ref().unwrap();
    assert_eq!(
        (
            history.tasks.len(),
            history.rows.len(),
            history.sources.len(),
            history.batches.len()
        ),
        (1, 4, 2, 1)
    );
    assert_eq!(
        (
            history.undo_tasks.len(),
            history.undo_rows.len(),
            history.undo_batches.len()
        ),
        (1, 2, 1)
    );
    assert_eq!(
        history.tasks[0].state,
        crate::csv_import::TaskState::Cancelled
    );
    assert_eq!(
        history.undo_tasks[0].state,
        crate::csv_undo::State::Cancelled
    );
    assert_eq!(
        (
            history.tasks[0].next_record,
            history.undo_tasks[0].next_ordinal
        ),
        (3, 2)
    );
    assert_eq!(store.content_digest().unwrap(), before);
    assert_eq!(
        store
            .csv_task(&task.target_id, &task.id)
            .unwrap()
            .next_record,
        3
    );
    assert_eq!(
        store
            .csv_undo_task(&task.target_id, &id(800))
            .unwrap()
            .next_ordinal,
        2
    );
}

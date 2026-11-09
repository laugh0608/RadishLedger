use super::*;
use serde_json::Value;

const INPUT: &str = include_str!("../../../docs/validation/archive-restored-review-input.json");
const VECTOR: &str = include_str!("../../../docs/validation/archive-restored-review-vector.json");
const LEGACY: &str = include_str!("../../../docs/validation/archive-container-review-inputs.json");

fn unhex(hex: &Value) -> Vec<u8> {
    let text = hex.as_str().unwrap();
    assert_eq!(text.len() % 2, 0);
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

struct Vector {
    input: Value,
    output: Value,
    entropy: Entropy,
    archive: Vec<u8>,
    cancel: AtomicBool,
}

impl Vector {
    fn new() -> Self {
        let input: Value = serde_json::from_str(INPUT).unwrap();
        let output: Value = serde_json::from_str(VECTOR).unwrap();
        let base = &input["baseInput"];
        let entropy = Entropy {
            id: unhex(&base["archiveIdHex"]).try_into().unwrap(),
            salt: unhex(&base["saltHex"]).try_into().unwrap(),
            wrap_nonce: unhex(&base["wrapNonceHex"]).try_into().unwrap(),
            body_nonce: unhex(&base["contentNonceHex"]).try_into().unwrap(),
            key: crypto::secret(&unhex(&base["archiveKeyHex"])).unwrap(),
        };
        let archive = unhex(&output["output"]["archiveHex"]);
        Self {
            input,
            output,
            entropy,
            archive,
            cancel: AtomicBool::new(false),
        }
    }

    fn password(&self) -> &[u8] {
        self.input["baseInput"]["passwordUtf8"]
            .as_str()
            .unwrap()
            .as_bytes()
    }

    fn payload(&self) -> &[u8] {
        self.input["baseInput"]["payloadUtf8"]
            .as_str()
            .unwrap()
            .as_bytes()
    }
}

fn error<T>(result: Result<T>) -> &'static str {
    result.err().expect("expected archive rejection").code
}

#[test]
fn restored_archive_matches_independent_nonempty_go_vector_and_preserves_legacy_reader() {
    let _guard = crypto::KDF_TESTS.lock().unwrap();
    let vector = Vector::new();
    assert_eq!(
        error(super::super::open_observed(
            &vector.archive,
            vector.password(),
            CANDIDATE_LIMITS,
            &vector.cancel,
            |_| panic!("legacy reader reached the KDF"),
        )),
        "unsupported_archive"
    );
    let Opened::Restored(restored) = open(
        &vector.archive,
        vector.password(),
        CANDIDATE_LIMITS,
        &vector.cancel,
    )
    .unwrap() else {
        panic!("wrong archive type");
    };
    assert_eq!(restored.id, vector.entropy.id);
    assert_eq!(
        restored.contents.verification.projection.net_assets_minor,
        "137"
    );
    assert_eq!(restored.contents.facts.ledger.revisions.len(), 2);
    let [legacy, current] = restored.contents.facts.history.as_slice() else {
        panic!("expected two histories");
    };
    assert_eq!((legacy.view().sequence, current.view().sequence), (4, 1));
    assert_eq!(
        (
            legacy.view().intents[0].expected_sequence,
            current.view().intents[0].expected_sequence
        ),
        (Some(3), Some(0))
    );
    let bytes = encode_contents(&restored.contents, MAX_PAYLOAD, &vector.cancel).unwrap();
    assert_eq!(bytes, vector.payload());
    assert_eq!(
        manifest_for(&restored.contents.facts, &bytes, CANDIDATE_LIMITS)
            .unwrap()
            .as_slice(),
        unhex(&vector.output["output"]["manifestHex"]),
    );
    let sealed = seal_with(
        &restored.contents,
        vector.password(),
        CANDIDATE_LIMITS,
        &vector.cancel,
        &vector.entropy,
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(sealed.as_bytes(), vector.archive);

    let fixture: Value = serde_json::from_str(LEGACY).unwrap();
    let legacy_archive = unhex(&fixture["cryptoExpected"]["archiveHex"]);
    let Opened::Legacy(opened) = open(
        &legacy_archive,
        b"public-review-password",
        CANDIDATE_LIMITS,
        &vector.cancel,
    )
    .unwrap() else {
        panic!("legacy vector changed type");
    };
    assert_eq!(opened.contents().facts.source_schema, 1);
    assert_eq!(
        opened.contents().verification.projection.net_assets_minor,
        "0"
    );
}

#[test]
fn restored_archive_distinguishes_authentication_from_authenticated_manifest_rejection() {
    let _guard = crypto::KDF_TESTS.lock().unwrap();
    let vector = Vector::new();
    let mut downgraded = vector.archive.clone();
    downgraded[11] = 1;
    assert_eq!(
        error(open(
            &downgraded,
            vector.password(),
            CANDIDATE_LIMITS,
            &vector.cancel
        )),
        "archive_unlock_failed"
    );
    assert_eq!(
        error(open(
            &vector.archive,
            b"wrong public password",
            CANDIDATE_LIMITS,
            &vector.cancel
        )),
        "archive_unlock_failed"
    );
    let expected = unhex(&vector.output["output"]["manifestHex"]);
    for (offset, new_value) in [(11, 1), (15, 3), (27, 2)] {
        let mut manifest: [u8; MANIFEST] = expected.clone().try_into().unwrap();
        manifest[offset] = new_value;
        let sealed = seal_payload(
            Version::Restored,
            vector.payload(),
            &manifest,
            vector.password(),
            &vector.cancel,
            &Entropy::random().unwrap(),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(
            error(open(
                sealed.as_bytes(),
                vector.password(),
                CANDIDATE_LIMITS,
                &vector.cancel
            )),
            "invalid_archive_payload",
        );
    }
}

#[test]
fn restored_archive_rejects_invalid_history_shape_duplicate_keys_and_cumulative_budget() {
    let vector = Vector::new();
    let valid: Value = serde_json::from_slice(vector.payload()).unwrap();
    let mut mutations = Vec::new();
    for (field, value) in [
        ("kind", "unknown".into()),
        ("imports", Value::Null),
        ("sequence", 0.into()),
    ] {
        let mut input = valid.clone();
        input["facts"]["history"][1][field] = value;
        mutations.push(input);
    }
    let mut missing = valid.clone();
    missing["facts"]["history"][0]
        .as_object_mut()
        .unwrap()
        .remove("imports");
    mutations.push(missing);
    let mut cross_segment = valid.clone();
    cross_segment["facts"]["history"][1]["intentHistory"][0]["expectedSequence"] = 3.into();
    mutations.push(cross_segment);
    let mut duplicate_operation = valid.clone();
    duplicate_operation["facts"]["history"][1]["intentHistory"][0]["operationId"] =
        valid["facts"]["history"][0]["intentHistory"][0]["operationId"].clone();
    mutations.push(duplicate_operation);
    for malformed in mutations {
        assert_eq!(
            error(decode(
                &serde_json::to_vec(&malformed).unwrap(),
                CANDIDATE_LIMITS,
                &vector.cancel
            )),
            "invalid_archive_payload",
        );
    }
    let duplicate_key = std::str::from_utf8(vector.payload()).unwrap().replace(
        "\"kind\":\"restored\"",
        "\"kind\":\"restored\",\"kind\":\"restored\"",
    );
    assert_eq!(
        error(decode(
            duplicate_key.as_bytes(),
            CANDIDATE_LIMITS,
            &vector.cancel
        )),
        "invalid_archive_payload"
    );
    let budget = Limits {
        records: 9,
        ..CANDIDATE_LIMITS
    };
    assert!(decode(vector.payload(), budget, &vector.cancel).is_ok());
    assert_eq!(
        error(decode(
            vector.payload(),
            Limits {
                records: 8,
                ..budget
            },
            &vector.cancel
        )),
        "archive_limit"
    );
}

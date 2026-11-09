use super::test_support::*;
use super::*;
use crate::domain::project;
use crate::store::{DbKey, Write};
use serde_json::Value;

#[test]
fn a01_handwritten_csv_matches_independent_events_and_projection_without_writes() {
    let (_directory, _key, mut store) = database();
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let accounting: Value = serde_json::from_str(ACCOUNTING).unwrap();
    let a01 = &accounting["cases"][0];
    assert_eq!(a01["id"], "A01");
    assert_eq!(fixture["profile"], PROFILE);
    let mut events = Vec::new();
    for (index, step) in a01["openingEvents"].as_array().unwrap().iter().enumerate() {
        let event = event_from_step(step, true);
        store
            .commit(&Write {
                operation_id: id(100 + index),
                event_id: id(200 + index),
                revision_id: id(300 + index),
                previous_revision_id: None,
                event: Some(event.clone()),
                expected_sequence: None,
                observed_balance_minor: None,
                target_balance_minor: None,
                resolve_candidates: BTreeSet::new(),
            })
            .unwrap();
        events.push(event);
    }
    let before = store.content_digest().unwrap();
    let preview = show(&store, fixture["a01Input"].as_str().unwrap(), &mapping());
    assert_eq!(preview.observed_sequence, 3);
    assert_eq!(preview.rows.len(), 6);
    for (index, row) in preview.rows.iter().enumerate() {
        let step = &a01["steps"][index + 1];
        assert_eq!(fixture["a01StepIds"][index], step["id"]);
        assert!(row.errors.is_empty());
        assert_eq!(row.record, index + 1);
        assert_eq!(row.source_match, SourceMatch::UniqueInFile);
        assert_eq!(
            row.source_id.as_deref(),
            Some(format!("src-{}", index + 1).as_str())
        );
        let candidate = row.candidate.as_ref().unwrap();
        assert_eq!(candidate.event, event_from_step(step, false));
        assert_eq!(candidate.note, fixture["a01Notes"][index].as_str().unwrap());
        events.push(candidate.event.clone());
        // 仅对候选集合计算预览结果；没有将 CSV 发布到 DB。
        let projection = project(
            &[id(1), id(2), id(3)],
            &[id(5), id(6)],
            &events,
            &["2026-01".into()],
        )
        .unwrap();
        for (key, value) in normalize(serde_json::to_value(projection).unwrap())
            .as_object()
            .unwrap()
        {
            assert_eq!(value, &step["expected"][key], "A01 {} {key}", index + 1);
        }
    }
    assert_eq!(store.content_digest().unwrap(), before);
    assert_eq!(store.outbox_count().unwrap(), 3);
}

#[test]
fn handwritten_syntax_vectors_preserve_exact_fields_or_reject() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    for case in fixture["syntax"].as_array().unwrap() {
        let result = input::parse(
            case["input"].as_str().unwrap().as_bytes(),
            false,
            &mut || Ok(()),
        );
        if let Some(code) = case["error"].as_str() {
            assert_eq!(result.err().unwrap().code, code, "{}", case["id"]);
        } else {
            let parsed = result.unwrap();
            assert_eq!(
                serde_json::to_value(parsed.records).unwrap(),
                case["records"],
                "{}",
                case["id"]
            );
        }
    }
    assert_eq!(
        input::parse(&[0xff], false, &mut || Ok(()))
            .err()
            .unwrap()
            .code,
        "csv_encoding"
    );
}

#[test]
fn semantic_errors_keep_logical_rows_and_preserve_database() {
    let (_directory, _key, store) = database();
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let before = store.content_digest().unwrap();
    let mut text = HEADER.to_owned();
    for case in fixture["invalidRows"].as_array().unwrap() {
        text.push_str(case["input"].as_str().unwrap());
        text.push('\n');
    }
    text.push_str(ONE);
    let result = show(&store, &text, &mapping());
    assert_eq!(result.rows.len(), 15);
    for (row, case) in result
        .rows
        .iter()
        .zip(fixture["invalidRows"].as_array().unwrap())
    {
        assert_eq!(
            row.errors[0].code,
            case["error"].as_str().unwrap(),
            "{}",
            case["id"]
        );
        assert_eq!(row.errors.len(), 1);
        assert!(
            !row.errors[0]
                .to_string()
                .contains(case["input"].as_str().unwrap())
        );
    }
    assert!(result.rows.last().unwrap().errors.is_empty());
    assert_eq!(store.content_digest().unwrap(), before);
}

#[test]
fn bom_headerless_explicit_ignored_columns_and_fingerprints() {
    let (_directory, _key, store) = database();
    let map = mapping();
    let with_header = show(&store, &(HEADER.to_owned() + ONE), &map);
    let bom = show(&store, &("\u{feff}".to_owned() + HEADER + ONE), &map);
    assert!(bom.had_bom);
    assert!(!with_header.had_bom);
    assert_ne!(bom.input_fingerprint, with_header.input_fingerprint);
    assert_eq!(bom.mapping_fingerprint, with_header.mapping_fingerprint);
    let mut no_header = map.clone();
    no_header.has_header = false;
    assert_eq!(show(&store, ONE, &no_header).rows.len(), 1);
    assert_ne!(
        show(&store, ONE, &no_header).mapping_fingerprint,
        with_header.mapping_fingerprint
    );
    let mut ignored = map.clone();
    ignored.fields.push(Field::Ignore);
    let extra = format!(
        "{},自选忽略列\n{},=1+1\n",
        HEADER.trim_end(),
        ONE.trim_end()
    );
    let report = show(&store, &extra, &ignored);
    assert_eq!(report.ignored_columns, vec![11]);
    assert!(report.rows[0].errors.is_empty());
    let mut other = map.clone();
    other.source_namespace = "source-B".into();
    assert_ne!(
        show(&store, &(HEADER.to_owned() + ONE), &other).mapping_fingerprint,
        with_header.mapping_fingerprint
    );
    let different = show(
        &store,
        &(HEADER.to_owned() + &ONE.replace("1.00", "2.00")),
        &map,
    );
    assert_ne!(different.input_fingerprint, with_header.input_fingerprint);
}

#[test]
fn malformed_headers_width_and_mapping_fail_entire_preview() {
    let (_directory, _key, store) = database();
    let run =
        |text: &str, map: &Mapping| preview(&store, text.as_bytes(), map, &AtomicBool::new(false));
    for (text, code) in [
        (HEADER.to_owned(), "csv_empty"),
        (HEADER.replace("金额", "类型") + ONE, "csv_header"),
        (HEADER.replace("金额", "") + ONE, "csv_header"),
        (HEADER.to_owned() + "a,b\n", "csv_columns"),
        (HEADER.to_owned() + ONE + "\n", "csv_columns"),
    ] {
        error(run(&text, &mapping()), code);
    }
    let mut map = mapping();
    map.fields[9] = Field::Amount;
    error(run(&(HEADER.to_owned() + ONE), &map), "csv_mapping");
    map = mapping();
    map.fields[1] = Field::Ignore;
    error(run(&(HEADER.to_owned() + ONE), &map), "csv_mapping");
    map = mapping();
    map.source_namespace = "bad namespace".into();
    error(run(&(HEADER.to_owned() + ONE), &map), "csv_mapping");
    map = mapping();
    map.accounts.insert("现金".into(), "not-a-core-id".into());
    error(run(&(HEADER.to_owned() + ONE), &map), "invalid_identity");
    map = mapping();
    for n in 0..257 {
        map.accounts.insert(format!("account-{n}"), id(1));
    }
    error(run(&(HEADER.to_owned() + ONE), &map), "csv_mapping");
}

#[test]
fn duplicate_source_is_a_hint_and_conflicting_source_blocks_every_member() {
    let (_directory, _key, store) = database();
    let repeated = show(&store, &(HEADER.to_owned() + ONE + ONE), &mapping());
    assert_eq!(repeated.rows.len(), 2);
    for row in repeated.rows {
        assert!(row.errors.is_empty());
        assert_eq!(
            row.source_match,
            SourceMatch::RepeatedInFile { first_record: 1 }
        );
    }
    for changed in [
        ONE.replace("合成备注", "不同备注"),
        ONE.replace("1.00", "2.00"),
        ONE.replace("1.00", "bad"),
    ] {
        let report = show(&store, &(HEADER.to_owned() + ONE + &changed), &mapping());
        for row in report.rows {
            assert_eq!(row.source_match, SourceMatch::ConflictingInFile);
            assert!(row.errors.iter().any(|e| e.code == "csv_source_conflict"));
        }
    }
    let missing = ONE.replace("src-1", "");
    let report = show(
        &store,
        &(HEADER.to_owned() + &missing + &missing),
        &mapping(),
    );
    assert_eq!(report.rows.len(), 2);
    assert!(
        report
            .rows
            .iter()
            .all(|row| row.source_match == SourceMatch::Missing && row.errors.is_empty())
    );
}

#[test]
fn field_record_column_and_file_bounds_include_exact_limits() {
    for n in [4095, 4096, 4097] {
        let result = input::parse("a".repeat(n).as_bytes(), false, &mut || Ok(()));
        if n <= 4096 {
            assert_eq!(result.unwrap().records[0][0].len(), n);
        } else {
            assert_eq!(result.err().unwrap().code, "csv_field_limit");
        }
    }
    for n in [15, 16, 17] {
        let text = vec!["a"; n].join(",");
        let result = input::parse(text.as_bytes(), false, &mut || Ok(()));
        if n <= 16 {
            assert_eq!(result.unwrap().records[0].len(), n);
        } else {
            assert_eq!(result.err().unwrap().code, "csv_column_limit");
        }
    }
    for n in [999, 1000, 1001] {
        for header in [false, true] {
            let text = "a\n".repeat(n + usize::from(header));
            let result = input::parse(text.as_bytes(), header, &mut || Ok(()));
            if n <= 1000 {
                assert_eq!(result.unwrap().records.len(), n + usize::from(header));
            } else {
                assert_eq!(result.err().unwrap().code, "csv_record_limit");
            }
        }
    }
    for n in [1048575, 1048576, 1048577] {
        let row = "a".repeat(4096) + "\n";
        let text = row.repeat(n / row.len()) + &"a".repeat(n % row.len());
        let result = input::parse(text.as_bytes(), false, &mut || Ok(()));
        if n <= 1048576 {
            assert!(result.is_ok());
        } else {
            assert_eq!(result.err().unwrap().code, "csv_file_limit");
        }
    }
    // 字节边界，非 Unicode 字符计数；引用转义后的解码字段也须受限。
    let text = "界".repeat(1366);
    assert_eq!(
        input::parse(text.as_bytes(), false, &mut || Ok(()))
            .err()
            .unwrap()
            .code,
        "csv_field_limit"
    );
    let quoted = format!("\"{}\"", "\"\"".repeat(4097));
    assert_eq!(
        input::parse(quoted.as_bytes(), false, &mut || Ok(()))
            .err()
            .unwrap()
            .code,
        "csv_field_limit"
    );
}

#[test]
fn cancelled_at_parse_mapping_and_database_boundaries_returns_no_partial_preview() {
    let (_directory, _key, store) = database();
    let before = store.content_digest().unwrap();
    let text = HEADER.to_owned() + &ONE.repeat(100);
    let mut calls = 0;
    preview_checked(&store, text.as_bytes(), &mapping(), &mut || {
        calls += 1;
        Ok(())
    })
    .unwrap();
    for stop in [1, 3, 8, calls / 2, calls - 1, calls] {
        let mut count = 0;
        let result = preview_checked(&store, text.as_bytes(), &mapping(), &mut || {
            count += 1;
            if count == stop {
                Err(CoreError::new("cancelled", "合成取消"))
            } else {
                Ok(())
            }
        });
        error(result, "cancelled");
        assert_eq!(store.content_digest().unwrap(), before);
    }
    error(
        preview(&store, text.as_bytes(), &mapping(), &AtomicBool::new(true)),
        "cancelled",
    );
}

#[test]
fn readonly_reopen_validates_missing_archived_and_wrong_category_references() {
    let (directory, key, mut store) = database();
    let mut map = mapping();
    map.accounts.insert("现金".into(), id(99));
    assert_eq!(
        show(&store, &(HEADER.to_owned() + ONE), &map).rows[0].errors[0].code,
        "missing_account"
    );
    map = mapping();
    map.categories.insert("餐饮".into(), id(99));
    assert_eq!(
        show(&store, &(HEADER.to_owned() + ONE), &map).rows[0].errors[0].code,
        "missing_category"
    );
    store.archive(&[], &[id(5)]).unwrap();
    let digest = store.content_digest().unwrap();
    drop(store);
    let readonly = Store::open_readonly(&directory.path().join("preview.db"), &key).unwrap();
    let report = show(&readonly, &(HEADER.to_owned() + ONE), &mapping());
    assert_eq!(report.rows[0].errors[0].code, "archived_reference");
    assert_eq!(report.observed_sequence, 1);
    assert_eq!(readonly.content_digest().unwrap(), digest);
}

#[test]
fn database_snapshot_does_not_mix_reference_states_during_concurrent_archive() {
    let (directory, key, store) = database();
    let mut writer = Store::open(&directory.path().join("preview.db"), &key).unwrap();
    let candidate = show(&store, &(HEADER.to_owned() + ONE), &mapping())
        .rows
        .remove(0)
        .candidate
        .unwrap();
    let mut changed = false;
    let (sequence, errors) = store
        .check_import_references(&[&candidate.event, &candidate.event], &mut || {
            if !changed {
                writer.archive(&[id(1)], &[])?;
                changed = true;
            }
            Ok(())
        })
        .unwrap();
    assert!(changed);
    assert_eq!(sequence, 0);
    assert!(errors.iter().all(Option::is_none));
    let report = show(&store, &(HEADER.to_owned() + ONE), &mapping());
    assert_eq!(report.observed_sequence, 1);
    assert_eq!(report.rows[0].errors[0].code, "archived_reference");
}

#[test]
fn broken_storage_aborts_preview_instead_of_becoming_a_row_error() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("broken.db");
    // 公开合成测试材料，仅在内存传入 SQLite；不接受用户密钥。
    let bytes = [0x51_u8; 32];
    let key = DbKey::from_secret(&bytes).unwrap();
    let mut store = Store::create(&path, &key).unwrap();
    store.add_account(&id(1), "cash", "合成账户").unwrap();
    let breaker = rusqlite::Connection::open(&path).unwrap();
    // SAFETY: breaker 独占此连接，固定测试字节在调用期间有效。
    assert_eq!(
        unsafe { rusqlite::ffi::sqlite3_key(breaker.handle(), bytes.as_ptr().cast(), 32) },
        rusqlite::ffi::SQLITE_OK
    );
    breaker.execute_batch("DROP TABLE categories;").unwrap();
    error(
        preview(
            &store,
            (HEADER.to_owned() + ONE).as_bytes(),
            &mapping(),
            &AtomicBool::new(false),
        ),
        "storage_error",
    );
    assert_eq!(store.sequence().unwrap(), 0);
    assert_eq!(store.outbox_count().unwrap(), 0);
}

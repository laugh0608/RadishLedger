//! 固定合成材料的接入实验；不接受用户账单或生产账库。
use crate::domain::{Event, EventKind, parse_yuan, validate_date};
use crate::store::{DbKey, Store, Write, WriteEffect};
use crate::{CoreError, Result, random_id};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const CASES: &str = include_str!("../../../docs/validation/accounting-cases.json");

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReport {
    pub fixture_version: u64,
    pub case_count: usize,
    pub checkpoint_count: usize,
    pub amount_input_count: usize,
    pub date_input_count: usize,
    pub rejected_intent_count: usize,
    pub cipher_version: String,
    pub native_target: String,
    pub duplicate_verified: bool,
    pub rollback_verified: bool,
    pub reopen_verified: bool,
    pub wrong_key_rejected: bool,
    pub cases: Vec<CaseReport>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseReport {
    pub id: String,
    pub checkpoints: usize,
    pub final_snapshot: Value,
}

fn invalid_fixture() -> CoreError {
    CoreError::new("invalid_fixture", "共同合成材料格式无效")
}
fn mismatch() -> CoreError {
    CoreError::new("fixture_mismatch", "核心实际结果与共同材料预期不一致")
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(invalid_fixture)
}
fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(invalid_fixture)
}

#[derive(Default)]
struct Symbols {
    ids: BTreeMap<String, String>,
    aliases: BTreeMap<String, String>,
}

impl Symbols {
    fn id(&mut self, namespace: &str, label: &str) -> Result<String> {
        let symbol = format!("{namespace}:{label}");
        if let Some(id) = self.ids.get(&symbol) {
            return Ok(id.clone());
        }
        let id = random_id()?;
        self.ids.insert(symbol, id.clone());
        self.aliases.insert(id.clone(), label.to_owned());
        Ok(id)
    }

    fn optional(&mut self, value: &Value, key: &str, namespace: &str) -> Result<Option<String>> {
        value
            .get(key)
            .map(|v| {
                v.as_str()
                    .ok_or_else(invalid_fixture)
                    .and_then(|label| self.id(namespace, label))
            })
            .transpose()
    }

    fn normalize(&self, value: Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.aliases.get(&text).cloned().unwrap_or(text)),
            Value::Array(values) => {
                Value::Array(values.into_iter().map(|v| self.normalize(v)).collect())
            }
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(k, v)| {
                        (
                            self.aliases.get(&k).cloned().unwrap_or(k),
                            self.normalize(v),
                        )
                    })
                    .collect(),
            ),
            other => other,
        }
    }

    fn event(&mut self, value: &Value, opening: bool) -> Result<Event> {
        let kind = if opening {
            EventKind::Opening
        } else {
            match text(value, "type")? {
                "income" => EventKind::Income,
                "expense" => EventKind::Expense,
                "refund" => EventKind::Refund,
                "transfer" => EventKind::Transfer,
                "adjustment" => EventKind::Adjustment,
                _ => return Err(invalid_fixture()),
            }
        };
        Ok(Event {
            kind,
            business_date: text(value, "businessDate")?.to_owned(),
            account_id: self.optional(value, "accountId", "account")?,
            category_id: self.optional(value, "categoryId", "category")?,
            from_account_id: self.optional(value, "fromAccountId", "account")?,
            to_account_id: self.optional(value, "toAccountId", "account")?,
            amount_minor: text(
                value,
                if matches!(kind, EventKind::Opening | EventKind::Adjustment) {
                    "deltaMinor"
                } else {
                    "amountMinor"
                },
            )?
            .to_owned(),
        })
    }
}

fn equivalent(actual: &Value, expected: &Value, partial: bool) -> bool {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => {
            (partial || actual.len() == expected.len())
                && expected.iter().all(|(key, wanted)| {
                    actual.get(key).is_some_and(|found| {
                        equivalent(found, wanted, key == "selectedRevisionIds")
                    })
                })
        }
        (Value::Array(actual), Value::Array(expected)) => {
            // 集合的顺序不是账务语义，但重复成员不能混入。
            let actual_set: BTreeSet<_> = actual.iter().map(Value::to_string).collect();
            let wanted: BTreeSet<_> = expected.iter().map(Value::to_string).collect();
            actual.len() == actual_set.len()
                && expected.len() == wanted.len()
                && actual_set == wanted
        }
        _ => actual == expected,
    }
}

fn snapshot(store: &Store, symbols: &Symbols, months: &[String]) -> Result<Value> {
    let mut value = symbols
        .normalize(serde_json::to_value(store.snapshot(months)?).map_err(|_| invalid_fixture())?);
    // UUID 排序在映射回合成别名后不再稳定；仅对语义为集合的字段重新排序。
    for field in [
        "activeEventIds",
        "deletedEventIds",
        "unresolvedCandidateRevisionIds",
        "archivedAccountIds",
        "archivedCategoryIds",
    ] {
        value[field]
            .as_array_mut()
            .ok_or_else(invalid_fixture)?
            .sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    }
    Ok(value)
}

fn commit_once(store: &mut Store, write: &Write) -> Result<()> {
    let count = store.outbox_count()?;
    store.verify_atomic_rejection(write)?;
    if store.commit(write)? != WriteEffect::Committed || store.outbox_count()? != count + 1 {
        return Err(mismatch());
    }
    if store.commit(write)? != WriteEffect::Replayed || store.outbox_count()? != count + 1 {
        return Err(mismatch());
    }
    Ok(())
}

fn new_write(symbols: &mut Symbols, step: &Value, event: Option<Event>) -> Result<Write> {
    Ok(Write {
        operation_id: random_id()?,
        event_id: symbols.id("event", text(step, "eventId")?)?,
        revision_id: symbols.id("revision", text(step, "revisionId")?)?,
        previous_revision_id: symbols.optional(step, "previousRevisionId", "revision")?,
        event,
        expected_sequence: None,
        observed_balance_minor: None,
        target_balance_minor: None,
        resolve_candidates: BTreeSet::new(),
    })
}

fn run_case(root: &Path, case: &Value, rejected: &[Value]) -> Result<CaseReport> {
    let key = DbKey::random()?;
    let database = root.join(format!("{}.db", text(case, "id")?));
    let mut store = Store::create(&database, &key)?;
    let mut symbols = Symbols::default();
    for (label, kind) in case
        .get("accounts")
        .and_then(Value::as_object)
        .ok_or_else(invalid_fixture)?
    {
        store.add_account(
            &symbols.id("account", label)?,
            kind.as_str().ok_or_else(invalid_fixture)?,
            label,
        )?;
    }
    for (label, kind) in case
        .get("categories")
        .and_then(Value::as_object)
        .ok_or_else(invalid_fixture)?
    {
        store.add_category(
            &symbols.id("category", label)?,
            kind.as_str().ok_or_else(invalid_fixture)?,
            label,
        )?;
    }
    for opening in array(case, "openingEvents")? {
        let event = symbols.event(opening, true)?;
        commit_once(&mut store, &new_write(&mut symbols, opening, Some(event))?)?;
    }
    let steps = array(case, "steps")?;
    let mut months = BTreeSet::new();
    for step in steps {
        months.extend(
            step.get("expected")
                .and_then(|v| v.get("monthlyReports"))
                .and_then(Value::as_object)
                .ok_or_else(invalid_fixture)?
                .keys()
                .cloned(),
        );
    }
    let months: Vec<_> = months.into_iter().collect();
    let mut observations = BTreeMap::new();
    let mut last_snapshot = Value::Null;
    for step in steps {
        let action = text(step, "action")?;
        match action {
            "observe" | "changeEnvironment" => {}
            "record" | "revise" | "reviseOpening" => {
                let event = symbols.event(step, action == "reviseOpening")?;
                let mut write = new_write(&mut symbols, step, Some(event))?;
                if text(step, "type").ok() == Some("adjustment") {
                    write.expected_sequence = Some(
                        *observations
                            .get(text(step, "observedCheckpointId")?)
                            .ok_or_else(invalid_fixture)?,
                    );
                    write.observed_balance_minor =
                        Some(text(step, "observedBalanceMinor")?.to_owned());
                    write.target_balance_minor = Some(text(step, "targetBalanceMinor")?.to_owned());
                }
                commit_once(&mut store, &write)?;
            }
            "delete" => commit_once(&mut store, &new_write(&mut symbols, step, None)?)?,
            "restore" => {
                let source = symbols.id("revision", text(step, "restoreFromRevisionId")?)?;
                let event = store.revision_event(&source)?.ok_or_else(invalid_fixture)?;
                commit_once(&mut store, &new_write(&mut symbols, step, Some(event))?)?;
            }
            "receiveCandidate" => {
                let event = symbols.event(step, false)?;
                let event_id = symbols.id("event", text(step, "eventId")?)?;
                let revision = symbols.id("revision", text(step, "revisionId")?)?;
                let predecessor = symbols.id("revision", text(step, "previousRevisionId")?)?;
                let count = store.outbox_count()?;
                store.receive_candidate(&event_id, &revision, &predecessor, &event)?;
                if count != store.outbox_count()? {
                    return Err(mismatch());
                }
            }
            "resolveCandidate" => {
                let source = symbols.id("revision", text(step, "chosenRevisionId")?)?;
                let event = store.revision_event(&source)?.ok_or_else(invalid_fixture)?;
                let mut write = new_write(&mut symbols, step, Some(event))?;
                let event_id = &write.event_id;
                write.previous_revision_id = Some(
                    store
                        .snapshot(&months)?
                        .selected_revision_ids
                        .get(event_id)
                        .ok_or_else(invalid_fixture)?
                        .clone(),
                );
                for candidate in array(step, "candidateRevisionIds")? {
                    write.resolve_candidates.insert(
                        symbols.id("revision", candidate.as_str().ok_or_else(invalid_fixture)?)?,
                    );
                }
                commit_once(&mut store, &write)?;
            }
            "archiveReferences" => {
                let accounts = array(step, "accountIds")?
                    .iter()
                    .map(|label| symbols.id("account", label.as_str().ok_or_else(invalid_fixture)?))
                    .collect::<Result<Vec<_>>>()?;
                let categories = array(step, "categoryIds")?
                    .iter()
                    .map(|label| {
                        symbols.id("category", label.as_str().ok_or_else(invalid_fixture)?)
                    })
                    .collect::<Result<Vec<_>>>()?;
                store.archive(&accounts, &categories)?;
            }
            "renameReferences" => {
                let mut remap = |field, namespace| -> Result<BTreeMap<String, String>> {
                    step.get(field)
                        .and_then(Value::as_object)
                        .ok_or_else(invalid_fixture)?
                        .iter()
                        .map(|(label, name)| {
                            Ok((
                                symbols.id(namespace, label)?,
                                name.as_str().ok_or_else(invalid_fixture)?.to_owned(),
                            ))
                        })
                        .collect()
                };
                let accounts = remap("accountNames", "account")?;
                let categories = remap("categoryNames", "category")?;
                store.rename(&accounts, &categories)?;
            }
            _ => return Err(invalid_fixture()),
        }
        last_snapshot = snapshot(&store, &symbols, &months)?;
        if !equivalent(
            &last_snapshot,
            step.get("expected").ok_or_else(invalid_fixture)?,
            true,
        ) {
            return Err(mismatch());
        }
        observations.insert(text(step, "id")?.to_owned(), store.sequence()?);
        for sample in rejected.iter().filter(|value| {
            value.get("caseId") == case.get("id") && value.get("afterStepId") == step.get("id")
        }) {
            let mut intent = sample.get("intent").ok_or_else(invalid_fixture)?.clone();
            intent["eventId"] = Value::String(format!("rejected-{}", text(sample, "id")?));
            intent["revisionId"] = Value::String(format!("rejected-{}-v1", text(sample, "id")?));
            let event = symbols.event(&intent, false)?;
            let write = new_write(&mut symbols, &intent, Some(event))?;
            let count = store.outbox_count()?;
            let sequence = store.sequence()?;
            let error = store.commit(&write).err().ok_or_else(mismatch)?;
            if error.code != text(sample, "failureClass")?
                || store.outbox_count()? != count
                || store.sequence()? != sequence
                || snapshot(&store, &symbols, &months)? != last_snapshot
            {
                return Err(mismatch());
            }
        }
    }
    let count = store.outbox_count()?;
    drop(store);
    let reopened = Store::open(&database, &key)?;
    if snapshot(&reopened, &symbols, &months)? != last_snapshot || reopened.outbox_count()? != count
    {
        return Err(mismatch());
    }
    drop(reopened);
    if Store::open(&database, &DbKey::random()?).is_ok() {
        return Err(CoreError::new(
            "wrong_key_accepted",
            "错误密钥不应读取原账库",
        ));
    }
    let bytes = std::fs::read(&database)
        .map_err(|_| CoreError::new("file_check_failed", "无法复核实验账库文件"))?;
    if bytes.starts_with(b"SQLite format 3") {
        return Err(CoreError::new("plaintext_database", "实验账库没有实际加密"));
    }
    Ok(CaseReport {
        id: text(case, "id")?.to_owned(),
        checkpoints: steps.len(),
        final_snapshot: last_snapshot,
    })
}

pub fn run(directory: &Path) -> Result<ProbeReport> {
    if !directory.is_absolute() || !directory.is_dir() {
        return Err(CoreError::new("invalid_directory", "实验临时目录不可用"));
    }
    let root = tempfile::Builder::new()
        .prefix("radishledger-m0-")
        .tempdir_in(directory)
        .map_err(|_| CoreError::new("temporary_storage_failed", "无法创建隔离实验目录"))?;
    let input: Value = serde_json::from_str(CASES).map_err(|_| invalid_fixture())?;
    if input.get("dataOrigin") != Some(&json!("synthetic"))
        || input.get("fixtureVersion") != Some(&json!(1))
        || input.get("currency") != Some(&json!("CNY"))
        || input.get("minorUnitScale") != Some(&json!(2))
    {
        return Err(invalid_fixture());
    }
    for field in ["cases", "amountInputs", "dateInputs", "rejectedIntents"] {
        if array(&input, field)?.is_empty() {
            return Err(invalid_fixture());
        }
    }
    let mut reports = Vec::new();
    for case in array(&input, "cases")? {
        reports.push(run_case(
            root.path(),
            case,
            array(&input, "rejectedIntents")?,
        )?);
    }
    for sample in array(&input, "amountInputs")? {
        let actual = parse_yuan(
            text(sample, "inputYuan")?,
            text(sample, "purpose")? == "signed_delta",
        );
        if sample
            .get("accepted")
            .and_then(Value::as_bool)
            .ok_or_else(invalid_fixture)?
        {
            if actual?.to_string() != text(sample, "expectedMinor")? {
                return Err(mismatch());
            }
        } else if actual.err().ok_or_else(mismatch)?.code != text(sample, "failureClass")? {
            return Err(mismatch());
        }
    }
    for sample in array(&input, "dateInputs")? {
        let actual = validate_date(text(sample, "input")?);
        if sample
            .get("accepted")
            .and_then(Value::as_bool)
            .ok_or_else(invalid_fixture)?
        {
            actual?;
        } else if actual.err().ok_or_else(mismatch)?.code != text(sample, "failureClass")? {
            return Err(mismatch());
        }
    }
    let key = DbKey::random()?;
    let cipher_version =
        Store::create(&root.path().join("cipher-identification.db"), &key)?.cipher_version()?;
    let report = ProbeReport {
        fixture_version: 1,
        case_count: reports.len(),
        checkpoint_count: reports.iter().map(|case| case.checkpoints).sum(),
        amount_input_count: array(&input, "amountInputs")?.len(),
        date_input_count: array(&input, "dateInputs")?.len(),
        rejected_intent_count: array(&input, "rejectedIntents")?.len(),
        cipher_version,
        native_target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        duplicate_verified: true,
        rollback_verified: true,
        reopen_verified: true,
        wrong_key_rejected: true,
        cases: reports,
    };
    // 仅清理 TempDir 本次独占创建的随机子目录；不触碰调用者目录或既有档案。
    root.close()
        .map_err(|_| CoreError::new("cleanup_failed", "实验结果已取得，但本次临时目录清理失败"))?;
    Ok(report)
}

pub fn run_json(directory: &str) -> String {
    let response = match run(Path::new(directory)) {
        Ok(report) => json!({"ok":true,"report":report}),
        Err(error) => json!({"ok":false,"error":error}),
    };
    response.to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn common_material_runs_against_encrypted_transactions() {
        let temp = tempfile::tempdir().expect("isolated test directory");
        let report = super::run(temp.path()).expect("actual core and hand-authored expectations");
        assert_eq!((report.case_count, report.checkpoint_count), (10, 34));
        assert_eq!(
            (
                report.amount_input_count,
                report.date_input_count,
                report.rejected_intent_count
            ),
            (18, 8, 5)
        );
        assert!(
            report.duplicate_verified
                && report.rollback_verified
                && report.reopen_verified
                && report.wrong_key_rejected
        );
        assert_eq!(
            temp.path()
                .read_dir()
                .expect("temporary parent exists")
                .count(),
            0
        );
    }

    #[test]
    fn invalid_directory_returns_an_actual_error() {
        assert_eq!(
            super::run(std::path::Path::new("relative-path"))
                .expect_err("reject relative path")
                .code,
            "invalid_directory"
        );
    }
}

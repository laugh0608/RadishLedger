//! M0-E 有界输入预览；没有发布、持久任务或生产 CSV 格式承诺。
use crate::domain::{Event, EventKind, parse_yuan};
use crate::store::Store;
use crate::{CoreError, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};

#[path = "csv_input.rs"]
mod input;
#[cfg(test)]
#[path = "csv_import_tests.rs"]
mod tests;

pub const PROFILE: &str = "m0-csv-preview-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Field {
    Kind,
    Amount,
    Date,
    Currency,
    Account,
    Category,
    FromAccount,
    ToAccount,
    SourceId,
    Note,
    Ignore,
}

/// 每个输入列都必须显式映射或 Ignore；不按表头猜类型或按名称自动建对象。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mapping {
    pub has_header: bool,
    pub fields: Vec<Field>,
    pub source_namespace: String,
    pub kinds: BTreeMap<String, EventKind>,
    pub accounts: BTreeMap<String, String>,
    pub categories: BTreeMap<String, String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Candidate {
    pub event: Event,
    // 现有实验 Event 尚无备注；预览完整保留，不能直接丢弃它后调用单笔 commit。
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceMatch {
    Missing,
    UniqueInFile,
    RepeatedInFile { first_record: usize },
    ConflictingInFile,
}

pub struct Row {
    /// 一基逻辑业务记录号，不含表头；仅定位本次预览，不是持久导入行身份。
    pub record: usize,
    pub source_id: Option<String>,
    pub candidate: Option<Candidate>,
    pub errors: Vec<CoreError>,
    pub source_match: SourceMatch,
}

/// 包含用户内容，不实现 Debug / Serialize，不能直接当作普通诊断日志。
pub struct Preview {
    pub profile: &'static str,
    pub input_fingerprint: String,
    pub mapping_fingerprint: String,
    pub had_bom: bool,
    pub ignored_columns: Vec<usize>,
    pub observed_sequence: i64,
    pub rows: Vec<Row>,
}

impl Mapping {
    fn validate(&self) -> Result<()> {
        if self.fields.is_empty() || self.fields.len() > input::MAX_COLUMNS {
            return Err(CoreError::new("csv_mapping", "CSV 映射列数无效"));
        }
        let mut seen = BTreeSet::new();
        for field in &self.fields {
            if *field != Field::Ignore && !seen.insert(*field) {
                return Err(CoreError::new("csv_mapping", "一个业务字段不能重复映射"));
            }
        }
        if ![Field::Kind, Field::Amount, Field::Date, Field::Currency]
            .iter()
            .all(|f| seen.contains(f))
            || self.source_namespace.is_empty()
            || self.source_namespace.len() > 128
            || !self
                .source_namespace
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err(CoreError::new(
                "csv_mapping",
                "缺少必需映射或来源命名空间无效",
            ));
        }
        if [self.kinds.len(), self.accounts.len(), self.categories.len()]
            .iter()
            .any(|n| *n > 256)
            || self
                .kinds
                .keys()
                .chain(self.accounts.keys())
                .chain(self.categories.keys())
                .any(|k| k.is_empty() || k.len() > 256 || k.contains('\0'))
        {
            return Err(CoreError::new("csv_mapping", "映射名称或数量超过实验边界"));
        }
        for id in self.accounts.values().chain(self.categories.values()) {
            crate::store::valid_id(id)?;
        }
        Ok(())
    }

    fn get<'a>(&self, row: &'a [String], field: Field) -> &'a str {
        self.fields
            .iter()
            .position(|f| *f == field)
            .map(|i| row[i].as_str())
            .unwrap_or("")
    }

    fn candidate(&self, row: &[String]) -> Result<Candidate> {
        let kind = *self
            .kinds
            .get(self.get(row, Field::Kind))
            .ok_or_else(|| CoreError::new("csv_type", "来源交易类型尚未明确映射"))?;
        if matches!(kind, EventKind::Opening | EventKind::Adjustment) {
            return Err(CoreError::new(
                "csv_unsupported_kind",
                "本预览切片尚未支持期初或差额调整",
            ));
        }
        if self.get(row, Field::Currency) != "CNY" {
            return Err(CoreError::new("csv_currency", "本预览切片只支持明确的 CNY"));
        }
        let reference = |field, bindings: &BTreeMap<String, String>| -> Result<Option<String>> {
            let label = self.get(row, field);
            if label.is_empty() {
                return Ok(None);
            }
            bindings
                .get(label)
                .cloned()
                .map(Some)
                .ok_or_else(|| CoreError::new("csv_reference_mapping", "来源引用尚未明确映射"))
        };
        let event = Event {
            kind,
            business_date: self.get(row, Field::Date).to_owned(),
            amount_minor: parse_yuan(self.get(row, Field::Amount), false)?.to_string(),
            account_id: reference(Field::Account, &self.accounts)?,
            category_id: reference(Field::Category, &self.categories)?,
            from_account_id: reference(Field::FromAccount, &self.accounts)?,
            to_account_id: reference(Field::ToAccount, &self.accounts)?,
        };
        event.validate()?;
        Ok(Candidate {
            event,
            note: self.get(row, Field::Note).to_owned(),
        })
    }
}

pub fn preview(
    store: &Store,
    bytes: &[u8],
    mapping: &Mapping,
    cancelled: &AtomicBool,
) -> Result<Preview> {
    preview_checked(store, bytes, mapping, &mut || {
        if cancelled.load(Ordering::Relaxed) {
            Err(CoreError::new("cancelled", "CSV 预览已取消，账务未改变"))
        } else {
            Ok(())
        }
    })
}

fn preview_checked(
    store: &Store,
    bytes: &[u8],
    mapping: &Mapping,
    check_cancelled: &mut impl FnMut() -> Result<()>,
) -> Result<Preview> {
    check_cancelled()?;
    mapping.validate()?;
    let parsed = input::parse(bytes, mapping.has_header, check_cancelled)?;
    if parsed
        .records
        .iter()
        .any(|row| row.len() != mapping.fields.len())
    {
        return Err(CoreError::new(
            "csv_columns",
            "CSV 记录列数与显式映射不一致",
        ));
    }
    if mapping.has_header {
        let header = &parsed.records[0];
        let unique = header.iter().collect::<BTreeSet<_>>();
        if unique.len() != header.len() || header.iter().any(|h| h.trim().is_empty()) {
            return Err(CoreError::new(
                "csv_header",
                "表头为空或重复，不能确定列身份",
            ));
        }
    }
    let mut rows = Vec::new();
    for (index, raw) in parsed
        .records
        .iter()
        .skip(usize::from(mapping.has_header))
        .enumerate()
    {
        check_cancelled()?;
        let source_id = mapping.get(raw, Field::SourceId);
        let (candidate, errors) = match mapping.candidate(raw) {
            Ok(candidate) => (Some(candidate), Vec::new()),
            Err(error) => (None, vec![error]),
        };
        rows.push(Row {
            record: index + 1,
            source_id: (!source_id.is_empty()).then(|| source_id.to_owned()),
            candidate,
            errors,
            source_match: if source_id.is_empty() {
                SourceMatch::Missing
            } else {
                SourceMatch::UniqueInFile
            },
        });
    }
    let candidates = rows
        .iter()
        .filter_map(|row| row.candidate.as_ref().map(|c| &c.event))
        .collect::<Vec<_>>();
    let (observed_sequence, errors) =
        store.check_import_references(&candidates, check_cancelled)?;
    for (row, error) in rows
        .iter_mut()
        .filter(|row| row.candidate.is_some())
        .zip(errors)
    {
        if let Some(error) = error {
            row.errors.push(error);
        }
    }
    let mut source_rows = BTreeMap::<String, Vec<usize>>::new();
    for (index, row) in rows.iter().enumerate() {
        if let Some(source) = &row.source_id {
            source_rows.entry(source.clone()).or_default().push(index);
        }
    }
    for indices in source_rows.values().filter(|v| v.len() > 1) {
        check_cancelled()?;
        let first = indices[0];
        let same = rows[first].candidate.is_some()
            && indices
                .iter()
                .all(|i| rows[*i].candidate == rows[first].candidate);
        for index in indices {
            rows[*index].source_match = if same {
                SourceMatch::RepeatedInFile {
                    first_record: first + 1,
                }
            } else {
                rows[*index].errors.push(CoreError::new(
                    "csv_source_conflict",
                    "同一文件的来源 ID 对应不同或无效内容",
                ));
                SourceMatch::ConflictingInFile
            };
        }
    }
    check_cancelled()?;
    let mapping_bytes = serde_json::to_vec(mapping)
        .map_err(|_| CoreError::new("encoding_error", "实验映射无法编码"))?;
    let input_fingerprint = crate::crypto::hex(&Sha256::digest(bytes));
    let mapping_fingerprint = crate::crypto::hex(&Sha256::digest(
        [PROFILE.as_bytes(), &mapping_bytes].concat(),
    ));
    check_cancelled()?;
    Ok(Preview {
        profile: PROFILE,
        input_fingerprint,
        // 内部候选版本的比较值；不是公开协议 / 归档的确定性编码或授权 token。
        mapping_fingerprint,
        had_bom: parsed.had_bom,
        ignored_columns: mapping
            .fields
            .iter()
            .enumerate()
            .filter_map(|(i, f)| (*f == Field::Ignore).then_some(i + 1))
            .collect(),
        observed_sequence,
        rows,
    })
}

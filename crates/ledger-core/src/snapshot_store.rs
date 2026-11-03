//! 单一 SQLite 读取事务内捕获声明集合；不读取原始 CSV、bootstrap 或系统秘密。
use super::*;
use crate::csv_import::{Decision, Mapping, TaskState};
use crate::csv_undo::{Origin, State};
use crate::logical_snapshot::*;
use rusqlite::types::ValueRef;
use serde::de::DeserializeOwned;
use std::sync::atomic::AtomicBool;

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
#[path = "snapshot_validate.rs"]
pub(crate) mod validate;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CapturePoint {
    Header,
    References,
    Revisions,
    Selections,
    Intents,
    Imports,
    BeforeFinish,
}

pub(super) fn corrupt() -> CoreError {
    CoreError::new(
        "snapshot_corrupt",
        "声明的账务、历史或任务关系不一致，快照未生成",
    )
}

struct Budget<'a> {
    limits: Limits,
    rows: usize,
    bytes: usize,
    cancel: &'a AtomicBool,
}

impl Budget<'_> {
    fn row(&mut self) -> Result<()> {
        cancelled(self.cancel)?;
        if self.rows == self.limits.rows {
            return Err(limit());
        }
        self.rows += 1;
        Ok(())
    }
    fn charge(&mut self, bytes: usize) -> Result<()> {
        cancelled(self.cancel)?;
        if bytes > self.limits.bytes - self.bytes {
            return Err(limit());
        }
        self.bytes += bytes;
        Ok(())
    }
}

fn text(row: &rusqlite::Row<'_>, column: usize, budget: &mut Budget<'_>) -> Result<String> {
    let value = row.get_ref(column).map_err(db_error)?;
    let ValueRef::Text(bytes) = value else {
        return Err(corrupt());
    };
    budget.charge(bytes.len())?;
    Ok(std::str::from_utf8(bytes)
        .map_err(|_| corrupt())?
        .to_owned())
}

fn optional_text(
    row: &rusqlite::Row<'_>,
    column: usize,
    budget: &mut Budget<'_>,
) -> Result<Option<String>> {
    if row.get_ref(column).map_err(db_error)? == ValueRef::Null {
        return Ok(None);
    }
    text(row, column, budget).map(Some)
}

fn number(row: &rusqlite::Row<'_>, column: usize, budget: &mut Budget<'_>) -> Result<i64> {
    budget.charge(8)?;
    row.get(column).map_err(db_error)
}

fn index(row: &rusqlite::Row<'_>, column: usize, budget: &mut Budget<'_>) -> Result<usize> {
    usize::try_from(number(row, column, budget)?).map_err(|_| corrupt())
}

fn boolean(row: &rusqlite::Row<'_>, column: usize, budget: &mut Budget<'_>) -> Result<bool> {
    match number(row, column, budget)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(corrupt()),
    }
}

fn json<T: DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|_| corrupt())
}

fn collect<T>(
    connection: &Connection,
    sql: &str,
    budget: &mut Budget<'_>,
    mut map: impl FnMut(&rusqlite::Row<'_>, &mut Budget<'_>) -> Result<T>,
) -> Result<Vec<T>> {
    let mut statement = connection.prepare(sql).map_err(db_error)?;
    let mut rows = statement.query([]).map_err(db_error)?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().map_err(db_error)? {
        budget.row()?;
        result.push(map(row, budget)?);
    }
    Ok(result)
}

fn check_tables(connection: &Connection, version: i64, cancel: &AtomicBool) -> Result<()> {
    let mut required = BTreeSet::from([
        "accounts",
        "categories",
        "revisions",
        "selected",
        "outbox",
        "candidates",
        "metadata",
    ]);
    if version >= 2 {
        required.extend([
            "csv_context",
            "csv_tasks",
            "csv_rows",
            "csv_sources",
            "csv_batches",
        ]);
    }
    if version == 3 {
        required.extend(["csv_undo_tasks", "csv_undo_rows", "csv_undo_batches"]);
    }
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .map_err(db_error)?;
    let mut rows = statement.query([]).map_err(db_error)?;
    while let Some(row) = rows.next().map_err(db_error)? {
        cancelled(cancel)?;
        let value = row.get_ref(0).map_err(db_error)?;
        let name = value.as_str().map_err(|_| corrupt())?;
        if name.starts_with("sqlite_") || matches!(name, "bootstrap_identity" | "bootstrap_fixture")
        {
            continue;
        }
        if !required.remove(name) {
            return Err(CoreError::new(
                "unsupported_snapshot_schema",
                "账库含未声明的业务表，停止快照以避免遗漏",
            ));
        }
        check_columns(connection, name, cancel)?;
    }
    if !required.is_empty() {
        return Err(corrupt());
    }
    Ok(())
}

fn check_columns(connection: &Connection, table: &str, cancel: &AtomicBool) -> Result<()> {
    // 只对上方固定声明的业务表检查布局；input 列存在但不读取其文件副本。
    let columns = match table {
        "accounts" | "categories" => "id,kind,name,archived",
        "revisions" => "id,event_id,predecessor,payload",
        "selected" => "event_id,revision_id",
        "outbox" => "operation_id,intent,revision_id",
        "candidates" => "revision_id,event_id",
        "metadata" => "id,sequence",
        "csv_context" => "id,target_id",
        "csv_tasks" => {
            "id,target_id,profile,input,input_fingerprint,mapping_fingerprint,request,namespace,state,total,next_record,batch_size"
        }
        "csv_rows" => "task_id,record,row_id,source_id,intent,outcome",
        "csv_sources" => "namespace,source_id,payload,event_id,revision_id",
        "csv_batches" => "task_id,first_record,last_record,imported,skipped",
        "csv_undo_tasks" => "id,import_task_id,request,state,total,next_ordinal,batch_size",
        "csv_undo_rows" => "undo_id,ordinal,record,row_id,intent,outcome",
        "csv_undo_batches" => "undo_id,first_ordinal,last_ordinal,deleted",
        _ => return Err(corrupt()),
    };
    let mut expected = columns.split(',');
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(db_error)?;
    let mut rows = statement.query([]).map_err(db_error)?;
    while let Some(row) = rows.next().map_err(db_error)? {
        cancelled(cancel)?;
        let name = row.get_ref(1).map_err(db_error)?;
        if expected.next() != Some(name.as_str().map_err(|_| corrupt())?) {
            return Err(CoreError::new(
                "unsupported_snapshot_schema",
                "业务表含未声明字段或布局，停止快照以避免遗漏",
            ));
        }
    }
    if expected.next().is_some() {
        return Err(corrupt());
    }
    Ok(())
}

fn references(
    connection: &Connection,
    sql: &str,
    budget: &mut Budget<'_>,
) -> Result<Vec<Reference>> {
    collect(connection, sql, budget, |r, b| {
        Ok(Reference {
            id: text(r, 0, b)?,
            kind: text(r, 1, b)?,
            name: text(r, 2, b)?,
            archived: boolean(r, 3, b)?,
        })
    })
}

fn selections(
    connection: &Connection,
    sql: &str,
    budget: &mut Budget<'_>,
) -> Result<Vec<Selection>> {
    collect(connection, sql, budget, |r, b| {
        Ok(Selection {
            event_id: text(r, 0, b)?,
            revision_id: text(r, 1, b)?,
        })
    })
}

fn imports(
    connection: &Connection,
    version: i64,
    budget: &mut Budget<'_>,
) -> Result<ImportHistory> {
    let context = collect(
        connection,
        "SELECT id,target_id FROM csv_context ORDER BY id",
        budget,
        |r, b| Ok((number(r, 0, b)?, text(r, 1, b)?)),
    )?;
    let [(1, target)] = context.as_slice() else {
        return Err(corrupt());
    };
    let tasks = collect(
        connection,
        "SELECT id,target_id,profile,input_fingerprint,mapping_fingerprint,request,namespace,state,total,next_record,batch_size FROM csv_tasks ORDER BY id",
        budget,
        |r, b| {
            let id = text(r, 0, b)?;
            if text(r, 1, b)? != *target {
                return Err(corrupt());
            }
            let profile = text(r, 2, b)?;
            let input_fingerprint = text(r, 3, b)?;
            let mapping_fingerprint = text(r, 4, b)?;
            let request: (String, String, Mapping, Vec<Decision>, usize) = json(&text(r, 5, b)?)?;
            let namespace = text(r, 6, b)?;
            let state = match text(r, 7, b)?.as_str() {
                "ready" => TaskState::Ready,
                "cancelled" => TaskState::Cancelled,
                "complete" => TaskState::Complete,
                _ => return Err(corrupt()),
            };
            let total = index(r, 8, b)?;
            let next_record = index(r, 9, b)?;
            let batch_size = index(r, 10, b)?;
            if request.0 != profile
                || request.1 != input_fingerprint
                || request.2.source_namespace != namespace
                || request.4 != batch_size
            {
                return Err(corrupt());
            }
            Ok(ImportTask {
                id,
                profile,
                input_fingerprint,
                mapping_fingerprint,
                mapping: request.2,
                decisions: request.3,
                state,
                total,
                next_record,
                batch_size,
            })
        },
    )?;
    let rows = collect(
        connection,
        "SELECT task_id,record,row_id,source_id,intent,outcome FROM csv_rows ORDER BY task_id,record",
        budget,
        |r, b| {
            Ok(ImportRow {
                task_id: text(r, 0, b)?,
                record: index(r, 1, b)?,
                row_id: text(r, 2, b)?,
                source_id: optional_text(r, 3, b)?,
                intent: optional_text(r, 4, b)?.map(|s| json(&s)).transpose()?,
                outcome: match text(r, 5, b)?.as_str() {
                    "pending" => ImportOutcome::Pending,
                    "imported" => ImportOutcome::Imported,
                    "skipped" => ImportOutcome::Skipped,
                    _ => return Err(corrupt()),
                },
            })
        },
    )?;
    let sources = collect(
        connection,
        "SELECT namespace,source_id,payload,event_id,revision_id FROM csv_sources ORDER BY namespace,source_id",
        budget,
        |r, b| {
            Ok(Source {
                namespace: text(r, 0, b)?,
                source_id: text(r, 1, b)?,
                event: json(&text(r, 2, b)?)?,
                event_id: text(r, 3, b)?,
                revision_id: text(r, 4, b)?,
            })
        },
    )?;
    let batches = collect(
        connection,
        "SELECT task_id,first_record,last_record,imported,skipped FROM csv_batches ORDER BY task_id,first_record",
        budget,
        |r, b| {
            Ok(ImportBatch {
                task_id: text(r, 0, b)?,
                first_record: index(r, 1, b)?,
                last_record: index(r, 2, b)?,
                imported: index(r, 3, b)?,
                skipped: index(r, 4, b)?,
            })
        },
    )?;
    let mut history = ImportHistory {
        source_target_id: target.clone(),
        tasks,
        rows,
        sources,
        batches,
        undo_tasks: Vec::new(),
        undo_rows: Vec::new(),
        undo_batches: Vec::new(),
    };
    if version == 3 {
        read_undos(connection, &mut history, budget)?;
    }
    Ok(history)
}

fn read_undos(
    connection: &Connection,
    history: &mut ImportHistory,
    budget: &mut Budget<'_>,
) -> Result<()> {
    history.undo_tasks = collect(
        connection,
        "SELECT id,import_task_id,request,state,total,next_ordinal,batch_size FROM csv_undo_tasks ORDER BY id",
        budget,
        |r, b| {
            let id = text(r, 0, b)?;
            let import_task_id = text(r, 1, b)?;
            let (profile, source, origins, batch): (String, String, Vec<Origin>, usize) =
                json(&text(r, 2, b)?)?;
            let state = match text(r, 3, b)?.as_str() {
                "ready" => State::Ready,
                "cancelled" => State::Cancelled,
                "complete" => State::Complete,
                _ => return Err(corrupt()),
            };
            let total = index(r, 4, b)?;
            let next_ordinal = index(r, 5, b)?;
            let batch_size = index(r, 6, b)?;
            if profile != "m0-csv-undo-v1" || source != import_task_id || batch != batch_size {
                return Err(corrupt());
            }
            Ok(UndoTask {
                id,
                import_task_id,
                selection: origins
                    .into_iter()
                    .map(|o| UndoSelection {
                        record: o.record,
                        row_id: o.row_id,
                        event_id: o.event_id,
                        revision_id: o.revision_id,
                    })
                    .collect(),
                state,
                total,
                next_ordinal,
                batch_size,
            })
        },
    )?;
    history.undo_rows = collect(
        connection,
        "SELECT undo_id,ordinal,record,row_id,intent,outcome FROM csv_undo_rows ORDER BY undo_id,ordinal",
        budget,
        |r, b| {
            Ok(UndoRow {
                undo_id: text(r, 0, b)?,
                ordinal: index(r, 1, b)?,
                record: index(r, 2, b)?,
                row_id: text(r, 3, b)?,
                intent: json(&text(r, 4, b)?)?,
                deleted: match text(r, 5, b)?.as_str() {
                    "pending" => false,
                    "deleted" => true,
                    _ => return Err(corrupt()),
                },
            })
        },
    )?;
    history.undo_batches = collect(
        connection,
        "SELECT undo_id,first_ordinal,last_ordinal,deleted FROM csv_undo_batches ORDER BY undo_id,first_ordinal",
        budget,
        |r, b| {
            Ok(UndoBatch {
                undo_id: text(r, 0, b)?,
                first_ordinal: index(r, 1, b)?,
                last_ordinal: index(r, 2, b)?,
                deleted: index(r, 3, b)?,
            })
        },
    )?;
    Ok(())
}

impl Store {
    pub fn logical_snapshot(
        &self,
        limits: Limits,
        months: &[String],
        cancel: &AtomicBool,
    ) -> Result<LogicalSnapshot> {
        self.logical_snapshot_observed(limits, months, cancel, |_| Ok(()))
    }

    fn logical_snapshot_observed(
        &self,
        limits: Limits,
        months: &[String],
        cancel: &AtomicBool,
        mut observe: impl FnMut(CapturePoint) -> Result<()>,
    ) -> Result<LogicalSnapshot> {
        cancelled(cancel)?;
        if !(1..=MAX_ROWS).contains(&limits.rows)
            || !(1..=MAX_BYTES).contains(&limits.bytes)
            || months.len() > 120
        {
            return Err(limit());
        }
        for month in months {
            cancelled(cancel)?;
            if month.len() != 7 || crate::domain::validate_date(&format!("{month}-01")).is_err() {
                return Err(CoreError::new("invalid_calendar_date", "统计月份无效"));
            }
        }
        let mut budget = Budget {
            limits,
            rows: 0,
            bytes: 0,
            cancel,
        };
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        let version: i64 = tx
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db_error)?;
        if !matches!(version, 1..=3) {
            return Err(CoreError::new(
                "unsupported_snapshot_schema",
                "逻辑快照不支持此实验 schema",
            ));
        }
        budget.charge(8)?;
        check_tables(&tx, version, cancel)?;
        let metadata = collect(
            &tx,
            "SELECT id,sequence FROM metadata ORDER BY id",
            &mut budget,
            |r, b| Ok((number(r, 0, b)?, number(r, 1, b)?)),
        )?;
        let [(1, sequence)] = metadata.as_slice() else {
            return Err(corrupt());
        };
        if *sequence < 0 {
            return Err(corrupt());
        }
        observe(CapturePoint::Header)?;
        let accounts = references(
            &tx,
            "SELECT id,kind,name,archived FROM accounts ORDER BY id",
            &mut budget,
        )?;
        let categories = references(
            &tx,
            "SELECT id,kind,name,archived FROM categories ORDER BY id",
            &mut budget,
        )?;
        observe(CapturePoint::References)?;
        let revisions = collect(
            &tx,
            "SELECT id,event_id,predecessor,payload FROM revisions ORDER BY id",
            &mut budget,
            |r, b| {
                Ok(Revision {
                    id: text(r, 0, b)?,
                    event_id: text(r, 1, b)?,
                    predecessor: optional_text(r, 2, b)?,
                    event: optional_text(r, 3, b)?.map(|s| json(&s)).transpose()?,
                })
            },
        )?;
        observe(CapturePoint::Revisions)?;
        let selected = selections(
            &tx,
            "SELECT event_id,revision_id FROM selected ORDER BY event_id",
            &mut budget,
        )?;
        let candidates = selections(
            &tx,
            "SELECT event_id,revision_id FROM candidates ORDER BY revision_id",
            &mut budget,
        )?;
        observe(CapturePoint::Selections)?;
        let intent_history = collect(
            &tx,
            "SELECT operation_id,intent,revision_id FROM outbox ORDER BY operation_id",
            &mut budget,
            |r, b| {
                let operation = text(r, 0, b)?;
                let intent: Write = json(&text(r, 1, b)?)?;
                let revision = text(r, 2, b)?;
                if operation != intent.operation_id || revision != intent.revision_id {
                    return Err(corrupt());
                }
                Ok(intent)
            },
        )?;
        observe(CapturePoint::Intents)?;
        let imports = if version >= 2 {
            Some(imports(&tx, version, &mut budget)?)
        } else {
            None
        };
        observe(CapturePoint::Imports)?;
        let facts = Facts {
            profile: PROFILE.into(),
            source_schema: version,
            sequence: *sequence,
            currency: "CNY".into(),
            minor_unit_scale: 2,
            remote_knowledge: RemoteKnowledge::Unknown,
            accounts,
            categories,
            revisions,
            selected,
            candidates,
            intent_history,
            imports,
        };
        let verification = validate::verify(&facts, months, cancel)?;
        observe(CapturePoint::BeforeFinish)?;
        cancelled(cancel)?;
        tx.commit().map_err(db_error)?;
        Ok(LogicalSnapshot {
            contents: Contents {
                facts,
                verification,
            },
            captured_rows: budget.rows,
            captured_bytes: budget.bytes,
        })
    }
}

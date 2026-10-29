//! CSV 实验库的计划登记、来源绑定和批次发布；所有业务写入复用 apply_write。
use super::*;
use crate::csv_import::{
    Batch, Decision, PROFILE, Prepared, Preview, RowOutcome, SourceMatch, Task, TaskRow, TaskState,
    check_cancelled,
};
use std::sync::atomic::AtomicBool;

#[cfg(test)]
#[path = "csv_store_tests.rs"]
mod tests;

pub const MAX_BATCH: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImportPoint {
    AfterFact,
    AfterSource,
    BeforeProgress,
    BeforeCommit,
    AfterCommit,
}

fn encode<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|_| CoreError::new("encoding_error", "导入计划无法编码"))
}

fn target(connection: &Connection) -> Result<String> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(db_error)?;
    if version != 2 {
        return Err(CoreError::new(
            "unsupported_schema",
            "持久导入需要新建 v2 实验库，原库未升级",
        ));
    }
    let id: String = connection
        .query_row("SELECT target_id FROM csv_context WHERE id=1", [], |r| {
            r.get(0)
        })
        .map_err(db_error)?;
    valid_id(&id)?;
    Ok(id)
}

fn check_target(connection: &Connection, expected: &str) -> Result<()> {
    if target(connection)? != expected {
        return Err(CoreError::new(
            "csv_target_changed",
            "导入目标已变化，请返回原账库核对",
        ));
    }
    Ok(())
}

fn source(
    connection: &Connection,
    namespace: &str,
    id: &str,
) -> Result<Option<(String, String, String)>> {
    connection.query_row("SELECT payload,event_id,revision_id FROM csv_sources WHERE namespace=?1 AND source_id=?2", params![namespace, id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(db_error)
}

fn check_source(
    connection: &Connection,
    namespace: &str,
    id: Option<&str>,
    event: &Event,
) -> Result<()> {
    if let Some(id) = id
        && let Some((payload, _, _)) = source(connection, namespace, id)?
    {
        return Err(if payload == encode(event)? {
            CoreError::new("csv_duplicate_source", "来源已导入，请重新预览并明确跳过")
        } else {
            CoreError::new("csv_source_conflict", "来源身份已有不同内容，不能覆盖")
        });
    }
    Ok(())
}

fn index(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<usize> {
    row.get::<_, u32>(column).map(|value| value as usize)
}

fn read_task(connection: &Connection, id: &str) -> Result<Task> {
    let row: Option<(String,String,usize,usize,usize,String)> = connection.query_row(
        "SELECT target_id,state,total,next_record,batch_size,profile FROM csv_tasks WHERE id=?1", [id],
        |r| Ok((r.get(0)?,r.get(1)?,index(r,2)?,index(r,3)?,index(r,4)?,r.get(5)?))).optional().map_err(db_error)?;
    let (target_id, state, total, next_record, batch_size, profile) =
        row.ok_or_else(|| CoreError::new("csv_missing_task", "导入任务不存在"))?;
    if total == 0
        || total > 1000
        || next_record == 0
        || next_record > total + 1
        || !(1..=MAX_BATCH).contains(&batch_size)
    {
        return Err(CoreError::new("csv_corrupt_task", "导入任务范围无效"));
    }
    if profile != PROFILE {
        return Err(CoreError::new("csv_profile", "导入任务版本不受支持"));
    }
    let state = match state.as_str() {
        "ready" => TaskState::Ready,
        "cancelled" => TaskState::Cancelled,
        "complete" => TaskState::Complete,
        _ => return Err(CoreError::new("csv_corrupt_task", "导入任务状态无效")),
    };
    let count = |status: &str| -> Result<usize> {
        connection
            .query_row(
                "SELECT count(*) FROM csv_rows WHERE task_id=?1 AND outcome=?2",
                params![id, status],
                |r| index(r, 0),
            )
            .map_err(db_error)
    };
    check_target(connection, &target_id)?;
    let imported = count("imported")?;
    let skipped = count("skipped")?;
    let pending = count("pending")?;
    if imported + skipped != next_record - 1
        || pending != total + 1 - next_record
        || (state == TaskState::Complete) != (next_record == total + 1)
    {
        return Err(CoreError::new(
            "csv_corrupt_task",
            "任务进度与持久行结果不一致",
        ));
    }
    Ok(Task {
        id: id.into(),
        target_id,
        state,
        total,
        next_record,
        batch_size,
        imported,
        skipped,
    })
}

impl Store {
    /// 仅新建 v2 隔离实验库；不升级 / 替换既有 v1，失败保留已创建文件供核对。
    pub fn create_csv_experiment(path: &Path, key: &DbKey) -> Result<Self> {
        let mut store = Self::create(path, key)?;
        let tx = store
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        tx.execute_batch("CREATE TABLE csv_context (id INTEGER PRIMARY KEY CHECK(id=1), target_id TEXT NOT NULL UNIQUE);
            CREATE TABLE csv_tasks (id TEXT PRIMARY KEY, target_id TEXT NOT NULL, profile TEXT NOT NULL,
                input BLOB NOT NULL, input_fingerprint TEXT NOT NULL, mapping_fingerprint TEXT NOT NULL,
                request TEXT NOT NULL, namespace TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('ready','cancelled','complete')),
                total INTEGER NOT NULL CHECK(total BETWEEN 1 AND 1000), next_record INTEGER NOT NULL CHECK(next_record BETWEEN 1 AND total+1),
                batch_size INTEGER NOT NULL CHECK(batch_size BETWEEN 1 AND 100));
            CREATE TABLE csv_rows (task_id TEXT NOT NULL REFERENCES csv_tasks(id), record INTEGER NOT NULL,
                row_id TEXT NOT NULL UNIQUE, source_id TEXT, intent TEXT, outcome TEXT NOT NULL CHECK(outcome IN ('pending','imported','skipped')),
                PRIMARY KEY(task_id,record));
            CREATE TABLE csv_sources (namespace TEXT NOT NULL, source_id TEXT NOT NULL, payload TEXT NOT NULL,
                event_id TEXT NOT NULL, revision_id TEXT NOT NULL REFERENCES revisions(id), PRIMARY KEY(namespace,source_id));
            CREATE TABLE csv_batches (task_id TEXT NOT NULL REFERENCES csv_tasks(id), first_record INTEGER NOT NULL,
                last_record INTEGER NOT NULL, imported INTEGER NOT NULL, skipped INTEGER NOT NULL, PRIMARY KEY(task_id,first_record));
            PRAGMA user_version=2;").map_err(db_error)?;
        tx.execute(
            "INSERT INTO csv_context VALUES(1,?1)",
            [crate::random_id()?],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(store)
    }

    pub(crate) fn inspect_csv_preview(
        &self,
        preview: &mut Preview,
        namespace: &str,
    ) -> Result<(String, Vec<String>)> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        let id = target(&tx)?;
        let sequence: i64 = tx
            .query_row("SELECT sequence FROM metadata WHERE id=1", [], |r| r.get(0))
            .map_err(db_error)?;
        if sequence != preview.observed_sequence {
            return Err(CoreError::new(
                "stale_observation",
                "预览期间账库已变化，请重新预览",
            ));
        }
        for row in &mut preview.rows {
            if let (Some(source_id), Some(candidate)) = (&row.source_id, &row.candidate)
                && let Some((payload, event_id, revision_id)) = source(&tx, namespace, source_id)?
            {
                if payload == encode(&candidate.fact())? {
                    row.source_match = SourceMatch::PreviouslyImported {
                        event_id,
                        revision_id,
                    };
                } else {
                    row.errors.push(CoreError::new(
                        "csv_source_conflict",
                        "来源身份已有不同内容，不能覆盖",
                    ));
                }
            }
        }
        // 同文件即报告已有任务，映射改变也不隐藏历史导入范围。
        let history = tx
            .prepare("SELECT id FROM csv_tasks WHERE input_fingerprint=?1 ORDER BY id")
            .map_err(db_error)?
            .query_map([&preview.input_fingerprint], |r| r.get(0))
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<String>, _>>()
            .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok((id, history))
    }

    /// task_id 由调用方在确认前保存；未知结果使用同一身份、同一计划重试。
    pub fn save_csv_task(
        &mut self,
        id: &str,
        plan: &Prepared,
        decisions: &[Decision],
        batch_size: usize,
        cancelled: &AtomicBool,
    ) -> Result<Task> {
        valid_id(id)?;
        check_cancelled(cancelled)?;
        if decisions.len() != plan.preview.rows.len() || !(1..=MAX_BATCH).contains(&batch_size) {
            return Err(CoreError::new(
                "csv_plan",
                "须逐行明确选择并使用 1 至 100 行批次",
            ));
        }
        let request = encode(&(
            PROFILE,
            &plan.preview.input_fingerprint,
            &plan.mapping,
            decisions,
            batch_size,
        ))?;
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        check_target(&tx, &plan.target_id)?;
        let prior: Option<(String, Vec<u8>)> = tx
            .query_row(
                "SELECT request,input FROM csv_tasks WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(db_error)?;
        if let Some((prior, input)) = prior {
            if prior != request || input != plan.bytes {
                return Err(CoreError::new(
                    "operation_reused",
                    "同一导入任务身份不能绑定新计划",
                ));
            }
            return read_task(&tx, id);
        }
        let sequence: i64 = tx
            .query_row("SELECT sequence FROM metadata WHERE id=1", [], |r| r.get(0))
            .map_err(db_error)?;
        if sequence != plan.preview.observed_sequence {
            return Err(CoreError::new(
                "stale_observation",
                "预览已过期，请重新核对",
            ));
        }
        tx.execute(
            "INSERT INTO csv_tasks VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'ready',?9,1,?10)",
            params![
                id,
                plan.target_id,
                PROFILE,
                plan.bytes,
                plan.preview.input_fingerprint,
                plan.preview.mapping_fingerprint,
                request,
                plan.mapping.source_namespace,
                decisions.len() as u32,
                batch_size as u32
            ],
        )
        .map_err(db_error)?;
        let mut sources = BTreeSet::new();
        for (row, decision) in plan.preview.rows.iter().zip(decisions) {
            check_cancelled(cancelled)?;
            let intent = if *decision == Decision::Import {
                if !row.errors.is_empty() {
                    return Err(CoreError::new(
                        "csv_invalid_row",
                        "所选记录仍有错误，请修正或明确跳过",
                    ));
                }
                let event = row
                    .candidate
                    .as_ref()
                    .ok_or_else(|| CoreError::new("csv_invalid_row", "所选记录没有有效候选"))?
                    .fact();
                validate_references(&tx, &event)?;
                check_source(
                    &tx,
                    &plan.mapping.source_namespace,
                    row.source_id.as_deref(),
                    &event,
                )?;
                if let Some(source) = &row.source_id
                    && !sources.insert(source)
                {
                    return Err(CoreError::new(
                        "csv_duplicate_source",
                        "同一来源重复行须明确选择一条并跳过其余行",
                    ));
                }
                Some(encode(&Write {
                    operation_id: crate::random_id()?,
                    event_id: crate::random_id()?,
                    revision_id: crate::random_id()?,
                    previous_revision_id: None,
                    event: Some(event),
                    expected_sequence: None,
                    observed_balance_minor: None,
                    target_balance_minor: None,
                    resolve_candidates: BTreeSet::new(),
                })?)
            } else {
                None
            };
            tx.execute(
                "INSERT INTO csv_rows VALUES (?1,?2,?3,?4,?5,'pending')",
                params![
                    id,
                    row.record as u32,
                    crate::random_id()?,
                    row.source_id,
                    intent
                ],
            )
            .map_err(db_error)?;
        }
        check_cancelled(cancelled)?;
        let task = read_task(&tx, id)?;
        tx.commit().map_err(db_error)?;
        Ok(task)
    }

    pub fn csv_task(&self, target_id: &str, id: &str) -> Result<Task> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        check_target(&tx, target_id)?;
        let task = read_task(&tx, id)?;
        tx.commit().map_err(db_error)?;
        Ok(task)
    }

    /// 列出任务是恢复入口，不自动继续任何任务。
    pub fn csv_tasks(&self) -> Result<Vec<Task>> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        target(&tx)?;
        let ids = tx
            .prepare("SELECT id FROM csv_tasks ORDER BY id")
            .map_err(db_error)?
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        let tasks = ids
            .iter()
            .map(|id| read_task(&tx, id))
            .collect::<Result<_>>()?;
        tx.commit().map_err(db_error)?;
        Ok(tasks)
    }

    pub fn csv_task_rows(&self, target_id: &str, id: &str) -> Result<Vec<TaskRow>> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        check_target(&tx, target_id)?;
        read_task(&tx, id)?;
        let rows = tx.prepare("SELECT record,row_id,intent,outcome FROM csv_rows WHERE task_id=?1 ORDER BY record").map_err(db_error)?
            .query_map([id], |r| Ok((index(r,0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?))).map_err(db_error)?
            .collect::<std::result::Result<Vec<_>,_>>().map_err(db_error)?;
        let mut result = Vec::new();
        for (record, row_id, intent, outcome) in rows {
            let write = intent.map(|s| decode_write(&s)).transpose()?;
            result.push(TaskRow {
                record,
                row_id,
                event_id: write.as_ref().map(|w| w.event_id.clone()),
                revision_id: write.as_ref().map(|w| w.revision_id.clone()),
                operation_id: write.map(|w| w.operation_id),
                outcome: match outcome.as_str() {
                    "pending" => RowOutcome::Pending,
                    "imported" => RowOutcome::Imported,
                    "skipped" => RowOutcome::Skipped,
                    _ => return Err(CoreError::new("csv_corrupt_task", "行处理状态无效")),
                },
            });
        }
        tx.commit().map_err(db_error)?;
        Ok(result)
    }

    /// cancel=true 停止后续批次；false 为显式恢复。已完成事实不撤回。
    pub fn set_csv_cancelled(&mut self, target_id: &str, id: &str, cancel: bool) -> Result<Task> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        check_target(&tx, target_id)?;
        let task = read_task(&tx, id)?;
        if task.state != TaskState::Complete {
            tx.execute(
                "UPDATE csv_tasks SET state=?2 WHERE id=?1",
                params![id, if cancel { "cancelled" } else { "ready" }],
            )
            .map_err(db_error)?;
        }
        let task = read_task(&tx, id)?;
        tx.commit().map_err(db_error)?;
        Ok(task)
    }

    pub fn publish_csv_batch(
        &mut self,
        target_id: &str,
        id: &str,
        first_record: usize,
        cancelled: &AtomicBool,
    ) -> Result<Batch> {
        self.publish_csv_observed(target_id, id, first_record, cancelled, |_| Ok(()))
    }

    pub(super) fn publish_csv_observed(
        &mut self,
        target_id: &str,
        id: &str,
        first_record: usize,
        cancelled: &AtomicBool,
        mut observe: impl FnMut(ImportPoint) -> Result<()>,
    ) -> Result<Batch> {
        if !(1..=1000).contains(&first_record) {
            return Err(CoreError::new("csv_batch_order", "批次起点超出输入边界"));
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        check_target(&tx, target_id)?;
        let task = read_task(&tx, id)?;
        let prior = tx.query_row("SELECT last_record,imported,skipped FROM csv_batches WHERE task_id=?1 AND first_record=?2",params![id,first_record as u32],|r| Ok(Batch {first_record,last_record:index(r,0)?,imported:index(r,1)?,skipped:index(r,2)?,replayed:true})).optional().map_err(db_error)?;
        if let Some(prior) = prior {
            return Ok(prior);
        }
        check_cancelled(cancelled)?;
        if task.state != TaskState::Ready {
            return Err(CoreError::new(
                "csv_task_stopped",
                "任务已停止或完成，请核对持久状态",
            ));
        }
        if first_record != task.next_record {
            return Err(CoreError::new(
                "csv_batch_order",
                "批次范围与已确认进度不符",
            ));
        }
        let last_record = (first_record + task.batch_size - 1).min(task.total);
        let namespace: String = tx
            .query_row("SELECT namespace FROM csv_tasks WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .map_err(db_error)?;
        let rows = tx.prepare("SELECT record,source_id,intent,outcome FROM csv_rows WHERE task_id=?1 AND record BETWEEN ?2 AND ?3 ORDER BY record").map_err(db_error)?
            .query_map(params![id,first_record as u32,last_record as u32], |r| Ok((index(r,0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?))).map_err(db_error)?
            .collect::<std::result::Result<Vec<_>,_>>().map_err(db_error)?;
        if rows.len() != last_record - first_record + 1 {
            return Err(CoreError::new("csv_corrupt_task", "批次记录不完整"));
        }
        let mut batch = Batch {
            first_record,
            last_record,
            imported: 0,
            skipped: 0,
            replayed: false,
        };
        for (record, source_id, intent, outcome) in rows {
            check_cancelled(cancelled)?;
            if outcome != "pending" {
                return Err(CoreError::new("csv_corrupt_task", "批次含不一致的已处理行"));
            }
            let status = if let Some(intent) = intent {
                let write = decode_write(&intent)?;
                let event = write
                    .event
                    .as_ref()
                    .ok_or_else(|| CoreError::new("csv_corrupt_task", "导入候选缺少事实"))?;
                check_source(&tx, &namespace, source_id.as_deref(), event)?;
                if apply_write(&tx, &write, &mut |_| Ok(()))? != WriteEffect::Committed {
                    return Err(CoreError::new(
                        "csv_corrupt_task",
                        "未完成行已有独立提交，停止自动恢复",
                    ));
                }
                observe(ImportPoint::AfterFact)?;
                if let Some(source_id) = source_id {
                    tx.execute(
                        "INSERT INTO csv_sources VALUES (?1,?2,?3,?4,?5)",
                        params![
                            namespace,
                            source_id,
                            encode(event)?,
                            write.event_id,
                            write.revision_id
                        ],
                    )
                    .map_err(db_error)?;
                }
                observe(ImportPoint::AfterSource)?;
                batch.imported += 1;
                "imported"
            } else {
                batch.skipped += 1;
                "skipped"
            };
            tx.execute(
                "UPDATE csv_rows SET outcome=?3 WHERE task_id=?1 AND record=?2",
                params![id, record as u32, status],
            )
            .map_err(db_error)?;
        }
        observe(ImportPoint::BeforeProgress)?;
        tx.execute(
            "INSERT INTO csv_batches VALUES (?1,?2,?3,?4,?5)",
            params![
                id,
                first_record as u32,
                last_record as u32,
                batch.imported as u32,
                batch.skipped as u32
            ],
        )
        .map_err(db_error)?;
        tx.execute(
            "UPDATE csv_tasks SET next_record=?2,state=?3 WHERE id=?1",
            params![
                id,
                (last_record + 1) as u32,
                if last_record == task.total {
                    "complete"
                } else {
                    "ready"
                }
            ],
        )
        .map_err(db_error)?;
        observe(ImportPoint::BeforeCommit)?;
        check_cancelled(cancelled)?;
        tx.commit().map_err(db_error)?;
        // 提交后不再按取消信号返回“未提交”；丢失响应由原批次回执查询恢复。
        observe(ImportPoint::AfterCommit)?;
        Ok(batch)
    }
}

fn decode_write(intent: &str) -> Result<Write> {
    let write: Write = serde_json::from_str(intent)
        .map_err(|_| CoreError::new("csv_corrupt_task", "导入意图无法解释"))?;
    if write.previous_revision_id.is_some()
        || write.expected_sequence.is_some()
        || write.observed_balance_minor.is_some()
        || write.target_balance_minor.is_some()
        || !write.resolve_candidates.is_empty()
        || !write.event.as_ref().is_some_and(|event| {
            matches!(
                event.kind,
                EventKind::Income | EventKind::Expense | EventKind::Transfer | EventKind::Refund
            )
        })
    {
        return Err(CoreError::new(
            "csv_corrupt_task",
            "导入意图超出新交易候选边界",
        ));
    }
    Ok(write)
}

//! 撤销任务、原身份恢复和有界删除批次；复用正常账务写入与 SQLCipher 事务。
use super::*;
use crate::csv_import::check_cancelled;
use crate::csv_undo::{Batch, Origin, Preview, Restriction, Row, State, Task, TaskRow};
use csv_store::{MAX_BATCH, check_target, decode_write, encode, index, read_task};
use std::sync::atomic::AtomicBool;

#[cfg(test)]
#[path = "csv_undo_tests.rs"]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UndoPoint {
    AfterDelete,
    AfterRow,
    BeforeProgress,
    BeforeCommit,
    AfterCommit,
}

fn corrupt() -> CoreError {
    CoreError::new(
        "csv_corrupt_undo",
        "撤销任务或原导入证据不一致，停止自动处理",
    )
}

fn stale() -> CoreError {
    CoreError::new(
        "csv_undo_stale",
        "撤销前置条件已变化，请重新预览并确认可处理集合",
    )
}

fn check_schema(connection: &Connection) -> Result<()> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(db_error)?;
    if version != 3 {
        return Err(CoreError::new(
            "unsupported_schema",
            "受限撤销需要新建 v3 实验库，原库未升级",
        ));
    }
    Ok(())
}

fn inspect_row(tx: &Transaction<'_>, task: &str, record: usize) -> Result<(Row, Option<Origin>)> {
    let (row_id, intent, outcome, source_id): (String, Option<String>, String, Option<String>) = tx
        .query_row(
            "SELECT row_id,intent,outcome,source_id FROM csv_rows WHERE task_id=?1 AND record=?2",
            params![task, record as u32],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(db_error)?;
    valid_id(&row_id)?;
    let original = intent.as_deref().map(decode_write).transpose()?;
    let mut row = Row {
        record,
        row_id: row_id.clone(),
        event_id: original.as_ref().map(|w| w.event_id.clone()),
        original_revision_id: original.as_ref().map(|w| w.revision_id.clone()),
        restriction: None,
    };
    match outcome.as_str() {
        "pending" => row.restriction = Some(Restriction::Pending),
        "skipped" => row.restriction = Some(Restriction::Skipped),
        "imported" => {}
        _ => return Err(corrupt()),
    }
    if row.restriction.is_some() {
        return Ok((row, None));
    }
    let write = original.ok_or_else(corrupt)?;
    for id in [&write.operation_id, &write.event_id, &write.revision_id] {
        valid_id(id)?;
    }
    let evidence: (String, Option<String>, Option<String>) = tx
        .query_row(
            "SELECT event_id,predecessor,payload FROM revisions WHERE id=?1",
            [&write.revision_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(db_error)?;
    let event = write.event.as_ref().ok_or_else(corrupt)?;
    let payload = evidence.2.ok_or_else(corrupt)?;
    let fact: Event = serde_json::from_str(&payload).map_err(|_| corrupt())?;
    if evidence.0 != write.event_id || evidence.1.is_some() || &fact != event {
        return Err(corrupt());
    }
    // Event 拒绝未知字段；当前仅有独立交易与账户 / 分类引用，不推断未建模的关联。
    event.validate()?;
    if let Some(source_id) = source_id {
        let binding: (String, String, String) = tx.query_row(
            "SELECT s.payload,s.event_id,s.revision_id FROM csv_sources s JOIN csv_tasks t ON t.namespace=s.namespace WHERE t.id=?1 AND s.source_id=?2",
            params![task, source_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(db_error)?;
        if binding
            != (
                encode(event)?,
                write.event_id.clone(),
                write.revision_id.clone(),
            )
        {
            return Err(corrupt());
        }
    }
    let (selected, selected_event, selected_payload): (String, String, Option<String>) = tx.query_row(
        "SELECT s.revision_id,r.event_id,r.payload FROM selected s JOIN revisions r ON r.id=s.revision_id WHERE s.event_id=?1",
        [&write.event_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(db_error)?;
    if selected_event != write.event_id {
        return Err(corrupt());
    }
    let undone: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM csv_undo_rows WHERE row_id=?1 AND outcome='deleted')",
            [&row_id],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    let history: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM revisions WHERE event_id=?1 AND id<>?2)",
            params![write.event_id, write.revision_id],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    row.restriction = if undone {
        Some(Restriction::AlreadyUndone)
    } else if !candidate_ids(tx, &write.event_id)?.is_empty() {
        Some(Restriction::ConflictCandidates)
    } else if selected_payload.is_none() {
        Some(Restriction::Deleted)
    } else if selected != write.revision_id {
        Some(Restriction::ChangedRevision)
    } else if history {
        Some(Restriction::DependentHistory)
    } else {
        None
    };
    Ok((
        row,
        Some(Origin {
            record,
            row_id,
            event_id: write.event_id,
            revision_id: write.revision_id,
        }),
    ))
}

fn read_undo(connection: &Connection, id: &str) -> Result<Task> {
    check_schema(connection)?;
    let data: Option<(String, String, usize, usize, usize)> = connection.query_row(
        "SELECT import_task_id,state,total,next_ordinal,batch_size FROM csv_undo_tasks WHERE id=?1", [id],
        |r| Ok((r.get(0)?, r.get(1)?, index(r,2)?, index(r,3)?, index(r,4)?))).optional().map_err(db_error)?;
    let (import_task_id, state, total, next_ordinal, batch_size) =
        data.ok_or_else(|| CoreError::new("csv_missing_undo", "撤销任务不存在"))?;
    let import = read_task(connection, &import_task_id)?;
    let state = match state.as_str() {
        "ready" => State::Ready,
        "cancelled" => State::Cancelled,
        "complete" => State::Complete,
        _ => return Err(corrupt()),
    };
    let (rows, deleted, valid): (usize, usize, usize) = connection.query_row(
        "SELECT count(*),coalesce(sum(outcome='deleted'),0),coalesce(sum(ordinal BETWEEN 1 AND ?2 AND ((ordinal<?3 AND outcome='deleted') OR (ordinal>=?3 AND outcome='pending'))),0) FROM csv_undo_rows WHERE undo_id=?1",
        params![id,total as u32,next_ordinal as u32], |r| Ok((index(r,0)?,index(r,1)?,index(r,2)?))).map_err(db_error)?;
    if !(1..=1000).contains(&total)
        || !(1..=total + 1).contains(&next_ordinal)
        || !(1..=MAX_BATCH).contains(&batch_size)
        || rows != total
        || valid != total
        || deleted != next_ordinal - 1
        || (state == State::Complete) != (next_ordinal == total + 1)
    {
        return Err(corrupt());
    }
    Ok(Task {
        id: id.into(),
        target_id: import.target_id,
        import_task_id,
        state,
        total,
        next_ordinal,
        batch_size,
        deleted,
    })
}

fn read_rows(connection: &Connection, id: &str) -> Result<Vec<TaskRow>> {
    let rows = connection.prepare("SELECT ordinal,record,row_id,intent,outcome FROM csv_undo_rows WHERE undo_id=?1 ORDER BY ordinal").map_err(db_error)?
        .query_map([id], |r| Ok((index(r,0)?,index(r,1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?))).map_err(db_error)?
        .collect::<std::result::Result<Vec<_>,_>>().map_err(db_error)?;
    rows.into_iter()
        .map(|(ordinal, record, row_id, intent, outcome)| {
            let write: Write = serde_json::from_str(&intent).map_err(|_| corrupt())?;
            let parent = write.previous_revision_id.ok_or_else(corrupt)?;
            for id in [
                &row_id,
                &write.event_id,
                &write.operation_id,
                &write.revision_id,
                &parent,
            ] {
                valid_id(id)?;
            }
            if write.event.is_some()
                || write.expected_sequence.is_some()
                || write.observed_balance_minor.is_some()
                || write.target_balance_minor.is_some()
                || !write.resolve_candidates.is_empty()
                || !(1..=1000).contains(&record)
            {
                return Err(corrupt());
            }
            let deleted = match outcome.as_str() {
                "pending" => false,
                "deleted" => true,
                _ => return Err(corrupt()),
            };
            Ok(TaskRow {
                ordinal,
                record,
                row_id,
                event_id: write.event_id,
                original_revision_id: parent,
                operation_id: write.operation_id,
                revision_id: write.revision_id,
                deleted,
            })
        })
        .collect()
}

fn deletion(row: &TaskRow) -> Write {
    Write {
        operation_id: row.operation_id.clone(),
        event_id: row.event_id.clone(),
        revision_id: row.revision_id.clone(),
        previous_revision_id: Some(row.original_revision_id.clone()),
        event: None,
        expected_sequence: None,
        observed_balance_minor: None,
        target_balance_minor: None,
        resolve_candidates: BTreeSet::new(),
    }
}

impl Store {
    /// 新建 v3 实验库；复用 v2 导入布局，不升级或替换任何既有库。
    pub fn create_csv_undo_experiment(path: &Path, key: &DbKey) -> Result<Self> {
        let mut store = Self::create_csv_experiment(path, key)?;
        let tx = store
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        tx.execute_batch("CREATE TABLE csv_undo_tasks (id TEXT PRIMARY KEY, import_task_id TEXT NOT NULL REFERENCES csv_tasks(id),
            request TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('ready','cancelled','complete')),
            total INTEGER NOT NULL CHECK(total BETWEEN 1 AND 1000), next_ordinal INTEGER NOT NULL CHECK(next_ordinal BETWEEN 1 AND total+1),
            batch_size INTEGER NOT NULL CHECK(batch_size BETWEEN 1 AND 100));
            CREATE TABLE csv_undo_rows (undo_id TEXT NOT NULL REFERENCES csv_undo_tasks(id), ordinal INTEGER NOT NULL,
            record INTEGER NOT NULL, row_id TEXT NOT NULL REFERENCES csv_rows(row_id), intent TEXT NOT NULL,
            outcome TEXT NOT NULL CHECK(outcome IN ('pending','deleted')), PRIMARY KEY(undo_id,ordinal), UNIQUE(undo_id,record));
            CREATE INDEX csv_undo_row_history ON csv_undo_rows(row_id,outcome);
            CREATE TABLE csv_undo_batches (undo_id TEXT NOT NULL REFERENCES csv_undo_tasks(id), first_ordinal INTEGER NOT NULL,
            last_ordinal INTEGER NOT NULL, deleted INTEGER NOT NULL, PRIMARY KEY(undo_id,first_ordinal));
            PRAGMA user_version=3;").map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(store)
    }

    pub fn preview_csv_undo(
        &self,
        target_id: &str,
        import_task_id: &str,
        cancelled: &AtomicBool,
    ) -> Result<Preview> {
        self.preview_csv_undo_observed(target_id, import_task_id, cancelled, |_| Ok(()))
    }

    fn preview_csv_undo_observed(
        &self,
        target_id: &str,
        import_task_id: &str,
        cancelled: &AtomicBool,
        mut observe: impl FnMut(usize) -> Result<()>,
    ) -> Result<Preview> {
        check_cancelled(cancelled)?;
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        check_schema(&tx)?;
        check_target(&tx, target_id)?;
        let import = read_task(&tx, import_task_id)?;
        let mut rows = Vec::with_capacity(import.total);
        for record in 1..=import.total {
            check_cancelled(cancelled)?;
            rows.push(inspect_row(&tx, import_task_id, record)?.0);
            observe(record)?;
        }
        check_cancelled(cancelled)?;
        tx.commit().map_err(db_error)?;
        Ok(Preview {
            target_id: target_id.into(),
            import_task_id: import_task_id.into(),
            rows,
        })
    }

    /// records 是严格升序的显式确认集合；任务与每条删除身份先持久化，再逐批发布。
    pub fn save_csv_undo(
        &mut self,
        id: &str,
        preview: &Preview,
        records: &[usize],
        batch_size: usize,
        cancelled: &AtomicBool,
    ) -> Result<Task> {
        valid_id(id)?;
        check_cancelled(cancelled)?;
        if records.is_empty()
            || records.len() > 1000
            || !(1..=MAX_BATCH).contains(&batch_size)
            || records.windows(2).any(|w| w[0] >= w[1])
        {
            return Err(CoreError::new(
                "csv_undo_plan",
                "须明确选择可撤销记录并使用 1 至 100 行批次",
            ));
        }
        let mut origins = Vec::with_capacity(records.len());
        for record in records {
            let row = record
                .checked_sub(1)
                .and_then(|n| preview.rows.get(n))
                .filter(|r| r.record == *record && r.restriction.is_none())
                .ok_or_else(|| CoreError::new("csv_undo_plan", "确认集合包含预览外或受限记录"))?;
            origins.push(Origin {
                record: *record,
                row_id: row.row_id.clone(),
                event_id: row.event_id.clone().ok_or_else(corrupt)?,
                revision_id: row.original_revision_id.clone().ok_or_else(corrupt)?,
            });
        }
        let request = encode(&(
            "m0-csv-undo-v1",
            &preview.import_task_id,
            &origins,
            batch_size,
        ))?;
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        check_schema(&tx)?;
        check_target(&tx, &preview.target_id)?;
        read_task(&tx, &preview.import_task_id)?;
        let prior: Option<String> = tx
            .query_row(
                "SELECT request FROM csv_undo_tasks WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if let Some(prior) = prior {
            if prior != request {
                return Err(CoreError::new(
                    "operation_reused",
                    "同一撤销任务身份不能绑定新确认集合",
                ));
            }
            return read_undo(&tx, id);
        }
        // 登记也核对前置条件；后续提交仍再次核对，登记不等于授权未来版本。
        for origin in &origins {
            check_cancelled(cancelled)?;
            let (row, current) = inspect_row(&tx, &preview.import_task_id, origin.record)?;
            if row.restriction.is_some() || current.as_ref() != Some(origin) {
                return Err(stale());
            }
        }
        tx.execute(
            "INSERT INTO csv_undo_tasks VALUES (?1,?2,?3,'ready',?4,1,?5)",
            params![
                id,
                preview.import_task_id,
                request,
                origins.len() as u32,
                batch_size as u32
            ],
        )
        .map_err(db_error)?;
        for (n, origin) in origins.iter().enumerate() {
            check_cancelled(cancelled)?;
            let row = TaskRow {
                ordinal: n + 1,
                record: origin.record,
                row_id: origin.row_id.clone(),
                event_id: origin.event_id.clone(),
                original_revision_id: origin.revision_id.clone(),
                operation_id: crate::random_id()?,
                revision_id: crate::random_id()?,
                deleted: false,
            };
            tx.execute(
                "INSERT INTO csv_undo_rows VALUES (?1,?2,?3,?4,?5,'pending')",
                params![
                    id,
                    row.ordinal as u32,
                    row.record as u32,
                    row.row_id,
                    encode(&deletion(&row))?
                ],
            )
            .map_err(db_error)?;
        }
        check_cancelled(cancelled)?;
        let task = read_undo(&tx, id)?;
        tx.commit().map_err(db_error)?;
        Ok(task)
    }

    pub fn csv_undo_task(&self, target_id: &str, id: &str) -> Result<Task> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        check_target(&tx, target_id)?;
        let task = read_undo(&tx, id)?;
        tx.commit().map_err(db_error)?;
        Ok(task)
    }

    /// 恢复只列出持久任务，不调度写入；取消的任务需显式恢复。
    pub fn csv_undo_tasks(&self, target_id: &str, import_task_id: &str) -> Result<Vec<Task>> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        check_schema(&tx)?;
        check_target(&tx, target_id)?;
        read_task(&tx, import_task_id)?;
        let ids = tx
            .prepare("SELECT id FROM csv_undo_tasks WHERE import_task_id=?1 ORDER BY id")
            .map_err(db_error)?
            .query_map([import_task_id], |r| r.get::<_, String>(0))
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        let tasks = ids
            .iter()
            .map(|id| read_undo(&tx, id))
            .collect::<Result<_>>()?;
        tx.commit().map_err(db_error)?;
        Ok(tasks)
    }

    pub fn csv_undo_rows(&self, target_id: &str, id: &str) -> Result<Vec<TaskRow>> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        check_target(&tx, target_id)?;
        read_undo(&tx, id)?;
        let rows = read_rows(&tx, id)?;
        tx.commit().map_err(db_error)?;
        Ok(rows)
    }

    pub fn set_csv_undo_cancelled(
        &mut self,
        target_id: &str,
        id: &str,
        cancel: bool,
    ) -> Result<Task> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        check_target(&tx, target_id)?;
        let task = read_undo(&tx, id)?;
        if task.state != State::Complete {
            tx.execute(
                "UPDATE csv_undo_tasks SET state=?2 WHERE id=?1",
                params![id, if cancel { "cancelled" } else { "ready" }],
            )
            .map_err(db_error)?;
        }
        let task = read_undo(&tx, id)?;
        tx.commit().map_err(db_error)?;
        Ok(task)
    }

    pub fn publish_csv_undo_batch(
        &mut self,
        target_id: &str,
        id: &str,
        first_ordinal: usize,
        cancelled: &AtomicBool,
    ) -> Result<Batch> {
        self.publish_csv_undo_observed(target_id, id, first_ordinal, cancelled, |_| Ok(()))
    }

    fn publish_csv_undo_observed(
        &mut self,
        target_id: &str,
        id: &str,
        first_ordinal: usize,
        cancelled: &AtomicBool,
        mut observe: impl FnMut(UndoPoint) -> Result<()>,
    ) -> Result<Batch> {
        if !(1..=1000).contains(&first_ordinal) {
            return Err(CoreError::new("csv_undo_order", "撤销批次起点超出边界"));
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        check_target(&tx, target_id)?;
        let task = read_undo(&tx, id)?;
        let prior = tx.query_row("SELECT last_ordinal,deleted FROM csv_undo_batches WHERE undo_id=?1 AND first_ordinal=?2",params![id,first_ordinal as u32],|r| Ok(Batch {first_ordinal,last_ordinal:index(r,0)?,deleted:index(r,1)?,replayed:true})).optional().map_err(db_error)?;
        // 已完成回执优先于取消 / 当前修订；重试不能变成对恢复后新版本的再次删除。
        if let Some(prior) = prior {
            return Ok(prior);
        }
        check_cancelled(cancelled)?;
        if task.state != State::Ready {
            return Err(CoreError::new(
                "csv_undo_stopped",
                "撤销已停止或完成，请核对持久状态",
            ));
        }
        if first_ordinal != task.next_ordinal {
            return Err(CoreError::new("csv_undo_order", "撤销批次与已确认进度不符"));
        }
        let last_ordinal = (first_ordinal + task.batch_size - 1).min(task.total);
        let rows = read_rows(&tx, id)?;
        let batch_rows = &rows[first_ordinal - 1..last_ordinal];
        for row in batch_rows {
            check_cancelled(cancelled)?;
            let (current, origin) = inspect_row(&tx, &task.import_task_id, row.record)?;
            let expected = Origin {
                record: row.record,
                row_id: row.row_id.clone(),
                event_id: row.event_id.clone(),
                revision_id: row.original_revision_id.clone(),
            };
            if row.deleted || current.restriction.is_some() || origin.as_ref() != Some(&expected) {
                return Err(stale());
            }
        }
        for row in batch_rows {
            check_cancelled(cancelled)?;
            if apply_write(&tx, &deletion(row), &mut |_| Ok(()))? != WriteEffect::Committed {
                return Err(corrupt());
            }
            observe(UndoPoint::AfterDelete)?;
            tx.execute(
                "UPDATE csv_undo_rows SET outcome='deleted' WHERE undo_id=?1 AND ordinal=?2",
                params![id, row.ordinal as u32],
            )
            .map_err(db_error)?;
            observe(UndoPoint::AfterRow)?;
        }
        observe(UndoPoint::BeforeProgress)?;
        tx.execute(
            "INSERT INTO csv_undo_batches VALUES (?1,?2,?3,?4)",
            params![
                id,
                first_ordinal as u32,
                last_ordinal as u32,
                batch_rows.len() as u32
            ],
        )
        .map_err(db_error)?;
        tx.execute(
            "UPDATE csv_undo_tasks SET next_ordinal=?2,state=?3 WHERE id=?1",
            params![
                id,
                (last_ordinal + 1) as u32,
                if last_ordinal == task.total {
                    "complete"
                } else {
                    "ready"
                }
            ],
        )
        .map_err(db_error)?;
        observe(UndoPoint::BeforeCommit)?;
        check_cancelled(cancelled)?;
        tx.commit().map_err(db_error)?;
        observe(UndoPoint::AfterCommit)?;
        Ok(Batch {
            first_ordinal,
            last_ordinal,
            deleted: batch_rows.len(),
            replayed: false,
        })
    }
}

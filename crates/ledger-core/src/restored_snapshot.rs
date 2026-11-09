//! 已选择恢复库的单事务捕获。类型只在内存中使用，暂无归档编码。
use super::*;
use validate::{HistoryView, LedgerView};

pub(crate) struct LocalHistory {
    pub sequence: i64,
    pub intents: Vec<Write>,
}

pub(crate) struct InheritedHistory {
    pub source_schema: i64,
    pub committed: LocalHistory,
    pub imports: Option<ImportHistory>,
}

pub(crate) struct RestoredSnapshot {
    pub ledger: LedgerRows,
    pub inherited: InheritedHistory,
    pub current: LocalHistory,
    pub verification: Snapshot,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CapturePoint {
    Established,
    BeforeFinish,
}

pub(crate) fn capture(
    store: &Store,
    months: &[String],
    limits: ArchiveLimits,
    cancel: &AtomicBool,
    mut observe: impl FnMut(CapturePoint) -> Result<()>,
) -> Result<RestoredSnapshot> {
    cancelled(cancel)?;
    limits.validate()?;
    validate_months(months, cancel)?;
    let tx = store.connection.unchecked_transaction().map_err(db_error)?;
    if version(&tx)? != CAPTURE_VERSION {
        return Err(CoreError::new(
            "unsupported_snapshot_schema",
            "此物化布局未登记独立历史核对材料",
        ));
    }
    let mut budget = materialization_budget(limits, cancel);
    let metadata = collect(
        &tx,
        "SELECT id,sequence FROM metadata ORDER BY id",
        &mut budget,
        |row, budget| Ok((number(row, 0, budget)?, number(row, 1, budget)?)),
    )?;
    let [(1, sequence)] = metadata.as_slice() else {
        return Err(mismatch());
    };
    observe(CapturePoint::Established)?;
    for table in CONTENT_TABLES.into_iter().chain([
        "metadata",
        "materialization",
        "archived_revisions",
        "archived_events",
    ]) {
        check_columns(&tx, table, cancel)?;
    }
    let (history, _) = read_history(&tx, CAPTURE_VERSION, limits, &mut budget)?;
    check_inherited(&history, &mut budget)?;
    let ledger = read_ledger(&tx, &mut budget)?;
    let current = LocalHistory {
        sequence: *sequence,
        intents: intents(&tx, &mut budget)?,
    };
    if current.sequence < i64::try_from(current.intents.len()).map_err(|_| limit())?
        || current.intents.iter().any(|intent| {
            intent
                .expected_sequence
                .is_some_and(|expected| expected >= current.sequence)
        })
    {
        return Err(mismatch());
    }
    check_indexes(
        &tx,
        CAPTURE_VERSION,
        &history,
        &current.intents,
        &mut budget,
    )?;
    let inherited = InheritedHistory {
        source_schema: history.source_schema,
        committed: LocalHistory {
            sequence: history.sequence,
            intents: history.intents,
        },
        imports: history.imports,
    };
    let verification = validate::verify_segments(
        LedgerView {
            accounts: &ledger.accounts,
            categories: &ledger.categories,
            revisions: &ledger.revisions,
            selected: &ledger.selected,
            candidates: &ledger.candidates,
        },
        &[
            HistoryView {
                source_schema: Some(inherited.source_schema),
                sequence: inherited.committed.sequence,
                intents: &inherited.committed.intents,
                imports: inherited.imports.as_ref(),
            },
            HistoryView {
                source_schema: None,
                sequence: current.sequence,
                intents: &current.intents,
                imports: None,
            },
        ],
        months,
        cancel,
    )?;
    observe(CapturePoint::BeforeFinish)?;
    cancelled(cancel)?;
    tx.commit().map_err(db_error)?;
    Ok(RestoredSnapshot {
        ledger,
        inherited,
        current,
        verification,
    })
}

fn check_inherited(history: &StoredHistory, budget: &mut Budget<'_>) -> Result<()> {
    if !matches!(history.source_schema, 1..=3)
        || (history.source_schema >= 2) != history.imports.is_some()
        || (history.source_schema == 2
            && history.imports.as_ref().is_some_and(|imports| {
                !imports.undo_tasks.is_empty()
                    || !imports.undo_rows.is_empty()
                    || !imports.undo_batches.is_empty()
            }))
    {
        return Err(mismatch());
    }
    budget.rows(history.intents.len())?;
    // 历史所在的 SQL 行、两个段及继承 CSV 记录共用读取预算。
    budget.rows(3)?;
    if let Some(imports) = &history.imports {
        for count in [
            imports.tasks.len(),
            imports.rows.len(),
            imports.sources.len(),
            imports.batches.len(),
            imports.undo_tasks.len(),
            imports.undo_rows.len(),
            imports.undo_batches.len(),
        ] {
            budget.rows(count)?;
        }
    }
    Ok(())
}

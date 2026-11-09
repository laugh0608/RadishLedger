//! 已选择恢复库的单事务捕获。
use super::*;
use crate::archive_container::restored::{History as Segment, RestoredContents};

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
) -> Result<RestoredContents> {
    cancelled(cancel)?;
    limits.validate()?;
    validate_months(months, cancel)?;
    let tx = store.connection.unchecked_transaction().map_err(db_error)?;
    let layout = version(&tx)?;
    if !has_history_digest(layout) {
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
    check_capture_columns(&tx, cancel)?;
    let mut segments = read_segments(&tx, layout, limits, &mut budget)?.segments;
    charge_segments(&segments, &mut budget)?;
    let ledger = read_ledger(&tx, &mut budget)?;
    let local = intents(&tx, &mut budget)?;
    check_segment_indexes(&tx, &segments, &local, &mut budget)?;

    // 参考资料的变化已体现在当前事实中。重复恢复时不累计无写入的尾段。
    if matches!(segments.last(), Some(Segment::Restored { sequence: 0, intent_history }) if intent_history.is_empty())
    {
        segments.pop();
    }
    budget.row()?;
    segments.push(Segment::Restored {
        sequence: *sequence,
        intent_history: local,
    });
    let contents =
        RestoredContents::from_rows(ledger, segments, months, cancel).map_err(|error| {
            if error.code == "invalid_archive_payload" {
                mismatch()
            } else {
                error
            }
        })?;
    observe(CapturePoint::BeforeFinish)?;
    cancelled(cancel)?;
    tx.commit().map_err(db_error)?;
    Ok(contents)
}

pub(super) fn charge_segments(segments: &[Segment], budget: &mut Budget<'_>) -> Result<()> {
    budget.rows(segments.len().checked_add(1).ok_or_else(limit)?)?;
    for segment in segments {
        let view = segment.view();
        budget.rows(view.intents.len())?;
        if let Some(imports) = view.imports {
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
    }
    Ok(())
}

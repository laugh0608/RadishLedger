//! 0.2 候选沿用日常账务表，按段保存继承历史。
use super::*;
use crate::archive_container::restored::RestoredContents;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SegmentedHistory<S> {
    pub(super) segments: S,
    months: Vec<String>,
}

pub(crate) fn prepare_restored(
    path: &Path,
    key: &DbKey,
    ticket: &Ticket,
    device: &str,
    stage: &Ticket,
    can_initialize: bool,
) -> Result<Store> {
    prepare_layout(
        path,
        key,
        ticket,
        device,
        stage,
        can_initialize,
        RESTORED_VERSION,
    )
}

fn imports(segments: &[Segment]) -> Option<&ImportHistory> {
    segments.first().and_then(|first| first.view().imports)
}

fn operations(segments: &[Segment]) -> BTreeSet<&str> {
    let Some((first, prior)) = segments.split_first() else {
        return BTreeSet::new();
    };
    let first = first.view();
    let mut ids = archived_operations(first.intents, first.imports);
    ids.extend(
        prior
            .iter()
            .flat_map(|s| s.view().intents)
            .map(|i| i.operation_id.as_str()),
    );
    ids
}

pub(crate) fn write_restored(
    store: &mut Store,
    contents: &RestoredContents,
    limits: ArchiveLimits,
    cancel: &AtomicBool,
    before_commit: impl FnOnce() -> Result<()>,
) -> Result<bool> {
    let segments = &contents.facts.history;
    let history = SegmentedHistory {
        segments,
        months: contents
            .verification
            .projection
            .monthly_reports
            .keys()
            .cloned()
            .collect(),
    };
    let encoded = bounded(&history, limits, cancel)?;
    let ledger = &contents.facts.ledger;
    write_records(
        store,
        Materialization {
            ledger: validate::LedgerView {
                accounts: &ledger.accounts,
                categories: &ledger.categories,
                revisions: &ledger.revisions,
                selected: &ledger.selected,
                candidates: &ledger.candidates,
            },
            imports: imports(segments),
            operations: operations(segments),
            history: std::str::from_utf8(&encoded).map_err(|_| mismatch())?,
            digest: crate::crypto::hex(&Sha256::digest(contents.encode(limits, cancel)?)),
        },
        cancel,
        before_commit,
    )
}

pub(super) fn read_segments(
    connection: &Connection,
    layout: i64,
    limits: ArchiveLimits,
    budget: &mut Budget<'_>,
) -> Result<SegmentedHistory<Vec<Segment>>> {
    if layout == RESTORED_VERSION {
        let (history, _) = history_record(connection, layout, limits, budget)?;
        let history: SegmentedHistory<Vec<Segment>> = json(&history)?;
        // 旧写入之前至少已有 0.1 来源和一段 0.2 来源。
        if history.segments.len() < 2 {
            return Err(mismatch());
        }
        return Ok(history);
    }
    let (history, _) = read_history(connection, layout, limits, budget)?;
    Ok(SegmentedHistory {
        segments: vec![Segment::Legacy {
            source_schema: history.source_schema,
            sequence: history.sequence,
            intent_history: history.intents,
            imports: history.imports,
        }],
        months: history.months,
    })
}

pub(super) fn check_segment_indexes(
    connection: &Connection,
    segments: &[Segment],
    local: &[Write],
    budget: &mut Budget<'_>,
) -> Result<()> {
    check_identity_indexes(
        connection,
        RESTORED_VERSION,
        imports(segments),
        &operations(segments),
        local,
        budget,
    )
}

pub(crate) fn inspect_restored_ready(
    store: &Store,
    limits: ArchiveLimits,
    cancel: &AtomicBool,
) -> Result<(RestoredContents, [u8; 32])> {
    let tx = store.connection.unchecked_transaction().map_err(db_error)?;
    require_empty_queue(&tx)?;
    if version(&tx)? != RESTORED_VERSION {
        return Err(mismatch());
    }
    check_capture_columns(&tx, cancel)?;
    let (history, expected) = history_record(
        &tx,
        RESTORED_VERSION,
        limits,
        &mut materialization_budget(limits, cancel),
    )?;
    let history: SegmentedHistory<Vec<Segment>> = json(&history)?;
    let mut budget = materialization_budget(limits, cancel);
    let ledger = read_ledger(&tx, &mut budget)?;
    check_segment_indexes(&tx, &history.segments, &[], &mut budget)?;
    let contents = RestoredContents::from_rows(ledger, history.segments, &history.months, cancel)?;
    let digest: [u8; 32] = Sha256::digest(contents.encode(limits, cancel)?).into();
    if crate::crypto::hex(&digest) != expected {
        return Err(mismatch());
    }
    tx.commit().map_err(db_error)?;
    Ok((contents, digest))
}

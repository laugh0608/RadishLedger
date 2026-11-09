//! 捕获后的关系与保全检查；历史引用允许归档，不复用“新增交易”拒绝规则。
use super::*;
use crate::csv_import::PROFILE as CSV_PROFILE;
use crate::domain::{minor, project};
use sha2::{Digest, Sha256};

struct Index<'a> {
    accounts: BTreeMap<&'a str, &'a Reference>,
    categories: BTreeMap<&'a str, &'a Reference>,
    revisions: BTreeMap<&'a str, &'a Revision>,
    intents: BTreeMap<&'a str, &'a Write>,
    sequence: i64,
    schema: i64,
}

fn identity(id: &str) -> Result<()> {
    valid_id(id).map_err(|_| corrupt())
}

fn event(event: &Event, index: &Index<'_>) -> Result<()> {
    event.validate().map_err(|_| corrupt())?;
    if index.schema == 1 && !event.note.is_empty() {
        return Err(corrupt());
    }
    for id in [
        &event.account_id,
        &event.from_account_id,
        &event.to_account_id,
    ]
    .into_iter()
    .flatten()
    {
        identity(id)?;
        if !index.accounts.contains_key(id.as_str()) {
            return Err(corrupt());
        }
    }
    if let Some(id) = &event.category_id {
        identity(id)?;
        let reference = index.categories.get(id.as_str()).ok_or_else(corrupt)?;
        let expected = if event.kind == EventKind::Income {
            "income"
        } else {
            "expense"
        };
        if reference.kind != expected {
            return Err(corrupt());
        }
    }
    Ok(())
}

fn intent(write: &Write, index: &Index<'_>, applied: bool) -> Result<()> {
    for id in [&write.operation_id, &write.event_id, &write.revision_id] {
        identity(id)?;
    }
    if let Some(parent) = &write.previous_revision_id {
        identity(parent)?;
        if index
            .revisions
            .get(parent.as_str())
            .ok_or_else(corrupt)?
            .event_id
            != write.event_id
        {
            return Err(corrupt());
        }
    }
    for candidate in &write.resolve_candidates {
        identity(candidate)?;
        if index
            .revisions
            .get(candidate.as_str())
            .ok_or_else(corrupt)?
            .event_id
            != write.event_id
        {
            return Err(corrupt());
        }
    }
    if write
        .expected_sequence
        .is_some_and(|n| n < 0 || n > index.sequence)
    {
        return Err(corrupt());
    }
    if let Some(fact) = &write.event {
        event(fact, index)?;
        if fact.kind == EventKind::Adjustment {
            let observed = minor(
                write
                    .observed_balance_minor
                    .as_deref()
                    .ok_or_else(corrupt)?,
            )
            .map_err(|_| corrupt())?;
            let target = minor(write.target_balance_minor.as_deref().ok_or_else(corrupt)?)
                .map_err(|_| corrupt())?;
            if write.expected_sequence.is_none()
                || i128::from(target) - i128::from(observed)
                    != i128::from(minor(&fact.amount_minor).map_err(|_| corrupt())?)
            {
                return Err(corrupt());
            }
        }
    } else if write.previous_revision_id.is_none() {
        return Err(corrupt());
    }
    if applied {
        let revision = index
            .revisions
            .get(write.revision_id.as_str())
            .ok_or_else(corrupt)?;
        if revision.event_id != write.event_id
            || revision.predecessor != write.previous_revision_id
            || revision.event != write.event
        {
            return Err(corrupt());
        }
        if index.intents.get(write.operation_id.as_str()).copied() != Some(write) {
            return Err(corrupt());
        }
    } else if index.revisions.contains_key(write.revision_id.as_str())
        || index.intents.contains_key(write.operation_id.as_str())
    {
        return Err(corrupt());
    }
    Ok(())
}

fn check_graph(index: &Index<'_>, cancel: &AtomicBool) -> Result<()> {
    let positions: BTreeMap<&str, usize> = index
        .revisions
        .keys()
        .enumerate()
        .map(|(n, id)| (*id, n))
        .collect();
    let revisions: Vec<_> = index.revisions.values().copied().collect();
    let mut states = vec![0_u8; revisions.len()];
    for start in 0..revisions.len() {
        cancelled(cancel)?;
        let mut current = Some(start);
        let mut path = Vec::new();
        while let Some(n) = current {
            cancelled(cancel)?;
            match states[n] {
                2 => break,
                1 => return Err(corrupt()),
                _ => {}
            }
            states[n] = 1;
            path.push(n);
            current = if let Some(parent) = &revisions[n].predecessor {
                let next = *positions.get(parent.as_str()).ok_or_else(corrupt)?;
                if revisions[n].event_id != revisions[next].event_id {
                    return Err(corrupt());
                }
                Some(next)
            } else {
                None
            };
        }
        for n in path {
            states[n] = 2;
        }
    }
    Ok(())
}

fn fingerprint(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(corrupt());
    }
    Ok(())
}

fn task_bounds(total: usize, next: usize, batch: usize, complete: bool) -> Result<()> {
    if !(1..=1000).contains(&total)
        || !(1..=total + 1).contains(&next)
        || !(1..=100).contains(&batch)
        || complete != (next == total + 1)
    {
        return Err(corrupt());
    }
    Ok(())
}

fn check_imports(history: &ImportHistory, index: &Index<'_>, cancel: &AtomicBool) -> Result<()> {
    identity(&history.source_target_id)?;
    let mut tasks = BTreeMap::new();
    for task in &history.tasks {
        cancelled(cancel)?;
        identity(&task.id)?;
        if tasks.insert(task.id.as_str(), task).is_some() {
            return Err(corrupt());
        }
        task_bounds(
            task.total,
            task.next_record,
            task.batch_size,
            task.state == TaskState::Complete,
        )?;
        if task.profile != CSV_PROFILE || task.decisions.len() != task.total {
            return Err(corrupt());
        }
        fingerprint(&task.input_fingerprint)?;
        fingerprint(&task.mapping_fingerprint)?;
        task.mapping.validate().map_err(|_| corrupt())?;
        let mapping = serde_json::to_vec(&task.mapping).map_err(|_| corrupt())?;
        if crate::crypto::hex(&Sha256::digest([CSV_PROFILE.as_bytes(), &mapping].concat()))
            != task.mapping_fingerprint
        {
            return Err(corrupt());
        }
    }
    let mut rows = BTreeMap::new();
    let mut row_ids = BTreeSet::new();
    let mut counts = BTreeMap::<&str, usize>::new();
    let mut owners = BTreeMap::new();
    let mut planned_operations = BTreeSet::new();
    for row in &history.rows {
        cancelled(cancel)?;
        identity(&row.row_id)?;
        let task = tasks.get(row.task_id.as_str()).ok_or_else(corrupt)?;
        if row.record == 0
            || row.record > task.total
            || !row_ids.insert(row.row_id.as_str())
            || rows
                .insert((row.task_id.as_str(), row.record), row)
                .is_some()
        {
            return Err(corrupt());
        }
        *counts.entry(row.task_id.as_str()).or_default() += 1;
        let pending = row.record >= task.next_record;
        let importing = task.decisions[row.record - 1] == Decision::Import;
        let expected = if pending {
            ImportOutcome::Pending
        } else if importing {
            ImportOutcome::Imported
        } else {
            ImportOutcome::Skipped
        };
        if row.outcome != expected
            || row.intent.is_some() != importing
            || row.source_id.as_deref() == Some("")
        {
            return Err(corrupt());
        }
        if let Some(write) = &row.intent {
            if write.previous_revision_id.is_some()
                || write.expected_sequence.is_some()
                || write.observed_balance_minor.is_some()
                || write.target_balance_minor.is_some()
                || !write.resolve_candidates.is_empty()
                || !write.event.as_ref().is_some_and(|e| {
                    matches!(
                        e.kind,
                        EventKind::Income
                            | EventKind::Expense
                            | EventKind::Transfer
                            | EventKind::Refund
                    )
                })
                || !planned_operations.insert(write.operation_id.as_str())
            {
                return Err(corrupt());
            }
            intent(write, index, !pending)?;
            if !pending
                && let Some(source) = &row.source_id
                && owners
                    .insert(
                        (task.mapping.source_namespace.as_str(), source.as_str()),
                        write,
                    )
                    .is_some()
            {
                return Err(corrupt());
            }
        }
    }
    for task in &history.tasks {
        if counts.get(task.id.as_str()).copied().unwrap_or(0) != task.total {
            return Err(corrupt());
        }
    }
    let mut source_keys = BTreeSet::new();
    for source in &history.sources {
        cancelled(cancel)?;
        let key = (source.namespace.as_str(), source.source_id.as_str());
        if !source_keys.insert(key) {
            return Err(corrupt());
        }
        let write = owners.get(&key).ok_or_else(corrupt)?;
        if source.event_id != write.event_id
            || source.revision_id != write.revision_id
            || write.event.as_ref() != Some(&source.event)
        {
            return Err(corrupt());
        }
    }
    if source_keys.len() != owners.len() {
        return Err(corrupt());
    }
    let mut next = BTreeMap::<&str, usize>::new();
    for batch in &history.batches {
        cancelled(cancel)?;
        let task = tasks.get(batch.task_id.as_str()).ok_or_else(corrupt)?;
        let first = next.entry(batch.task_id.as_str()).or_insert(1);
        if batch.first_record != *first
            || batch.first_record > task.total
            || batch.last_record != (batch.first_record + task.batch_size - 1).min(task.total)
            || batch.last_record >= task.next_record
        {
            return Err(corrupt());
        }
        let mut imported = 0;
        let mut skipped = 0;
        for record in batch.first_record..=batch.last_record {
            match rows
                .get(&(batch.task_id.as_str(), record))
                .ok_or_else(corrupt)?
                .outcome
            {
                ImportOutcome::Imported => imported += 1,
                ImportOutcome::Skipped => skipped += 1,
                _ => return Err(corrupt()),
            }
        }
        if (batch.imported, batch.skipped) != (imported, skipped) {
            return Err(corrupt());
        }
        *first = batch.last_record + 1;
    }
    for task in &history.tasks {
        if next.get(task.id.as_str()).copied().unwrap_or(1) != task.next_record {
            return Err(corrupt());
        }
    }
    check_undos(history, &tasks, &rows, index, cancel)
}

fn check_undos(
    history: &ImportHistory,
    imports: &BTreeMap<&str, &ImportTask>,
    import_rows: &BTreeMap<(&str, usize), &ImportRow>,
    index: &Index<'_>,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut tasks = BTreeMap::new();
    for task in &history.undo_tasks {
        cancelled(cancel)?;
        identity(&task.id)?;
        if tasks.insert(task.id.as_str(), task).is_some()
            || !imports.contains_key(task.import_task_id.as_str())
        {
            return Err(corrupt());
        }
        task_bounds(
            task.total,
            task.next_ordinal,
            task.batch_size,
            task.state == State::Complete,
        )?;
        if task.selection.len() != task.total
            || task
                .selection
                .windows(2)
                .any(|w| w[0].record >= w[1].record)
        {
            return Err(corrupt());
        }
    }
    let mut rows = BTreeMap::new();
    let mut counts = BTreeMap::<&str, usize>::new();
    let mut operations = BTreeSet::new();
    for row in &history.undo_rows {
        cancelled(cancel)?;
        let task = tasks.get(row.undo_id.as_str()).ok_or_else(corrupt)?;
        if row.ordinal == 0
            || row.ordinal > task.total
            || rows
                .insert((row.undo_id.as_str(), row.ordinal), row)
                .is_some()
        {
            return Err(corrupt());
        }
        let selection = &task.selection[row.ordinal - 1];
        let original = import_rows
            .get(&(task.import_task_id.as_str(), row.record))
            .ok_or_else(corrupt)?;
        let original_intent = original.intent.as_ref().ok_or_else(corrupt)?;
        let write = &row.intent;
        if original.outcome != ImportOutcome::Imported
            || row.row_id != original.row_id
            || selection.row_id != row.row_id
            || selection.record != row.record
            || selection.event_id != write.event_id
            || selection.revision_id != original_intent.revision_id
            || write.previous_revision_id.as_ref() != Some(&selection.revision_id)
            || write.event_id != original_intent.event_id
            || write.event.is_some()
            || write.expected_sequence.is_some()
            || write.observed_balance_minor.is_some()
            || write.target_balance_minor.is_some()
            || !write.resolve_candidates.is_empty()
            || row.deleted != (row.ordinal < task.next_ordinal)
            || !operations.insert(write.operation_id.as_str())
        {
            return Err(corrupt());
        }
        intent(write, index, row.deleted)?;
        *counts.entry(row.undo_id.as_str()).or_default() += 1;
    }
    for task in &history.undo_tasks {
        if counts.get(task.id.as_str()).copied().unwrap_or(0) != task.total {
            return Err(corrupt());
        }
    }
    let mut next = BTreeMap::<&str, usize>::new();
    for batch in &history.undo_batches {
        cancelled(cancel)?;
        let task = tasks.get(batch.undo_id.as_str()).ok_or_else(corrupt)?;
        let first = next.entry(batch.undo_id.as_str()).or_insert(1);
        if batch.first_ordinal != *first
            || batch.first_ordinal > task.total
            || batch.last_ordinal != (batch.first_ordinal + task.batch_size - 1).min(task.total)
            || batch.last_ordinal >= task.next_ordinal
            || batch.deleted != batch.last_ordinal - batch.first_ordinal + 1
        {
            return Err(corrupt());
        }
        *first = batch.last_ordinal + 1;
    }
    for task in &history.undo_tasks {
        if next.get(task.id.as_str()).copied().unwrap_or(1) != task.next_ordinal {
            return Err(corrupt());
        }
    }
    Ok(())
}

pub(crate) fn verify(facts: &Facts, months: &[String], cancel: &AtomicBool) -> Result<Snapshot> {
    let mut index = Index {
        accounts: BTreeMap::new(),
        categories: BTreeMap::new(),
        revisions: BTreeMap::new(),
        intents: BTreeMap::new(),
        sequence: facts.sequence,
        schema: facts.source_schema,
    };
    for reference in &facts.accounts {
        cancelled(cancel)?;
        identity(&reference.id)?;
        if reference.name.len() > 256
            || !matches!(
                reference.kind.as_str(),
                "cash" | "bank" | "wallet" | "credit_card"
            )
            || index
                .accounts
                .insert(reference.id.as_str(), reference)
                .is_some()
        {
            return Err(corrupt());
        }
    }
    for reference in &facts.categories {
        cancelled(cancel)?;
        identity(&reference.id)?;
        if reference.name.len() > 256
            || !matches!(reference.kind.as_str(), "income" | "expense")
            || index
                .categories
                .insert(reference.id.as_str(), reference)
                .is_some()
        {
            return Err(corrupt());
        }
    }
    let mut event_ids = BTreeSet::new();
    let mut roots = BTreeSet::new();
    for revision in &facts.revisions {
        cancelled(cancel)?;
        identity(&revision.id)?;
        identity(&revision.event_id)?;
        if index
            .revisions
            .insert(revision.id.as_str(), revision)
            .is_some()
        {
            return Err(corrupt());
        }
        event_ids.insert(revision.event_id.as_str());
        if revision.predecessor.is_none()
            && (!roots.insert(revision.event_id.as_str()) || revision.event.is_none())
        {
            return Err(corrupt());
        }
        if let Some(fact) = &revision.event {
            event(fact, &index)?;
        }
    }
    if event_ids != roots {
        return Err(corrupt());
    }
    check_graph(&index, cancel)?;
    for write in &facts.intent_history {
        cancelled(cancel)?;
        if index
            .intents
            .insert(write.operation_id.as_str(), write)
            .is_some()
        {
            return Err(corrupt());
        }
    }
    for write in &facts.intent_history {
        cancelled(cancel)?;
        intent(write, &index, true)?;
    }
    let mut selected = BTreeMap::new();
    let mut active = BTreeSet::new();
    let mut deleted = BTreeSet::new();
    let mut selected_events = Vec::new();
    for selection in &facts.selected {
        cancelled(cancel)?;
        let revision = index
            .revisions
            .get(selection.revision_id.as_str())
            .ok_or_else(corrupt)?;
        if selection.event_id != revision.event_id
            || selected
                .insert(selection.event_id.clone(), selection.revision_id.clone())
                .is_some()
        {
            return Err(corrupt());
        }
        if let Some(fact) = &revision.event {
            active.insert(selection.event_id.clone());
            selected_events.push(fact.clone());
        } else {
            deleted.insert(selection.event_id.clone());
        }
    }
    if selected.keys().map(String::as_str).collect::<BTreeSet<_>>() != event_ids {
        return Err(corrupt());
    }
    let mut candidates = BTreeSet::new();
    let mut groups = BTreeMap::<&str, BTreeSet<&str>>::new();
    for candidate in &facts.candidates {
        cancelled(cancel)?;
        let revision = index
            .revisions
            .get(candidate.revision_id.as_str())
            .ok_or_else(corrupt)?;
        if candidate.event_id != revision.event_id
            || !candidates.insert(candidate.revision_id.clone())
        {
            return Err(corrupt());
        }
        groups
            .entry(candidate.event_id.as_str())
            .or_default()
            .insert(candidate.revision_id.as_str());
    }
    for (event, group) in groups {
        if group.len() < 2 || !group.contains(selected.get(event).ok_or_else(corrupt)?.as_str()) {
            return Err(corrupt());
        }
    }
    if let Some(history) = &facts.imports {
        check_imports(history, &index, cancel)?;
    }
    let accounts = facts
        .accounts
        .iter()
        .map(|r| r.id.clone())
        .collect::<Vec<_>>();
    let categories = facts
        .categories
        .iter()
        .filter(|r| r.kind == "expense")
        .map(|r| r.id.clone())
        .collect::<Vec<_>>();
    let months = months
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    cancelled(cancel)?;
    let projection = project(&accounts, &categories, &selected_events, &months)?;
    Ok(Snapshot {
        projection,
        active_event_ids: active,
        selected_revision_ids: selected,
        deleted_event_ids: deleted,
        unresolved_candidate_revision_ids: candidates,
        archived_account_ids: facts
            .accounts
            .iter()
            .filter(|r| r.archived)
            .map(|r| r.id.clone())
            .collect(),
        archived_category_ids: facts
            .categories
            .iter()
            .filter(|r| r.archived)
            .map(|r| r.id.clone())
            .collect(),
    })
}

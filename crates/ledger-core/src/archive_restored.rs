//! 0.2 宿主归档候选。解封只交付经过核对的内存事实。
use super::*;
use crate::logical_snapshot::{ImportHistory, RemoteKnowledge, encode_contents};
use crate::store::{
    Snapshot, Write,
    snapshot_store::{
        materialized::{LedgerRows, archived_operations, reserved_ids},
        validate::{HistoryView, LedgerView, verify_segments},
    },
};
use serde::{Deserialize, Serialize};

#[cfg(test)]
#[path = "archive_restored_tests.rs"]
mod tests;

const PROFILE: &str = "m0-logical-snapshot-v2";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RestoredFacts {
    profile: String,
    currency: String,
    minor_unit_scale: u8,
    remote_knowledge: RemoteKnowledge,
    pub ledger: LedgerRows,
    pub history: Vec<History>,
}

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum History {
    Legacy {
        source_schema: i64,
        sequence: i64,
        intent_history: Vec<Write>,
        #[serde(deserialize_with = "nullable_imports")]
        imports: Option<ImportHistory>,
    },
    Restored {
        sequence: i64,
        intent_history: Vec<Write>,
    },
}

fn nullable_imports<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> std::result::Result<Option<ImportHistory>, D::Error> {
    Option::deserialize(decoder)
}

impl History {
    pub(crate) fn view(&self) -> HistoryView<'_> {
        match self {
            Self::Legacy {
                source_schema,
                sequence,
                intent_history,
                imports,
            } => HistoryView {
                source_schema: Some(*source_schema),
                sequence: *sequence,
                intents: intent_history,
                imports: imports.as_ref(),
            },
            Self::Restored {
                sequence,
                intent_history,
            } => HistoryView {
                source_schema: None,
                sequence: *sequence,
                intents: intent_history,
                imports: None,
            },
        }
    }
}

#[derive(Serialize)]
pub(crate) struct RestoredContents {
    pub facts: RestoredFacts,
    pub verification: Snapshot,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    facts: RestoredFacts,
    verification: serde_json::Value,
}

pub(crate) enum Opened {
    Legacy(Box<OpenedArchive>),
    Restored(Box<OpenedRestored>),
}

pub(crate) struct OpenedRestored {
    pub id: [u8; 16],
    pub contents: RestoredContents,
}

fn manifest_for(facts: &RestoredFacts, payload: &[u8], limits: Limits) -> Result<[u8; MANIFEST]> {
    let ledger = &facts.ledger;
    let mut sizes = [
        ledger.accounts.len(),
        ledger.categories.len(),
        ledger.revisions.len(),
        ledger.selected.len(),
        ledger.candidates.len(),
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    for segment in &facts.history {
        let segment = segment.view();
        sizes[5] = sizes[5]
            .checked_add(segment.intents.len())
            .ok_or_else(limit)?;
        if let Some(imports) = segment.imports {
            sizes[6..].copy_from_slice(&import_counts(imports));
        }
    }
    let segments = u32::try_from(facts.history.len()).map_err(|_| limit())?;
    let sequence = facts
        .history
        .last()
        .ok_or_else(payload_error)?
        .view()
        .sequence;
    Ok(build_manifest(
        Version::Restored,
        segments,
        u64::try_from(sequence).map_err(|_| payload_error())?,
        checked_counts(sizes, facts.history.len(), limits)?,
        payload,
    ))
}

fn verify(facts: &RestoredFacts, months: &[String], cancel: &AtomicBool) -> Result<Snapshot> {
    let [legacy, restored @ ..] = facts.history.as_slice() else {
        return Err(payload_error());
    };
    let legacy = legacy.view();
    if facts.profile != PROFILE
        || facts.currency != "CNY"
        || facts.minor_unit_scale != 2
        || restored.is_empty()
        || !matches!(legacy.source_schema, Some(1..=3))
        || (legacy.source_schema != Some(1)) != legacy.imports.is_some()
        || (legacy.source_schema == Some(2)
            && legacy.imports.is_some_and(|imports| {
                !imports.undo_tasks.is_empty()
                    || !imports.undo_rows.is_empty()
                    || !imports.undo_batches.is_empty()
            }))
    {
        return Err(payload_error());
    }
    let archived = archived_operations(legacy.intents, legacy.imports);
    let [(_, reserved_revisions), (_, reserved_events)] = reserved_ids(legacy.imports);
    let mut revisions: std::collections::BTreeSet<_> = legacy
        .intents
        .iter()
        .map(|intent| intent.revision_id.as_str())
        .collect();
    let mut views = Vec::new();
    views
        .try_reserve_exact(facts.history.len())
        .map_err(|_| resource())?;
    views.push(legacy);
    for segment in restored {
        cancelled(cancel)?;
        let view = segment.view();
        if view.source_schema.is_some()
            || view.sequence < i64::try_from(view.intents.len()).map_err(|_| limit())?
        {
            return Err(payload_error());
        }
        for intent in view.intents {
            cancelled(cancel)?;
            if intent
                .expected_sequence
                .is_some_and(|expected| expected >= view.sequence)
                || archived.contains(intent.operation_id.as_str())
                || reserved_revisions.contains(intent.revision_id.as_str())
                || (intent.previous_revision_id.is_none()
                    && reserved_events.contains(intent.event_id.as_str()))
                || !revisions.insert(intent.revision_id.as_str())
            {
                return Err(payload_error());
            }
        }
        views.push(view);
    }
    let ledger = &facts.ledger;
    verify_segments(
        LedgerView {
            accounts: &ledger.accounts,
            categories: &ledger.categories,
            revisions: &ledger.revisions,
            selected: &ledger.selected,
            candidates: &ledger.candidates,
        },
        &views,
        months,
        cancel,
    )
}

pub(crate) fn decode(
    payload: &[u8],
    limits: Limits,
    cancel: &AtomicBool,
) -> Result<RestoredContents> {
    json::scan(payload, limits, cancel)?;
    let mut wire: Wire = serde_json::from_slice(payload).map_err(|_| payload_error())?;
    let verification = json::checked_projection(&wire.verification, cancel, |months| {
        verify(&wire.facts, months, cancel)
    })?;
    let ledger = &mut wire.facts.ledger;
    ledger.accounts.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    ledger.categories.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    ledger.revisions.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    ledger
        .selected
        .sort_unstable_by(|a, b| a.event_id.cmp(&b.event_id));
    ledger
        .candidates
        .sort_unstable_by(|a, b| a.revision_id.cmp(&b.revision_id));
    Ok(RestoredContents {
        facts: wire.facts,
        verification,
    })
}

impl RestoredContents {
    pub(crate) fn from_rows(
        ledger: LedgerRows,
        history: Vec<History>,
        months: &[String],
        cancel: &AtomicBool,
    ) -> Result<Self> {
        let facts = RestoredFacts {
            profile: PROFILE.into(),
            currency: "CNY".into(),
            minor_unit_scale: 2,
            remote_knowledge: RemoteKnowledge::Unknown,
            ledger,
            history,
        };
        let verification = verify(&facts, months, cancel)?;
        Ok(Self {
            facts,
            verification,
        })
    }

    pub(crate) fn encode(&self, limits: Limits, cancel: &AtomicBool) -> Result<Zeroizing<Vec<u8>>> {
        let bytes = Zeroizing::new(encode_contents(self, limits.payload_bytes, cancel).map_err(
            |error| {
                if error.code == "snapshot_limit" {
                    limit()
                } else {
                    error
                }
            },
        )?);
        json::scan(&bytes, limits, cancel)?;
        Ok(bytes)
    }
}

pub(crate) fn seal(
    contents: RestoredContents,
    password: &[u8],
    limits: Limits,
    cancel: &AtomicBool,
) -> Result<SealedArchive> {
    seal_with(
        &contents,
        password,
        limits,
        cancel,
        &Entropy::random()?,
        |_| Ok(()),
    )
}

fn seal_with(
    contents: &RestoredContents,
    password: &[u8],
    limits: Limits,
    cancel: &AtomicBool,
    entropy: &Entropy,
    observe: impl FnMut(Point) -> Result<()>,
) -> Result<SealedArchive> {
    limits.validate()?;
    cancelled(cancel)?;
    crypto::validate_password(password)?;
    let payload = contents.encode(limits, cancel)?;
    let manifest = manifest_for(&contents.facts, &payload, limits)?;
    seal_payload(
        Version::Restored,
        &payload,
        &manifest,
        password,
        cancel,
        entropy,
        observe,
    )
}

pub(crate) fn open(
    input: &[u8],
    password: &[u8],
    limits: Limits,
    cancel: &AtomicBool,
) -> Result<Opened> {
    open_observed(input, password, limits, cancel, |_| Ok(()))
}

fn open_observed(
    input: &[u8],
    password: &[u8],
    limits: Limits,
    cancel: &AtomicBool,
    mut observe: impl FnMut(Point) -> Result<()>,
) -> Result<Opened> {
    let (header, body) = open_payload(
        input,
        password,
        limits,
        cancel,
        Version::Restored,
        &mut observe,
    )?;
    let id = header.id();
    let opened = match header.version {
        Version::Legacy => Opened::Legacy(Box::new(OpenedArchive {
            id,
            contents: decode_legacy(&body, limits, cancel)?,
        })),
        Version::Restored => {
            let payload = &body[128..body.len() - 8];
            let contents = decode(payload, limits, cancel)?;
            if body[4..120] != manifest_for(&contents.facts, payload, limits)? {
                return Err(payload_error());
            }
            Opened::Restored(Box::new(OpenedRestored { id, contents }))
        }
    };
    observe(Point::AfterJson)?;
    cancelled(cancel)?;
    observe(Point::Complete)?;
    cancelled(cancel)?;
    Ok(opened)
}

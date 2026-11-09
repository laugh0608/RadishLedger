//! Unix 测试专用布局。活动事实共用 Store，原意图与任务只作历史。
use super::*;
use crate::archive_container::{Limits as ArchiveLimits, json::scan};
use crate::restore_stage::Ticket;
use sha2::{Digest, Sha256};

pub(crate) const VERSION: i64 = 1002;
const SCHEMA: &str = "
    CREATE TABLE materialization (
        id INTEGER PRIMARY KEY CHECK(id=1), target TEXT NOT NULL, stage TEXT NOT NULL,
        history TEXT, digest TEXT, CHECK((history IS NULL) = (digest IS NULL)));
    CREATE TABLE archived_operations (id TEXT PRIMARY KEY);";
const CONTENT_TABLES: [&str; 8] = [
    "accounts",
    "categories",
    "revisions",
    "selected",
    "candidates",
    "outbox",
    "csv_sources",
    "archived_operations",
];

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct History<I, C> {
    source_schema: i64,
    sequence: i64,
    months: Vec<String>,
    intents: I,
    imports: C,
}

type StoredHistory = History<Vec<Write>, Option<ImportHistory>>;

pub(crate) fn mismatch() -> CoreError {
    CoreError::new(
        "materialization_mismatch",
        "物化目标或已登记事实不一致，保留原库与现场",
    )
}

fn version(connection: &Connection) -> Result<i64> {
    connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(db_error)
}

fn bounded<T: Serialize>(
    value: &T,
    limits: ArchiveLimits,
    cancel: &AtomicBool,
) -> Result<Zeroizing<Vec<u8>>> {
    Ok(Zeroizing::new(encode_contents(
        value,
        limits.payload_bytes,
        cancel,
    )?))
}

fn json_text<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|_| mismatch())
}

fn binding(store: &Store, ticket: &Ticket, device: &str) -> Result<()> {
    if version(&store.connection)? != VERSION {
        return Err(mismatch());
    }
    store.verify_bootstrap(ticket.target_id(), device)?;
    let connection = &store.connection;
    let target: String = connection
        .query_row(
            "SELECT target FROM materialization WHERE id=1 AND length(CAST(target AS BLOB))<=4096",
            [],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    if target != json_text(ticket)? {
        return Err(mismatch());
    }
    let mut expected = BTreeSet::from(CONTENT_TABLES);
    expected.extend([
        "metadata",
        "bootstrap_identity",
        "bootstrap_fixture",
        "materialization",
    ]);
    let mut statement = connection
        .prepare("SELECT type, name FROM sqlite_schema WHERE sql IS NOT NULL")
        .map_err(db_error)?;
    let mut rows = statement.query([]).map_err(db_error)?;
    while let Some(row) = rows.next().map_err(db_error)? {
        let name: &str = row
            .get_ref(1)
            .map_err(db_error)?
            .as_str()
            .map_err(|_| mismatch())?;
        if row.get_ref(0).map_err(db_error)?.as_str() != Ok("table") || !expected.remove(name) {
            return Err(mismatch());
        }
    }
    if !expected.is_empty() {
        return Err(mismatch());
    }
    Ok(())
}

pub(crate) fn open(
    path: &Path,
    key: &DbKey,
    ticket: &Ticket,
    device: &str,
    readonly: bool,
) -> Result<Store> {
    let store = Store::connect(path, key, readonly)?;
    binding(&store, ticket, device)?;
    Ok(store)
}

pub(crate) fn prepare(
    path: &Path,
    key: &DbKey,
    ticket: &Ticket,
    device: &str,
    stage: &Ticket,
    can_initialize: bool,
) -> Result<Store> {
    let mut store = Store::connect(path, key, false)?;
    if version(&store.connection)? == 0 && can_initialize {
        let tx = store
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let objects: i64 = tx
            .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))
            .map_err(db_error)?;
        if objects != 0 {
            return Err(mismatch());
        }
        tx.execute_batch(LEDGER_SCHEMA).map_err(db_error)?;
        tx.execute_batch(csv_store::SOURCE_SCHEMA)
            .map_err(db_error)?;
        tx.execute_batch(SCHEMA).map_err(db_error)?;
        Store::insert_bootstrap(&tx, ticket.target_id(), device)?;
        tx.execute(
            "INSERT INTO materialization(id,target,stage) VALUES(1,?1,?2)",
            params![json_text(ticket)?, json_text(stage)?],
        )
        .map_err(db_error)?;
        tx.pragma_update(None, "user_version", VERSION)
            .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
    }
    binding(&store, ticket, device)?;
    let recorded_stage: String = store
        .connection
        .query_row(
            "SELECT stage FROM materialization WHERE id=1 AND length(CAST(stage AS BLOB))<=4096",
            [],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    if recorded_stage != json_text(stage)? {
        return Err(mismatch());
    }
    Ok(store)
}

fn ordered<T>(values: &[T], key: impl Fn(&T) -> &str) -> Vec<&T> {
    let mut order: Vec<_> = values.iter().collect();
    order.sort_unstable_by(|a, b| key(a).cmp(key(b)));
    order
}

pub(crate) fn digest(
    contents: &Contents,
    limits: ArchiveLimits,
    cancel: &AtomicBool,
) -> Result<[u8; 32]> {
    let f = &contents.facts;
    // 对事实按稳定 ID 比较，不依赖归档集合的物理排列。
    let canonical = (
        f.source_schema,
        f.sequence,
        ordered(&f.accounts, |r| &r.id),
        ordered(&f.categories, |r| &r.id),
        ordered(&f.revisions, |r| &r.id),
        ordered(&f.selected, |r| &r.event_id),
        ordered(&f.candidates, |r| &r.revision_id),
        &f.intent_history,
        &f.imports,
        &contents.verification,
    );
    Ok(Sha256::digest(bounded(&canonical, limits, cancel)?.as_slice()).into())
}

fn archived_operations(facts: &Facts) -> BTreeSet<&str> {
    facts
        .intent_history
        .iter()
        .chain(
            facts
                .imports
                .iter()
                .flat_map(|i| i.rows.iter().filter_map(|r| r.intent.as_ref())),
        )
        .chain(
            facts
                .imports
                .iter()
                .flat_map(|i| i.undo_rows.iter().map(|r| &r.intent)),
        )
        .map(|intent| intent.operation_id.as_str())
        .collect()
}

pub(crate) fn check_new_operation(connection: &Connection, id: &str) -> Result<()> {
    if version(connection)? == VERSION {
        let archived: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM archived_operations WHERE id=?1)",
                [id],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        if archived {
            return Err(CoreError::new(
                "archived_operation",
                "恢复历史中的操作身份不能用于新写入",
            ));
        }
    }
    Ok(())
}

pub(crate) fn write(
    store: &mut Store,
    source: &Contents,
    limits: ArchiveLimits,
    cancel: &AtomicBool,
    before_commit: impl FnOnce() -> Result<()>,
) -> Result<bool> {
    let tx = store
        .connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let written: bool = tx
        .query_row(
            "SELECT history IS NOT NULL FROM materialization WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    if written {
        return Ok(false);
    }
    for table in CONTENT_TABLES {
        let has_rows: bool = tx
            .query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table})"), [], |r| {
                r.get(0)
            })
            .map_err(db_error)?;
        if has_rows {
            return Err(mismatch());
        }
    }
    let facts = &source.facts;
    for (table, values) in [
        ("accounts", &facts.accounts),
        ("categories", &facts.categories),
    ] {
        for value in values {
            cancelled(cancel)?;
            tx.execute(
                &format!("INSERT INTO {table} VALUES(?1,?2,?3,?4)"),
                params![value.id, value.kind, value.name, value.archived],
            )
            .map_err(db_error)?;
        }
    }
    for revision in &facts.revisions {
        cancelled(cancel)?;
        tx.execute(
            "INSERT INTO revisions VALUES(?1,?2,?3,?4)",
            params![
                revision.id,
                revision.event_id,
                revision.predecessor,
                revision.event.as_ref().map(json_text).transpose()?
            ],
        )
        .map_err(db_error)?;
    }
    for value in &facts.selected {
        cancelled(cancel)?;
        tx.execute(
            "INSERT INTO selected VALUES(?1,?2)",
            params![value.event_id, value.revision_id],
        )
        .map_err(db_error)?;
    }
    for value in &facts.candidates {
        cancelled(cancel)?;
        tx.execute(
            "INSERT INTO candidates VALUES(?1,?2)",
            params![value.revision_id, value.event_id],
        )
        .map_err(db_error)?;
    }
    for source in facts.imports.iter().flat_map(|i| &i.sources) {
        cancelled(cancel)?;
        tx.execute(
            "INSERT INTO csv_sources VALUES(?1,?2,?3,?4,?5)",
            params![
                source.namespace,
                source.source_id,
                json_text(&source.event)?,
                source.event_id,
                source.revision_id
            ],
        )
        .map_err(db_error)?;
    }
    for id in archived_operations(facts) {
        cancelled(cancel)?;
        tx.execute("INSERT INTO archived_operations VALUES(?1)", [id])
            .map_err(db_error)?;
    }
    let history = History {
        source_schema: facts.source_schema,
        sequence: facts.sequence,
        months: source
            .verification
            .projection
            .monthly_reports
            .keys()
            .cloned()
            .collect(),
        intents: &facts.intent_history,
        imports: &facts.imports,
    };
    let encoded = bounded(&history, limits, cancel)?;
    let encoded = std::str::from_utf8(&encoded).map_err(|_| mismatch())?;
    tx.execute(
        "UPDATE materialization SET history=?1,digest=?2 WHERE id=1",
        params![
            encoded,
            crate::crypto::hex(&digest(source, limits, cancel)?)
        ],
    )
    .map_err(db_error)?;
    before_commit()?;
    cancelled(cancel)?;
    tx.commit().map_err(db_error)?;
    Ok(true)
}

pub(crate) fn inspect_ready(
    store: &Store,
    limits: ArchiveLimits,
    cancel: &AtomicBool,
) -> Result<Contents> {
    let tx = store.connection.unchecked_transaction().map_err(db_error)?;
    let sequence: i64 = tx
        .query_row("SELECT sequence FROM metadata WHERE id=1", [], |r| r.get(0))
        .map_err(db_error)?;
    let queued: bool = tx
        .query_row("SELECT EXISTS(SELECT 1 FROM outbox)", [], |r| r.get(0))
        .map_err(db_error)?;
    if sequence != 0 || queued {
        return Err(mismatch());
    }
    let mut budget = Budget {
        limits: Limits {
            rows: limits.records,
            bytes: limits.payload_bytes,
        },
        rows: 0,
        bytes: 0,
        cancel,
    };
    let (history, expected) = tx
        .query_row(
            "SELECT history,digest FROM materialization WHERE id=1",
            [],
            |row| {
                Ok((
                    row.get_ref(0)?
                        .as_str()
                        .ok()
                        .filter(|s| s.len() <= limits.payload_bytes)
                        .map(str::to_owned),
                    row.get_ref(1)?
                        .as_str()
                        .ok()
                        .filter(|s| s.len() == 64)
                        .map(str::to_owned),
                ))
            },
        )
        .map_err(db_error)?;
    let history = Zeroizing::new(history.ok_or_else(mismatch)?);
    scan(history.as_bytes(), limits, cancel)?;
    let history: StoredHistory = json(&history)?;
    let facts = Facts {
        profile: PROFILE.into(),
        source_schema: history.source_schema,
        sequence: history.sequence,
        currency: "CNY".into(),
        minor_unit_scale: 2,
        remote_knowledge: RemoteKnowledge::Unknown,
        accounts: references(
            &tx,
            "SELECT id,kind,name,archived FROM accounts ORDER BY id",
            &mut budget,
        )?,
        categories: references(
            &tx,
            "SELECT id,kind,name,archived FROM categories ORDER BY id",
            &mut budget,
        )?,
        revisions: collect(
            &tx,
            "SELECT id,event_id,predecessor,payload FROM revisions ORDER BY id",
            &mut budget,
            |r, b| {
                Ok(Revision {
                    id: text(r, 0, b)?,
                    event_id: text(r, 1, b)?,
                    predecessor: optional_text(r, 2, b)?,
                    event: optional_text(r, 3, b)?
                        .map(|value| json(&value))
                        .transpose()?,
                })
            },
        )?,
        selected: selections(
            &tx,
            "SELECT event_id,revision_id FROM selected ORDER BY event_id",
            &mut budget,
        )?,
        candidates: selections(
            &tx,
            "SELECT event_id,revision_id FROM candidates ORDER BY revision_id",
            &mut budget,
        )?,
        intent_history: history.intents,
        imports: history.imports,
    };
    let verification = validate::verify(&facts, &history.months, cancel)?;
    let contents = Contents {
        facts,
        verification,
    };
    let actual = crate::crypto::hex(&digest(&contents, limits, cancel)?);
    if expected.as_deref() != Some(actual.as_str()) {
        return Err(mismatch());
    }
    check_indexes(&tx, &contents.facts, &mut budget)?;
    tx.commit().map_err(db_error)?;
    Ok(contents)
}

fn check_indexes(connection: &Connection, facts: &Facts, budget: &mut Budget<'_>) -> Result<()> {
    let operations = collect(
        connection,
        "SELECT id FROM archived_operations ORDER BY id",
        budget,
        |r, b| text(r, 0, b),
    )?;
    if !operations
        .iter()
        .map(String::as_str)
        .eq(archived_operations(facts))
    {
        return Err(mismatch());
    }
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
    let mut expected: Vec<_> = facts.imports.iter().flat_map(|i| &i.sources).collect();
    expected
        .sort_unstable_by(|a, b| (&a.namespace, &a.source_id).cmp(&(&b.namespace, &b.source_id)));
    if sources.len() != expected.len()
        || sources.iter().zip(expected).any(|(a, b)| {
            a.namespace != b.namespace
                || a.source_id != b.source_id
                || a.event != b.event
                || a.event_id != b.event_id
                || a.revision_id != b.revision_id
        })
    {
        return Err(mismatch());
    }
    Ok(())
}

pub(crate) fn preview(
    store: &Store,
    bytes: &[u8],
    mapping: &Mapping,
    cancel: &AtomicBool,
) -> Result<crate::csv_import::Preview> {
    let mut preview = crate::csv_import::preview(store, bytes, mapping, cancel)?;
    let tx = store.connection.unchecked_transaction().map_err(db_error)?;
    csv_store::inspect_preview_sources(&tx, &mut preview, &mapping.source_namespace)?;
    cancelled(cancel)?;
    tx.commit().map_err(db_error)?;
    Ok(preview)
}

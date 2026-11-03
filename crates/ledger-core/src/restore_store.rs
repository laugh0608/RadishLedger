//! 独立暂存 schema：实际业务行与惰性历史，不存在 outbox / 可执行 CSV 任务表。
use super::*;
use crate::archive_container::{Limits, json::scan};
use crate::logical_snapshot::{self, *};
use crate::restore_stage::{Point, Ticket, check_cancel, mismatch};
use serde::de::DeserializeOwned;
use std::sync::atomic::AtomicBool;

const VERSION: i64 = 1001;
const SCHEMA: [(&str, &str); 14] = [
    (
        "restore_control",
        "CREATE TABLE restore_control (id INTEGER PRIMARY KEY CHECK(id=1), ticket TEXT NOT NULL, state INTEGER NOT NULL CHECK(state BETWEEN 0 AND 2), source_schema INTEGER NOT NULL, source_sequence INTEGER NOT NULL, source_target TEXT)",
    ),
    (
        "restore_accounts",
        "CREATE TABLE restore_accounts (position INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, kind TEXT NOT NULL, name TEXT NOT NULL, archived INTEGER NOT NULL CHECK(archived IN (0,1)))",
    ),
    (
        "restore_categories",
        "CREATE TABLE restore_categories (position INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, kind TEXT NOT NULL, name TEXT NOT NULL, archived INTEGER NOT NULL CHECK(archived IN (0,1)))",
    ),
    (
        "restore_revisions",
        "CREATE TABLE restore_revisions (position INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, event_id TEXT NOT NULL, predecessor TEXT, payload TEXT)",
    ),
    (
        "restore_selected",
        "CREATE TABLE restore_selected (position INTEGER PRIMARY KEY, event_id TEXT NOT NULL UNIQUE, revision_id TEXT NOT NULL REFERENCES restore_revisions(id))",
    ),
    (
        "restore_candidates",
        "CREATE TABLE restore_candidates (position INTEGER PRIMARY KEY, event_id TEXT NOT NULL, revision_id TEXT NOT NULL UNIQUE REFERENCES restore_revisions(id))",
    ),
    (
        "restore_intents",
        "CREATE TABLE restore_intents (position INTEGER PRIMARY KEY, payload TEXT NOT NULL)",
    ),
    (
        "restore_import_tasks",
        "CREATE TABLE restore_import_tasks (position INTEGER PRIMARY KEY, payload TEXT NOT NULL)",
    ),
    (
        "restore_import_rows",
        "CREATE TABLE restore_import_rows (position INTEGER PRIMARY KEY, payload TEXT NOT NULL)",
    ),
    (
        "restore_import_sources",
        "CREATE TABLE restore_import_sources (position INTEGER PRIMARY KEY, payload TEXT NOT NULL)",
    ),
    (
        "restore_import_batches",
        "CREATE TABLE restore_import_batches (position INTEGER PRIMARY KEY, payload TEXT NOT NULL)",
    ),
    (
        "restore_undo_tasks",
        "CREATE TABLE restore_undo_tasks (position INTEGER PRIMARY KEY, payload TEXT NOT NULL)",
    ),
    (
        "restore_undo_rows",
        "CREATE TABLE restore_undo_rows (position INTEGER PRIMARY KEY, payload TEXT NOT NULL)",
    ),
    (
        "restore_undo_batches",
        "CREATE TABLE restore_undo_batches (position INTEGER PRIMARY KEY, payload TEXT NOT NULL)",
    ),
];

#[derive(Serialize)]
struct Review<'a> {
    facts: &'a Facts,
    verification: &'a Snapshot,
}
fn bounded<T: Serialize>(
    value: &T,
    limits: Limits,
    cancel: &AtomicBool,
) -> Result<Zeroizing<Vec<u8>>> {
    let bytes = Zeroizing::new(
        logical_snapshot::encode_contents(value, limits.payload_bytes, cancel).map_err(|e| {
            if e.code == "snapshot_limit" {
                CoreError::new("archive_limit", "暂存规范编码超过本次预算")
            } else {
                e
            }
        })?,
    );
    scan(&bytes, limits, cancel)?;
    Ok(bytes)
}
pub(crate) fn check_payload(source: &Contents, limits: Limits, cancel: &AtomicBool) -> Result<()> {
    bounded(source, limits, cancel)?;
    Ok(())
}

pub(crate) struct StageDb {
    store: Store,
}

fn encode<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|_| mismatch())
}
fn state(connection: &Connection) -> Result<i64> {
    connection
        .query_row("SELECT state FROM restore_control WHERE id=1", [], |r| {
            r.get(0)
        })
        .map_err(db_error)
}
fn layout(connection: &Connection) -> Result<()> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(db_error)?;
    if version != VERSION {
        return Err(mismatch());
    }
    let mut required: BTreeMap<_, _> = SCHEMA.into_iter().collect();
    let mut query = connection
        .prepare("SELECT name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'")
        .map_err(db_error)?;
    let mut rows = query.query([]).map_err(db_error)?;
    while let Some(row) = rows.next().map_err(db_error)? {
        let name: &str = row
            .get_ref(0)
            .map_err(db_error)?
            .as_str()
            .map_err(|_| mismatch())?;
        let sql: &str = row
            .get_ref(1)
            .map_err(db_error)?
            .as_str()
            .map_err(|_| mismatch())?;
        if required.remove(name) != Some(sql) {
            return Err(mismatch());
        }
    }
    if !required.is_empty() {
        return Err(mismatch());
    }
    Ok(())
}
fn binding(connection: &Connection, ticket: &Ticket) -> Result<()> {
    layout(connection)?;
    let count: i64 = connection
        .query_row("SELECT count(*) FROM restore_control", [], |r| r.get(0))
        .map_err(db_error)?;
    let bytes = connection
        .query_row(
            "SELECT ticket FROM restore_control WHERE id=1 AND length(CAST(ticket AS BLOB))<=4096",
            [],
            |r| r.get::<_, String>(0),
        )
        .map_err(db_error)?;
    if count != 1 || bytes != encode(ticket)? || !matches!(state(connection)?, 0..=2) {
        return Err(mismatch());
    }
    Ok(())
}

impl StageDb {
    pub fn prepare(
        path: &Path,
        key: &DbKey,
        ticket: &Ticket,
        source: &Contents,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        check_cancel(cancel)?;
        if !path.exists() {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(path).map_err(|error| {
                let mut e = CoreError::new("restore_io", "无法创建独占暂存数据库");
                e.os_code = error.raw_os_error();
                e
            })?;
        }
        let store = Store::connect(path, key, false)?;
        let version: i64 = store
            .connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db_error)?;
        if version == 0 {
            let count: i64 = store
                .connection
                .query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))
                .map_err(db_error)?;
            if count != 0 {
                return Err(mismatch());
            }
            let tx = store.connection.unchecked_transaction().map_err(db_error)?;
            for (_, sql) in SCHEMA {
                tx.execute_batch(sql).map_err(db_error)?;
            }
            tx.execute(
                "INSERT INTO restore_control VALUES(1,?1,0,?2,?3,?4)",
                params![
                    encode(ticket)?,
                    source.facts.source_schema,
                    source.facts.sequence,
                    source.facts.imports.as_ref().map(|i| &i.source_target_id)
                ],
            )
            .map_err(db_error)?;
            tx.pragma_update(None, "user_version", VERSION)
                .map_err(db_error)?;
            check_cancel(cancel)?;
            tx.commit().map_err(db_error)?;
        }
        binding(&store.connection, ticket)?;
        let db = Self { store };
        db.check_source(source)?;
        Ok(db)
    }
    pub fn open(path: &Path, key: &DbKey, ticket: &Ticket) -> Result<Self> {
        let store = Store::connect(path, key, false)?;
        binding(&store.connection, ticket)?;
        Ok(Self { store })
    }
    pub fn open_readonly(path: &Path, key: &DbKey, ticket: &Ticket) -> Result<Self> {
        let store = Store::connect(path, key, true)?;
        binding(&store.connection, ticket)?;
        Ok(Self { store })
    }
    pub fn check_abandon(path: &Path, key: &DbKey, ticket: &Ticket) -> Result<()> {
        let store = Store::connect(path, key, false)?;
        let version: i64 = store
            .connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db_error)?;
        if version == 0 {
            let count: i64 = store
                .connection
                .query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))
                .map_err(db_error)?;
            if count == 0 {
                return Ok(());
            }
        }
        binding(&store.connection, ticket)
    }
    fn check_source(&self, source: &Contents) -> Result<()> {
        let row:(i64,i64,Option<String>)=self.store.connection.query_row("SELECT source_schema,source_sequence,source_target FROM restore_control WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(db_error)?;
        if row
            != (
                source.facts.source_schema,
                source.facts.sequence,
                source
                    .facts
                    .imports
                    .as_ref()
                    .map(|i| i.source_target_id.clone()),
            )
        {
            return Err(mismatch());
        }
        Ok(())
    }
    pub fn write(
        &mut self,
        source: &Contents,
        cancel: &AtomicBool,
        observe: &mut impl FnMut(Point) -> Result<()>,
    ) -> Result<()> {
        let tx = self
            .store
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        if state(&tx)? != 0 {
            return Ok(());
        }
        for (table, _) in &SCHEMA[1..] {
            let n: i64 = tx
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .map_err(db_error)?;
            if n != 0 {
                return Err(mismatch());
            }
        }
        let facts = &source.facts;
        for (table, refs) in [
            ("restore_accounts", &facts.accounts),
            ("restore_categories", &facts.categories),
        ] {
            for (n, r) in refs.iter().enumerate() {
                check_cancel(cancel)?;
                tx.execute(
                    &format!("INSERT INTO {table} VALUES(?1,?2,?3,?4,?5)"),
                    params![n as i64, r.id, r.kind, r.name, r.archived],
                )
                .map_err(db_error)?;
            }
        }
        observe(Point::References)?;
        check_cancel(cancel)?;
        for (n, r) in facts.revisions.iter().enumerate() {
            check_cancel(cancel)?;
            tx.execute(
                "INSERT INTO restore_revisions VALUES(?1,?2,?3,?4,?5)",
                params![
                    n as i64,
                    r.id,
                    r.event_id,
                    r.predecessor,
                    r.event.as_ref().map(encode).transpose()?
                ],
            )
            .map_err(db_error)?;
        }
        observe(Point::Revisions)?;
        check_cancel(cancel)?;
        for (table, values) in [
            ("restore_selected", &facts.selected),
            ("restore_candidates", &facts.candidates),
        ] {
            for (n, r) in values.iter().enumerate() {
                check_cancel(cancel)?;
                tx.execute(
                    &format!("INSERT INTO {table} VALUES(?1,?2,?3)"),
                    params![n as i64, r.event_id, r.revision_id],
                )
                .map_err(db_error)?;
            }
        }
        observe(Point::Selections)?;
        check_cancel(cancel)?;
        insert_history(&tx, "restore_intents", &facts.intent_history, cancel)?;
        if let Some(i) = &facts.imports {
            insert_history(&tx, "restore_import_tasks", &i.tasks, cancel)?;
            insert_history(&tx, "restore_import_rows", &i.rows, cancel)?;
            insert_history(&tx, "restore_import_sources", &i.sources, cancel)?;
            insert_history(&tx, "restore_import_batches", &i.batches, cancel)?;
            insert_history(&tx, "restore_undo_tasks", &i.undo_tasks, cancel)?;
            insert_history(&tx, "restore_undo_rows", &i.undo_rows, cancel)?;
            insert_history(&tx, "restore_undo_batches", &i.undo_batches, cancel)?;
        }
        observe(Point::History)?;
        check_cancel(cancel)?;
        tx.execute("UPDATE restore_control SET state=1 WHERE id=1", [])
            .map_err(db_error)?;
        observe(Point::BeforeCommit)?;
        check_cancel(cancel)?;
        tx.commit().map_err(db_error)?;
        observe(Point::Written)?;
        check_cancel(cancel)
    }
    pub fn verify(
        &self,
        source: &Contents,
        limits: Limits,
        cancel: &AtomicBool,
    ) -> Result<Contents> {
        check_cancel(cancel)?;
        let tx = self
            .store
            .connection
            .unchecked_transaction()
            .map_err(db_error)?;
        self.check_source(source)?;
        if state(&tx)? == 0 {
            return Err(CoreError::new(
                "restore_incomplete",
                "暂存事实尚未完整提交，请重新认证并继续原任务",
            ));
        }
        let mut budget = ReadBudget {
            limits,
            rows: 0,
            bytes: 0,
            cancel,
        };
        let accounts = references(&tx, "restore_accounts", &mut budget)?;
        let categories = references(&tx, "restore_categories", &mut budget)?;
        let revisions = read_rows(&tx, "restore_revisions", &mut budget, |r, b| {
            Ok(Revision {
                id: b.text(r, 1)?,
                event_id: b.text(r, 2)?,
                predecessor: b.optional(r, 3)?,
                event: b
                    .optional(r, 4)?
                    .map(|s| decode(&s, limits, cancel))
                    .transpose()?,
            })
        })?;
        let selected = selections(&tx, "restore_selected", &mut budget)?;
        let candidates = selections(&tx, "restore_candidates", &mut budget)?;
        let intent_history = history(&tx, "restore_intents", &mut budget)?;
        let tasks = history(&tx, "restore_import_tasks", &mut budget)?;
        let rows = history(&tx, "restore_import_rows", &mut budget)?;
        let sources = history(&tx, "restore_import_sources", &mut budget)?;
        let batches = history(&tx, "restore_import_batches", &mut budget)?;
        let undo_tasks = history(&tx, "restore_undo_tasks", &mut budget)?;
        let undo_rows = history(&tx, "restore_undo_rows", &mut budget)?;
        let undo_batches = history(&tx, "restore_undo_batches", &mut budget)?;
        let imports = if let Some(i) = &source.facts.imports {
            Some(ImportHistory {
                source_target_id: i.source_target_id.clone(),
                tasks,
                rows,
                sources,
                batches,
                undo_tasks,
                undo_rows,
                undo_batches,
            })
        } else {
            if !tasks.is_empty()
                || !rows.is_empty()
                || !sources.is_empty()
                || !batches.is_empty()
                || !undo_tasks.is_empty()
                || !undo_rows.is_empty()
                || !undo_batches.is_empty()
            {
                return Err(mismatch());
            }
            None
        };
        let facts = Facts {
            profile: PROFILE.into(),
            source_schema: source.facts.source_schema,
            sequence: source.facts.sequence,
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
        // 在关系遍历 / 投影重建前核对累计 JSON 工作量；单行检查不替代总预算。
        bounded(
            &Review {
                facts: &facts,
                verification: &source.verification,
            },
            limits,
            cancel,
        )?;
        let months: Vec<_> = source
            .verification
            .projection
            .monthly_reports
            .keys()
            .cloned()
            .collect();
        // 同一事实关系 / 金额 / 报告重建入口，不顺序重放历史意图。
        let verification = snapshot_store::validate::verify(&facts, &months, cancel)
            .map_err(|e| if e.code == "cancelled" { e } else { mismatch() })?;
        let output = Contents {
            facts,
            verification,
        };
        let actual = bounded(&output, limits, cancel)?;
        let expected = bounded(source, limits, cancel)?;
        if *actual != *expected {
            return Err(mismatch());
        }
        check_cancel(cancel)?;
        tx.commit().map_err(db_error)?;
        Ok(output)
    }
    pub fn mark_verified(&self) -> Result<()> {
        let changed = self
            .store
            .connection
            .execute(
                "UPDATE restore_control SET state=2 WHERE id=1 AND state IN (1,2)",
                [],
            )
            .map_err(db_error)?;
        if changed != 1 {
            return Err(mismatch());
        }
        Ok(())
    }
}

fn insert_history<T: Serialize>(
    tx: &Transaction<'_>,
    table: &str,
    values: &[T],
    cancel: &AtomicBool,
) -> Result<()> {
    for (n, value) in values.iter().enumerate() {
        check_cancel(cancel)?;
        tx.execute(
            &format!("INSERT INTO {table} VALUES(?1,?2)"),
            params![n as i64, encode(value)?],
        )
        .map_err(db_error)?;
    }
    Ok(())
}
fn decode<T: DeserializeOwned>(text: &str, limits: Limits, cancel: &AtomicBool) -> Result<T> {
    scan(text.as_bytes(), limits, cancel)?;
    serde_json::from_str(text).map_err(|_| mismatch())
}
struct ReadBudget<'a> {
    limits: Limits,
    rows: usize,
    bytes: usize,
    cancel: &'a AtomicBool,
}
impl ReadBudget<'_> {
    fn text(&mut self, row: &rusqlite::Row<'_>, column: usize) -> Result<String> {
        check_cancel(self.cancel)?;
        let value = row
            .get_ref(column)
            .map_err(db_error)?
            .as_str()
            .map_err(|_| mismatch())?;
        self.bytes = self.bytes.checked_add(value.len()).ok_or_else(mismatch)?;
        if self.bytes > self.limits.payload_bytes {
            return Err(CoreError::new("archive_limit", "暂存读取超过字节预算"));
        }
        Ok(value.to_owned())
    }
    fn optional(&mut self, row: &rusqlite::Row<'_>, column: usize) -> Result<Option<String>> {
        if row.get_ref(column).map_err(db_error)? == rusqlite::types::ValueRef::Null {
            return Ok(None);
        }
        self.text(row, column).map(Some)
    }
}
fn read_rows<T>(
    connection: &Connection,
    table: &str,
    budget: &mut ReadBudget<'_>,
    mut map: impl FnMut(&rusqlite::Row<'_>, &mut ReadBudget<'_>) -> Result<T>,
) -> Result<Vec<T>> {
    let mut query = connection
        .prepare(&format!("SELECT * FROM {table} ORDER BY position"))
        .map_err(db_error)?;
    let mut rows = query.query([]).map_err(db_error)?;
    let mut output = Vec::new();
    while let Some(row) = rows.next().map_err(db_error)? {
        check_cancel(budget.cancel)?;
        if budget.rows == budget.limits.records {
            return Err(CoreError::new("archive_limit", "暂存读取超过记录预算"));
        }
        budget.rows += 1;
        let position: i64 = row.get(0).map_err(db_error)?;
        if position != output.len() as i64 {
            return Err(mismatch());
        }
        output.push(map(row, budget)?);
    }
    Ok(output)
}
fn references(
    connection: &Connection,
    table: &str,
    budget: &mut ReadBudget<'_>,
) -> Result<Vec<Reference>> {
    read_rows(connection, table, budget, |r, b| {
        let archived: i64 = r.get(4).map_err(db_error)?;
        if !matches!(archived, 0 | 1) {
            return Err(mismatch());
        }
        Ok(Reference {
            id: b.text(r, 1)?,
            kind: b.text(r, 2)?,
            name: b.text(r, 3)?,
            archived: archived == 1,
        })
    })
}
fn selections(
    connection: &Connection,
    table: &str,
    budget: &mut ReadBudget<'_>,
) -> Result<Vec<Selection>> {
    read_rows(connection, table, budget, |r, b| {
        Ok(Selection {
            event_id: b.text(r, 1)?,
            revision_id: b.text(r, 2)?,
        })
    })
}
fn history<T: DeserializeOwned>(
    connection: &Connection,
    table: &str,
    budget: &mut ReadBudget<'_>,
) -> Result<Vec<T>> {
    read_rows(connection, table, budget, |r, b| {
        decode(&b.text(r, 1)?, b.limits, b.cancel)
    })
}

#[cfg(test)]
pub(crate) fn test_sql(path: &Path, key: &DbKey, sql: &str) -> Result<()> {
    Store::connect(path, key, false)?
        .connection
        .execute_batch(sql)
        .map_err(db_error)
}
#[cfg(test)]
pub(crate) fn test_count(path: &Path, key: &DbKey, sql: &str) -> Result<i64> {
    Store::connect(path, key, true)?
        .connection
        .query_row(sql, [], |r| r.get(0))
        .map_err(db_error)
}
#[cfg(test)]
pub(crate) fn test_integrity(path: &Path, key: &DbKey) -> Result<()> {
    let store = Store::connect(path, key, true)?;
    let result: String = store
        .connection
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(db_error)?;
    if result != "ok" {
        return Err(mismatch());
    }
    Ok(())
}

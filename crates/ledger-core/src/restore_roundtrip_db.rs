//! SQLCipher 暂存 0.2 的规范化载荷，只有任务归属与一个原子写入点。
use super::*;
use crate::archive_container::restored::{RestoredContents, decode};
use sha2::{Digest, Sha256};

const VERSION: i64 = 1004;

pub(crate) struct RestoredStageDb(Store);

impl RestoredStageDb {
    pub(crate) fn prepare(
        path: &Path,
        key: &DbKey,
        ticket: &Ticket,
        can_initialize: bool,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        check_cancel(cancel)?;
        let mut store = Store::connect(path, key, false)?;
        if can_initialize && Self::uninitialized(&store.connection)? {
            let tx = store
                .connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(db_error)?;
            tx.execute_batch(
                "CREATE TABLE restore_bundle (
                    id INTEGER PRIMARY KEY CHECK(id=1),
                    ticket TEXT NOT NULL,
                    payload BLOB
                );",
            )
            .map_err(db_error)?;
            tx.execute(
                "INSERT INTO restore_bundle(id,ticket) VALUES(1,?1)",
                [encode(ticket)?],
            )
            .map_err(db_error)?;
            tx.pragma_update(None, "user_version", VERSION)
                .map_err(db_error)?;
            check_cancel(cancel)?;
            tx.commit().map_err(db_error)?;
        }
        Self::binding(&store.connection, ticket)?;
        Ok(Self(store))
    }

    pub(crate) fn open(path: &Path, key: &DbKey, ticket: &Ticket) -> Result<Self> {
        let store = Store::connect(path, key, true)?;
        Self::binding(&store.connection, ticket)?;
        Ok(Self(store))
    }

    pub(crate) fn check_abandon(
        path: &Path,
        key: &DbKey,
        ticket: &Ticket,
        can_initialize: bool,
    ) -> Result<()> {
        let store = Store::connect(path, key, true)?;
        if can_initialize && Self::uninitialized(&store.connection)? {
            return Ok(());
        }
        Self::binding(&store.connection, ticket)
    }

    fn uninitialized(connection: &Connection) -> Result<bool> {
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(db_error)?;
        let empty: bool = connection
            .query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM sqlite_schema)",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        Ok(version == 0 && empty)
    }

    fn binding(connection: &Connection, ticket: &Ticket) -> Result<()> {
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(db_error)?;
        let single_table: bool = connection.query_row(
            "SELECT count(*)=1 AND min(name='restore_bundle' AND type='table') FROM sqlite_schema",
            [], |row| row.get(0),
        ).map_err(db_error)?;
        if version != VERSION || !single_table {
            return Err(mismatch());
        }
        let columns_match: bool = connection
            .query_row(
                "SELECT count(*)=3 AND sum(
                (cid=0 AND name='id' AND type='INTEGER' AND pk=1 AND hidden=0)
                OR (cid=1 AND name='ticket' AND type='TEXT' AND \"notnull\"=1 AND hidden=0)
                OR (cid=2 AND name='payload' AND type='BLOB' AND hidden=0)
            )=3 FROM pragma_table_xinfo('restore_bundle')",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if !columns_match {
            return Err(mismatch());
        }
        let actual: String = connection.query_row(
            "SELECT ticket FROM restore_bundle WHERE id=1 AND length(CAST(ticket AS BLOB))<=4096",
            [], |row| row.get(0),
        ).map_err(db_error)?;
        if actual != encode(ticket)? {
            return Err(mismatch());
        }
        Ok(())
    }

    pub(crate) fn write(
        &mut self,
        payload: &[u8],
        cancel: &AtomicBool,
        before_commit: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let tx = self
            .0
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        tx.execute(
            "UPDATE restore_bundle SET payload=?1 WHERE id=1 AND payload IS NULL",
            [payload],
        )
        .map_err(db_error)?;
        before_commit()?;
        check_cancel(cancel)?;
        tx.commit().map_err(db_error)
    }

    pub(crate) fn verify(
        &self,
        expected: &[u8; 32],
        limits: Limits,
        cancel: &AtomicBool,
    ) -> Result<RestoredContents> {
        let mut stmt = self
            .0
            .connection
            .prepare("SELECT payload FROM restore_bundle WHERE id=1")
            .map_err(db_error)?;
        let mut rows = stmt.query([]).map_err(db_error)?;
        let row = rows.next().map_err(db_error)?.ok_or_else(mismatch)?;
        let payload = row
            .get_ref(0)
            .map_err(db_error)?
            .as_blob()
            .map_err(|_| mismatch())?;
        check_cancel(cancel)?;
        if payload.len() > limits.payload_bytes
            || &<[u8; 32]>::from(Sha256::digest(payload)) != expected
        {
            return Err(mismatch());
        }
        decode(payload, limits, cancel)
    }
}

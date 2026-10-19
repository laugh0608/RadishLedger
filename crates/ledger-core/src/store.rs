use crate::domain::{Event, EventKind, Projection, minor, project};
use crate::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use zeroize::Zeroizing;

#[cfg(test)]
#[path = "store_file_tests.rs"]
mod file_tests;

pub struct DbKey(Zeroizing<[u8; 32]>);

impl DbKey {
    pub(crate) fn from_secret(bytes: &[u8]) -> Result<Self> {
        Ok(Self(crate::crypto::secret(bytes)?))
    }
    // 公开合成材料，仅供 M0 跨进程故障实验；不是用户密钥或恢复方案。
    pub(crate) fn public_recovery_fixture() -> Self {
        Self(Zeroizing::new([0xA7; 32]))
    }

    pub fn random() -> Result<Self> {
        let mut bytes = Zeroizing::new([0_u8; 32]);
        getrandom::fill(bytes.as_mut())
            .map_err(|_| CoreError::new("random_unavailable", "系统随机源不可用"))?;
        Ok(Self(bytes))
    }
}

fn db_error(error: rusqlite::Error) -> CoreError {
    // SQL / 绑定值不进入错误通道；不泄露账务或解密材料。
    let mut result = CoreError::new("storage_error", "加密存储操作失败，原有数据保留");
    if let rusqlite::Error::SqliteFailure(code, _) = error {
        result.sqlite_code = Some(code.extended_code);
    }
    result
}

pub struct Store {
    connection: Connection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Write {
    pub operation_id: String,
    pub event_id: String,
    pub revision_id: String,
    pub previous_revision_id: Option<String>,
    pub event: Option<Event>,
    pub expected_sequence: Option<i64>,
    pub observed_balance_minor: Option<String>,
    pub target_balance_minor: Option<String>,
    pub resolve_candidates: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteEffect {
    Committed,
    Replayed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    #[serde(flatten)]
    pub projection: Projection,
    pub active_event_ids: BTreeSet<String>,
    pub selected_revision_ids: BTreeMap<String, String>,
    pub deleted_event_ids: BTreeSet<String>,
    pub unresolved_candidate_revision_ids: BTreeSet<String>,
    pub archived_account_ids: BTreeSet<String>,
    pub archived_category_ids: BTreeSet<String>,
}

fn valid_id(id: &str) -> Result<()> {
    if id.len() != 32
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(CoreError::new("invalid_identity", "对象身份无效"));
    }
    Ok(())
}

impl Store {
    pub fn create(path: &Path, key: &DbKey) -> Result<Self> {
        // 仅创建全新实验 DB，不能把既有打不开的库当作空白覆盖。
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|_| CoreError::new("create_failed", "无法创建新的隔离账库"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| CoreError::new("permissions_failed", "账库文件权限设置失败"))?;
        }
        drop(file);
        let store = Self::connect(path, key, false)?;
        store.connection.execute_batch(
            "BEGIN IMMEDIATE;
            CREATE TABLE accounts (id TEXT PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE categories (id TEXT PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE revisions (id TEXT PRIMARY KEY, event_id TEXT NOT NULL, predecessor TEXT, payload TEXT);
            CREATE TABLE selected (event_id TEXT PRIMARY KEY, revision_id TEXT NOT NULL REFERENCES revisions(id));
            CREATE TABLE outbox (operation_id TEXT PRIMARY KEY, intent TEXT NOT NULL, revision_id TEXT NOT NULL REFERENCES revisions(id));
            CREATE TABLE candidates (revision_id TEXT PRIMARY KEY REFERENCES revisions(id), event_id TEXT NOT NULL);
            CREATE TABLE metadata (id INTEGER PRIMARY KEY CHECK(id = 1), sequence INTEGER NOT NULL CHECK(typeof(sequence)='integer' AND sequence >= 0));
            INSERT INTO metadata VALUES (1, 0);
            PRAGMA user_version=1;
            COMMIT;"
        ).map_err(db_error)?;
        Ok(store)
    }

    pub fn open(path: &Path, key: &DbKey) -> Result<Self> {
        Self::open_mode(path, key, false)
    }

    pub(crate) fn open_readonly(path: &Path, key: &DbKey) -> Result<Self> {
        Self::open_mode(path, key, true)
    }

    fn open_mode(path: &Path, key: &DbKey, readonly: bool) -> Result<Self> {
        if !path.is_file() {
            return Err(CoreError::new("missing_database", "原账库文件不存在"));
        }
        let store = Self::connect(path, key, readonly)?;
        let version: i64 = store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(db_error)?;
        if version != 1 {
            return Err(CoreError::new("unsupported_schema", "实验账库版本不兼容"));
        }
        Ok(store)
    }

    fn connect(path: &Path, key: &DbKey, readonly: bool) -> Result<Self> {
        let flags = if readonly {
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
        };
        let connection = Connection::open_with_flags(path, flags).map_err(db_error)?;
        // SAFETY: 连接归本函数所有；32 字节密钥仅在调用期间借用，不通过 SQL / 日志传递。
        let status =
            unsafe { rusqlite::ffi::sqlite3_key(connection.handle(), key.0.as_ptr().cast(), 32) };
        if status != rusqlite::ffi::SQLITE_OK {
            return Err(CoreError::new("key_failed", "本机密钥不可用"));
        }
        let cipher: String = connection
            .pragma_query_value(None, "cipher_version", |row| row.get(0))
            .map_err(db_error)?;
        if cipher.is_empty() {
            return Err(CoreError::new("cipher_unavailable", "实际构建未启用加密库"));
        }
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA temp_store=MEMORY;",
            )
            .map_err(db_error)?;
        if !readonly {
            connection
                .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
                .map_err(db_error)?;
        }
        connection
            .busy_timeout(std::time::Duration::from_secs(2))
            .map_err(db_error)?;
        Ok(Self { connection })
    }

    pub(crate) fn bind_bootstrap(&self, vault: &str, device: &str) -> Result<()> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        tx.execute_batch("CREATE TABLE bootstrap_identity (id INTEGER PRIMARY KEY CHECK(id=1), vault TEXT NOT NULL, device TEXT NOT NULL); CREATE TABLE bootstrap_fixture (id INTEGER PRIMARY KEY CHECK(id=1), digest TEXT NOT NULL);").map_err(db_error)?;
        tx.execute(
            "INSERT INTO bootstrap_identity VALUES (1, ?1, ?2)",
            params![vault, device],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)
    }

    pub(crate) fn verify_bootstrap(&self, vault: &str, device: &str) -> Result<()> {
        let identity: (String, String) = self
            .connection
            .query_row(
                "SELECT vault, device FROM bootstrap_identity WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(db_error)?;
        if identity != (vault.into(), device.into()) {
            return Err(CoreError::new(
                "identity_mismatch",
                "封装与原账库身份不一致",
            ));
        }
        Ok(())
    }

    pub(crate) fn content_digest(&self) -> Result<String> {
        use rusqlite::types::ValueRef;
        use sha2::{Digest, Sha256};
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        let mut digest = Sha256::new();
        // 固定内部表名；完整业务内容，而非只比较余额或行数。
        for (table, key) in [
            ("accounts", "id"),
            ("categories", "id"),
            ("revisions", "id"),
            ("selected", "event_id"),
            ("outbox", "operation_id"),
            ("candidates", "revision_id"),
            ("metadata", "id"),
        ] {
            digest.update((table.len() as u64).to_be_bytes());
            digest.update(table.as_bytes());
            let mut statement = tx
                .prepare(&format!("SELECT * FROM {table} ORDER BY {key}"))
                .map_err(db_error)?;
            let count = statement.column_count();
            let mut rows = statement.query([]).map_err(db_error)?;
            while let Some(row) = rows.next().map_err(db_error)? {
                digest.update([0xff]);
                for column in 0..count {
                    match row.get_ref(column).map_err(db_error)? {
                        ValueRef::Null => digest.update([0]),
                        ValueRef::Integer(n) => {
                            digest.update([1]);
                            digest.update(n.to_be_bytes());
                        }
                        ValueRef::Text(bytes) => {
                            digest.update([2]);
                            digest.update((bytes.len() as u64).to_be_bytes());
                            digest.update(bytes);
                        }
                        _ => {
                            return Err(CoreError::new(
                                "invalid_storage_type",
                                "账务内容存在非预期存储类型",
                            ));
                        }
                    }
                }
            }
            digest.update([0xfe]);
        }
        tx.commit().map_err(db_error)?;
        Ok(crate::crypto::hex(&digest.finalize()))
    }

    pub(crate) fn fixture_digest(&self) -> Result<Option<String>> {
        self.connection
            .query_row("SELECT digest FROM bootstrap_fixture WHERE id=1", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(db_error)
    }

    pub(crate) fn save_fixture_digest(&self, digest: &str) -> Result<()> {
        self.connection
            .execute("INSERT INTO bootstrap_fixture VALUES (1, ?1)", [digest])
            .map_err(db_error)?;
        Ok(())
    }

    /// 仅重放本隔离合成账库内已提交的原意图，不生成新 operation identity。
    pub(crate) fn replay_fixture_outbox(&mut self) -> Result<()> {
        let writes = {
            let mut statement = self
                .connection
                .prepare("SELECT intent FROM outbox ORDER BY operation_id")
                .map_err(db_error)?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(db_error)?
                .map(|row| {
                    serde_json::from_str::<Write>(&row.map_err(db_error)?)
                        .map_err(|_| CoreError::new("corrupt_fact", "合成原意图无法解释"))
                })
                .collect::<Result<Vec<_>>>()?
        };
        for write in writes {
            if self.commit(&write)? != WriteEffect::Replayed {
                return Err(CoreError::new(
                    "fixture_mismatch",
                    "已提交的合成意图不应重复入队",
                ));
            }
        }
        Ok(())
    }

    pub fn cipher_version(&self) -> Result<String> {
        self.connection
            .pragma_query_value(None, "cipher_version", |row| row.get(0))
            .map_err(db_error)
    }

    pub(crate) fn verify_atomic_rejection(&mut self, write: &Write) -> Result<()> {
        // 仅由固定合成 probe 使用：在 selected / revisions 写入后真实拒绝 outbox。
        let before = serde_json::to_value(self.snapshot(&[])?)
            .map_err(|_| CoreError::new("encoding_error", "实验快照无法编码"))?;
        let count = self.outbox_count()?;
        let sequence = self.sequence()?;
        let revisions: i64 = self
            .connection
            .query_row("SELECT count(*) FROM revisions", [], |row| row.get(0))
            .map_err(db_error)?;
        self.connection.execute_batch("CREATE TRIGGER m0_reject_outbox BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT, 'synthetic_outbox_failure'); END;").map_err(db_error)?;
        let result = self.commit(write);
        self.connection
            .execute_batch("DROP TRIGGER m0_reject_outbox;")
            .map_err(db_error)?;
        if result.err().and_then(|error| error.sqlite_code)
            != Some(rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER)
        {
            return Err(CoreError::new(
                "atomic_probe_failed",
                "未在预期持久化边界拒绝写入",
            ));
        }
        let after = serde_json::to_value(self.snapshot(&[])?)
            .map_err(|_| CoreError::new("encoding_error", "实验快照无法编码"))?;
        let after_revisions: i64 = self
            .connection
            .query_row("SELECT count(*) FROM revisions", [], |row| row.get(0))
            .map_err(db_error)?;
        if before != after
            || count != self.outbox_count()?
            || sequence != self.sequence()?
            || revisions != after_revisions
        {
            return Err(CoreError::new(
                "atomic_probe_failed",
                "事务拒绝后出现部分账务或队列",
            ));
        }
        Ok(())
    }

    pub fn add_account(&mut self, id: &str, kind: &str, name: &str) -> Result<()> {
        valid_id(id)?;
        if !matches!(kind, "cash" | "bank" | "wallet" | "credit_card") || name.len() > 256 {
            return Err(CoreError::new("invalid_account", "账户类型或名称无效"));
        }
        self.connection
            .execute(
                "INSERT INTO accounts(id,kind,name) VALUES (?1,?2,?3)",
                params![id, kind, name],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn add_category(&mut self, id: &str, kind: &str, name: &str) -> Result<()> {
        valid_id(id)?;
        if !matches!(kind, "income" | "expense") || name.len() > 256 {
            return Err(CoreError::new("invalid_category", "分类类型或名称无效"));
        }
        self.connection
            .execute(
                "INSERT INTO categories(id,kind,name) VALUES (?1,?2,?3)",
                params![id, kind, name],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn sequence(&self) -> Result<i64> {
        self.connection
            .query_row("SELECT sequence FROM metadata WHERE id=1", [], |row| {
                row.get(0)
            })
            .map_err(db_error)
    }

    pub fn outbox_count(&self) -> Result<usize> {
        let count: i64 = self
            .connection
            .query_row("SELECT count(*) FROM outbox", [], |row| row.get(0))
            .map_err(db_error)?;
        usize::try_from(count).map_err(|_| CoreError::new("corrupt_metadata", "队列数量无效"))
    }

    pub fn commit(&mut self, write: &Write) -> Result<WriteEffect> {
        self.commit_observed(write, |_| Ok(()))
    }

    // 实验观测点与正常提交共用唯一事务路径；不提供第二套保存实现。
    pub(crate) fn commit_observed(
        &mut self,
        write: &Write,
        mut observe: impl FnMut(crate::validation::CommitPoint) -> Result<()>,
    ) -> Result<WriteEffect> {
        for id in [&write.operation_id, &write.event_id, &write.revision_id] {
            valid_id(id)?;
        }
        let intent = serde_json::to_string(write)
            .map_err(|_| CoreError::new("encoding_error", "账务意图无法编码"))?;
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let prior: Option<String> = tx
            .query_row(
                "SELECT intent FROM outbox WHERE operation_id=?1",
                [&write.operation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if let Some(prior) = prior {
            return if prior == intent {
                Ok(WriteEffect::Replayed)
            } else {
                Err(CoreError::new(
                    "operation_reused",
                    "同一操作身份不能用于不同账务意图",
                ))
            };
        }
        let selected: Option<String> = tx
            .query_row(
                "SELECT revision_id FROM selected WHERE event_id=?1",
                [&write.event_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if selected != write.previous_revision_id {
            return Err(CoreError::new("stale_revision", "原修订已变化，请重新核对"));
        }
        let candidates = candidate_ids(&tx, &write.event_id)?;
        if candidates != write.resolve_candidates {
            return Err(CoreError::new(
                "stale_candidates",
                "冲突候选已变化，请重新核对",
            ));
        }
        if let Some(sequence) = write.expected_sequence {
            let current: i64 = tx
                .query_row("SELECT sequence FROM metadata WHERE id=1", [], |row| {
                    row.get(0)
                })
                .map_err(db_error)?;
            if current != sequence {
                return Err(CoreError::new("stale_observation", "对账观察已过期"));
            }
        }
        if let Some(event) = &write.event {
            validate_references(&tx, event)?;
            if event.kind == EventKind::Adjustment {
                let observed = write
                    .observed_balance_minor
                    .as_deref()
                    .ok_or_else(|| CoreError::new("invalid_adjustment", "缺少观察余额"))?;
                let target = write
                    .target_balance_minor
                    .as_deref()
                    .ok_or_else(|| CoreError::new("invalid_adjustment", "缺少目标余额"))?;
                if write.expected_sequence.is_none()
                    || i128::from(minor(target)?) - i128::from(minor(observed)?)
                        != i128::from(event.validate()?)
                {
                    return Err(CoreError::new(
                        "invalid_adjustment",
                        "对账差额或观察版本无效",
                    ));
                }
                let actual = read_snapshot(&tx, &[])?.projection.balances_minor;
                if event
                    .account_id
                    .as_ref()
                    .and_then(|id| actual.get(id))
                    .map(String::as_str)
                    != Some(observed)
                {
                    return Err(CoreError::new(
                        "invalid_adjustment",
                        "观察余额与当前事实不一致",
                    ));
                }
            }
        } else if selected.is_none() {
            return Err(CoreError::new("missing_event", "待删除交易不存在"));
        }
        let payload = write
            .event
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| CoreError::new("encoding_error", "账务事实无法编码"))?;
        tx.execute(
            "INSERT INTO revisions(id,event_id,predecessor,payload) VALUES (?1,?2,?3,?4)",
            params![
                write.revision_id,
                write.event_id,
                write.previous_revision_id,
                payload
            ],
        )
        .map_err(db_error)?;
        tx.execute("INSERT INTO selected(event_id,revision_id) VALUES (?1,?2) ON CONFLICT(event_id) DO UPDATE SET revision_id=excluded.revision_id", params![write.event_id,write.revision_id]).map_err(db_error)?;
        observe(crate::validation::CommitPoint::BeforeOutbox)?;
        tx.execute(
            "INSERT INTO outbox(operation_id,intent,revision_id) VALUES (?1,?2,?3)",
            params![write.operation_id, intent, write.revision_id],
        )
        .map_err(db_error)?;
        tx.execute(
            "DELETE FROM candidates WHERE event_id=?1",
            [&write.event_id],
        )
        .map_err(db_error)?;
        tx.execute("UPDATE metadata SET sequence=sequence+1 WHERE id=1", [])
            .map_err(db_error)?;
        observe(crate::validation::CommitPoint::BeforeCommit)?;
        tx.commit().map_err(db_error)?;
        observe(crate::validation::CommitPoint::AfterCommit)?;
        Ok(WriteEffect::Committed)
    }

    pub fn receive_candidate(
        &mut self,
        event_id: &str,
        revision_id: &str,
        predecessor: &str,
        event: &Event,
    ) -> Result<()> {
        for id in [event_id, revision_id, predecessor] {
            valid_id(id)?;
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        validate_references(&tx, event)?;
        let parent: Option<String> = tx
            .query_row(
                "SELECT event_id FROM revisions WHERE id=?1",
                [predecessor],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if parent.as_deref() != Some(event_id) {
            return Err(CoreError::new("missing_predecessor", "候选前置修订不存在"));
        }
        let current: String = tx
            .query_row(
                "SELECT revision_id FROM selected WHERE event_id=?1",
                [event_id],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if current == predecessor {
            return Err(CoreError::new("not_concurrent", "该修订不是并发候选"));
        }
        let payload = serde_json::to_string(event)
            .map_err(|_| CoreError::new("encoding_error", "候选无法编码"))?;
        tx.execute(
            "INSERT INTO revisions(id,event_id,predecessor,payload) VALUES (?1,?2,?3,?4)",
            params![revision_id, event_id, predecessor, payload],
        )
        .map_err(db_error)?;
        for id in [current.as_str(), revision_id] {
            tx.execute(
                "INSERT OR IGNORE INTO candidates(revision_id,event_id) VALUES (?1,?2)",
                params![id, event_id],
            )
            .map_err(db_error)?;
        }
        tx.execute("UPDATE metadata SET sequence=sequence+1 WHERE id=1", [])
            .map_err(db_error)?;
        tx.commit().map_err(db_error)
    }

    pub fn revision_event(&self, revision: &str) -> Result<Option<Event>> {
        let payload: Option<String> = self
            .connection
            .query_row(
                "SELECT payload FROM revisions WHERE id=?1",
                [revision],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        payload
            .map(|value| {
                serde_json::from_str(&value)
                    .map_err(|_| CoreError::new("corrupt_fact", "账务事实无法解释"))
            })
            .transpose()
    }

    pub fn archive(&mut self, accounts: &[String], categories: &[String]) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        for id in accounts {
            if tx
                .execute("UPDATE accounts SET archived=1 WHERE id=?1", [id])
                .map_err(db_error)?
                != 1
            {
                return Err(CoreError::new("missing_account", "资金账户不存在"));
            }
        }
        for id in categories {
            if tx
                .execute("UPDATE categories SET archived=1 WHERE id=?1", [id])
                .map_err(db_error)?
                != 1
            {
                return Err(CoreError::new("missing_category", "分类不存在"));
            }
        }
        tx.execute("UPDATE metadata SET sequence=sequence+1 WHERE id=1", [])
            .map_err(db_error)?;
        tx.commit().map_err(db_error)
    }

    pub fn rename(
        &mut self,
        accounts: &BTreeMap<String, String>,
        categories: &BTreeMap<String, String>,
    ) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db_error)?;
        for (id, name) in accounts {
            if name.len() > 256
                || tx
                    .execute("UPDATE accounts SET name=?2 WHERE id=?1", params![id, name])
                    .map_err(db_error)?
                    != 1
            {
                return Err(CoreError::new("invalid_account", "账户改名失败"));
            }
        }
        for (id, name) in categories {
            if name.len() > 256
                || tx
                    .execute(
                        "UPDATE categories SET name=?2 WHERE id=?1",
                        params![id, name],
                    )
                    .map_err(db_error)?
                    != 1
            {
                return Err(CoreError::new("invalid_category", "分类改名失败"));
            }
        }
        tx.execute("UPDATE metadata SET sequence=sequence+1 WHERE id=1", [])
            .map_err(db_error)?;
        tx.commit().map_err(db_error)
    }

    pub(crate) fn validation_audit(&self) -> Result<crate::validation::Audit> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        let snapshot = read_snapshot(&tx, &[])?;
        let sequence = tx
            .query_row("SELECT sequence FROM metadata WHERE id=1", [], |r| r.get(0))
            .map_err(db_error)?;
        let revisions = tx
            .query_row("SELECT count(*) FROM revisions", [], |r| r.get(0))
            .map_err(db_error)?;
        let operations = {
            let mut stmt = tx
                .prepare("SELECT operation_id FROM outbox ORDER BY operation_id")
                .map_err(db_error)?;
            stmt.query_map([], |r| r.get(0))
                .map_err(db_error)?
                .collect::<std::result::Result<Vec<String>, _>>()
                .map_err(db_error)?
        };
        tx.commit().map_err(db_error)?;
        Ok(crate::validation::Audit {
            snapshot,
            sequence,
            revisions,
            operations,
        })
    }

    pub fn snapshot(&self, months: &[String]) -> Result<Snapshot> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        let snapshot = read_snapshot(&tx, months)?;
        tx.commit().map_err(db_error)?;
        Ok(snapshot)
    }
}

fn read_snapshot(connection: &Connection, months: &[String]) -> Result<Snapshot> {
    let ids = |sql: &str| -> Result<Vec<String>> {
        connection
            .prepare(sql)
            .map_err(db_error)?
            .query_map([], |row| row.get(0))
            .map_err(db_error)?
            .collect::<std::result::Result<_, _>>()
            .map_err(db_error)
    };
    let accounts = ids("SELECT id FROM accounts ORDER BY id")?;
    let categories = ids("SELECT id FROM categories WHERE kind='expense' ORDER BY id")?;
    let mut selected = BTreeMap::new();
    let mut events = Vec::new();
    let mut active = BTreeSet::new();
    let mut deleted = BTreeSet::new();
    let mut stmt = connection.prepare("SELECT s.event_id,s.revision_id,r.payload FROM selected s JOIN revisions r ON r.id=s.revision_id ORDER BY s.event_id").map_err(db_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(db_error)?;
    for row in rows {
        let (event, revision, payload) = row.map_err(db_error)?;
        selected.insert(event.clone(), revision);
        if let Some(payload) = payload {
            events.push(
                serde_json::from_str::<Event>(&payload)
                    .map_err(|_| CoreError::new("corrupt_fact", "账务事实无法解释"))?,
            );
            active.insert(event);
        } else {
            deleted.insert(event);
        }
    }
    Ok(Snapshot {
        projection: project(&accounts, &categories, &events, months)?,
        active_event_ids: active,
        selected_revision_ids: selected,
        deleted_event_ids: deleted,
        unresolved_candidate_revision_ids: ids(
            "SELECT revision_id FROM candidates ORDER BY revision_id",
        )?
        .into_iter()
        .collect(),
        archived_account_ids: ids("SELECT id FROM accounts WHERE archived=1 ORDER BY id")?
            .into_iter()
            .collect(),
        archived_category_ids: ids("SELECT id FROM categories WHERE archived=1 ORDER BY id")?
            .into_iter()
            .collect(),
    })
}

fn candidate_ids(tx: &Transaction<'_>, event: &str) -> Result<BTreeSet<String>> {
    tx.prepare("SELECT revision_id FROM candidates WHERE event_id=?1")
        .map_err(db_error)?
        .query_map([event], |row| row.get(0))
        .map_err(db_error)?
        .collect::<std::result::Result<_, _>>()
        .map_err(db_error)
}

fn validate_references(tx: &Transaction<'_>, event: &Event) -> Result<()> {
    event.validate()?;
    for account in [
        &event.account_id,
        &event.from_account_id,
        &event.to_account_id,
    ]
    .into_iter()
    .flatten()
    {
        let archived: Option<bool> = tx
            .query_row(
                "SELECT archived FROM accounts WHERE id=?1",
                [account],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        match archived {
            None => return Err(CoreError::new("missing_account", "资金账户不存在")),
            Some(true) => return Err(CoreError::new("archived_reference", "引用账户已经归档")),
            _ => {}
        }
    }
    if let Some(category) = &event.category_id {
        let row: Option<(String, bool)> = tx
            .query_row(
                "SELECT kind,archived FROM categories WHERE id=?1",
                [category],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(db_error)?;
        let (kind, archived) =
            row.ok_or_else(|| CoreError::new("missing_category", "分类不存在"))?;
        if archived {
            return Err(CoreError::new("archived_reference", "引用分类已经归档"));
        }
        let expected = if event.kind == EventKind::Income {
            "income"
        } else {
            "expense"
        };
        if kind != expected {
            return Err(CoreError::new("category_type_mismatch", "分类类型不匹配"));
        }
    }
    Ok(())
}

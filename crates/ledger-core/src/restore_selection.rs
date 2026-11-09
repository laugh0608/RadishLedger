//! E18 / E19 宿主候选。仅供合成账库实验，不接入应用 runtime。
use super::{Ticket, files as restore_files, materialization};
use crate::archive_container::CANDIDATE_LIMITS;
use crate::bootstrap_files::{self as files, FaultStage};
use crate::logical_snapshot::{Limits, cancelled};
use crate::store::snapshot_store::materialized;
use crate::store::{DbKey, Store};
use crate::{CoreError, Result, random_id};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use zeroize::Zeroizing;

#[path = "restore_materialization_tests.rs"]
mod restore_materialization_tests;
#[path = "restore_selection_tests.rs"]
mod tests;

const ACTIVE: &str = "active.json";
const PENDING: &str = "pending.json";
const LOCK: &str = "selection.lock";
const RECORD_LIMIT: usize = 4096;
const SNAPSHOT_LIMITS: Limits = Limits {
    rows: 100_000,
    bytes: 16 * 1024 * 1024,
};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Slot {
    Original,
    Candidate,
}

impl Slot {
    fn database(self) -> &'static str {
        match self {
            Self::Original => "original.db",
            Self::Candidate => "candidate.db",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct Target {
    slot: Slot,
    vault: String,
    device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    materialization: Option<Ticket>,
}

impl Target {
    fn validate(&self) -> Result<()> {
        for id in [&self.vault, &self.device] {
            files::identity(id).map_err(|_| mismatch())?;
        }
        if let Some(ticket) = &self.materialization {
            ticket.validate()?;
            if self.slot != Slot::Candidate || ticket.target_id() != self.vault {
                return Err(mismatch());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    version: u8,
    id: String,
    target: Target,
}

impl Selection {
    fn new(target: Target) -> Result<Self> {
        Ok(Self {
            version: 1,
            id: random_id()?,
            target,
        })
    }

    fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(mismatch());
        }
        files::identity(&self.id).map_err(|_| mismatch())?;
        self.target.validate()
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    old: Selection,
    new: Selection,
}

impl Pending {
    fn validate(&self) -> Result<()> {
        self.old.validate()?;
        self.new.validate()?;
        if self.old.id == self.new.id
            || self.old.target.slot != Slot::Original
            || self.new.target.slot != Slot::Candidate
            || self.old.target.vault == self.new.target.vault
            || self.old.target.device == self.new.target.device
        {
            return Err(mismatch());
        }
        Ok(())
    }
}

/// 确认在内存中一次性消费。进程重开只能查询；再切换须重新确认并生成身份。
struct Confirmed {
    pending: Pending,
    old_checkpoint: [u8; 32],
    new_checkpoint: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Point {
    BeforePending,
    Pending,
    Selected,
    Reopened,
    PendingRemoved,
    Settled,
}

#[derive(Default)]
struct Faults {
    pending: Option<FaultStage>,
    selection: Option<FaultStage>,
}

struct Host<'a> {
    root: &'a Path,
    original_key: &'a DbKey,
    candidate_key: Option<&'a DbKey>,
}

fn mismatch() -> CoreError {
    CoreError::new("selection_mismatch", "活动选择或账库绑定不一致，保留两库")
}

fn uncertain(error: CoreError) -> CoreError {
    let mut result = CoreError::new(
        "selection_uncertain",
        "活动发布结果未定，须重新核对后再允许写入",
    );
    result.os_code = error.os_code;
    result.sqlite_code = error.sqlite_code;
    result
}

fn io_error(error: std::io::Error) -> CoreError {
    let mut result = CoreError::new("selection_io", "活动选择文件操作失败，保留两库");
    result.os_code = error.raw_os_error();
    result
}

fn protected(path: &Path, directory: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    let valid_kind = if directory {
        metadata.is_dir()
    } else {
        metadata.is_file() && metadata.nlink() == 1
    };
    if !valid_kind || metadata.mode() & 0o077 != 0 {
        return Err(mismatch());
    }
    Ok(())
}

fn checkpoint(store: &Store, cancel: &AtomicBool) -> Result<[u8; 32]> {
    let snapshot = store.logical_snapshot(SNAPSHOT_LIMITS, &[], cancel)?;
    let bytes = Zeroizing::new(snapshot.encode(SNAPSHOT_LIMITS.bytes, cancel)?);
    Ok(Sha256::digest(bytes.as_slice()).into())
}

impl Host<'_> {
    fn lock(&self) -> Result<File> {
        if !self.root.is_absolute() {
            return Err(mismatch());
        }
        protected(self.root, true)?;
        let path = self.root.join(LOCK);
        protected(&path, false)?;
        let lock = File::open(path).map_err(io_error)?;
        files::exclusive_lock(&lock)?;
        Ok(lock)
    }

    fn read<T: DeserializeOwned>(&self, name: &str) -> Result<T> {
        let path = self.root.join(name);
        protected(&path, false)?;
        serde_json::from_slice(&files::read(&path, RECORD_LIMIT)?).map_err(|_| mismatch())
    }

    fn active(&self) -> Result<Selection> {
        let selection: Selection = self.read(ACTIVE)?;
        selection.validate()?;
        Ok(selection)
    }

    fn pending(&self) -> Result<Option<Pending>> {
        if !files::exists(&self.root.join(PENDING))? {
            return Ok(None);
        }
        let pending: Pending = self.read(PENDING)?;
        pending.validate()?;
        Ok(Some(pending))
    }

    fn settled(&self) -> Result<Selection> {
        if self.pending()?.is_some() {
            return Err(uncertain(mismatch()));
        }
        let active = self.active()?;
        // 宿主入口每次确认控制目录已同步，覆盖撤除 pending 后响应丢失。
        files::sync(self.root)?;
        Ok(active)
    }

    fn database(&self, target: &Target) -> PathBuf {
        match &target.materialization {
            Some(ticket) => materialization::directory(self.root, ticket).join("stage.db"),
            None => self.root.join(target.slot.database()),
        }
    }

    fn lock_target(&self, target: &Target) -> Result<Option<restore_files::Files>> {
        let Some(ticket) = &target.materialization else {
            return Ok(None);
        };
        let files = restore_files::Files::acquire(self.root, ticket, false)?;
        files.require_preparing()?;
        if !files.database_exists()? {
            return Err(materialized::mismatch());
        }
        Ok(Some(files))
    }

    fn key(&self, target: &Target) -> Result<&DbKey> {
        match target.slot {
            Slot::Original => Ok(self.original_key),
            Slot::Candidate => self.candidate_key.ok_or_else(|| {
                CoreError::new(
                    "selection_key_unavailable",
                    "新库钥暂不可用，保留原选择和两库",
                )
            }),
        }
    }

    fn read_store(&self, selection: &Selection) -> Result<Store> {
        let path = self.database(&selection.target);
        protected(&path, false)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(mismatch)?;
        for suffix in ["wal", "shm"] {
            let sidecar = path.with_file_name(format!("{name}-{suffix}"));
            if files::exists(&sidecar)? {
                protected(&sidecar, false)?;
            }
        }
        if let Some(ticket) = &selection.target.materialization {
            return materialized::open(
                &path,
                self.key(&selection.target)?,
                ticket,
                &selection.target.device,
                true,
            );
        }
        let store = Store::open_readonly(&path, self.key(&selection.target)?)?;
        store.verify_bootstrap(&selection.target.vault, &selection.target.device)?;
        Ok(store)
    }

    fn ready_checkpoint(&self, selection: &Selection, cancel: &AtomicBool) -> Result<[u8; 32]> {
        let store = self.read_store(selection)?;
        if selection.target.materialization.is_some() {
            let contents = materialized::inspect_ready(&store, CANDIDATE_LIMITS, cancel)?;
            return materialized::digest(&contents, CANDIDATE_LIMITS, cancel);
        }
        let checkpoint = checkpoint(&store, cancel)?;
        // 仅复用 bootstrap 合成库内的加密核对材料，控制文件不保存账务摘要。
        let expected = crate::crypto::hex(&checkpoint);
        if store.fixture_digest()?.as_deref() != Some(expected.as_str()) {
            return Err(mismatch());
        }
        Ok(checkpoint)
    }

    fn capture_restored(
        &self,
        origin: &Selection,
        months: &[String],
        limits: crate::archive_container::Limits,
        cancel: &AtomicBool,
        observe: impl FnMut(materialized::CapturePoint) -> Result<()>,
    ) -> Result<materialized::RestoredSnapshot> {
        cancelled(cancel)?;
        let _gate = self.lock()?;
        let active = self.settled()?;
        if &active != origin {
            return Err(CoreError::new(
                "selection_stale",
                "旧读取请求不属于当前活动选择",
            ));
        }
        if active.target.materialization.is_none() {
            return Err(mismatch());
        }
        let _target = self.lock_target(&active.target)?;
        let store = self.read_store(&active)?;
        materialized::capture(&store, months, limits, cancel, observe)
    }

    fn confirm(&self, target: Target, cancel: &AtomicBool) -> Result<Confirmed> {
        let _gate = self.lock()?;
        let old = self.settled()?;
        let pending = Pending {
            old,
            new: Selection::new(target)?,
        };
        pending.validate()?;
        let _target = self.lock_target(&pending.new.target)?;
        let old_checkpoint = checkpoint(&self.read_store(&pending.old)?, cancel)?;
        let new_checkpoint = self.ready_checkpoint(&pending.new, cancel)?;
        Ok(Confirmed {
            pending,
            old_checkpoint,
            new_checkpoint,
        })
    }

    fn write<T>(
        &self,
        origin: &Selection,
        operation: impl FnOnce(&mut Store) -> Result<T>,
    ) -> Result<(Selection, T)> {
        let _gate = self.lock()?;
        let active = self.settled()?;
        if &active != origin {
            return Err(CoreError::new(
                "selection_stale",
                "旧写入不属于当前活动选择",
            ));
        }
        let _target = self.lock_target(&active.target)?;
        self.read_store(&active)?;
        let path = self.database(&active.target);
        let key = self.key(&active.target)?;
        let mut store = if let Some(ticket) = &active.target.materialization {
            materialized::open(&path, key, ticket, &active.target.device, false)?
        } else {
            let store = Store::open(&path, key)?;
            store.verify_bootstrap(&active.target.vault, &active.target.device)?;
            store
        };
        let result = operation(&mut store)?;
        Ok((active, result))
    }

    fn accept_result<T>(&self, (origin, value): (Selection, T)) -> Result<Option<T>> {
        let _gate = self.lock()?;
        Ok((self.settled()? == origin).then_some(value))
    }

    fn persist<T: Serialize>(
        &self,
        name: &str,
        record: &T,
        replace: bool,
        fault: Option<FaultStage>,
    ) -> Result<()> {
        let bytes = serde_json::to_vec(record).map_err(|_| mismatch())?;
        if bytes.len() > RECORD_LIMIT {
            return Err(mismatch());
        }
        files::publish_with_fault(&self.root.join(name), &bytes, replace, fault)
    }

    fn publish(
        &self,
        confirmed: Confirmed,
        cancel: &AtomicBool,
        faults: Faults,
        mut observe: impl FnMut(Point) -> Result<()>,
    ) -> Result<Selection> {
        let _gate = self.lock()?;
        let Confirmed {
            pending,
            old_checkpoint,
            new_checkpoint,
        } = confirmed;
        let active = self.settled()?;
        if active != pending.old
            || checkpoint(&self.read_store(&active)?, cancel)? != old_checkpoint
        {
            return Err(CoreError::new(
                "selection_stale",
                "旧活动选择或账库已有变化，须重新确认",
            ));
        }
        let target = self.lock_target(&pending.new.target)?;
        if self.ready_checkpoint(&pending.new, cancel)? != new_checkpoint {
            return Err(mismatch());
        }
        if let Some(target) = &target {
            materialization::sync_database(&target.path)?;
        } else {
            let database = self.database(&pending.new.target);
            File::open(database)
                .and_then(|db| db.sync_all())
                .map_err(io_error)?;
            let wal = self.root.join("candidate.db-wal");
            if files::exists(&wal)? {
                protected(&wal, false)?;
                File::open(wal)
                    .and_then(|wal| wal.sync_all())
                    .map_err(io_error)?;
            }
        }
        files::sync(self.root)?;
        observe(Point::BeforePending)?;
        cancelled(cancel)?;
        self.persist(PENDING, &pending, false, faults.pending)
            .map_err(|error| {
                if error.code == "bootstrap_publish_uncertain" {
                    uncertain(error)
                } else {
                    error
                }
            })?;
        observe(Point::Pending).map_err(uncertain)?;
        if let Err(cancelled) = cancelled(cancel) {
            self.settle_pending(&pending, &mut observe)?;
            return Err(cancelled);
        }
        self.persist(ACTIVE, &pending.new, true, faults.selection)
            .map_err(uncertain)?;
        observe(Point::Selected).map_err(uncertain)?;
        self.settle_pending(&pending, &mut observe)
    }

    fn recover(&self) -> Result<Selection> {
        let _gate = self.lock()?;
        if let Some(pending) = self.pending()? {
            let active = self.active()?;
            if active != pending.old && active != pending.new {
                return Err(mismatch());
            }
            let _target = self.lock_target(&active.target)?;
            return self.settle_pending(&pending, &mut |_| Ok(()));
        }
        let active = self.settled()?;
        let _target = self.lock_target(&active.target)?;
        self.read_store(&active)?;
        Ok(active)
    }

    fn discard_materialized(&self, target: &Target) -> Result<()> {
        target.validate()?;
        let ticket = target.materialization.as_ref().ok_or_else(mismatch)?;
        let _gate = self.lock()?;
        let active = self.settled()?;
        if self.database(&active.target) == self.database(target) {
            return Err(CoreError::new("selection_in_use", "保留已选中的物化候选库"));
        }
        {
            let _active_target = self.lock_target(&active.target)?;
            self.read_store(&active)?;
        }
        if restore_files::already_absent(self.root, ticket)? {
            return Ok(());
        }
        let mut target_files = restore_files::Files::acquire(self.root, ticket, false)?;
        if !target_files.discarding() {
            if !target_files.database_exists()? {
                return Err(materialized::mismatch());
            }
            let path = target_files.path.join("stage.db");
            materialized::open(&path, self.key(target)?, ticket, &target.device, true)?;
        }
        target_files.permit_discard()?;
        target_files.discard(&mut |_| Ok(()))
    }

    fn settle_pending(
        &self,
        pending: &Pending,
        observe: &mut impl FnMut(Point) -> Result<()>,
    ) -> Result<Selection> {
        let active = self.active()?;
        let keep_checking = AtomicBool::new(false);
        if active == pending.old {
            checkpoint(&self.read_store(&active)?, &keep_checking)?;
        } else if active == pending.new {
            self.ready_checkpoint(&active, &keep_checking)?;
        } else {
            return Err(mismatch());
        }
        observe(Point::Reopened).map_err(uncertain)?;
        files::sync(self.root).map_err(uncertain)?;
        fs::remove_file(self.root.join(PENDING))
            .map_err(io_error)
            .map_err(uncertain)?;
        observe(Point::PendingRemoved).map_err(uncertain)?;
        files::sync(self.root).map_err(uncertain)?;
        observe(Point::Settled).map_err(uncertain)?;
        Ok(active)
    }
}

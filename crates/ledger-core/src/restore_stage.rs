//! E-RESTORE 宿主候选：独占的加密暂存，不提供活动发布或日常写入能力。
use crate::archive_container::{self, Limits, OpenedArchive};
use crate::logical_snapshot::Contents;
use crate::store::{
    DbKey,
    restore_store::{self, StageDb},
};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

#[path = "restore_files.rs"]
mod files;
#[cfg(all(test, unix))]
#[path = "restore_materialization.rs"]
mod materialization;
#[cfg(all(test, unix))]
#[path = "restore_selection.rs"]
mod restore_selection;
#[cfg(all(test, unix))]
#[path = "restore_roundtrip.rs"]
mod roundtrip;
#[cfg(test)]
#[path = "restore_tests.rs"]
mod tests;

/// 非秘密恢复定位材料；路径不来自归档，密钥不进入本结构或磁盘进度。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(all(test, unix), derive(Debug))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ticket {
    task_id: String,
    target_id: String,
    archive_id: [u8; 16],
    input_sha256: String,
}
impl Ticket {
    pub fn task_id(&self) -> &str {
        &self.task_id
    }
    pub fn target_id(&self) -> &str {
        &self.target_id
    }
    pub(crate) fn validate(&self) -> Result<()> {
        crate::store::valid_id(&self.task_id)?;
        crate::store::valid_id(&self.target_id)?;
        if self.input_sha256.len() != 64
            || !self
                .input_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(mismatch());
        }
        Ok(())
    }
}

pub struct Task {
    ticket: Ticket,
    key: DbKey,
    allow_create: Mutex<bool>,
}

/// 每次从实际暂存记录重建的结果；并非 Store、运行权限或可发布选择。
pub struct VerifiedStage {
    ticket: Ticket,
    contents: Contents,
}
impl VerifiedStage {
    pub fn ticket(&self) -> &Ticket {
        &self.ticket
    }
    pub fn contents(&self) -> &Contents {
        &self.contents
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Point {
    Owned,
    Bound,
    Schema,
    References,
    Revisions,
    Selections,
    History,
    BeforeCommit,
    Written,
    Verified,
    Marked,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CleanupPoint {
    Permitted,
    PayloadRemoved(&'static str),
    PayloadSynced,
    LegacyPermitRemoved,
    OwnerRemoved,
    LockRemoved,
    DirectoryRemoved,
}

pub(crate) fn mismatch() -> CoreError {
    CoreError::new(
        "restore_mismatch",
        "恢复输入、归属或暂存内容不一致，停止并保留现场",
    )
}
pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(CoreError::new(
            "cancelled",
            "恢复准备已取消，原库保留；暂存须重新核对",
        ))
    } else {
        Ok(())
    }
}

impl Task {
    /// 在任何暂存 IO 前取得独立新 DB Key；失败 / 取消时调用方仍持有本任务。
    pub fn new(input: &[u8]) -> Result<Self> {
        if input.len() < 329 || input.len() > archive_container::MAX_PAYLOAD + 328 {
            return Err(CoreError::new("invalid_archive", "恢复输入长度无效"));
        }
        let archive_id = input[24..40].try_into().map_err(|_| mismatch())?;
        Ok(Self {
            ticket: Ticket {
                task_id: crate::random_id()?,
                target_id: crate::random_id()?,
                archive_id,
                input_sha256: crate::crypto::hex(&Sha256::digest(input)),
            },
            key: DbKey::random()?,
            allow_create: Mutex::new(true),
        })
    }

    /// 仅重开已有目标；本宿主切片不负责保存 / 封装恢复钥。
    pub fn resume(ticket: Ticket, key: DbKey) -> Result<Self> {
        ticket.validate()?;
        Ok(Self {
            ticket,
            key,
            allow_create: Mutex::new(false),
        })
    }
    pub fn ticket(&self) -> &Ticket {
        &self.ticket
    }
    pub fn into_parts(self) -> (Ticket, DbKey) {
        (self.ticket, self.key)
    }

    fn authenticate(
        &self,
        input: &[u8],
        password: &[u8],
        limits: Limits,
        cancel: &AtomicBool,
    ) -> Result<OpenedArchive> {
        self.check_input(input, cancel)?;
        let opened = archive_container::open(input, password, limits, cancel)?;
        if *opened.archive_id() != self.ticket.archive_id {
            return Err(mismatch());
        }
        restore_store::check_payload(opened.contents(), limits, cancel)?;
        Ok(opened)
    }

    fn check_input(&self, input: &[u8], cancel: &AtomicBool) -> Result<()> {
        self.ticket.validate()?;
        check_cancel(cancel)?;
        if input.len() > archive_container::MAX_PAYLOAD + 328
            || crate::crypto::hex(&Sha256::digest(input)) != self.ticket.input_sha256
        {
            return Err(mismatch());
        }
        Ok(())
    }

    fn claim(&self, parent: &Path) -> Result<files::Files> {
        let mut allow_create = self.allow_create.lock().map_err(|_| mismatch())?;
        let files = files::Files::acquire(parent, &self.ticket, *allow_create)?;
        *allow_create = false;
        Ok(files)
    }

    pub fn prepare(
        &self,
        parent: &Path,
        input: &[u8],
        password: &[u8],
        limits: Limits,
        cancel: &AtomicBool,
    ) -> Result<VerifiedStage> {
        self.prepare_observed(parent, input, password, limits, cancel, |_| Ok(()))
    }

    fn prepare_observed(
        &self,
        parent: &Path,
        input: &[u8],
        password: &[u8],
        limits: Limits,
        cancel: &AtomicBool,
        mut observe: impl FnMut(Point) -> Result<()>,
    ) -> Result<VerifiedStage> {
        let opened = self.authenticate(input, password, limits, cancel)?;
        // 全部认证 / 业务校验成功后才创建或修改本任务的暂存目录。
        let mut files = self.claim(parent)?;
        files.require_preparing()?;
        observe(Point::Owned)?;
        check_cancel(cancel)?;
        if !files.database_exists()? {
            files.create_database()?;
        }
        let database = files.path.join("stage.db");
        let mut db = StageDb::prepare(
            &database,
            &self.key,
            &self.ticket,
            opened.contents(),
            files.can_initialize(),
            cancel,
        )?;
        observe(Point::Bound)?;
        check_cancel(cancel)?;
        // 事实写入前先持久登记数据库已建立，缺库时才能拒绝重建。
        files.mark_initialized()?;
        observe(Point::Schema)?;
        check_cancel(cancel)?;
        db.write(opened.contents(), cancel, &mut observe)?;
        // 关闭写连接，再从实际加密目标重开，不以写端内存对象代替落盘核对。
        drop(db);
        let db = StageDb::open(&database, &self.key, &self.ticket)?;
        let contents = db.verify(opened.contents(), limits, cancel)?;
        observe(Point::Verified)?;
        check_cancel(cancel)?;
        db.mark_verified()?;
        observe(Point::Marked)?;
        check_cancel(cancel)?;
        crate::bootstrap_files::sync(&files.path).map_err(files::file_error)?;
        Ok(VerifiedStage {
            ticket: self.ticket.clone(),
            contents,
        })
    }

    /// 每次重新认证原输入并核对完整目标；不信任 persisted verified 标志。
    pub fn inspect(
        &self,
        parent: &Path,
        input: &[u8],
        password: &[u8],
        limits: Limits,
        cancel: &AtomicBool,
    ) -> Result<VerifiedStage> {
        self.inspect_locked(parent, input, password, limits, cancel, Ok)
    }

    fn inspect_locked<T>(
        &self,
        parent: &Path,
        input: &[u8],
        password: &[u8],
        limits: Limits,
        cancel: &AtomicBool,
        checked: impl FnOnce(VerifiedStage) -> Result<T>,
    ) -> Result<T> {
        let opened = self.authenticate(input, password, limits, cancel)?;
        let files = files::Files::acquire(parent, &self.ticket, false)?;
        files.require_preparing()?;
        files.database_exists()?;
        let db = StageDb::open_readonly(&files.path.join("stage.db"), &self.key, &self.ticket)?;
        let contents = db.verify(opened.contents(), limits, cancel)?;
        check_cancel(cancel)?;
        checked(VerifiedStage {
            ticket: self.ticket.clone(),
            contents,
        })
    }

    /// 只删除本任务已核对的固定文件；重试须仍有归属或目录已不存在。
    pub fn abandon(&self, parent: &Path) -> Result<()> {
        self.abandon_observed(parent, |_| Ok(()))
    }

    fn abandon_observed(
        &self,
        parent: &Path,
        observe: impl FnMut(CleanupPoint) -> Result<()>,
    ) -> Result<()> {
        self.abandon_checked(parent, observe, |path, can_initialize| {
            StageDb::check_abandon(path, &self.key, &self.ticket, can_initialize)
        })
    }

    fn abandon_checked(
        &self,
        parent: &Path,
        mut observe: impl FnMut(CleanupPoint) -> Result<()>,
        check: impl FnOnce(&Path, bool) -> Result<()>,
    ) -> Result<()> {
        *self.allow_create.lock().map_err(|_| mismatch())? = false;
        if files::already_absent(parent, &self.ticket)? {
            return Ok(());
        }
        let mut files = files::Files::acquire(parent, &self.ticket, false)?;
        if !files.discarding() && files.database_exists()? {
            check(&files.path.join("stage.db"), files.can_initialize())?;
        }
        files.permit_discard()?;
        observe(CleanupPoint::Permitted)?;
        files.discard(&mut observe)
    }
}

//! 0.2 恢复复用任务归属、独占目录、持久阶段和清理规则。
use super::*;
use crate::archive_container::restored::{self, Opened, RestoredContents};
use crate::store::restore_store::roundtrip::RestoredStageDb;

impl Task {
    fn authenticate_restored(
        &self,
        input: &[u8],
        password: &[u8],
        limits: Limits,
        cancel: &AtomicBool,
    ) -> Result<RestoredContents> {
        self.check_input(input, cancel)?;
        match restored::open(input, password, limits, cancel)? {
            Opened::Restored(opened) if opened.id == self.ticket.archive_id => Ok(opened.contents),
            _ => Err(mismatch()),
        }
    }

    pub(super) fn prepare_restored(
        &self,
        parent: &Path,
        input: &[u8],
        password: &[u8],
        limits: Limits,
        cancel: &AtomicBool,
        mut observe: impl FnMut(Point) -> Result<()>,
    ) -> Result<()> {
        let contents = self.authenticate_restored(input, password, limits, cancel)?;
        let payload = contents.encode(limits, cancel)?;
        drop(contents);
        let digest = Sha256::digest(&payload).into();
        let mut files = self.claim(parent)?;
        files.require_preparing()?;
        observe(Point::Owned)?;
        check_cancel(cancel)?;
        if !files.database_exists()? {
            files.create_database()?;
        }
        let path = files.path.join("stage.db");
        let mut db = RestoredStageDb::prepare(
            &path,
            &self.key,
            &self.ticket,
            files.can_initialize(),
            cancel,
        )?;
        files.mark_initialized()?;
        observe(Point::Schema)?;
        db.write(&payload, cancel, || observe(Point::BeforeCommit))?;
        drop(db);
        drop(payload);
        observe(Point::Written)?;
        check_cancel(cancel)?;
        RestoredStageDb::open(&path, &self.key, &self.ticket)?.verify(&digest, limits, cancel)?;
        observe(Point::Verified)?;
        check_cancel(cancel)?;
        materialization::sync_database(&files.path)
    }

    pub(super) fn inspect_restored_locked<T>(
        &self,
        parent: &Path,
        input: &[u8],
        password: &[u8],
        limits: Limits,
        cancel: &AtomicBool,
        checked: impl FnOnce(RestoredContents, [u8; 32]) -> Result<T>,
    ) -> Result<T> {
        let contents = self.authenticate_restored(input, password, limits, cancel)?;
        let digest = Sha256::digest(contents.encode(limits, cancel)?).into();
        drop(contents);
        let files = files::Files::acquire(parent, &self.ticket, false)?;
        files.require_preparing()?;
        if !files.database_exists()? {
            return Err(mismatch());
        }
        let db = RestoredStageDb::open(&files.path.join("stage.db"), &self.key, &self.ticket)?;
        let contents = db.verify(&digest, limits, cancel)?;
        check_cancel(cancel)?;
        checked(contents, digest)
    }

    pub(super) fn abandon_restored(&self, parent: &Path) -> Result<()> {
        self.abandon_checked(
            parent,
            |_| Ok(()),
            |path, can_initialize| {
                RestoredStageDb::check_abandon(path, &self.key, &self.ticket, can_initialize)
            },
        )
    }
}

//! 已核对暂存到独立 SQLCipher 候选，始终在宿主试验中运行。
use super::*;
use crate::archive_container::restored::RestoredContents;
use crate::bootstrap_files as io;
use crate::store::snapshot_store::materialized;
use std::fs::File;
use std::path::PathBuf;

pub(super) struct StageInput<'a> {
    pub task: &'a Task,
    pub parent: &'a Path,
    pub archive: &'a [u8],
    pub password: &'a [u8],
}

pub(super) struct Candidate {
    pub ticket: Ticket,
    pub device: String,
    pub key: DbKey,
    allow_create: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Point {
    Registered,
    BeforeCommit,
    Written,
    Reopened,
}

fn fingerprint(ticket: &Ticket) -> Result<String> {
    let bytes = serde_json::to_vec(ticket).map_err(|_| materialized::mismatch())?;
    Ok(crate::crypto::hex(&Sha256::digest(bytes)))
}

pub(super) fn directory(parent: &Path, ticket: &Ticket) -> PathBuf {
    parent.join(format!("restore-{}", ticket.task_id()))
}

pub(super) fn sync_database(path: &Path) -> Result<()> {
    for name in ["stage.db", "stage.db-wal"] {
        let file = path.join(name);
        if name == "stage.db" || io::exists(&file).map_err(files::file_error)? {
            File::open(file)
                .and_then(|f| f.sync_all())
                .map_err(io::io_error)
                .map_err(files::file_error)?;
        }
    }
    io::sync(path).map_err(files::file_error)
}

impl Candidate {
    pub fn new(stage: &Task) -> Result<Self> {
        let ticket = Ticket {
            task_id: crate::random_id()?,
            target_id: crate::random_id()?,
            archive_id: stage.ticket.archive_id,
            input_sha256: fingerprint(stage.ticket())?,
        };
        Ok(Self {
            ticket,
            device: crate::random_id()?,
            key: DbKey::random()?,
            allow_create: true,
        })
    }

    pub fn resume(ticket: Ticket, device: String, key: DbKey) -> Result<Self> {
        ticket.validate()?;
        io::identity(&device)?;
        Ok(Self {
            ticket,
            device,
            key,
            allow_create: false,
        })
    }

    pub fn prepare(
        &mut self,
        input: &StageInput<'_>,
        parent: &Path,
        limits: Limits,
        cancel: &AtomicBool,
        mut observe: impl FnMut(Point) -> Result<()>,
    ) -> Result<Contents> {
        self.check_stage(input.task.ticket())?;
        // inspect_locked 在整个闭包期间持有暂存锁，先暂存后目标。
        input.task.inspect_locked(
            input.parent,
            input.archive,
            input.password,
            limits,
            cancel,
            |verified| {
                let mut target = self.claim_target(parent)?;
                let path = target.path.join("stage.db");
                let mut store = materialized::prepare(
                    &path,
                    &self.key,
                    &self.ticket,
                    &self.device,
                    verified.ticket(),
                    target.can_initialize(),
                )?;
                target.mark_initialized()?;
                observe(Point::Registered)?;
                check_cancel(cancel)?;
                if materialized::write(&mut store, verified.contents(), limits, cancel, || {
                    observe(Point::BeforeCommit)
                })? {
                    observe(Point::Written)?;
                }
                check_cancel(cancel)?;
                drop(store);
                let store = materialized::open(&path, &self.key, &self.ticket, &self.device, true)?;
                let contents = materialized::inspect_ready(&store, limits, cancel)?;
                if materialized::digest(&contents, limits, cancel)?
                    != materialized::digest(verified.contents(), limits, cancel)?
                {
                    return Err(materialized::mismatch());
                }
                drop(store);
                observe(Point::Reopened)?;
                check_cancel(cancel)?;
                sync_database(&target.path)?;
                Ok(contents)
            },
        )
    }

    pub fn prepare_restored(
        &mut self,
        input: &StageInput<'_>,
        parent: &Path,
        limits: Limits,
        cancel: &AtomicBool,
        mut observe: impl FnMut(Point) -> Result<()>,
    ) -> Result<RestoredContents> {
        self.check_stage(input.task.ticket())?;
        input.task.inspect_restored_locked(
            input.parent,
            input.archive,
            input.password,
            limits,
            cancel,
            |contents, expected| {
                let mut target = self.claim_target(parent)?;
                let path = target.path.join("stage.db");
                let mut store = materialized::prepare_restored(
                    &path,
                    &self.key,
                    &self.ticket,
                    &self.device,
                    input.task.ticket(),
                    target.can_initialize(),
                )?;
                target.mark_initialized()?;
                observe(Point::Registered)?;
                check_cancel(cancel)?;
                if materialized::write_restored(&mut store, &contents, limits, cancel, || {
                    observe(Point::BeforeCommit)
                })? {
                    observe(Point::Written)?;
                }
                check_cancel(cancel)?;
                drop(store);
                drop(contents);
                let store = materialized::open(&path, &self.key, &self.ticket, &self.device, true)?;
                let (contents, actual) =
                    materialized::inspect_restored_ready(&store, limits, cancel)?;
                if expected != actual {
                    return Err(materialized::mismatch());
                }
                drop(store);
                observe(Point::Reopened)?;
                check_cancel(cancel)?;
                sync_database(&target.path)?;
                Ok(contents)
            },
        )
    }

    fn check_stage(&self, stage: &Ticket) -> Result<()> {
        if self.ticket.input_sha256 != fingerprint(stage)?
            || self.ticket.archive_id != stage.archive_id
        {
            return Err(materialized::mismatch());
        }
        Ok(())
    }

    fn claim_target(&mut self, parent: &Path) -> Result<files::Files> {
        let target = files::Files::acquire(parent, &self.ticket, self.allow_create)?;
        self.allow_create = false;
        target.require_preparing()?;
        if !target.database_exists()? {
            target.create_database()?;
        }
        Ok(target)
    }
}

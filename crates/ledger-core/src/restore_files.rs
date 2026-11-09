//! 固定路径与精确归属；不从归档读取文件名，不递归清理。
use super::*;
use crate::bootstrap_files as io;
use std::fs::{self, File, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::PathBuf;

const MEMBERS: [&str; 6] = [
    "owner",
    "lock",
    "stage.db",
    "stage.db-wal",
    "stage.db-shm",
    "discard",
];
const DATABASE_FILES: [&str; 3] = ["stage.db-wal", "stage.db-shm", "stage.db"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Legacy,
    Pending,
    Initialized,
    Discarding,
}

impl Phase {
    fn prefix(self) -> &'static [u8] {
        match self {
            Self::Legacy => b"RadishLedger M0-E protected restore v1\n",
            Self::Pending => b"RadishLedger M0-E protected restore v2 pending\n",
            Self::Initialized => b"RadishLedger M0-E protected restore v2 initialized\n",
            Self::Discarding => b"RadishLedger M0-E protected restore v2 discarding\n",
        }
    }
}

pub(super) fn file_error(error: CoreError) -> CoreError {
    let mut result = CoreError::new(
        match error.code {
            "vault_busy" => "restore_busy",
            "bootstrap_publish_uncertain" => "restore_progress_uncertain",
            _ => "restore_io",
        },
        "恢复暂存文件操作失败，保留已登记目标供核对 / 重试",
    );
    result.os_code = error.os_code;
    result
}
fn os_error(error: std::io::Error) -> CoreError {
    let mut result = CoreError::new("restore_io", "恢复暂存文件操作失败，原库保留");
    result.os_code = error.raw_os_error();
    result
}
fn marker(ticket: &Ticket, phase: Phase) -> Result<Vec<u8>> {
    let mut bytes = phase.prefix().to_vec();
    serde_json::to_writer(&mut bytes, ticket).map_err(|_| mismatch())?;
    bytes.push(b'\n');
    Ok(bytes)
}
fn protected(path: &Path, directory: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(os_error)?;
    if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(mismatch());
    }
    #[cfg(unix)]
    {
        if metadata.mode() & 0o077 != 0 || (!directory && metadata.nlink() != 1) {
            return Err(mismatch());
        }
    }
    Ok(())
}

pub(super) struct Files {
    pub path: PathBuf,
    parent: PathBuf,
    ticket: Ticket,
    phase: Phase,
    legacy_discard: bool,
    _lock: File,
}
impl Files {
    pub fn acquire(parent: &Path, ticket: &Ticket, allow_create: bool) -> Result<Self> {
        ticket.validate()?;
        if !parent.is_absolute() || !parent.is_dir() || parent.as_os_str().len() > 3072 {
            return Err(CoreError::new(
                "invalid_directory",
                "恢复父目录必须是存在的绝对目录",
            ));
        }
        let parent = parent.canonicalize().map_err(os_error)?;
        let path = parent.join(format!("restore-{}", ticket.task_id));
        let created = match fs::symlink_metadata(&path) {
            Ok(_) => false,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_create => {
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                builder.mode(0o700);
                builder.create(&path).map_err(os_error)?;
                true
            }
            Err(error) => return Err(os_error(error)),
        };
        protected(&path, true)?;
        let owner = path.join("owner");
        let lock_path = path.join("lock");
        if !created {
            if !io::exists(&owner).map_err(file_error)? {
                return Err(CoreError::new(
                    "restore_owner_missing",
                    "恢复暂存缺少归属凭据，停止并保留目录",
                ));
            }
            protected(&owner, false)?;
            protected(&lock_path, false)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(created);
        #[cfg(unix)]
        options.mode(0o600);
        let lock = options.open(&lock_path).map_err(os_error)?;
        io::exclusive_lock(&lock).map_err(file_error)?;
        if created {
            io::sync(&parent).map_err(file_error)?;
            io::publish(&owner, &marker(ticket, Phase::Pending)?, false).map_err(file_error)?;
        }
        Self::check_members(&path)?;
        let bytes = io::read(&owner, 4096).map_err(file_error)?;
        let legacy_marker = marker(ticket, Phase::Legacy)?;
        let expected = &legacy_marker[Phase::Legacy.prefix().len()..];
        let phase = [
            Phase::Pending,
            Phase::Initialized,
            Phase::Discarding,
            Phase::Legacy,
        ]
        .into_iter()
        .find(|phase| bytes.strip_prefix(phase.prefix()) == Some(expected))
        .ok_or_else(mismatch)?;
        let discard_path = path.join("discard");
        let legacy_discard = io::exists(&discard_path).map_err(file_error)?;
        if legacy_discard
            && (!matches!(phase, Phase::Legacy | Phase::Discarding)
                || io::read(&discard_path, 4096).map_err(file_error)? != legacy_marker)
        {
            return Err(mismatch());
        }
        Ok(Self {
            path,
            parent,
            ticket: ticket.clone(),
            phase,
            legacy_discard,
            _lock: lock,
        })
    }
    fn check_members(path: &Path) -> Result<()> {
        for entry in fs::read_dir(path).map_err(os_error)? {
            let entry = entry.map_err(os_error)?;
            let name = entry.file_name();
            if !MEMBERS.iter().any(|n| name == *n) {
                return Err(mismatch());
            }
            protected(&entry.path(), false)?;
        }
        Ok(())
    }
    pub fn discarding(&self) -> bool {
        self.phase == Phase::Discarding || self.legacy_discard
    }
    pub fn require_preparing(&self) -> Result<()> {
        if self.discarding() {
            return Err(CoreError::new(
                "restore_discarding",
                "暂存正在放弃，只能继续原任务清理",
            ));
        }
        Ok(())
    }

    pub fn can_initialize(&self) -> bool {
        self.phase == Phase::Pending
    }

    pub fn database_exists(&self) -> Result<bool> {
        match fs::symlink_metadata(self.path.join("stage.db")) {
            Ok(metadata) => {
                if !metadata.is_file() || (!self.can_initialize() && metadata.len() == 0) {
                    return Err(mismatch());
                }
                return Ok(true);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(os_error(error)),
        }
        if !self.can_initialize() {
            return Err(CoreError::new(
                "restore_stage_missing",
                "恢复暂存数据库缺失，停止并保留原任务现场",
            ));
        }
        for name in ["stage.db-wal", "stage.db-shm"] {
            if io::exists(&self.path.join(name)).map_err(file_error)? {
                return Err(mismatch());
            }
        }
        Ok(false)
    }

    pub fn create_database(&self) -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        options.open(self.path.join("stage.db")).map_err(os_error)?;
        Ok(())
    }

    fn persist_phase(&mut self, phase: Phase) -> Result<()> {
        if self.phase == phase {
            // 前一次发布可能在替换后、目录同步前返回结果未定。
            return io::sync(&self.path).map_err(file_error);
        }
        io::publish(
            &self.path.join("owner"),
            &marker(&self.ticket, phase)?,
            true,
        )
        .map_err(file_error)?;
        self.phase = phase;
        Ok(())
    }

    pub fn mark_initialized(&mut self) -> Result<()> {
        self.persist_phase(Phase::Initialized)
    }

    pub fn permit_discard(&mut self) -> Result<()> {
        self.persist_phase(Phase::Discarding)
    }

    pub fn discard(self, observe: &mut impl FnMut(CleanupPoint) -> Result<()>) -> Result<()> {
        Self::check_members(&self.path)?;
        for name in DATABASE_FILES {
            if io::exists(&self.path.join(name)).map_err(file_error)? {
                fs::remove_file(self.path.join(name)).map_err(os_error)?;
                observe(CleanupPoint::PayloadRemoved(name))?;
            }
        }
        io::sync(&self.path).map_err(file_error)?;
        observe(CleanupPoint::PayloadSynced)?;
        if self.legacy_discard {
            fs::remove_file(self.path.join("discard")).map_err(os_error)?;
            observe(CleanupPoint::LegacyPermitRemoved)?;
        }
        // owner 保留到载荷和旧许可均已删除。先撤除归属，再释放锁路径。
        fs::remove_file(self.path.join("owner")).map_err(os_error)?;
        observe(CleanupPoint::OwnerRemoved)?;
        fs::remove_file(self.path.join("lock")).map_err(os_error)?;
        observe(CleanupPoint::LockRemoved)?;
        fs::remove_dir(&self.path).map_err(os_error)?;
        observe(CleanupPoint::DirectoryRemoved)?;
        io::sync(&self.parent).map_err(file_error)
    }
}

pub(super) fn already_absent(parent: &Path, ticket: &Ticket) -> Result<bool> {
    ticket.validate()?;
    if !parent.is_absolute() || !parent.is_dir() {
        return Err(CoreError::new("invalid_directory", "恢复父目录无效"));
    }
    let parent = parent.canonicalize().map_err(os_error)?;
    match fs::symlink_metadata(parent.join(format!("restore-{}", ticket.task_id))) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            io::sync(&parent).map_err(file_error)?;
            Ok(true)
        }
        Err(e) => Err(os_error(e)),
        Ok(_) => Ok(false),
    }
}

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
const PREFIX: &[u8] = b"RadishLedger M0-E protected restore v1\n";

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
fn marker(ticket: &Ticket) -> Result<Vec<u8>> {
    let mut bytes = PREFIX.to_vec();
    bytes.extend_from_slice(&serde_json::to_vec(ticket).map_err(|_| mismatch())?);
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
        match fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_create => {
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                builder.mode(0o700);
                builder.create(&path).map_err(os_error)?;
                io::sync(&parent).map_err(file_error)?;
                io::publish(&path.join("owner"), &marker(ticket)?, false).map_err(file_error)?;
            }
            Err(error) => return Err(os_error(error)),
        }
        protected(&path, true)?;
        protected(&path.join("owner"), false)?;
        if io::read(&path.join("owner"), 4096).map_err(file_error)? != marker(ticket)? {
            return Err(mismatch());
        }
        Self::check_members(&path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        options.mode(0o600);
        let lock = options.open(path.join("lock")).map_err(os_error)?;
        protected(&path.join("lock"), false)?;
        io::exclusive_lock(&lock).map_err(file_error)?;
        Self::check_members(&path)?;
        Ok(Self {
            path,
            parent,
            ticket: ticket.clone(),
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
    pub fn discarding(&self) -> Result<bool> {
        if !io::exists(&self.path.join("discard")).map_err(file_error)? {
            return Ok(false);
        }
        if io::read(&self.path.join("discard"), 4096).map_err(file_error)? != marker(&self.ticket)?
        {
            return Err(mismatch());
        }
        Ok(true)
    }
    pub fn require_preparing(&self) -> Result<()> {
        if self.discarding()? {
            return Err(CoreError::new(
                "restore_discarding",
                "暂存正在放弃，只能继续原任务清理",
            ));
        }
        Ok(())
    }
    pub fn permit_discard(&self) -> Result<()> {
        io::publish(&self.path.join("discard"), &marker(&self.ticket)?, false).map_err(file_error)
    }
    pub fn discard(self, after_remove: &mut impl FnMut(&str) -> Result<()>) -> Result<()> {
        Self::check_members(&self.path)?;
        for name in ["stage.db-wal", "stage.db-shm", "stage.db"] {
            if io::exists(&self.path.join(name)).map_err(file_error)? {
                fs::remove_file(self.path.join(name)).map_err(os_error)?;
                after_remove(name)?;
            }
        }
        io::sync(&self.path).map_err(file_error)?;
        // 删除数据完成后才移除定位材料；最后目录操作失败仍保留真实失败。
        fs::remove_file(self.path.join("discard")).map_err(os_error)?;
        fs::remove_file(self.path.join("lock")).map_err(os_error)?;
        fs::remove_file(self.path.join("owner")).map_err(os_error)?;
        fs::remove_dir(&self.path).map_err(os_error)?;
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

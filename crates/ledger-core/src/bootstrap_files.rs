//! 仅管理单个本次 M0-C 合成账库；普通文件有界读取，发布不覆盖既有身份。
use crate::{CoreError, Result};
use serde::Deserialize;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

const OWNER: &str = "RadishLedger M0-C synthetic vault v1\n";
pub(crate) const MAX_BOOTSTRAP: usize = 16 * 1024;
pub(crate) struct Files {
    pub path: PathBuf,
    _lock: File,
    fault: Option<FileFault>,
}

/// 仅供隔离 M0 实验，每个请求显式携带；不使用环境变量或全局故障开关。
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileFault {
    pub file: FaultFile,
    pub stage: FaultStage,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FaultFile {
    Bootstrap,
    Ready,
    LocalProtection,
    ActiveProtection,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FaultStage {
    ShortWrite,
    WriteZero,
    WriteError,
    FileSync,
    Publish,
    DirectorySync,
}

impl FaultFile {
    fn matches(self, name: &str) -> bool {
        match self {
            Self::Bootstrap => name == "bootstrap.json",
            Self::Ready => name == "ready.json",
            Self::LocalProtection => name.starts_with("local-") && name.ends_with(".json"),
            Self::ActiveProtection => name == "active-protection.json",
        }
    }
}

fn reject(stage: Option<FaultStage>, at: FaultStage) -> std::io::Result<()> {
    if stage == Some(at) {
        Err(std::io::Error::other(
            "M0 synthetic publication I/O failure",
        ))
    } else {
        Ok(())
    }
}

struct PublicationWriter<'a> {
    file: &'a mut File,
    stage: Option<FaultStage>,
    written: usize,
}

impl Write for PublicationWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.written > 0 {
            if self.stage == Some(FaultStage::WriteZero) {
                return Ok(0);
            }
            reject(self.stage, FaultStage::WriteError)?;
        }
        // 正数短写应由同一 write_all 路径续写；零写 / IO 错误在真实部分写入后拒绝。
        let limit = if matches!(
            self.stage,
            Some(FaultStage::ShortWrite | FaultStage::WriteZero | FaultStage::WriteError)
        ) {
            bytes.len().min(7)
        } else {
            bytes.len()
        };
        let n = self.file.write(&bytes[..limit])?;
        self.written += n;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

pub(crate) fn io_error(error: std::io::Error) -> CoreError {
    let mut result = CoreError::new("bootstrap_io", "bootstrap 文件操作失败，原有材料保留");
    result.os_code = error.raw_os_error();
    result
}

pub(crate) fn identity(id: &str) -> Result<()> {
    if id.len() != 32
        || !id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(CoreError::new("invalid_identity", "实验身份无效"));
    }
    Ok(())
}

pub(crate) fn regular(path: &Path) -> Result<()> {
    if !fs::symlink_metadata(path)
        .map_err(io_error)?
        .file_type()
        .is_file()
    {
        return Err(CoreError::new("invalid_file", "实验材料必须是普通文件"));
    }
    Ok(())
}

pub(crate) fn read(path: &Path, limit: usize) -> Result<Vec<u8>> {
    regular(path)?;
    let file = File::open(path).map_err(io_error)?;
    if file.metadata().map_err(io_error)?.len() > limit as u64 {
        return Err(CoreError::new(
            "oversized_bootstrap",
            "实验材料超过允许长度",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > limit {
        return Err(CoreError::new(
            "oversized_bootstrap",
            "实验材料超过允许长度",
        ));
    }
    Ok(bytes)
}

pub(crate) fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            regular(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(error)),
    }
}

pub(crate) fn sync(directory: &Path) -> Result<()> {
    File::open(directory)
        .map_err(io_error)?
        .sync_all()
        .map_err(io_error)
}

fn exclusive_lock(file: &File) -> Result<()> {
    // Rust 1.96 的 std::fs::File::try_lock 未接入 Android；直接使用其 NDK flock。
    // 锁绑定打开的文件描述符，进程退出或 File drop 后由内核释放。
    #[cfg(target_os = "android")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: file 持有有效描述符，flock 不借用任何 Rust 内存。
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            Err(CoreError::new("vault_busy", "该实验账库正在被另一调用使用"))
        } else {
            Err(io_error(error))
        }
    }
    #[cfg(not(target_os = "android"))]
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => {
            CoreError::new("vault_busy", "该实验账库正在被另一调用使用")
        }
        std::fs::TryLockError::Error(error) => io_error(error),
    })
}

pub(crate) fn publish(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    publish_with_fault(path, bytes, replace, None)
}

fn publish_with_fault(
    path: &Path,
    bytes: &[u8],
    replace: bool,
    stage: Option<FaultStage>,
) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::new("invalid_directory", "实验目录无效"))?;
    if bytes.len() > MAX_BOOTSTRAP {
        return Err(CoreError::new(
            "oversized_bootstrap",
            "实验材料超过允许长度",
        ));
    }
    if exists(path)? && !replace {
        return Err(CoreError::new("already_exists", "已有材料不能被覆盖"));
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
    #[cfg(unix)]
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(io_error)?;
    PublicationWriter {
        file: temporary.as_file_mut(),
        stage,
        written: 0,
    }
    .write_all(bytes)
    .map_err(io_error)?;
    reject(stage, FaultStage::FileSync).map_err(io_error)?;
    temporary.as_file().sync_all().map_err(io_error)?;
    reject(stage, FaultStage::Publish).map_err(io_error)?;
    if replace {
        temporary
            .persist(path)
            .map_err(|error| io_error(error.error))?;
    } else {
        temporary
            .persist_noclobber(path)
            .map_err(|error| io_error(error.error))?;
    }
    // rename 已完成，不能把目录同步失败报告成“旧引用未变”，也不能回写旧文件。
    reject(stage, FaultStage::DirectorySync)
        .and_then(|()| File::open(parent)?.sync_all())
        .map_err(|error| {
            let mut result = CoreError::new(
                "bootstrap_publish_uncertain",
                "文件已发布但目录同步失败，持久化结果未定，请重新核对",
            );
            result.os_code = error.raw_os_error();
            result
        })
}

impl Files {
    pub fn acquire(directory: &Path, run: &str, create: bool) -> Result<Self> {
        identity(run)?;
        if !directory.is_absolute()
            || !directory.is_dir()
            || directory.to_string_lossy().len() > 3072
        {
            return Err(CoreError::new("invalid_directory", "实验私有目录不可用"));
        }
        let directory = directory.canonicalize().map_err(io_error)?;
        let path = directory.join(format!("m0c-{run}"));
        if create {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            builder.mode(0o700);
            builder.create(&path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    CoreError::new("already_exists", "本次账库身份已经存在，不能重建")
                } else {
                    io_error(error)
                }
            })?;
            sync(&directory)?;
            publish(
                &path.join("owner"),
                format!("{OWNER}{run}\n").as_bytes(),
                false,
            )?;
        }
        if !fs::symlink_metadata(&path)
            .map_err(io_error)?
            .file_type()
            .is_dir()
        {
            return Err(CoreError::new(
                "invalid_directory",
                "不能跟随外部实验目录链接",
            ));
        }
        if read(&path.join("owner"), 128)? != format!("{OWNER}{run}\n").as_bytes() {
            return Err(CoreError::new(
                "foreign_directory",
                "该目录不属于本次合成实验",
            ));
        }
        let lock_path = path.join("lock");
        if exists(&lock_path)? {
            regular(&lock_path)?;
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(io_error)?;
        #[cfg(unix)]
        lock.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(io_error)?;
        exclusive_lock(&lock)?;
        Ok(Self {
            path,
            _lock: lock,
            fault: None,
        })
    }
    pub fn with_fault(mut self, fault: Option<FileFault>) -> Self {
        self.fault = fault;
        self
    }
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        read(&self.path.join(name), MAX_BOOTSTRAP)
    }
    pub fn publish(&self, name: &str, bytes: &[u8], replace: bool) -> Result<()> {
        let stage = self
            .fault
            .filter(|fault| fault.file.matches(name))
            .map(|fault| fault.stage);
        publish_with_fault(&self.path.join(name), bytes, replace, stage)
    }
    pub fn exists(&self, name: &str) -> Result<bool> {
        exists(&self.path.join(name))
    }
    pub fn local_generations(&self) -> Result<Vec<String>> {
        let mut generations = Vec::new();
        for entry in fs::read_dir(&self.path).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| CoreError::new("invalid_file", "实验文件名无效"))?;
            if let Some(id) = name
                .strip_prefix("local-")
                .and_then(|name| name.strip_suffix(".json"))
            {
                identity(id)?;
                regular(&entry.path())?;
                generations.push(id.to_owned());
            }
        }
        generations.sort();
        Ok(generations)
    }
    pub fn cleanup(self) -> Result<()> {
        // 仅显式清理已核对 owner 的本次平面实验目录，不递归跟随任何链接。
        let paths = fs::read_dir(&self.path)
            .map_err(io_error)?
            .map(|entry| entry.map(|e| e.path()).map_err(io_error))
            .collect::<Result<Vec<_>>>()?;
        for path in &paths {
            regular(path)?;
        }
        fs::remove_file(self.path.join("owner")).map_err(io_error)?;
        for path in paths {
            if path.file_name() != Some(std::ffi::OsStr::new("owner")) {
                fs::remove_file(path).map_err(io_error)?;
            }
        }
        fs::remove_dir(&self.path).map_err(io_error)?;
        sync(
            self.path
                .parent()
                .ok_or_else(|| CoreError::new("invalid_directory", "实验目录无效"))?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_ownership_and_failed_publication_preserve_files() {
        let root = tempfile::tempdir().unwrap();
        let run = crate::random_id().unwrap();
        let files = Files::acquire(root.path(), &run, true).unwrap();
        assert_eq!(
            Files::acquire(root.path(), &run, false).err().unwrap().code,
            "vault_busy"
        );
        assert_eq!(
            Files::acquire(root.path(), &run, true).err().unwrap().code,
            "already_exists"
        );
        files
            .publish("test.json", b"original synthetic bytes", false)
            .unwrap();
        assert_eq!(
            files
                .publish("test.json", b"replacement", false)
                .unwrap_err()
                .code,
            "already_exists"
        );
        assert_eq!(
            files.read("test.json").unwrap(),
            b"original synthetic bytes"
        );
        std::fs::create_dir(files.path.join("blocked.json")).unwrap();
        assert_eq!(
            files
                .publish("blocked.json", b"new", true)
                .unwrap_err()
                .code,
            "invalid_file"
        );
        std::fs::remove_dir(files.path.join("blocked.json")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                files.path.join("test.json"),
                files.path.join("linked.json"),
            )
            .unwrap();
            assert_eq!(files.read("linked.json").unwrap_err().code, "invalid_file");
            std::fs::remove_file(files.path.join("linked.json")).unwrap();
        }
        drop(files);
        Files::acquire(root.path(), &run, false)
            .unwrap()
            .cleanup()
            .unwrap();
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

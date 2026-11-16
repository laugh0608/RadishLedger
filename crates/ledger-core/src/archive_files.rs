//! E15 宿主归档保存。调用方提供稳定、私有的合成目录和单个文件名。
use super::{Limits, OVERHEAD, SealedArchive, buffer, cancelled, invalid, limit};
use crate::{CoreError, Result};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read, Seek, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[cfg(test)]
#[path = "archive_file_tests.rs"]
mod tests;

const TEMP_PREFIX: &str = ".radishledger-archive-";
const CHUNK_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Point {
    PartialWrite,
    FileSync,
    ReadBack,
    Publish,
    Published,
    DirectorySync,
}

/// 归档保存和复核共用的宿主目录。调用方负责在操作期间保持目录路径稳定。
pub struct ArchiveFiles<'a> {
    parent: PathBuf,
    directory: File,
    password: &'a [u8],
    limits: Limits,
    cancel: &'a AtomicBool,
    #[cfg(test)]
    use_restored_reader: bool,
}

impl<'a> ArchiveFiles<'a> {
    pub fn open(
        parent: &Path,
        password: &'a [u8],
        limits: Limits,
        cancel: &'a AtomicBool,
    ) -> Result<Self> {
        limits.validate()?;
        cancelled(cancel)?;
        if !parent.is_absolute() {
            return Err(private_directory());
        }
        let parent = parent.canonicalize().map_err(io_error)?;
        check_directory(&fs::symlink_metadata(&parent).map_err(io_error)?)?;
        let directory = File::open(&parent).map_err(io_error)?;
        check_directory(&directory.metadata().map_err(io_error)?)?;
        Ok(Self {
            parent,
            directory,
            password,
            limits,
            cancel,
            #[cfg(test)]
            use_restored_reader: false,
        })
    }

    /// 和 0.2 的编解码、物化及活动选择使用同一宿主测试边界。
    #[cfg(test)]
    pub(crate) fn with_restored_reader(mut self) -> Self {
        self.use_restored_reader = true;
        self
    }

    /// 只有新文件及其目录项均已同步才成功。已有目标一律保留。
    pub fn save_new(&self, name: &OsStr, archive: &SealedArchive) -> Result<()> {
        self.save_observed(name, archive, |_, _| Ok(()))
    }

    fn save_observed(
        &self,
        name: &OsStr,
        archive: &SealedArchive,
        mut observe: impl FnMut(Point, &Path) -> io::Result<()>,
    ) -> Result<()> {
        cancelled(self.cancel)?;
        self.check_length(archive.as_bytes().len() as u64)?;
        let target = self.target(name)?;
        match fs::symlink_metadata(&target) {
            Ok(_) => return Err(already_exists()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
        let mut temporary = tempfile::Builder::new()
            .prefix(TEMP_PREFIX)
            .permissions(fs::Permissions::from_mode(0o600))
            .tempfile_in(&self.parent)
            .map_err(io_error)?;
        let published = self
            .write_verified(&mut temporary, archive, &mut observe)
            .and_then(|()| {
                // 链接是唯一发布点，即使有并发创建也不会覆盖目标。
                fs::hard_link(temporary.path(), &target).map_err(|error| {
                    if error.kind() == io::ErrorKind::AlreadyExists {
                        already_exists()
                    } else {
                        io_error(error)
                    }
                })
            });
        if let Err(error) = published {
            temporary.close().map_err(cleanup_failed)?;
            return Err(error);
        }

        // 发布后即使取消或同步失败也必须保全目标，留给调用方重新核对。
        let after_publish = observe(Point::Published, temporary.path()).map_err(uncertain);
        let cleanup = temporary.close().map_err(uncertain);
        let synced = observe(Point::DirectorySync, &target)
            .and_then(|()| self.directory.sync_all())
            .map_err(uncertain);
        after_publish.and(cleanup).and(synced)
    }

    fn write_verified(
        &self,
        temporary: &mut tempfile::NamedTempFile,
        archive: &SealedArchive,
        observe: &mut impl FnMut(Point, &Path) -> io::Result<()>,
    ) -> Result<()> {
        let bytes = archive.as_bytes();
        let (header, rest) = bytes.split_at(super::HEADER);
        temporary.write_all(header).map_err(io_error)?;
        observe(Point::PartialWrite, temporary.path()).map_err(io_error)?;
        for chunk in rest.chunks(CHUNK_BYTES) {
            cancelled(self.cancel)?;
            temporary.write_all(chunk).map_err(io_error)?;
        }
        observe(Point::FileSync, temporary.path()).map_err(io_error)?;
        temporary.as_file().sync_all().map_err(io_error)?;
        observe(Point::ReadBack, temporary.path()).map_err(io_error)?;
        let file = temporary.as_file_mut();
        file.rewind().map_err(io_error)?;
        let written = self.read_bounded(file)?;
        self.authenticate(&written)?;
        if written.as_bytes() != bytes {
            return Err(mismatch());
        }
        observe(Point::Publish, temporary.path()).map_err(io_error)?;
        cancelled(self.cancel)
    }

    /// 从最终路径冷读并认证密文，结果可交给既有恢复任务。
    pub fn read_saved(&self, name: &OsStr) -> Result<SealedArchive> {
        let (mut file, _) = self.open_saved(name)?;
        let bytes = self.read_bounded(&mut file)?;
        self.authenticate(&bytes)?;
        Ok(bytes)
    }

    /// 对发布后未定的结果确认同一密文，并重新同步文件和目录。
    /// 调用方在保存前可用 `SealedArchive::ciphertext_sha256` 取得预期指纹。
    pub fn recheck_saved(&self, name: &OsStr, expected_sha256: &[u8; 32]) -> Result<()> {
        let (mut file, target) = self.open_saved(name)?;
        let bytes = self.read_bounded(&mut file)?;
        if &bytes.ciphertext_sha256() != expected_sha256 {
            return Err(mismatch());
        }
        self.authenticate(&bytes)?;
        if !same_file(
            &file.metadata().map_err(io_error)?,
            &fs::symlink_metadata(target).map_err(io_error)?,
        ) {
            return Err(mismatch());
        }
        cancelled(self.cancel)?;
        file.sync_all()
            .and_then(|()| self.directory.sync_all())
            .map_err(uncertain)
    }

    fn target(&self, name: &OsStr) -> Result<PathBuf> {
        if Path::new(name).file_name() != Some(name)
            || name.as_encoded_bytes().starts_with(TEMP_PREFIX.as_bytes())
        {
            return Err(CoreError::new(
                "invalid_archive_name",
                "归档目标必须是私有目录内的单个文件名",
            ));
        }
        let parent = fs::symlink_metadata(&self.parent).map_err(io_error)?;
        check_directory(&parent)?;
        if !same_file(&parent, &self.directory.metadata().map_err(io_error)?) {
            return Err(private_directory());
        }
        Ok(self.parent.join(name))
    }

    fn open_saved(&self, name: &OsStr) -> Result<(File, PathBuf)> {
        cancelled(self.cancel)?;
        let target = self.target(name)?;
        let entry = fs::symlink_metadata(&target).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                CoreError::new("archive_not_found", "未找到已发布的归档")
            } else {
                io_error(error)
            }
        })?;
        if !entry.is_file() {
            return Err(invalid());
        }
        let file = File::open(&target).map_err(io_error)?;
        let opened = file.metadata().map_err(io_error)?;
        if !opened.is_file() || !same_file(&entry, &opened) {
            return Err(mismatch());
        }
        Ok((file, target))
    }

    fn check_length(&self, length: u64) -> Result<usize> {
        if length <= OVERHEAD as u64 {
            return Err(invalid());
        }
        if length > (self.limits.payload_bytes + OVERHEAD) as u64 {
            return Err(limit());
        }
        Ok(length as usize)
    }

    fn read_bounded(&self, file: &mut File) -> Result<SealedArchive> {
        cancelled(self.cancel)?;
        let size = self.check_length(file.metadata().map_err(io_error)?.len())?;
        let mut bytes = buffer(size)?;
        bytes.resize(size, 0);
        for chunk in bytes.chunks_mut(CHUNK_BYTES) {
            cancelled(self.cancel)?;
            file.read_exact(chunk).map_err(io_error)?;
        }
        if file.read(&mut [0]).map_err(io_error)? != 0 {
            return Err(invalid());
        }
        Ok(SealedArchive(bytes))
    }

    fn authenticate(&self, input: &SealedArchive) -> Result<()> {
        #[cfg(test)]
        if self.use_restored_reader {
            return super::restored::open(
                input.as_bytes(),
                self.password,
                self.limits,
                self.cancel,
            )
            .map(drop);
        }
        super::open(input.as_bytes(), self.password, self.limits, self.cancel).map(drop)
    }
}

impl SealedArchive {
    /// 非秘密密文指纹，用于识别同一次保存。
    pub fn ciphertext_sha256(&self) -> [u8; 32] {
        Sha256::digest(self.as_bytes()).into()
    }
}

fn same_file(first: &fs::Metadata, second: &fs::Metadata) -> bool {
    first.dev() == second.dev() && first.ino() == second.ino()
}

fn check_directory(metadata: &fs::Metadata) -> Result<()> {
    if !metadata.is_dir() || metadata.mode() & 0o077 != 0 {
        return Err(private_directory());
    }
    Ok(())
}

fn private_directory() -> CoreError {
    CoreError::new("invalid_archive_directory", "归档目录必须是稳定的私有目录")
}

fn already_exists() -> CoreError {
    CoreError::new("archive_exists", "已有归档目标已保留")
}

fn mismatch() -> CoreError {
    CoreError::new("archive_file_mismatch", "文件与本次归档不一致，保留原文件")
}

fn io_error(error: io::Error) -> CoreError {
    with_os_code("archive_file_io", "归档文件操作失败，原有归档保留", error)
}

fn cleanup_failed(error: io::Error) -> CoreError {
    with_os_code(
        "archive_save_cleanup_failed",
        "归档未发布，临时文件清理失败",
        error,
    )
}

fn uncertain(error: io::Error) -> CoreError {
    with_os_code(
        "archive_save_uncertain",
        "归档已发布，保存结果未定，请重新核对同一文件",
        error,
    )
}

fn with_os_code(code: &'static str, message: &'static str, error: io::Error) -> CoreError {
    let mut result = CoreError::new(code, message);
    result.os_code = error.raw_os_error();
    result
}

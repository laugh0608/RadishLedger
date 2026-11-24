//! Android 内部归档交接。暂存中只出现密文，回读材料由下载提供方重新打开后写入。
use super::Reply;
use crate::archive_container::{self, CANDIDATE_LIMITS};
use crate::bootstrap_files::{self, io_error};
use crate::logical_snapshot;
use crate::restore_stage::{Task, Ticket, VerifiedStage};
use crate::store::DbKey;
use crate::{CoreError, Result, store::Store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn stage_path(directory: &Path, stage: &str) -> Result<PathBuf> {
    let invalid = || CoreError::new("invalid_archive_stage", "归档暂存目录无效");
    let suffix = stage.strip_prefix("archive-").ok_or_else(invalid)?;
    if !directory.is_absolute()
        || !(6..=32).contains(&suffix.len())
        || !suffix.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return Err(invalid());
    }
    let path = directory.join(stage);
    if !path.symlink_metadata().map_err(io_error)?.is_dir() {
        return Err(invalid());
    }
    Ok(path)
}

pub(super) fn credentials(input: &[u8], archive_bytes: usize) -> Result<(&[u8], &[u8])> {
    if !(1..=80).contains(&archive_bytes) || input.len() <= archive_bytes {
        return Err(CoreError::new(
            "invalid_archive_password",
            "归档口令需为 1–80 个 UTF-8 字节",
        ));
    }
    let (local, archive) = input.split_at(input.len() - archive_bytes);
    if local == archive || std::str::from_utf8(archive).is_err() {
        return Err(CoreError::new(
            "invalid_archive_password",
            "请使用独立的归档口令",
        ));
    }
    Ok((local, archive))
}

pub(super) fn prepare(
    store: Store,
    directory: &Path,
    stage: &str,
    password: &[u8],
) -> Result<[u8; 32]> {
    let path = stage_path(directory, stage)?.join("sealed");
    let cancel = AtomicBool::new(false);
    let snapshot = store.logical_snapshot(
        logical_snapshot::Limits {
            rows: logical_snapshot::MAX_ROWS,
            bytes: logical_snapshot::MAX_BYTES,
        },
        &[],
        &cancel,
    )?;
    drop(store);
    let sealed = archive_container::seal(&snapshot, password, CANDIDATE_LIMITS, &cancel)?;
    let mut file = File::create_new(path).map_err(io_error)?;
    file.write_all(sealed.as_bytes()).map_err(io_error)?;
    Ok(Sha256::digest(sealed.as_bytes()).into())
}

pub(super) fn verify(
    directory: &Path,
    stage: &str,
    expected: [u8; 32],
    password: &[u8],
) -> Result<()> {
    let path = stage_path(directory, stage)?.join("readback");
    let actual = bootstrap_files::read(&path, archive_container::MAX_SEALED_BYTES)?;
    if Sha256::digest(&actual)[..] != expected {
        return Err(CoreError::new(
            "archive_file_mismatch",
            "下载目录回读的文件与本次归档不一致",
        ));
    }
    archive_container::open(&actual, password, CANDIDATE_LIMITS, &AtomicBool::new(false))?;
    Ok(())
}

fn restored_report(stage: &VerifiedStage) -> Value {
    let contents = stage.contents();
    let facts = &contents.facts;
    let verification = &contents.verification;
    // 名称已按逻辑快照契约限制在 256 字节以内，只返回适合桥接回执的核对样本。
    let balances: Vec<_> = facts.accounts.iter().take(8).map(|account| {
        json!({"name":account.name, "minor":verification.projection.balances_minor[&account.id]})
    }).collect();
    json!({
        "balances":balances,
        "ticket":stage.ticket(),
        "sequence":facts.sequence,
        "accounts":facts.accounts.len(),
        "categories":facts.categories.len(),
        "activeEvents":verification.active_event_ids.len(),
        "revisions":facts.revisions.len(),
        "intents":facts.intent_history.len(),
        "deleted":verification.deleted_event_ids.len(),
        "candidates":verification.unresolved_candidate_revision_ids.len(),
        "netAssetsMinor":verification.projection.net_assets_minor,
        "assetsMinor":verification.projection.assets_minor,
        "liabilitiesMinor":verification.projection.liabilities_minor,
    })
}

fn restore_input(parent: &Path) -> Result<Vec<u8>> {
    bootstrap_files::read(
        &parent.join("source.rla"),
        archive_container::MAX_SEALED_BYTES,
    )
}

pub(super) fn prepare_restore(directory: &Path, stage: &str, password: &[u8]) -> Result<Reply> {
    let parent = stage_path(directory, stage)?;
    // 两个准备请求不能各自在同一选择目录里创建目标。
    let source = parent.join("source.rla");
    bootstrap_files::regular(&source)?;
    let lock = File::open(&source).map_err(io_error)?;
    bootstrap_files::exclusive_lock(&lock)?;
    for member in parent.read_dir().map_err(io_error)? {
        if member.map_err(io_error)?.file_name() != "source.rla" {
            return Err(CoreError::new(
                "restore_target_not_empty",
                "此恢复位置已有暂存或未知文件，保留原目标",
            ));
        }
    }
    let input = restore_input(&parent)?;
    let task = Task::new(&input)?;
    let verified = match task.prepare(
        &parent,
        &input,
        password,
        CANDIDATE_LIMITS,
        &AtomicBool::new(false),
    ) {
        Ok(verified) => verified,
        Err(error) => {
            task.abandon(&parent)?;
            return Err(error);
        }
    };
    let (_, key) = task.into_parts();
    Ok(Reply::secret(
        restored_report(&verified),
        key.into_secret().as_ref(),
    ))
}

pub(super) fn inspect_restore(
    directory: &Path,
    stage: &str,
    ticket: Ticket,
    input: &[u8],
) -> Result<Reply> {
    let (key, password) = input
        .split_at_checked(32)
        .ok_or_else(|| CoreError::new("invalid_argument", "恢复核对材料不完整"))?;
    let parent = stage_path(directory, stage)?;
    let task = Task::resume(ticket, DbKey::from_secret(key)?)?;
    let verified = task.inspect(
        &parent,
        &restore_input(&parent)?,
        password,
        CANDIDATE_LIMITS,
        &AtomicBool::new(false),
    )?;
    Ok(Reply::public(restored_report(&verified)))
}

pub(super) fn discard_restore(
    directory: &Path,
    stage: &str,
    ticket: Ticket,
    key: &[u8],
) -> Result<Reply> {
    let parent = stage_path(directory, stage)?;
    Task::resume(ticket, DbKey::from_secret(key)?)?.abandon(&parent)?;
    Ok(Reply::public(json!({"discarded":true})))
}

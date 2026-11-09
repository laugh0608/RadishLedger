//! E-SNAPSHOT 宿主候选：完整声明集合的只读内存快照，不是认证归档或恢复入口。
use crate::csv_import::{Decision, Mapping, TaskState};
use crate::csv_undo::State;
use crate::domain::Event;
use crate::store::{Snapshot as Verification, Write as Intent};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

pub const PROFILE: &str = "m0-logical-snapshot-v1";
pub const MAX_ROWS: usize = 100_000;
pub const MAX_BYTES: usize = 16 * 1024 * 1024;

/// 实验预算，调用方显式提供；不是生产容量或性能承诺。
#[derive(Clone, Copy)]
pub struct Limits {
    pub rows: usize,
    pub bytes: usize,
}

pub(crate) fn limit() -> CoreError {
    CoreError::new("snapshot_limit", "逻辑快照超过本次行数或字节预算")
}

pub(crate) fn cancelled(flag: &AtomicBool) -> Result<()> {
    if flag.load(Ordering::Relaxed) {
        Err(CoreError::new("cancelled", "逻辑快照已取消，账务未改变"))
    } else {
        Ok(())
    }
}

/// 包含账务明文，无 Debug / 隐式 Serialize；编码须显式提供预算。
pub struct LogicalSnapshot {
    pub(crate) contents: Contents,
    pub(crate) captured_rows: usize,
    pub(crate) captured_bytes: usize,
}

impl LogicalSnapshot {
    pub fn contents(&self) -> &Contents {
        &self.contents
    }

    pub fn captured_rows(&self) -> usize {
        self.captured_rows
    }

    /// 所读固定字段的 UTF-8 字节及整数各 8 字节，不含排除的原始 CSV BLOB。
    pub fn captured_bytes(&self) -> usize {
        self.captured_bytes
    }

    /// 固定字段顺序、稳定集合顺序的 JSON 候选；未冻结为备份字节协议。
    pub fn encode(&self, max_bytes: usize, cancel: &AtomicBool) -> Result<Vec<u8>> {
        encode_contents(&self.contents, max_bytes, cancel)
    }
}

pub(crate) fn encode_contents<T: Serialize>(
    contents: &T,
    max_bytes: usize,
    cancel: &AtomicBool,
) -> Result<Vec<u8>> {
    cancelled(cancel)?;
    if !(1..=MAX_BYTES).contains(&max_bytes) {
        return Err(limit());
    }
    let mut writer = Encoder {
        bytes: Vec::new(),
        max_bytes,
        cancel,
        failure: None,
    };
    if serde_json::to_writer(&mut writer, contents).is_err() {
        return Err(writer
            .failure
            .unwrap_or_else(|| CoreError::new("encoding_error", "逻辑快照无法编码")));
    }
    cancelled(cancel)?;
    Ok(writer.bytes)
}

struct Encoder<'a> {
    bytes: Vec<u8>,
    max_bytes: usize,
    cancel: &'a AtomicBool,
    failure: Option<CoreError>,
}

impl Write for Encoder<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let result = cancelled(self.cancel).and_then(|()| {
            if buffer.len() > self.max_bytes - self.bytes.len() {
                Err(limit())
            } else {
                Ok(())
            }
        });
        if let Err(error) = result {
            self.failure = Some(error);
            return Err(std::io::Error::other("snapshot encoding stopped"));
        }
        self.bytes.try_reserve(buffer.len()).map_err(|_| {
            self.failure = Some(CoreError::new(
                "resource_unavailable",
                "逻辑快照编码内存不可用",
            ));
            std::io::Error::other("snapshot allocation stopped")
        })?;
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Contents {
    pub facts: Facts,
    pub verification: Verification,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Facts {
    pub profile: String,
    pub source_schema: i64,
    pub sequence: i64,
    pub currency: String,
    pub minor_unit_scale: u8,
    /// 当前实验没有远端确认 / cursor 模型，不能伪造已同步范围。
    pub remote_knowledge: RemoteKnowledge,
    pub accounts: Vec<Reference>,
    pub categories: Vec<Reference>,
    pub revisions: Vec<Revision>,
    pub selected: Vec<Selection>,
    pub candidates: Vec<Selection>,
    /// 仅业务意图历史，无可发送 envelope 或会话；不会自动激活。
    pub intent_history: Vec<Intent>,
    pub imports: Option<ImportHistory>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteKnowledge {
    Unknown,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub archived: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Revision {
    pub id: String,
    pub event_id: String,
    pub predecessor: Option<String>,
    pub event: Option<Event>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    pub event_id: String,
    pub revision_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportHistory {
    /// 原本机实验目标，仅为来源证据，不授予新 runtime / 同步域身份。
    pub source_target_id: String,
    pub tasks: Vec<ImportTask>,
    pub rows: Vec<ImportRow>,
    pub sources: Vec<Source>,
    pub batches: Vec<ImportBatch>,
    pub undo_tasks: Vec<UndoTask>,
    pub undo_rows: Vec<UndoRow>,
    pub undo_batches: Vec<UndoBatch>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportTask {
    pub id: String,
    pub profile: String,
    pub input_fingerprint: String,
    pub mapping_fingerprint: String,
    pub mapping: Mapping,
    pub decisions: Vec<Decision>,
    pub state: TaskState,
    pub total: usize,
    pub next_record: usize,
    pub batch_size: usize,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportOutcome {
    Pending,
    Imported,
    Skipped,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportRow {
    pub task_id: String,
    pub record: usize,
    pub row_id: String,
    pub source_id: Option<String>,
    pub intent: Option<Intent>,
    pub outcome: ImportOutcome,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Source {
    pub namespace: String,
    pub source_id: String,
    pub event: Event,
    pub event_id: String,
    pub revision_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportBatch {
    pub task_id: String,
    pub first_record: usize,
    pub last_record: usize,
    pub imported: usize,
    pub skipped: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UndoSelection {
    pub record: usize,
    pub row_id: String,
    pub event_id: String,
    // 兼容原内部登记 request 的字段名称；不代表备份格式已冻结。
    pub revision_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UndoTask {
    pub id: String,
    pub import_task_id: String,
    pub selection: Vec<UndoSelection>,
    pub state: State,
    pub total: usize,
    pub next_ordinal: usize,
    pub batch_size: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UndoRow {
    pub undo_id: String,
    pub ordinal: usize,
    pub record: usize,
    pub row_id: String,
    pub intent: Intent,
    pub deleted: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UndoBatch {
    pub undo_id: String,
    pub first_ordinal: usize,
    pub last_ordinal: usize,
    pub deleted: usize,
}

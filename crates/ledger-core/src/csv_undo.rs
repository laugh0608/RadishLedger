//! M0-E 受限撤销候选；只处理现有平坦交易模型，不提供通用依赖推断。
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restriction {
    Pending,
    Skipped,
    ChangedRevision,
    ConflictCandidates,
    DependentHistory,
    Deleted,
    AlreadyUndone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Origin {
    pub record: usize,
    pub row_id: String,
    pub event_id: String,
    pub revision_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub record: usize,
    pub row_id: String,
    pub event_id: Option<String>,
    pub original_revision_id: Option<String>,
    pub restriction: Option<Restriction>,
}

/// 只能由同一读取快照构造；调用方不能将受限行改成已获确认的候选。
pub struct Preview {
    pub(crate) target_id: String,
    pub(crate) import_task_id: String,
    pub(crate) rows: Vec<Row>,
}

impl Preview {
    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    pub fn import_task_id(&self) -> &str {
        &self.import_task_id
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Ready,
    Cancelled,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub target_id: String,
    pub import_task_id: String,
    pub state: State,
    pub total: usize,
    /// 一基确认集合序号；不是原 CSV 记录号。
    pub next_ordinal: usize,
    pub batch_size: usize,
    pub deleted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub ordinal: usize,
    pub record: usize,
    pub row_id: String,
    pub event_id: String,
    pub original_revision_id: String,
    pub operation_id: String,
    pub revision_id: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub first_ordinal: usize,
    pub last_ordinal: usize,
    pub deleted: usize,
    pub replayed: bool,
}

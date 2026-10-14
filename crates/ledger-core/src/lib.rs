//! M0 移动接入所需的有界账务与加密事务核心；尚非可交付的用户账库。
pub mod domain;
pub mod probe;
pub mod store;
pub mod validation;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreError {
    pub code: &'static str,
    pub message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sqlite_code: Option<i32>,
}

impl CoreError {
    pub const fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            sqlite_code: None,
        }
    }
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CoreError {}

pub type Result<T> = std::result::Result<T, CoreError>;

pub fn random_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| CoreError::new("random_unavailable", "系统随机源不可用"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

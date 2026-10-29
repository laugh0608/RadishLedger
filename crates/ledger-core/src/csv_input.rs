//! 严格、有界的实验 CSV 方言；不猜分隔符，不修复畸形引号。
use crate::{CoreError, Result};

pub(super) const MAX_BYTES: usize = 1024 * 1024;
pub(super) const MAX_FIELD_BYTES: usize = 4096;
pub(super) const MAX_COLUMNS: usize = 16;
pub(super) const MAX_RECORDS: usize = 1000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Start,
    Bare,
    Quoted,
    Closed,
}

pub(super) struct Input {
    pub records: Vec<Vec<String>>,
    pub had_bom: bool,
}

fn append(field: &mut Vec<u8>, byte: u8) -> Result<()> {
    if field.len() == MAX_FIELD_BYTES {
        return Err(CoreError::new(
            "csv_field_limit",
            "CSV 字段超过实验字节上限",
        ));
    }
    field.push(byte);
    Ok(())
}

fn finish_field(record: &mut Vec<String>, field: &mut Vec<u8>) -> Result<()> {
    if record.len() == MAX_COLUMNS {
        return Err(CoreError::new("csv_column_limit", "CSV 列数超过实验上限"));
    }
    let text = String::from_utf8(std::mem::take(field))
        .map_err(|_| CoreError::new("csv_encoding", "CSV 必须是有效 UTF-8"))?;
    record.push(text);
    Ok(())
}

fn finish_record(
    records: &mut Vec<Vec<String>>,
    record: &mut Vec<String>,
    limit: usize,
) -> Result<()> {
    if records.len() == limit {
        return Err(CoreError::new("csv_record_limit", "CSV 记录数超过实验上限"));
    }
    records.push(std::mem::take(record));
    Ok(())
}

pub(super) fn parse(
    bytes: &[u8],
    has_header: bool,
    check_cancelled: &mut impl FnMut() -> Result<()>,
) -> Result<Input> {
    check_cancelled()?;
    if bytes.len() > MAX_BYTES {
        return Err(CoreError::new("csv_file_limit", "CSV 超过实验文件字节上限"));
    }
    std::str::from_utf8(bytes)
        .map_err(|_| CoreError::new("csv_encoding", "CSV 必须是有效 UTF-8"))?;
    if bytes.contains(&0) {
        return Err(CoreError::new("csv_nul", "CSV 不能包含 NUL"));
    }
    let had_bom = bytes.starts_with(b"\xef\xbb\xbf");
    let bytes = if had_bom { &bytes[3..] } else { bytes };
    let limit = MAX_RECORDS + usize::from(has_header);
    let (mut records, mut record, mut field) = (Vec::new(), Vec::new(), Vec::new());
    let mut state = State::Start;
    let mut i = 0;
    while i < bytes.len() {
        if i % 1024 == 0 {
            check_cancelled()?;
        }
        let byte = bytes[i];
        if state == State::Quoted {
            if byte == b'"' {
                state = State::Closed;
            } else {
                append(&mut field, byte)?;
            }
        } else if byte == b',' || byte == b'\n' || byte == b'\r' {
            if byte == b'\r' {
                if bytes.get(i + 1) != Some(&b'\n') {
                    return Err(CoreError::new("csv_newline", "记录分隔只支持 LF 或 CRLF"));
                }
                i += 1;
            }
            finish_field(&mut record, &mut field)?;
            if byte != b',' {
                finish_record(&mut records, &mut record, limit)?;
            }
            state = State::Start;
        } else if byte == b'"' {
            match state {
                State::Start => state = State::Quoted,
                State::Closed => {
                    append(&mut field, b'"')?;
                    state = State::Quoted;
                }
                _ => return Err(CoreError::new("csv_quote", "CSV 引号位置无效")),
            }
        } else {
            if state == State::Closed {
                return Err(CoreError::new("csv_quote", "闭合引号后存在非分隔内容"));
            }
            append(&mut field, byte)?;
            state = State::Bare;
        }
        i += 1;
    }
    if state == State::Quoted {
        return Err(CoreError::new("csv_quote", "CSV 引号未闭合"));
    }
    if state != State::Start || !record.is_empty() {
        finish_field(&mut record, &mut field)?;
        finish_record(&mut records, &mut record, limit)?;
    }
    if records.len() <= usize::from(has_header) {
        return Err(CoreError::new("csv_empty", "CSV 没有可预览的业务记录"));
    }
    check_cancelled()?;
    Ok(Input { records, had_bom })
}

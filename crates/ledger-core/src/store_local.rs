use super::{
    Store, Write, WriteEffect, apply_write, db_error, insert_account, insert_category,
    read_snapshot, valid_id,
};
use crate::domain::{Event, EventKind, parse_yuan};
use crate::logical_snapshot::Reference;
use crate::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteIdentity {
    operation_id: String,
    event_id: String,
    revision_id: String,
}

impl WriteIdentity {
    fn write(self, event: Option<Event>, previous_revision_id: Option<String>) -> Write {
        Write {
            operation_id: self.operation_id,
            event_id: self.event_id,
            revision_id: self.revision_id,
            previous_revision_id,
            event,
            expected_sequence: None,
            observed_balance_minor: None,
            target_balance_minor: None,
            resolve_candidates: Default::default(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Income,
    Expense,
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum LedgerCommand {
    Account {
        id: String,
        account_kind: String,
        name: String,
        amount_yuan: String,
        business_date: String,
        identity: WriteIdentity,
    },
    Category {
        id: String,
        category_kind: String,
        name: String,
    },
    Entry {
        direction: Direction,
        amount_yuan: String,
        business_date: String,
        account_id: String,
        category_id: String,
        identity: WriteIdentity,
    },
    DeleteEntry {
        previous_revision_id: String,
        identity: WriteIdentity,
    },
    ReviseEntry {
        previous_revision_id: String,
        amount_yuan: String,
        business_date: String,
        identity: WriteIdentity,
    },
}

pub(crate) enum SaveAttempt<'a> {
    Operation(&'a str),
    Category(&'a str),
}

impl SaveAttempt<'_> {
    pub(crate) fn encode(&self) -> [u8; 32] {
        let (id, category) = match self {
            Self::Operation(id) => (id, false),
            Self::Category(id) => (id, true),
        };
        // 调用方在事务中已经验证了固定长度的小写十六进制身份。
        let mut bytes = [0; 32];
        bytes.copy_from_slice(id.as_bytes());
        if category {
            bytes[0] |= 0x80;
        }
        bytes
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BookView {
    accounts: Vec<Reference>,
    categories: Vec<Reference>,
    balances_minor: BTreeMap<String, String>,
    recent_entries: Vec<LocalEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalEntry {
    id: String,
    revision_id: String,
    kind: EventKind,
    business_date: String,
    account_id: String,
    category_id: String,
    amount_minor: String,
}

fn references(connection: &Connection, sql: &str) -> Result<Vec<Reference>> {
    let rows: Vec<_> = connection
        .prepare(sql)
        .map_err(db_error)?
        .query_map([], |row| {
            Ok(Reference {
                id: row.get(0)?,
                kind: row.get(1)?,
                name: row.get(2)?,
                archived: row.get(3)?,
            })
        })
        .map_err(db_error)?
        .collect::<std::result::Result<_, _>>()
        .map_err(db_error)?;
    if rows.len() > 256 {
        return Err(CoreError::new(
            "local_reference_limit",
            "内部版本每类最多显示 256 个账户或分类",
        ));
    }
    Ok(rows)
}

fn read_book(connection: &Connection) -> Result<BookView> {
    let accounts = references(
        connection,
        "SELECT id,kind,name,archived FROM accounts ORDER BY name,id LIMIT 257",
    )?;
    let categories = references(
        connection,
        "SELECT id,kind,name,archived FROM categories ORDER BY name,id LIMIT 257",
    )?;
    let balances_minor = read_snapshot(connection, &[])?.projection.balances_minor;
    let mut statement = connection
        .prepare(
            "SELECT s.event_id,s.revision_id,r.payload FROM selected s JOIN revisions r ON r.id=s.revision_id
             WHERE json_extract(r.payload,'$.kind') IN ('income','expense')
             ORDER BY json_extract(r.payload,'$.businessDate') DESC,s.event_id DESC LIMIT 20",
        )
        .map_err(db_error)?;
    let recent_entries = statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get::<_, String>(2)?))
        })
        .map_err(db_error)?
        .map(|row| {
            let (id, revision_id, payload) = row.map_err(db_error)?;
            let event: Event = serde_json::from_str(&payload)
                .map_err(|_| CoreError::new("corrupt_fact", "账务事实无法解释"))?;
            let (Some(account_id), Some(category_id)) = (event.account_id, event.category_id)
            else {
                return Err(CoreError::new("corrupt_fact", "收支引用不完整"));
            };
            if !categories.iter().any(|category| category.id == category_id) {
                return Err(CoreError::new("missing_category", "无法解释流水的原分类"));
            }
            Ok(LocalEntry {
                id,
                revision_id,
                kind: event.kind,
                business_date: event.business_date,
                account_id,
                category_id,
                amount_minor: event.amount_minor,
            })
        })
        .collect::<Result<_>>()?;
    Ok(BookView {
        accounts,
        categories,
        balances_minor,
        recent_entries,
    })
}

// 使用表单所见的不可变修订。apply_write 在同一事务内核对重放、当前所选版本和候选。
fn ordinary_entry_revision(
    connection: &Connection,
    event_id: &str,
    revision_id: &str,
) -> Result<Event> {
    valid_id(event_id)?;
    valid_id(revision_id)?;
    let payload: Option<String> = connection
        .query_row(
            "SELECT payload FROM revisions WHERE id=?1 AND event_id=?2 AND payload IS NOT NULL",
            [revision_id, event_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    let payload =
        payload.ok_or_else(|| CoreError::new("stale_revision", "原流水修订不存在，请重新核对"))?;
    let event: Event = serde_json::from_str(&payload)
        .map_err(|_| CoreError::new("corrupt_fact", "账务事实无法解释"))?;
    if !matches!(event.kind, EventKind::Income | EventKind::Expense) {
        return Err(CoreError::new(
            "invalid_entry_kind",
            "此操作仅支持普通收入或支出",
        ));
    }
    event.validate()?;
    Ok(event)
}

fn checked_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.contains('\0') {
        return Err(CoreError::new("invalid_name", "请输入有效的名称"));
    }
    Ok(name)
}

fn existing_reference(
    connection: &Connection,
    sql: &str,
    id: &str,
    kind: &str,
    name: &str,
) -> Result<bool> {
    valid_id(id)?;
    let prior: Option<(String, String, bool)> = connection
        .query_row(sql, [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()
        .map_err(db_error)?;
    match prior {
        None => Ok(false),
        Some((prior_kind, prior_name, false)) if prior_kind == kind && prior_name == name => {
            Ok(true)
        }
        Some(_) => Err(CoreError::new(
            "operation_reused",
            "引用身份已经用于其他意图",
        )),
    }
}

impl Store {
    pub(crate) fn local_book(&self) -> Result<BookView> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        let book = read_book(&tx)?;
        tx.commit().map_err(db_error)?;
        Ok(book)
    }

    pub(crate) fn local_save_committed(&self, mut receipt: [u8; 32]) -> Result<bool> {
        let category = receipt[0] & 0x80 != 0;
        receipt[0] &= 0x7f;
        let id = std::str::from_utf8(&receipt)
            .map_err(|_| CoreError::new("invalid_save_receipt", "无法核对上次保存"))?;
        valid_id(id)?;
        let sql = if category {
            "SELECT EXISTS(SELECT 1 FROM categories WHERE id=?1)"
        } else {
            "SELECT EXISTS(SELECT 1 FROM outbox WHERE operation_id=?1)"
        };
        self.connection
            .query_row(sql, [id], |row| row.get(0))
            .map_err(db_error)
    }

    pub(crate) fn save_local(
        &mut self,
        command: LedgerCommand,
        before_commit: impl FnOnce(SaveAttempt<'_>) -> Result<()>,
    ) -> Result<BookView> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let (is_category, receipt_id) = match command {
            LedgerCommand::Account {
                id,
                account_kind,
                name,
                amount_yuan,
                business_date,
                identity,
            } => {
                let name = checked_name(&name)?;
                let opening = Event {
                    kind: EventKind::Opening,
                    business_date,
                    account_id: Some(id.clone()),
                    category_id: None,
                    from_account_id: None,
                    to_account_id: None,
                    amount_minor: parse_yuan(&amount_yuan, true)?.to_string(),
                    note: String::new(),
                };
                let existed = existing_reference(
                    &tx,
                    "SELECT kind,name,archived FROM accounts WHERE id=?1",
                    &id,
                    &account_kind,
                    name,
                )?;
                if !existed {
                    insert_account(&tx, &id, &account_kind, name)?;
                }
                let write = identity.write(Some(opening), None);
                let effect = apply_write(&tx, &write, &mut |_| Ok(()))?;
                if existed != (effect == WriteEffect::Replayed) {
                    return Err(CoreError::new(
                        "operation_reused",
                        "已有账户须核对原期初操作",
                    ));
                }
                (false, write.operation_id)
            }
            LedgerCommand::Category {
                id,
                category_kind,
                name,
            } => {
                let name = checked_name(&name)?;
                if !existing_reference(
                    &tx,
                    "SELECT kind,name,archived FROM categories WHERE id=?1",
                    &id,
                    &category_kind,
                    name,
                )? {
                    insert_category(&tx, &id, &category_kind, name)?;
                }
                (true, id)
            }
            LedgerCommand::Entry {
                direction,
                amount_yuan,
                business_date,
                account_id,
                category_id,
                identity,
            } => {
                let event = Event {
                    kind: match direction {
                        Direction::Income => EventKind::Income,
                        Direction::Expense => EventKind::Expense,
                    },
                    business_date,
                    account_id: Some(account_id),
                    category_id: Some(category_id),
                    from_account_id: None,
                    to_account_id: None,
                    amount_minor: parse_yuan(&amount_yuan, false)?.to_string(),
                    note: String::new(),
                };
                let write = identity.write(Some(event), None);
                apply_write(&tx, &write, &mut |_| Ok(()))?;
                (false, write.operation_id)
            }
            LedgerCommand::DeleteEntry {
                identity,
                previous_revision_id,
            } => {
                ordinary_entry_revision(&tx, &identity.event_id, &previous_revision_id)?;
                let write = identity.write(None, Some(previous_revision_id));
                apply_write(&tx, &write, &mut |_| Ok(()))?;
                (false, write.operation_id)
            }
            LedgerCommand::ReviseEntry {
                identity,
                previous_revision_id,
                amount_yuan,
                business_date,
            } => {
                let mut event =
                    ordinary_entry_revision(&tx, &identity.event_id, &previous_revision_id)?;
                event.amount_minor = parse_yuan(&amount_yuan, false)?.to_string();
                event.business_date = business_date;
                let write = identity.write(Some(event), Some(previous_revision_id));
                apply_write(&tx, &write, &mut |_| Ok(()))?;
                (false, write.operation_id)
            }
        };
        let book = read_book(&tx)?;
        before_commit(if is_category {
            SaveAttempt::Category(&receipt_id)
        } else {
            SaveAttempt::Operation(&receipt_id)
        })?;
        tx.commit().map_err(db_error)?;
        Ok(book)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::DbKey;

    fn delete(event_id: &str, previous: &str, operation: &str, revision: &str) -> LedgerCommand {
        LedgerCommand::DeleteEntry {
            previous_revision_id: previous.into(),
            identity: WriteIdentity {
                event_id: event_id.into(),
                operation_id: operation.into(),
                revision_id: revision.into(),
            },
        }
    }

    #[test]
    fn ordinary_delete_reopens_with_tombstone_and_replays_after_restore() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("delete.db");
        let key = DbKey::random().unwrap();
        let mut store = Store::create(&path, &key).unwrap();
        crate::probe::populate_snapshot_fixture(&mut store, "A08", "edit-date").unwrap();
        let entry = store.local_book().unwrap().recent_entries.remove(0);
        let original = store.revision_event(&entry.revision_id).unwrap().unwrap();
        let operation = crate::random_id().unwrap();
        let tombstone = crate::random_id().unwrap();
        let command = || delete(&entry.id, &entry.revision_id, &operation, &tombstone);
        let view = store.save_local(command(), |_| Ok(())).unwrap();
        assert!(view.recent_entries.is_empty());
        assert_eq!(view.balances_minor[&entry.account_id], "10000");
        drop(store);
        let mut store = Store::open(&path, &key).unwrap();
        let snapshot = store
            .snapshot(&["2026-01".into(), "2026-02".into()])
            .unwrap();
        assert_eq!(snapshot.deleted_event_ids, [entry.id.clone()].into());
        assert_eq!(snapshot.selected_revision_ids[&entry.id], tombstone);
        assert_eq!(
            snapshot.projection.balances_minor[&entry.account_id],
            "10000"
        );
        for month in ["2026-01", "2026-02"] {
            let report = &snapshot.projection.monthly_reports[month];
            assert_eq!(report.net_expense_minor, "0");
            assert_eq!(report.expense_categories_minor[&entry.category_id], "0");
        }
        assert_eq!(store.revision_event(&tombstone).unwrap(), None);
        assert_eq!(
            store.revision_event(&entry.revision_id).unwrap(),
            Some(original.clone())
        );
        let digest = store.content_digest().unwrap();
        store.save_local(command(), |_| Ok(())).unwrap();
        let duplicate = delete(
            &entry.id,
            &tombstone,
            &crate::random_id().unwrap(),
            &crate::random_id().unwrap(),
        );
        assert_eq!(
            store.save_local(duplicate, |_| Ok(())).err().unwrap().code,
            "stale_revision"
        );
        assert_eq!(store.content_digest().unwrap(), digest);

        let restored = crate::random_id().unwrap();
        let restore = WriteIdentity {
            operation_id: crate::random_id().unwrap(),
            event_id: entry.id.clone(),
            revision_id: restored.clone(),
        }
        .write(Some(original), Some(tombstone.clone()));
        store.commit(&restore).unwrap();
        let digest = store.content_digest().unwrap();
        let view = store.save_local(command(), |_| Ok(())).unwrap();
        assert_eq!(view.recent_entries[0].revision_id, restored);
        assert_eq!(view.balances_minor[&entry.account_id], "7500");
        assert_eq!(store.content_digest().unwrap(), digest);
    }

    #[test]
    fn ordinary_delete_rejects_stale_conflicted_and_non_entry_targets() {
        let dir = tempfile::tempdir().unwrap();
        let mut store =
            Store::create(&dir.path().join("refused.db"), &DbKey::random().unwrap()).unwrap();
        let aliases =
            crate::probe::populate_snapshot_fixture(&mut store, "A08", "edit-date").unwrap();
        let id = |name| {
            aliases
                .iter()
                .find(|(_, label)| label.as_str() == name)
                .unwrap()
                .0
        };
        let event = id("expense");
        let older = id("expense-v1");
        let current = id("expense-v2");
        let candidate = store.revision_event(older).unwrap().unwrap();
        store
            .receive_candidate(event, &crate::random_id().unwrap(), older, &candidate)
            .unwrap();
        let digest = store.content_digest().unwrap();
        for (target, previous, code) in [
            (event, older, "stale_revision"),
            (event, current, "stale_candidates"),
            (id("open-cash"), id("open-cash-v1"), "invalid_entry_kind"),
            (id("open-cash"), current, "stale_revision"),
        ] {
            let command = delete(
                target,
                previous,
                &crate::random_id().unwrap(),
                &crate::random_id().unwrap(),
            );
            assert_eq!(
                store
                    .save_local(command, |_| panic!("rejected delete registered"))
                    .err()
                    .unwrap()
                    .code,
                code
            );
        }
        assert_eq!(store.content_digest().unwrap(), digest);
    }
}

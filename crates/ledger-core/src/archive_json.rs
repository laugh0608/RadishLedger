//! 先限额与拒绝重复键，再解为唯一事实类型；复用捕获端的关系 / 投影校验。
use super::*;
use serde::Deserialize;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt;

struct Budget<'a> {
    limits: Limits,
    cancel: &'a AtomicBool,
    strings: usize,
    entries: usize,
    records: usize,
    failure: Option<CoreError>,
}

impl Budget<'_> {
    fn fail<E: de::Error>(&mut self, error: CoreError) -> E {
        self.failure = Some(error);
        E::custom("archive input rejected")
    }
    fn check<E: de::Error>(&mut self) -> std::result::Result<(), E> {
        cancelled(self.cancel).map_err(|e| self.fail(e))
    }
    fn text<E: de::Error>(&mut self, text: &str) -> std::result::Result<(), E> {
        self.check()?;
        if text.len() > self.limits.string_bytes
            || text.len() > self.limits.decoded_string_bytes - self.strings
        {
            return Err(self.fail(limit()));
        }
        self.strings += text.len();
        Ok(())
    }
    fn entry<E: de::Error>(&mut self) -> std::result::Result<(), E> {
        self.check()?;
        if self.entries == self.limits.collection_entries {
            return Err(self.fail(limit()));
        }
        self.entries += 1;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Context {
    Root,
    Facts,
    Imports,
    Records,
    Other,
}
struct Seed<'a, 'b> {
    budget: &'a mut Budget<'b>,
    depth: usize,
    context: Context,
}
impl<'de> DeserializeSeed<'de> for Seed<'_, '_> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> std::result::Result<(), D::Error> {
        self.budget.check()?;
        decoder.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Seed<'_, '_> {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded archive JSON")
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<(), E> {
        self.budget.check()
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> std::result::Result<(), E> {
        self.budget.check()
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> std::result::Result<(), E> {
        self.budget.check()
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> std::result::Result<(), E> {
        self.budget.check()
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<(), E> {
        self.budget.check()
    }
    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<(), E> {
        self.budget.text(value)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> std::result::Result<(), A::Error> {
        if self.depth >= self.budget.limits.json_depth {
            return Err(self.budget.fail(limit()));
        }
        while let Some(()) = sequence.next_element_seed(Seed {
            budget: self.budget,
            depth: self.depth + 1,
            context: Context::Other,
        })? {
            self.budget.entry()?;
            if matches!(self.context, Context::Records) {
                if self.budget.records == self.budget.limits.records {
                    return Err(self.budget.fail(limit()));
                }
                self.budget.records += 1;
            }
        }
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<(), A::Error> {
        if self.depth >= self.budget.limits.json_depth {
            return Err(self.budget.fail(limit()));
        }
        let mut keys = BTreeSet::new();
        while let Some(key) = map.next_key_seed(Key {
            budget: self.budget,
        })? {
            self.budget.entry()?;
            let context = match (self.context, key.as_str()) {
                (Context::Root, "facts") => Context::Facts,
                (Context::Facts, "imports") => Context::Imports,
                (
                    Context::Facts,
                    "accounts" | "categories" | "revisions" | "selected" | "candidates"
                    | "intentHistory",
                )
                | (
                    Context::Imports,
                    "tasks" | "rows" | "sources" | "batches" | "undoTasks" | "undoRows"
                    | "undoBatches",
                ) => Context::Records,
                _ => Context::Other,
            };
            if !keys.insert(key) {
                return Err(self.budget.fail(payload_error()));
            }
            map.next_value_seed(Seed {
                budget: self.budget,
                depth: self.depth + 1,
                context,
            })?;
        }
        Ok(())
    }
}

struct Key<'a, 'b> {
    budget: &'a mut Budget<'b>,
}
impl<'de> DeserializeSeed<'de> for Key<'_, '_> {
    type Value = String;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> std::result::Result<String, D::Error> {
        decoder.deserialize_str(self)
    }
}
impl Visitor<'_> for Key<'_, '_> {
    type Value = String;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded unique object key")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<String, E> {
        self.budget.text(value)?;
        let mut key = String::new();
        key.try_reserve_exact(value.len())
            .map_err(|_| self.budget.fail(resource()))?;
        key.push_str(value);
        Ok(key)
    }
}

pub(super) fn scan(input: &[u8], limits: Limits, cancel: &AtomicBool) -> Result<()> {
    cancelled(cancel)?;
    if input.is_empty() || input.len() > limits.payload_bytes {
        return Err(limit());
    }
    if input.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(payload_error());
    }
    let mut budget = Budget {
        limits,
        cancel,
        strings: 0,
        entries: 0,
        records: 0,
        failure: None,
    };
    let mut decoder = serde_json::Deserializer::from_slice(input);
    if (Seed {
        budget: &mut budget,
        depth: 0,
        context: Context::Root,
    })
    .deserialize(&mut decoder)
    .is_err()
        || decoder.end().is_err()
    {
        return Err(budget.failure.unwrap_or_else(payload_error));
    }
    cancelled(cancel)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    facts: Facts,
    // 复用实际重建的 Snapshot 序列化比较，不另复制一份账务核对字段契约。
    verification: Value,
}

pub(super) fn decode(input: &[u8], limits: Limits, cancel: &AtomicBool) -> Result<Contents> {
    scan(input, limits, cancel)?;
    let wire: Wire = serde_json::from_slice(input).map_err(|_| payload_error())?;
    cancelled(cancel)?;
    let facts = wire.facts;
    if facts.profile != crate::logical_snapshot::PROFILE
        || !matches!(facts.source_schema, 1..=3)
        || facts.sequence < 0
        || facts.currency != "CNY"
        || facts.minor_unit_scale != 2
        || (facts.source_schema >= 2) != facts.imports.is_some()
        || (facts.source_schema == 2
            && facts.imports.as_ref().is_some_and(|i| {
                !i.undo_tasks.is_empty() || !i.undo_rows.is_empty() || !i.undo_batches.is_empty()
            }))
    {
        return Err(payload_error());
    }
    let reports = wire
        .verification
        .get("monthlyReports")
        .and_then(Value::as_object)
        .ok_or_else(payload_error)?;
    if reports.len() > 120 {
        return Err(limit());
    }
    let mut months = Vec::new();
    months
        .try_reserve_exact(reports.len())
        .map_err(|_| resource())?;
    for month in reports.keys() {
        cancelled(cancel)?;
        if month.len() != 7 || crate::domain::validate_date(&format!("{month}-01")).is_err() {
            return Err(payload_error());
        }
        months.push(month.clone());
    }
    let verification = crate::store::snapshot_store::validate::verify(&facts, &months, cancel)
        .map_err(|e| {
            if e.code == "cancelled" {
                e
            } else {
                payload_error()
            }
        })?;
    cancelled(cancel)?;
    if serde_json::to_value(&verification).map_err(|_| payload_error())? != wire.verification {
        return Err(payload_error());
    }
    Ok(Contents {
        facts,
        verification,
    })
}

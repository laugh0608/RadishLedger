use super::*;
use crate::store::DbKey;
use serde_json::Value;

pub(crate) const FIXTURE: &str = include_str!("../../../docs/validation/csv-preview-cases.json");
pub(crate) const ACCOUNTING: &str = include_str!("../../../docs/validation/accounting-cases.json");
pub(crate) const HEADER: &str = "类型,金额,日期,币种,账户,类别,转出,转入,来源,备注\n";
pub(crate) const ONE: &str = "支出,1.00,2026-01-01,CNY,现金,餐饮,,,src-1,合成备注\n";

pub(crate) fn id(n: usize) -> String {
    format!("{n:032x}")
}

pub(crate) fn mapping() -> Mapping {
    Mapping {
        has_header: true,
        fields: vec![
            Field::Kind,
            Field::Amount,
            Field::Date,
            Field::Currency,
            Field::Account,
            Field::Category,
            Field::FromAccount,
            Field::ToAccount,
            Field::SourceId,
            Field::Note,
        ],
        source_namespace: "source-A".into(),
        kinds: [
            ("收入", EventKind::Income),
            ("支出", EventKind::Expense),
            ("转账", EventKind::Transfer),
            ("还款", EventKind::Transfer),
            ("退款", EventKind::Refund),
            ("期初", EventKind::Opening),
            ("调整", EventKind::Adjustment),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v))
        .collect(),
        accounts: [("现金", id(1)), ("银行", id(2)), ("信用卡", id(3))]
            .into_iter()
            .map(|(k, v)| (k.into(), v))
            .collect(),
        categories: [("工资", id(4)), ("餐饮", id(5)), ("购物", id(6))]
            .into_iter()
            .map(|(k, v)| (k.into(), v))
            .collect(),
    }
}

pub(crate) fn database() -> (tempfile::TempDir, DbKey, Store) {
    let directory = tempfile::tempdir().unwrap();
    let key = DbKey::random().unwrap();
    let mut store = Store::create(&directory.path().join("preview.db"), &key).unwrap();
    for (n, kind) in [(1, "cash"), (2, "bank"), (3, "credit_card")] {
        store.add_account(&id(n), kind, "合成账户").unwrap();
    }
    for (n, kind) in [(4, "income"), (5, "expense"), (6, "expense")] {
        store.add_category(&id(n), kind, "合成分类").unwrap();
    }
    (directory, key, store)
}

pub(crate) fn show(store: &Store, text: &str, map: &Mapping) -> Preview {
    preview(store, text.as_bytes(), map, &AtomicBool::new(false)).unwrap()
}

pub(crate) fn error(result: Result<Preview>, code: &str) {
    assert_eq!(result.err().expect("expected rejection").code, code);
}

pub(crate) fn event_from_step(step: &Value, opening: bool) -> Event {
    let account = |key: &str| {
        step[key].as_str().map(|v| match v {
            "cash" => id(1),
            "bank" => id(2),
            "card" => id(3),
            _ => panic!("fixture account"),
        })
    };
    Event {
        note: String::new(),
        kind: if opening {
            EventKind::Opening
        } else {
            serde_json::from_value(step["type"].clone()).unwrap()
        },
        business_date: step["businessDate"].as_str().unwrap().into(),
        amount_minor: step[if opening { "deltaMinor" } else { "amountMinor" }]
            .as_str()
            .unwrap()
            .into(),
        account_id: account("accountId"),
        from_account_id: account("fromAccountId"),
        to_account_id: account("toAccountId"),
        category_id: step["categoryId"].as_str().map(|v| match v {
            "salary" => id(4),
            "dining" => id(5),
            "shopping" => id(6),
            _ => panic!("fixture category"),
        }),
    }
}

pub(crate) fn normalize(value: Value) -> Value {
    let alias = |s: String| {
        (1..=6)
            .find(|n| s == id(*n))
            .map(|n| ["cash", "bank", "card", "salary", "dining", "shopping"][n - 1].into())
            .unwrap_or(s)
    };
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (alias(k), normalize(v)))
                .collect(),
        ),
        other => other,
    }
}

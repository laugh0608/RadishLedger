use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub fn minor(value: &str) -> Result<i64> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty()
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
        || value == "-0"
    {
        return Err(CoreError::new(
            "invalid_amount",
            "金额须为规范的整数分字符串",
        ));
    }
    value
        .parse()
        .map_err(|_| CoreError::new("out_of_range", "金额超出范围"))
}

pub fn parse_yuan(value: &str, signed: bool) -> Result<i64> {
    if value.is_empty() || value.ends_with('.') {
        return Err(CoreError::new("incomplete_input", "金额尚未输入完成"));
    }
    if value.contains(',') {
        return Err(CoreError::new("ambiguous_format", "金额分隔格式不明确"));
    }
    let negative = value.starts_with('-');
    let digits = value.strip_prefix('-').unwrap_or(value);
    let mut parts = digits.split('.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(CoreError::new("unsupported_format", "不支持此金额格式"));
    }
    if fraction.len() > 2 {
        return Err(CoreError::new("excess_precision", "人民币金额最多两位小数"));
    }
    if whole.len() > 18 {
        return Err(CoreError::new("out_of_range", "金额超出范围"));
    }
    let units = whole
        .parse::<i128>()
        .map_err(|_| CoreError::new("out_of_range", "金额超出范围"))?;
    let cents = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<i128>()
            .map_err(|_| CoreError::new("unsupported_format", "不支持此金额格式"))?
            * if fraction.len() == 1 { 10 } else { 1 }
    };
    let result = (units * 100 + cents) * if negative { -1 } else { 1 };
    let result =
        i64::try_from(result).map_err(|_| CoreError::new("out_of_range", "金额超出范围"))?;
    if !signed && result <= 0 {
        return Err(CoreError::new("non_positive", "普通金额必须为正"));
    }
    Ok(result)
}

pub fn validate_date(value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(CoreError::new("incomplete_input", "请选择业务日期"));
    }
    if value.len() != 10
        || value.as_bytes()[4] != b'-'
        || value.as_bytes()[7] != b'-'
        || !value
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return Err(CoreError::new("ambiguous_format", "业务日期格式不明确"));
    }
    let date_error = || CoreError::new("invalid_calendar_date", "业务日期不是有效日历日");
    let year = value[..4].parse::<u32>().map_err(|_| date_error())?;
    let month = value[5..7].parse::<usize>().map_err(|_| date_error())?;
    let day = value[8..].parse::<u32>().map_err(|_| date_error())?;
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if year == 0 || !(1..=12).contains(&month) || day == 0 || day > days[month - 1] {
        return Err(CoreError::new(
            "invalid_calendar_date",
            "业务日期不是有效日历日",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Event {
    pub kind: EventKind,
    pub business_date: String,
    pub account_id: Option<String>,
    pub category_id: Option<String>,
    pub from_account_id: Option<String>,
    pub to_account_id: Option<String>,
    pub amount_minor: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Opening,
    Income,
    Expense,
    Transfer,
    Refund,
    Adjustment,
}

impl Event {
    pub fn validate(&self) -> Result<i64> {
        if self.note.len() > 4096 || self.note.contains('\0') {
            return Err(CoreError::new("invalid_note", "备注超过边界或含 NUL"));
        }
        validate_date(&self.business_date)?;
        let value = minor(&self.amount_minor)?;
        if !matches!(self.kind, EventKind::Opening | EventKind::Adjustment) && value <= 0 {
            return Err(CoreError::new("non_positive", "普通金额必须为正"));
        }
        if self.kind == EventKind::Transfer {
            if self.from_account_id.is_none()
                || self.to_account_id.is_none()
                || self.account_id.is_some()
                || self.category_id.is_some()
            {
                return Err(CoreError::new("invalid_reference", "转账引用不完整"));
            }
            if self.from_account_id == self.to_account_id {
                return Err(CoreError::new("same_account", "转账账户必须不同"));
            }
        } else if self.account_id.is_none()
            || self.from_account_id.is_some()
            || self.to_account_id.is_some()
        {
            return Err(CoreError::new("invalid_reference", "资金账户引用不完整"));
        } else if self.category_id.is_some()
            != matches!(
                self.kind,
                EventKind::Income | EventKind::Expense | EventKind::Refund
            )
        {
            return Err(CoreError::new(
                "invalid_reference",
                "分类引用与事件类型不匹配",
            ));
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MonthlyReport {
    pub income_minor: String,
    pub gross_expense_minor: String,
    pub refund_minor: String,
    pub net_expense_minor: String,
    pub surplus_minor: String,
    pub expense_categories_minor: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Projection {
    pub balances_minor: BTreeMap<String, String>,
    pub net_assets_minor: String,
    pub assets_minor: String,
    pub liabilities_minor: String,
    pub monthly_reports: BTreeMap<String, MonthlyReport>,
}

fn narrow(value: i128) -> Result<String> {
    i64::try_from(value)
        .map(|v| v.to_string())
        .map_err(|_| CoreError::new("projection_overflow", "派生金额超出范围，账务事实仍保留"))
}

pub fn project(
    accounts: &[String],
    expense_categories: &[String],
    events: &[Event],
    months: &[String],
) -> Result<Projection> {
    let mut balances: BTreeMap<String, i128> = accounts.iter().map(|id| (id.clone(), 0)).collect();
    for event in events {
        let amount = i128::from(event.validate()?);
        let changes = if event.kind == EventKind::Transfer {
            vec![
                (event.from_account_id.as_ref(), -amount),
                (event.to_account_id.as_ref(), amount),
            ]
        } else {
            vec![(
                event.account_id.as_ref(),
                if event.kind == EventKind::Expense {
                    -amount
                } else {
                    amount
                },
            )]
        };
        for (account, delta) in changes {
            let current = account
                .and_then(|id| balances.get_mut(id))
                .ok_or_else(|| CoreError::new("missing_account", "资金账户不存在"))?;
            *current = current
                .checked_add(delta)
                .ok_or_else(|| CoreError::new("projection_overflow", "派生金额超出范围"))?;
        }
    }
    let sum = |values: Vec<i128>| -> Result<i128> {
        values.into_iter().try_fold(0_i128, |total, value| {
            total
                .checked_add(value)
                .ok_or_else(|| CoreError::new("projection_overflow", "派生金额超出范围"))
        })
    };
    let net = sum(balances.values().copied().collect())?;
    let assets = sum(balances.values().copied().filter(|v| *v > 0).collect())?;
    let liabilities = sum(balances
        .values()
        .copied()
        .filter(|v| *v < 0)
        .map(|v| -v)
        .collect())?;
    let mut reports = BTreeMap::new();
    for month in months {
        if month.len() != 7 || validate_date(&format!("{month}-01")).is_err() {
            return Err(CoreError::new("invalid_calendar_date", "统计月份无效"));
        }
        let (mut income, mut expense, mut refund) = (0_i128, 0_i128, 0_i128);
        let mut categories: BTreeMap<String, i128> = expense_categories
            .iter()
            .map(|id| (id.clone(), 0))
            .collect();
        for event in events
            .iter()
            .filter(|event| event.business_date.starts_with(month))
        {
            let amount = i128::from(event.validate()?);
            match event.kind {
                EventKind::Income => income += amount,
                EventKind::Expense | EventKind::Refund => {
                    let category = event
                        .category_id
                        .as_ref()
                        .and_then(|id| categories.get_mut(id))
                        .ok_or_else(|| CoreError::new("missing_category", "支出分类不存在"))?;
                    if event.kind == EventKind::Expense {
                        expense += amount;
                        *category += amount;
                    } else {
                        refund += amount;
                        *category -= amount;
                    }
                }
                _ => {}
            }
        }
        reports.insert(
            month.clone(),
            MonthlyReport {
                income_minor: narrow(income)?,
                gross_expense_minor: narrow(expense)?,
                refund_minor: narrow(refund)?,
                net_expense_minor: narrow(expense - refund)?,
                surplus_minor: narrow(income - expense + refund)?,
                expense_categories_minor: categories
                    .into_iter()
                    .map(|(id, v)| Ok((id, narrow(v)?)))
                    .collect::<Result<_>>()?,
            },
        );
    }
    Ok(Projection {
        balances_minor: balances
            .into_iter()
            .map(|(id, value)| Ok((id, narrow(value)?)))
            .collect::<Result<_>>()?,
        net_assets_minor: narrow(net)?,
        assets_minor: narrow(assets)?,
        liabilities_minor: narrow(liabilities)?,
        monthly_reports: reports,
    })
}

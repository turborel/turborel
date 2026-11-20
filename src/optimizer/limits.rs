//! \[DESIGN\] FETCH FIRST moved into the operands of UNION ALL (RULES.md
//! D374). OFFSET m and FETCH FIRST n ROWS ONLY keep the rows of their
//! input from the (m + 1)th to the (m + n)th in its order [ISO/IEC
//! 9075-2:2023 §7.17 GR6, GR7]. Of UNION ALL, whose rows are each
//! operand's [GR3b iii 3 A], those are among the first m + n of each
//! operand in that order; of its rows sorted, among each operand's first
//! m + n sorted by the same keys, its rows' order that of their keys
//! [§10.10 GR1h]. So each operand gets FETCH FIRST m + n ROWS ONLY, over a
//! sort by the same keys if the rows are sorted, and the OFFSET and FETCH
//! FIRST above stay to keep the rows they kept.
//!
//! Only constant counts move, which are evaluated once whatever the plan,
//! of ONLY and at least 1, which raises nothing [GR7c]: WITH TIES keeps the
//! peers of the last row too [GR7d i], which an operand's own last row
//! would not tell, and a PERCENT counts the rows of the whole input [GR7b
//! ii]. Rows peers in the sort keys come in an order the standard leaves
//! to the implementation [§10.10 GR1i] (US043), so which of them a count
//! keeps may change, never how many nor their keys.

use super::{Rule, RuleContext};
use crate::logical::{Expr, Fetch, Limit, LogicalPlan, Search, SetOp, SortKey};
use crate::value::{Decimal, Value};

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const PUSH_LIMIT_THROUGH_UNION: &str = "push_limit_through_union";

/// FETCH FIRST moved into the operands of UNION ALL, as the module says.
pub struct PushLimitThroughUnion;

impl Rule for PushLimitThroughUnion {
    fn name(&self) -> &'static str {
        PUSH_LIMIT_THROUGH_UNION
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Limit { limit, input } = plan else { return None };
        let (keys, union) =
            if let LogicalPlan::Sort { keys, input: rows } = &**input { (Some(keys.as_slice()), &**rows) } else { (None, &**input) };
        let LogicalPlan::SetOp { op: SetOp::Union, all: true, compared, left, right } = union else { return None };
        let (fetch, most) = moved(limit)?;
        if bounded(left, most, keys) && bounded(right, most, keys) {
            return None;
        }
        let each = |operand: &LogicalPlan| Box::new(limited(operand, (fetch, most), keys));
        let pushed = LogicalPlan::SetOp { op: SetOp::Union, all: true, compared: *compared, left: each(left), right: each(right) };
        let sorted = match keys {
            Some(keys) => LogicalPlan::Sort { keys: keys.to_vec(), input: Box::new(pushed) },
            None => pushed,
        };
        Some(LogicalPlan::Limit { limit: limit.clone(), input: Box::new(sorted) })
    }
}

/// Of `limit`, its FETCH FIRST and the rows an operand must give, OFFSET's
/// and FETCH FIRST's counts summed, if both are constants and it is of
/// ONLY, neither a PERCENT nor an approximate search.
fn moved(limit: &Limit) -> Option<(&Fetch, usize)> {
    let fetch = limit.fetch.as_ref()?;
    if fetch.ties.is_some() || fetch.percent || matches!(fetch.search, Search::Approximate(_)) {
        return None;
    }
    let skipped = match &limit.offset {
        Some(offset) => count(offset)?,
        None => 0,
    };
    let fetched = count(&fetch.count).filter(|n| *n >= 1)?;
    Some((fetch, skipped.checked_add(fetched)?))
}

/// A constant count of rows, an exact number of scale 0 not below 0.
fn count(e: &Expr) -> Option<usize> {
    let Expr::Const(Value::Exact(d)) = e else { return None };
    if d.scale != 0 {
        return None;
    }
    usize::try_from(d.units).ok()
}

/// Whether `operand` gives at most `most` rows already, of FETCH FIRST,
/// over a sort by `keys` if there are any: so the rule has moved its
/// count there, and moves nothing again.
fn bounded(operand: &LogicalPlan, most: usize, keys: Option<&[SortKey]>) -> bool {
    let LogicalPlan::Limit { limit, input } = operand else { return false };
    let within = limit.offset.is_none() && limit.most().is_some_and(|m| m <= most);
    let Some(keys) = keys else { return within };
    let LogicalPlan::Sort { keys: own, .. } = &**input else { return false };
    within && own.as_slice() == keys
}

/// `operand` under FETCH FIRST `most` ROWS ONLY of `fetch`'s search, over a
/// sort by `keys` if there are any.
fn limited(operand: &LogicalPlan, (fetch, most): (&Fetch, usize), keys: Option<&[SortKey]>) -> LogicalPlan {
    let rows = match keys {
        Some(keys) => LogicalPlan::Sort { keys: keys.to_vec(), input: Box::new(operand.clone()) },
        None => operand.clone(),
    };
    let units = i128::try_from(most).unwrap_or(i128::MAX);
    let count = Expr::Const(Value::Exact(Decimal::new(units, 0)));
    let limit = Limit { offset: None, fetch: Some(Fetch { count, ..fetch.clone() }) };
    LogicalPlan::Limit { limit, input: Box::new(rows) }
}

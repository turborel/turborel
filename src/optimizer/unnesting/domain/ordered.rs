//! \[DESIGN\] The dependent join with D pushed down ORDER BY, OFFSET and
//! FETCH FIRST, and window functions.
//!
//! A window partitions by D's columns too, so each binding's rows are
//! partitioned as the subquery's alone were (Neumann, "Improving
//! Unnesting of Complex Queries", 2025, §3.3). OFFSET and FETCH FIRST of
//! constant row counts keep the rows of each binding whose ROW_NUMBER,
//! in a window partitioned by D and ordered by the sort below them, is
//! past the offset and within the count [ISO/IEC 9075-2:2023 §7.17 GR6,
//! GR7] (§4.4 there). Its rows' order is the subquery's own, which only
//! those counts read [§7.17 GR5]. WITH TIES, PERCENT, APPROX and EXACT
//! are left nested.

use super::{Domain, shifted};
use crate::logical::{
    Bound, Cond, Exclusion, Expr, Frame, Limit, LogicalPlan, Search, SortKey, Units, Window, WindowCall, WindowKind, and_all,
};
use crate::value::{CompOp, Decimal, Value};

/// The label EXPLAIN shows of the row numbers a per-binding FETCH FIRST
/// reads.
const ROW_NUMBER: &str = "ROW_NUMBER()";

/// A count of `e`, a constant exact number of scale 0 that is not
/// negative.
fn count(e: &Expr) -> Option<i128> {
    let Expr::Const(Value::Exact(d)) = e else { return None };
    (d.scale == 0 && d.units >= 0).then_some(d.units)
}

/// The constant `n`.
fn constant(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

impl Domain {
    /// A sort by `keys`, of columns that follow D's.
    pub(super) fn sort(&self, keys: &[SortKey], input: &LogicalPlan) -> Option<LogicalPlan> {
        let keys = keys.iter().map(|k| SortKey { column: k.column.saturating_add(self.width), ..*k }).collect();
        Some(LogicalPlan::Sort { keys, input: Box::new(self.over(input)?) })
    }

    /// OFFSET and FETCH FIRST of constant row counts, for each binding.
    pub(super) fn limit(&self, limit: &Limit, input: &LogicalPlan) -> Option<LogicalPlan> {
        let offset = limit.offset.as_ref().map_or(Some(0), count)?;
        let most = match &limit.fetch {
            None => None,
            Some(f) => {
                let plain = !f.percent && f.ties.is_none() && matches!(f.search, Search::Unstated);
                if !plain {
                    return None;
                }
                Some(offset.checked_add(count(&f.count)?)?)
            }
        };
        let (order, rows) = if let LogicalPlan::Sort { keys, input } = input { (keys.clone(), &**input) } else { (Vec::new(), input) };
        let by = self.width;
        let wide = by.saturating_add(rows.width());
        let order = order.iter().map(|k| SortKey { column: k.column.saturating_add(by), ..*k }).collect();
        let frame = Frame {
            units: Units::Rows,
            start: Bound::UnboundedPreceding,
            end: Bound::CurrentRow,
            exclusion: Exclusion::NoOthers,
            matching: None,
        };
        let call = WindowCall { kind: WindowKind::RowNumber, frame, label: ROW_NUMBER.to_owned(), nested: Vec::new() };
        let numbered = Window { partition: (0..by).collect(), order, calls: vec![call], input: self.over(rows)? };
        let number = Expr::Column(wide);
        let mut conds = Vec::new();
        if offset > 0 {
            conds.push(Cond::Compare(number.clone(), CompOp::Gt, constant(offset)));
        }
        if let Some(m) = most {
            conds.push(Cond::Compare(number, CompOp::Le, constant(m)));
        }
        let kept = match and_all(conds) {
            Some(cond) => LogicalPlan::Filter { cond, input: Box::new(LogicalPlan::Window(Box::new(numbered))) },
            None => LogicalPlan::Window(Box::new(numbered)),
        };
        Some(LogicalPlan::Project { exprs: (0..wide).map(Expr::Column).collect(), input: Box::new(kept) })
    }

    /// Window functions, partitioned by D's columns too; not of a row
    /// pattern, which unnesting leaves alone.
    pub(super) fn window(&self, w: &Window<LogicalPlan>) -> Option<LogicalPlan> {
        if w.calls.iter().any(|c| c.frame.matching.is_some()) {
            return None;
        }
        let by = self.width;
        let mut moved = Window {
            partition: (0..by).chain(w.partition.iter().map(|p| p.saturating_add(by))).collect(),
            order: w.order.iter().map(|k| SortKey { column: k.column.saturating_add(by), ..*k }).collect(),
            calls: w.calls.clone(),
            input: self.over(&w.input)?,
        };
        moved.map_exprs(&mut |e| shifted(e, by));
        Some(LogicalPlan::Window(Box::new(moved)))
    }
}

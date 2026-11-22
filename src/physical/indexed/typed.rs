//! \[DESIGN\] The declared type of a column of a plan an index join's
//! values come from, which an index places among its keys only if of its
//! column's kind (`alike`). Of an UNNEST, an element's is its collection's
//! element type [ISO/IEC 9075-2:2023 §7.6 SR8n]; of a grouping, a set
//! function's its result type of its argument's; of a projection, a sum
//! or difference of exact numbers is exact (RULES.md D774).

use crate::logical::{AggCall, Expr, LogicalPlan, Unnest};
use crate::physical::PhysicalPlanner;
use crate::value::{ArithOp, DataType};

impl PhysicalPlanner<'_> {
    /// The type of `plan`'s column `column`, if it is a table's column.
    pub(in crate::physical) fn column_type(&self, plan: &LogicalPlan, column: usize) -> Option<DataType> {
        match plan {
            LogicalPlan::Scan { table, .. } => self.db.catalog.table(table)?.columns.get(column).map(|c| c.data_type.clone()),
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Distinct { input }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::SemiJoin { left: input, .. } => self.column_type(input, column),
            LogicalPlan::MarkJoin(m) => match column.checked_sub(m.left.width()) {
                Some(_) => Some(DataType::Boolean),
                None => self.column_type(&m.left, column),
            },
            LogicalPlan::Product { left, right } | LogicalPlan::OuterJoin { left, right, .. } => match column.checked_sub(left.width()) {
                Some(of_right) => self.column_type(right, of_right),
                None => self.column_type(left, column),
            },
            // A constant or slot a filter fixes, made a column (`indexed/fixed.rs`).
            LogicalPlan::Project { exprs, input } => self.expr_type(exprs.get(column)?, input),
            LogicalPlan::Values { types, .. } => types.get(column).cloned(),
            LogicalPlan::Unnest(u) => self.unnested_type(u, column),
            LogicalPlan::Aggregate { groups, aggs, input, .. } => match column.checked_sub(groups.len()) {
                Some(at) => self.aggregated_type(aggs.get(at)?, input),
                None => self.column_type(input, *groups.get(column)?),
            },
            LogicalPlan::SetOp { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::Window(..)
            | LogicalPlan::Recognize(..)
            | LogicalPlan::Polymorphic(..)
            | LogicalPlan::WorkTable { .. }
            | LogicalPlan::Transition { .. } => None,
        }
    }

    /// The type of `e`'s values over `input`'s rows, if known: a column's,
    /// a constant's or a slot's, a CAST's target, a function's result of
    /// its arguments' types, or of a sum or difference of exact numbers an
    /// exact number.
    fn expr_type(&self, e: &Expr, input: &LogicalPlan) -> Option<DataType> {
        if let Expr::Column(c) = e {
            return self.column_type(input, *c);
        }
        if let Expr::Dynamic(slot) = e {
            return Some(slot.data_type.clone());
        }
        if let Expr::Const(value) = e {
            return crate::exec::slot_type(value);
        }
        if let Expr::Cast(_, t) = e {
            return Some(t.clone());
        }
        if let Expr::Call(f, args) = e {
            let types = args.iter().map(|a| self.expr_type(a, input)).collect::<Option<Vec<_>>>()?;
            return f.0.return_type(&types).ok();
        }
        let Expr::Arith(a, ArithOp::Add | ArithOp::Sub, b) = e else { return None };
        let (of_a, of_b) = (self.expr_type(a, input)?, self.expr_type(b, input)?);
        matches!((&of_a, &of_b), (DataType::Exact { .. }, DataType::Exact { .. })).then_some(of_a)
    }

    /// The type of column `column` of the UNNEST `u`: its input's, or its
    /// collection's element type, of one that is no row type; none of its
    /// ordinal, a JSON table's or a GRAPH_TABLE's.
    fn unnested_type(&self, u: &Unnest<LogicalPlan>, column: usize) -> Option<DataType> {
        let Some(mut at) = column.checked_sub(u.input.width()) else { return self.column_type(&u.input, column) };
        if u.json.is_some() || u.graph.is_some() {
            return None;
        }
        for (collection, spread) in u.collections.iter().zip(&u.spread) {
            let width = spread.unwrap_or(1);
            if at >= width {
                at = at.saturating_sub(width);
                continue;
            }
            let of = self.expr_type(collection, &u.input)?;
            if let DataType::Array(a) = &of {
                return spread.is_none().then(|| a.element.clone());
            }
            let DataType::Multiset(m) = &of else { return None };
            return spread.is_none().then(|| m.element.clone());
        }
        None
    }

    /// The type of the set function `call`'s result over `input`'s rows,
    /// if its argument's is known.
    fn aggregated_type(&self, call: &AggCall, input: &LogicalPlan) -> Option<DataType> {
        let arg = match &call.arg {
            Some(e) => Some(self.expr_type(e, input)?),
            None => None,
        };
        call.func.0.result_type(arg).ok()
    }
}

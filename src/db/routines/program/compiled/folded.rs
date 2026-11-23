//! \[DESIGN\] A SELECT ... INTO of set functions over a delta table, of a
//! change a compiled program runs straight (RULES.md D933). The query
//! groups the delta table's rows by no column, so its set functions are
//! folded over the rows the change collected, as a direct read folds a
//! range's (`exec/range/fold.rs`), and its select list is evaluated over
//! the one row of their values, even of no rows [X3.135-1986 §5.23 GR1,
//! §5.25 GR1]. No plan of the query is opened and no table of the rows
//! is bound. A grouping by a column, a set function with a filter, and a
//! select list or an argument that reads beyond its row are the general
//! path's.

use super::change::remapped;
use super::run::Exec;
use crate::error::Result;
use crate::exec::{Framed, RangeAt, RangeFold};
use crate::logical::{AggCall, Expr, LogicalPlan};
use crate::storage::Row;

/// The set functions of a query over a delta table, each argument an
/// expression over the delta table's row.
#[derive(Debug)]
pub(super) struct Folded {
    fold: RangeFold,
    /// The functions' arguments, which say what columns it reads.
    read: Vec<Expr>,
}

impl Folded {
    /// The select list of `plan` over the row of its set functions'
    /// values, and those functions, if it is a grouping by no column of a
    /// delta table's rows alone.
    pub(super) fn of(plan: &LogicalPlan) -> Option<(Vec<Expr>, Folded)> {
        let LogicalPlan::Project { exprs, input } = plan else { return None };
        let LogicalPlan::Aggregate { groups, collations, aggs, seeds, input } = &**input else { return None };
        if !groups.is_empty() || !collations.is_empty() || !seeds.is_empty() || aggs.iter().any(|a| a.filter.is_some()) {
            return None;
        }
        let aggs = if let LogicalPlan::Transition { .. } = &**input {
            aggs.clone()
        } else {
            let LogicalPlan::Project { exprs: inner, input } = &**input else { return None };
            let LogicalPlan::Transition { .. } = &**input else { return None };
            let columns = inner.iter().map(|e| if let Expr::Column(c) = e { Some(*c) } else { None }).collect::<Option<Vec<usize>>>()?;
            let over = |a: &AggCall| {
                let arg = match &a.arg {
                    Some(e) => Some(remapped(e.clone(), &columns)?),
                    None => None,
                };
                Some(AggCall { arg, ..a.clone() })
            };
            aggs.iter().map(over).collect::<Option<Vec<AggCall>>>()?
        };
        let read = aggs.iter().filter_map(|a| a.arg.clone()).collect();
        let fold = RangeFold::of_aggs(&aggs)?;
        exprs.iter().all(Expr::reads_nothing).then(|| (exprs.clone(), Folded { fold, read }))
    }

    /// The functions' arguments.
    pub(super) fn read(&self) -> &[Expr] {
        &self.read
    }

    /// The row of the functions' values over the rows `visit` shows, each
    /// a row of the delta table.
    ///
    /// # Errors
    ///
    /// An argument's, a function's, or `visit`'s.
    pub(super) fn row(&self, exec: &Exec<'_>, visit: impl FnOnce(&mut dyn FnMut(&Row) -> Result<()>) -> Result<()>) -> Result<Row> {
        // The arguments read the frame's variables and parameters as slots.
        let values = exec.frame.dynamic_values();
        let framed = Framed { inner: &exec.hosted, frame: values, outer: &[] };
        let at = RangeAt { st: exec.cx.st, tx: exec.cx.tx.id, values, subqueries: &framed };
        let mut folding = self.fold.begun(&at);
        visit(&mut |row| self.fold.feed(&mut folding, row, &at))?;
        folding.finished()
    }
}

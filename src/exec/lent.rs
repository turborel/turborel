//! \[DESIGN\] RULES.md D903: a kept plan whose rows are made of its slots'
//! values alone, read where it is kept.
//!
//! The executor takes a plan whole, and each operator it opens owns its
//! part, so every execution of a kept plan ran a copy of it. A projection
//! of a collection derived table over nothing before it, as the lines of
//! an order unnested from its arrays are, reads no table of its own. Its
//! rows are the elements of the collections its slots hold, each given to
//! the expressions of the projection. So it is run here from the plan as
//! it is kept, lent, with no copy made and no operator opened.
//!
//! The rows are made as the operators make them (`unnest.rs`, `exec.rs`):
//! the collections evaluated once, a row for each ordinal, each
//! expression evaluated over it in turn, its subqueries run by one runner,
//! a row's draws of a sequence generator one, a cancel looked for before
//! the rows are made and as each is given (RULES.md D44). Any other plan
//! is opened by the executor as before.

use super::{Db, Drawn, Runner, Subqueries, batch, batched, eval, unnest};
use crate::error::Result;
use crate::logical::{Expr, Unnest};
use crate::physical::Plan;
use crate::storage::{Row, sized_row};
use crate::txn::TxId;

/// A kept plan read where it is kept: a projection of a collection
/// derived table over one row of no column.
pub(crate) struct Lent<'p> {
    exprs: &'p [Expr],
    unnest: &'p Unnest<Plan>,
}

impl<'p> Lent<'p> {
    /// `plan`, if it is read so in `db`: a projection, of more than its
    /// input's columns as they are, of a collection derived table that is
    /// no JSON table and no GRAPH_TABLE, over one row of no column. None
    /// of a projection the executor reads by batches.
    pub(crate) fn of(plan: &'p Plan, db: &Db) -> Option<Lent<'p>> {
        let Plan::Project { exprs, input, .. } = plan else { return None };
        let Plan::Unnest(unnest) = &**input else { return None };
        let Plan::Values { rows, .. } = &unnest.input else { return None };
        let over_one = rows.len() == 1 && rows.iter().all(Vec::is_empty);
        let of_elements = unnest.json.is_none() && unnest.graph.is_none();
        let columns = exprs.iter().all(|e| matches!(e, Expr::Column(_)));
        if !over_one || !of_elements || columns || (batched(db) && batch::projectable(plan, db)) {
            return None;
        }
        Some(Lent { exprs, unnest })
    }

    /// Its rows, as transaction `tx` sees what their subqueries read, each
    /// given to `each` in order.
    ///
    /// # Errors
    ///
    /// As the plan's, run by the executor; `each`'s.
    pub(crate) fn rows(&self, db: &Db, tx: TxId, each: &mut dyn FnMut(Row) -> Result<()>) -> Result<()> {
        let runner = Runner::new(db, tx);
        runner.interrupted()?;
        let made = unnest::of_row(self.unnest, Row::new(), &runner)?;
        runner.interrupted()?;
        // [ISO/IEC 9075-2:2023 §4.29.1] a row's draws of one generator are one.
        let draws = self.exprs.iter().any(Expr::draws);
        for row in made {
            let given = if draws { self.projected(&row, &Drawn::new(&runner)) } else { self.projected(&row, &runner) };
            each(given?)?;
            runner.interrupted()?;
        }
        Ok(())
    }

    /// [X3.135-1986 §5.25 GR2] Each of its expressions applied to `row`.
    fn projected(&self, row: &Row, subqueries: &dyn Subqueries) -> Result<Row> {
        sized_row(self.exprs.len(), self.exprs.iter().map(|e| eval(e, row, subqueries)))
    }
}

#[cfg(test)]
mod tests;

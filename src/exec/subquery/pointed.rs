//! \[DESIGN\] RULES.md D900: a correlated subquery whose plan kept is one
//! probe of a whole unique key, its columns given as its table holds them,
//! run as a point read.
//!
//! A run of a plan kept copied the plan, filled the copy with the run's
//! outer values, opened the executor's operators over it and gathered the
//! one row they gave into rows that may be read again. A plan that is a
//! point read (`exec/point.rs`, RULES.md D752, D881) finds that row by
//! the probe placed with the values as they are, the plan lent and no
//! operator opened. It reads, records its read and counts its entries as
//! the plan's scan does, so the rows, the errors and the conflicts are
//! the plan's. Its rows are kept for its outer values as any run's are.
//!
//! A plan that is no point read, a run whose values its slots do not
//! take, a subquery still learning its plan and one planned at each run
//! are run as before (`planned.rs`).

use std::sync::Arc;

use super::super::AsIs;
use super::planned::{KeptRun, Planned};
use super::{Runner, SubqueryRows};
use crate::error::Result;
use crate::logical::Subquery;
use crate::settings::KEEP_SUBQUERY_PLANS;
use crate::value::Value;

impl Runner {
    /// The rows of a run of `subquery` for `outer`, at most `limit`, read
    /// by the point read its plan kept is; none if it keeps no plan, its
    /// plan is no point read or the values are not its slots', and it is
    /// run by `per_run`.
    ///
    /// # Errors
    ///
    /// As `PointRead::read`.
    pub(super) fn pointed(&self, subquery: &Subquery, outer: &[Value], limit: Option<usize>) -> Result<Option<SubqueryRows>> {
        if outer.is_empty() || !self.db.settings.is_on(KEEP_SUBQUERY_PLANS) {
            return Ok(None);
        }
        let Some(kept) = self.kept_run(subquery.id()).filter(|kept| kept.fits(outer)) else { return Ok(None) };
        let Some(point) = kept.point(&self.db) else { return Ok(None) };
        let row = match point.read_as_is(&self.db, self.tx, outer)? {
            AsIs::Read(row) => row,
            AsIs::Unplaced => return Ok(None),
        };
        let rows = row.into_iter().take(limit.unwrap_or(usize::MAX)).collect();
        Ok(Some(SubqueryRows::held(rows, &self.db)))
    }

    /// The plan the subquery `at` keeps, if it keeps one: as this runner
    /// settled it, or, of a kept plan's subquery it has not yet run, as an
    /// earlier execution did (RULES.md D793), noted as its own.
    fn kept_run(&self, at: u64) -> Option<Arc<KeptRun>> {
        match self.seen.borrow().get(&at).map(|seen| &seen.planned) {
            Some(Planned::Kept(kept)) => return Some(Arc::clone(kept)),
            Some(Planned::Learning(..) | Planned::Varies) => return None,
            Some(Planned::Untried) | None => {}
        }
        let Some(Planned::Kept(kept)) = self.db.subqueries().and_then(|k| k.get(at)) else { return None };
        self.seen.borrow_mut().entry(at).or_default().planned = Planned::Kept(Arc::clone(&kept));
        Some(kept)
    }
}

#[cfg(test)]
mod tests;

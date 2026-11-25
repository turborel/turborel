//! \[DESIGN\] (RULES.md D1129) A grouping that a filter's input and the
//! subqueries of its condition compute alike, computed once. A view or a
//! WITH query read in a statement and again in a subquery of it is
//! planned in each place, and each plan was run: the grouping of a table
//! read twice, grouped twice.
//!
//! As a filter is opened, each subquery of its condition that has no
//! outer reference is planned, as its run would plan it. The groupings by
//! hash of the filter's input and of those plans are then written out
//! (`shared/written.rs`). Two written alike are the same plan of the same
//! tables, so they give the same rows of one statement's reading. Each
//! place such a grouping stands is made a reader of one cell
//! (`Plan::Shared`). The reader that opens first runs the grouping and
//! keeps its rows, in memory within its work area and in a run of the
//! spill area past it (RULES.md D38). Every reader reads them from
//! there. The subqueries' plans go to the runner that runs them
//! (`Runner::ready`).
//!
//! Nothing is run that was not run before, nor sooner. A reader runs the
//! grouping as it is opened, where its plan ran it, and a place never
//! opened runs nothing. A grouping that raises an error raises it of
//! each reader that runs it, and keeps no rows.

mod written;

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use super::join::Rescanned;
use super::work::Work;
use super::{Db, RowIter, Runner, broken, execute, failed};
use crate::error::{Result, SqlError};
use crate::logical::{Cond, Expr, Subquery};
use crate::physical::{Plan, Shared};
use crate::txn::TxId;

/// \[DESIGN\] How deep a plan or a condition is walked (RULES.md R-CF-6):
/// deeper than any plan the binder makes nests.
const MAX_DEPTH: usize = 1024;

/// The rows of a shared subplan, once a reader ran it, held for every
/// place it is read.
#[derive(Clone, Default)]
pub struct SharedRows(Arc<Mutex<Option<Rescanned>>>);

impl fmt::Debug for SharedRows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SharedRows")
    }
}

/// A shared subplan's lock held by a thread that failed.
fn poisoned() -> SqlError {
    broken("a shared subplan's lock was poisoned")
}

impl SharedRows {
    /// Its rows from the first, if a reader ran its subplan.
    fn held(&self) -> Result<Option<RowIter>> {
        self.0.lock().map_err(|_| poisoned())?.as_mut().map(Rescanned::pass).transpose()
    }

    /// Its rows from the first, `rows` kept as them unless another
    /// reader's were kept meanwhile.
    fn kept(&self, rows: Rescanned) -> Result<RowIter> {
        self.0.lock().map_err(|_| poisoned())?.get_or_insert(rows).pass()
    }
}

/// The rows of `plan`, a place a shared subplan is read: those held, or,
/// of the first reader, those of its subplan, run now and kept. The
/// subplan runs with the cell's lock free, as its own operators take
/// theirs (RULES.md R-ERR-4).
#[inline(never)]
pub(super) fn shared(plan: Plan, db: &Db, tx: TxId) -> RowIter {
    let Plan::Shared(s) = plan else { return failed(broken("a shared subplan's read is not one")) };
    let Shared { input, rows } = *s;
    let read = || {
        if let Some(held) = rows.held()? {
            return Ok(held);
        }
        let (made, _) = Rescanned::of(execute(input, db, tx), &mut Work::of(db, tx))?;
        rows.kept(made)
    };
    read().unwrap_or_else(failed)
}

/// The input of a filter of `cond` and the runner of its subqueries, as
/// the filter is opened: `input` and a new runner, or, if a grouping is
/// computed alike in several places of the input and the subqueries of
/// no outer reference, the input with each a reader of one cell, and a
/// runner holding those subqueries' plans with theirs.
pub(super) fn readied(cond: &Cond, mut input: Plan, db: &Db, tx: TxId) -> (Plan, Runner) {
    let runner = Runner::new(db, tx);
    let mut subqueries = Vec::new();
    alone(cond, &mut subqueries);
    let mut places = HashMap::new();
    if subqueries.is_empty() || !noted(&mut input, &mut places, 0) {
        return (input, runner);
    }
    let mut plans: Vec<(u64, Plan)> = subqueries.iter().map(|s| (s.id(), runner.planned_ahead(s))).collect();
    for (_, plan) in &mut plans {
        noted(plan, &mut places, 0);
    }
    let cells: HashMap<String, SharedRows> =
        places.into_iter().filter(|(_, count)| *count > 1).map(|(key, _)| (key, SharedRows::default())).collect();
    if !cells.is_empty() {
        read_once(&mut input, &cells, 0);
        for (_, plan) in &mut plans {
            read_once(plan, &cells, 0);
        }
    }
    // Planned once: the runner runs these, shared or not.
    for (id, plan) in plans {
        runner.ready(id, plan);
    }
    (input, runner)
}

/// Adds to `out` the subqueries of `cond` with no outer reference: its
/// predicates' and its expressions', not those nested in them.
fn alone<'c>(cond: &'c Cond, out: &mut Vec<&'c Subquery>) {
    out.extend(cond.subqueries().into_iter().filter(|s| s.outer.is_empty()));
    cond.for_each_expr(&mut |e| scalar(e, out, 0));
}

/// Adds to `out` the scalar subqueries of `e` with no outer reference.
fn scalar<'c>(e: &'c Expr, out: &mut Vec<&'c Subquery>, depth: usize) {
    if let Expr::Subquery(s, _) = e
        && s.outer.is_empty()
    {
        out.push(s);
    }
    if depth < MAX_DEPTH {
        e.for_each_child(&mut |child| scalar(child, out, depth.saturating_add(1)));
    }
}

/// Counts in `places` each grouping of `plan` that can be shared, by
/// how it is written out, those within one another too; whether it found
/// any.
fn noted(plan: &mut Plan, places: &mut HashMap<String, usize>, depth: usize) -> bool {
    if depth >= MAX_DEPTH || matches!(plan, Plan::Shared(_)) {
        return false;
    }
    let deeper = depth.saturating_add(1);
    if let Some(key) = written::grouping(plan) {
        let count = places.entry(key).or_default();
        *count = count.saturating_add(1);
        // Below its marks and itself, which are written with it.
        if let Some(input) = written::below(plan, 0) {
            noted(input, places, deeper);
        }
        return true;
    }
    let mut found = false;
    plan.each_input(&mut |input| found |= noted(input, places, deeper));
    found
}

/// Makes each grouping of `plan` written as a key of `cells` a reader of
/// that cell, the outermost of those within one another.
fn read_once(plan: &mut Plan, cells: &HashMap<String, SharedRows>, depth: usize) {
    if depth >= MAX_DEPTH || matches!(plan, Plan::Shared(_)) {
        return;
    }
    if let Some(rows) = written::grouping(plan).and_then(|key| cells.get(&key)) {
        let input = std::mem::replace(plan, Plan::Values { rows: Vec::new(), detail: String::new() });
        *plan = Plan::Shared(Box::new(Shared { input, rows: rows.clone() }));
        return;
    }
    plan.each_input(&mut |input| read_once(input, cells, depth.saturating_add(1)));
}

// Unit tests: what is written alike, which places read once, and what a
// reader reads (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

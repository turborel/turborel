//! [ISO/IEC 9075-2:2023 §7.17 GR2c ii] The rows of a member of a
//! recursive stratum: its members' plans evaluated over their work tables
//! until a fixpoint is reached, then the member's result, all of it found
//! as the query opens, as a grouping's is.
//!
//! As the General Rules say (`Fixpoint::Whole`), each iteration evaluates
//! every member's plan again over every row the iteration before gave, until
//! none has more rows than it had. Of one member whose query is a UNION of a
//! non-recursive operand and a recursive one that distributes over UNION ALL
//! (`Fixpoint::Delta`), each iteration evaluates the recursive operand over
//! only the rows the one before added, which gives the same rows. UNION
//! DISTINCT keeps no row twice, as a bag of the rows' values says
//! (`exec/work.rs`).
//!
//! A work table is a list of held rows (`exec/held.rs`), spilled past
//! MEMORY.WORK_BYTES, which every plan that reads it shares. Each is read
//! only while the statement is not cancelled (RULES.md D44), and past
//! EXECUTOR.MAX_ITERATIONS iterations the query is refused (SQLCODE -918),
//! as one with no finite fixpoint never ends [NOTE 322].

use std::fmt;
use std::sync::{Arc, Mutex};

use super::work::{Bag, Work};
use super::{HeldRows, RowIter, broken, execute, failed};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::logical::Fixpoint;
use crate::physical::Plan;
use crate::settings::MAX_ITERATIONS;
use crate::spill::norm_row;
use crate::storage::Row;
use crate::txn::{TxId, checked};

/// \[DESIGN\] The default of EXECUTOR.MAX_ITERATIONS: the most iterations
/// a recursive query takes before it is refused.
pub const DEFAULT_MAX_ITERATIONS: usize = 1_000_000;

/// The rows of a work table: a member's after the last iteration, shared
/// by every plan that reads it.
#[derive(Clone)]
pub struct WorkRows(Arc<Mutex<HeldRows<Row>>>);

impl fmt::Debug for WorkRows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WorkRows")
    }
}

impl WorkRows {
    /// The work table of `rows`.
    fn new(rows: HeldRows<Row>) -> WorkRows {
        WorkRows(Arc::new(Mutex::new(rows)))
    }

    /// How many rows it has.
    fn len(&self) -> Result<usize> {
        Ok(self.0.lock().map_err(|_| poisoned())?.len())
    }

    /// Its rows, once no plan reads them.
    fn into_rows(self) -> Result<HeldRows<Row>> {
        let rows = Arc::try_unwrap(self.0).map_err(|_| broken("a work table is still read after its iteration"))?;
        rows.into_inner().map_err(|_| poisoned())
    }
}

/// A work table's lock held by a thread that failed.
fn poisoned() -> SqlError {
    broken("a work table's lock was poisoned")
}

/// The rows of `plan`, a recursive stratum's member, for the statement of
/// `tx`.
pub(super) fn recursive(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::Recursive { member, fixpoint } = plan else { return failed(broken("a recursive query is not one")) };
    let found = match fixpoint {
        Fixpoint::Whole(plans) => whole(member, &plans, db, tx),
        Fixpoint::Delta { anchor, step, distinct } => delta((*anchor, &step), distinct, db, tx),
    };
    match found {
        Ok(rows) => Box::new(rows.into_iter()),
        Err(e) => failed(e),
    }
}

/// The rows of the work table `plan`, each read while the statement of
/// `tx` is not cancelled.
pub(super) fn work_table(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::WorkTable { rows: Some(rows), .. } = plan else { return failed(broken("a work table is read outside its recursive query")) };
    let mut place = 0_usize;
    let read = std::iter::from_fn(move || {
        let row = rows.0.lock().map_err(|_| poisoned()).and_then(|mut held| held.get(place));
        place = place.saturating_add(1);
        row.transpose()
    });
    Box::new(checked(read, db.tx.stop_of(tx)))
}

/// [ISO/IEC 9075-2:2023 §7.17 GR2c ii 3, 4] Member `member`'s rows of the
/// members' `plans`, as the General Rules find them.
fn whole(member: usize, plans: &[Plan], db: &Arc<State>, tx: TxId) -> Result<HeldRows<Row>> {
    let limit = db.settings.count(MAX_ITERATIONS);
    let mut work: Vec<WorkRows> = plans.iter().map(|_| WorkRows::new(HeldRows::new(db.list_space()))).collect();
    let mut results = evaluated(plans, &work, db, tx)?;
    let mut iterations = 0_usize;
    while grew(&results, &work)? {
        iterations = iterations.saturating_add(1);
        within(iterations, limit)?;
        work = results.into_iter().map(WorkRows::new).collect();
        results = evaluated(plans, &work, db, tx)?;
    }
    results.into_iter().nth(member).ok_or_else(|| broken("a recursive query has no such member"))
}

/// Whether some member's `results` has more rows than its `work` table.
fn grew(results: &[HeldRows<Row>], work: &[WorkRows]) -> Result<bool> {
    for (result, table) in results.iter().zip(work) {
        if result.len() > table.len()? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The rows of each of `plans`, each reading the work tables `work`.
fn evaluated(plans: &[Plan], work: &[WorkRows], db: &Arc<State>, tx: TxId) -> Result<Vec<HeldRows<Row>>> {
    plans.iter().map(|plan| added(execute(given(plan.clone(), work), db, tx), &mut None, db)).collect()
}

/// The rows of the member whose query is the UNION of `anchor` and `step`,
/// `distinct` if UNION DISTINCT: `anchor`'s, then `step`'s over the rows
/// last added, until it adds none.
fn delta((anchor, step): (Plan, &Plan), distinct: bool, db: &Arc<State>, tx: TxId) -> Result<HeldRows<Row>> {
    let limit = db.settings.count(MAX_ITERATIONS);
    let mut seen = distinct.then(|| Bag::new(Work::of(db, tx)));
    let mut all = HeldRows::new(db.list_space());
    let mut new = added(execute(anchor, db, tx), &mut seen, db)?;
    let mut iterations = 0_usize;
    while !new.is_empty() {
        iterations = iterations.saturating_add(1);
        within(iterations, limit)?;
        let last = WorkRows::new(new);
        new = added(execute(given(step.clone(), std::slice::from_ref(&last)), db, tx), &mut seen, db)?;
        // Each row moved, not the list appended: its rows then join the
        // segments `all` holds, which a list appended a segment an
        // iteration would not.
        for row in last.into_rows()? {
            all.push(row?)?;
        }
    }
    Ok(all)
}

/// The rows of `rows`, held: of those `seen` is given, each not added
/// before alone, and added to it.
fn added(rows: RowIter, seen: &mut Option<Bag>, db: &State) -> Result<HeldRows<Row>> {
    let mut out = HeldRows::new(db.list_space());
    for row in rows {
        let row = row?;
        if let Some(bag) = seen.as_mut()
            && !bag.add_new(&norm_row(&row))?
        {
            continue;
        }
        out.push(row)?;
    }
    Ok(out)
}

/// \[DESIGN\] Refuses the `iterations`th iteration past `limit`, unless it
/// is 0, for none.
fn within(iterations: usize, limit: usize) -> Result<()> {
    if limit > 0 && iterations > limit {
        return Err(SqlError::new(
            Code::Resources,
            format!(
                "[ISO/IEC 9075-2:2023 §7.17 GR2c ii 4] a recursive query reached no fixpoint in {limit} iterations, as EXECUTOR.MAX_ITERATIONS allows"
            ),
        ));
    }
    Ok(())
}

/// `plan`, each work table of its stratum given the rows of its member's
/// of `work`: not those of a recursive query in it, of a stratum of its own.
fn given(mut plan: Plan, work: &[WorkRows]) -> Plan {
    give(&mut plan, work);
    plan
}

/// `given`, in place.
fn give(plan: &mut Plan, work: &[WorkRows]) {
    match plan {
        Plan::WorkTable { member, rows } => *rows = work.get(*member).cloned(),
        Plan::Recursive { .. } | Plan::SeqScan { .. } | Plan::IndexScan { .. } | Plan::System { .. } | Plan::Values { .. } => {}
        Plan::Filter { input, .. }
        | Plan::IndexJoin { left: input, .. }
        | Plan::Aggregate { input, .. }
        | Plan::Project { input, .. }
        | Plan::Distinct { input }
        | Plan::Sort { input, .. } => give(input, work),
        Plan::Unnest(u) => give(&mut u.input, work),
        Plan::Window { window, .. } => give(&mut window.input, work),
        Plan::Product { left, right }
        | Plan::HashJoin { left, right, .. }
        | Plan::OuterJoin { left, right, .. }
        | Plan::SetOp { left, right, .. }
        | Plan::Append { left, right } => {
            give(left, work);
            give(right, work);
        }
    }
}

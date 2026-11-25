//! \[DESIGN\] (RULES.md D389, D1063) Parts run in parallel read by batch
//! by an operator that is not of them. A pipeline's morsels give their
//! batches in order (`parallel/batches.rs`). A grouping by hash run in
//! parallel that takes filters (`batch::filterable`) is read by batch by
//! a hash join, which tells it its runtime filters, as the join reads
//! the grouping run alone (`told.rs`, RULES.md D600): the part's pipeline
//! is told the filters of its grouping columns, and each morsel judges
//! them of its batches, so a group the join's other operand has no key
//! for is never made.

use super::joined::gathered;
use super::late::Deferred;
use super::stream::{self, Batches};
use super::{Taken, read_of, runtime, told, typed};
use crate::db::State;
use crate::error::Result;
use crate::exec::parallel::Held;
use crate::exec::{Db, broken};
use crate::physical::{Counted, Parallel, Plan};
use crate::txn::TxId;

/// Whether `plan` is a grouping run in parallel that takes filters, which
/// a hash join reads by batch to tell it its filters.
pub(super) fn grouping(plan: &Plan, db: &State) -> bool {
    grouping_of(plan).is_some_and(|grouping| told::layout(grouping, db).is_some())
}

/// The grouping by hash `plan`, a part run in parallel, is of, if it is
/// of one.
pub(super) fn grouping_of(plan: &Plan) -> Option<&Plan> {
    let Plan::Parallel(p) = plan else { return None };
    grouping_in(&p.input)
}

/// The grouping by hash `input`, what a part runs in parallel, is, if it
/// is one, a DISTINCT by hash among them: the part's operator, or the
/// operator an analyzed plan counts there.
pub(super) fn grouping_in(input: &Plan) -> Option<&Plan> {
    let top = if let Plan::Counted(c) = input { &c.input } else { input };
    matches!(top, Plan::Aggregate { streamed: false, .. } | Plan::Distinct { streamed: false, .. }).then_some(top)
}

/// The batches of `plan`, a part run in parallel: a grouping's groups
/// gathered (`grouped_batches`), or a pipeline's batches in order
/// (`parallel/batches.rs`); of a pipeline that gives way, those of its
/// plan alone.
///
/// # Errors
///
/// As `stream::open`.
pub(super) fn batches(plan: Plan, (db, tx): (&Db, TxId), deferred: &Deferred) -> Result<Batches> {
    let grouped = grouping(&plan, db);
    let Plan::Parallel(p) = plan else { return Err(broken("a part run in parallel is not one")) };
    if grouped {
        return grouped_batches(*p, (db, tx), deferred);
    }
    // A window's whole partitions computed on the part's threads (D560).
    if matches!(p.input, Plan::Window { .. }) {
        return crate::exec::window::window_batches_on(p.input, (db, tx), p.degree);
    }
    // A part whose batches hold no layout of its own is read as its plan.
    let Some(shape) = stream::layout(&p.input, db) else { return stream::batches_with(p.input, (db, tx), deferred) };
    match crate::exec::parallel::streamed_batches(&p.input, (db, tx), p.degree, &shape)? {
        Held::Kept(batches) => Ok(batches),
        Held::GaveWay => stream::batches_with(p.input, (db, tx), deferred),
    }
}

/// The batches of `part`, a grouping run in parallel that takes filters
/// (`grouping`), its groups gathered: its pipeline told the filters of
/// its grouping columns `deferred` tells it, each morsel judging them of
/// its batches, if no set function takes a column that may hold a null,
/// whose warning a row dropped would lose [ISO/IEC 9075-2:2023 §10.9
/// GR4b]; else told none, and every row grouped. A part that gives way is
/// grouped alone (`told::grouped_batches`).
///
/// # Errors
///
/// As its input's; SQLCODE -910 of a part not such a grouping.
fn grouped_batches(part: Parallel, (db, tx): (&Db, TxId), deferred: &Deferred) -> Result<Batches> {
    let Parallel { degree, input } = part;
    // An analyzed plan's count of the grouping's rows, taken of the part's
    // (RULES.md D378).
    if let Plan::Counted(c) = input {
        let Counted { count, input: counted } = *c;
        let run = Plan::Parallel(Box::new(Parallel { degree, input: counted }));
        return stream::batches_with(Plan::Counted(Box::new(Counted { count, input: run })), (db, tx), deferred);
    }
    let input = told::grouping_of(input, db);
    let (columns, _) = told::layout(&input, db).ok_or_else(|| broken("a grouping that gives no batches"))?;
    let (Some(read), Plan::Aggregate { input: rows, .. }) = (read_of(&input, db), &input) else {
        return Err(broken("a grouping that gives no batches"));
    };
    let (input_columns, _) = stream::layout(rows, db).unwrap_or_default();
    let nullable = |at: &usize| input_columns.get(*at).is_none_or(|&c| !typed::never_null(rows, c, db));
    let warns = |t: &Taken| if let Taken::Column(at) = t { nullable(at) } else { false };
    let below = if read.taken.iter().any(warns) { Vec::new() } else { runtime::taken(&input, deferred.filters(), db).0 };
    let below = below.into_iter().map(runtime::Told::from_above).collect();
    match crate::exec::parallel::grouped_told(&input, (db, tx), degree, below)? {
        Held::Kept(groups) => Ok(gathered(groups, &columns)),
        Held::GaveWay => told::grouped_batches(input, (db, tx), deferred),
    }
}

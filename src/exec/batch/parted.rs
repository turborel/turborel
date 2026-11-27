//! \[DESIGN\] (RULES.md D389, D1063) Parts run in parallel read by batch
//! by an operator that is not of them. A pipeline's morsels give their
//! batches in order (`parallel/batches.rs`). A grouping by hash run in
//! parallel that takes filters (`batch::filterable`) is read by batch by
//! a hash join, which tells it its runtime filters, as the join reads
//! the grouping run alone (`told.rs`, RULES.md D600): the part's pipeline
//! is told the filters of its grouping columns, and each morsel judges
//! them of its batches, so a group the join's other operand has no key
//! for is never made.
//!
//! \[DESIGN\] (RULES.md D1221) A join that finds its built operand's rows
//! by their keys alone, and whose keys hold every grouping column of a
//! grouping it builds of (`keyed_whole`), finds at most one group of a
//! key, so the order the groups are built in shows in none of its rows.
//! It asks for them in any order (`Deferred::in_any_order`), and the
//! grouping hands over its partitions' batches as they are, none merged
//! with another's.

use super::late::Deferred;
use super::stream::{self, Batches};
use super::{Taken, read_of, runtime, told, typed};
use crate::db::State;
use crate::error::Result;
use crate::exec::parallel::Held;
use crate::exec::{Db, broken};
use crate::logical::Expr;
use crate::physical::{Counted, JoinKey, Parallel, Plan};
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

/// \[DESIGN\] (RULES.md D1221) Whether `plan`, a join's right operand,
/// is a grouping run in parallel no two of whose groups one key of `keys`
/// finds, or projections over one, which give a row of each group in the
/// order they are given it. Every grouping column must be a key's right
/// column, as a projection gives it, and each must be grouped by the
/// default collation, under which two strings are one group where the
/// join finds them equal [X3.135-1986 §5.11 GR5]. A column grouped by
/// another collation may make two groups of strings the join matches
/// with one key, so such a grouping is not one of these, and neither is
/// a DISTINCT.
pub(super) fn keyed_whole(plan: &Plan, keys: &[JoinKey]) -> bool {
    let (part, keyed) = under(plan, keys.iter().map(|k| k.right).collect());
    let Some(Plan::Aggregate { groups, collations, .. }) = grouping_of(part) else { return false };
    collations.iter().all(Option::is_none) && (0..groups.len()).all(|column| keyed.contains(&column))
}

/// The operator under the projections and an analyzed plan's counts
/// over `plan`, and of the columns `columns` of `plan`'s rows those a
/// projection names of that operator's, as it numbers them.
fn under(plan: &Plan, mut columns: Vec<usize>) -> (&Plan, Vec<usize>) {
    let mut part = plan;
    loop {
        if let Plan::Counted(c) = part {
            part = &c.input;
        } else if let Plan::Project { exprs, input, .. } = part {
            let named = |c: &usize| if let Some(Expr::Column(of)) = exprs.get(*c) { Some(*of) } else { None };
            columns = columns.iter().filter_map(named).collect();
            part = input;
        } else {
            return (part, columns);
        }
    }
}

/// \[DESIGN\] (RULES.md D1221) Whether a join reads `plan`, the operand
/// it builds of, by batch. It does where every table `plan` scans reads
/// by batch (`stream::reads_by_batch`), and where `plan` is a grouping by
/// hash run in parallel, alone or under projections. Such a grouping's
/// partitions give their groups as batches whatever its tables are read
/// by (RULES.md D1125), so a row made of each group would only be put
/// back into a batch for the join's table.
pub(in crate::exec) fn built_by_batch(plan: &Plan, db: &State) -> bool {
    stream::reads_by_batch(plan, db) || matches!(grouping_of(under(plan, Vec::new()).0), Some(Plan::Aggregate { .. }))
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
    // A grouping that takes no filter, alone or under its HAVING
    // condition, its rows gathered (RULES.md D1072).
    if told::part_layout(&p.input).is_some() {
        return told::part_batches(*p, (db, tx), !deferred.any_order());
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
/// GR4b]; else told none, and every row grouped. Its groups come in the
/// order of their first rows unless `deferred` asks for them in any
/// (RULES.md D1221). A part that gives way is grouped alone
/// (`told::grouped_batches`).
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
    let below: Vec<_> = below.into_iter().map(runtime::Told::given_above).collect();
    // Its partitions' groups as batches, no row made between (RULES.md D1220).
    match crate::exec::parallel::grouped_batches(&input, (db, tx, degree), &below, (&columns, !deferred.any_order()))? {
        Held::Kept(batches) => Ok(batches),
        Held::GaveWay => told::grouped_batches(input, (db, tx), deferred),
    }
}

// Unit tests: which groupings a join's keys find one group of a key in
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::physical::NodeCount;
    use crate::value::collation::{CollationRef, unpadded_scalar};
    use std::sync::Arc;

    /// A grouping by hash of the first `groups` columns of no rows, each
    /// grouped as `collations` says.
    fn grouping(groups: usize, collations: Vec<Option<CollationRef>>) -> Plan {
        Plan::Aggregate {
            groups: (0..groups).collect(),
            collations,
            aggs: Vec::new(),
            seeds: Vec::new(),
            detail: String::new(),
            input: Box::new(Plan::Values { rows: Vec::new(), detail: String::new() }),
            est: 0.0_f64,
            streamed: false,
            first: None,
        }
    }

    /// `input` as a part run in parallel.
    fn part(input: Plan) -> Plan {
        Plan::Parallel(Box::new(Parallel { degree: 4, input }))
    }

    /// Verifies: [DESIGN] (RULES.md D1221) a join's keys find at most one
    /// group of a grouping run in parallel when every grouping column is
    /// a key's right column, whatever else is a key and in any order, and
    /// under an analyzed plan's counts; not when a grouping column is no
    /// key, when a column is grouped by a collation of its own, of a
    /// grouping not run in parallel, of a DISTINCT, or of a part that is
    /// no grouping.
    #[test]
    fn keys_holding_every_grouping_column_find_one_group() {
        let key = |right: usize| JoinKey { left: 0, right, nulls: false };
        let plain = || part(grouping(2, vec![None, None]));
        assert!(keyed_whole(&plain(), &[key(0), key(1)]));
        assert!(keyed_whole(&plain(), &[key(2), key(1), key(0)]), "a set function's column a key too");
        assert!(keyed_whole(&part(grouping(1, Vec::new())), &[key(0)]), "no collation named");
        let counted = |input: Plan| Plan::Counted(Box::new(Counted { count: Arc::new(NodeCount::default()), input }));
        assert!(keyed_whole(&counted(part(counted(grouping(1, Vec::new())))), &[key(0)]), "counted above and within");
        assert!(!keyed_whole(&plain(), &[key(0)]), "a grouping column that is no key");
        assert!(!keyed_whole(&plain(), &[key(1), key(2)]));
        assert!(!keyed_whole(&part(grouping(1, vec![Some(unpadded_scalar())])), &[key(0)]), "grouped by a collation of its own");
        assert!(!keyed_whole(&grouping(1, Vec::new()), &[key(0)]), "not run in parallel");
        let values = || Plan::Values { rows: Vec::new(), detail: String::new() };
        assert!(!keyed_whole(&part(Plan::Distinct { input: Box::new(values()), est: 0.0_f64, streamed: false }), &[key(0)]));
        assert!(!keyed_whole(&part(values()), &[key(0)]), "a part that is no grouping");
        // A projection gives the groups' rows in their order, a key's
        // column the grouping's it names.
        let project = |exprs: Vec<Expr>, input: Plan| Plan::Project { exprs, detail: String::new(), input: Box::new(input) };
        let swapped = || project(vec![Expr::Column(2), Expr::Column(1), Expr::Column(0)], plain());
        assert!(keyed_whole(&swapped(), &[key(2), key(1)]), "under a projection, by the columns it names");
        assert!(keyed_whole(&counted(project(vec![Expr::Column(2), Expr::Column(1)], swapped())), &[key(1), key(0)]), "under two");
        assert!(!keyed_whole(&swapped(), &[key(0), key(1)]), "a key of a set function's column and one grouping column");
        let made = project(vec![Expr::Neg(Box::new(Expr::Column(0))), Expr::Column(1)], plain());
        assert!(!keyed_whole(&made, &[key(0), key(1)]), "a key of a value the projection computes");
    }

    /// Verifies: [DESIGN] (RULES.md D1221) a join reads the operand it
    /// builds of by batch where it is a grouping by hash run in parallel,
    /// alone, under projections or under an analyzed plan's counts,
    /// whatever its tables are read by; not a grouping run alone, a
    /// DISTINCT, a grouping under its HAVING condition, nor a part that
    /// is no grouping, none of whose tables read by batch.
    #[test]
    fn a_grouping_run_in_parallel_is_built_of_by_batch() {
        let st = crate::exec::tests::state();
        let values = || Plan::Values { rows: Vec::new(), detail: String::new() };
        let counted = |input: Plan| Plan::Counted(Box::new(Counted { count: Arc::new(NodeCount::default()), input }));
        let project = |input: Plan| Plan::Project {
            exprs: vec![Expr::Neg(Box::new(Expr::Column(0)))],
            detail: String::new(),
            input: Box::new(input),
        };
        assert!(built_by_batch(&part(grouping(1, Vec::new())), &st));
        assert!(built_by_batch(&counted(project(part(counted(grouping(2, vec![None, Some(unpadded_scalar())]))))), &st));
        assert!(!built_by_batch(&grouping(1, Vec::new()), &st), "a grouping run alone");
        assert!(!built_by_batch(&part(Plan::Distinct { input: Box::new(values()), est: 0.0_f64, streamed: false }), &st));
        let having = Plan::Filter {
            cond: crate::logical::Cond::IsNull(Expr::Column(0), false),
            detail: String::new(),
            input: Box::new(grouping(1, Vec::new())),
            est: 0.0_f64,
            of_read: false,
        };
        assert!(!built_by_batch(&part(having), &st), "under its HAVING condition");
        assert!(!built_by_batch(&part(values()), &st), "a part that is no grouping");
    }
}

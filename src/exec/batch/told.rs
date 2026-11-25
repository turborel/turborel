//! \[DESIGN\] (RULES.md D600) Operators run row by row whose rows a stream
//! gathers into batches, so a hash join over them runs by batch and its
//! runtime filters reach below them. An index scan judges a filter of
//! the rows it finds, its probe of every entry bounded by the filter's
//! range where its index's key begins with the filter's column, a number
//! (`bounded`). An inner index join, not spatial, takes a filter of a
//! column of its left operand to it, so a left row the filter drops
//! probes nothing. An inner merge join takes a filter of either
//! operand's column to that operand, and of its key to the other
//! operand's key too. A grouping by batch whose set functions raise
//! nothing takes a filter of a grouping column to its input, as a
//! group the filter drops gives no row the join keeps (Yang et al.,
//! "Predicate Transfer", CIDR 2024, §3): COUNT, MIN and MAX, and SUM and
//! AVG of exact values whose sum the declared types keep within an exact
//! number's digits (RULES.md D1060, D1062). Any other SUM or AVG can
//! overflow, so a grouping of one keeps every row (RULES.md D15). A set
//! function of a column that may hold a null warns of one it eliminates
//! [ISO/IEC 9075-2:2023 §10.9 GR4b], so a grouping of one judges the
//! filter of its input's batches itself and warns if a row it dropped
//! held a null there, as it would of every row
//! (`runtime::applied_warning`). Each join passes a filter only if the
//! rest of its condition cannot raise (`runtime::passed`).
//!
//! (RULES.md D1072) A grouping that takes no filter is gathered too, and
//! one run in parallel, alone or under its HAVING condition, so a join
//! over it runs by batch. No filter told it reaches its input.

use super::joined::{gathered, rows_of};
use super::late::Deferred;
use super::runtime::{self, Told};
use super::stream::{self, Batches};
use super::{Taken, filterable, read_of, typed};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::exec::join::{Reads, indexed_over, merged_over};
use crate::exec::{Db, broken, execute, failed};
use crate::index::{BTREE_METHOD, Probe};
use crate::physical::{Parallel, Plan, Preserved};
use crate::txn::TxId;
use crate::value::{CompOp, DataType};

/// The columns of `plan`'s rows its batches hold, and their width, of an
/// operator run row by row a stream gathers (`stream::layout`): an index
/// scan, an inner index join and an inner merge join of streams, and a
/// grouping that takes filters (`batch::filterable`).
pub(super) fn layout(plan: &Plan, db: &State) -> Option<(Vec<usize>, usize)> {
    if let Plan::IndexScan { table, columns, located: false, .. } = plan {
        return Some((columns.clone(), db.catalog.table(table)?.columns.len()));
    }
    if let Plan::IndexJoin { outer: Preserved::Inner, spatial: None, located: false, left, width, columns, .. } = plan {
        let (_, left_width) = stream::layout(left, db)?;
        let right = columns.iter().map(|c| c.saturating_add(left_width));
        return Some(((0..left_width).chain(right).collect(), left_width.saturating_add(*width)));
    }
    if let Plan::MergeJoin(m) = plan
        && !m.outer
    {
        stream::layout(&m.left, db)?;
        stream::layout(&m.right, db)?;
        let width = m.widths.0.saturating_add(m.widths.1);
        return Some(((0..width).collect(), width));
    }
    // A DISTINCT by hash is the grouping of its rows by every column
    // (`batch::distinct_grouping`, RULES.md D1066).
    if let Plan::Distinct { streamed: false, .. } = plan {
        return super::distinct_groupable(plan, db).map(|width| ((0..width).collect(), width));
    }
    let Plan::Aggregate { groups, aggs, streamed: false, .. } = plan else { return None };
    let width = groups.len().saturating_add(aggs.len());
    filterable(plan, db).then(|| ((0..width).collect(), width))
}

/// \[DESIGN\] (RULES.md D1072) The layout of `plan` as a stream gathers
/// it: one that takes filters (`layout`), or a grouping not streamed that
/// takes none, as one of SUM does, every column of its rows. So a join
/// over a grouping runs by batch whatever its set functions, and tells
/// its other operand's scan its filters.
pub(super) fn gathered_layout(plan: &Plan, db: &State) -> Option<(Vec<usize>, usize)> {
    layout(plan, db).or_else(|| grouping_layout(plan))
}

/// Every column of the rows of `plan`, a grouping not streamed, and their
/// width.
fn grouping_layout(plan: &Plan) -> Option<(Vec<usize>, usize)> {
    let Plan::Aggregate { groups, aggs, streamed: false, .. } = plan else { return None };
    let width = groups.len().saturating_add(aggs.len());
    Some(((0..width).collect(), width))
}

/// \[DESIGN\] (RULES.md D1072) The layout of `part`, a part run in
/// parallel that is no pipeline, if it is a grouping, or a filter over
/// one, its HAVING condition (RULES.md D1073): every column of its rows,
/// which its threads make and a stream gathers.
pub(super) fn part_layout(part: &Plan) -> Option<(Vec<usize>, usize)> {
    grouping_layout(if let Plan::Filter { input, .. } = part { input } else { part })
}

/// The batches of `part`, a part run in parallel that is a grouping, or
/// a filter over one (`part_layout`): its rows, made by the part's
/// threads, gathered.
///
/// # Errors
///
/// As the part's rows; SQLCODE -910 of a part that gives no batches.
pub(super) fn part_batches(part: Parallel, (db, tx): (&Db, TxId)) -> Result<Batches> {
    let (columns, _) = part_layout(&part.input).ok_or_else(|| broken("a part run in parallel that gives no batches"))?;
    Ok(gathered(execute(Plan::Parallel(Box::new(part)), db, tx), &columns))
}

/// \[DESIGN\] (RULES.md D1129) The batches of `plan`, a subplan computed
/// once for its statement: its rows, read from where they are held or
/// made by this reader (`exec/shared.rs`), gathered. It is told no
/// filter, as every reader is given all its rows.
///
/// # Errors
///
/// SQLCODE -910 of a plan whose batches hold no layout.
pub(super) fn shared_batches(plan: Plan, (db, tx): (&Db, TxId), _deferred: &Deferred) -> Result<Batches> {
    let (columns, _) = stream::layout(&plan, db).ok_or_else(|| broken("a shared subplan that gives no batches"))?;
    Ok(gathered(execute(plan, db, tx), &columns))
}

/// Whether `plan`, an operator run row by row whose rows a stream
/// gathers, reads by batch as an operand of an inner hash join: an index
/// scan does, and the others if their operands do (`stream::reads`).
pub(super) fn reads(plan: &Plan, db: &State) -> bool {
    if layout(plan, db).is_none() {
        return false;
    }
    if let Plan::IndexJoin { left: input, .. } | Plan::Aggregate { input, .. } | Plan::Distinct { input, .. } = plan {
        return stream::reads(input, db, true);
    }
    if let Plan::MergeJoin(m) = plan {
        return stream::reads(&m.left, db, true) && stream::reads(&m.right, db, true);
    }
    true
}

/// The batches of the index scan `plan`, its rows found gathered, each
/// filter `deferred` tells it judged of them, its probe bounded by them
/// (`bounded`).
///
/// # Errors
///
/// SQLCODE -910 of a plan not of an index scan that gives batches.
pub(super) fn index_batches(plan: Plan, (db, tx): (&Db, TxId), deferred: &Deferred) -> Result<Batches> {
    let (columns, _) = layout(&plan, db).ok_or_else(|| broken("an index scan that gives no batches"))?;
    let told = deferred.filters().to_vec();
    let plan = told.iter().fold(plan, |plan, t| bounded(plan, t, db));
    Ok(runtime::applied(gathered(execute(plan, db, tx), &columns), told, &columns, (db, false)))
}

/// The index scan `plan`, its probe of every entry of a B-tree index
/// whose key begins with `told`'s column, an exact number, bounded by
/// the filter's range. A row out of it, or of a null key, the filter
/// drops.
fn bounded(mut plan: Plan, told: &Told, db: &State) -> Plan {
    if let Plan::IndexScan { table, index, probe, .. } = &mut plan
        && matches!(probe, Probe::All)
        && keyed_by(db, (table, index), told.column)
        && let Some((low, high)) = told.filter.bounds()
    {
        *probe = Probe::Range { equal: Vec::new(), low: Box::new((CompOp::Ge, low)), high: Box::new((CompOp::Le, high)) };
    }
    plan
}

/// Whether `index`, of `table`, is a B-tree whose key begins with the
/// table's column `column`, of an exact numeric type.
fn keyed_by(db: &State, (table, index): (&QualifiedName, &str), column: usize) -> bool {
    let Some(ix) = db.catalog.index(index) else { return false };
    let exact = db.catalog.table(table).and_then(|t| t.columns.get(column)).is_some_and(|c| matches!(c.data_type, DataType::Exact { .. }));
    ix.method == BTREE_METHOD && ix.expressions.is_empty() && ix.columns.first() == Some(&column) && exact
}

/// The batches of the inner index join `plan`, its rows gathered, its
/// left operand's scans told the filters of its columns `deferred` tells
/// it.
///
/// # Errors
///
/// SQLCODE -910 of a plan not of an index join that gives batches.
pub(super) fn index_join_batches(plan: Plan, (db, tx): (&Db, TxId), deferred: &Deferred) -> Result<Batches> {
    let (columns, _) = layout(&plan, db).ok_or_else(|| broken("an index join that gives no batches"))?;
    let left = told_rows(runtime::taken(&plan, deferred.filters(), db).0, (db, tx));
    Ok(gathered(indexed_over(plan, left, db, tx), &columns))
}

/// The batches of the inner merge join `plan`, its rows gathered, each
/// operand's scans told the filters of its columns `deferred` tells it
/// (`runtime::taken`, `runtime::built_side`).
///
/// # Errors
///
/// SQLCODE -910 of a plan not of a merge join that gives batches.
pub(super) fn merge_batches(plan: Plan, (db, tx): (&Db, TxId), deferred: &Deferred) -> Result<Batches> {
    let (columns, _) = layout(&plan, db).ok_or_else(|| broken("a merge join that gives no batches"))?;
    let (left, right) = (runtime::taken(&plan, deferred.filters(), db).0, runtime::built_side(&plan, deferred.filters(), db));
    Ok(gathered(merged_over(plan, (told_rows(left, (db, tx)), told_rows(right, (db, tx))), db, tx), &columns))
}

/// The batches of the grouping `plan`: of one that takes no filter, its
/// rows gathered; of one that takes filters
/// (`batch::filterable`), its groups gathered, its input told the
/// filters of its grouping columns `deferred` tells it: its scans, if no
/// set function takes a column that may hold a null; else its batches,
/// warning of a null a set function would have eliminated of a row a
/// filter dropped.
///
/// # Errors
///
/// As its input's; SQLCODE -910 of a plan not of such a grouping.
pub(super) fn grouped_batches(plan: Plan, (db, tx): (&Db, TxId), deferred: &Deferred) -> Result<Batches> {
    let plan = grouping_of(plan, db);
    // One that takes no filter gives its rows, gathered (RULES.md D1072).
    if layout(&plan, db).is_none() {
        let (columns, _) = grouping_layout(&plan).ok_or_else(|| broken("a grouping that gives no batches"))?;
        return Ok(gathered(execute(plan, db, tx), &columns));
    }
    let (columns, _) = layout(&plan, db).ok_or_else(|| broken("a grouping that gives no batches"))?;
    let below = runtime::taken(&plan, deferred.filters(), db).0;
    let (Some(read), Plan::Aggregate { aggs, input, .. }) = (read_of(&plan, db), plan) else {
        return Err(broken("a grouping that gives no batches"));
    };
    let (input_columns, _) = stream::layout(&input, db).unwrap_or_default();
    // A column declared to hold no null has none to eliminate, so no
    // warning to keep (RULES.md D1062).
    let nullable = |at: &usize| input_columns.get(*at).is_none_or(|&c| !typed::never_null(&input, c, db));
    let taken = read.taken.iter().filter_map(|t| if let Taken::Column(at) = t { Some(*at) } else { None });
    let args: Vec<usize> = taken.filter(nullable).collect();
    let batches = if args.is_empty() {
        stream::whole_told(*input, (db, tx), below)?
    } else {
        let whole = stream::whole_told(*input, (db, tx), Vec::new())?;
        runtime::applied_warning(whole, below, (&input_columns, args), (db, tx))
    };
    Ok(gathered(super::groups::grouped_all(&read, &aggs, (db, tx), batches)?, &columns))
}

/// \[DESIGN\] (RULES.md D1066) `plan`, a DISTINCT by hash made the
/// grouping of its rows by every column it is; any other plan as it is.
pub(super) fn grouping_of(plan: Plan, db: &State) -> Plan {
    match super::distinct_groupable(&plan, db).filter(|_| matches!(plan, Plan::Distinct { streamed: false, .. })) {
        Some(width) => super::distinct_grouping(plan, width),
        None => plan,
    }
}

/// What reads an operand's rows: of its batches, its scans told `told`;
/// row by row, if it is told none and some table of it does not read by
/// batch, as a table that makes each row of a batch gives them as fast so.
fn told_rows(told: Vec<Told>, (db, tx): (&Db, TxId)) -> Reads {
    let db = db.clone();
    Box::new(move |plan: Plan| {
        if told.is_empty() && !stream::reads_by_batch(&plan, &db) {
            return execute(plan, &db, tx);
        }
        let (columns, width) = stream::layout(&plan, &db).unwrap_or_default();
        match stream::whole_told(plan, (&db, tx), told) {
            Ok(batches) => rows_of(batches, columns, width),
            Err(e) => failed(e),
        }
    })
}

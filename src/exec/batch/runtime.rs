//! \[DESIGN\] (RULES.md D369) Where a hash join's runtime filters judge
//! the rows of its left operand. A join told its filters to its left
//! operand's stream, each of a column of its rows; each operator takes
//! them down to its scan through those that pass a column on as it is and
//! raise no error: a filter whose condition cannot raise and reads no
//! subquery, a projection of columns, the column it passes on mapped to
//! the one it is of, if none of its expressions can raise, and an inner
//! hash join, to both its operands (RULES.md D600): its left of the
//! left's columns, or of the left key a right key equals, and its right,
//! as it builds it, of the right's columns, or of the right key a left
//! key equals. A filter moved across an equality judges exact numbers
//! alone, whose equality is their value's. A part hoisted under its
//! guard (RULES.md D373) is such a projection over a product, so a
//! filter never reaches the operand the guard asks has a row. Any other
//! operator, as an aggregation, an outer join, a LIMIT or one that can
//! raise, judges them of the batches it gives, right above it, as a row
//! below it a filter dropped could have raised an error row by row
//! (RULES.md D15). An analyzed plan's count of an operator's rows passes
//! them on, so it counts the rows the operator gives as it runs. A scan
//! in no order is told a filter's range as conjuncts (`stream::scan_of`),
//! and judges its rows by it right as it gives each batch, before any
//! column it gives late is decoded.

use std::sync::Arc;

use super::stream::{self, Batches};
use super::told;
use crate::db::State;
use crate::exec::table::RuntimeFilter;
use crate::logical::{Cond, Expr};
use crate::physical::{Plan, SemiJoin, SemiMethod};
use crate::storage::Batch;
use ordered::Placed;

mod ordered;

/// Whether hash joins make runtime filters, as EXECUTOR.RUNTIME_FILTERS
/// says.
pub(in crate::exec) fn wanted(db: &State) -> bool {
    if let crate::value::Value::Char(s) = db.settings.get(crate::settings::EXECUTOR_RUNTIME_FILTERS) { s != "OFF" } else { true }
}

/// A runtime filter told a stream: of the column of its rows' it judges.
#[derive(Debug, Clone)]
pub(in crate::exec) struct Told {
    pub(in crate::exec) filter: Arc<RuntimeFilter>,
    pub(in crate::exec) column: usize,
    /// Whether it crossed a join's equality of keys to this column, so it
    /// judges exact numbers alone (`RuntimeFilter::kept`).
    pub(in crate::exec) moved: bool,
}

impl Told {
    /// The filter of the join's key, told of its left column `column`.
    pub(in crate::exec) fn of(filter: Arc<RuntimeFilter>, column: usize) -> Told {
        Told { filter, column, moved: false }
    }

    /// It, told of `column` in its place, moved if `moved` too.
    fn to(&self, (column, moved): (usize, bool)) -> Told {
        Told { filter: Arc::clone(&self.filter), column, moved: self.moved || moved }
    }
}

/// The column of the rows of `plan`'s input a filter of its column
/// `column` judges, if its operator takes the filter down to it, as the
/// module's documentation says, and whether it crossed an equality of
/// keys to it; of a scan, that column of its table. Of an inner hash
/// join, its left operand's: the column itself, or the left key a right
/// key of it equals (`built_side` gives the right operand's).
fn passed(plan: &Plan, column: usize, db: &State) -> Option<(usize, bool)> {
    match plan {
        Plan::SeqScan { order, .. } => order.is_empty().then_some((column, false)),
        Plan::Filter { cond, .. } => (!cond.can_raise() && !cond.has_subquery()).then_some((column, false)),
        Plan::Project { exprs, .. } => moved(exprs, column).filter(|_| !exprs.iter().any(Expr::can_raise)).map(|c| (c, false)),
        Plan::HashJoin { left, keys, .. } => {
            let (_, width) = stream::layout(left, db)?;
            let right = column.checked_sub(width);
            right.map_or(Some((column, false)), |r| keys.iter().find(|k| k.right == r).map(|k| (k.left, true)))
        }
        // A semi join's columns are its left's (RULES.md D540).
        Plan::SemiJoin(s) => stream::layout(&s.left, db).filter(|&(_, width)| column < width).map(|_| (column, false)),
        // A mark join gives every left row on, its columns the left's then
        // the mark (RULES.md D643).
        Plan::MarkJoin(m) => stream::layout(&m.left, db).filter(|&(_, width)| column < width).map(|_| (column, false)),
        // A whole sort of a stream's every column keeps a row's place.
        Plan::Sort { most: None, .. } => stream::layout(plan, db).map(|_| (column, false)),
        Plan::IndexScan { .. } | Plan::IndexJoin { .. } | Plan::MergeJoin(_) | Plan::Aggregate { .. } => {
            told::layout(plan, db)?;
            row_by_row(plan, column, db)
        }
        Plan::Nearest(_)
        | Plan::RangeJoin(_)
        | Plan::GroupJoin(..)
        | Plan::Product { .. }
        | Plan::OuterJoin { .. }
        | Plan::Append { .. }
        | Plan::MergeAppend { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Limit { .. }
        | Plan::Distinct { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Parallel(..) => None,
        // An analyzed plan's count of an operator's rows passes them on.
        Plan::Counted(_) => Some((column, false)),
    }
}

/// \[DESIGN\] (RULES.md D600) Of an operator run row by row whose rows a
/// stream gathers (`told.rs`), the column of its input a filter of its
/// column `column` judges, and whether it crossed an equality: an index
/// scan's, its table's; an inner index join's, of its left's columns; an
/// inner merge join's, of its left's columns or the left key its right
/// key equals; a grouping's, the input column of a grouping column. A
/// join passes none if the rest of its condition can raise (RULES.md
/// D15).
fn row_by_row(plan: &Plan, column: usize, db: &State) -> Option<(usize, bool)> {
    match plan {
        Plan::IndexScan { .. } => Some((column, false)),
        Plan::IndexJoin { left, cond, .. } => (!raises(cond.as_ref()) && column < stream::layout(left, db)?.1).then_some((column, false)),
        Plan::MergeJoin(m) => {
            if raises(m.cond.as_ref()) {
                return None;
            }
            column.checked_sub(m.widths.0).map_or(Some((column, false)), |r| (r == m.key.right).then_some((m.key.left, true)))
        }
        Plan::Aggregate { groups, .. } => groups.get(column).map(|&g| (g, false)),
        Plan::SeqScan { .. }
        | Plan::Nearest(_)
        | Plan::Filter { .. }
        | Plan::Project { .. }
        | Plan::HashJoin { .. }
        | Plan::RangeJoin(_)
        | Plan::Product { .. }
        | Plan::OuterJoin { .. }
        | Plan::Append { .. }
        | Plan::MergeAppend { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Sort { .. }
        | Plan::Limit { .. }
        | Plan::Distinct { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Parallel(..)
        | Plan::SemiJoin(_)
        | Plan::GroupJoin(_)
        | Plan::MarkJoin(_)
        | Plan::Counted(_) => None,
    }
}

/// Whether a join's rest of its condition `cond` can raise or reads a
/// subquery.
fn raises(cond: Option<&Cond>) -> bool {
    cond.is_some_and(|c| c.can_raise() || c.has_subquery())
}

/// The column of a projection's input that its column `column` is, of
/// `exprs`, if it is one moved as it is.
fn moved(exprs: &[Expr], column: usize) -> Option<usize> {
    if let Some(Expr::Column(c)) = exprs.get(column) { Some(*c) } else { None }
}

/// Of `told`, the filters a projection by `exprs` takes down to its input,
/// each of the column it moves: those of columns moved as they are.
pub(super) fn projected(exprs: &[Expr], told: &[Told]) -> Vec<Told> {
    told.iter().filter_map(|t| moved(exprs, t.column).map(|column| t.to((column, false)))).collect()
}

/// Of `told`, the filters `plan`'s operator takes down to its input, each
/// of the column of its rows it judges there (`passed`), and those judged
/// of the batches it gives.
pub(in crate::exec) fn taken(plan: &Plan, told: &[Told], db: &State) -> (Vec<Told>, Vec<Told>) {
    let (mut below, mut here) = (Vec::new(), Vec::new());
    for t in told {
        match passed(plan, t.column, db) {
            Some(to) => below.push(t.to(to)),
            None => here.push(t.clone()),
        }
    }
    (below, here)
}

/// \[DESIGN\] (RULES.md D600) Of `told`, the filters an inner hash join
/// `plan` tells its right operand as it builds it, each of a column of
/// the right's rows: one of the right's own columns, or the right key
/// equal to a left key it judges. A row of the right it drops matches
/// only left rows whose joined rows the filter drops.
pub(in crate::exec) fn built_side(plan: &Plan, told: &[Told], db: &State) -> Vec<Told> {
    told.iter().filter_map(|t| built_column(plan, t.column, db).map(|to| t.to(to))).collect()
}

/// The column of the right operand of `plan`, an inner hash join, a
/// filter of its column `column` judges as it builds it, and whether it
/// crossed an equality of keys to it (`built_side`).
fn built_column(plan: &Plan, column: usize, db: &State) -> Option<(usize, bool)> {
    if let Plan::MergeJoin(m) = plan {
        // Its right operand, as `row_by_row` passes its left (RULES.md D600).
        told::layout(plan, db)?;
        if raises(m.cond.as_ref()) {
            return None;
        }
        let right = column.checked_sub(m.widths.0).map(|r| (r, false));
        return right.or_else(|| (column == m.key.left).then_some((m.key.right, true)));
    }
    if let Plan::SemiJoin(s) = plan {
        return semi_built(s, column);
    }
    let Plan::HashJoin { left, keys, .. } = plan else { return None };
    let (_, width) = stream::layout(left, db)?;
    column.checked_sub(width).map(|r| (r, false)).or_else(|| keys.iter().find(|k| k.left == column).map(|k| (k.right, true)))
}

/// Of `told`, the filters `plan`'s operator takes down to an input, as
/// told it, of its rows' columns, and those judged of the batches it
/// gives (`taken`). An inner hash join's filter its left does not take
/// but its right does (`built_side`) is in both.
pub(super) fn parted(plan: &Plan, told: &[Told], db: &State) -> (Vec<Told>, Vec<Told>) {
    let left = |t: &&Told| passed(plan, t.column, db).is_some();
    let kept = told.iter().filter(|t| left(t) || built_column(plan, t.column, db).is_some()).cloned().collect();
    (kept, told.iter().filter(|t| !left(t)).cloned().collect())
}

/// Whether a filter of column `column` of `plan`'s rows would reach a scan
/// in no order, each operator on the way taking it down (`passed`), an
/// inner hash join to either operand (`built_side`).
pub(super) fn reaches(plan: &Plan, column: usize, db: &State) -> bool {
    if join_right(plan).is_some_and(|right| built_column(plan, column, db).is_some_and(|(r, _)| reaches(right, r, db))) {
        return true;
    }
    let Some((next, _)) = passed(plan, column, db) else { return false };
    match below(plan) {
        Some(input) => reaches(input, next, db),
        // A grouping judges it of its input's batches at least.
        None => matches!(plan, Plan::SeqScan { .. } | Plan::IndexScan { .. } | Plan::Aggregate { .. }),
    }
}

/// \[DESIGN\] (RULES.md D600) The column of the right operand of the
/// semi join `s` a filter of its column `column` judges as it builds it:
/// the right key a left key equals. A right row the filter drops matches
/// only left rows it drops. Of an antijoin none, as a right row of a null
/// key it drops would change what NOT IN finds of every left row; of a
/// semi join whose condition can raise none (RULES.md D15).
fn semi_built(s: &SemiJoin, column: usize) -> Option<(usize, bool)> {
    if s.anti || !matches!(s.method, SemiMethod::Hash) || raises(s.cond.as_ref()) {
        return None;
    }
    s.keys.iter().find(|k| k.left == column).map(|k| (k.right, true))
}

/// The right operand of the join `plan`: a hash join's, a merge join's or
/// a semi join's.
fn join_right(plan: &Plan) -> Option<&Plan> {
    if let Plan::HashJoin { right, .. } = plan {
        return Some(right);
    }
    if let Plan::SemiJoin(s) = plan {
        return Some(&s.right);
    }
    if let Plan::MergeJoin(m) = plan { Some(&m.right) } else { None }
}

/// The input of `plan`, or its left operand, that a filter `passed` goes
/// down to, other than a scan's and a grouping's.
fn below(plan: &Plan) -> Option<&Plan> {
    match plan {
        Plan::Filter { input, .. } | Plan::Project { input, .. } | Plan::Sort { input, .. } => Some(input),
        Plan::HashJoin { left, .. } | Plan::IndexJoin { left, .. } => Some(left),
        Plan::Counted(c) => Some(&c.input),
        Plan::SemiJoin(s) => Some(&s.left),
        // A mark join gives every left row on (RULES.md D643).
        Plan::MarkJoin(m) => Some(&m.left),
        Plan::MergeJoin(m) => Some(&m.left),
        Plan::SeqScan { .. }
        | Plan::IndexScan { .. }
        | Plan::Aggregate { .. }
        | Plan::Nearest(_)
        | Plan::RangeJoin(_)
        | Plan::Product { .. }
        | Plan::OuterJoin { .. }
        | Plan::Append { .. }
        | Plan::MergeAppend { .. }
        | Plan::SetOp { .. }
        | Plan::Values { .. }
        | Plan::System { .. }
        | Plan::Limit { .. }
        | Plan::Distinct { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::Polymorphic(..)
        | Plan::Recognize { .. }
        | Plan::WorkTable { .. }
        | Plan::Transition { .. }
        | Plan::Parallel(..)
        | Plan::GroupJoin(_) => None,
    }
}

/// \[DESIGN\] (RULES.md D389) What a filter judges, its range and its
/// Bloom filter if it `blooms`, of a scan if `at_scan`, or above an
/// operator: none if neither. A scan judges the range among its
/// conjuncts, so the filter does not again.
fn asked((blooms, at_scan): (bool, bool)) -> Option<(bool, bool)> {
    (blooms || !at_scan).then_some((!at_scan, blooms))
}

/// `batches`, each of whose vectors holds the column of its rows at its
/// place in `columns`, the rows each of `told` keeps of each batch kept,
/// judged while it judges (`RuntimeFilter::judging`), so a filter that
/// keeps most rows stops costing anything (RULES.md D389), the filter
/// that kept the least first (`ordered.rs`). Of a scan, `at_scan`, its
/// ranges are the scan's own conjuncts, and its Bloom filter alone is
/// judged, right as the scan gives each batch, before any column given
/// late is decoded; above another operator, both.
pub(in crate::exec) fn applied(batches: Batches, told: Vec<Told>, columns: &[usize], (db, at_scan): (&Arc<State>, bool)) -> Batches {
    let mut placed: Vec<Placed> = told
        .into_iter()
        .filter_map(|t| {
            let at = columns.iter().position(|&c| c == t.column)?;
            asked((t.filter.blooms(), at_scan)).map(|asked| Placed::new(t.filter, (at, t.moved), asked))
        })
        .collect();
    if placed.is_empty() {
        return batches;
    }
    let db = db.clone();
    Box::new(batches.map(move |kept| {
        let (batch, rows) = kept?;
        let rows = ordered::judged(&mut placed, (&batch, rows), db.functions.kernels())?;
        Ok((batch, rows))
    }))
}

/// \[DESIGN\] (RULES.md D600, D15) `batches`, of a grouping's input, the
/// rows each of `told` keeps of each batch kept, as `applied` keeps them
/// above an operator, and the warning of a null eliminated by a set
/// function [ISO/IEC 9075-2:2023 §10.9 GR4b] raised if a row a filter
/// dropped holds a null in a column at a place of `args`, which the set
/// functions take: the grouping warns as it would of every row.
pub(super) fn applied_warning(
    batches: Batches,
    told: Vec<Told>,
    (columns, args): (&[usize], Vec<usize>),
    (db, tx): (&crate::exec::Db, crate::txn::TxId),
) -> Batches {
    let mut placed: Vec<Placed> = told
        .into_iter()
        .filter_map(|t| columns.iter().position(|&c| c == t.column).map(|at| Placed::new(t.filter, (at, t.moved), (true, true))))
        .collect();
    if placed.is_empty() {
        return batches;
    }
    let runner = crate::exec::Runner::new(db, tx);
    Box::new(batches.map(move |kept| {
        let (batch, rows) = kept?;
        let given = crate::storage::sel::places_of(rows.clone(), batch.rows);
        let rows = ordered::judged(&mut placed, (&batch, rows), runner.kernels())?;
        super::eliminated(dropped_null(&batch, &args, (&given, rows.as_deref())), &runner);
        Ok((batch, rows))
    }))
}

/// Whether a row of `given` that `kept` leaves out holds a null in a
/// column of `batch` at a place of `args`.
fn dropped_null(batch: &Batch, args: &[usize], (given, kept): (&[usize], Option<&[u32]>)) -> bool {
    let Some(kept) = kept else { return false };
    let dropped = |r: &&usize| u32::try_from(**r).is_ok_and(|r| kept.binary_search(&r).is_err());
    given.iter().filter(dropped).any(|&r| args.iter().any(|&c| batch.cell(c, r).is_null()))
}

// Unit tests: what a filter judges where (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;
    use crate::db::Database;
    use crate::exec::table::{Hashing, KeyHashes, Seed};
    use crate::physical::JoinKey;
    use crate::storage::{Batch, Column, Ints};

    /// Verifies: [DESIGN] RULES.md D389 (MC/DC): of a scan, the range,
    /// among its conjuncts, is not judged again, and the Bloom filter is
    /// if there is one; above an operator, the range and the Bloom filter
    /// if any; of a scan and no Bloom filter, nothing.
    #[test]
    fn a_scan_judges_its_range_once() {
        assert_eq!(asked((true, true)), Some((false, true)), "a scan");
        assert_eq!(asked((true, false)), Some((true, true)), "above an operator");
        assert_eq!(asked((false, false)), Some((true, false)), "above an operator, no Bloom filter");
        assert_eq!(asked((false, true)), None, "a scan, no Bloom filter");
    }

    /// The scan of both columns of table `t` of schema A.
    fn scan(t: &str) -> Box<Plan> {
        let table = QualifiedName { auth: "A".into(), ident: t.into() };
        Box::new(Plan::SeqScan {
            table,
            columns: vec![0, 1],
            rows: 0,
            cost: 0.0_f64,
            order: Vec::new(),
            backward: false,
            most: None,
            located: false,
        })
    }

    /// A filter of built keys 1 and 2, told of `column`.
    fn told(column: usize) -> Told {
        let seed = Seed::fixed(7);
        let how = Hashing { seed: &seed, keyed: false, direct: false };
        let built = Batch { columns: vec![Column::Ints(Ints::new(vec![1, 2], 0, None))], rows: 2 };
        let mut hashes = KeyHashes::default();
        hashes.of_batch((&built, None), (&[0], &[false]), how).unwrap();
        Told::of(Arc::new(RuntimeFilter::of_built(&built, (0, false), &hashes, how).unwrap()), column)
    }

    /// Each of `told`'s columns, and whether it was moved.
    fn columns(told: &[Told]) -> Vec<(usize, bool)> {
        told.iter().map(|t| (t.column, t.moved)).collect()
    }

    /// Verifies: [DESIGN] RULES.md D600 (MC/DC): an inner hash join of T
    /// and U on T.K = U.B takes a filter of T.K to its left as it is and
    /// to its right as U.B, moved; of U.B to its left as T.K, moved, and
    /// to its right as it is; of U.K, no key, to its right alone, judged
    /// above it too; of T.A to its left alone; each reaches a scan.
    #[test]
    fn a_hash_join_tells_both_operands_by_its_keys() {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, A INTEGER) CREATE TABLE U (K INTEGER, B INTEGER)").unwrap();
        let state = db.snapshot().unwrap();
        let keys = vec![JoinKey { left: 0, right: 1, nulls: false }];
        let join = Plan::HashJoin { left: scan("T"), right: scan("U"), keys, detail: String::new(), est: 0.0_f64 };
        let (all, left_of, right_of) = (
            [told(0), told(1), told(2), told(3)],
            |t: &[Told]| columns(&taken(&join, t, &state).0),
            |t: &[Told]| columns(&built_side(&join, t, &state)),
        );
        assert_eq!(left_of(&all), vec![(0, false), (1, false), (0, true)]);
        assert_eq!(right_of(&all), vec![(1, true), (0, false), (1, false)]);
        assert_eq!(columns(&taken(&join, &all, &state).1), vec![(2, false)], "U.K judged above it");
        let (kept, here) = parted(&join, &all, &state);
        assert_eq!((columns(&kept).len(), columns(&here)), (4, vec![(2, false)]), "U.K both told the right and judged above");
        assert!((0..4).all(|c| reaches(&join, c, &state)));
        assert_eq!(columns(&[told(0).to((2, true)).to((1, false))]), vec![(1, true)], "moved once, moved after");
    }
}

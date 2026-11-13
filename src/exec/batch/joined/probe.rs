//! \[DESIGN\] An inner hash join of streams, and a LEFT outer join on keys
//! whose condition beyond them cannot raise an error and whose operands
//! have no condition of their own (RULES.md D31): the right operand read
//! first and held as vectors within the work area (`held.rs`); each of the
//! left operand's batches probes it, the condition judged of each pair of
//! equal keys, of a row of the pair's values, and the right's columns of
//! the pairs gathered of its vectors. Of a LEFT join, a left row no right
//! row matches is joined with nulls [ISO/IEC 9075:1992 §7.5 GR4, GR5b].
//! Past the work area the right operand is built as the row path builds it
//! (`exec/join.rs`): of a LEFT join, it moves to a table of the spill
//! area, so every left row probes it in the left's order; of an inner one,
//! it is partitioned (`parted.rs`).

use std::sync::Arc;

use super::super::stream::{self, Batches, Kept};
use super::held::{self, HeldRight, Read, Rest};
use super::pairs::{Out, kept_rows};
use super::{gathered, in_order, left_batches, narrow_rows, parted};
use crate::db::State;
use crate::error::Result;
use crate::exec::join::{Built, Full, Right, partitioned_joined};
use crate::exec::work::Work;
use crate::exec::{Runner, column, eval_cond};
use crate::logical::{Cond, Truth};
use crate::physical::{JoinKey, Plan};
use crate::storage::{Batch, Row};
use crate::txn::TxId;
use crate::value::{Bucket, Value, key_bucket, key_equal};

/// Which rows a join of streams probing its built right operand gives.
pub(super) enum Kind {
    /// The pairs that match.
    Inner,
    /// Those for which the condition, if any, is true, its operands of
    /// the widths given; and each left row none matches, with nulls.
    Left(Option<Cond>, (usize, usize)),
}

/// The batches of a hash join of `left` and `right` on `keys`, as `kind`
/// says.
///
/// # Errors
///
/// The right operand's; the spill area's; a key the left's batches lack.
pub(super) fn probed((left, right, keys): (Plan, Plan, Vec<JoinKey>), kind: Kind, db: &Arc<State>, tx: TxId) -> Result<Batches> {
    let (left_columns, _) = stream::layout(&left, db).unwrap_or_default();
    let (right_columns, _) = stream::layout(&right, db).unwrap_or_default();
    let mut work = Work::of(db, tx);
    let full = match kind {
        Kind::Inner => Full::Partition,
        Kind::Left(..) => Full::Table,
    };
    let places: Option<Vec<JoinKey>> =
        keys.iter().map(|&k| left_columns.iter().position(|&c| c == k.left).map(|at| JoinKey { left: at, ..k })).collect();
    let places = places.ok_or_else(|| crate::exec::broken("a join key the left operand's batch lacks"))?;
    let scratch = match &kind {
        Kind::Left(Some(_), (l, r)) => vec![Value::Null; l.saturating_add(*r)],
        Kind::Inner | Kind::Left(None, _) => Vec::new(),
    };
    // The right operand first, held as vectors if it fits the work area.
    let rest = match held::read(right, &keys, work.limit(), (db, tx))? {
        Read::Held(held) => {
            let out = Out::over(left_columns.len(), Arc::clone(&held.batch));
            let joined = Joined { left_columns, right_columns, kind, scratch, runner: Runner::new(db, tx) };
            let left = left_batches(left, db, tx)?;
            return Ok(Box::new(Probe { left, joined, right: Holding::Vectors(held), places, out, _work: work }));
        }
        Read::Rest(rest) => rest,
    };
    // An inner join's rows, which no condition reads whole, are built and
    // partitioned of the columns their batches hold alone.
    let Kind::Inner = kind else {
        let built = match Built::of(rest.rows(), keys, (&mut work, full))? {
            Right::Held(built) => built,
            Right::Partitioned(_) => return Err(crate::exec::broken("a LEFT join's right operand was partitioned")),
        };
        let out = Out::new(left_columns.len(), right_columns.clone());
        let joined = Joined { left_columns, right_columns, kind, scratch, runner: Runner::new(db, tx) };
        let left = left_batches(left, db, tx)?;
        return Ok(Box::new(Probe { left, joined, right: Holding::Built(built), places, out, _work: work }));
    };
    narrowed((left, rest, keys), (Joined { left_columns, right_columns, kind, scratch, runner: Runner::new(db, tx) }, places), work, db, tx)
}

/// The batches of an inner hash join of `left` and a right operand past
/// the work area, `rest`, on `keys`, its pairs kept by `joined`, the keys'
/// columns at `places` of a left batch: both operands' rows of the columns
/// their batches hold alone, the keys at their places among them, the
/// right built of them in `work`, or partitioned.
///
/// # Errors
///
/// As `probed`.
fn narrowed(
    (left, rest, keys): (Plan, Rest, Vec<JoinKey>),
    (joined, places): (Joined, Vec<JoinKey>),
    mut work: Work,
    db: &Arc<State>,
    tx: TxId,
) -> Result<Batches> {
    let place = |c: usize, of: &[usize]| of.iter().position(|&h| h == c).ok_or_else(|| crate::exec::broken("a join key its batch lacks"));
    let (left_columns, right_columns) = (&joined.left_columns, &joined.right_columns);
    let narrow = keys
        .iter()
        .map(|&k| Ok(JoinKey { left: place(k.left, left_columns)?, right: place(k.right, right_columns)?, nulls: k.nulls }))
        .collect::<Result<Vec<_>>>()?;
    let places = places.iter().zip(&narrow).map(|(&at, &n)| JoinKey { right: n.right, ..at }).collect();
    let (left_width, right_width) = (left_columns.len(), right_columns.len());
    let (lefts, rights): (Vec<usize>, Vec<usize>) = ((0..left_width).collect(), (0..right_width).collect());
    match Built::of(rest.narrow(), narrow, (&mut work, Full::Partition))? {
        Right::Held(built) => {
            let (out, left) = (Out::new(left_width, rights), left_batches(left, db, tx)?);
            Ok(Box::new(Probe { left, joined, right: Holding::Built(built), places, out, _work: work }))
        }
        Right::Partitioned(parts) => {
            // In no order but the partitions', where none is asked for.
            if !in_order(&left) {
                return parted::joined(narrow_rows(left, db, tx), parts, work, (lefts, rights));
            }
            let columns: Vec<usize> = (0..left_width.saturating_add(right_width)).collect();
            Ok(gathered(partitioned_joined(narrow_rows(left, db, tx), parts, work), &columns))
        }
    }
}

/// Which pairs a hash join of streams keeps: the columns of its left
/// operand's rows its batches hold, which rows it gives, and the row a
/// condition is judged of, of every column of the pair, with what runs its
/// subqueries.
struct Joined {
    left_columns: Vec<usize>,
    /// The columns of the right operand's rows its batches hold.
    right_columns: Vec<usize>,
    kind: Kind,
    scratch: Row,
    runner: Runner,
}

impl Joined {
    /// Whether left row `r` of `batch` and `right` are a pair the join
    /// keeps: their keys equal, as `equal` says, and the condition, if
    /// any, true of them [ISO/IEC 9075:1992 §7.5 GR1c].
    fn keeps(&mut self, (batch, r): (&Batch, usize), right: &Row, equal: bool) -> Result<bool> {
        let Kind::Left(Some(cond), (width, _)) = &self.kind else { return Ok(equal) };
        Ok(equal && judged(cond, (&mut self.scratch, *width), (batch, r), (right, &self.left_columns), &self.runner)?)
    }

    /// Whether left row `r` of `batch` and row `p` of `right`, held as
    /// vectors, are a pair the join keeps, as `keeps` says.
    fn keeps_held(&mut self, (batch, r): (&Batch, usize), (right, p): (&Batch, usize), equal: bool) -> Result<bool> {
        let Joined { left_columns, right_columns, kind, scratch, runner } = self;
        let Kind::Left(Some(cond), (width, _)) = kind else { return Ok(equal) };
        if !equal {
            return Ok(false);
        }
        for (at, &c) in right_columns.iter().enumerate() {
            if let Some(slot) = scratch.get_mut(width.saturating_add(c)) {
                *slot = right.cell(at, p).to_value();
            }
        }
        judged_of(cond, scratch, (batch, r), left_columns, runner)
    }
}

/// A hash join's right operand: held as vectors, or built as the row path
/// builds it.
enum Holding {
    Vectors(HeldRight),
    Built(Built),
}

/// Whether `cond` is true of the pair of left row `r` of `batch`, whose
/// vectors hold `left_columns`, and `right`: of a row of their values made
/// in `scratch`, the left operand's `width` columns first.
fn judged(
    cond: &Cond,
    (scratch, width): (&mut Row, usize),
    (batch, r): (&Batch, usize),
    (right, left_columns): (&Row, &[usize]),
    runner: &Runner,
) -> Result<bool> {
    for (slot, v) in scratch.iter_mut().skip(width).zip(right) {
        slot.clone_from(v);
    }
    judged_of(cond, scratch, (batch, r), left_columns, runner)
}

/// Whether `cond` is true of the row in `scratch`, the right row's values
/// in it, once left row `r` of `batch`, whose vectors hold `left_columns`,
/// is put in it too.
fn judged_of(cond: &Cond, scratch: &mut Row, (batch, r): (&Batch, usize), left_columns: &[usize], runner: &Runner) -> Result<bool> {
    for (at, &c) in left_columns.iter().enumerate() {
        if let Some(slot) = scratch.get_mut(c) {
            *slot = batch.cell(at, r).to_value();
        }
    }
    Ok(eval_cond(cond, scratch, runner)? == Truth::True)
}

/// A hash join's batches: each of its left operand's probing its right,
/// held.
struct Probe {
    left: Batches,
    joined: Joined,
    right: Holding,
    /// Each key, its left column's place in a left batch, with its right
    /// column.
    places: Vec<JoinKey>,
    out: Out,
    /// The work area the right operand is built in, held while it is.
    _work: Work,
}

impl Probe {
    /// Probes the right operand with the rows of `batch` at `rows`.
    fn probe(&mut self, batch: &Batch, rows: &[usize]) -> Result<()> {
        let mut values: Vec<Value> = Vec::with_capacity(self.places.len());
        let mut key: Vec<Bucket> = Vec::with_capacity(self.places.len());
        for &r in rows {
            values.clear();
            values.extend(self.places.iter().map(|k| batch.cell(k.left, r).to_value()));
            let keyed = keyed(&self.places, &values, &mut key)?;
            let found = match &self.right {
                Holding::Vectors(held) => {
                    let pairs = (&mut self.joined, &mut self.out);
                    keyed && probe_held(held, pairs, (batch, r), (&values, &key))?
                }
                Holding::Built(built) => {
                    let pairs = (&mut self.joined, &mut self.out, self.places.as_slice());
                    keyed && probe_built(built, pairs, (batch, r), (&values, &key))?
                }
            };
            // [ISO/IEC 9075:1992 §7.5 GR4, GR5b] A LEFT join's left row
            // no right row matches, with nulls.
            if !found && matches!(self.joined.kind, Kind::Left(..)) {
                self.out.push_held(Some(batch), Some(r), None);
            }
        }
        Ok(())
    }
}

/// Whether the key `values` of `keys` has a bucket, put in `key`: none if
/// a value is null, as a null equals nothing [X3.135-1986 §5.11 GR2], but
/// of a key on which a null matches a null [ISO/IEC 9075-2:2023 §8.15].
///
/// # Errors
///
/// SQLCODE -910 if a value nests deeper than a type may.
pub(super) fn keyed(keys: &[JoinKey], values: &[Value], key: &mut Vec<Bucket>) -> Result<bool> {
    key.clear();
    for (k, v) in keys.iter().zip(values) {
        match key_bucket(v, k.nulls)? {
            Some(b) => key.push(b),
            None => return Ok(false),
        }
    }
    Ok(true)
}

/// Adds to `out` the pairs `joined` keeps of left row `r` of `batch`, of
/// key `values`, bucketed as `key`, and the rows of that key of `built`,
/// their keys' right columns those `places` gives: whether it kept any.
fn probe_built(
    built: &Built,
    (joined, out, places): (&mut Joined, &mut Out, &[JoinKey]),
    (batch, r): (&Batch, usize),
    (values, key): (&[Value], &[Bucket]),
) -> Result<bool> {
    let mut found = false;
    for right in built.bucket_rows(key)? {
        let right = right?;
        if joined.keeps((batch, r), &right, keys_match(places, values, &right)?)? {
            found = true;
            out.push(Some(batch), Some(r), Some(&right));
        }
    }
    Ok(found)
}

/// Whether the right columns of `keys` of row `right` match the left's
/// `values` as keys (`key_equal`).
///
/// # Errors
///
/// A column the row lacks, or a value nested deeper than a type may
/// (SQLCODE -910).
pub(super) fn keys_match(keys: &[JoinKey], values: &[Value], right: &Row) -> Result<bool> {
    for (k, v) in keys.iter().zip(values) {
        if !key_equal(v, column(right, k.right)?, k.nulls)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Adds to `out` the pairs `joined` keeps of left row `r` of `batch`, of
/// key `values`, bucketed as `key`, and the rows of that key of `held`:
/// whether it kept any.
fn probe_held(
    held: &HeldRight,
    (joined, out): (&mut Joined, &mut Out),
    (batch, r): (&Batch, usize),
    (values, key): (&[Value], &[Bucket]),
) -> Result<bool> {
    let mut found = false;
    for &p in held.rows_of(key) {
        if joined.keeps_held((batch, r), (&held.batch, p), held.keys_equal(p, values)?)? {
            found = true;
            out.push_held(Some(batch), Some(r), Some(p));
        }
    }
    Ok(found)
}

impl Iterator for Probe {
    type Item = Result<Kept>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(given) = self.out.next() {
                return Some(given);
            }
            let (batch, rows) = match self.left.next()? {
                Ok(kept) => kept,
                Err(e) => return Some(Err(e)),
            };
            let rows = kept_rows(rows, batch.rows);
            let done = self.probe(&batch, &rows);
            self.out.settle(Some(&batch), done);
        }
    }
}

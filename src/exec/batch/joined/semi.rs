//! \[DESIGN\] A semi join or an antijoin of streams, hashed (RULES.md
//! D540): the right operand read first and held as vectors within the work
//! area, or past it in a table of the spill area (`held.rs`); each left
//! batch's rows looked up among its rows by their keys until one matches,
//! the condition beyond the keys judged of the pair as a LEFT join's is
//! (`probe.rs`). A left batch is given on, those of its rows kept, or of an
//! antijoin those not, selected, so no value is copied.
//!
//! A semi join keeps only left rows whose keys the right holds, so, as an
//! inner join, it hands the left's scan a runtime filter of each key
//! (RULES.md D369); an antijoin keeps the others, so it hands none.
//!
//! One planned to build its left operand, the operand of fewer rows
//! (`SemiMethod::BuildLeft`), holds the left and marks its rows as it
//! reads the right (`semi/marked.rs`, RULES.md D1070); it comes here,
//! the left's batches it read given first, only if they outgrow the work
//! area.

mod marked;
mod pairing;
mod split;
mod step;

pub(in crate::exec) use split::held_split;
pub(in crate::exec) use step::SemiStep;

use std::sync::Arc;

use super::super::late::Deferred;
use super::super::runtime::{self, Told};
use super::super::stream::{self, Batches, Kept};
use super::held::{self, HeldRight, Read};
use super::pairs::kept_rows;
use super::probe::{Joined, keyed, keys_match};
use super::{in_order, left_late_batches};
use crate::error::Result;
use crate::exec::Db;
use crate::exec::join::{Built, Full, Right};
use crate::exec::table::KeyHashes;
use crate::exec::work::Work;
use crate::exec::{Runner, broken};
use crate::logical::Cond;
use crate::physical::{JoinKey, Plan, SemiJoin};
use crate::storage::{Batch, Sel};
use crate::txn::TxId;
use crate::value::{Bucket, Value};

/// The batches of `plan`, a hashed semi join or antijoin of streams.
///
/// # Errors
///
/// The right operand's; the spill area's; a key the left's batches lack.
pub(super) fn semi_batches(plan: Plan, db: &Db, tx: TxId) -> Result<Batches> {
    told_batches(plan, &Deferred::default(), (db, tx))
}

/// As `semi_batches`, the runtime filters `deferred` tells of its columns,
/// its left's, handed to its left with its own.
///
/// # Errors
///
/// As `semi_batches`.
pub(super) fn told_batches(plan: Plan, deferred: &Deferred, (db, tx): (&Db, TxId)) -> Result<Batches> {
    let building = runtime::built_side(&plan, deferred.filters(), db);
    let Plan::SemiJoin(s) = plan else { return Err(broken("a semi join is not one")) };
    let (left_columns, _) = stream::layout(&s.left, db).unwrap_or_default();
    if !marked::builds_left(&s) {
        let filtered = filtered_keys(&s, &s.left, db);
        let join = Hashed { left_columns, filtered, building };
        let SemiJoin { anti, left, right, keys, cond, widths, .. } = *s;
        return hashed((anti, right, keys, cond, widths), (Left::Plan(Box::new(left)), join), deferred, (db, tx));
    }
    match marked::batches(*s, (&left_columns, deferred), (db, tx))? {
        marked::Read::Marked(batches) => Ok(batches),
        marked::Read::GaveWay(rest, left) => {
            let join = Hashed { left_columns, filtered: Vec::new(), building };
            hashed(*rest, (Left::Open(left), join), deferred, (db, tx))
        }
    }
}

/// The places among `s`'s keys of those a runtime filter is made of, of a
/// semi join that builds its right: each whose filter reaches the scan of
/// `left`, in no order; of an antijoin none.
fn filtered_keys(s: &SemiJoin, left: &Plan, db: &Db) -> Vec<usize> {
    if s.anti || !runtime::wanted(db) || in_order(left) {
        return Vec::new();
    }
    s.keys.iter().enumerate().filter(|(_, k)| runtime::reaches(left, k.left, db)).map(|(n, _)| n).collect()
}

/// A hashed semi join's left operand: its plan, opened once the right is
/// held, so it is told the right's filters; or its batches, opened by a
/// join that built its left and gave way (`semi/marked.rs`).
pub(super) enum Left {
    Plan(Box<Plan>),
    Open(Batches),
}

/// What a hashed semi join knows of its operands before it reads them:
/// the columns its left's batches hold, the places among its keys of
/// those a runtime filter is made of, and the filters its right is told
/// as it is built.
struct Hashed {
    left_columns: Vec<usize>,
    filtered: Vec<usize>,
    building: Vec<Told>,
}

/// Whether an antijoin, the right operand, the keys, the condition beyond
/// them and the operands' widths of a semi join.
type Parts = (bool, Plan, Vec<JoinKey>, Option<Cond>, (usize, usize));

/// The batches of the semi join of `parts` and `left`, its right built
/// as `join` says, its left's scans told what `deferred` tells.
///
/// # Errors
///
/// As `semi_batches`.
fn hashed(parts: Parts, (left, join): (Left, Hashed), deferred: &Deferred, (db, tx): (&Db, TxId)) -> Result<Batches> {
    let (anti, right, keys, cond, widths) = parts;
    let Hashed { left_columns, filtered, building } = join;
    let (right_columns, _) = stream::layout(&right, db).unwrap_or_default();
    let places: Option<Vec<JoinKey>> =
        keys.iter().map(|&k| left_columns.iter().position(|&c| c == k.left).map(|at| JoinKey { left: at, ..k })).collect();
    let places = places.ok_or_else(|| broken("a join key the left operand's batch lacks"))?;
    let mut work = Work::of(db, tx);
    let mut told = deferred.filters().to_vec();
    let side = match held::read(right, (&keys, &filtered, building), work.limit(), (db, tx))? {
        Read::Held(held) => {
            told.extend(held.filters.iter().filter_map(|(k, f)| keys.get(*k).map(|key| Told::of(Arc::clone(f), key.left))));
            Side::Vectors(held)
        }
        Read::Rest(rest) => match Built::of(rest.rows(), keys, (&mut work, Full::Table))? {
            Right::Held(built) => Side::Built(built),
            Right::Partitioned(_) => return Err(broken("a semi join's right operand was partitioned")),
        },
    };
    let left = match left {
        Left::Plan(plan) => left_late_batches(*plan, (db, tx), &Deferred::default().with_told(told))?,
        Left::Open(batches) => batches,
    };
    let joined = Joined::matching((left_columns, right_columns), cond, widths, Runner::new(db, tx));
    Ok(Box::new(SemiProbe { left, joined, side, places, anti, _work: work }))
}

/// A semi join's right operand: held as vectors, or built as the row path
/// builds it.
enum Side {
    Vectors(Box<HeldRight>),
    Built(Built),
}

/// A semi join's batches: each of its left operand's rows looked up.
struct SemiProbe {
    left: Batches,
    joined: Joined,
    side: Side,
    /// Each key, its left column's place in a left batch, with its right
    /// column.
    places: Vec<JoinKey>,
    anti: bool,
    /// The work area the right operand is built in, held while it is.
    _work: Work,
}

impl SemiProbe {
    /// The rows of `batch` at `rows` it keeps.
    fn kept(&mut self, batch: &Batch, rows: &[usize]) -> Result<Vec<u32>> {
        if let Side::Vectors(held) = &mut self.side {
            let left: Vec<usize> = self.places.iter().map(|k| k.left).collect();
            held.hash_probing((batch, &left), rows)?;
            let probing = std::mem::take(&mut held.probing);
            let kept = hashed_kept(held, &mut self.joined, (batch, &left), (rows, &probing), self.anti);
            held.probing = probing;
            let kept = kept?;
            held.settle()?;
            return Ok(kept);
        }
        let mut values: Vec<Value> = Vec::with_capacity(self.places.len());
        let mut key: Vec<Bucket> = Vec::with_capacity(self.places.len());
        let mut out = Vec::with_capacity(rows.len());
        for &r in rows {
            values.clear();
            values.extend(self.places.iter().map(|k| batch.cell(k.left, r).to_value()));
            let found = keyed(&self.places, &values, &mut key)? && self.found((batch, r), &values, &key)?;
            if found != self.anti {
                out.push(u32::try_from(r).map_err(|_| broken("a batch's row past its count"))?);
            }
        }
        Ok(out)
    }

    /// Whether a right row matches left row `r` of `batch`, of key
    /// `values`, bucketed as `key`.
    fn found(&mut self, (batch, r): (&Batch, usize), values: &[Value], key: &[Bucket]) -> Result<bool> {
        let Side::Built(built) = &self.side else { return Ok(false) };
        for right in built.bucket_rows(key)? {
            let right = right?;
            if self.joined.keeps((batch, r), &right, keys_match(&self.places, values, &right)?)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// The rows of `batch` at `rows`, its key columns at `left`, their keys'
/// hashes `probing`, that a row of `held` matches, as `joined` judges, or
/// of an antijoin, `anti`, those none does.
///
/// # Errors
///
/// The condition's; SQLCODE -910 if a value nests deeper than a type may.
fn hashed_kept(
    held: &HeldRight,
    joined: &mut Joined,
    (batch, left): (&Batch, &[usize]),
    (rows, probing): (&[usize], &KeyHashes),
    anti: bool,
) -> Result<Vec<u32>> {
    let mut out = Vec::with_capacity(rows.len());
    for ((&r, &h), &miss) in rows.iter().zip(&probing.hashes).zip(&probing.misses) {
        let mut found = false;
        for p in held.candidates(h).filter(|_| !miss) {
            if joined.keeps_held((batch, r), (&held.batch, p), held.matches((batch, left, r), p)?)? {
                found = true;
                break;
            }
        }
        if found != anti {
            out.push(u32::try_from(r).map_err(|_| broken("a batch's row past its count"))?);
        }
    }
    Ok(out)
}

impl Iterator for SemiProbe {
    type Item = Result<Kept>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (batch, rows) = match self.left.next()? {
                Ok(kept) => kept,
                Err(e) => return Some(Err(e)),
            };
            let rows = kept_rows(rows, batch.rows);
            match self.kept(&batch, &rows) {
                Ok(kept) => {
                    if !kept.is_empty() {
                        return Some(Ok((batch, Some(Sel::of(kept)))));
                    }
                }
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

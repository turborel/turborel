//! \[DESIGN\] A semi join or an antijoin of streams, hashed (RULES.md
//! D540): the right operand read first and held as vectors within the work
//! area, or past it in a table of the spill area (`held.rs`); each left
//! batch's rows looked up among its rows by their keys until one matches,
//! the condition beyond the keys judged of the pair as a LEFT join's is
//! (`probe.rs`). A left batch is given on, those of its rows kept, or of an
//! antijoin those not, selected, so no value is copied.

use std::sync::Arc;

use super::super::stream::{self, Batches, Kept};
use super::held::{self, HeldRight, Read};
use super::left_batches;
use super::pairs::kept_rows;
use super::probe::{Joined, keyed, keys_match};
use crate::db::State;
use crate::error::Result;
use crate::exec::join::{Built, Full, Right};
use crate::exec::work::Work;
use crate::exec::{Runner, broken};
use crate::physical::{JoinKey, Plan, SemiJoin};
use crate::storage::{Batch, Sel};
use crate::txn::TxId;
use crate::value::{Bucket, Value};

/// The batches of `plan`, a hashed semi join or antijoin of streams.
///
/// # Errors
///
/// The right operand's; the spill area's; a key the left's batches lack.
pub(super) fn semi_batches(plan: Plan, db: &Arc<State>, tx: TxId) -> Result<Batches> {
    let Plan::SemiJoin(s) = plan else { return Err(broken("a semi join is not one")) };
    let SemiJoin { anti, left, right, keys, cond, widths, .. } = *s;
    let (left_columns, _) = stream::layout(&left, db).unwrap_or_default();
    let (right_columns, _) = stream::layout(&right, db).unwrap_or_default();
    let places: Option<Vec<JoinKey>> =
        keys.iter().map(|&k| left_columns.iter().position(|&c| c == k.left).map(|at| JoinKey { left: at, ..k })).collect();
    let places = places.ok_or_else(|| broken("a join key the left operand's batch lacks"))?;
    let mut work = Work::of(db, tx);
    let side = match held::read(right, &keys, work.limit(), (db, tx))? {
        Read::Held(held) => Side::Vectors(held),
        Read::Rest(rest) => match Built::of(rest.rows(), keys, (&mut work, Full::Table))? {
            Right::Held(built) => Side::Built(built),
            Right::Partitioned(_) => return Err(broken("a semi join's right operand was partitioned")),
        },
    };
    let joined = Joined::matching((left_columns, right_columns), cond, widths, Runner::new(db, tx));
    let left = left_batches(left, db, tx)?;
    Ok(Box::new(SemiProbe { left, joined, side, places, anti, _work: work }))
}

/// A semi join's right operand: held as vectors, or built as the row path
/// builds it.
enum Side {
    Vectors(HeldRight),
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
        match &self.side {
            Side::Vectors(held) => {
                for &p in held.rows_of(key) {
                    if self.joined.keeps_held((batch, r), (&held.batch, p), held.keys_equal(p, values)?)? {
                        return Ok(true);
                    }
                }
            }
            Side::Built(built) => {
                for right in built.bucket_rows(key)? {
                    let right = right?;
                    if self.joined.keeps((batch, r), &right, keys_match(&self.places, values, &right)?)? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }
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

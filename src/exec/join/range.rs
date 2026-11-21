//! \[DESIGN\] The range join (`physical/range.rs`, RULES.md D580): the rows
//! of a product its comparisons by order keep [X3.135-1986 §5.20 GR2b,
//! §5.21 GR2], or of a LEFT outer join [ISO/IEC 9075:1992 §7.5], each left
//! row no pair keeps with nulls. The right operand is read first, whole,
//! and its rows sorted by the values compared (`range/index.rs`); each left
//! row then searches them, its matches judged by the rest of the condition
//! in the order of the right's rows, so the rows come as the filter over
//! the product gives them.
//!
//! The right's rows and what sorts them are held in memory while they fit
//! the work area (RULES.md D38). Past it they are searched a part at a
//! time, the left read again for each part (`range/parts.rs`), so the rows
//! come part by part. If a value is of a kind the search cannot order, the
//! part's rows are read for each left row as a nested loop reads them, and
//! the whole condition judged of every pair, which gives the same rows.

mod index;
mod parts;
mod pruned;
mod sorting;

use std::sync::Arc;

use super::super::work::Work;
use super::super::{RowIter, Runner, broken, eval_cond, execute, failed};
use super::rescan::{Pass, Rescanned};
use crate::db::State;
use crate::error::Result;
use crate::logical::{Cond, Expr, Truth};
use crate::physical::{self, Bound, JoinKey, Plan, Pruned};
use crate::spill::row_bytes;
use crate::storage::Row;
use crate::txn::{TxId, checked};
use crate::value::Value;
use index::Index;
use parts::Parts;

/// The bytes held of each right row beyond the row: its sorted values, its
/// place, its tree's two nodes and what holds them.
const SORTED_BYTES: usize = 6 * size_of::<Value>() + 2 * size_of::<usize>();

/// What a range join searches by and judges (`physical::RangeJoin`).
pub(super) struct Spec {
    keys: Vec<JoinKey>,
    sorted: Expr,
    lower: Option<Bound>,
    upper: Option<Bound>,
    pruned: Option<Pruned>,
    cond: Option<Cond>,
    full: Cond,
}

/// What a left row's search found: the rows it matches, the rest of the
/// condition to judge; or rows it may match, the whole condition to judge.
pub(super) enum Searched {
    Found,
    Judged,
}

/// The rows of the range join `plan`. Never inlined into `execute`, whose
/// frame every level of a plan adds to the stack (RULES.md R-CF-6).
#[inline(never)]
pub(in crate::exec) fn ranged(plan: Plan, db: &Arc<State>, tx: TxId) -> RowIter {
    let Plan::RangeJoin(plan) = plan else { return failed(broken("a range join is not one")) };
    let physical::RangeJoin { outer, left, right, keys, sorted, lower, upper, pruned, cond, full, widths, .. } = *plan;
    let joining = Joining {
        left: execute(left, db, tx),
        right: Some(execute(right, db, tx)),
        spec: Spec { keys, sorted, lower, upper, pruned, cond, full },
        side: None,
        runner: Runner::new(db, tx),
        work: Work::of(db, tx),
        nulls: outer.then(|| vec![Value::Null; widths.1]),
        current: None,
        found: Vec::new(),
        parts: None,
    };
    Box::new(checked(joining, db.tx.stop_of(tx)))
}

/// The right operand as it is searched: sorted, or read again for each
/// left row.
enum Side {
    Indexed(Index),
    Nested(Rescanned),
}

/// The rows a left row is being paired with.
enum Matches {
    /// Places among the sorted rows, the next first.
    Places(std::vec::IntoIter<usize>),
    /// A pass of every right row.
    Pass(Pass),
}

/// The left row being paired, its matches, whether the whole condition is
/// judged of them, and whether a pair of it was kept.
struct Current {
    left: Row,
    matches: Matches,
    whole: bool,
    matched: bool,
}

/// A range join being read.
struct Joining {
    left: RowIter,
    /// The right operand, until it is read.
    right: Option<RowIter>,
    spec: Spec,
    side: Option<Side>,
    runner: Runner,
    work: Work,
    /// Of a LEFT join, the nulls a left row matching none is kept with.
    nulls: Option<Vec<Value>>,
    current: Option<Current>,
    /// The places a search finds, kept for the next.
    found: Vec<usize>,
    /// Past the work area, the right's parts still to search.
    parts: Option<Parts>,
}

impl Joining {
    /// The next row of the join, if any.
    fn step(&mut self) -> Result<Option<Row>> {
        if self.side.is_none() {
            self.side = Some(self.built()?);
        }
        loop {
            if let Some(row) = self.paired()? {
                return Ok(Some(row));
            }
            if let Some(Current { left, matched, .. }) = self.current.take() {
                if let Some(parts) = &mut self.parts {
                    parts.read(matched);
                } else if let (false, Some(nulls)) = (matched, &self.nulls) {
                    return Ok(Some(left.into_iter().chain(nulls.iter().cloned()).collect()));
                }
                continue;
            }
            let Some(left) = self.left.next().transpose()? else {
                if self.next_part()? {
                    continue;
                }
                return Ok(None);
            };
            if let Some(parts) = self.parts.as_mut().filter(|p| p.finishing) {
                let kept = parts.unmatched();
                parts.read(false);
                if let (true, Some(nulls)) = (kept, &self.nulls) {
                    return Ok(Some(left.into_iter().chain(nulls.iter().cloned()).collect()));
                }
                continue;
            }
            self.current = Some(self.started(left)?);
        }
    }

    /// Past the work area, the next part of the right sorted and the left
    /// read again to search it; or, the last part searched, of a LEFT join,
    /// the left read a last time for its rows no part matched. Whether
    /// there is more to read.
    fn next_part(&mut self) -> Result<bool> {
        let Some(parts) = self.parts.as_mut().filter(|p| !p.finishing) else { return Ok(false) };
        if let Some(rows) = parts.part(&self.work)? {
            self.side = Some(sorted(rows, &self.spec, &self.runner)?);
            self.left = parts.again()?;
            return Ok(true);
        }
        if self.nulls.is_none() {
            return Ok(false);
        }
        parts.finishing = true;
        self.left = parts.again()?;
        Ok(true)
    }

    /// The right operand read whole and sorted, if it fits the work area;
    /// else its first part, the rest and the left gathered (`Parts`).
    fn built(&mut self) -> Result<Side> {
        let Some(mut right) = self.right.take() else { return Err(broken("a range join's right operand was read twice")) };
        let (mut held, mut bytes) = (Vec::new(), 0_usize);
        while let Some(row) = right.next().transpose()? {
            bytes = bytes.saturating_add(row_bytes(&row)).saturating_add(SORTED_BYTES);
            held.push(row);
            if bytes > self.work.limit() {
                let left = std::mem::replace(&mut self.left, Box::new(std::iter::empty()));
                let (mut parts, first) = Parts::of((held, right), left, self.nulls.is_some(), &mut self.work)?;
                self.left = first;
                let rows = parts.part(&self.work)?.unwrap_or_default();
                self.parts = Some(parts);
                return sorted(rows, &self.spec, &self.runner);
            }
        }
        sorted(held, &self.spec, &self.runner)
    }

    /// `left` started: the rows it may match.
    fn started(&mut self, left: Row) -> Result<Current> {
        self.work.look()?;
        let (matches, whole) = match &mut self.side {
            Some(Side::Indexed(index)) => {
                let searched = index.search(&left, &self.spec, &self.runner, &mut self.found)?;
                let places = std::mem::take(&mut self.found);
                (Matches::Places(places.into_iter()), matches!(searched, Searched::Judged))
            }
            Some(Side::Nested(rows)) => (Matches::Pass(rows.pass()?), true),
            None => return Err(broken("a range join's right operand was not read")),
        };
        Ok(Current { left, matches, whole, matched: false })
    }

    /// The next pair of the left row being paired that is kept.
    fn paired(&mut self) -> Result<Option<Row>> {
        let Some(current) = &mut self.current else { return Ok(None) };
        let rows = match &self.side {
            Some(Side::Indexed(index)) => Some(index.rows()),
            Some(Side::Nested(_)) | None => None,
        };
        loop {
            // The pair made at once of a held row, not of a copy of it.
            let pair: Row = match &mut current.matches {
                Matches::Places(places) => match places.next() {
                    Some(at) => {
                        let right = rows.and_then(|r| r.get(at)).ok_or_else(|| broken("a range join's place is past its rows"))?;
                        current.left.iter().chain(right).cloned().collect()
                    }
                    None => return Ok(None),
                },
                Matches::Pass(pass) => match pass.next() {
                    Some(row) => current.left.iter().cloned().chain(row?).collect(),
                    None => return Ok(None),
                },
            };
            let judged = if current.whole { Some(&self.spec.full) } else { self.spec.cond.as_ref() };
            let kept = match judged {
                Some(c) => eval_cond(c, &pair, &self.runner)? == Truth::True,
                None => true,
            };
            if kept {
                current.matched = true;
                return Ok(Some(pair));
            }
        }
    }
}

/// The right rows `rows` sorted by `spec`, its values computed by
/// `runner`; else, if a value cannot be ordered, read for each left row.
///
/// # Errors
///
/// As computing a value fails.
fn sorted(rows: Vec<Row>, spec: &Spec, runner: &Runner) -> Result<Side> {
    let rows = Arc::new(rows);
    Ok(match Index::of(Arc::clone(&rows), spec, runner)? {
        Some(index) => Side::Indexed(index),
        None => Side::Nested(Rescanned::Held(rows)),
    })
}

impl Iterator for Joining {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        self.step().transpose()
    }
}

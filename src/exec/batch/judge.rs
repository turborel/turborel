//! \[DESIGN\] A filter over a stream's batches (RULES.md D31): the conjuncts
//! it can judge over a batch's vectors, by the rules a row's judgment uses
//! (`exec/eval.rs`), each keeping the rows it is true of; and the rest, judged
//! of the rows they keep, each made a row. Judged so: a column compared with a
//! constant; a column IS NULL or IS NOT NULL; a column IN, or NOT IN, a list
//! of constants; a column compared with a constant unpadded, as a collation
//! of NO PAD that orders strings as their octets do compares them, the
//! column's text read in place (RULES.md D1183); a column LIKE a constant pattern, split once, or LIKE a
//! column's pattern, or ESCAPE a column's character, each pattern split once
//! for the statement (`judge/likes.rs`); and any other conjunct of columns
//! the batch holds, no subquery in it, compiled once and evaluated a vector
//! at a time into truth values (`expr.rs`, RULES.md D362), as `A > B`, `-B >
//! C`, `B * 2 > C` and `CASE ... END = 1`. Of a filter that cannot raise,
//! such a conjunct whose evaluation gives none of a batch is judged a row at a
//! time of the rows kept. A filter that can raise an error is judged over
//! vectors only if all of it can be, each conjunct of every row given, so it
//! is judged of every row the filter row by row would judge it of; one of
//! whose conjuncts gives none of a batch, as one that would raise does, has
//! the batch judged whole a row at a time, so every row it would raise for
//! still does; and a filter of which some cannot be is judged so of every
//! batch. A
//! batch whose judgment raises an error is judged again a row at a time, in
//! order, to give the rows before its failing one first, as a filter read row
//! by row does, for an EXISTS that stops at one of them (RULES.md D15).
//!
//! \[DESIGN\] (RULES.md D1266) The first filter over a scan told its
//! conjuncts judges of each batch the rows the scan kept alone, and passes
//! over each conjunct the scan says it judged exactly of them
//! (`judge/vetted.rs`).

use super::expr::Program;
use super::project::{Place, place_of};
use super::vetted::Said;
use crate::error::{Result, SqlError};
use crate::exec::eval::compared;
use crate::exec::{Runner, Subqueries, eval_cond};
use crate::kernels::Kernels;
use likes::Operand;
use std::sync::{Arc, OnceLock};

use crate::logical::{Cond, Expr, InSet, Truth, compared_unpadded, compared_with_constant, in_list, told_conjuncts};
use crate::storage::mask::index;
use crate::storage::sel::rows_of;
use crate::storage::{Batch, Column, Mask, Sel, Vetted};
use crate::value::{CompOp, LikePattern, Value, constant_pattern};

pub(crate) use bound::bound_by;

/// A conjunct judged over a batch's vectors, of the column at a place of
/// the batch.
#[derive(Debug, Clone)]
pub(super) enum Judged {
    /// [X3.135-1986 §5.11] the column compared with a constant.
    Compare(usize, CompOp, Value),
    /// [X3.135-1986 §5.15] the column IS NULL, or, if the flag, IS NOT NULL.
    IsNull(usize, bool),
    /// [X3.135-1986 §5.13] the column IN the constants, or, if the flag,
    /// NOT IN them: a long list by its set, made once (`logical/sets.rs`).
    In(usize, Arc<InSet>, bool),
    /// [X3.135-1986 §5.14] the column LIKE a constant pattern, split once;
    /// none if a null pattern or escape makes it unknown of every row.
    Like(usize, Option<LikePattern>),
    /// [ISO/IEC 9075-2:2023 §4.3.2] (RULES.md D1183) the column compared
    /// with a constant's octets unpadded, a prefix the less.
    Unpadded(usize, CompOp, Vec<u8>),
    /// [X3.135-1986 §5.14] the column LIKE a pattern not a constant, a
    /// column's or a constant, ESCAPE one if given (`judge/likes.rs`).
    LikeOf(usize, Operand, Option<Operand>),
    /// \[DESIGN\] (RULES.md D362) any other conjunct, compiled for
    /// vectors, its truth values evaluated a vector at a time; and the
    /// conjunct, judged a row at a time of a batch of which that gives
    /// none.
    Truth(Arc<Program>, Box<Cond>),
    /// [X3.135-1986 §5.20 GR4] an OR of conjuncts judged over vectors, each
    /// judged of the same rows, a row kept if one keeps it (RULES.md D354).
    Any(Vec<Judged>),
}

impl Judged {
    /// The rows of `over`, rows of `batch`, or of all, it is true of,
    /// ascending: of integers, approximate numbers, datetimes, truth values
    /// and dictionary texts by a kernel of `kernels` (`judge/ints.rs`,
    /// `judge/kinds.rs`); of another conjunct by its program (`expr.rs`);
    /// none if that gives none, or it is a LIKE of a pattern not a constant
    /// that raises an error of one; such a LIKE matched by `subqueries`.
    fn kept(&self, batch: &Batch, over: Option<&[u32]>, subqueries: &dyn Subqueries, kernels: &dyn Kernels) -> Option<Vec<u32>> {
        let at = match self {
            Judged::Compare(at, ..) | Judged::IsNull(at, _) | Judged::In(at, ..) | Judged::Like(at, _) | Judged::Unpadded(at, ..) => *at,
            Judged::Any(disjuncts) => return kinds::any(disjuncts, batch, over, (subqueries, kernels)),
            Judged::Truth(program, _) => return program.kept(batch, over, (subqueries, kernels)),
            Judged::LikeOf(at, pattern, escape) => {
                return likes::kept((*at, pattern, escape.as_ref()), batch, &places(over, batch.rows), subqueries).map(rows_of);
            }
        };
        let column = batch.columns.get(at);
        let typed = match column {
            Some(Column::Ints(column)) => ints::kept(self, column, over, kernels),
            Some(Column::Floats(column)) => kinds::floats(self, column, over, kernels),
            Some(Column::Datetimes(column)) => kinds::datetimes(self, column, over, kernels),
            Some(Column::Bools(column)) => kinds::bools(self, column, over, kernels),
            Some(Column::Texts(column)) => texts::kept(self, column, over, kernels),
            // Of a column its scan measured, by its lengths where they tell it (RULES.md D1267).
            Some(Column::Late(late)) => measured::kept(self, late, over),
            Some(Column::Values(_) | Column::Nulls(_)) | None => None,
        };
        if typed.is_some() {
            return typed;
        }
        let vector = batch.values(at);
        let over = places(over, batch.rows);
        // A large object is read to be compared, which may fail: judged a
        // row at a time (`exec/eval.rs`).
        if over.iter().any(|&r| matches!(vector.get(r), Some(Value::Lob(_)))) {
            return None;
        }
        Some(rows_of(over.into_iter().filter(|&r| vector.get(r).is_some_and(|v| self.keeps(v)))))
    }

    /// Whether it is true of `v`.
    fn keeps(&self, v: &Value) -> bool {
        match self {
            Judged::Compare(_, op, constant) => compared(v, *op, constant) == Truth::True,
            // [X3.135-1986 §5.15 GR2-GR4] never unknown.
            Judged::IsNull(_, not) => v.is_null() != *not,
            // [X3.135-1986 §5.13 GR2, §5.18] an OR of equalities: true if one
            // is; NOT of it true only if every one is false.
            Judged::In(_, set, not) => set.truth(v) == wanted(*not),
            // [X3.135-1986 §5.14 GR4, GR5] unknown of a null.
            Judged::Like(_, pattern) => {
                if let (Some(pattern), Value::Char(s)) = (pattern, v) {
                    pattern.matches(s)
                } else {
                    false
                }
            }
            // [ISO/IEC 9075-2:2023 §4.3.2] unknown of a null.
            Judged::Unpadded(_, op, octets) => {
                if let Value::Char(s) = v {
                    op.holds(s.as_bytes().cmp(octets))
                } else {
                    false
                }
            }
            // Of two expressions, or of several, not of one value.
            Judged::Truth(..) | Judged::LikeOf(..) | Judged::Any(_) => false,
        }
    }
}

/// The places of the rows `over` keeps of a batch of `all` rows, or of
/// all.
fn places(over: Option<&[u32]>, all: usize) -> Vec<usize> {
    over.map_or_else(|| (0..all).collect(), |rows| rows.iter().map(|&r| index(r)).collect())
}

/// How a filter judges a scan's batches: its conjuncts judged over the
/// vectors; the rest of it judged a row at a time, each row made of the
/// columns the batches hold, as wide as the last of them.
#[derive(Clone)]
pub(in crate::exec) struct Judging {
    columns: Vec<usize>,
    judged: Vec<Judged>,
    rest: Option<Cond>,
    /// The filter whole, judged a row at a time of a batch of which a
    /// conjunct over vectors gives none.
    whole: Cond,
    /// Whether it can raise an error.
    raising: bool,
    width: usize,
    /// \[DESIGN\] (RULES.md D1127) Of a filter holding a scalar subquery
    /// that reads nothing of its rows, how the filter judges with each
    /// such subquery the constant it gives, made as its first row is
    /// judged (`judge/bound.rs`); none of a filter holding no such
    /// subquery.
    late: Option<OnceLock<Box<Judging>>>,
    /// \[DESIGN\] (RULES.md D1266) Of the first filter over a scan told
    /// its conjuncts, the bit of each of `judged` among those told
    /// (`placed`), in their order; of any other filter, none.
    told: Vec<u64>,
    /// What that scan says of each batch, of the stream this judges
    /// (`judge/vetted.rs`).
    said: Option<Arc<Said>>,
}

impl Judging {
    /// How `cond`, over a scan of `columns`, judges its batches.
    pub(super) fn of(cond: &Cond, columns: &[usize]) -> Judging {
        let (judged, rest) = split(cond, columns);
        let width = columns.iter().max().map_or(0, |&c| c.saturating_add(1));
        let late = bound::binds(cond).then(OnceLock::new);
        let (whole, raising) = (cond.clone(), cond.can_raise());
        Judging { columns: columns.to_vec(), judged, rest, whole, raising, width, late, told: Vec::new(), said: None }
    }

    /// \[DESIGN\] (RULES.md D1127) The filter with its subqueries of no
    /// outer reference made constants, held in `late`: made the first
    /// time it is asked for, as the filter's first row is judged.
    fn bound<'j>(&self, late: &'j OnceLock<Box<Judging>>, runner: &Runner) -> Result<&'j Judging> {
        if let Some(made) = late.get() {
            return Ok(made);
        }
        let cond = bound::bound(&self.whole, runner)?;
        Ok(late.get_or_init(|| Box::new(Judging::of(&cond, &self.columns))))
    }

    /// The rows of `batch` the filter keeps, of those at `rows`, which a
    /// filter before kept, or of all: those its conjuncts judged over
    /// vectors keep, then of those, those the rest keeps. A filter that
    /// cannot raise passes each conjunct the rows the one before kept, a
    /// selection chained (RULES.md D354); one that can judges each of every
    /// row given, as row by row it would be (RULES.md D15), the rows each
    /// keeps a mask, the masks joined.
    pub(super) fn kept_of(&self, batch: &Batch, rows: Option<&[u32]>, runner: &Runner) -> Result<Sel> {
        if let Some(late) = &self.late {
            // No row judged runs no subquery.
            if rows.map_or(batch.rows, <[u32]>::len) == 0 {
                return Ok(Sel::of(Vec::new()));
            }
            return self.bound(late, runner)?.kept_of(batch, rows, runner);
        }
        if self.raising {
            return self.kept_raising(batch, rows, runner);
        }
        let mut kept: Option<Vec<u32>> = None;
        // What the scan judged exactly of these rows is not judged again,
        // nor here what it judged of the first of them (RULES.md D1266).
        let heard = self.heard();
        for (n, judged) in self.judged.iter().enumerate() {
            if self.told.get(n).is_some_and(|&told| heard.passes(told)) {
                continue;
            }
            let over = kept.as_deref().or(rows);
            kept = Some(match judged.kept(batch, over, runner, runner.kernels()) {
                Some(true_of) => true_of,
                // None given over vectors: the conjunct judged a row at a
                // time of the rows kept; or the batch judged whole so.
                None => match judged {
                    Judged::Truth(_, conjunct) => self.by_rows(conjunct, batch, &places(over, batch.rows), runner)?,
                    Judged::Compare(..)
                    | Judged::IsNull(..)
                    | Judged::In(..)
                    | Judged::Like(..)
                    | Judged::LikeOf(..)
                    | Judged::Any(_)
                    | Judged::Unpadded(..) => {
                        return self.by_rows(&self.whole, batch, &places(rows, batch.rows), runner).map(Sel::of);
                    }
                },
            });
        }
        // The conjunct the scan judged of the batch's first rows alone,
        // judged of the rest (RULES.md D1266).
        if let Some((conjunct, from)) = self.partly_judged(heard) {
            let Some(with_rest) = conjunct.rest_kept(from, kept, (batch, rows), runner) else {
                return self.by_rows(&self.whole, batch, &places(rows, batch.rows), runner).map(Sel::of);
            };
            kept = Some(with_rest);
        }
        let kept = kept.map_or_else(|| places(rows, batch.rows), |k| k.iter().map(|&r| index(r)).collect());
        match &self.rest {
            Some(rest) => self.by_rows(rest, batch, &kept, runner).map(Sel::of),
            None => Ok(kept.into_iter().collect()),
        }
    }

    /// `kept_of` of a filter that can raise an error: each conjunct judged
    /// of every row given, the rows it keeps a mask, the masks joined.
    fn kept_raising(&self, batch: &Batch, rows: Option<&[u32]>, runner: &Runner) -> Result<Sel> {
        let mut all: Option<Mask> = None;
        for judged in &self.judged {
            let Some(true_of) = judged.kept(batch, rows, runner, runner.kernels()) else {
                return self.by_rows(&self.whole, batch, &places(rows, batch.rows), runner).map(Sel::of);
            };
            let mut mask = Mask::none(batch.rows);
            for &r in &true_of {
                mask.set(index(r));
            }
            all = Some(all.map_or(mask.clone(), |held| held.and(&mask)));
        }
        let mut out = Vec::new();
        match &all {
            Some(mask) => runner.kernels().select_mask(mask.words(), (None, rows), &mut out),
            None => out.extend(rows.map_or_else(|| rows_of(0..batch.rows), <[u32]>::to_vec)),
        }
        match &self.rest {
            Some(rest) => self.by_rows(rest, batch, &places(Some(&out), batch.rows), runner).map(Sel::of),
            None => Ok(Sel::of(out)),
        }
    }

    /// The rows of `batch` at `rows` that `cond` is true of, each made a
    /// row and judged as one.
    fn by_rows(&self, cond: &Cond, batch: &Batch, rows: &[usize], runner: &Runner) -> Result<Vec<u32>> {
        let (kept, failed) = self.in_order(cond, batch, rows, runner);
        failed.map_or(Ok(rows_of(kept)), Err)
    }

    /// The rows of `batch` at `rows`, or of all, the whole filter keeps,
    /// judged a row at a time in order up to the first that raises an
    /// error, and that error: what the filter gives read row by row. A
    /// batch whose judgment failed is judged again so, and gives the rows
    /// before its failing one before the failure, so that a reader that
    /// stops at one of them, as EXISTS does (RULES.md D15), sees the error
    /// as it would row by row: not at all.
    pub(super) fn in_order_of(&self, batch: &Batch, rows: Option<&[u32]>, runner: &Runner) -> (Sel, Option<SqlError>) {
        // A subquery that raises an error is run by the first row judged,
        // which then raises it among its own, in their order.
        let judges = rows.map_or(batch.rows, <[u32]>::len) > 0;
        if let Some(late) = self.late.as_ref().filter(|_| judges)
            && let Ok(made) = self.bound(late, runner)
        {
            return made.in_order_of(batch, rows, runner);
        }
        let (kept, failed) = self.in_order(&self.whole, batch, &places(rows, batch.rows), runner);
        (kept.into_iter().collect(), failed)
    }

    /// The rows of `batch` at `rows` that `cond` is true of, judged in
    /// order up to the first that raises an error, and that error.
    fn in_order(&self, cond: &Cond, batch: &Batch, rows: &[usize], runner: &Runner) -> (Vec<usize>, Option<SqlError>) {
        if let Some(call) = calls::Call::of(cond, &self.columns) {
            return call.in_order(batch, rows, runner.temp_lobs());
        }
        let mut row = vec![Value::Null; self.width];
        let mut out = Vec::with_capacity(rows.len());
        for &r in rows {
            // Each cell of the row alone made a value (RULES.md D367).
            for (at, &c) in self.columns.iter().enumerate().take(batch.columns.len()) {
                if let Some(slot) = row.get_mut(c) {
                    *slot = batch.cell(at, r).to_value();
                }
            }
            match eval_cond(cond, &row, runner) {
                Ok(Truth::True) => out.push(r),
                Ok(Truth::False | Truth::Unknown) => {}
                Err(e) => return (out, Some(e)),
            }
        }
        (out, None)
    }
}

/// The conjuncts of `cond` judged over a batch of `columns`, and the rest,
/// judged a row at a time: all of it if it can raise an error and not all
/// of it is judged over vectors, as every row must see raised what it
/// would be.
fn split(cond: &Cond, columns: &[usize]) -> (Vec<Judged>, Option<Cond>) {
    let (judged, rest) = placed(cond, columns);
    (judged.into_iter().map(|(conjunct, _)| conjunct).collect(), rest)
}

/// \[DESIGN\] (RULES.md D1266) As `split`, each conjunct judged over
/// vectors with the bit (`Vetted::bit`) of the conjunct a scan told
/// `cond`'s is told that is the conjunct itself
/// (`logical::told_conjuncts`), none set if none is. Each is paired with
/// its bit as it is gathered, before they are put in order, so whatever
/// orders them keeps each with its own.
fn placed(cond: &Cond, columns: &[usize]) -> (Vec<(Judged, u64)>, Option<Cond>) {
    let (mut judged, mut rest) = (Vec::new(), Vec::new());
    for (conjunct, told) in told_conjuncts(cond) {
        let mut found = Vec::new();
        gather(conjunct, columns, (&mut found, &mut rest));
        judged.extend(found.into_iter().map(|j| (j, told.map_or(0, Vetted::bit))));
    }
    if cond.can_raise() && !rest.is_empty() {
        return (Vec::new(), Some(cond.clone()));
    }
    // A LIKE last: it costs more a row than the rest, which may leave it
    // fewer.
    judged.sort_by_key(|(j, _)| matches!(j, Judged::Like(..) | Judged::LikeOf(..)));
    (judged, rest.into_iter().reduce(|a, b| Cond::And(Box::new(a), Box::new(b))))
}

/// Gathers the conjuncts of `cond` into those `split` judges over vectors
/// and the rest.
fn gather(cond: &Cond, columns: &[usize], (judged, rest): (&mut Vec<Judged>, &mut Vec<Cond>)) {
    let place = |c: usize| columns.iter().position(|&x| x == c);
    let found = match cond {
        Cond::And(a, b) => {
            gather(a, columns, (&mut *judged, &mut *rest));
            gather(b, columns, (judged, rest));
            return;
        }
        Cond::IsNull(Expr::Column(c), not) => place(*c).map(|at| Judged::IsNull(at, *not)),
        Cond::Or(..) => listed(cond)
            .and_then(|(c, set)| Some(Judged::In(place(c)?, set, false)))
            .or_else(|| disjoined(cond, columns))
            .or_else(|| vectored(cond, columns)),
        Cond::In(..) => listed(cond).and_then(|(c, set)| Some(Judged::In(place(c)?, set, false))).or_else(|| vectored(cond, columns)),
        Cond::Not(inner) => listed(inner).and_then(|(c, set)| Some(Judged::In(place(c)?, set, true))).or_else(|| vectored(cond, columns)),
        Cond::Like(Expr::Column(c), pattern, escape) => liked((*c, pattern, escape.as_ref()), columns).or_else(|| vectored(cond, columns)),
        Cond::Compare(..)
        | Cond::Distinct(..)
        | Cond::Const(_)
        | Cond::IsNull(..)
        | Cond::Is(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(..)
        | Cond::Quantified(..)
        | Cond::Match { .. }
        | Cond::Value(_) => compared_with_constant(cond)
            .and_then(|(c, op, v)| Some(Judged::Compare(place(c)?, op, v.clone())))
            .or_else(|| compared_unpadded(cond).and_then(|(c, op, octets)| Some(Judged::Unpadded(place(c)?, op, octets.to_vec()))))
            .or_else(|| vectored(cond, columns)),
    };
    match found {
        Some(j) => judged.push(j),
        None => rest.push(cond.clone()),
    }
}

/// \[DESIGN\] (RULES.md D354) The OR `cond` judged over vectors, if each of
/// its disjuncts is one conjunct judged so: its disjuncts gathered on a
/// stack of their own, as an OR is as long as its statement's (RULES.md
/// R-CF-6).
fn disjoined(cond: &Cond, columns: &[usize]) -> Option<Judged> {
    let (mut stack, mut disjuncts) = (vec![cond], Vec::new());
    while let Some(at) = stack.pop() {
        if let Cond::Or(a, b) = at {
            stack.push(b);
            stack.push(a);
            continue;
        }
        let (mut judged, mut rest) = (Vec::new(), Vec::new());
        gather(at, columns, (&mut judged, &mut rest));
        let ([one], true) = (judged.as_mut_slice(), rest.is_empty()) else { return None };
        // One of a program: the OR whole is one program, its disjuncts
        // its nodes.
        if matches!(one, Judged::Truth(..)) {
            return None;
        }
        disjuncts.push(std::mem::replace(one, Judged::Any(Vec::new())));
    }
    Some(Judged::Any(disjuncts))
}

/// Of a LIKE of column `c`, `pattern`, `escape`, how it is judged over a
/// batch of `columns`: of a constant pattern and escape, if any, split
/// once; of a column's or a constant, matched a row at a time; else none.
fn liked((c, pattern, escape): (usize, &Expr, Option<&Expr>), columns: &[usize]) -> Option<Judged> {
    let place = columns.iter().position(|&x| x == c)?;
    let constant = match escape {
        None => Some(None),
        Some(Expr::Const(e)) => Some(Some(e)),
        Some(_) => None,
    };
    // Constants that may raise, with no partitioning, are rest: judged a
    // row at a time, as every batch would be.
    if let (Expr::Const(p), Some(e)) = (pattern, constant) {
        return constant_pattern(p, e).map(|split| Judged::Like(place, split));
    }
    let operand = |e: &Expr| match place_of(e, columns)? {
        Place::Column(at) => Some(Operand::At(at)),
        Place::Const(v) => Some(Operand::Const(v)),
        Place::Expr(_) => None,
    };
    let escape = match escape {
        Some(e) => Some(operand(e)?),
        None => None,
    };
    Some(Judged::LikeOf(place, operand(pattern)?, escape))
}

/// The column and constants of `cond`, if it is IN a list of them: its
/// set, or one made of the OR of their equalities.
fn listed(cond: &Cond) -> Option<(usize, Arc<InSet>)> {
    if let Cond::In(Expr::Column(c), set) = cond {
        return Some((*c, Arc::clone(set)));
    }
    in_list(cond).map(|(c, list)| (c, Arc::new(InSet::of(list))))
}

/// The truth an IN keeps of a row: true, or, if `not`, false, as NOT of
/// it is then true.
fn wanted(not: bool) -> Truth {
    if not { Truth::False } else { Truth::True }
}

/// \[DESIGN\] (RULES.md D362) `cond` compiled for vectors, if it reads
/// only columns of `columns`, no subquery nor outer reference in it, and
/// its program's root is evaluated over vectors.
fn vectored(cond: &Cond, columns: &[usize]) -> Option<Judged> {
    let Place::Expr(program) = place_of(&Expr::Truth(Box::new(cond.clone())), columns)? else { return None };
    program.vectored().then(|| Judged::Truth(program, Box::new(cond.clone())))
}

// Unit tests: which conjuncts of a filter are judged over vectors, and
// what each keeps (RULES.md R-TEST-1).
mod bound;
mod calls;
mod ints;
mod kinds;
mod likes;
mod measured;
mod texts;
mod vetted;

#[cfg(test)]
mod tests;

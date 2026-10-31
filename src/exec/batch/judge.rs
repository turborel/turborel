//! \[DESIGN\] A filter over a stream's batches (RULES.md D31): the conjuncts
//! it can judge over a batch's vectors, by the rules a row's judgment uses
//! (`exec/eval.rs`), each keeping the rows it is true of; and the rest,
//! judged of the rows they keep, each made a row. Judged so: a column
//! compared with a constant; a column IS NULL or IS NOT NULL; a column IN,
//! or NOT IN, a list of constants; a column LIKE a constant pattern, split
//! once; of a filter that can raise an error, two expressions of columns
//! and exact constants compared, each evaluated as integers
//! (`vector/ints.rs`). A filter that can raise an
//! error is judged over vectors only if all of it can be, each conjunct of
//! every row given, so it is judged of every row the filter row by row
//! would judge it of; one of whose expressions no integers are made, as
//! of a column of values or a result past 18 digits, has the batch judged
//! whole a row at a time, so every row it would raise for still does; and
//! a filter of which some cannot be is judged so of every batch. A batch
//! whose judgment raises an error is judged again a row at a time, in
//! order, to give the rows before its failing one first, as a filter read
//! row by row does, for an EXISTS that stops at one of them (RULES.md
//! D15). Split from batch.rs (RULES.md R-SIZE-2).

use super::project::{Place, place_of};
use super::vector::ints::evaluable;
use crate::error::{Result, SqlError};
use crate::exec::eval::compared;
use crate::exec::{Runner, eval_cond};
use std::sync::Arc;

use crate::logical::{Cond, Expr, InSet, Truth, compared_with_constant, in_list};
use crate::storage::{Batch, Column};
use crate::value::{CompOp, LikePattern, Value, constant_pattern};

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
    /// [X3.135-1986 §5.11] two expressions compared, each of a row of the
    /// values at its places, evaluated as integers.
    Compared(Box<Side>, CompOp, Box<Side>),
}

/// An expression a comparison judged over vectors compares, of a row of
/// the values at its places.
pub(super) type Side = (Expr, Vec<usize>);

impl Judged {
    /// The rows of `over`, rows of `batch`, it is true of: of integers, as
    /// integers (`judge/ints.rs`); none if it compares expressions whose
    /// integers are not made.
    fn kept(&self, batch: &Batch, over: &[usize]) -> Option<Vec<usize>> {
        let at = match self {
            Judged::Compare(at, ..) | Judged::IsNull(at, _) | Judged::In(at, ..) | Judged::Like(at, _) => *at,
            Judged::Compared(a, op, b) => return ints::compared((a, *op, b), batch, over),
        };
        let typed = match batch.columns.get(at) {
            Some(Column::Ints(column)) => ints::kept(self, column, over),
            Some(Column::Floats(column)) => floats::kept(self, column, over),
            Some(Column::Texts(column)) => texts::kept(self, column, over),
            Some(Column::Values(_)) | None => None,
        };
        if typed.is_some() {
            return typed;
        }
        let vector = batch.values(at);
        Some(over.iter().copied().filter(|&r| vector.get(r).is_some_and(|v| self.keeps(v))).collect())
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
            // Of two expressions, not of one value.
            Judged::Compared(..) => false,
        }
    }
}

/// How a filter judges a scan's batches: its conjuncts judged over the
/// vectors; the rest of it judged a row at a time, each row made of the
/// columns the batches hold, as wide as the last of them.
pub(super) struct Judging {
    columns: Vec<usize>,
    judged: Vec<Judged>,
    rest: Option<Cond>,
    /// The filter whole, judged a row at a time of a batch whose integers
    /// a comparison of expressions cannot make.
    whole: Cond,
    /// Whether it can raise an error.
    raising: bool,
    width: usize,
}

impl Judging {
    /// How `cond`, over a scan of `columns`, judges its batches.
    pub(super) fn of(cond: &Cond, columns: &[usize]) -> Judging {
        let (judged, rest) = split(cond, columns);
        let width = columns.iter().max().map_or(0, |&c| c.saturating_add(1));
        Judging { columns: columns.to_vec(), judged, rest, whole: cond.clone(), raising: cond.can_raise(), width }
    }

    /// The rows of `batch` the filter keeps, of those at `rows`, which a
    /// filter before kept, or of all: those its conjuncts judged over
    /// vectors keep, then of those, those the rest keeps.
    pub(super) fn kept_of(&self, batch: &Batch, rows: Option<&[usize]>, runner: &Runner) -> Result<Vec<usize>> {
        let given: Vec<usize> = rows.map_or_else(|| (0..batch.rows).collect(), <[usize]>::to_vec);
        let mut kept = given.clone();
        for judged in &self.judged {
            // Of a filter that can raise, each conjunct of every row given.
            let over = if self.raising { &given } else { &kept };
            let Some(true_of) = judged.kept(batch, over) else {
                // Integers not made: the batch judged whole, row by row.
                return self.by_rows(&self.whole, batch, &given, runner);
            };
            kept = if self.raising { both(&kept, &true_of) } else { true_of };
        }
        match &self.rest {
            Some(rest) => self.by_rows(rest, batch, &kept, runner),
            None => Ok(kept),
        }
    }

    /// The rows of `batch` at `rows` that `cond` is true of, each made a
    /// row and judged as one.
    fn by_rows(&self, cond: &Cond, batch: &Batch, rows: &[usize], runner: &Runner) -> Result<Vec<usize>> {
        let (kept, failed) = self.in_order(cond, batch, rows, runner);
        failed.map_or(Ok(kept), Err)
    }

    /// The rows of `batch` at `rows`, or of all, the whole filter keeps,
    /// judged a row at a time in order up to the first that raises an
    /// error, and that error: what the filter gives read row by row. A
    /// batch whose judgment failed is judged again so, and gives the rows
    /// before its failing one before the failure, so that a reader that
    /// stops at one of them, as EXISTS does (RULES.md D15), sees the error
    /// as it would row by row: not at all.
    pub(super) fn in_order_of(&self, batch: &Batch, rows: Option<&[usize]>, runner: &Runner) -> (Vec<usize>, Option<SqlError>) {
        let given: Vec<usize> = rows.map_or_else(|| (0..batch.rows).collect(), <[usize]>::to_vec);
        self.in_order(&self.whole, batch, &given, runner)
    }

    /// The rows of `batch` at `rows` that `cond` is true of, judged in
    /// order up to the first that raises an error, and that error.
    fn in_order(&self, cond: &Cond, batch: &Batch, rows: &[usize], runner: &Runner) -> (Vec<usize>, Option<SqlError>) {
        let mut row = vec![Value::Null; self.width];
        let mut out = Vec::with_capacity(rows.len());
        for &r in rows {
            for (vector, &c) in batch.columns.iter().zip(&self.columns) {
                if let (Some(slot), Some(v)) = (row.get_mut(c), vector.values().get(r)) {
                    slot.clone_from(v);
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

/// The rows of sorted `kept` also in sorted `true_of`.
fn both(kept: &[usize], true_of: &[usize]) -> Vec<usize> {
    kept.iter().copied().filter(|r| true_of.binary_search(r).is_ok()).collect()
}

/// The conjuncts of `cond` judged over a batch of `columns`, and the rest,
/// judged a row at a time: all of it if it can raise an error and not all
/// of it is judged over vectors, as every row must see raised what it
/// would be.
fn split(cond: &Cond, columns: &[usize]) -> (Vec<Judged>, Option<Cond>) {
    let (mut judged, mut rest) = (Vec::new(), Vec::new());
    gather(cond, (columns, cond.can_raise()), (&mut judged, &mut rest));
    if cond.can_raise() && !rest.is_empty() {
        return (Vec::new(), Some(cond.clone()));
    }
    // A LIKE last: it costs more a row than the rest, which may leave it
    // fewer.
    judged.sort_by_key(|j| matches!(j, Judged::Like(..)));
    (judged, rest.into_iter().reduce(|a, b| Cond::And(Box::new(a), Box::new(b))))
}

/// Gathers the conjuncts of `cond` into those `split` judges over vectors
/// and the rest; comparisons of expressions among the first only of a
/// filter that can raise, which it is `raising`.
fn gather(cond: &Cond, (columns, raising): (&[usize], bool), (judged, rest): (&mut Vec<Judged>, &mut Vec<Cond>)) {
    let place = |c: usize| columns.iter().position(|&x| x == c);
    let found = match cond {
        Cond::And(a, b) => {
            gather(a, (columns, raising), (&mut *judged, &mut *rest));
            gather(b, (columns, raising), (judged, rest));
            return;
        }
        Cond::IsNull(Expr::Column(c), not) => place(*c).map(|at| Judged::IsNull(at, *not)),
        Cond::Or(..) | Cond::In(..) => listed(cond).and_then(|(c, set)| Some(Judged::In(place(c)?, set, false))),
        Cond::Not(inner) => listed(inner).and_then(|(c, set)| Some(Judged::In(place(c)?, set, true))),
        Cond::Like(Expr::Column(c), Expr::Const(pattern), escape) => {
            let escape = match escape {
                None => Some(None),
                Some(Expr::Const(e)) => Some(Some(e)),
                Some(_) => None,
            };
            escape.and_then(|e| constant_pattern(pattern, e)).and_then(|p| Some(Judged::Like(place(*c)?, p)))
        }
        Cond::Compare(..)
        | Cond::Const(_)
        | Cond::IsNull(..)
        | Cond::Is(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(_)
        | Cond::Quantified(..)
        | Cond::Match { .. } => compared_with_constant(cond)
            .and_then(|(c, op, v)| Some(Judged::Compare(place(c)?, op, v.clone())))
            // Of a filter that can raise, else judged a row at a time of
            // the rows the rest keep, as one that cannot raise needs no
            // more.
            .or_else(|| raising.then(|| compared_of_integers(cond, columns)).flatten()),
    };
    match found {
        Some(j) => judged.push(j),
        None => rest.push(cond.clone()),
    }
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

/// `cond`, if it compares two expressions of columns of `columns` and exact
/// constants that may be evaluated as integers (`vector/ints.rs`).
fn compared_of_integers(cond: &Cond, columns: &[usize]) -> Option<Judged> {
    let Cond::Compare(a, op, b) = cond else { return None };
    let side = |e: &Expr| {
        let made = match place_of(e, columns)? {
            Place::Column(at) => (Expr::Column(0), vec![at]),
            Place::Const(v) => (Expr::Const(v), Vec::new()),
            Place::Expr(own, inputs) => (own, inputs),
        };
        evaluable(&made.0).then(|| Box::new(made))
    };
    Some(Judged::Compared(side(a)?, *op, side(b)?))
}

// Unit tests: which conjuncts of a filter are judged over vectors, and
// what each keeps (RULES.md R-TEST-1).
mod floats;
mod ints;
mod texts;

#[cfg(test)]
mod tests;

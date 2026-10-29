//! \[DESIGN\] A filter over a scan's batches (RULES.md D31): the conjuncts
//! it can judge over a batch's vectors, by the rules a row's judgment uses
//! (`exec/eval.rs`), each keeping the rows it is true of; and the rest,
//! judged of the rows they keep, each made a row. Judged so: a column
//! compared with a constant; a column IS NULL or IS NOT NULL; a column IN,
//! or NOT IN, a list of constants; a column LIKE a constant pattern, split
//! once. A filter that can raise an error is
//! judged whole a row at a time, so every row it would raise for still
//! does. Split from batch.rs (RULES.md R-SIZE-2).

use crate::error::Result;
use crate::exec::eval::compared;
use crate::exec::{Runner, eval_cond};
use crate::logical::{Cond, Expr, Truth, compared_with_constant, in_list};
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
    /// NOT IN them.
    In(usize, Vec<Value>, bool),
    /// [X3.135-1986 §5.14] the column LIKE a constant pattern, split once;
    /// none if a null pattern or escape makes it unknown of every row.
    Like(usize, Option<LikePattern>),
}

impl Judged {
    /// Its column's place in a batch.
    fn place(&self) -> usize {
        match self {
            Judged::Compare(at, ..) | Judged::IsNull(at, _) | Judged::In(at, ..) | Judged::Like(at, _) => *at,
        }
    }

    /// Whether it is true of `v`.
    fn keeps(&self, v: &Value) -> bool {
        match self {
            Judged::Compare(_, op, constant) => compared(v, *op, constant) == Truth::True,
            // [X3.135-1986 §5.15 GR2-GR4] never unknown.
            Judged::IsNull(_, not) => v.is_null() != *not,
            // [X3.135-1986 §5.13 GR2, §5.18] an OR of equalities: true if one
            // is; NOT of it true only if every one is false.
            Judged::In(_, list, false) => list.iter().any(|c| compared(v, CompOp::Eq, c) == Truth::True),
            Judged::In(_, list, true) => list.iter().all(|c| compared(v, CompOp::Eq, c) == Truth::False),
            // [X3.135-1986 §5.14 GR4, GR5] unknown of a null.
            Judged::Like(_, pattern) => {
                if let (Some(pattern), Value::Char(s)) = (pattern, v) {
                    pattern.matches(s)
                } else {
                    false
                }
            }
        }
    }
}

/// How a filter judges a scan's batches: its conjuncts judged over the
/// vectors; the rest of it judged a row at a time, each row made of the
/// scan's columns, as wide as the table.
pub(super) struct Judging {
    columns: Vec<usize>,
    judged: Vec<Judged>,
    rest: Option<Cond>,
    width: usize,
}

impl Judging {
    /// How `cond`, over a scan of `columns`, judges its batches.
    pub(super) fn of(cond: &Cond, columns: &[usize]) -> Judging {
        let (judged, rest) = split(cond, columns);
        let width = columns.iter().max().map_or(0, |&c| c.saturating_add(1));
        Judging { columns: columns.to_vec(), judged, rest, width }
    }

    /// The rows of `batch` the filter keeps, of those at `rows`, which a
    /// filter before kept, or of all: those its conjuncts judged over
    /// vectors keep, then of those, those the rest keeps.
    pub(super) fn kept_of(&self, batch: &Batch, rows: Option<&[usize]>, runner: &Runner) -> Result<Vec<usize>> {
        let mut kept: Vec<usize> = rows.map_or_else(|| (0..batch.rows).collect(), <[usize]>::to_vec);
        for judged in &self.judged {
            // Of integers, judged as integers (`judge/ints.rs`).
            if let Some(Column::Ints(column)) = batch.columns.get(judged.place())
                && let Some(true_of) = ints::kept(judged, column, &kept)
            {
                kept = true_of;
                continue;
            }
            let vector = batch.values(judged.place());
            kept.retain(|&r| vector.get(r).is_some_and(|v| judged.keeps(v)));
        }
        let Some(rest) = &self.rest else { return Ok(kept) };
        let mut row = vec![Value::Null; self.width];
        let mut out = Vec::with_capacity(kept.len());
        for r in kept {
            for (vector, &c) in batch.columns.iter().zip(&self.columns) {
                if let (Some(slot), Some(v)) = (row.get_mut(c), vector.values().get(r)) {
                    slot.clone_from(v);
                }
            }
            if eval_cond(rest, &row, runner)? == Truth::True {
                out.push(r);
            }
        }
        Ok(out)
    }
}

/// The conjuncts of `cond` judged over a batch of `columns`, and the rest,
/// judged a row at a time: all of it if it can raise an error, which every
/// row must see raised as it would be (`simple_conjuncts`).
fn split(cond: &Cond, columns: &[usize]) -> (Vec<Judged>, Option<Cond>) {
    if cond.can_raise() {
        return (Vec::new(), Some(cond.clone()));
    }
    let (mut judged, mut rest) = (Vec::new(), Vec::new());
    gather(cond, columns, (&mut judged, &mut rest));
    // A LIKE last: it costs more a row than the rest, which may leave it
    // fewer (`S LIKE '%5%' AND G = 3` of 400,000 rows: 29.7 ms LIKE first).
    judged.sort_by_key(|j| matches!(j, Judged::Like(..)));
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
        Cond::Or(..) => in_list(cond).and_then(|(c, list)| Some(Judged::In(place(c)?, list, false))),
        Cond::Not(inner) => in_list(inner).and_then(|(c, list)| Some(Judged::In(place(c)?, list, true))),
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
        | Cond::Match { .. } => compared_with_constant(cond).and_then(|(c, op, v)| Some(Judged::Compare(place(c)?, op, v.clone()))),
    };
    match found {
        Some(j) => judged.push(j),
        None => rest.push(cond.clone()),
    }
}

// Unit tests: which conjuncts of a filter are judged over vectors, and
// what each keeps (RULES.md R-TEST-1).
mod ints;

#[cfg(test)]
mod tests;

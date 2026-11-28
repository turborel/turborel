//! \[DESIGN\] (RULES.md D361) Which columns of a stream's scans are given
//! late, and where they are decoded. A column is late if no operator of
//! the stream reads it before the stream is read whole, and an operator
//! that drops rows lies between it and that read: a filter, or an inner
//! hash join of whose left operand it is. Such a column is carried as the
//! places of its rows (`storage::Late`), passed through a projection of
//! columns and gathered through the join as places, and decoded as the
//! stream is read (`resolved`), of the rows kept alone. A column read on
//! the way, by a condition, a join's keys or a projection's expressions,
//! is decoded as before, and so is every column of a stream with no
//! operator that drops rows, as Abadi, Myers, DeWitt and Madden (ICDE
//! 2007) find early materialization faster where most rows are kept.
//!
//! \[DESIGN\] (RULES.md D1267) A column whose text no operator of the
//! stream reads, nor whoever reads the stream, is asked of its scan
//! measured (`measured`). Its only readers take its octet length, by
//! OCTET_LENGTH of the column or by comparing it unpadded with the empty
//! string, and the scan gives it late with its lengths beside its
//! places (`storage::Late::measured`). It is never decoded, whether an
//! operator drops rows or not.
//!
//! What a column store's scan decodes late is what it would have decoded
//! then, as its groups are replaced, never changed, and a batch holds the
//! group it reads (`rows/frozen/late.rs`). A damaged page is reported as
//! the column is decoded, later than the scan would have, which only an
//! error of the stored data, never one of the data's values, can be.

use std::sync::Arc;

use super::runtime::Told;
use super::stream::{Batches, Stream, batches_with, layout};
use crate::error::Result;
use crate::exec::Db;
use crate::lexer::kw;
use crate::logical::{AggCall, Cond, Expr, compared_unpadded, cond_columns, expr_columns};
use crate::physical::{JoinKey, Plan};
use crate::storage::{Conjunct, Threshold};
use crate::txn::TxId;

/// Of a top-N, its first key's column and the bound it raises, told the
/// scan (RULES.md D379).
pub(in crate::exec) type Bound = Option<(usize, Arc<Threshold>)>;

/// \[DESIGN\] (RULES.md D361) What the operators above a subtree of a
/// stream leave of its rows' columns unread before the stream is read
/// whole, and whether one of them drops rows; and the runtime filters a
/// hash join above tells it (RULES.md D369, `runtime.rs`).
#[derive(Debug, Clone, Default)]
pub(in crate::exec) struct Deferred {
    /// Whether an operator above drops rows.
    reduced: bool,
    /// The columns of the subtree's rows none above reads, ascending.
    unread: Vec<usize>,
    /// The runtime filters told it, each of a column of its rows.
    told: Vec<Told>,
    /// Of a sort of which only the first rows are asked for, its first
    /// key's column of the subtree's rows and the bound it raises, told
    /// the scan (RULES.md D379).
    bound: Bound,
    /// \[DESIGN\] (RULES.md D1221) Whether what reads the subtree finds
    /// its rows by a key no two of them share, so the order they are
    /// given in shows in nothing it gives. A projection of columns hands
    /// it on to its input, and no other operator does.
    any_order: bool,
    /// \[DESIGN\] (RULES.md D1267) The columns of the subtree's rows
    /// whose text none above reads, ascending. A scan gives them
    /// measured.
    measured: Vec<usize>,
}

impl Deferred {
    /// Of a stream read whole, its rows `width` columns wide: no column read
    /// yet, no row dropped.
    pub(in crate::exec) fn all(width: usize) -> Deferred {
        Deferred { reduced: false, unread: (0..width).collect(), ..Deferred::default() }
    }

    /// \[DESIGN\] (RULES.md D379, D380) Of a sort's input of which only the
    /// first rows are asked for, `keys` the columns its keys read: those
    /// read, rows dropped, as the sort keeps those rows alone; and the
    /// bound of its first key, `bound`, told the scan if given.
    pub(in crate::exec) fn topped(&self, keys: &[usize], bound: Bound) -> Deferred {
        Deferred { reduced: true, unread: without(&self.unread, keys), bound, ..Deferred::default() }
    }

    /// \[DESIGN\] (RULES.md D1267) It, of a reader that reads the text
    /// of the columns `read` alone of the subtree's rows, `width` wide.
    /// Every other column it reads for its octet length or not at all,
    /// so a scan may give it measured.
    pub(in crate::exec) fn measuring(self, read: &[usize], width: usize) -> Deferred {
        Deferred { measured: without(&(0..width).collect::<Vec<_>>(), read), ..self }
    }

    /// Whether it leaves any column unread or measured, or tells a bound.
    pub(in crate::exec) fn any(&self) -> bool {
        !self.unread.is_empty() || self.bound.is_some() || !self.measured.is_empty()
    }

    /// The conjunct of the bound it tells the scan, if any.
    pub(in crate::exec) fn told(&self) -> Option<Conjunct> {
        self.bound.as_ref().map(|(column, threshold)| Conjunct::Dynamic { column: *column, threshold: Arc::clone(threshold) })
    }

    /// Of a filter's input, `cond` the filter's condition: the columns it
    /// reads read, rows dropped, a bound kept unless it can raise. A
    /// condition with a subquery is judged of rows made of every column,
    /// so it reads them all.
    pub(in crate::exec) fn filtered(&self, cond: &Cond) -> Deferred {
        if cond.has_subquery() {
            return Deferred::default();
        }
        let mut read = Vec::new();
        cond_columns(cond, &mut read);
        // A condition that can raise sees every row, so no group is passed
        // over under it.
        let bound = self.bound.clone().filter(|_| !cond.can_raise());
        // A column the condition reads the text of is measured no more (RULES.md D1267).
        let measured = without(&self.measured, &read_as_text(&[], &[cond]));
        Deferred { reduced: true, unread: without(&self.unread, &read), bound, measured, ..Deferred::default() }
    }

    /// \[DESIGN\] (RULES.md D1063) Of the input of runtime filters judged
    /// of its batches, `columns` those they judge: those read, rows
    /// dropped, as a filter keeps only rows its join may match, so the
    /// columns no step reads before are decoded of those rows alone.
    pub(in crate::exec) fn judged(&self, columns: &[usize]) -> Deferred {
        Deferred { reduced: true, unread: without(&self.unread, columns), bound: self.bound.clone(), ..Deferred::default() }
    }

    /// Of a projection's input, `exprs` its expressions: a column one of
    /// them is, left unread if the projection's column is, as it is moved
    /// (`stream::pick`); every column any other reads read. A projection
    /// not of columns alone reads every column of a batch a filter kept
    /// rows of (`project::computed`), so reads them all.
    pub(in crate::exec) fn projected(&self, exprs: &[Expr]) -> Deferred {
        // A column moved, or the null value, which reads none (RULES.md D1167).
        let moved: Option<Vec<Option<usize>>> = exprs
            .iter()
            .map(|e| {
                if let Expr::Column(c) = e {
                    Some(Some(*c))
                } else if let Expr::Const(v) = e {
                    v.is_null().then_some(None)
                } else {
                    None
                }
            })
            .collect();
        let Some(moved) = moved else {
            // Every column is read, but the bound still reaches the scan,
            // by the column it is of, under expressions that raise
            // nothing: a group the bound passes over holds no row of the
            // first, so none whose expression the sort would see (RULES.md
            // D1183, a sort key made beside its rows' columns).
            let quiet = !exprs.iter().any(Expr::can_raise);
            let of_column = |at: usize| if let Some(Expr::Column(c)) = exprs.get(at) { Some(*c) } else { None };
            let bound = self.bound.as_ref().filter(|_| quiet).and_then(|(at, t)| of_column(*at).map(|c| (c, Arc::clone(t))));
            // The columns it reads for their octet length alone (RULES.md D1267).
            let all: Vec<&Expr> = exprs.iter().collect();
            return Deferred { bound, measured: without(&measured_in(&all), &read_as_text(&all, &[])), ..Deferred::default() };
        };
        let (mut unread, mut read) = (Vec::new(), Vec::new());
        for (n, (e, c)) in exprs.iter().zip(&moved).enumerate() {
            if self.unread.binary_search(&n).is_ok() {
                unread.extend(*c);
            } else {
                expr_columns(e, &mut read);
            }
        }
        unread.sort_unstable();
        unread.dedup();
        // The bound's column moved to the column it is of.
        let bound = self.bound.as_ref().and_then(|(at, t)| moved.get(*at).copied().flatten().map(|c| (c, Arc::clone(t))));
        // A projection of columns gives its input's rows in their order,
        // so what may come in any order of it may of its input (RULES.md D1221).
        // A column moved to one measured is measured, unless another is made of it.
        let mut measured: Vec<usize> = self.measured.iter().filter_map(|&n| moved.get(n).copied().flatten()).collect();
        measured.sort_unstable();
        measured.dedup();
        let moves_of = |c: usize| moved.iter().enumerate().filter(move |(_, m)| **m == Some(c)).map(|(n, _)| n);
        measured.retain(|&c| moves_of(c).all(|n| self.measured.binary_search(&n).is_ok()));
        let measured = without(&measured, &read);
        Deferred { reduced: self.reduced, unread: without(&unread, &read), bound, any_order: self.any_order, told: Vec::new(), measured }
    }

    /// Of an inner hash join's left operand, `width` columns wide, `keys`
    /// the join's: its columns the join's rows leave unread, but its keys,
    /// rows dropped, as a join keeps the rows that match alone.
    pub(in crate::exec) fn left_of(&self, keys: &[JoinKey], width: usize) -> Deferred {
        let read: Vec<usize> = keys.iter().map(|k| k.left).collect();
        let unread: Vec<usize> = self.unread.iter().copied().filter(|&c| c < width).collect();
        Deferred { reduced: true, unread: without(&unread, &read), ..Deferred::default() }
    }

    /// The runtime filters told it.
    pub(in crate::exec) fn filters(&self) -> &[Told] {
        &self.told
    }

    /// It, told `told` in place of the filters it was.
    pub(in crate::exec) fn with_told(self, told: Vec<Told>) -> Deferred {
        Deferred { told, ..self }
    }

    /// \[DESIGN\] (RULES.md D1221) It, of a reader that finds the
    /// subtree's rows by a key no two of them share if `keyed`, so it
    /// may be given them in any order.
    pub(in crate::exec) fn in_any_order(self, keyed: bool) -> Deferred {
        Deferred { any_order: keyed, ..self }
    }

    /// Whether the subtree's rows may be given in any order.
    pub(in crate::exec) fn any_order(&self) -> bool {
        self.any_order
    }

    /// \[DESIGN\] (RULES.md D1267) The columns of a scan of `columns` it
    /// asks for measured: those whose text none above reads.
    pub(in crate::exec) fn measured_of(&self, columns: &[usize]) -> Vec<usize> {
        columns.iter().copied().filter(|c| self.measured.binary_search(c).is_ok()).collect()
    }

    /// The columns of a scan of `columns` it gives late: those left unread,
    /// if an operator above drops rows.
    pub(in crate::exec) fn late_of(&self, columns: &[usize]) -> Vec<usize> {
        if !self.reduced {
            return Vec::new();
        }
        columns.iter().copied().filter(|c| self.unread.binary_search(c).is_ok()).collect()
    }
}

/// \[DESIGN\] (RULES.md D1267) The column `e` takes the octet length of,
/// if it is OCTET_LENGTH of a column.
fn measured_operand(e: &Expr) -> Option<usize> {
    let Expr::Call(function, args) = e else { return None };
    let [Expr::Column(c)] = args.as_slice() else { return None };
    (function.0.name() == kw::OCTET_LENGTH).then_some(*c)
}

/// \[DESIGN\] (RULES.md D1267) The column `cond` compares unpadded with
/// the empty string, if it is such a comparison. It is one of the
/// column's octet length with 0 (`judge/measured.rs`).
fn measured_compared(cond: &Cond) -> Option<usize> {
    compared_unpadded(cond).filter(|(_, _, octets)| octets.is_empty()).map(|(column, ..)| column)
}

/// Adds to `out` every column `e` reads the text of: every column it
/// refers to but as the operand of OCTET_LENGTH.
fn expr_texts(e: &Expr, out: &mut Vec<usize>) {
    if measured_operand(e).is_some() {
        return;
    }
    if let Expr::Column(c) = e {
        out.push(*c);
    }
    e.for_each_child(&mut |child| expr_texts(child, out));
}

/// As `expr_texts`, of a condition: of each conjunct, every column it
/// refers to, but that of a comparison unpadded with the empty string.
fn cond_texts(cond: &Cond, out: &mut Vec<usize>) {
    if let Cond::And(a, b) = cond {
        cond_texts(a, out);
        cond_texts(b, out);
    } else if measured_compared(cond).is_none() {
        cond.for_each_expr(&mut |e| expr_texts(e, out));
    }
}

/// \[DESIGN\] (RULES.md D1267) The columns `exprs` and `conds` read the
/// text of, ascending: every column they refer to, but one read only as
/// the operand of OCTET_LENGTH or in a conjunct that compares it
/// unpadded with the empty string.
pub(in crate::exec) fn read_as_text(exprs: &[&Expr], conds: &[&Cond]) -> Vec<usize> {
    let mut read = Vec::new();
    for e in exprs {
        expr_texts(e, &mut read);
    }
    for c in conds {
        cond_texts(c, &mut read);
    }
    read.sort_unstable();
    read.dedup();
    read
}

/// The columns `exprs` take the octet length of, ascending.
fn measured_in(exprs: &[&Expr]) -> Vec<usize> {
    fn walk(e: &Expr, out: &mut Vec<usize>) {
        out.extend(measured_operand(e));
        e.for_each_child(&mut |child| walk(child, out));
    }
    let mut out = Vec::new();
    for e in exprs {
        walk(e, &mut out);
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// \[DESIGN\] (RULES.md D1267) The columns a grouping by `groups`
/// computing `aggs` reads the text of, ascending: its grouping columns
/// and those its set functions' expressions read as text
/// (`read_as_text`).
pub(in crate::exec) fn grouping_texts(groups: &[usize], aggs: &[AggCall]) -> Vec<usize> {
    let exprs: Vec<&Expr> = aggs.iter().flat_map(AggCall::exprs).collect();
    let mut read = read_as_text(&exprs, &[]);
    read.extend_from_slice(groups);
    read.sort_unstable();
    read.dedup();
    read
}

/// `columns`, ascending, but those of `read`.
fn without(columns: &[usize], read: &[usize]) -> Vec<usize> {
    columns.iter().copied().filter(|c| !read.contains(c)).collect()
}

/// \[DESIGN\] (RULES.md D361) `batches`, each of its late columns decoded
/// of the rows kept of it alone (`Batch::resolve`), as the stream is read
/// whole: by a grouping, a sort, a projection, a set operation or the rows
/// made of it; gathered on `kernels` (RULES.md D570).
pub(in crate::exec) fn resolved(batches: Batches, kernels: Arc<dyn crate::kernels::Kernels>) -> Batches {
    Box::new(batches.map(move |kept| {
        let (mut batch, rows) = kept?;
        if batch.has_late() {
            batch.resolve(rows.as_deref(), &*kernels)?;
        }
        Ok((batch, rows))
    }))
}

/// \[DESIGN\] (RULES.md D1267) The stream of `plan` as `stream::open`
/// gives it, read whole by one that reads the text of the columns `read`
/// alone of its rows. A scan's column whose text neither that reader
/// nor an operator of the stream reads is given measured and left so
/// (`Batch::resolve`).
///
/// # Errors
///
/// As `stream::open`.
pub(super) fn opened_measuring(plan: Plan, (db, tx): (&Db, TxId), read: &[usize]) -> Result<Stream> {
    let (columns, width) = layout(&plan, db).ok_or_else(|| crate::exec::broken("a plan that gives no batches"))?;
    let batches = batches_with(plan, (db, tx), &Deferred::all(width).measuring(read, width))?;
    Ok(Stream { batches: resolved(batches, crate::db::kernels::chosen(db)), columns })
}

// Unit tests: the bound a top-N tells its scan, moved by projections and
// dropped where rows are not its own (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

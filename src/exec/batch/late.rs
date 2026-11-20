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
//! What a column store's scan decodes late is what it would have decoded
//! then, as its groups are replaced, never changed, and a batch holds the
//! group it reads (`rows/frozen/late.rs`). A damaged page is reported as
//! the column is decoded, later than the scan would have, which only an
//! error of the stored data, never one of the data's values, can be.

use super::stream::Batches;
use crate::logical::{Cond, Expr, cond_columns, expr_columns};
use crate::physical::JoinKey;

/// \[DESIGN\] (RULES.md D361) What the operators above a subtree of a
/// stream leave of its rows' columns unread before the stream is read
/// whole, and whether one of them drops rows.
#[derive(Debug, Clone, Default)]
pub(in crate::exec) struct Deferred {
    /// Whether an operator above drops rows.
    reduced: bool,
    /// The columns of the subtree's rows none above reads, ascending.
    unread: Vec<usize>,
}

impl Deferred {
    /// Of a stream read whole, its rows `width` columns wide: no column read
    /// yet, no row dropped.
    pub(in crate::exec) fn all(width: usize) -> Deferred {
        Deferred { reduced: false, unread: (0..width).collect() }
    }

    /// Whether it leaves any column unread.
    pub(in crate::exec) fn any(&self) -> bool {
        !self.unread.is_empty()
    }

    /// Of a filter's input, `cond` the filter's condition: the columns it
    /// reads read, rows dropped. A condition with a subquery is judged of
    /// rows made of every column, so it reads them all.
    pub(in crate::exec) fn filtered(&self, cond: &Cond) -> Deferred {
        if cond.has_subquery() {
            return Deferred::default();
        }
        let mut read = Vec::new();
        cond_columns(cond, &mut read);
        Deferred { reduced: true, unread: without(&self.unread, &read) }
    }

    /// Of a projection's input, `exprs` its expressions: a column one of
    /// them is, left unread if the projection's column is, as it is moved
    /// (`stream::pick`); every column any other reads read. A projection
    /// not of columns alone reads every column of a batch a filter kept
    /// rows of (`project::computed`), so reads them all.
    pub(in crate::exec) fn projected(&self, exprs: &[Expr]) -> Deferred {
        let moved: Option<Vec<usize>> = exprs.iter().map(|e| if let Expr::Column(c) = e { Some(*c) } else { None }).collect();
        let Some(moved) = moved else { return Deferred::default() };
        let (mut unread, mut read) = (Vec::new(), Vec::new());
        for (n, (e, &c)) in exprs.iter().zip(&moved).enumerate() {
            if self.unread.binary_search(&n).is_ok() {
                unread.push(c);
            } else {
                expr_columns(e, &mut read);
            }
        }
        unread.sort_unstable();
        unread.dedup();
        Deferred { reduced: self.reduced, unread: without(&unread, &read) }
    }

    /// Of an inner hash join's left operand, `width` columns wide, `keys`
    /// the join's: its columns the join's rows leave unread, but its keys,
    /// rows dropped, as a join keeps the rows that match alone.
    pub(super) fn left_of(&self, keys: &[JoinKey], width: usize) -> Deferred {
        let read: Vec<usize> = keys.iter().map(|k| k.left).collect();
        let unread: Vec<usize> = self.unread.iter().copied().filter(|&c| c < width).collect();
        Deferred { reduced: true, unread: without(&unread, &read) }
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

/// `columns`, ascending, but those of `read`.
fn without(columns: &[usize], read: &[usize]) -> Vec<usize> {
    columns.iter().copied().filter(|c| !read.contains(c)).collect()
}

/// \[DESIGN\] (RULES.md D361) `batches`, each of its late columns decoded
/// of the rows kept of it alone (`Batch::resolve`), as the stream is read
/// whole: by a grouping, a sort, a projection, a set operation or the rows
/// made of it.
pub(in crate::exec) fn resolved(batches: Batches) -> Batches {
    Box::new(batches.map(|kept| {
        let (mut batch, rows) = kept?;
        if batch.has_late() {
            batch.resolve(rows.as_deref())?;
        }
        Ok((batch, rows))
    }))
}

//! Temporal foreign keys [ISO/IEC 9075-2:2023 §4.25.3.3, §11.8 SR18a i]: a
//! row's period is covered by the union of the periods of the referenced
//! rows its values match, its match type taking nulls as of any foreign
//! key; and a referenced row deleted, or its key or period changed, leaves
//! every row whose period overlapped its own still covered.
//!
//! The referenced rows of a key are read through the index of the unique
//! constraint WITHOUT OVERLAPS the foreign key references, in order of
//! their periods' starts, those starting before the row's period ends: a
//! sweep that holds nothing and stops at the first gap. Without such an
//! index of them (a method that orders no keys or finds none by their
//! first columns, a collated column, or MATCH PARTIAL with a null), the
//! key's periods are gathered and sorted, as many as the referenced rows
//! of the key.

use std::cmp::Ordering;
use std::ops::ControlFlow;
use std::sync::Arc;

use super::references::{partially, wanted};
use crate::catalog::{ForeignKey, IndexDesc, MatchType, TableDesc};
use crate::db::{State, api};
use crate::error::Result;
use crate::index::{KeyOrder, Probe};
use crate::storage::{Row, project};
use crate::txn::Txn;
use crate::value::{CompOp, Value, compare};

/// A period's start and end.
type Span = (Value, Value);

impl State {
    /// [§11.8 GR2; ISO/IEC 9075-2:2023 §4.25.3.3] Whether `row`, of
    /// `child`, satisfies its foreign key `fk`: its values match a row of
    /// the referenced table, and of a temporal one, its period is covered
    /// by those of the rows they match.
    pub(super) fn matches_row(&self, tx: &Txn, (child, fk): (&TableDesc, &ForeignKey), row: &Row) -> Result<bool> {
        let values = project(row, &fk.columns).ok_or_else(|| api(format!("[DESIGN] a row of {} is short", child.name)))?;
        let Some((own, _)) = &fk.period else { return self.matches(tx, fk, &values) };
        let (columns, wanted) = match wanted(fk, &values) {
            ControlFlow::Break(decided) => return Ok(decided),
            ControlFlow::Continue(found) => found,
        };
        let span = span_of(child, own, row)?;
        self.covered(tx, fk, (&columns, &wanted), &span)
    }

    /// Whether the periods of the rows of `fk`'s referenced table whose
    /// `columns` equal `wanted` cover `span`.
    fn covered(&self, tx: &Txn, fk: &ForeignKey, (columns, wanted): (&[usize], &[Value]), span: &Span) -> Result<bool> {
        let parent = self.catalog.table(&fk.table).ok_or_else(|| api(format!("[DESIGN] {} is not defined", fk.table)))?;
        let theirs = fk.period.as_ref().map_or("", |(_, theirs)| theirs.as_str());
        let bounds = parent.period_columns(theirs).ok_or_else(|| api(format!("[DESIGN] {} has no period {theirs}", fk.table)))?;
        if let Some(ix) = self.swept(parent, columns, bounds) {
            return self.sweep(tx, (parent, ix), (columns, wanted), span);
        }
        let mut spans = Vec::new();
        self.each_row_with(tx, &fk.table, (columns, wanted), &mut |_, row| {
            let [start, end] = bounds.map(|at| row.get(at).cloned().unwrap_or(Value::Null));
            spans.push((start, end));
            Ok(ControlFlow::Continue(()))
        })?;
        spans.sort_by(|a, b| order(&a.0, &b.0));
        Ok(cover(spans.into_iter(), span))
    }

    /// The index of `parent` whose key is `columns`, in any order, then the
    /// period's `bounds`, that is ordered, of no collation, and finds keys
    /// by their first columns and an operator on the next: the index of a
    /// unique constraint WITHOUT OVERLAPS.
    fn swept(&self, parent: &TableDesc, columns: &[usize], bounds: [usize; 2]) -> Option<&IndexDesc> {
        let n = columns.len();
        let fits = |ix: &&IndexDesc| {
            let method = self.methods.get(&ix.method);
            let (first, rest) = ix.columns.split_at(n.min(ix.columns.len()));
            let keyed = first.len() == n && first.iter().all(|c| columns.contains(c)) && rest == bounds;
            keyed && !ix.is_collated() && method.is_some_and(|m| m.ordered() && m.supports_prefix(n.saturating_add(2), n, Some(CompOp::Lt)))
        };
        self.catalog.indexes(&parent.name).into_iter().find(fits)
    }

    /// `covered`, through `ix`, the index of `parent` that `swept` found:
    /// the rows of the key starting before `span` ends, in order of their
    /// starts, each taken by an entry of its key as it is now.
    fn sweep(
        &self,
        tx: &Txn,
        (parent, ix): (&TableDesc, &IndexDesc),
        (columns, wanted): (&[usize], &[Value]),
        span: &Span,
    ) -> Result<bool> {
        let n = columns.len();
        let ordered: Option<Vec<Value>> =
            ix.columns.iter().take(n).map(|c| columns.iter().position(|x| x == c).and_then(|at| wanted.get(at)).cloned()).collect();
        let equal = ordered.ok_or_else(|| api("[DESIGN] an index key is not the values looked up"))?;
        let probe = Probe::Prefix { equal, then: Some((CompOp::Lt, span.1.clone())) };
        self.read_lock(tx, &parent.name)?;
        let storage = self.storage_of(&parent.name)?;
        let mut reached = span.0.clone();
        for entry in Arc::clone(self.index_of(&ix.name)?).search(&probe, KeyOrder::Ascending) {
            if order(&reached, &span.1) != Ordering::Less {
                return Ok(true);
            }
            let (key, id) = entry?;
            // An entry of a key the row has no more is passed by.
            if storage.get(tx.id, id)?.is_none_or(|r| ix.key_of(&r).as_ref() != Some(&key)) {
                continue;
            }
            let [start, end] = [n, n.saturating_add(1)].map(|at| key.0.get(at).cloned().unwrap_or(Value::Null));
            if order(&start, &reached) == Ordering::Greater {
                return Ok(false);
            }
            if order(&end, &reached) == Ordering::Greater {
                reached = end;
            }
        }
        Ok(order(&reached, &span.1) != Ordering::Less)
    }

    /// [§11.8 GR2] NO ACTION of a temporal foreign key `fk` of `child`:
    /// whether a row that matched `old`, a row of its referenced table
    /// whose referenced values were `key`, deleted or updated, and whose
    /// period overlapped its own, is no longer covered.
    pub(super) fn temporal_orphans(&self, tx: &Txn, (child, fk): (&TableDesc, &ForeignKey), old: &Row, key: &[Value]) -> Result<bool> {
        let Some((own, theirs)) = &fk.period else { return Ok(false) };
        let parent = self.catalog.table(&fk.table).ok_or_else(|| api(format!("[DESIGN] {} is not defined", fk.table)))?;
        let was = span_of(parent, theirs, old)?;
        let mut orphaned = false;
        let mut each = |_, row: Row| {
            if overlap(&span_of(child, own, &row)?, &was) {
                orphaned = !self.matches_row(tx, (child, fk), &row)?;
            }
            Ok(if orphaned { ControlFlow::Break(()) } else { ControlFlow::Continue(()) })
        };
        match fk.match_type {
            MatchType::Simple | MatchType::Full => {
                if !key.iter().any(Value::is_null) {
                    self.each_row_with(tx, &child.name, (&fk.columns, key), &mut each)?;
                }
            }
            MatchType::Partial => {
                let matching = |row: &Row| project(row, &fk.columns).is_some_and(|v| partially(&v, key));
                self.each_row_where(tx, &child.name, &matching, &mut each)?;
            }
        }
        Ok(orphaned)
    }
}

/// The span of `row`'s period `name`, of `desc`.
fn span_of(desc: &TableDesc, name: &str, row: &Row) -> Result<Span> {
    let bounds = desc.period_columns(name).ok_or_else(|| api(format!("[DESIGN] {} has no period {name}", desc.name)))?;
    let [start, end] = bounds.map(|at| row.get(at).cloned().unwrap_or(Value::Null));
    Ok((start, end))
}

/// How `a` and `b`, points in time of one type, compare.
fn order(a: &Value, b: &Value) -> Ordering {
    compare(a, b).unwrap_or(Ordering::Equal)
}

/// [ISO/IEC 9075-2:2023 §4.16.2] Whether spans `a` and `b` overlap: each
/// starts before the other ends.
fn overlap(a: &Span, b: &Span) -> bool {
    order(&a.0, &b.1) == Ordering::Less && order(&b.0, &a.1) == Ordering::Less
}

/// Whether `spans`, in order of their starts, cover `span`: from its start,
/// each next begins no later than the last reached, until its end.
fn cover(spans: impl Iterator<Item = Span>, span: &Span) -> bool {
    let mut reached = span.0.clone();
    for (start, end) in spans {
        if order(&reached, &span.1) != Ordering::Less {
            return true;
        }
        if order(&start, &reached) == Ordering::Greater {
            return false;
        }
        if order(&end, &reached) == Ordering::Greater {
            reached = end;
        }
    }
    order(&reached, &span.1) != Ordering::Less
}

// Unit tests: the coverage of a span by others, gaps and overlaps
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

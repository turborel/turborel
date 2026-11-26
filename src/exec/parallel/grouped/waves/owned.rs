//! \[DESIGN\] (RULES.md D1121) The groups a morsel owns: those whose value
//! of a grouping column no other morsel's rows can hold, so the thread
//! that grouped the morsel finishes them at once, and no partition merges
//! them. A table is loaded and frozen in about the order of a key, as
//! LINEITEM is of its order key, and its morsels then hold ranges of the
//! key one after another, meeting at the morsels' edges alone.
//!
//! Nothing is taken of an order the table is said to keep. The split
//! bounds each morsel's values of the column by the zones kept of its
//! frozen groups, widened by the values of the rows of the tree in its
//! range, read (`TableStorage::split`, `Split::bounds`). A zone holds
//! every value its rows hold, as a scan already trusts it to pass over
//! rows. Of each morsel's bounds, `lo` and `hi`, a morsel is clean if
//! every other morsel's bounds lie apart from its own or meet them at `lo`
//! or `hi` alone. Then a value strictly between `lo` and `hi` is in no
//! other morsel, and a group of it, whatever its other grouping values,
//! has every row there. A clean morsel's groups of a value at `lo` or
//! `hi`, its groups of a null, and every group of a morsel not clean go to
//! the partitions as before (`waves.rs`), so the groups the partitions
//! merge and those owned never share a value.
//!
//! A morsel the split cannot bound may hold any value, so if any is
//! unbounded, no morsel owns anything. A column is owned by only where
//! the grouping's equality is the zones' order: exact numbers at any
//! scale, whose groups are of equal numbers, and datetimes of no time
//! zone (`Keys::inside`); of a column of any other type no morsel owns a
//! group.

use std::cmp::Ordering;

use super::super::super::pipeline::{Branch, Pipeline, Step};
use super::super::Grouping;
use crate::error::Result;
use crate::exec::table::GroupTable;
use crate::exec::{Runner, eval_cond};
use crate::logical::{Expr, Truth};
use crate::storage::Row;
use crate::value::{Value, compare};

/// The bounds of each morsel's values of a grouping column, of a morsel
/// that is clean, as `of` finds them.
pub(super) struct Owned {
    /// The grouping column, by its place among the grouping's.
    lane: usize,
    /// Of each morsel, its bounds if it is clean.
    clean: Vec<Option<(Value, Value)>>,
}

impl Owned {
    /// The morsels of `pipeline` that own groups of `grouping`, by the
    /// first of its grouping columns its one table's split bounds in
    /// every morsel; none if no column is so bounded, no morsel is clean,
    /// or the grouping is of DISTINCT or of a pair, whose groups go to the
    /// partitions whole (RULES.md D1113, D1133).
    pub(super) fn of(pipeline: &Pipeline, grouping: &Grouping) -> Option<Owned> {
        if grouping.distinct || grouping.outer.is_some() {
            return None;
        }
        let [branch] = pipeline.branches.as_slice() else { return None };
        grouping.spec.groups.iter().enumerate().find_map(|(lane, &column)| {
            let source = source_of(branch, column)?;
            let bounds: Vec<Option<(Value, Value)>> =
                (0..branch.split.morsels()).map(|n| branch.split.bounds(n, source)).collect::<Option<_>>()?;
            let clean = cleaned(bounds)?;
            clean.iter().any(Option::is_some).then_some(Owned { lane, clean })
        })
    }

    /// Of morsel `m`'s groups in `table`, which it owns; none if it is not
    /// clean or owns none.
    pub(super) fn of_morsel(&self, m: usize, table: &GroupTable) -> Option<Vec<bool>> {
        let (lo, hi) = self.clean.get(m)?.as_ref()?;
        table.inside(self.lane, (lo, hi)).filter(|owned| owned.contains(&true))
    }
}

/// Of each morsel's bounds, those of a clean morsel: one whose bounds
/// every other's lie apart from or meet at an end alone. Sorted by their
/// least values, then greatest, two bounds meet beyond an end only if
/// the later begins before the earlier ends, so a morsel is not clean if
/// one before it ends past its least, or the one after it begins before
/// its greatest. None if two values do not compare, so no morsel owns.
fn cleaned(bounds: Vec<Option<(Value, Value)>>) -> Option<Vec<Option<(Value, Value)>>> {
    let mut order: Vec<usize> = (0..bounds.len()).filter(|&m| bounds.get(m).is_some_and(Option::is_some)).collect();
    let of = |m: usize| bounds.get(m).and_then(Option::as_ref);
    let mut failed = false;
    let mut compared = |a: &Value, b: &Value| {
        compare(a, b).unwrap_or_else(|| {
            failed = true;
            Ordering::Equal
        })
    };
    order.sort_by(|&a, &b| match (of(a), of(b)) {
        (Some(a), Some(b)) => compared(&a.0, &b.0).then_with(|| compared(&a.1, &b.1)),
        (_, _) => Ordering::Equal,
    });
    let mut clean = vec![true; bounds.len()];
    let mut furthest: Option<&Value> = None;
    for (at, &m) in order.iter().enumerate() {
        let (lo, hi) = of(m)?;
        let ends_past = furthest.is_some_and(|f| compared(f, lo) == Ordering::Greater);
        let next_begins =
            order.get(at.saturating_add(1)).and_then(|&n| of(n)).is_some_and(|(next, _)| compared(next, hi) == Ordering::Less);
        if let Some(mark) = clean.get_mut(m) {
            *mark = !ends_past && !next_begins;
        }
        furthest = Some(furthest.map_or(hi, |f| if compared(hi, f) == Ordering::Greater { hi } else { f }));
    }
    if failed {
        return None;
    }
    Some(bounds.into_iter().zip(clean).map(|(b, kept)| b.filter(|_| kept)).collect())
}

/// The column of the table `branch` scans that column `column` of its
/// rows at its end is, carried unchanged by each step: by a filter, a
/// semi join, a runtime filter or a count; by a projection of a column;
/// by a hash join's probe, of the probing row's columns. None of a
/// column an expression makes, a built operand's, or one past an outer
/// join, whose rows may be null.
fn source_of(branch: &Branch, column: usize) -> Option<usize> {
    let (scanned, width) = branch.scanned.as_ref()?;
    let (mut given, mut wide) = (Vec::with_capacity(branch.steps.len()), *width);
    for step in &branch.steps {
        given.push(wide);
        wide = match step {
            Step::Project(exprs) => exprs.len(),
            Step::Probe(table) | Step::Outer(table) => table.shared.joined_layout((&[], wide)).1,
            Step::Filter(_) | Step::Semi(..) | Step::Runtime(_) | Step::Count(_) => wide,
        };
    }
    let mut at = column;
    for (step, &left) in branch.steps.iter().zip(&given).rev() {
        at = match step {
            Step::Filter(_) | Step::Semi(..) | Step::Runtime(_) | Step::Count(_) => at,
            Step::Project(exprs) => {
                let Expr::Column(c) = exprs.get(at)? else { return None };
                *c
            }
            Step::Probe(_) => (at < left).then_some(at)?,
            Step::Outer(_) => return None,
        };
    }
    scanned.contains(&at).then_some(at)
}

impl Grouping {
    /// \[DESIGN\] (RULES.md D1121) The groups of `table` that `owned`
    /// marks, finished, that its HAVING condition is true of, as `kept_of`
    /// keeps a table's, in the order of their first rows.
    ///
    /// # Errors
    ///
    /// As finishing a group, or judging its condition, fails.
    pub(super) fn kept_among(&self, table: &GroupTable, owned: &[bool], runner: &Runner) -> Result<Vec<(u64, Row)>> {
        let cond = self.having.as_ref();
        let mut groups = match (cond, cond.and_then(|c| self.bounded(c))) {
            (_, Some(bounded)) => table.finished_above_among(Some(owned), bounded)?,
            (Some(cond), None) => table.finished_if_among(Some(owned), |row| Ok(eval_cond(cond, row, runner)? == Truth::True))?,
            (None, None) => table.finished_if_among(Some(owned), |_| Ok(true))?,
        };
        groups.sort_unstable_by_key(|g| g.0);
        Ok(groups)
    }
}

// Unit tests: which morsels are clean, and the column a branch's step
// carries (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

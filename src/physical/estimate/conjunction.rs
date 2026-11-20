//! \[DESIGN\] The share of a table's rows a conjunction keeps (RULES.md
//! D381, D382, D383), each step answering what the one before cannot:
//!
//! * conjuncts on one column are one factor: its bounds one range, a
//!   BETWEEN's two halves its share between them, not the product of
//!   each, and any other conjunct on it bounding it further;
//! * equalities with constants of columns that are together an index's
//!   key, or hold a unique key, are one factor, one in the distinct
//!   keys the index counts (`Statistics::group_distinct`), which counts
//!   correlated columns once;
//! * conjuncts that go together, or that no synopsis answers, are
//!   evaluated on the table's sample of rows (`estimate/sampled.rs`);
//! * the factors left are combined by exponential backoff
//!   (`estimate/backoff.rs`).

use super::{Known, backoff, sampled, selectivity};
use crate::logical::{Cond, compared_with_constant, cond_columns};
use crate::value::{CompOp, Value};

/// \[DESIGN\] The most columns of equalities whose sets an index's key
/// is looked for among, each of two or more: six, 57 sets, and no index
/// of the planner's shapes is wider.
const MOST_GROUPED: usize = 6;

/// The share of rows the conjuncts `conds`, over one table's columns,
/// keep together, by what is `known`.
pub fn conjunction(conds: &[Cond], known: Known<'_>) -> f64 {
    let all: Vec<&Cond> = conds.iter().collect();
    let width = known.stats.and_then(|s| s.sample()).and_then(|s| s.rows().first()).map_or(0, Vec::len);
    let (told, untold): (Vec<&Cond>, Vec<&Cond>) = all.iter().copied().partition(|c| sampled::evaluable(c, width));
    if let Some(sample) = sampled::eligible(&told, known) {
        let synopses = backoff::combined(&factors(&told, known));
        let mut shares = factors(&untold, known);
        shares.push(sampled::kept(&told, sample, synopses));
        return backoff::combined(&shares).clamp(0.0, 1.0);
    }
    backoff::combined(&factors(&all, known)).clamp(0.0, 1.0)
}

/// The factors of `conds` the backoff combines: one for each column's
/// conjuncts, one for each set of equalities an index's key answers, one
/// for each conjunct of several columns or none.
fn factors(conds: &[&Cond], known: Known<'_>) -> Vec<f64> {
    let mut columns: Vec<(usize, Vec<&Cond>)> = Vec::new();
    let mut shares = Vec::new();
    for &c in conds {
        match one_column(c) {
            Some(column) => match columns.iter_mut().find(|(at, _)| *at == column) {
                Some((_, of)) => of.push(c),
                None => columns.push((column, vec![c])),
            },
            None => shares.push(selectivity(c, known)),
        }
    }
    let grouped = grouped(&columns, known);
    if let Some((share, covered)) = &grouped {
        columns.retain(|(column, _)| !covered.contains(column));
        shares.push(*share);
    }
    shares.extend(columns.iter().map(|(column, of)| column_share(*column, of, known)));
    shares
}

/// The one column `c` reads, if it reads one.
fn one_column(c: &Cond) -> Option<usize> {
    let mut columns = Vec::new();
    cond_columns(c, &mut columns);
    let first = *columns.first()?;
    columns.iter().all(|&at| at == first).then_some(first)
}

/// The share of rows the conjuncts `of`, each of `column` alone, keep:
/// its bounds by constants as one range, at most what each other one
/// keeps.
fn column_share(column: usize, of: &[&Cond], known: Known<'_>) -> f64 {
    let (mut low, mut high, mut least) = (None::<f64>, None::<f64>, 1.0_f64);
    for &c in of {
        let share = selectivity(c, known);
        match compared_with_constant(c).map(|(_, op, _)| op) {
            Some(CompOp::Gt | CompOp::Ge) => low = Some(low.map_or(share, |l| l.min(share))),
            Some(CompOp::Lt | CompOp::Le) => high = Some(high.map_or(share, |h| h.min(share))),
            Some(CompOp::Eq | CompOp::Ne) | None => least = least.min(share),
        }
    }
    let range = match (low, high) {
        (Some(a), Some(b)) => {
            // Of one column, both hold where either does not fail: a + b
            // less its rows not null, if the statistics know; else as if
            // independent, as an index's range is priced (`access/keys.rs`).
            let present = 1.0_f64 - known.stats.and_then(|s| s.nulls(column)).unwrap_or(0.0_f64);
            let between = a + b - present;
            if known.stats.is_some() && between >= 0.0_f64 { between } else { a * b }
        }
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => 1.0_f64,
    };
    range.min(least)
}

/// Of the columns `columns` each with its conjuncts, those that are one
/// equality with a constant not common, the largest set of two or more
/// whose joint distinct values the statistics know: its share, rows not
/// null over those distinct values, and its columns.
fn grouped(columns: &[(usize, Vec<&Cond>)], known: Known<'_>) -> Option<(f64, Vec<usize>)> {
    let stats = known.stats?;
    let equal: Vec<usize> = columns.iter().filter_map(|(column, of)| single_equality(of, known).then_some(*column)).collect();
    if equal.len() < 2 || equal.len() > MOST_GROUPED {
        return None;
    }
    let sets = 1_usize.checked_shl(u32::try_from(equal.len()).ok()?)?;
    let mut best: Option<(f64, Vec<usize>)> = None;
    for mask in 3..sets {
        let set: Vec<usize> = equal.iter().enumerate().filter(|&(at, _)| has(mask, at)).map(|(_, &c)| c).collect();
        let larger = best.as_ref().is_none_or(|(_, b)| set.len() > b.len());
        if set.len() < 2 || !larger {
            continue;
        }
        if let Some(distinct) = stats.group_distinct(&set) {
            let present = set.iter().map(|&c| 1.0_f64 - stats.nulls(c).unwrap_or(0.0_f64)).fold(1.0_f64, f64::min);
            best = Some((present / distinct.max(1.0_f64), set));
        }
    }
    best
}

/// Whether `of` is one equality of its column with a constant, not null
/// and not among the column's most common values, whose own share says
/// more than the index's average.
fn single_equality(of: &[&Cond], known: Known<'_>) -> bool {
    let [c] = of else { return false };
    let Some((column, CompOp::Eq, value)) = compared_with_constant(c) else { return false };
    let common = known.stats.and_then(|s| s.common(column)).is_some_and(|list| list.iter().any(|(v, _)| same(v, value)));
    !value.is_null() && !common
}

/// Whether `a` and `b` compare equal.
fn same(a: &Value, b: &Value) -> bool {
    crate::value::compare(a, b) == Some(std::cmp::Ordering::Equal)
}

/// Whether the set `mask` has its member `at`.
fn has(mask: usize, at: usize) -> bool {
    u32::try_from(at).ok().and_then(|at| mask.checked_shr(at)).is_some_and(|m| m & 1 == 1)
}

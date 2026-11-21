//! \[DESIGN\] A range join's sorted rows (RULES.md D580): of each right row
//! held, its equality keys' values and its sorted value, the rows sorted
//! by them, so the rows of one key are together, each key's in the order
//! of the sorted value; and the tree of its pruning value, if it has one.
//! A left row finds its key's rows, then the rows between its bounds, by
//! halving, then of them those its pruning comparison holds of.
//!
//! A row with a null among these values matches no left row, as a
//! comparison with a null is not True [X3.135-1986 §5.11 GR2], and is not
//! held; neither is one whose value is not equal to itself. The values are
//! ordered by the comparison `value::compare` makes of them, the one the
//! condition's comparisons judge by, of kinds whose values it orders
//! wholly. Of any other kind, or of values of kinds that do not compare,
//! no sorted rows are made, and every pair is judged instead; a left row
//! whose values do not compare with them judges every row of its key.

use std::cmp::Ordering;
use std::sync::Arc;

use super::pruned::Tree;
use super::sorting::{first, sorted};
use super::{Searched, Spec};
use crate::error::Result;
use crate::exec::{Runner, eval};
use crate::storage::Row;
use crate::value::{Value, compare};

/// The sorted rows of a range join's right operand.
pub(super) struct Index {
    /// The right's rows, in their order.
    rows: Arc<Vec<Row>>,
    /// The equality keys, how many.
    keys: usize,
    /// Of each sorted row, its keys' values, `keys` of them each.
    equal: Vec<Value>,
    /// Of each sorted row, its sorted value.
    sorted: Vec<Value>,
    /// Of each sorted row, its place among `rows`.
    places: Vec<usize>,
    tree: Option<Tree>,
}

/// The values a right row is sorted and pruned by: its keys', its sorted
/// value and its pruning value.
struct Held {
    equal: Vec<Value>,
    sorted: Vec<Value>,
    pruned: Vec<Value>,
    places: Vec<usize>,
}

/// Whether `v` is of a kind whose values `compare` orders wholly, and equal
/// to itself: a number, a string, a truth value, a datetime or an interval.
fn orderable(v: &Value) -> bool {
    let kind = matches!(
        v,
        Value::Char(_)
            | Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
    );
    kind && compare(v, v) == Some(Ordering::Equal)
}

impl Index {
    /// The sorted rows of `rows` by `spec`, its values computed by
    /// `runner`; none if a value is of a kind not ordered wholly or two do
    /// not compare.
    ///
    /// # Errors
    ///
    /// As computing a value fails.
    pub(super) fn of(rows: Arc<Vec<Row>>, spec: &Spec, runner: &Runner) -> Result<Option<Index>> {
        let mut held = Held { equal: Vec::new(), sorted: Vec::new(), pruned: Vec::new(), places: Vec::new() };
        for (place, row) in rows.iter().enumerate() {
            let mut values: Vec<Value> = spec.keys.iter().map(|k| row.get(k.right).cloned().unwrap_or(Value::Null)).collect();
            values.push(eval(&spec.sorted, row, runner)?);
            if let Some(p) = &spec.pruned {
                values.push(eval(&p.build, row, runner)?);
            }
            if values.iter().any(Value::is_null) {
                continue;
            }
            if !values.iter().all(orderable) {
                return Ok(None);
            }
            let pruned = spec.pruned.as_ref().and_then(|_| values.pop());
            held.sorted.extend(values.pop());
            held.equal.extend(values);
            held.pruned.extend(pruned);
            held.places.push(place);
        }
        Ok(Self::made(rows, &held, spec))
    }

    /// The sorted rows of what `held` holds of `rows`; none if two values
    /// do not compare.
    fn made(rows: Arc<Vec<Row>>, held: &Held, spec: &Spec) -> Option<Index> {
        let keys = spec.keys.len();
        let n = held.places.len();
        let order = {
            let of = |i: usize| (held.equal.get(i.saturating_mul(keys)..i.saturating_mul(keys).saturating_add(keys)), held.sorted.get(i));
            sorted(n, &mut |a, b| {
                let ((Some(ea), Some(sa)), (Some(eb), Some(sb))) = (of(a), of(b)) else { return None };
                ordered((ea, sa), (eb, sb))
            })?
        };
        let mut equal = Vec::with_capacity(held.equal.len());
        let (mut sorted_values, mut places, mut pruned) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::new());
        for &i in &order {
            equal.extend(held.equal.get(i.saturating_mul(keys)..i.saturating_mul(keys).saturating_add(keys))?.iter().cloned());
            sorted_values.push(held.sorted.get(i)?.clone());
            places.push(*held.places.get(i)?);
            pruned.extend(held.pruned.get(i).cloned());
        }
        let tree = match &spec.pruned {
            Some(p) => Some(Tree::of(pruned, p.op)?),
            None => None,
        };
        Some(Index { rows, keys, equal, sorted: sorted_values, places, tree })
    }

    /// The right's rows, in their order.
    pub(super) fn rows(&self) -> &Arc<Vec<Row>> {
        &self.rows
    }

    /// The equality keys' values of sorted row `i`.
    fn equal_of(&self, i: usize) -> Option<&[Value]> {
        self.equal.get(i.saturating_mul(self.keys)..i.saturating_mul(self.keys).saturating_add(self.keys))
    }

    /// The places of the rows `left` may match, sorted, onto `out`: those
    /// the search finds, or, if a value of `left` does not compare with
    /// the rows', every row of its key.
    ///
    /// # Errors
    ///
    /// As computing a value of `left` fails.
    pub(super) fn search(&self, left: &Row, spec: &Spec, runner: &Runner, out: &mut Vec<usize>) -> Result<Searched> {
        out.clear();
        let Some(probe) = Probe::of(left, spec, runner)? else { return Ok(Searched::Found) };
        let all = 0..self.sorted.len();
        let run = first(all.clone(), &mut |i| Some(compare_slices(self.equal_of(i)?, &probe.equal)? == Ordering::Less)).and_then(|a| {
            first(a..all.end, &mut |i| Some(compare_slices(self.equal_of(i)?, &probe.equal)? != Ordering::Greater)).map(|b| a..b)
        });
        let Some(run) = run else {
            out.extend(self.places.iter().copied());
            out.sort_unstable();
            return Ok(Searched::Judged);
        };
        let searched = if self.found(run.clone(), &probe, out).is_some() {
            Searched::Found
        } else {
            out.clear();
            out.extend(self.places.get(run).into_iter().flatten().copied());
            Searched::Judged
        };
        out.sort_unstable();
        Ok(searched)
    }

    /// The places of the rows of `run` between `probe`'s bounds whose
    /// pruning value holds, onto `out`; none if a value does not compare.
    fn found(&self, run: std::ops::Range<usize>, probe: &Probe, out: &mut Vec<usize>) -> Option<()> {
        let start = match &probe.lower {
            Some((lo, closed)) => first(run.clone(), &mut |i| Some(below(compare(self.sorted.get(i)?, lo)?, *closed)))?,
            None => run.start,
        };
        let end = match &probe.upper {
            Some((hi, closed)) => first(start..run.end, &mut |i| Some(within(compare(self.sorted.get(i)?, hi)?, *closed)))?,
            None => run.end,
        };
        if start >= end {
            return Some(());
        }
        match (&self.tree, &probe.pruned) {
            (Some(tree), Some(value)) => {
                let mut at = Vec::new();
                tree.found(start..end, value, &mut at)?;
                out.extend(at.into_iter().filter_map(|i| self.places.get(i).copied()));
            }
            (Some(_) | None, None) | (None, Some(_)) => out.extend(self.places.get(start..end)?.iter().copied()),
        }
        Some(())
    }
}

/// Whether a sorted value that compares with a bound as `ord` is below it:
/// less, or, if the bound is not `closed`, equal too.
fn below(ord: Ordering, closed: bool) -> bool {
    ord == Ordering::Less || (!closed && ord == Ordering::Equal)
}

/// Whether a sorted value that compares with a bound as `ord` is within
/// it: less, or, if the bound is `closed`, equal too.
fn within(ord: Ordering, closed: bool) -> bool {
    ord == Ordering::Less || (closed && ord == Ordering::Equal)
}

/// Equality keys' values, then a sorted value, compared in that order.
fn ordered((ea, sa): (&[Value], &Value), (eb, sb): (&[Value], &Value)) -> Option<Ordering> {
    Some(compare_slices(ea, eb)?.then(compare(sa, sb)?))
}

/// Values compared in order, the first unequal pair deciding.
fn compare_slices(a: &[Value], b: &[Value]) -> Option<Ordering> {
    for (x, y) in a.iter().zip(b) {
        let ord = compare(x, y)?;
        if ord != Ordering::Equal {
            return Some(ord);
        }
    }
    Some(Ordering::Equal)
}

/// What a left row searches by: its keys' values, its bounds, each with
/// whether it is closed, and its pruning value.
struct Probe {
    equal: Vec<Value>,
    lower: Option<(Value, bool)>,
    upper: Option<(Value, bool)>,
    pruned: Option<Value>,
}

impl Probe {
    /// What `left` searches by; none if a value is null, as it then
    /// matches no row.
    fn of(left: &Row, spec: &Spec, runner: &Runner) -> Result<Option<Probe>> {
        let equal: Vec<Value> = spec.keys.iter().map(|k| left.get(k.left).cloned().unwrap_or(Value::Null)).collect();
        let bound = |b: &Option<crate::physical::Bound>| -> Result<Option<(Value, bool)>> {
            b.as_ref().map(|b| Ok((eval(&b.probe, left, runner)?, b.closed))).transpose()
        };
        let (lower, upper) = (bound(&spec.lower)?, bound(&spec.upper)?);
        let pruned = spec.pruned.as_ref().map(|p| eval(&p.probe, left, runner)).transpose()?;
        let nulls = equal.iter().chain(lower.iter().map(|b| &b.0)).chain(upper.iter().map(|b| &b.0)).chain(pruned.iter());
        if nulls.clone().any(Value::is_null) {
            return Ok(None);
        }
        Ok(Some(Probe { equal, lower, upper, pruned }))
    }
}

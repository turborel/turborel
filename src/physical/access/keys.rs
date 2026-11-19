//! \[DESIGN\] The probes of an index's key columns that a query's
//! comparisons answer: equalities of its first columns, then a bound, a
//! range or an OR of ranges of the next (`PhysicalPlanner::access`); and
//! whether reading such an index in key order gives the order asked.

use super::Candidate;
use crate::index::Probe;
use crate::logical::SortKey;
use crate::physical::ProbeSlot;
use crate::physical::estimate::{self, Known};
use crate::physical::kept::Place;
use crate::physical::probe::{Alternatives, Compared, probes_of_any};
use crate::value::{CompOp, DataType, Value};

/// The probes of an index on `columns`, of types `types`, that the
/// comparisons `compared` and their ORs `ors` answer: the equalities of
/// its first columns, as many as there are, then each comparison of the
/// next column, a lower and an upper bound of it, each OR of it, or none.
/// Values are those an index places as SQL compares them (`coerce`). An
/// index on one column is asked with `Probe::Compare`, which every method
/// answers that supports the operator. A `?`'s value stands in a probe
/// with its slot noted, so another execution's may take its place
/// (`physical/kept.rs`).
pub(super) fn probes(
    columns: &[usize],
    types: &[DataType],
    (compared, ors): (&[Compared], &[Alternatives]),
    all: f64,
    known: Known<'_>,
) -> Vec<Candidate> {
    let on = |at: usize| {
        let (column, data_type) = (columns.get(at).copied(), types.get(at));
        compared.iter().filter(move |c| Some(c.column) == column).flat_map(move |c| {
            let each = data_type.and_then(|t| c.placed(t)).unwrap_or_default();
            each.into_iter().map(move |(op, value)| (c, op, value))
        })
    };
    let mut held = Held { equal: Vec::new(), used: Vec::new(), slots: Vec::new(), guess: all };
    while let Some((c, _, value)) = on(held.equal.len()).find(|(_, op, _)| *op == CompOp::Eq) {
        held.guess *= estimate::compare((c.column, CompOp::Eq, &value), known);
        held.slots.extend(c.slot_at(Place::Key(held.equal.len())));
        held.equal.push(value);
        held.used.push(c.at);
    }
    let fixed = held.equal.len();
    let mut out: Vec<Candidate> = on(fixed).filter(|(_, op, _)| *op != CompOp::Eq || fixed == 0).map(|b| held.then(b, known)).collect();
    // [ISO/IEC 9075:1992 §8.3 GR1] a lower and an upper bound of the next
    // column, as of BETWEEN: one range, the keys past the upper not read.
    let bound =
        |below: bool| on(fixed).find(|(_, op, _)| matches!(op, CompOp::Lt | CompOp::Le) == below && *op != CompOp::Eq && *op != CompOp::Ne);
    if let (Some(low), Some(high)) = (bound(false), bound(true)) {
        out.push(held.between(low, high, known));
    }
    // An OR of the next column's ranges, an IN list among them: a probe of
    // each range. Of no `?` among the equal values, which each probe holds,
    // and a kept plan's `?`s stand in one probe alone.
    for any in ors.iter().filter(|a| Some(a.column) == columns.get(fixed).copied() && held.slots.is_empty()) {
        let Some(probes) = types.get(fixed).and_then(|t| probes_of_any(&held.equal, t, any)) else { continue };
        let used = if any.exact { [held.used.as_slice(), &[any.at]].concat() } else { held.used.clone() };
        out.push(Candidate::of(probes, (used, fixed, held.guess * any.share, !any.exact)));
    }
    if fixed > 0 {
        out.push(held.prefix(columns.len()));
    }
    out
}

/// The equalities of an index's first columns a probe holds: their values,
/// the conjuncts they answer, the `?`s among them, and the rows they are
/// guessed to keep.
struct Held {
    equal: Vec<Value>,
    used: Vec<usize>,
    slots: Vec<ProbeSlot>,
    guess: f64,
}

/// A comparison of an index's column, placed (`Compared::placed`): the
/// comparison, and the operator and value it is placed as.
type Bound<'c> = (&'c Compared, CompOp, Value);

impl Held {
    /// The conjuncts answered: of the equalities, and of `also` if exactly.
    fn with(&self, also: &[&Compared]) -> Vec<usize> {
        [self.used.clone(), also.iter().filter(|c| c.exact).map(|c| c.at).collect()].concat()
    }

    /// The probe of the equalities, then the comparison `(c, op, value)` of
    /// the next column.
    fn then(&self, (c, op, value): Bound<'_>, known: Known<'_>) -> Candidate {
        let guess = self.guess * estimate::compare((c.column, op, &value), known);
        let (fixed, mut slots) = (self.equal.len(), self.slots.clone());
        slots.extend(c.slot_at(Place::Key(fixed)));
        let probe =
            if fixed == 0 { Probe::Compare { op, value } } else { Probe::Prefix { equal: self.equal.clone(), then: Some((op, value)) } };
        Candidate { slots, ..Candidate::one(probe, (self.with(&[c]), fixed, guess, !c.exact)) }
    }

    /// The probe of the equalities, then of the next column between `low`
    /// and `high`.
    fn between(&self, (lo_c, lo_op, lo): Bound<'_>, (hi_c, hi_op, hi): Bound<'_>, known: Known<'_>) -> Candidate {
        let (a, b) = (estimate::compare((lo_c.column, lo_op, &lo), known), estimate::compare((hi_c.column, hi_op, &hi), known));
        // Of one column, both hold where either does not fail: a + b - 1,
        // if the statistics know; else as if independent.
        let guess = self.guess * (a + b - 1.0_f64).max(a * b);
        let mut slots = self.slots.clone();
        slots.extend(lo_c.slot_at(Place::Low).into_iter().chain(hi_c.slot_at(Place::High)));
        let probe = Probe::Range { equal: self.equal.clone(), low: Box::new((lo_op, lo)), high: Box::new((hi_op, hi)) };
        let fixed = self.equal.len();
        Candidate { slots, ..Candidate::one(probe, (self.with(&[lo_c, hi_c]), fixed, guess, !lo_c.exact || !hi_c.exact)) }
    }

    /// The probe of the equalities alone, of an index `width` columns wide.
    fn prefix(self, width: usize) -> Candidate {
        let fixed = self.equal.len();
        let probe = match (self.equal.as_slice(), width) {
            ([only], 1) => Probe::Compare { op: CompOp::Eq, value: only.clone() },
            _ => Probe::Prefix { equal: self.equal, then: None },
        };
        Candidate { slots: self.slots, ..Candidate::one(probe, (self.used, fixed, self.guess, false)) }
    }
}

/// Whether reading an index on `columns` in key order, its first `fixed`
/// held equal by its probe, gives rows in the order of `want`: once the
/// keys of the columns held equal are left out, the rest are the index's
/// next columns, in order, all ascending, or all descending, when it is
/// read backward. Some(backward) if so. Any null ordering is kept, as an
/// index read in key order gives no null in them: its probe is true of no
/// null, and a column it does not test is NOT NULL (`best_path`). Function
/// lookups that may keep nulls are excluded by `best_path` (D34).
pub(super) fn index_order(columns: &[usize], fixed: usize, want: &[SortKey]) -> Option<bool> {
    let (held, free) = columns.split_at(fixed.min(columns.len()));
    let keys: Vec<&SortKey> = want.iter().filter(|k| !held.contains(&k.column)).collect();
    let Some(first) = keys.first() else { return Some(false) };
    let in_order = keys.len() <= free.len() && keys.iter().zip(free).all(|(k, c)| k.column == *c && k.descending == first.descending);
    in_order.then_some(first.descending)
}

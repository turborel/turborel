//! \[DESIGN\] (RULES.md D1325) The states of runs handed on an entry
//! each, set as each run's. An entry takes every row of its run, so its
//! state is that of those rows and of no other: COUNT(\*) the run's
//! length, and a set function of an integer column its rows' integers
//! taken in the order they lie, added of a sum or a mean, the best kept
//! of an extreme, a null passed over and the rows that are not null
//! counted. A run all of nulls leaves its entry the state of no value,
//! as a group fed those rows holds. The rows of a run lie one after
//! another in their batch's vector, so each state is made beside the
//! vector and written once at its entry's place, where feeding the rows
//! adds each into a state at that place.
//!
//! A sum that leaves 64 bits within one run sets nothing, and the
//! column is fed, which sums as wide as a sum goes (RULES.md D1308). Any
//! other column of states is fed too, each row to the entry of its run.

use super::super::{Exact, Totals, Typed};
use super::{Held, ints_of};
use crate::exec::batch::Taken;
use crate::exec::table::Runs;
use crate::exec::table::states::States;
use crate::functions::ColumnarKind;
use crate::storage::mask::index;
use crate::storage::sel::nth;
use crate::storage::{Ints, Sel};

/// The state a run of `ints` leaves a function of `kind`: its total or
/// best value of the rows from `from` to `to` of `rows`, or of all, and
/// how many of them are not null [X3.135-1986 §5.8 GR1]; 0 and none of a
/// run all of nulls. None of a sum that leaves 64 bits.
fn folded(kind: ColumnarKind, ints: &Ints, rows: Option<&[u32]>, (from, to): (usize, usize)) -> Option<(i64, i64)> {
    let (mut total, mut count) = (0_i64, 0_i64);
    for n in from..to {
        let r = nth(rows, n);
        if ints.is_null(r) {
            continue;
        }
        let unit = ints.units.get(r).copied().unwrap_or(0);
        total = match kind {
            ColumnarKind::Sum | ColumnarKind::Avg | ColumnarKind::Count => total.checked_add(unit)?,
            ColumnarKind::Min => {
                if count == 0 {
                    unit
                } else {
                    total.min(unit)
                }
            }
            ColumnarKind::Max => {
                if count == 0 {
                    unit
                } else {
                    total.max(unit)
                }
            }
        };
        count = count.saturating_add(1);
    }
    Some((total, count))
}

/// How many rows each entry of `held` took, its run's, or one, the
/// entries in their order at `places`.
fn lengths(held: &Held, runs: &Runs, places: &[u32]) -> Vec<i64> {
    let mut lengths = vec![0_i64; places.len()];
    let mut place = places.iter();
    for (b, (batch, heads)) in held.iter().enumerate() {
        for (from, to) in runs.spans(b, heads.as_ref().map_or(batch.rows, Sel::len)) {
            if let Some(length) = place.next().and_then(|&p| lengths.get_mut(index(p))) {
                *length = i64::try_from(to.saturating_sub(from)).unwrap_or(i64::MAX);
            }
        }
    }
    lengths
}

impl States {
    /// \[DESIGN\] (RULES.md D1325) Makes its states those of entries of
    /// a run each, as `Typed::set_runs` makes a column's: whether a null
    /// was passed over; none, nothing made, of states that are fed.
    pub(in crate::exec::table) fn set_runs(&mut self, take: &Taken, of: (&Held, &Runs), places: &[u32]) -> Option<bool> {
        if let States::Typed(typed) = self { typed.set_runs(take, of, places) } else { None }
    }
}

impl Typed {
    /// \[DESIGN\] (RULES.md D1325) Makes its states those of the entries
    /// of `held`, a morsel's batches held an entry a run as `runs` says,
    /// the entries in their order at `places`, each entry's state that of
    /// its run's rows taken as `take` says. Gives whether a null was
    /// passed over, to be warned of. None, nothing made, of states it
    /// holds already, of a sum that leaves 64 bits within a run, and of a
    /// function or values it does not set so, which are fed.
    fn set_runs(&mut self, take: &Taken, (held, runs): (&Held, &Runs), places: &[u32]) -> Option<bool> {
        match self {
            Typed::Count(counts) => {
                // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
                let Taken::Count = take else { return None };
                if !counts.is_empty() {
                    return None;
                }
                *counts = lengths(held, runs, places);
                Some(false)
            }
            Typed::Exact(exact) => exact.set_runs(take, (held, runs), places),
            Typed::Approx(_) | Typed::Best(_) => None,
        }
    }
}

impl Exact {
    /// As `Typed::set_runs`, of a column of integers of one scale: each
    /// entry's total or best value that of its run's integers, and its
    /// count the rows of the run that are not null.
    fn set_runs(&mut self, take: &Taken, (held, runs): (&Held, &Runs), places: &[u32]) -> Option<bool> {
        let Taken::Column(c) = take else { return None };
        let (ints, scale) = ints_of(held, *c)?;
        if self.totals.len() != 0 || self.scale.is_some_and(|of| of != scale) {
            return None;
        }
        let (mut totals, mut counts) = (vec![0_i64; places.len()], vec![0_i64; places.len()]);
        let (mut any, mut place) = (false, places.iter());
        for (b, (of, (batch, heads))) in ints.iter().zip(held).enumerate() {
            let rows = runs.rows_of(b, heads.as_ref());
            for (from, to) in runs.spans(b, heads.as_ref().map_or(batch.rows, Sel::len)) {
                let (total, count) = folded(self.kind, of, rows, (from, to))?;
                let p = index(*place.next()?);
                any = any || usize::try_from(count).ok() != Some(to.saturating_sub(from));
                if let (Some(at), Some(taken)) = (totals.get_mut(p), counts.get_mut(p)) {
                    (*at, *taken) = (total, count);
                }
            }
        }
        (self.totals, self.counts, self.scale) = (Totals::Narrow(totals), counts, Some(scale));
        Some(any)
    }
}

//! [ISO/IEC 9075-2:2023 §6.10, §9.23 SR5] Rank functions, LEAD and LAG and
//! the value functions over a partition held as vectors, every row's at
//! once, as `window.rs` and `value.rs` find them row after row (RULES.md
//! D560). A rank is arithmetic of the row's place and its peer group's;
//! LEAD, LAG and a value function pick the argument of a row found by
//! arithmetic, under IGNORE NULLS among the rows whose argument is not
//! null, counted by a prefix count of them.

use super::frames::Part;
use super::lanes::{Built, Lane};
use crate::exec::window::frame::Ranges;
use crate::exec::window::value::{Count, tile};
use crate::logical::{Offset, Pick, WindowKind};
use crate::value::{Decimal, Value};

/// Adds to `out` the value of the rank function or NTILE `kind`, of
/// `tiles`, of each row of `part` [§9.23 SR5, §6.10 GR1a], as `window::rank`
/// gives it.
pub(in crate::exec::window) fn ranks(kind: &WindowKind, tiles: Count, part: &Part, out: &mut Built) {
    let len = part.len;
    for row in 0..len {
        let peers = part.peers(row);
        match kind {
            WindowKind::RowNumber => out.push_count(row.saturating_add(1)),
            WindowKind::Rank => out.push_count(peers.0.saturating_add(1)),
            WindowKind::DenseRank => out.push_count(part.group_of(row).saturating_add(1)),
            WindowKind::PercentRank => {
                let others = len.saturating_sub(1);
                out.push(Value::Approx(if others == 0 { 0.0_f64 } else { real(peers.0) / real(others) }));
            }
            WindowKind::CumeDist => out.push(Value::Approx(real(peers.1) / real(len.max(1)))),
            WindowKind::Ntile(_) => match tiles {
                Count::Of(n) => out.push_count(tile(row, len, n)),
                Count::Null | Count::Unused => out.push(Value::Null),
            },
            WindowKind::Aggregate(_) | WindowKind::Offset(_) | WindowKind::Value(_) | WindowKind::Measure(_) => out.push(Value::Null),
        }
    }
}

/// The `f64` nearest to the count `n`.
fn real(n: usize) -> f64 {
    Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0).to_f64()
}

/// The rows of a lane whose value is not null, in order, and of each place
/// how many are before it: one more than there are places.
struct NotNull {
    places: Vec<usize>,
    before: Vec<usize>,
}

impl NotNull {
    /// Of `lane`.
    fn of(lane: &Lane) -> NotNull {
        let (mut places, mut before) = (Vec::new(), Vec::with_capacity(lane.len().saturating_add(1)));
        for at in 0..lane.len() {
            before.push(places.len());
            if !lane.is_null(at) {
                places.push(at);
            }
        }
        before.push(places.len());
        NotNull { places, before }
    }

    /// How many are before place `at`; all of a place past the last.
    fn before(&self, at: usize) -> usize {
        self.before.get(at).copied().unwrap_or(self.places.len())
    }
}

/// [§6.10 GR1b] Adds to `out` LEAD or LAG `o` of each row of a partition:
/// the extent `extent` of the row `o.offset` rows after or before it,
/// under IGNORE NULLS of those whose extent is not null, else its default
/// of `defaults`; of an offset of 0, its own extent.
pub(in crate::exec::window) fn offsets(o: &Offset, (extent, defaults): (&Lane, &Lane), out: &mut Built) {
    let (len, offset) = (extent.len(), usize::try_from(o.offset).unwrap_or(usize::MAX));
    let not_null = o.ignore_nulls.then(|| NotNull::of(extent));
    for row in 0..len {
        if offset == 0 {
            out.push(extent.value(row));
            continue;
        }
        let target = match &not_null {
            None => {
                if o.lead {
                    row.checked_add(offset).filter(|&t| t < len)
                } else {
                    row.checked_sub(offset)
                }
            }
            Some(n) => {
                let k = if o.lead {
                    n.before(row.saturating_add(1)).checked_add(offset.saturating_sub(1)).filter(|&k| k < n.places.len())
                } else {
                    n.before(row).checked_sub(offset)
                };
                k.and_then(|k| n.places.get(k).copied())
            }
        };
        out.push(target.map_or_else(|| defaults.value(row), |at| extent.value(at)));
    }
}

/// [§6.10 GR1c, GR1d] Adds to `out` the value function `pick` of `count`
/// over each of `frames`: the argument `arg` of its first, last or nth
/// row, under IGNORE NULLS (`ignore`) of those whose argument is not null;
/// null if there is none.
pub(in crate::exec::window) fn values((pick, count, ignore): (&Pick, Count, bool), arg: &Lane, frames: &[Ranges], out: &mut Built) {
    let not_null = ignore.then(|| NotNull::of(arg));
    for frame in frames {
        let mut spans = [(0_usize, 0_usize); 3];
        for (span, &(from, to)) in spans.iter_mut().zip(frame).filter(|(_, r)| r.0 < r.1) {
            *span = not_null.as_ref().map_or((from, to), |n| (n.before(from), n.before(to)));
        }
        let total = spans.iter().fold(0_usize, |t, &(first, past)| t.saturating_add(past.saturating_sub(first)));
        let k = match (pick, count) {
            (Pick::First, _) => (total > 0).then_some(0),
            (Pick::Last, _) => total.checked_sub(1),
            (Pick::Nth { from_last: true, .. }, Count::Of(n)) => total.checked_sub(n),
            (Pick::Nth { from_last: false, .. }, Count::Of(n)) => n.checked_sub(1).filter(|&k| k < total),
            (Pick::Nth { .. }, Count::Null | Count::Unused) => None,
        };
        let at = k.and_then(|mut k| {
            spans.iter().find_map(|&(first, past)| {
                let here = past.saturating_sub(first);
                if k < here {
                    return Some(first.saturating_add(k));
                }
                k = k.saturating_sub(here);
                None
            })
        });
        let place = at.and_then(|at| not_null.as_ref().map_or(Some(at), |n| n.places.get(at).copied()));
        out.push(place.map_or(Value::Null, |p| arg.value(p)));
    }
}

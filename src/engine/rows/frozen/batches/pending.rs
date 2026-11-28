//! \[DESIGN\] (RULES.md D366) The rows a column store's scan kept of a
//! group's runs and holds until they make a batch (`run.rs`), and the
//! values of each column judging them decoded, gathered at the rows kept
//! as each run is judged. A batch of them then takes those columns as
//! held, so a column a filter reads is decoded once, to judge its run,
//! and not again at the rows it kept.
//!
//! \[DESIGN\] (RULES.md D1266) It holds, too, the conjuncts every run it
//! gathered rows of judged exactly: a conjunct one of them did not is
//! not known true of that run's rows, so it is said of the batch only if
//! each run says it. A conjunct a run judged of its first rows alone is
//! said of none of the rows gathered, whose places are no longer the
//! run's.

use crate::storage::mask::index;
use crate::storage::{Column, Floats, Ints, Mask};

/// \[DESIGN\] The places of the rows kept and not yet given, and the
/// columns judging their runs decoded, of those rows.
#[derive(Debug, Default)]
pub(super) struct Pending {
    places: Vec<u32>,
    piles: Vec<(usize, Pile)>,
    /// The conjuncts judged exactly of every row held (`Judged::exact`).
    exact: u64,
}

/// \[DESIGN\] A column's values of the rows held, in their order: a vector
/// of integers of one scale or of approximate numbers; of any other kind,
/// or of a run that did not decode it, none, decoded at the places
/// instead.
#[derive(Debug)]
enum Pile {
    Ints(Ints),
    Floats(Floats),
    Off,
}

impl Pending {
    /// How many rows it holds.
    pub(super) fn len(&self) -> usize {
        self.places.len()
    }

    /// Whether it holds none.
    pub(super) fn is_empty(&self) -> bool {
        self.places.is_empty()
    }

    /// Holds the rows at `places` too, the rows `rows` of the run whose
    /// columns `decoded` judging decoded whole, `exact` the conjuncts it
    /// judged exactly of them.
    pub(super) fn add(&mut self, places: &[u32], (rows, decoded): (&[u32], &[(usize, Column)]), exact: u64) {
        if places.is_empty() {
            return;
        }
        let first = self.places.is_empty();
        self.exact = if first { exact } else { self.exact & exact };
        for (c, column) in decoded {
            if let Some((_, pile)) = self.piles.iter_mut().find(|(p, _)| p == c) {
                pile.add(column, rows);
            } else if first {
                let mut pile = Pile::of(column);
                pile.add(column, rows);
                self.piles.push((*c, pile));
            }
        }
        // A column this run did not decode cannot follow its rows.
        for (c, pile) in &mut self.piles {
            if !decoded.iter().any(|(d, _)| d == c) {
                *pile = Pile::Off;
            }
        }
        self.places.extend_from_slice(places);
    }

    /// The places of the rows held, the columns held of them and the
    /// conjuncts judged exactly of them all, it left holding none.
    pub(super) fn take(&mut self) -> (Vec<u32>, Vec<(usize, Column)>, u64) {
        let piles = std::mem::take(&mut self.piles).into_iter().filter_map(|(c, pile)| pile.made().map(|m| (c, m))).collect();
        (std::mem::take(&mut self.places), piles, std::mem::take(&mut self.exact))
    }
}

impl Pile {
    /// An empty pile of the kind of `column`.
    fn of(column: &Column) -> Pile {
        match column {
            Column::Ints(ints) => Pile::Ints(Ints::new(Vec::new(), ints.scale, None)),
            Column::Floats(_) => Pile::Floats(Floats::new(Vec::new(), None)),
            Column::Values(_) | Column::Texts(_) | Column::Datetimes(_) | Column::Bools(_) | Column::Nulls(_) | Column::Late(_) => {
                Pile::Off
            }
        }
    }

    /// Adds the rows `rows` of `column`, or is none if it is not of its
    /// kind.
    fn add(&mut self, column: &Column, rows: &[u32]) {
        let held = match (&mut *self, column) {
            (Pile::Ints(into), Column::Ints(from)) => {
                into.scale == from.scale && {
                    let before = into.units.len();
                    into.units.extend(rows.iter().map(|&r| from.units.get(index(r)).copied().unwrap_or(0)));
                    nulls_added(&mut into.nulls, before, from.nulls.as_ref(), rows);
                    true
                }
            }
            (Pile::Floats(into), Column::Floats(from)) => {
                let before = into.numbers.len();
                into.numbers.extend(rows.iter().map(|&r| from.numbers.get(index(r)).copied().unwrap_or(0.0_f64)));
                nulls_added(&mut into.nulls, before, from.nulls.as_ref(), rows);
                true
            }
            (Pile::Ints(_) | Pile::Floats(_) | Pile::Off, _) => false,
        };
        if !held {
            *self = Pile::Off;
        }
    }

    /// The vector it holds, if any.
    fn made(self) -> Option<Column> {
        match self {
            Pile::Ints(ints) => Some(Column::Ints(Ints::new(ints.units, ints.scale, ints.nulls))),
            Pile::Floats(floats) => Some(Column::Floats(floats)),
            Pile::Off => None,
        }
    }
}

/// Adds to `into`, of `before` rows, whether each row of `rows` is null
/// by `from`: none made while no row is.
fn nulls_added(into: &mut Option<Mask>, before: usize, from: Option<&Mask>, rows: &[u32]) {
    let Some(from) = from else {
        if let Some(mask) = into {
            mask.extend_unset(rows.len());
        }
        return;
    };
    let mask = into.get_or_insert_with(|| Mask::none(before));
    for &r in rows {
        mask.push(from.get(index(r)));
    }
}

#[cfg(test)]
mod tests;

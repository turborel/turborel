//! \[DESIGN\] (RULES.md D1313) The states of rows handed on ungrouped,
//! set as each row's. A row that is a group of its own gives its group
//! the state of one row: COUNT(\*) one, and a set function of an integer
//! column that integer taken once, or nothing of a null. So a column of
//! such states is made at its size, each group's written once in the
//! order the groups lie, where feeding the rows adds each into a state
//! made empty first, at a place far from the row's before. The states
//! are those the feed leaves. Any other column of states is fed.

use super::{Exact, Totals, Typed};
use crate::exec::batch::Taken;
use crate::storage::mask::index;
use crate::storage::{Batch, Column, Ints, Sel};

mod runs;

/// A morsel's batches as it holds them, each with the rows kept of it.
type Held = [(Batch, Option<Sel>)];

/// The integers of the column at `c` of `batch`, if it holds them so.
fn column(batch: &Batch, c: usize) -> Option<&Ints> {
    if let Some(Column::Ints(ints)) = batch.columns.get(c) { Some(ints) } else { None }
}

/// The integers of the column at `c` of every batch of `held`, and their
/// scale, if each batch holds the column as integers of one scale.
fn ints_of(held: &Held, c: usize) -> Option<(Vec<&Ints>, u32)> {
    let ints = held.iter().map(|(batch, _)| column(batch, c)).collect::<Option<Vec<_>>>()?;
    let scale = ints.first()?.scale;
    ints.iter().all(|of| of.scale == scale).then_some((ints, scale))
}

impl Typed {
    /// \[DESIGN\] (RULES.md D1313) Makes its states those of groups of
    /// one row each, the group at each place of `sources` of the row
    /// named there, its batch of `held` and its row in it, taken as
    /// `take` says. Gives whether a null was passed over, to be warned
    /// of. None, nothing made, of states it holds already, and of a
    /// function or values it does not set so, which are fed.
    pub(in crate::exec::table) fn set_apart(&mut self, take: &Taken, held: &Held, sources: &[(u32, u32)]) -> Option<bool> {
        match self {
            Typed::Count(counts) => {
                // [X3.135-1986 §5.8 GR4b] COUNT(*) counts rows.
                let Taken::Count = take else { return None };
                if !counts.is_empty() {
                    return None;
                }
                *counts = vec![1; sources.len()];
                Some(false)
            }
            Typed::Exact(exact) => exact.set_apart(take, held, sources),
            Typed::Approx(_) | Typed::Best(_) => None,
        }
    }
}

impl Exact {
    /// As `Typed::set_apart`, of a column of integers of one scale: each
    /// group's total or best value its row's integer, taken once, and of
    /// a null nothing taken [X3.135-1986 §5.8 GR1].
    fn set_apart(&mut self, take: &Taken, held: &Held, sources: &[(u32, u32)]) -> Option<bool> {
        let Taken::Column(c) = take else { return None };
        let (ints, scale) = ints_of(held, *c)?;
        if self.totals.len() != 0 || self.scale.is_some_and(|of| of != scale) {
            return None;
        }
        let (mut units, mut counts) = (Vec::with_capacity(sources.len()), Vec::with_capacity(sources.len()));
        let mut any = false;
        for &(b, r) in sources {
            let r = index(r);
            let read = ints.get(index(b)).filter(|of| !of.is_null(r)).and_then(|of| of.units.get(r));
            any = any || read.is_none();
            units.push(read.copied().unwrap_or(0));
            counts.push(i64::from(read.is_some()));
        }
        (self.totals, self.counts, self.scale) = (Totals::Narrow(units), counts, Some(scale));
        Some(any)
    }
}

//! \[DESIGN\] (RULES.md D1308) The exact totals of a column of states, a
//! group each, in one of two forms. A grouping into many groups writes
//! each group's states and reads them again, so the bytes a total takes
//! are memory the statement moves, and most totals never leave 64 bits.
//! A column holds its totals in 64 bits while every one of them fits, and
//! is copied to 128 bits, once, at the first add or value that leaves 64.
//!
//! The add is checked, so the form follows what was added, and nothing is
//! asked of the argument's type or of how many rows a group may take. A
//! total is the same integer in either form. Whatever finishes, writes or
//! compares one reads it as 128 bits, so a group's result, its error and
//! its written state do not tell the form its column held it in. A sum
//! past 128 bits is raised by the add in 128 bits, as before, since a sum
//! of two integers of 64 bits never leaves 128.

use super::{beyond, null_at};
use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::storage::Ints;
use crate::storage::mask::index;

/// The totals or best values of a column of exact states, a group each.
#[derive(Debug)]
pub(super) enum Totals {
    /// Every total fits 64 bits.
    Narrow(Vec<i64>),
    /// As wide as the states sum in.
    Wide(Vec<i128>),
}

impl Totals {
    /// Of no group yet, narrow.
    pub(super) const fn new() -> Totals {
        Totals::Narrow(Vec::new())
    }

    /// How many groups.
    pub(super) fn len(&self) -> usize {
        match self {
            Totals::Narrow(totals) => totals.len(),
            Totals::Wide(totals) => totals.len(),
        }
    }

    /// Group `g`'s total, if it holds the group.
    pub(super) fn get(&self, g: usize) -> Option<i128> {
        match self {
            Totals::Narrow(totals) => totals.get(g).copied().map(i128::from),
            Totals::Wide(totals) => totals.get(g).copied(),
        }
    }

    /// Adds a group's, of nothing.
    pub(super) fn grow(&mut self) {
        match self {
            Totals::Narrow(totals) => totals.push(0),
            Totals::Wide(totals) => totals.push(0),
        }
    }

    /// Adds `more` groups', of nothing.
    pub(super) fn grow_by(&mut self, more: usize) {
        match self {
            Totals::Narrow(totals) => totals.resize(totals.len().saturating_add(more), 0),
            Totals::Wide(totals) => totals.resize(totals.len().saturating_add(more), 0),
        }
    }

    /// Room for `more` groups more.
    pub(super) fn reserve(&mut self, more: usize) {
        match self {
            Totals::Narrow(totals) => totals.reserve(more),
            Totals::Wide(totals) => totals.reserve(more),
        }
    }

    /// Keeps the first `len` groups', in the room they take.
    pub(super) fn truncate(&mut self, len: usize) {
        match self {
            Totals::Narrow(totals) => {
                totals.truncate(len);
                totals.shrink_to_fit();
            }
            Totals::Wide(totals) => {
                totals.truncate(len);
                totals.shrink_to_fit();
            }
        }
    }

    /// The totals of the groups at `order`, in that order, in the form
    /// they are held in.
    pub(super) fn gathered(&self, order: &[u32]) -> Totals {
        match self {
            Totals::Narrow(totals) => Totals::Narrow(order.iter().map(|&i| totals.get(index(i)).copied().unwrap_or(0)).collect()),
            Totals::Wide(totals) => Totals::Wide(order.iter().map(|&i| totals.get(index(i)).copied().unwrap_or(0)).collect()),
        }
    }

    /// The bytes it holds, at the width of its form.
    pub(super) fn bytes(&self) -> usize {
        match self {
            Totals::Narrow(totals) => totals.capacity().saturating_mul(size_of::<i64>()),
            Totals::Wide(totals) => totals.capacity().saturating_mul(size_of::<i128>()),
        }
    }

    /// Copies narrow totals to 128 bits, with the room they had. Wide
    /// ones stay as they are.
    pub(super) fn widen(&mut self) {
        if let Totals::Narrow(totals) = self {
            let mut wide = Vec::with_capacity(totals.capacity());
            wide.extend(totals.iter().copied().map(i128::from));
            *self = Totals::Wide(wide);
        }
    }

    /// Adds `units` to group `g`'s total: in 64 bits where the sum fits
    /// them, else in 128, the column widened first.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 of a sum past 128 bits.
    pub(super) fn add(&mut self, g: usize, units: i128) -> Result<()> {
        if let Totals::Narrow(totals) = self {
            let Some(total) = totals.get_mut(g) else { return Ok(()) };
            if let Some(sum) = i64::try_from(units).ok().and_then(|unit| total.checked_add(unit)) {
                *total = sum;
                return Ok(());
            }
            self.widen();
        }
        if let Totals::Wide(totals) = self
            && let Some(total) = totals.get_mut(g)
        {
            *total = total.checked_add(units).map_or_else(beyond, Ok)?;
        }
        Ok(())
    }

    /// Makes group `g`'s total `units`, the column widened first if 64
    /// bits do not hold it.
    pub(super) fn set(&mut self, g: usize, units: i128) {
        if let Totals::Narrow(totals) = self {
            let Some(total) = totals.get_mut(g) else { return };
            if let Ok(unit) = i64::try_from(units) {
                *total = unit;
                return;
            }
            self.widen();
        }
        if let Totals::Wide(totals) = self
            && let Some(total) = totals.get_mut(g)
        {
            *total = units;
        }
    }

    /// Adds to its totals and to `counts`, a group each, the integers of
    /// `ints` at `places` of the rows of groups `groups`, in turn, while
    /// the column is narrow and each sum fits 64 bits. Gives how many rows
    /// it took, and whether a null was passed over among them. The row it
    /// stops at, and those after it, are not taken: every row of a narrow
    /// column no sum of which leaves 64 bits is, and none of a wide one.
    pub(super) fn fed(&mut self, counts: &mut [i64], ints: &Ints, (places, groups): (&[u32], &[u32])) -> (usize, bool) {
        let Totals::Narrow(totals) = self else { return (0, false) };
        let mut any = false;
        for (n, (&at, &g)) in places.iter().zip(groups).enumerate() {
            let (at, g) = (index(at), index(g));
            if null_at(ints.nulls.as_ref(), at) {
                any = true;
            } else if let (Some(total), Some(held)) = (totals.get_mut(g), counts.get_mut(g)) {
                let Some(sum) = total.checked_add(ints.units.get(at).copied().unwrap_or(0)) else { return (n, any) };
                (*total, *held) = (sum, held.saturating_add(1));
            }
        }
        (places.len().min(groups.len()), any)
    }

    /// Appends the totals of the groups at `places`, 128 bits each
    /// whatever the form (RULES.md D1226).
    pub(super) fn put_groups(&self, places: &[u32], out: &mut Vec<u8>) {
        for &i in places {
            out.put_i128(self.get(index(i)).unwrap_or(0));
        }
    }

    /// Reads the totals of `len` groups `put_groups` wrote, narrow if 64
    /// bits hold every one.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the bytes end before them.
    pub(super) fn read(reader: &mut Reader<'_>, len: usize) -> Result<Totals> {
        let mut totals = Totals::new();
        for g in 0..len {
            totals.grow();
            totals.set(g, reader.i128()?);
        }
        Ok(totals)
    }
}

// Unit tests: totals in either form are the same integers, added, set,
// gathered, written and read back, across the widening (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;

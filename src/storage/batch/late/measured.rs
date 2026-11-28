//! \[DESIGN\] (RULES.md D1267) A late column that holds its values' octet
//! lengths. A reader that takes OCTET_LENGTH of a column and never reads
//! its text needs no text made, and a column store knows a VARCHAR
//! value's length from how it is stored. Its scan gives such a column
//! late (`late.rs`) with the length of the value at each of its places
//! beside them, and the function reads those.
//!
//! The column is still the column. It keeps where its rows are and how
//! to decode them, so a reader that wants its text, by plan or by a path
//! no plan foresaw, decodes it as it would any late column and reads the
//! values the scan would have given. The lengths are a second way to
//! read one property of those values, so a column given measured where
//! its text is read after all costs a decoding and changes no result.

use std::sync::Arc;

use super::super::mask::index;
use super::super::{Ints, Mask};
use super::Late;

impl Late {
    /// It, holding `lengths`, the octet length of the value at each of
    /// its places in order, a null's null. Lengths not as many as its
    /// places are not held.
    #[must_use]
    pub fn measured(mut self, lengths: Ints) -> Late {
        if lengths.units.len() == self.positions.len() && lengths.scale == 0 {
            self.lengths = Some(Arc::new(lengths));
        }
        self
    }

    /// Whether it holds its values' octet lengths.
    pub fn is_measured(&self) -> bool {
        self.lengths.is_some()
    }

    /// The octet length of each of its rows' values, as exact numbers of
    /// scale 0, a null of a row that is null or that a join paired with
    /// none. None if its scan did not measure it.
    pub fn lengths(&self) -> Option<Ints> {
        let held = self.lengths.as_deref()?;
        let Some(picks) = &self.picks else { return Some(Ints::new(held.units.clone(), 0, held.nulls.clone())) };
        let mut nulls = Mask::none(picks.len());
        let mut units = Vec::with_capacity(picks.len());
        for (n, pick) in picks.iter().enumerate() {
            let place = pick.map(index).filter(|&p| p < held.units.len() && !held.is_null(p));
            if place.is_none() {
                nulls.set(n);
            }
            units.push(place.and_then(|p| held.units.get(p)).copied().unwrap_or(0));
        }
        Some(Ints::new(units, 0, nulls.any().then_some(nulls)))
    }
}

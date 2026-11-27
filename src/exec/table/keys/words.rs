//! \[DESIGN\] (RULES.md D1274) A lane of integers compared by its words.
//! A key looked for in a table comes as a row of a batch or as a group
//! of another table, and where its value of a lane is read from a vector
//! of integers of the lane's scale, the two are equal as their units
//! are. So no cell is made of either and no bytes are written, as the
//! compare by cells does for every type (`Lane::equal`), which any other
//! pair still takes. A table of many groups is found in by a key
//! compared with one group or two, each read from memory the processor
//! does not hold. With no null in the lane (`Lane::is_null`) the compare
//! reads one word of it where it read two.

use super::{Data, Key, Lane};
use crate::storage::Column;

impl Key<'_> {
    /// \[DESIGN\] (RULES.md D1274) Its value of the `lane`th grouping
    /// column as the units of an integer of `scale`, and whether it is
    /// null, where it is read from a vector of integers of that scale,
    /// a batch's column or another table's lane. None of any other,
    /// which is compared as a cell.
    fn word(self, lane: usize, scale: u32) -> Option<(i64, bool)> {
        let (unit, same, null) = match self {
            Key::Row(batch, at, r) => {
                let Column::Ints(ints) = batch.columns.get(*at.get(lane)?)? else { return None };
                (ints.units.get(r), ints.scale == scale, ints.is_null(r))
            }
            Key::Group(keys, g) => {
                let theirs = keys.lanes.get(lane)?;
                let Data::Ints { units, scale: of } = &theirs.data else { return None };
                (units.get(g), *of == scale, theirs.is_null(g))
            }
            Key::Values(_) => return None,
        };
        same.then(|| (unit.copied().unwrap_or(0), null || unit.is_none()))
    }
}

impl Lane {
    /// \[DESIGN\] (RULES.md D1274) Whether group `g`'s equals `key`'s
    /// value of this lane, the `lane`th, told by their words. That is of
    /// a lane of integers against integers of its scale read from a
    /// vector (`Key::word`), nulls equal to each other alone. None of
    /// any other pair, which is compared as cells.
    pub(super) fn equal_words(&self, g: usize, key: Key<'_>, lane: usize) -> Option<bool> {
        let Data::Ints { units, scale } = &self.data else { return None };
        let (theirs, null) = key.word(lane, *scale)?;
        let mine = self.is_null(g);
        Some(if mine || null { mine && null } else { units.get(g) == Some(&theirs) })
    }
}

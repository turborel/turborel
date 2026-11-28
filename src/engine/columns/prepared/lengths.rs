//! \[DESIGN\] (RULES.md D1267) The octet lengths a scan took of a run to
//! judge a conjunct of a measured column, kept for the batch. A scan
//! that gives a column measured reads each value's length of its chunks
//! (`scan/lengths.rs`). A conjunct that compares such a column unpadded
//! with the empty string is judged of those lengths
//! (`judged/lengths.rs`), and the lengths of a run judged whole are held
//! here until the run's batch is made, which takes them, so the run's
//! chunks are read for their lengths once, as a column decoded to be
//! judged is handed on as the batch's.

use super::Prepared;
use crate::storage::Ints;

/// The columns a scan gives measured, and the lengths held of one run.
#[derive(Default)]
pub(super) struct Lengths {
    columns: Vec<usize>,
    /// A column's lengths of every row of the run from the first place
    /// to the second, as the scan took them to judge it.
    held: Option<(usize, (u32, u32), Ints)>,
}

impl Lengths {
    /// Lets what is held go, as the next group is taken up.
    pub(super) fn clear(&mut self) {
        self.held = None;
    }
}

impl Prepared {
    /// Notes `columns` as those the scan gives measured, for every
    /// group it reads.
    pub fn measure(&mut self, columns: &[usize]) {
        self.lengths.columns = columns.to_vec();
    }

    /// Whether the scan gives column `column` measured.
    pub(in crate::engine::columns) fn measures(&self, column: usize) -> bool {
        self.lengths.columns.contains(&column)
    }

    /// Holds `lengths`, those of every row of the run `from` to `to` of
    /// column `column`, in place of any held before.
    pub(in crate::engine::columns) fn hold_lengths(&mut self, column: usize, (from, to): (u32, u32), lengths: Ints) {
        self.lengths.held = Some((column, (from, to), lengths));
    }

    /// The lengths held of column `column` if they are those of the rows
    /// at `places`, ascending, every row of one run. They are given once.
    pub fn take_lengths(&mut self, column: usize, places: &[u32]) -> Option<Ints> {
        let (held, (from, to), lengths) = self.lengths.held.as_ref()?;
        let whole = usize::try_from(to.saturating_sub(*from)).is_ok_and(|rows| rows == places.len() && rows == lengths.units.len());
        if *held != column || !whole || places.first() != Some(from) || places.last().copied() != to.checked_sub(1) {
            return None;
        }
        self.lengths.held.take().map(|(_, _, taken)| taken)
    }
}

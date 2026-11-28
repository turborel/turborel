//! \[DESIGN\] (RULES.md D1266) What a scan judged of a batch it gives,
//! said beside the batch on the scan's own stream
//! (`TableStorage::scan_vetted`, `Split::morsel_vetted`), for the first
//! filter over the scan to read. A scan judges the conjuncts it is told
//! before it decodes the columns its filter does not read, and the filter
//! judges every row given (RULES.md D15), so a conjunct the scan judged of
//! a run it gives whole was judged twice of each row. Here the scan says
//! which rows of the batch its judging kept and which of the told
//! conjuncts it judged exactly of them, and the filter starts from those
//! rows and passes over those conjuncts.
//!
//! It is no part of a batch. A batch is what every operator reads and
//! passes on, and what a scan judged is true only of the batch as the
//! scan gave it, against the condition that scan was told, so no operator
//! but the filter whose conjuncts the scan was told may read it.

use super::{Batch, Batches};
use crate::error::Result;

/// \[DESIGN\] (RULES.md D1266) The told conjuncts a set of them holds:
/// those at the places a word has bits for. A conjunct told past them is
/// never said to be judged exactly, and the filter judges it.
pub const VETTED_PLACES: usize = 64;

/// \[DESIGN\] (RULES.md D1266) What a scan judged of a batch it gives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vetted {
    /// The rows of the batch the conjuncts the scan judged keep,
    /// ascending, or none if they keep every row of it. A told conjunct
    /// is false of each other row, or a sort's bound leaves it out, so
    /// the filter over the scan keeps none of those.
    pub rows: Option<Vec<u32>>,
    /// The told conjuncts judged exactly, a bit set for each by its place
    /// in the condition (`Vetted::bit`): the conjunct itself was
    /// evaluated of every row of `rows`, or of the batch, and is true of
    /// each. Not set of a conjunct judged by a summary alone, of a bound
    /// that may tighten, of one some row was passed unjudged by, nor of a
    /// batch holding a row the scan did not judge.
    pub exact: u64,
    /// One more told conjunct, by its bit, judged exactly of the rows of
    /// `rows`, or of the batch, before the row at a place alone, the
    /// rows from that place on kept unjudged: as a LIKE is whose judging
    /// stopped part way through a run given whole (RULES.md D1251). The
    /// filter judges it of the rows from that place on.
    pub partly: Option<(u64, u32)>,
}

impl Vetted {
    /// The bit of the told conjunct at place `at` of the condition: none
    /// set of a place past `VETTED_PLACES`.
    pub fn bit(at: usize) -> u64 {
        u32::try_from(at).ok().and_then(|at| 1_u64.checked_shl(at)).unwrap_or(0)
    }
}

/// \[DESIGN\] (RULES.md D1266) Batches of a scan's rows, each with what
/// the scan judged of it, none of a batch it judged nothing of.
pub type VettedBatches = Box<dyn Iterator<Item = Result<(Batch, Option<Vetted>)>> + Send>;

/// `batches`, nothing said of any: the default of an engine that judges
/// no conjunct itself, or says nothing of what it judged.
pub fn unvetted(batches: Batches) -> VettedBatches {
    Box::new(batches.map(|batch| batch.map(|b| (b, None))))
}

// Unit tests: the bit of a place (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] (RULES.md D1266) a told conjunct's bit is its
    /// own, apart from every other's, up to the last place a word holds,
    /// and a conjunct told past that has none, so it is never said to be
    /// judged exactly.
    #[test]
    fn a_place_past_the_word_has_no_bit() {
        assert_eq!(Vetted::bit(0), 1);
        assert_eq!(Vetted::bit(VETTED_PLACES - 1), 0x8000_0000_0000_0000_u64);
        assert_eq!(Vetted::bit(3) & Vetted::bit(4), 0);
        assert_eq!(Vetted::bit(VETTED_PLACES), 0);
        assert_eq!(Vetted::bit(usize::MAX), 0);
    }
}

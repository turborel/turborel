//! \[DESIGN\] (RULES.md D354, D293) What a grouping run in parallel reads
//! of a batch of its pipeline, and the number its morsels' rows are
//! numbered from: the first rows a group is numbered by, a morsel's
//! number above its row's place, so the groups of every partition merge
//! into the order the serial grouping gives them.

use super::super::batched::ByBatch;
use super::Grouping;
use crate::exec::batch::{Read, read_for};
use crate::exec::group::unpadded_keys;

/// \[DESIGN\] The bits of a group's number its row's place in its morsel
/// takes: more than any morsel's rows need.
const PLACE_BITS: u32 = 32;

/// \[DESIGN\] (RULES.md D354) What `grouping` reads of each batch of a
/// pipeline read `by` batch, if it reads them so: of DISTINCT, every
/// column of batches holding every column of the rows; else its grouping
/// columns and set functions as a grouping by batch reads them, no seed
/// among them, nor a collation but one whose key is the text itself
/// (RULES.md D1183).
pub(super) fn read_of(grouping: &Grouping, by: &ByBatch) -> Option<Read> {
    if grouping.distinct {
        let whole = by.layout.iter().copied().eq(0..by.width);
        return whole.then(|| read_for(&by.layout, &[], &by.layout)).flatten();
    }
    let spec = &grouping.spec;
    if !spec.seeds.is_empty() {
        return None;
    }
    let unpadded = unpadded_keys(&spec.collations)?;
    read_for(&spec.groups, &spec.aggs, &by.layout).map(|read| Read { unpadded, ..read })
}

/// The number of the first row of morsel `m`: after the seeds, numbered
/// first, its morsel's number in the high bits.
pub(super) fn first_of(grouping: &Grouping, m: usize) -> u64 {
    let seeds = u64::try_from(grouping.spec.seeds.len()).unwrap_or(u64::MAX);
    seeds.saturating_add(u64::try_from(m).unwrap_or(u64::MAX).checked_shl(PLACE_BITS).unwrap_or(u64::MAX))
}

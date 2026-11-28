//! \[DESIGN\] (RULES.md D1226) A table's groups, frozen whole, written to
//! a spill and read back as they were: how many, their keys, hashes and
//! first rows' numbers, and each set function's states, so a partition of
//! a grouping run in parallel is held in the spill between waves, not in
//! memory. No two groups written hold one key, as a morsel's rows handed
//! on ungrouped are grouped before they are written (RULES.md D1260). A partition is written in pieces of at most `PIECE` groups each,
//! one record a piece, so neither its writing nor its reading holds its
//! groups twice. A piece read back is a table's groups frozen whole, its
//! groups in the order written, merged into another table as any other.

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::logical::AggCall;

use super::super::states::Sent;
use super::super::{Keys, Seed, narrow};
use super::{Made, part_of};

/// \[DESIGN\] The most groups one record holds: few enough that a record
/// is small beside the grant, many enough that a record's lanes are each
/// written in one run.
pub(in crate::exec) const PIECE: usize = 1 << 16;

impl Made {
    /// How many groups it holds.
    pub(in crate::exec) fn len(&self) -> usize {
        self.hashes.len()
    }

    /// The places of partition `part`'s groups, in their order.
    pub(in crate::exec) fn part_places(&self, part: usize) -> &[u32] {
        self.places_of(part)
    }

    /// Appends the groups at `places`, in that order: whether every lane
    /// and set function could write them.
    pub(in crate::exec) fn put_groups(&self, places: &[u32], out: &mut Vec<u8>) -> bool {
        let words = |words: &[u64], to: &mut Vec<u8>| {
            for &i in places {
                to.put_u64(words.get(crate::storage::mask::index(i)).copied().unwrap_or(0));
            }
        };
        out.put_u32(narrow(places.len()));
        words(&self.hashes, out);
        words(&self.firsts, out);
        self.keys.put_groups(places, out) && self.states.iter().all(|sent| sent.put_groups(places, out))
    }

    /// Reads groups `put_groups` wrote, of the set functions `aggs`.
    ///
    /// # Errors
    ///
    /// SQLCODE -901 if `bytes` are not such groups.
    pub(in crate::exec) fn read(bytes: &[u8], aggs: &[AggCall]) -> Result<Made> {
        let mut reader = Reader::new(bytes, "a partition's groups spilled");
        let len = crate::storage::mask::index(reader.u32()?);
        let hashes = (0..len).map(|_| reader.u64()).collect::<Result<Vec<_>>>()?;
        let firsts = (0..len).map(|_| reader.u64()).collect::<Result<Vec<_>>>()?;
        let keys = Keys::read(&mut reader, len)?;
        let states = aggs.iter().map(|agg| Sent::read(&mut reader, len, agg)).collect::<Result<Vec<_>>>()?;
        reader.finish()?;
        Ok(Made { keys, hashes, firsts, states, places: Vec::new(), starts: Vec::new(), keyed: None, repeats: false })
    }

    /// \[DESIGN\] (RULES.md D1113, D1226) The partition of `parts`, at
    /// `level` levels below the first, that group `g` falls in: of the hash
    /// its partition was found by, its key's of `lanes` alone if any, else
    /// its whole key's, six bits below those of the level above, so a
    /// partition split again keeps every group of a value of `lanes` in
    /// one part.
    pub(in crate::exec) fn part_at(&self, g: usize, (lanes, seed): (&[usize], &Seed), (parts, level): (usize, u32)) -> usize {
        let hash = if lanes.is_empty() { self.hashes.get(g).copied().unwrap_or(0) } else { self.keys.hash_of(g, lanes, seed) };
        part_of(hash.checked_shl(level.saturating_mul(PART_BITS)).unwrap_or(0), parts)
    }
}

/// The bits of a hash each level of partitions takes.
const PART_BITS: u32 = 6;

// Unit tests: groups read back merge as those written (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

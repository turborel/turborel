//! \[DESIGN\] (RULES.md D1253) A LIKE judged of a chunk kept as a
//! dictionary once an entry, its rows then by their codes. A dictionary
//! keeps its entries' octets one after another, as they were written,
//! so they are one arena whose values end where the entries do, and the
//! pattern's longest piece is searched for in all of it at once by the
//! database's kernels (`value/like/arena.rs`, D1119), which compare a
//! vector of places at a time. An entry the search never stops in does
//! not match, and one it stops in is matched whole by its pieces.
//!
//! Each entry is matched once, where a row at a time matches the entry
//! of each row asked for. Both search the same octets by the same
//! pieces, so the entries cost no more than the rows once the rows asked
//! of the chunk are no fewer than its entries, whatever kernels search,
//! and that is when the entries are judged. Of fewer rows, as an earlier
//! conjunct leaves of a dictionary of many entries, each row's entry is
//! matched alone (`coded.rs`), and the entries no row asks for are never
//! read.

use super::super::encoded::Body;
use super::super::packed::get;
use super::super::{Chunk, WORD, read_word};
use crate::kernels::Kernels;
use crate::storage::Liked;
use crate::storage::mask::index;

/// Pushes to `out` the rows of `these`, places from `from`, that `like`
/// keeps of `chunk`, a dictionary whose first row is `first`, each entry
/// judged once, searched for by `kernels`, and each row by its code:
/// none of a null. None, nothing pushed, if the chunk is not a
/// dictionary, if the rows asked are fewer than its entries, which are
/// then matched a row at a time, or if its entries do not lie one after
/// another within its octets or a code is of no entry.
pub(super) fn liked(
    chunk: &Chunk<'_>,
    (like, kernels): (&Liked, &dyn Kernels),
    (first, from): (u32, u32),
    these: &[u32],
    out: &mut Vec<u32>,
) -> Option<()> {
    let Body::Dictionary { bits, offsets, entries, codes, .. } = &chunk.body else { return None };
    let count = offsets.len().checked_div(WORD)?.checked_sub(1)?;
    if these.len() < count {
        return None;
    }
    let start = read_word(offsets, 0).ok()?;
    let ends = (1..=count).map(|e| read_word(offsets, e.checked_mul(WORD)?).ok()?.checked_sub(start)).collect::<Option<Vec<usize>>>()?;
    // Entries one after another, each within the octets held.
    if !ends.is_sorted() {
        return None;
    }
    let arena = entries.get(start..start.checked_add(ends.last().copied().unwrap_or(0))?)?;
    let kept = like.keeps_each((arena, &ends), kernels);
    let mut rows = Vec::with_capacity(these.len());
    for &r in these {
        let n = index(from.saturating_add(r).saturating_sub(first));
        if chunk.is_null(n) {
            continue;
        }
        if *kept.get(usize::try_from(get(codes, *bits, n)?).ok()?)? {
            rows.push(r);
        }
    }
    out.extend(rows);
    Some(())
}

// Unit tests: a dictionary's entries judged once against each row's
// entry matched alone (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;

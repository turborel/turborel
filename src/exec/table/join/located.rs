//! \[DESIGN\] (RULES.md D602) A batch's probe of a join's table in two
//! passes. The first reads the directory word of each probing row's hash
//! and keeps the rows whose slot holds its tag, as hits, each with where
//! its slot's rows begin and end: a tight loop of loads no one of which
//! waits on another, so a processor has many misses of a table past its
//! caches in flight at once, as a vectorized probe's independent loads
//! hide them (Kersten et al., PVLDB 2018, §4.1; Sompolski, Zukowski and
//! Boncz, DaMoN 2011, §4). The second reads the rows of the hits alone,
//! the rows no built row can match left out before any is compared, as
//! Behm et al. (SIGMOD 2022, §6.4) compact a sparse batch before its
//! probe.
//!
//! Of a directory past `PREFETCH_BYTES`, the first pass asks the
//! processor for the words of the next `GROUP` rows' slots while it reads
//! those of the present ones (`Kernels::prefetch`), as Menon, Mowry and
//! Pavlo (PVLDB 2017, §5.5) prefetch a group of probes ahead.

use super::{Candidates, JoinTable};
use crate::kernels::Kernels;

/// \[DESIGN\] (RULES.md D602) The bytes of a directory past which a probe
/// asks for its words ahead of their reads: beyond a share of a last-level
/// cache, below which they are mostly there already.
pub(in crate::exec) const PREFETCH_BYTES: usize = 8 << 20;

/// \[DESIGN\] (RULES.md D602) The rows whose directory words a probe asks
/// for at once, a group ahead of those it reads: the best group of Menon,
/// Mowry and Pavlo (PVLDB 2017, Fig. 4).
const GROUP: usize = 16;

/// A probing row whose slot holds its hash's tag: its place among the
/// probing rows, and where its slot's rows begin and end.
#[derive(Debug, Clone, Copy)]
pub(in crate::exec) struct Hit {
    /// Its place among the hashes the probe gave.
    at: u32,
    from: u32,
    to: u32,
}

impl Hit {
    /// Its place among the hashes the probe gave.
    pub(in crate::exec) fn place(self) -> usize {
        usize::try_from(self.at).unwrap_or(usize::MAX)
    }
}

impl JoinTable {
    /// Puts in `hits`, in order, each of the probing rows whose key's
    /// `hashes` are given, but those `misses` marks, that a built row may
    /// match: of a hashed table, those whose slot holds their tag, asking
    /// `kernels` for the words of the next group's slots ahead of their
    /// reads if the directory is past `PREFETCH_BYTES`; of a direct one,
    /// those whose number has rows.
    pub(in crate::exec) fn locate(&self, (hashes, misses): (&[u64], &[bool]), kernels: &dyn Kernels, hits: &mut Vec<Hit>) {
        hits.clear();
        let keyed = hashes.iter().zip(misses).enumerate().filter(|(_, (_, m))| !**m);
        if let Some(dense) = &self.dense {
            let found = keyed.filter(|(_, (h, _))| !dense.candidates(**h).is_empty());
            hits.extend(found.filter_map(|(n, _)| Some(Hit { at: u32::try_from(n).ok()?, from: 0, to: 0 })));
            return;
        }
        if self.directory.len().saturating_mul(8) <= PREFETCH_BYTES {
            hits.extend(keyed.filter_map(|(n, (&h, _))| self.hit(n, h)));
            return;
        }
        let mut places = [0_u32; GROUP];
        for (g, group) in hashes.chunks(GROUP).enumerate() {
            let start = g.saturating_mul(GROUP);
            let next = hashes.get(start.saturating_add(GROUP)..).unwrap_or_default();
            let mut count = 0_usize;
            for (p, &h) in places.iter_mut().zip(next) {
                *p = u32::try_from(self.slot(h).saturating_add(1)).unwrap_or(u32::MAX);
                count = count.saturating_add(1);
            }
            kernels.prefetch(&self.directory, places.get(..count).unwrap_or_default());
            for (n, (&h, _)) in group.iter().zip(misses.get(start..).unwrap_or_default()).enumerate().filter(|(_, (_, m))| !**m) {
                hits.extend(self.hit(start.saturating_add(n), h));
            }
        }
    }

    /// The hit of the probing row of place `n` and hash `h`, if its slot
    /// holds its tag.
    fn hit(&self, n: usize, h: u64) -> Option<Hit> {
        let (from, to) = self.slot_of(h)?;
        Some(Hit { at: u32::try_from(n).ok()?, from: u32::try_from(from).ok()?, to: u32::try_from(to).ok()? })
    }

    /// The numbers of the rows of `hit`, whose hash is `h`, in the order
    /// they were built: those of `candidates(h)`.
    pub(in crate::exec) fn found(&self, hit: Hit, h: u64) -> Candidates<'_> {
        if let Some(dense) = &self.dense {
            return Candidates::Run(dense.candidates(h).iter());
        }
        let span = (usize::try_from(hit.from).unwrap_or(0), usize::try_from(hit.to).unwrap_or(0));
        self.within(span, h)
    }
}

// Unit tests: hits against a probe's candidates, and the prefetch asked
// for past its bound (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

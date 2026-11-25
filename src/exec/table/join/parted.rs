//! \[DESIGN\] (RULES.md D1080) A join's table made in parts, each by the
//! thread that takes it, of a built operand of more rows than a morsel
//! holds. A hashed table's slot is the high bits of a hash, so the rows
//! whose hashes share their `PART_BITS` highest bits own a run of slots
//! next to each other and no other row is placed among them: each such
//! part is a table of its own, its directory, its rows and their hashes,
//! made of its rows alone, and a probe finds its part by those bits. A
//! direct table (RULES.md D601) is parted likewise by the high bits of a
//! key's place in the span. No thread writes what another reads, so no
//! placing is atomic (RULES.md R-SAFE-1).
//!
//! The rows are first sorted by part, a run of them at a time
//! (`Parted`), each run by one thread, a counting sort that keeps the
//! rows of a part in the order they were built; a part is then made of
//! every run's rows of it, the runs in order, so a slot's rows, and a
//! key's, are in the order built, as the table made by one thread holds
//! them (`placed.rs`), and a probe finds the same rows in the same order
//! of either.

use super::direct::{DensePart, Span, place_of};
use super::{MIN_SLOTS, Part, placed};
use crate::storage::mask::index;

/// \[DESIGN\] The highest bits of a hash, or of a place in a span, that
/// say its part: 6, so a table has `BUILD_PARTS` parts.
const PART_BITS: u32 = 6;

/// \[DESIGN\] The parts a table built by many threads is made in: as many
/// as a parallel grouping's partitions (`parallel/probe.rs`), enough that
/// each thread of any degree takes several, a power of two, as a part is
/// the high bits of a slot.
pub(in crate::exec) const BUILD_PARTS: usize = 1 << PART_BITS;

/// How a table made in parts places its rows.
#[derive(Debug, Clone, Copy)]
pub(in crate::exec) enum Parting {
    /// A hashed table's: how far a hash is turned to give its slot, and
    /// how many of a slot's bits place it within its part.
    Hashed { shift: u32, within: u32 },
    /// A direct table's: its least key, the places of its span, and how
    /// many of a place's bits place it within its part.
    Dense { least: i64, width: usize, within: u32 },
}

/// A part of a table, made by one thread.
#[derive(Debug)]
pub(in crate::exec) enum BuiltPart {
    /// Of a hashed table.
    Hashed(Part),
    /// Of a direct table.
    Dense(DensePart),
}

/// A run of a build's rows sorted by part: each with its hash, or of a
/// direct table its key's place in the span.
#[derive(Debug, Default)]
pub(in crate::exec) struct Parted {
    pairs: Vec<(u32, u64)>,
    /// Where each part's pairs end, after a first 0.
    ends: Vec<usize>,
    /// Of a direct table, the run's rows of a null key whose nulls match.
    pub(super) nulls: Vec<u32>,
}

/// The low `bits` bits set.
pub(super) fn low_bits(bits: u32) -> usize {
    1_usize.checked_shl(bits).map_or(usize::MAX, |b| b.saturating_sub(1))
}

impl Parting {
    /// How a table of `keyed` rows that match is parted: direct if `span`,
    /// what its keys' numbers say of one, is dense (RULES.md D601), else
    /// hashed, of a slot for each two rows at least and `MIN_SLOTS` a
    /// part.
    pub(in crate::exec) fn of(span: Option<Span>, keyed: usize) -> Parting {
        if let Some((least, width)) = span.and_then(|s| s.dense()) {
            let size = width.div_ceil(BUILD_PARTS).checked_next_power_of_two().unwrap_or(1);
            return Parting::Dense { least, width, within: size.trailing_zeros() };
        }
        let fewest = MIN_SLOTS.saturating_mul(BUILD_PARTS);
        let slots = keyed.saturating_mul(2).max(fewest).checked_next_power_of_two().unwrap_or(fewest);
        let bits = slots.trailing_zeros();
        Parting::Hashed { shift: u64::BITS.saturating_sub(bits), within: bits.saturating_sub(PART_BITS) }
    }

    /// Whether the table is direct, its rows placed by their keys'
    /// numbers, not by hashes.
    pub(in crate::exec) fn direct(&self) -> bool {
        match self {
            Parting::Hashed { .. } => false,
            Parting::Dense { .. } => true,
        }
    }

    /// The part of a row and what places it within the part: of a hashed
    /// table, of its hash `word`, the hash; of a direct one, of its key's
    /// number `word`, the key's place in the span, none of a null's.
    fn placed(&self, word: u64) -> Option<(usize, u64)> {
        match self {
            Parting::Hashed { .. } => {
                let part = word.checked_shr(u64::BITS.saturating_sub(PART_BITS)).unwrap_or(0);
                Some((usize::try_from(part).unwrap_or(0), word))
            }
            Parting::Dense { least, within, .. } => {
                let place = place_of(word, *least)?;
                Some((place.checked_shr(*within).unwrap_or(0), u64::try_from(place).unwrap_or(u64::MAX)))
            }
        }
    }

    /// Part `p` of the table, of the rows of it in `runs`, in order.
    pub(in crate::exec) fn part(&self, runs: &[Parted], p: usize) -> BuiltPart {
        let count = runs.iter().fold(0_usize, |n, run| n.saturating_add(run.part(p).len()));
        let built = || runs.iter().flat_map(|run| run.part(p).iter().map(|&(r, w)| (index(r), w)));
        match self {
            Parting::Hashed { shift, within } => {
                let mask = low_bits(*within);
                let slot = |h: u64| usize::try_from(h.checked_shr(*shift).unwrap_or(0)).unwrap_or(0) & mask;
                BuiltPart::Hashed(Part::of(built, (mask.saturating_add(1), count), slot))
            }
            Parting::Dense { width, within, .. } => {
                let mask = low_bits(*within);
                let size = mask.saturating_add(1);
                let places = width.saturating_sub(p.saturating_mul(size)).min(size);
                let at = |place: u64| usize::try_from(place).unwrap_or(usize::MAX) & mask;
                BuiltPart::Dense(DensePart::of(built, (places, count), at))
            }
        }
    }
}

impl Parted {
    /// The run of the rows from `base` on whose hashes, or keys' numbers
    /// of a direct table, are `words`, but those `misses` marks, sorted by
    /// part as `parting` parts them, each part's in order.
    pub(in crate::exec) fn of(parting: &Parting, (words, misses): (&[u64], &[bool]), base: usize) -> Parted {
        let mut nulls = Vec::new();
        let mut keyed: Vec<(usize, (u32, u64))> = Vec::with_capacity(words.len());
        let mut ends = vec![0_usize; BUILD_PARTS.saturating_add(1)];
        for (r, (&word, _)) in words.iter().zip(misses).enumerate().filter(|(_, (_, m))| !**m) {
            let row = u32::try_from(base.saturating_add(r)).unwrap_or(u32::MAX);
            let Some((part, placed)) = parting.placed(word) else {
                nulls.push(row);
                continue;
            };
            if let Some(end) = ends.get_mut(part.saturating_add(1)) {
                *end = end.saturating_add(1);
                keyed.push((part, (row, placed)));
            }
        }
        placed::summed(&mut ends);
        let mut next = ends.clone();
        let mut pairs = vec![(0_u32, 0_u64); keyed.len()];
        for (part, pair) in keyed {
            let Some(at) = next.get_mut(part) else { continue };
            if let Some(slot) = pairs.get_mut(*at) {
                *slot = pair;
            }
            *at = at.saturating_add(1);
        }
        Parted { pairs, ends, nulls }
    }

    /// Its rows of part `p`, in order.
    fn part(&self, p: usize) -> &[(u32, u64)] {
        let from = self.ends.get(p).copied().unwrap_or(0);
        let to = self.ends.get(p.saturating_add(1)).copied().unwrap_or(from);
        self.pairs.get(from..to).unwrap_or_default()
    }
}

// Unit tests: a table made in parts against the table one thread makes
// (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::super::direct::NULL_NUMBER;
    use super::super::{JoinTable, numbers_hashed};
    use super::*;
    use crate::exec::table::Seed;

    /// The table of `words` but those `misses` marks, made in parts of
    /// runs of `each` rows, direct if `span` says so.
    fn in_parts(words: &[u64], misses: &[bool], each: usize, direct: bool) -> JoinTable {
        let spans = words.chunks(each).zip(misses.chunks(each)).map(|(w, m)| Span::of(w, m));
        let span = if direct { spans.reduce(Span::with) } else { None };
        let parting = Parting::of(span, misses.iter().filter(|&&m| !m).count());
        assert_eq!(parting.direct(), direct);
        let runs: Vec<Parted> = words
            .chunks(each)
            .zip(misses.chunks(each))
            .enumerate()
            .map(|(n, run)| Parted::of(&parting, run, n.saturating_mul(each)))
            .collect();
        let parts = (0..BUILD_PARTS).map(|p| parting.part(&runs, p)).collect();
        JoinTable::of_parts(&parting, parts, &runs)
    }

    /// Verifies: [DESIGN] RULES.md D1080: a hashed table made in parts
    /// finds of every hash the rows the table one thread makes finds, in
    /// the order built, whatever the runs its rows were sorted in: a hash
    /// of many rows spread over runs, a slot sorted, a miss never found, a
    /// hash none holds.
    #[test]
    fn a_hashed_table_in_parts_finds_what_one_thread_s_finds() {
        let hash = crate::hash::murmur::fmix64;
        let mut hashes: Vec<u64> = (0..5_000_u64).map(|k| hash(k % 1_700)).collect();
        hashes.extend(std::iter::repeat_n(hash(9_999), 40));
        let misses: Vec<bool> = (0..hashes.len()).map(|r| r % 11 == 3).collect();
        let alone = JoinTable::of(&hashes, &misses);
        for each in [1, 7, 512, hashes.len()] {
            let table = in_parts(&hashes, &misses, each, false);
            assert_eq!(table.parts(), BUILD_PARTS);
            for k in (0..1_800_u64).chain([9_999]) {
                let (want, got): (Vec<usize>, Vec<usize>) = (alone.candidates(hash(k)).collect(), table.candidates(hash(k)).collect());
                assert_eq!(got, want, "key {k}, runs of {each}");
            }
            assert_eq!(table.candidates(hash(9_999)).count(), 37, "the rows of one hash, less its misses");
        }
    }

    /// Verifies: [DESIGN] RULES.md D1080, D601: a direct table made in
    /// parts finds of every key the rows the one a thread makes finds, in
    /// the order built: keys negative, a key of many rows, a null's rows
    /// kept apart, a key out of the span and a fraction none.
    #[test]
    fn a_direct_table_in_parts_finds_what_one_thread_s_finds() {
        let bits = |k: i64| f64::from(i32::try_from(k).unwrap()).to_bits();
        let mut numbers: Vec<u64> = (0..3_000_i64).map(|r| bits((r * 7) % 1_000 - 300)).collect();
        for r in [5, 77, 2_999] {
            numbers[r] = NULL_NUMBER;
        }
        let misses: Vec<bool> = (0..numbers.len()).map(|r| r % 13 == 1).collect();
        let alone = JoinTable::direct(&numbers, &misses).unwrap();
        for each in [1, 100, numbers.len()] {
            let table = in_parts(&numbers, &misses, each, true);
            assert!(table.is_direct() && table.parts() == BUILD_PARTS);
            for probe in (-310..710).map(bits).chain([NULL_NUMBER, 0.5_f64.to_bits(), f64::NAN.to_bits()]) {
                let (want, got): (Vec<usize>, Vec<usize>) = (alone.candidates(probe).collect(), table.candidates(probe).collect());
                assert_eq!(got, want, "number {probe:#x}, runs of {each}");
            }
        }
        assert_eq!(alone.candidates(NULL_NUMBER).count(), 3);
    }

    /// Verifies: [DESIGN] RULES.md D1080 (MC/DC): runs' keys make a direct
    /// table as one thread's would. Decision: every number is whole, there
    /// is a key, and the span is at most `DENSE_SPAN` times the keys. Each
    /// alone makes the table hashed: a fraction in one run, runs of nulls
    /// and misses alone, and a span one place too wide.
    #[test]
    fn runs_say_together_whether_a_table_is_direct() {
        /// What runs of numbers, none a miss, say together.
        fn of(runs: &[&[f64]]) -> Option<Span> {
            let numbers = runs.iter().map(|r| r.iter().map(|f| f.to_bits()).collect::<Vec<u64>>());
            numbers.map(|n| Span::of(&n, &vec![false; n.len()])).reduce(Span::with)
        }
        assert!(Parting::of(of(&[&[0.0_f64], &[7.0_f64]]), 2).direct(), "span 8 of 2 rows");
        assert!(!Parting::of(of(&[&[0.0_f64, 3.0_f64], &[2.5_f64]]), 3).direct(), "a fraction");
        assert!(!Parting::of(of(&[&[0.0_f64], &[8.0_f64]]), 2).direct(), "span 9 of 2 rows");
        let nulls = Span::of(&[NULL_NUMBER, 5.0_f64.to_bits()], &[false, true]);
        assert!(!Parting::of(Some(nulls.with(nulls)), 2).direct(), "no key");
        assert!(!Parting::of(None, 2).direct(), "no numbers asked");
    }

    /// Verifies: [DESIGN] RULES.md D1080: a hashed table has `MIN_SLOTS`
    /// slots a part at least, and two a row; a direct one parts of a
    /// power of two places that cover its span.
    #[test]
    fn a_parting_sizes_its_parts() {
        let sized = |keyed: usize| match Parting::of(None, keyed) {
            Parting::Hashed { shift, within } => (shift, within),
            Parting::Dense { .. } => panic!("not hashed"),
        };
        assert_eq!(sized(0), (54, 4));
        assert_eq!(sized(3_000), (51, 7), "8,192 slots of 6,000 asked");
        let numbers: Vec<u64> = (0_i32..1_000_i32).map(|k| f64::from(k).to_bits()).collect();
        let Parting::Dense { least, width, within } = Parting::of(Some(Span::of(&numbers, &[false; 1_000])), 1_000) else {
            panic!("not direct")
        };
        assert_eq!((least, width, within), (0, 1_000, 4), "64 parts of 16 places");
    }

    /// Verifies: [DESIGN] RULES.md D1080: keys given as numbers are hashed
    /// as a hashed table hashes them of their vector, a null's as a null.
    #[test]
    fn numbers_are_hashed_as_their_vectors_keys() {
        use super::super::{Hashing, KeyHashes};
        use crate::storage::{Batch, Column, Ints, Mask};
        let seed = Seed::fixed(0x5EED);
        let ints = Ints::new(vec![4, -9, 0, 250], 1, Some(Mask::from_bools(&[false, false, true, false])));
        let batch = Batch { columns: vec![Column::Ints(ints)], rows: 4 };
        let hashes = |direct: bool| {
            let mut out = KeyHashes::default();
            out.of_batch((&batch, None), (&[0], &[true]), Hashing { seed: &seed, keyed: false, direct }).unwrap();
            out.hashes
        };
        assert_eq!(numbers_hashed(&hashes(true), &seed), hashes(false));
    }
}

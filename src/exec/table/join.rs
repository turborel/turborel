//! \[DESIGN\] (RULES.md D368) The executor's table of a hash join's built
//! rows, which a join of streams builds of its right operand held as one
//! batch (`batch/joined/held.rs`) and each of its left operand's batches
//! probes a batch at a time. Its rows' keys are hashed a vector at a time
//! (`join/keys.rs`), as a value's bucket (`value::key_bucket`) says it
//! matches, so a key hashes alike in any vector that holds it.
//!
//! Its directory has a word for each of a power of two slots, at least
//! twice its rows, a slot the high bits of a hash. A word holds where its
//! slot's rows end among the table's, and above that 16 bits, one set for
//! each row's hash by 4 of its low bits, so a probe of a hash none of the
//! slot's rows shares a bit with is turned away by that word alone, as
//! Leis et al. (SIGMOD 2014) §4.2 put a filter of 16 tag bits in each
//! pointer of a chained table. The rows of a slot are held next to each
//! other in a vector of rows and one of their hashes, as Birler, Schmidt,
//! Fent and Neumann (2024) hold an "unchained" table's, not chained one
//! to the next: a probe reads them in order, not at random. A slot's rows
//! are in the order they were built; a slot of more than `LINEAR` rows,
//! of a key many rows share, is sorted by hash, stably, so a probe finds
//! the rows of its hash by a binary search, those of one key in the order
//! they were built either way, and a probe of a key no row has costs no
//! walk of another key's many rows.
//!
//! \[DESIGN\] (RULES.md D601) A table of one key column of whole numbers
//! dense in their span is direct instead (`direct.rs`): a probe gives it
//! its key's number, not a hash (`Hashing::direct`), and finds the rows of
//! that number at its place in the span.
//!
//! \[DESIGN\] (RULES.md D1080) A table is held in parts, each the rows
//! whose slots share their high bits, with a directory of its own: one
//! part, if one thread built it, or `BUILD_PARTS`, each made by the
//! thread that took it (`parted.rs`). A probe finds its part by the high
//! bits of its slot, then its slot within the part.

mod direct;
mod equal;
mod keys;
mod parted;
mod placed;

#[cfg(test)]
mod tests;

pub(in crate::exec) use direct::{DENSE_SPAN, Span};
pub(in crate::exec) use equal::keys_equal;
pub(in crate::exec) use keys::{Hashing, KeyHashes, numbers_hashed, values_hash};
pub(in crate::exec) use parted::{BUILD_PARTS, BuiltPart, Parted, Parting};

use crate::kernels::Kernels;

/// \[DESIGN\] How far a directory word's 16 tag bits are turned.
const TAG_SHIFT: u32 = 48;

/// \[DESIGN\] The bits of a directory word that hold where its slot's rows
/// end: those below its tags.
const END_MASK: u64 = (1_u64 << TAG_SHIFT) - 1;

/// \[DESIGN\] The low bits of a hash that pick its tag among 16.
const TAG_BITS: u64 = 15;

/// \[DESIGN\] The fewest slots of a directory, so its index takes at least
/// 4 bits of a hash, apart from the 4 low bits of its tag.
const MIN_SLOTS: usize = 16;

/// \[DESIGN\] (RULES.md D368) The most rows of a slot read one after
/// another; a slot of more is sorted by hash and searched.
pub(super) const LINEAR: usize = 8;

/// The rows of a table whose slots share their high bits (RULES.md
/// D1080): every row of a table one thread built.
#[derive(Debug, Default)]
pub(in crate::exec) struct Part {
    /// Of each slot, after a word of 0: its tags above where its rows end.
    directory: Vec<u64>,
    /// The built rows, those of each slot after the slot before's.
    rows: Vec<u32>,
    /// Each of `rows`' hash.
    hashes: Vec<u64>,
}

impl Part {
    /// The part of `slots` slots of the rows `built` gives each time it
    /// is asked, each with its hash, `count` of them, `slot` of a hash
    /// its slot among the part's.
    fn of<I: Iterator<Item = (usize, u64)>>(built: impl Fn() -> I, (slots, count): (usize, usize), slot: impl Fn(u64) -> usize) -> Part {
        let mut ends = vec![0_usize; slots.saturating_add(1)];
        for (_, h) in built() {
            if let Some(end) = ends.get_mut(slot(h).saturating_add(1)) {
                *end = end.saturating_add(1);
            }
        }
        placed::summed(&mut ends);
        let (mut rows, mut hashes) = placed::scattered(built(), &ends, &slot, count);
        placed::sorted(&ends, &mut rows, &mut hashes);
        let mut directory = vec![0_u64; slots.saturating_add(1)];
        for (word, (from, to)) in directory.iter_mut().skip(1).zip(ends.iter().zip(ends.iter().skip(1))) {
            let tags = hashes.get(*from..*to).unwrap_or_default().iter().fold(0_u64, |t, &h| t | tag(h));
            *word = tags | u64::try_from(*to).unwrap_or(END_MASK);
        }
        Part { directory, rows, hashes }
    }
}

/// A hash join's built rows, found by their keys' hashes.
#[derive(Debug, Default)]
pub(in crate::exec) struct JoinTable {
    /// Its parts, each the rows whose slots share their high bits: one,
    /// or `BUILD_PARTS` (RULES.md D1080).
    parts: Vec<Part>,
    /// How far a hash is turned to give its slot.
    shift: u32,
    /// How far a slot is turned to give its part.
    within: u32,
    /// The bits of a slot that place it within its part.
    mask: usize,
    /// Of a direct table, its rows by their keys' places (RULES.md D601),
    /// boxed, as an operator holds its table beside its state.
    dense: Option<Box<direct::Dense>>,
}

impl JoinTable {
    /// The table of the rows whose `hashes` are given, each one's number
    /// its place among them, but those `misses` marks, whose keys match
    /// nothing.
    pub(in crate::exec) fn of(hashes: &[u64], misses: &[bool]) -> JoinTable {
        let keyed = misses.iter().filter(|&&m| !m).count();
        let slots = keyed.saturating_mul(2).max(MIN_SLOTS).checked_next_power_of_two().unwrap_or(MIN_SLOTS);
        let (bits, shift) = (slots.trailing_zeros(), u64::BITS.saturating_sub(slots.trailing_zeros()));
        let built = || hashes.iter().zip(misses).enumerate().filter(|(_, (_, m))| !**m).map(|(r, (&h, _))| (r, h));
        let part = Part::of(built, (slots, keyed), |h| usize::try_from(h.checked_shr(shift).unwrap_or(0)).unwrap_or(0));
        JoinTable { parts: vec![part], shift, within: bits, mask: parted::low_bits(bits), dense: None }
    }

    /// \[DESIGN\] (RULES.md D1080) The table of `parts`, each made as
    /// `parting` parts its rows (`Parting::part`), in order; of a direct
    /// table, the rows of a null key those of `runs`, in order.
    pub(in crate::exec) fn of_parts(parting: &Parting, parts: Vec<BuiltPart>, runs: &[Parted]) -> JoinTable {
        let (mut hashed, mut placed) = (Vec::new(), Vec::new());
        for part in parts {
            match part {
                BuiltPart::Hashed(h) => hashed.push(h),
                BuiltPart::Dense(d) => placed.push(d),
            }
        }
        match parting {
            Parting::Hashed { shift, within } => {
                JoinTable { parts: hashed, shift: *shift, within: *within, mask: parted::low_bits(*within), dense: None }
            }
            Parting::Dense { least, within, .. } => {
                let nulls = runs.iter().flat_map(|run| run.nulls.iter().copied()).collect();
                let dense = direct::Dense::of_parts(*least, placed, *within, nulls);
                JoinTable { dense: Some(Box::new(dense)), ..JoinTable::default() }
            }
        }
    }

    /// \[DESIGN\] (RULES.md D601) The direct table of the rows whose keys'
    /// `numbers` are given, each the bits of its key's nearest `f64`
    /// (`Hashing::direct`), but those `misses` marks; none unless they are
    /// whole numbers dense in their span (`direct.rs`).
    pub(in crate::exec) fn direct(numbers: &[u64], misses: &[bool]) -> Option<JoinTable> {
        direct::Dense::of(numbers, misses).map(|dense| JoinTable { dense: Some(Box::new(dense)), ..JoinTable::default() })
    }

    /// The table of the rows of one key column, `place` of `batch`, of
    /// whole numbers dense in their span, direct; none of another, so it
    /// is hashed (RULES.md D601).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a value nests deeper than a type may.
    pub(in crate::exec) fn dense_of(
        batch: &crate::storage::Batch,
        (places, nulls): (&[usize], &[bool]),
        seed: &crate::exec::Seed,
    ) -> crate::error::Result<Option<JoinTable>> {
        let [place] = places else { return Ok(None) };
        if !matches!(batch.columns.get(*place), Some(crate::storage::Column::Ints(_))) {
            return Ok(None);
        }
        let mut numbers = KeyHashes::default();
        numbers.of_batch((batch, None), (places, nulls), Hashing { seed, keyed: false, direct: true })?;
        Ok(JoinTable::direct(&numbers.hashes, &numbers.misses))
    }

    /// How many parts it was made in (RULES.md D1080).
    #[cfg(test)]
    pub(in crate::exec) fn parts(&self) -> usize {
        self.dense.as_ref().map_or(self.parts.len(), |dense| dense.parts())
    }

    /// \[DESIGN\] (RULES.md D1083) Of a direct table, the rows of the key
    /// that is the whole number `key`, in the order built, as
    /// `candidates` gives them of that number's bits, no number made of
    /// it; of a hashed table, none.
    pub(in crate::exec) fn rows_of(&self, key: i64) -> &[u32] {
        let Some(dense) = &self.dense else { return &[] };
        dense.rows_of(key)
    }

    /// The bytes its hashed parts hold: their directories, rows and hashes.
    pub(in crate::exec) fn bytes(&self) -> usize {
        let of =
            |p: &Part| p.directory.len().saturating_add(p.hashes.len()).saturating_mul(8).saturating_add(p.rows.len().saturating_mul(4));
        self.parts.iter().map(of).fold(0, usize::saturating_add)
    }

    /// Whether it is direct, its probes giving their keys' numbers, not
    /// hashes (RULES.md D601).
    pub(in crate::exec) fn is_direct(&self) -> bool {
        self.dense.is_some()
    }

    /// The slot of hash `h`: its high bits.
    fn slot(&self, h: u64) -> usize {
        usize::try_from(h.checked_shr(self.shift).unwrap_or(0)).unwrap_or(0)
    }

    /// The numbers of the rows whose hash is `h`, in the order they were
    /// built: those its key may match, compared then (`keys_equal`); of a
    /// direct table, `h` the number of the key (RULES.md D601).
    pub(in crate::exec) fn candidates(&self, h: u64) -> Candidates<'_> {
        if self.dense.is_some() {
            return self.candidates_at(h, Located::default());
        }
        self.candidates_at(h, self.locate(h))
    }

    /// \[DESIGN\] (RULES.md D1095) Where the rows of hash `h` lie: its
    /// slot's directory words read, so a probe locating a batch's rows
    /// at once has their reads from memory under way together. None of a
    /// slot whose tags do not hold `h`'s.
    pub(in crate::exec) fn locate(&self, h: u64) -> Located {
        let slot = self.slot(h);
        let part = slot.checked_shr(self.within).unwrap_or(0);
        let Some(of) = self.parts.get(part) else { return Located::default() };
        let at = slot & self.mask;
        let Some(&word) = of.directory.get(at.saturating_add(1)) else { return Located::default() };
        if word & tag(h) == 0 {
            return Located::default();
        }
        Located { part, from: of.directory.get(at).map_or(0, |&w| end_of(w)), to: end_of(word) }
    }

    /// The candidates of hash `h`, as `candidates` gives them, its rows
    /// found where `located`, that `locate` gave of it, says.
    pub(in crate::exec) fn candidates_at(&self, h: u64, located: Located) -> Candidates<'_> {
        if let Some(dense) = &self.dense {
            return Candidates::Run(dense.candidates(h).iter());
        }
        let Some(part) = self.parts.get(located.part) else { return Candidates::Run([].iter()) };
        let (from, to) = (located.from, located.to);
        let (Some(hashes), Some(rows)) = (part.hashes.get(from..to), part.rows.get(from..to)) else {
            return Candidates::Run([].iter());
        };
        if hashes.len() <= LINEAR {
            return Candidates::Scan(hashes.iter().zip(rows.iter()), h);
        }
        let first = hashes.partition_point(|&x| x < h);
        let count = hashes.get(first..).map_or(0, |rest| rest.partition_point(|&x| x == h));
        Candidates::Run(rows.get(first..first.saturating_add(count)).unwrap_or_default().iter())
    }

    /// \[DESIGN\] (RULES.md D1095) Asks `kernels` to bring the first hash
    /// and row number of each of `located` into the caches, a part's at
    /// once, so a probe reading them after finds them there.
    pub(in crate::exec) fn prefetch(&self, located: &[Located], kernels: &dyn Kernels) {
        let mut at: Vec<(usize, u32)> =
            located.iter().filter(|l| l.from < l.to).filter_map(|l| Some((l.part, u32::try_from(l.from).ok()?))).collect();
        at.sort_unstable();
        let mut firsts = Vec::with_capacity(at.len());
        for run in at.chunk_by(|a, b| a.0 == b.0) {
            let Some(part) = run.first().and_then(|&(p, _)| self.parts.get(p)) else { continue };
            firsts.clear();
            firsts.extend(run.iter().map(|&(_, from)| from));
            kernels.prefetch_u64(&part.hashes, &firsts);
            kernels.prefetch_u32(&part.rows, &firsts);
        }
    }
}

impl JoinTable {
    /// \[DESIGN\] (RULES.md D1095) The number of the first row of each of
    /// `located`'s slots, into `out`, `u32::MAX` of one of no row: the
    /// built row whose key a probe compares first.
    pub(in crate::exec) fn firsts(&self, located: &[Located], out: &mut Vec<u32>) {
        out.clear();
        out.extend(located.iter().map(|l| {
            let first = (l.from < l.to).then(|| self.parts.get(l.part)?.rows.get(l.from).copied()).flatten();
            first.unwrap_or(u32::MAX)
        }));
    }
}

/// \[DESIGN\] (RULES.md D1095) Where a hash's rows lie in a table: its
/// part, and the places in it from which to which its slot's rows are;
/// none if they are no places.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::exec) struct Located {
    part: usize,
    from: usize,
    to: usize,
}

/// The tag bit of hash `h` in a directory word.
fn tag(h: u64) -> u64 {
    1_u64 << (TAG_SHIFT.saturating_add(u32::try_from(h & TAG_BITS).unwrap_or(0)))
}

/// Where the rows of the slot of directory word `word` end.
fn end_of(word: u64) -> usize {
    usize::try_from(word & END_MASK).unwrap_or(0)
}

/// The numbers of the built rows a probe's hash may match, in the order
/// they were built: a slot's rows of that hash, or a run of a sorted
/// slot's.
pub(in crate::exec) enum Candidates<'t> {
    /// Rows all of the hash.
    Run(std::slice::Iter<'t, u32>),
    /// A slot's rows and their hashes, those of this hash taken.
    Scan(std::iter::Zip<std::slice::Iter<'t, u64>, std::slice::Iter<'t, u32>>, u64),
}

impl Iterator for Candidates<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        let row = match self {
            Candidates::Run(rows) => rows.next().copied(),
            Candidates::Scan(pairs, h) => pairs.find(|(x, _)| **x == *h).map(|(_, &r)| r),
        };
        row.map(crate::storage::mask::index)
    }

    /// The `n`th row on, of a run at once, so a probe that gives its pairs
    /// a batch at a time takes up a key's rows where it left them for what
    /// one step costs (`batch/joined/shared/probing.rs`).
    fn nth(&mut self, n: usize) -> Option<usize> {
        match self {
            Candidates::Run(rows) => rows.nth(n).copied().map(crate::storage::mask::index),
            Candidates::Scan(..) => {
                for _ in 0..n {
                    self.next()?;
                }
                self.next()
            }
        }
    }
}

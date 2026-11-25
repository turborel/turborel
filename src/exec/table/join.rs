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

mod direct;
mod equal;
mod keys;
mod placed;

#[cfg(test)]
mod tests;

pub(in crate::exec) use direct::DENSE_SPAN;
pub(in crate::exec) use equal::keys_equal;
pub(in crate::exec) use keys::{Hashing, KeyHashes, values_hash};

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

/// A hash join's built rows, found by their keys' hashes.
#[derive(Debug, Default)]
pub(in crate::exec) struct JoinTable {
    /// Of each slot, after a word of 0: its tags above where its rows end.
    directory: Vec<u64>,
    /// The built rows, those of each slot after the slot before's.
    rows: Vec<u32>,
    /// Each of `rows`' hash.
    hashes: Vec<u64>,
    /// How far a hash is turned to give its slot.
    shift: u32,
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
        let shift = u64::BITS.saturating_sub(slots.trailing_zeros());
        let mut table = JoinTable { directory: vec![0; slots.saturating_add(1)], rows: Vec::new(), hashes: Vec::new(), shift, dense: None };
        let mut ends = vec![0_usize; slots.saturating_add(1)];
        let built = || hashes.iter().zip(misses).enumerate().filter(|(_, (_, m))| !**m).map(|(r, (&h, _))| (r, h));
        for (_, h) in built() {
            if let Some(end) = ends.get_mut(table.slot(h).saturating_add(1)) {
                *end = end.saturating_add(1);
            }
        }
        placed::summed(&mut ends);
        (table.rows, table.hashes) = placed::scattered(built(), &ends, |h| table.slot(h), keyed);
        placed::sorted(&ends, &mut table.rows, &mut table.hashes);
        for (word, (from, to)) in table.directory.iter_mut().skip(1).zip(ends.iter().zip(ends.iter().skip(1))) {
            let tags = table.hashes.get(*from..*to).unwrap_or_default().iter().fold(0_u64, |t, &h| t | tag(h));
            *word = tags | u64::try_from(*to).unwrap_or(END_MASK);
        }
        table
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
        if let Some(dense) = &self.dense {
            return Candidates::Run(dense.candidates(h).iter());
        }
        let at = self.slot(h);
        let Some(&word) = self.directory.get(at.saturating_add(1)) else { return Candidates::Run([].iter()) };
        if word & tag(h) == 0 {
            return Candidates::Run([].iter());
        }
        let from = self.directory.get(at).map_or(0, |&w| end_of(w));
        let to = end_of(word);
        let (Some(hashes), Some(rows)) = (self.hashes.get(from..to), self.rows.get(from..to)) else {
            return Candidates::Run([].iter());
        };
        if hashes.len() <= LINEAR {
            return Candidates::Scan(hashes.iter().zip(rows.iter()), h);
        }
        let first = hashes.partition_point(|&x| x < h);
        let count = hashes.get(first..).map_or(0, |rest| rest.partition_point(|&x| x == h));
        Candidates::Run(rows.get(first..first.saturating_add(count)).unwrap_or_default().iter())
    }
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

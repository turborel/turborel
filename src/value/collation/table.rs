//! \[EXTENSION\] The Unicode data the collations read (RULES.md D60):
//! `unicode13.bin`, which `cargo xtask unicode` makes from Unicode 13.0's
//! `allkeys.txt`, `UnicodeData.txt` and `CaseFolding.txt`. UTS #10 maps UCA
//! 13.0.0 to ISO/IEC 14651:2020.
//!
//! The file is a magic, the count of each section's records, then the
//! sections, each of fixed-size little-endian records sorted by code
//! point, read in place by binary search.

use crate::error::{Code, Result, SqlError};

/// The data's version, stored with a database that uses it.
pub(super) const VERSION: &str = "UCA 13.0.0";
/// The table, generated, and no source to review (RULES.md D3).
const BYTES: &[u8] = include_bytes!("unicode13.bin");
/// What the table begins with.
const MAGIC: &[u8; 8] = b"TBUCA130";
/// The bytes before the first section: the magic and eight counts.
const HEADER: usize = 40;
/// The digest of the table as generated (`cargo xtask unicode`).
#[cfg(test)]
pub(super) const DIGEST: u64 = 0x32fd_464e_0802_3215;
#[cfg(test)]
const FNV_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
#[cfg(test)]
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
/// The flag of a character that begins a contraction.
const BEGINS_CONTRACTION: u32 = 1 << 31;
/// Where a record's count sits above its offset.
const COUNT_SHIFT: u32 = 24;
/// The offset's bits, below its count.
const OFFSET: u32 = (1 << COUNT_SHIFT) - 1;
/// The most characters of a contraction.
pub(super) const MOST_CONTRACTED: usize = 3;
/// A section's record sizes.
const CHAR_BYTES: usize = 8;
const ELEMENT_BYTES: usize = 8;
const CONTRACTION_BYTES: usize = 16;
const DECOMPOSITION_BYTES: usize = 8;
const PART_BYTES: usize = 4;
const PAIR_BYTES: usize = 8;
const RANGE_BYTES: usize = 16;

/// One section: where it starts, and its records.
#[derive(Clone, Copy, Default)]
struct Section {
    start: usize,
    records: usize,
    size: usize,
}

impl Section {
    /// The `n`-th number of record `i`.
    fn u32_at(self, i: usize, n: usize) -> Option<u32> {
        let at = i.checked_mul(self.size)?.checked_add(self.start)?.checked_add(n.checked_mul(PART_BYTES)?)?;
        read_u32(at)
    }

    /// The record whose first number, under `key`, is `code`.
    fn find(self, code: u32, key: u32) -> Option<usize> {
        let (mut low, mut high) = (0, self.records);
        while low < high {
            let middle = low.midpoint(high);
            let found = self.u32_at(middle, 0)? & key;
            match found.cmp(&code) {
                std::cmp::Ordering::Equal => return Some(middle),
                std::cmp::Ordering::Less => low = middle.checked_add(1)?,
                std::cmp::Ordering::Greater => high = middle,
            }
        }
        None
    }
}

fn read_u32(at: usize) -> Option<u32> {
    let bytes = BYTES.get(at..at.checked_add(PART_BYTES)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn read_u16(at: usize) -> Option<u16> {
    let bytes = BYTES.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

/// A collation element \[UTS #10 §3\].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Element {
    /// Its primary, secondary and tertiary weights.
    pub(super) weights: [u16; 3],
    /// Whether it is variable \[UTS #10 §4\].
    pub(super) variable: bool,
}

/// A run of elements: its count and where it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Run {
    count: usize,
    at: usize,
}

/// What the table says of a character.
pub(super) struct Entry {
    /// Its elements.
    pub(super) run: Run,
    /// Whether a contraction begins with it.
    pub(super) contracts: bool,
}

/// A range of implicit weights \[UTS #10 §10.1.3\].
#[derive(Debug, Clone, Copy)]
pub(super) struct Implicit {
    /// Its primary.
    pub(super) primary: u32,
    /// The code point it counts from.
    pub(super) origin: u32,
}

/// The table, its sections found.
pub(super) struct Table {
    chars: Section,
    elements: Section,
    contractions: Section,
    decompositions: Section,
    parts: Section,
    classes: Section,
    folds: Section,
    ranges: Section,
}

fn damaged() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] the collation table is damaged")
}

impl Table {
    /// The table, its sections found.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it does not hold what its header says.
    pub(super) fn read() -> Result<Table> {
        if BYTES.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
            return Err(damaged());
        }
        let sizes = [CHAR_BYTES, ELEMENT_BYTES, CONTRACTION_BYTES, DECOMPOSITION_BYTES, PART_BYTES, PAIR_BYTES, PAIR_BYTES, RANGE_BYTES];
        let mut sections = [Section::default(); 8];
        let mut start = HEADER;
        for (n, (section, size)) in sections.iter_mut().zip(sizes).enumerate() {
            let at = MAGIC.len().saturating_add(n.saturating_mul(PART_BYTES));
            let records = read_u32(at).and_then(|r| usize::try_from(r).ok()).ok_or_else(damaged)?;
            *section = Section { start, records, size };
            start = records.checked_mul(size).and_then(|b| b.checked_add(start)).ok_or_else(damaged)?;
        }
        if start != BYTES.len() {
            return Err(damaged());
        }
        let [chars, elements, contractions, decompositions, parts, classes, folds, ranges] = sections;
        Ok(Table { chars, elements, contractions, decompositions, parts, classes, folds, ranges })
    }

    /// What the table says of `c`, if it has weights of its own.
    pub(super) fn entry(&self, c: char) -> Option<Entry> {
        let i = self.chars.find(u32::from(c), !BEGINS_CONTRACTION)?;
        let code = self.chars.u32_at(i, 0)?;
        Some(Entry { run: run_of(self.chars.u32_at(i, 1)?)?, contracts: code & BEGINS_CONTRACTION != 0 })
    }

    /// The elements of the contraction `chars`, if it is one.
    pub(super) fn contraction(&self, chars: &[char]) -> Option<Run> {
        let mut key = [0_u32; MOST_CONTRACTED];
        for (k, c) in key.iter_mut().zip(chars) {
            *k = u32::from(*c);
        }
        let (mut low, mut high) = (0, self.contractions.records);
        while low < high {
            let middle = low.midpoint(high);
            let mut found = [0_u32; MOST_CONTRACTED];
            for (n, f) in found.iter_mut().enumerate() {
                *f = self.contractions.u32_at(middle, n)?;
            }
            match found.cmp(&key) {
                std::cmp::Ordering::Equal => return run_of(self.contractions.u32_at(middle, MOST_CONTRACTED)?),
                std::cmp::Ordering::Less => low = middle.checked_add(1)?,
                std::cmp::Ordering::Greater => high = middle,
            }
        }
        None
    }

    /// The elements of `run`, added to `out`.
    pub(super) fn elements(&self, run: Run, out: &mut Vec<Element>) {
        for i in 0..run.count {
            if let Some(element) = run.at.checked_add(i).and_then(|at| self.element(at)) {
                out.push(element);
            }
        }
    }

    /// Every element the table holds.
    pub(super) fn all_elements(&self) -> impl Iterator<Item = Element> + '_ {
        (0..self.elements.records).filter_map(|i| self.element(i))
    }

    fn element(&self, i: usize) -> Option<Element> {
        let at = i.checked_mul(ELEMENT_BYTES)?.checked_add(self.elements.start)?;
        let weight = |n: usize| read_u16(at.checked_add(n.checked_mul(2)?)?);
        let variable = *BYTES.get(at.checked_add(6)?)? != 0;
        Some(Element { weights: [weight(0)?, weight(1)?, weight(2)?], variable })
    }

    /// `c`'s full canonical decomposition, added to `out`; `c` if it has
    /// none.
    pub(super) fn decompose(&self, c: char, out: &mut Vec<char>) {
        let parts = self.decompositions.find(u32::from(c), u32::MAX).and_then(|i| {
            let record = self.decompositions.u32_at(i, 1)?;
            let run = run_of(record)?;
            (0..run.count).map(|n| self.parts.u32_at(run.at.checked_add(n)?, 0).and_then(char::from_u32)).collect::<Option<Vec<char>>>()
        });
        match parts {
            Some(parts) => out.extend(parts),
            None => out.push(c),
        }
    }

    /// `c`'s canonical combining class.
    pub(super) fn class(&self, c: char) -> u32 {
        self.classes.find(u32::from(c), u32::MAX).and_then(|i| self.classes.u32_at(i, 1)).unwrap_or(0)
    }

    /// `c`'s simple case folding.
    pub(super) fn fold(&self, c: char) -> char {
        let folded = self.folds.find(u32::from(c), u32::MAX).and_then(|i| self.folds.u32_at(i, 1));
        folded.and_then(char::from_u32).unwrap_or(c)
    }

    /// The implicit weight range `code` falls in, if any.
    pub(super) fn implicit(&self, code: u32) -> Option<Implicit> {
        (0..self.ranges.records).find_map(|i| {
            let (first, last) = (self.ranges.u32_at(i, 0)?, self.ranges.u32_at(i, 1)?);
            let primary = self.ranges.u32_at(i, 2)?;
            let origin = self.ranges.u32_at(i, 3)?;
            (first <= code && code <= last).then_some(Implicit { primary, origin })
        })
    }
}

/// The table's FNV-1a digest, to hold it to `DIGEST`.
#[cfg(test)]
pub(super) fn digest() -> u64 {
    fnv(BYTES)
}

/// The FNV-1a digest of `bytes`.
#[cfg(test)]
pub(crate) fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_BASIS, |hash, &b| (hash ^ u64::from(b)).wrapping_mul(FNV_PRIME))
}

/// A record's count and offset, in one number.
fn run_of(record: u32) -> Option<Run> {
    let count = usize::try_from(record >> COUNT_SHIFT).ok()?;
    let at = usize::try_from(record & OFFSET).ok()?;
    Some(Run { count, at })
}

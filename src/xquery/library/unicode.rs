//! The Unicode data the functions on strings read (RULES.md D3, D86):
//! `unicode13.bin`, which `cargo xtask unicode` makes from Unicode 13.0's
//! `UnicodeData.txt`, `CompositionExclusions.txt` and `SpecialCasing.txt`,
//! the version the collations and regular expressions read (D60, D61). Its
//! notice is `LICENSE-UNICODE`, beside it.
//!
//! The file is a magic, the count of each section's records, then the
//! sections, each of little-endian numbers sorted by code point: the
//! canonical combining classes not zero, as pairs; four mappings, each
//! records of a code point and its parts' count and place, then the parts:
//! the full canonical decompositions, the full compatibility
//! decompositions \[UAX #15 §1.3\], the full upper case and the full lower
//! case [XQueryFO §7.4.7, §7.4.8]; and the primary composites, each two
//! code points and the one they compose to \[UAX #15 §1.3\].

use crate::error::{Code, Result, SqlError};

/// The table, generated, and no source to review (RULES.md D3).
const BYTES: &[u8] = include_bytes!("unicode13.bin");
/// What the table begins with.
const MAGIC: &[u8; 8] = b"TBXQF130";
/// How many sections the table has.
const SECTIONS: usize = 10;
/// The octets of a number.
const NUMBER_BYTES: usize = 4;
/// The bytes before the first section: the magic and the counts.
const HEADER: usize = 8 + SECTIONS * NUMBER_BYTES;
/// The octets of a record of a pair, of a mapping's record, and of a
/// composite's.
const PAIR_BYTES: usize = 8;
const COMPOSITE_BYTES: usize = 12;
/// Where a mapping record's count sits above its parts' place.
const COUNT_SHIFT: u32 = 24;
/// The bits of a mapping record's parts' place.
const OFFSET_MASK: u32 = (1 << COUNT_SHIFT) - 1;
/// The digest of the table as generated (`cargo xtask unicode`).
#[cfg(test)]
const DIGEST: u64 = 0x764b_7c80_7c0b_5ac9;

/// One section: where it starts, its records and their size.
#[derive(Debug, Clone, Copy, Default)]
struct Section {
    start: usize,
    records: usize,
    size: usize,
}

impl Section {
    /// The `n`-th number of record `i`.
    fn number(self, i: usize, n: usize) -> Option<u32> {
        let at = i.checked_mul(self.size)?.checked_add(self.start)?.checked_add(n.checked_mul(NUMBER_BYTES)?)?;
        let bytes = BYTES.get(at..at.checked_add(NUMBER_BYTES)?)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }

    /// The record whose first numbers are `key`, by a binary search.
    fn find(self, key: &[u32]) -> Option<usize> {
        let (mut low, mut high) = (0, self.records);
        while low < high {
            let middle = low.saturating_add(high.saturating_sub(low).checked_div(2).unwrap_or(0));
            let here: Option<Vec<u32>> = (0..key.len()).map(|n| self.number(middle, n)).collect();
            match here?.as_slice().cmp(key) {
                std::cmp::Ordering::Less => low = middle.saturating_add(1),
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return Some(middle),
            }
        }
        None
    }
}

/// A mapping of code points to sequences of them: its records and parts.
#[derive(Debug, Clone, Copy, Default)]
struct Mapping {
    records: Section,
    parts: Section,
}

impl Mapping {
    /// What `c` maps to, if it maps.
    fn of(self, c: char) -> Option<Vec<char>> {
        let at = self.records.find(&[u32::from(c)])?;
        let counted = self.records.number(at, 1)?;
        let (count, offset) = (counted.checked_shr(COUNT_SHIFT)?, counted & OFFSET_MASK);
        let first = usize::try_from(offset).ok()?;
        (0..usize::try_from(count).ok()?).map(|n| self.parts.number(first.checked_add(n)?, 0).and_then(char::from_u32)).collect()
    }
}

/// Which of the four mappings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Map {
    /// The full canonical decomposition.
    Canonical,
    /// The full compatibility decomposition.
    Compatible,
    /// The full upper case.
    Upper,
    /// The full lower case.
    Lower,
}

/// The table, its sections found.
#[derive(Debug, Clone, Copy)]
pub struct Table {
    classes: Section,
    canonical: Mapping,
    compatible: Mapping,
    upper: Mapping,
    lower: Mapping,
    composites: Section,
}

/// The error of a table that is not what its header says.
fn damaged() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] the Unicode table of the XQuery functions is damaged")
}

impl Table {
    /// The table, its sections found.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it does not hold what its header says.
    pub fn read() -> Result<Table> {
        if BYTES.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
            return Err(damaged());
        }
        let sizes = [PAIR_BYTES, PAIR_BYTES, NUMBER_BYTES, PAIR_BYTES, NUMBER_BYTES, PAIR_BYTES, NUMBER_BYTES, PAIR_BYTES, NUMBER_BYTES];
        let mut sections = [Section::default(); SECTIONS];
        let mut start = HEADER;
        for (n, (section, size)) in sections.iter_mut().zip(sizes.into_iter().chain([COMPOSITE_BYTES])).enumerate() {
            let at = MAGIC.len().saturating_add(n.saturating_mul(NUMBER_BYTES));
            let count = BYTES.get(at..at.saturating_add(NUMBER_BYTES)).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
            let records = count.and_then(|r| usize::try_from(r).ok()).ok_or_else(damaged)?;
            *section = Section { start, records, size };
            start = records.checked_mul(size).and_then(|b| b.checked_add(start)).ok_or_else(damaged)?;
        }
        if start != BYTES.len() {
            return Err(damaged());
        }
        let [classes, cr, cp, kr, kp, ur, up, lr, lp, composites] = sections;
        let mapping = |records, parts| Mapping { records, parts };
        Ok(Table {
            classes,
            canonical: mapping(cr, cp),
            compatible: mapping(kr, kp),
            upper: mapping(ur, up),
            lower: mapping(lr, lp),
            composites,
        })
    }

    /// The canonical combining class of `c` \[UAX #15 §1.3\], 0 of a
    /// starter.
    #[must_use]
    pub fn class(&self, c: char) -> u32 {
        self.classes.find(&[u32::from(c)]).and_then(|at| self.classes.number(at, 1)).unwrap_or(0)
    }

    /// What `c` maps to by `map`, if it maps to anything but itself.
    #[must_use]
    pub fn mapped(&self, c: char, map: Map) -> Option<Vec<char>> {
        match map {
            Map::Canonical => self.canonical,
            Map::Compatible => self.compatible,
            Map::Upper => self.upper,
            Map::Lower => self.lower,
        }
        .of(c)
    }

    /// The primary composite of `first` and `second`, if they have one
    /// \[UAX #15 §1.3\].
    #[must_use]
    pub fn composite(&self, first: char, second: char) -> Option<char> {
        let at = self.composites.find(&[u32::from(first), u32::from(second)])?;
        self.composites.number(at, 2).and_then(char::from_u32)
    }
}

/// The digest of the table, by FNV-1a.
#[cfg(test)]
fn digest() -> u64 {
    crate::value::collation::fnv(BYTES)
}

#[cfg(test)]
mod tests;

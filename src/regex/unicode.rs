//! \[EXTENSION\] The Unicode data regular expressions read (RULES.md D61):
//! `unicode13.bin`, which `cargo xtask unicode` makes from Unicode 13.0's
//! `UnicodeData.txt`, `Blocks.txt` and `SpecialCasing.txt`. Its notice is
//! `LICENSE-UNICODE`, beside it.
//!
//! The file is a magic, the count of each section's records, then the
//! sections: runs of one General Category, each its first code point and
//! category; blocks, each its first and last code points and where its
//! normalized name lies in the names; the names, ASCII; and pairs of case
//! variants [F&O 3.1 §5.6.2], sorted.

use super::class::CharSet;
use crate::error::{Code, Result, SqlError};

/// The table, generated, and no source to review (RULES.md D3).
const BYTES: &[u8] = include_bytes!("unicode13.bin");
/// What the table begins with.
const MAGIC: &[u8; 8] = b"TBREG130";
/// The bytes before the first section: the magic and four counts.
const HEADER: usize = 24;
/// The digest of the table as generated (`cargo xtask unicode`).
#[cfg(test)]
const DIGEST: u64 = 0x0f16_3df8_d6b7_c962;
/// A section's record sizes.
const RUN_BYTES: usize = 8;
const BLOCK_BYTES: usize = 16;
const NAME_BYTES: usize = 1;
const PAIR_BYTES: usize = 8;
const PART_BYTES: usize = 4;
/// The last code point.
const LAST: u32 = 0x10_FFFF;

/// The General Categories [Unicode 13.0 §4.5], in the order the table
/// numbers them (`xtask/src/unicode/regex.rs`).
pub(super) const CATEGORIES: [&str; 30] = [
    "Lu", "Ll", "Lt", "Lm", "Lo", "Mn", "Mc", "Me", "Nd", "Nl", "No", "Pc", "Pd", "Ps", "Pe", "Pi", "Pf", "Po", "Sm", "Sc", "Sk", "So",
    "Zs", "Zl", "Zp", "Cc", "Cf", "Cs", "Co", "Cn",
];

/// [XSD 1.1 Part 2 §G.4.2.3] Block names of Unicode 3.1, which XSD 1.0
/// read, that later versions renamed: each still names its block, as XSD
/// 1.1 encourages.
const SUPERSEDED: [(&str, u32, u32); 5] = [
    ("Greek", 0x370, 0x3FF),
    ("CombiningMarksforSymbols", 0x20D0, 0x20FF),
    ("PrivateUse", 0xE000, 0xF8FF),
    ("PrivateUse", 0xF_0000, 0xF_FFFD),
    ("PrivateUse", 0x10_0000, 0x10_FFFD),
];

/// One section: where it starts, and its records.
#[derive(Debug, Clone, Copy, Default)]
struct Section {
    start: usize,
    records: usize,
    size: usize,
}

impl Section {
    /// The `n`-th number of record `i`.
    fn u32_at(self, i: usize, n: usize) -> Option<u32> {
        let at = i.checked_mul(self.size)?.checked_add(self.start)?.checked_add(n.checked_mul(PART_BYTES)?)?;
        let bytes = BYTES.get(at..at.checked_add(PART_BYTES)?)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }

    /// Record `i`'s two first numbers.
    fn pair(self, i: usize) -> Option<(u32, u32)> {
        Some((self.u32_at(i, 0)?, self.u32_at(i, 1)?))
    }
}

/// The table, its sections found.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Table {
    runs: Section,
    blocks: Section,
    names: Section,
    pairs: Section,
}

fn damaged() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] the regular expression table is damaged")
}

impl Table {
    /// The table, its sections found.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it does not hold what its header says.
    pub(crate) fn read() -> Result<Table> {
        if BYTES.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
            return Err(damaged());
        }
        let mut sections = [Section::default(); 4];
        let mut start = HEADER;
        for (n, (section, size)) in sections.iter_mut().zip([RUN_BYTES, BLOCK_BYTES, NAME_BYTES, PAIR_BYTES]).enumerate() {
            let at = MAGIC.len().saturating_add(n.saturating_mul(PART_BYTES));
            let count = BYTES.get(at..at.saturating_add(PART_BYTES)).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
            let records = count.and_then(|r| usize::try_from(r).ok()).ok_or_else(damaged)?;
            *section = Section { start, records, size };
            start = records.checked_mul(size).and_then(|b| b.checked_add(start)).ok_or_else(damaged)?;
        }
        if start != BYTES.len() {
            return Err(damaged());
        }
        let [runs, blocks, names, pairs] = sections;
        Ok(Table { runs, blocks, names, pairs })
    }

    /// The characters whose General Category is among `wanted`, places in
    /// `CATEGORIES`.
    pub(super) fn categories(&self, wanted: &[usize]) -> CharSet {
        let mut ranges = Vec::new();
        for i in 0..self.runs.records {
            let Some((first, category)) = self.runs.pair(i) else { continue };
            let next =
                i.checked_add(1).filter(|n| *n < self.runs.records).and_then(|n| self.runs.u32_at(n, 0)).unwrap_or(LAST.saturating_add(1));
            if usize::try_from(category).is_ok_and(|c| wanted.contains(&c)) {
                ranges.push((first, next.saturating_sub(1)));
            }
        }
        CharSet::of(ranges)
    }

    /// The characters of the block whose normalized name is `name`
    /// [XSD 1.1 Part 2 §G.4.2.3], case and hyphens counting; none if no
    /// block has it.
    pub(super) fn block(&self, name: &str) -> Option<CharSet> {
        let mut ranges: Vec<(u32, u32)> = SUPERSEDED.iter().filter(|(n, ..)| *n == name).map(|&(_, first, last)| (first, last)).collect();
        for i in 0..self.blocks.records {
            let (Some((first, last)), Some(at), Some(length)) = (self.blocks.pair(i), self.blocks.u32_at(i, 2), self.blocks.u32_at(i, 3))
            else {
                continue;
            };
            let at = usize::try_from(at).ok().and_then(|a| a.checked_add(self.names.start));
            let end = at.zip(usize::try_from(length).ok()).and_then(|(a, l)| a.checked_add(l));
            if at.zip(end).and_then(|(a, e)| BYTES.get(a..e)) == Some(name.as_bytes()) {
                ranges.push((first, last));
            }
        }
        (!ranges.is_empty()).then(|| CharSet::of(ranges))
    }

    /// The index of the first pair whose first character is `code` or
    /// after.
    fn first_pair(&self, code: u32) -> usize {
        let (mut low, mut high) = (0, self.pairs.records);
        while low < high {
            let middle = low.midpoint(high);
            if self.pairs.u32_at(middle, 0).is_some_and(|c| c < code) {
                low = middle.saturating_add(1);
            } else {
                high = middle;
            }
        }
        low
    }

    /// The case variants of each character from `first` to `last`.
    pub(super) fn variants(&self, first: u32, last: u32) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for i in self.first_pair(first)..self.pairs.records {
            let Some((a, b)) = self.pairs.pair(i) else { break };
            if a > last {
                break;
            }
            out.push((b, b));
        }
        out
    }

    /// Whether `a` and `b` are one character or case variants.
    pub(super) fn alike(&self, a: char, b: char) -> bool {
        let (a, b) = (u32::from(a), u32::from(b));
        a == b || self.variants(a, a).contains(&(b, b))
    }
}

/// The table's FNV-1a digest, to hold it to `DIGEST`.
#[cfg(test)]
fn digest() -> u64 {
    crate::value::collation::fnv(BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D3 the table is the one generated, by
    /// its digest, and reads.
    #[test]
    fn the_table_is_as_generated() {
        assert_eq!(digest(), DIGEST, "regenerate with cargo xtask unicode and update DIGEST");
        assert!(Table::read().is_ok());
    }

    /// Verifies: [EXTENSION] [XSD 1.1 Part 2 §G.4.2.2, §G.4.2.3] a
    /// category holds its characters and no others, unassigned code
    /// points being Cn; a block is found by its normalized name, case and
    /// hyphens counting, a superseded name too, an unknown one not.
    #[test]
    fn categories_and_blocks_are_read() {
        let table = Table::read().unwrap();
        let at = |name: &str| CATEGORIES.iter().position(|c| *c == name).unwrap();
        let lu = table.categories(&[at("Lu")]);
        assert!(lu.contains('A') && lu.contains('Ω') && !lu.contains('a') && !lu.contains('1'));
        let cn = table.categories(&[at("Cn")]);
        assert!(cn.contains('\u{378}') && cn.contains('\u{10FFFF}') && !cn.contains('A'));
        assert!(table.categories(&[at("Nd")]).contains('\u{1FBF0}'), "a digit Unicode 13.0 added");
        let latin = table.block("Latin-1Supplement").unwrap();
        assert!(latin.contains('é') && !latin.contains('e'));
        assert!(table.block("latin-1supplement").is_none() && table.block("Latin1Supplement").is_none());
        assert!(table.block("Greek").unwrap().contains('α') && table.block("GreekandCoptic").unwrap().contains('α'));
        assert!(table.block("PrivateUse").unwrap().contains('\u{10FFFD}'));
        assert!(table.block("NoSuchBlock").is_none());
    }

    /// Verifies: [EXTENSION] [F&O 3.1 §5.6.2] case variants are of full
    /// case mappings: k, K and the Kelvin sign each of the others; sharp s
    /// and capital sharp s; dotted capital I none of i or I.
    #[test]
    fn case_variants_are_read() {
        let table = Table::read().unwrap();
        assert!(table.alike('k', 'K') && table.alike('K', '\u{212A}') && table.alike('\u{212A}', 'k'));
        assert!(table.alike('ß', 'ẞ') && table.alike('x', 'x'));
        assert!(!table.alike('\u{130}', 'i') && !table.alike('I', '\u{130}') && !table.alike('a', 'b'));
        assert_eq!(table.variants(u32::from('A'), u32::from('B')), [(0x61, 0x61), (0x62, 0x62)]);
    }
}

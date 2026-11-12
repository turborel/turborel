//! The table XQuery regular expressions read, `src/regex/unicode13.bin`,
//! made from Unicode 13.0's `UnicodeData.txt`, `Blocks.txt` and
//! `SpecialCasing.txt` (RULES.md D3, D61): each code point's General
//! Category, each block's range and normalized name, and each character's
//! case variants.
//!
//! The layout is the one `src/regex/unicode.rs` reads: a magic, the count
//! of each section's records, then the sections. Categories are runs, each
//! its first code point and its category, covering every code point.
//! Blocks are their first and last code points, and where their names lie
//! in the names, which are ASCII. Variants are pairs of code points, sorted.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use super::{count, hex, put, read};
use crate::error::{Error, Result};

/// The table's path, from the workspace root.
const TABLE: &str = "src/regex/unicode13.bin";
/// What the table begins with.
const MAGIC: &[u8; 8] = b"TBREG130";
/// Each character's General Category and simple case mappings.
const UNICODE_DATA: &str = "UnicodeData.txt";
/// The blocks.
const BLOCKS: &str = "Blocks.txt";
/// The full case mappings beyond the simple ones.
const SPECIAL_CASING: &str = "SpecialCasing.txt";
/// The first line of each file the table is made from that names its version.
const BLOCKS_VERSION: &str = "# Blocks-13.0.0.txt";
const SPECIAL_CASING_VERSION: &str = "# SpecialCasing-13.0.0.txt";
/// One past the last code point.
const CODE_POINTS: u32 = 0x11_0000;
/// The General Categories, each stored as its place here; `src/regex/unicode.rs`
/// lists them in the same order.
pub const CATEGORIES: [&str; 30] = [
    "Lu", "Ll", "Lt", "Lm", "Lo", "Mn", "Mc", "Me", "Nd", "Nl", "No", "Pc", "Pd", "Ps", "Pe", "Pi", "Pf", "Po", "Sm", "Sc", "Sk", "So",
    "Zs", "Zl", "Zp", "Cc", "Cf", "Cs", "Co", "Cn",
];
/// The category of a code point `UnicodeData.txt` does not list.
const UNASSIGNED: &str = "Cn";
/// Where the fields of a line of `UnicodeData.txt` are.
const NAME_FIELD: usize = 1;
const CATEGORY_FIELD: usize = 2;
const UPPER_FIELD: usize = 12;
const LOWER_FIELD: usize = 13;
/// What ends the name of a range's first and last line.
const RANGE_FIRST: &str = ", First>";
const RANGE_LAST: &str = ", Last>";

/// What the files hold: each code point's category, the blocks, and each
/// character's full lower and upper case.
struct Data {
    categories: Vec<u8>,
    blocks: Vec<(u32, u32, String)>,
    lower: BTreeMap<u32, Vec<u32>>,
    upper: BTreeMap<u32, Vec<u32>>,
}

/// Generates the table from the files in `from`.
pub fn generate(root: &Path, from: &Path) -> Result<()> {
    let data = Data {
        categories: categories(&read(from, UNICODE_DATA)?)?,
        blocks: blocks(&read(from, BLOCKS)?)?,
        lower: BTreeMap::new(),
        upper: BTreeMap::new(),
    };
    let data = cased(data, &read(from, UNICODE_DATA)?, &read(from, SPECIAL_CASING)?)?;
    let bytes = table(&data)?;
    let path = root.join(TABLE);
    fs::write(&path, &bytes).map_err(|e| Error::new(format!("cannot write {}: {e}", path.display())))?;
    println!("unicode: wrote {} bytes to {TABLE}", bytes.len());
    Ok(())
}

/// The fields of each line of `text` that is no comment, split at `;`.
fn lines(text: &str) -> impl Iterator<Item = Vec<&str>> {
    text.lines().map(|l| l.split('#').next().unwrap_or_default()).filter(|l| !l.trim().is_empty()).map(|l| l.split(';').collect())
}

/// `fields[n]`, or an error naming the line.
fn field<'a>(fields: &[&'a str], n: usize) -> Result<&'a str> {
    fields.get(n).copied().ok_or_else(|| Error::new(format!("a line has no field {n}: {}", fields.join(";"))))
}

/// The place of the category `name` in `CATEGORIES`.
fn category(name: &str) -> Result<u8> {
    let at = CATEGORIES.iter().position(|c| *c == name).ok_or_else(|| Error::new(format!("{name} is no General Category")))?;
    u8::try_from(at).map_err(|e| Error::new(format!("category {at}: {e}")))
}

/// Each code point's category, from `UnicodeData.txt`: a range's first and
/// last lines giving every code point between them theirs.
fn categories(text: &str) -> Result<Vec<u8>> {
    let mut out = vec![category(UNASSIGNED)?; usize::try_from(CODE_POINTS).unwrap_or_default()];
    let mut first: Option<u32> = None;
    for fields in lines(text) {
        let (code, name, gc) = (hex(field(&fields, 0)?)?, field(&fields, NAME_FIELD)?, category(field(&fields, CATEGORY_FIELD)?)?);
        let from = if name.ends_with(RANGE_LAST) { first.take().unwrap_or(code) } else { code };
        if name.ends_with(RANGE_FIRST) {
            first = Some(code);
        }
        for c in from..=code {
            let slot = usize::try_from(c).ok().and_then(|i| out.get_mut(i)).ok_or_else(|| Error::new(format!("{c:X} is no code point")))?;
            *slot = gc;
        }
    }
    Ok(out)
}

/// The normalized name of the block `name` [XSD 1.1 Part 2 §G.4.2.3]: its
/// white space and underbars stripped, its hyphens and case kept.
fn normalized(name: &str) -> String {
    name.chars().filter(|c| !c.is_whitespace() && *c != '_').collect()
}

/// The blocks of `Blocks.txt`, which must be of version 13.0.
fn blocks(text: &str) -> Result<Vec<(u32, u32, String)>> {
    if text.lines().next() != Some(BLOCKS_VERSION) {
        return Err(Error::new(format!("{BLOCKS} is not of Unicode 13.0")));
    }
    let mut out = Vec::new();
    for fields in lines(text) {
        let (range, name) = (field(&fields, 0)?, field(&fields, 1)?);
        let (first, last) = range.split_once("..").ok_or_else(|| Error::new(format!("'{range}' is no range")))?;
        out.push((hex(first)?, hex(last)?, normalized(name)));
    }
    Ok(out)
}

/// The code points of a mapping's field, none if it is empty.
fn mapping(field: &str) -> Result<Vec<u32>> {
    field.split_whitespace().map(hex).collect()
}

/// `data` with each character's full lower and upper case: the simple
/// mappings of `UnicodeData.txt`, then SpecialCasing's unconditional ones
/// over them, those a context or language governs left out.
fn cased(mut data: Data, unicode_data: &str, special: &str) -> Result<Data> {
    if special.lines().next() != Some(SPECIAL_CASING_VERSION) {
        return Err(Error::new(format!("{SPECIAL_CASING} is not of Unicode 13.0")));
    }
    for fields in lines(unicode_data) {
        let code = hex(field(&fields, 0)?)?;
        for (n, into) in [(LOWER_FIELD, &mut data.lower), (UPPER_FIELD, &mut data.upper)] {
            let mapped = mapping(field(&fields, n)?)?;
            if !mapped.is_empty() {
                into.insert(code, mapped);
            }
        }
    }
    for fields in lines(special) {
        // code; lower; title; upper; [condition;] with the line's last ";"
        // leaving one empty field after.
        if fields.len() != 5 {
            continue;
        }
        let code = hex(field(&fields, 0)?)?;
        data.lower.insert(code, mapping(field(&fields, 1)?)?);
        data.upper.insert(code, mapping(field(&fields, 3)?)?);
    }
    Ok(data)
}

/// Each character's case variants [F&O 3.1 §5.6.2]: the other characters
/// whose full lower case, or whose full upper case, is the same as its.
fn variants(data: &Data) -> BTreeSet<(u32, u32)> {
    let mut characters: BTreeSet<u32> = data.lower.keys().chain(data.upper.keys()).copied().collect();
    for mapped in data.lower.values().chain(data.upper.values()) {
        if let [one] = mapped.as_slice() {
            characters.insert(*one);
        }
    }
    let cased = |map: &BTreeMap<u32, Vec<u32>>, c: u32| map.get(&c).cloned().unwrap_or_else(|| vec![c]);
    let mut out = BTreeSet::new();
    for map in [&data.lower, &data.upper] {
        let mut by_case: BTreeMap<Vec<u32>, Vec<u32>> = BTreeMap::new();
        for &c in &characters {
            by_case.entry(cased(map, c)).or_default().push(c);
        }
        for same in by_case.values() {
            for &a in same {
                out.extend(same.iter().filter(|&&b| b != a).map(|&b| (a, b)));
            }
        }
    }
    out
}

/// The table of `data`.
fn table(data: &Data) -> Result<Vec<u8>> {
    let (mut runs, mut run_count) = (Vec::new(), 0_usize);
    let mut previous = None;
    for (code, &gc) in (0..CODE_POINTS).zip(&data.categories) {
        if previous != Some(gc) {
            put(&mut runs, code);
            put(&mut runs, u32::from(gc));
            previous = Some(gc);
            run_count = run_count.saturating_add(1);
        }
    }
    let (mut blocks, mut names) = (Vec::new(), Vec::new());
    for (first, last, name) in &data.blocks {
        for value in [*first, *last, count(names.len())?, count(name.len())?] {
            put(&mut blocks, value);
        }
        names.extend_from_slice(name.as_bytes());
    }
    let pairs = variants(data);
    let mut variant_bytes = Vec::new();
    for (a, b) in &pairs {
        put(&mut variant_bytes, *a);
        put(&mut variant_bytes, *b);
    }
    let mut out = MAGIC.to_vec();
    for n in [run_count, data.blocks.len(), names.len(), pairs.len()] {
        put(&mut out, count(n)?);
    }
    for section in [&runs, &blocks, &names, &variant_bytes] {
        out.extend_from_slice(section);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [TOOL] R-TOOL-2 a range's first and last lines give every
    /// code point between them their category, and an unlisted one is Cn.
    #[test]
    fn categories_cover_ranges() {
        let text = "0041;LATIN CAPITAL LETTER A;Lu;0;L;;;;;N;;;;0061;\n4E00;<CJK Ideograph, First>;Lo;0;L;;;;;N;;;;;\n9FFC;<CJK Ideograph, Last>;Lo;0;L;;;;;N;;;;;\n";
        let out = categories(text).unwrap();
        let lo = category("Lo").unwrap();
        assert_eq!(out[0x41], category("Lu").unwrap());
        assert_eq!((out[0x4E00], out[0x5000], out[0x9FFC]), (lo, lo, lo));
        assert_eq!(out[0x42], category(UNASSIGNED).unwrap());
        assert!(category("Xx").is_err());
    }

    /// Verifies: [TOOL] R-TOOL-2 a block's name is normalized, its spaces
    /// and underbars stripped and hyphens kept; a file of another version
    /// is refused.
    #[test]
    fn blocks_are_normalized() {
        let out = blocks("# Blocks-13.0.0.txt\n0080..00FF; Latin-1 Supplement\n0370..03FF; Greek_and Coptic\n").unwrap();
        assert_eq!(out[0], (0x80, 0xFF, "Latin-1Supplement".to_owned()));
        assert_eq!(out[1].2, "GreekandCoptic");
        assert!(blocks("# Blocks-14.0.0.txt\n").is_err());
    }

    /// Verifies: [TOOL] R-TOOL-2 case variants are the characters of the
    /// same full lower or upper case: K, k and the Kelvin sign each of the
    /// others; sharp s and capital sharp s, by their lower case; dotted
    /// capital I, of full lower case two characters, of none; a condition
    /// leaves a special casing out.
    #[test]
    fn case_variants_are_of_full_mappings() {
        let unicode_data = "004B;K;Lu;;;;;;;;;;;006B;\n006B;k;Ll;;;;;;;;;;004B;;004B\n212A;KELVIN;Lu;;;;;;;;;;;006B;\n00DF;sharp;Ll;;;;;;;;;;;;\n1E9E;SHARP;Lu;;;;;;;;;;;00DF;\n0130;I DOT;Lu;;;;;;;;;;;0069;\n0069;i;Ll;;;;;;;;;;0049;;0049\n0049;I;Lu;;;;;;;;;;;0069;\n03A3;SIGMA;Lu;;;;;;;;;;;03C3;\n";
        let special = "# SpecialCasing-13.0.0.txt\n00DF; 00DF; 0053 0073; 0053 0053; # sharp\n0130; 0069 0307; 0130; 0130; # I dot\n03A3; 03C2; 03A3; 03A3; Final_Sigma; # sigma\n";
        let data = Data { categories: Vec::new(), blocks: Vec::new(), lower: BTreeMap::new(), upper: BTreeMap::new() };
        let pairs = variants(&cased(data, unicode_data, special).unwrap());
        for (a, b) in [(0x4B, 0x6B), (0x4B, 0x212A), (0x6B, 0x212A), (0x212A, 0x4B), (0xDF, 0x1E9E), (0x1E9E, 0xDF), (0x49, 0x69)] {
            assert!(pairs.contains(&(a, b)), "{a:X} {b:X}");
        }
        assert!(!pairs.iter().any(|&(a, _)| a == 0x130));
        assert!(pairs.contains(&(0x3A3, 0x3C3)));
    }
}

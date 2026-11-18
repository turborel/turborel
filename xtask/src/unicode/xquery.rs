//! The table XQuery's functions on strings read,
//! `src/xquery/library/unicode13.bin`, made from Unicode 13.0's
//! `UnicodeData.txt`, `CompositionExclusions.txt` and `SpecialCasing.txt`
//! (RULES.md D3, D86): each character's canonical combining class, its
//! full canonical and full compatibility decompositions, its full upper
//! and lower case, and the primary composites \[UAX #15 §1.3\].
//!
//! The layout is the one `src/xquery/library/unicode.rs` reads: a magic,
//! the count of each section's records, then the sections. Classes are
//! pairs of a code point and its class. A mapping is records of a code
//! point and its parts' count and place, then the parts. Composites are a
//! first and second code point and the one they compose to. Each section
//! is sorted.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use super::regex::{Data, cased, field, lines};
use super::{count, counted, hex, put, read};
use crate::error::{Error, Result};

/// The table's path, from the workspace root.
const TABLE: &str = "src/xquery/library/unicode13.bin";
/// What the table begins with.
const MAGIC: &[u8; 8] = b"TBXQF130";
/// Each character's combining class, decomposition and simple cases.
const UNICODE_DATA: &str = "UnicodeData.txt";
/// The characters excluded from composition that their decompositions
/// alone do not show.
const EXCLUSIONS: &str = "CompositionExclusions.txt";
/// The first line of the exclusions, which names their version.
const EXCLUSIONS_VERSION: &str = "# CompositionExclusions-13.0.0.txt";
/// The full case mappings beyond the simple ones.
const SPECIAL_CASING: &str = "SpecialCasing.txt";
/// Where the fields of a line of `UnicodeData.txt` are.
const CLASS_FIELD: usize = 3;
const DECOMPOSITION_FIELD: usize = 5;

/// What `UnicodeData.txt` says of decomposition: each character's class
/// if not 0, and its one-level mapping, whether of compatibility.
#[derive(Default)]
struct Decompositions {
    classes: BTreeMap<u32, u32>,
    mappings: BTreeMap<u32, (bool, Vec<u32>)>,
}

/// The classes and one-level mappings of `text`.
fn decompositions(text: &str) -> Result<Decompositions> {
    let mut out = Decompositions::default();
    for fields in lines(text) {
        let code = hex(field(&fields, 0)?)?;
        let class: u32 = field(&fields, CLASS_FIELD)?.parse().map_err(|e| Error::new(format!("{code:X}'s class: {e}")))?;
        if class != 0 {
            out.classes.insert(code, class);
        }
        let mapping = field(&fields, DECOMPOSITION_FIELD)?.trim();
        if mapping.is_empty() {
            continue;
        }
        let (compatible, parts) = match mapping.strip_prefix('<') {
            Some(tagged) => (true, tagged.split_once('>').map_or("", |(_, rest)| rest)),
            None => (false, mapping),
        };
        out.mappings.insert(code, (compatible, parts.split_whitespace().map(hex).collect::<Result<Vec<u32>>>()?));
    }
    Ok(out)
}

/// `code` decomposed fully \[UAX #15 §1.3\]: by canonical mappings alone,
/// or by compatibility ones too if `compatible`.
fn full(code: u32, data: &Decompositions, compatible: bool) -> Vec<u32> {
    let (mut done, mut waiting) = (Vec::new(), vec![code]);
    while let Some(next) = waiting.pop() {
        match data.mappings.get(&next).filter(|(k, _)| compatible || !*k) {
            Some((_, parts)) => waiting.extend(parts.iter().rev()),
            None => done.push(next),
        }
    }
    done
}

/// The mapping of every character that decomposes, fully.
fn full_mappings(data: &Decompositions, compatible: bool) -> BTreeMap<u32, Vec<u32>> {
    data.mappings
        .iter()
        .filter(|(_, (k, _))| compatible || !*k)
        .map(|(&code, _)| (code, full(code, data, compatible)))
        .filter(|(code, parts)| parts.as_slice() != [*code])
        .collect()
}

/// The code points `text` lists, its first line naming version 13.0.
fn exclusions(text: &str) -> Result<BTreeSet<u32>> {
    if text.lines().next() != Some(EXCLUSIONS_VERSION) {
        return Err(Error::new(format!("{EXCLUSIONS} is not of Unicode 13.0")));
    }
    lines(text).map(|fields| field(&fields, 0).and_then(|f| hex(f.trim()))).collect()
}

/// The primary composites \[UAX #15 §1.3\]: each character whose one-level
/// canonical mapping is two characters, but those excluded: listed, of a
/// non-zero class, or decomposing to a non-starter first.
fn composites(data: &Decompositions, excluded: &BTreeSet<u32>) -> BTreeMap<(u32, u32), u32> {
    let class = |c: u32| data.classes.get(&c).copied().unwrap_or(0);
    data.mappings
        .iter()
        .filter(|(code, (compatible, _))| !compatible && !excluded.contains(code) && class(**code) == 0)
        .filter_map(|(&code, (_, parts))| {
            let [first, second] = parts.as_slice() else { return None };
            (class(*first) == 0).then_some(((*first, *second), code))
        })
        .collect()
}

/// `map` as a mapping section: its records, its parts, and how many.
fn mapping_sections(map: &BTreeMap<u32, Vec<u32>>) -> Result<(Vec<u8>, Vec<u8>, usize)> {
    let (mut records, mut parts, mut held) = (Vec::new(), Vec::new(), 0_usize);
    for (&code, mapped) in map {
        put(&mut records, code);
        put(&mut records, counted(mapped.len(), held)?);
        held = held.saturating_add(mapped.len());
        for &part in mapped {
            put(&mut parts, part);
        }
    }
    Ok((records, parts, held))
}

/// The table of `data`, the case maps `cases` and the composites.
fn table(data: &Decompositions, cases: &Data, composed: &BTreeMap<(u32, u32), u32>) -> Result<Vec<u8>> {
    let mut sizes = vec![data.classes.len()];
    let mut sections = vec![data.classes.iter().flat_map(|(&k, &v)| [k, v]).flat_map(u32::to_le_bytes).collect::<Vec<u8>>()];
    for map in [&full_mappings(data, false), &full_mappings(data, true), &cases.upper, &cases.lower] {
        let (records, parts, held) = mapping_sections(map)?;
        sizes.extend([map.len(), held]);
        sections.extend([records, parts]);
    }
    sizes.push(composed.len());
    sections.push(composed.iter().flat_map(|(&(a, b), &c)| [a, b, c]).flat_map(u32::to_le_bytes).collect());
    let mut out = MAGIC.to_vec();
    for size in sizes {
        put(&mut out, count(size)?);
    }
    for section in sections {
        out.extend(section);
    }
    Ok(out)
}

/// Generates the table from the files in `from`.
pub fn generate(root: &Path, from: &Path) -> Result<()> {
    let unicode_data = read(from, UNICODE_DATA)?;
    let data = decompositions(&unicode_data)?;
    let excluded = exclusions(&read(from, EXCLUSIONS)?)?;
    let empty = Data { categories: Vec::new(), blocks: Vec::new(), lower: BTreeMap::new(), upper: BTreeMap::new() };
    let cases = cased(empty, &unicode_data, &read(from, SPECIAL_CASING)?)?;
    let bytes = table(&data, &cases, &composites(&data, &excluded))?;
    let path = root.join(TABLE);
    fs::write(&path, &bytes).map_err(|e| Error::new(format!("cannot write {}: {e}", path.display())))?;
    println!("unicode: wrote {} bytes to {TABLE}", bytes.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &str = "00C5;A RING;Lu;0;L;0041 030A;;;;N;;;;00E5;\n0041;A;Lu;0;L;;;;;N;;;;0061;\n030A;RING;Mn;230;NSM;;;;;N;;;;;\n212B;ANGSTROM;Lu;0;L;00C5;;;;N;;;;00E5;\nFB01;FI;Ll;0;L;<compat> 0066 0069;;;;N;;;;;\n0958;QA;Lo;0;L;0915 093C;;;;N;;;;;\n0344;DIALYTIKA TONOS;Mn;230;NSM;0308 0301;;;;N;;;;;\n";

    /// Verifies: [TOOL] R-TOOL-2 decompositions are read one level deep,
    /// a tagged one of compatibility; full ones expand every level, a
    /// canonical one past no compatibility mapping.
    #[test]
    fn decompositions_expand() {
        let data = decompositions(DATA).unwrap();
        assert_eq!(data.classes.get(&0x30A), Some(&230));
        assert_eq!(data.mappings.get(&0xFB01), Some(&(true, vec![0x66, 0x69])));
        assert_eq!(full(0x212B, &data, false), vec![0x41, 0x30A]);
        assert_eq!(full(0xFB01, &data, false), vec![0xFB01]);
        assert_eq!(full(0xFB01, &data, true), vec![0x66, 0x69]);
        assert!(!full_mappings(&data, false).contains_key(&0xFB01));
    }

    /// Verifies: [TOOL] R-TOOL-2 the primary composites are the canonical
    /// pairs of no listed exclusion, singleton or non-starter; the
    /// exclusions' version is checked.
    #[test]
    fn composites_exclude() {
        let data = decompositions(DATA).unwrap();
        let excluded = exclusions("# CompositionExclusions-13.0.0.txt\n0958    #  QA\n").unwrap();
        let pairs = composites(&data, &excluded);
        assert_eq!(pairs.get(&(0x41, 0x30A)), Some(&0xC5));
        assert_eq!(pairs.len(), 1);
        assert!(exclusions("# CompositionExclusions-14.0.0.txt\n").is_err());
    }
}

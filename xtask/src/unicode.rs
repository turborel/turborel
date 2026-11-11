//! `cargo xtask unicode --from DIR`: generates the table the collation
//! UNICODE reads, `src/value/collation/unicode13.bin`, from the Unicode
//! Character Database files in DIR (RULES.md D60). It reads local files and
//! uses no network (R-TEST-5), and only std (R-TOOL-2).
//!
//! The layout is the one `src/value/collation/table.rs` reads: a magic, the
//! count of each section's records, then the sections, each a run of
//! fixed-size little-endian records sorted by code point.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// The table's path, from the workspace root.
const TABLE: &str = "src/value/collation/unicode13.bin";
/// What the table begins with.
const MAGIC: &[u8; 8] = b"TBUCA130";
/// The Unicode Collation Algorithm's weights.
const ALLKEYS: &str = "allkeys.txt";
/// Each character's combining class and canonical decomposition.
const UNICODE_DATA: &str = "UnicodeData.txt";
/// Simple case folding.
const CASE_FOLDING: &str = "CaseFolding.txt";
/// The version of the files, which the table must be made from.
const VERSION_LINE: &str = "@version 13.0.0";
/// The line of an implicit weight range, in `ALLKEYS`.
const IMPLICIT_LINE: &str = "@implicitweights";
/// The most characters of a contraction.
const MOST_CONTRACTED: usize = 3;
/// Where a record's count sits above its offset.
const COUNT_SHIFT: u32 = 24;
/// The flag of a character that begins a contraction.
const BEGINS_CONTRACTION: u32 = 1 << 31;
/// A field holds hexadecimal.
const HEX: u32 = 16;

/// One collation element: a primary, secondary and tertiary weight, and
/// whether it is variable.
#[derive(Clone, Copy)]
pub struct Element {
    pub weights: [u16; 3],
    pub variable: bool,
}

/// What `allkeys.txt` holds.
#[derive(Default)]
pub struct Weights {
    pub single: BTreeMap<u32, Vec<Element>>,
    contracted: BTreeMap<[u32; MOST_CONTRACTED], Vec<Element>>,
    /// The first and last code point of a range, its primary and origin.
    pub implicit: Vec<[u32; 4]>,
}

/// The Unicode files a table is made from, read.
pub struct Sources {
    pub decompositions: BTreeMap<u32, Vec<u32>>,
    pub classes: BTreeMap<u32, u32>,
    pub folds: BTreeMap<u32, u32>,
}

impl Sources {
    /// `UnicodeData.txt` and `CaseFolding.txt` in `from`, read.
    pub fn read(from: &Path) -> Result<Sources> {
        let (decompositions, classes) = parsed_data(&read(from, UNICODE_DATA)?)?;
        let folds = parsed_folds(&read(from, CASE_FOLDING)?)?;
        Ok(Sources { decompositions, classes, folds })
    }
}

/// Generates the table from the files in `from`.
pub fn generate(root: &Path, from: &Path) -> Result<()> {
    let weights = parsed_weights(&read(from, ALLKEYS)?)?;
    let bytes = table(*MAGIC, &weights, &Sources::read(from)?)?;
    let path = root.join(TABLE);
    fs::write(&path, &bytes).map_err(|e| Error::new(format!("cannot write {}: {e}", path.display())))?;
    println!("unicode: wrote {} bytes to {TABLE}", bytes.len());
    Ok(())
}

/// The weights of `allkeys.txt` in `from`.
pub fn ducet(from: &Path) -> Result<Weights> {
    parsed_weights(&read(from, ALLKEYS)?)
}

pub fn read(from: &Path, name: &str) -> Result<String> {
    let path = from.join(name);
    fs::read_to_string(&path).map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))
}

pub fn hex(field: &str) -> Result<u32> {
    u32::from_str_radix(field.trim(), HEX).map_err(|e| Error::new(format!("'{field}' is no hexadecimal number: {e}")))
}

/// The element of `text`, between its brackets: `*0201.0020.0002`.
fn element(text: &str) -> Result<Element> {
    let variable = text.starts_with('*');
    let mut fields = text.trim_start_matches(['*', '.']).split('.');
    let mut weights = [0_u16; 3];
    for weight in &mut weights {
        let field = fields.next().ok_or_else(|| Error::new(format!("'{text}' has too few weights")))?;
        *weight = u16::try_from(hex(field)?).map_err(|e| Error::new(format!("'{text}': {e}")))?;
    }
    Ok(Element { weights, variable })
}

/// The range of an `@implicitweights` line: `17000..18AFF; FB00 # Tangut`.
fn implicit_range(line: &str) -> Result<[u32; 3]> {
    let wrong = || Error::new(format!("'{line}' is no implicit weight range"));
    let body = line.strip_prefix(IMPLICIT_LINE).ok_or_else(wrong)?;
    let (range, rest) = body.split_once(';').ok_or_else(wrong)?;
    let (first, last) = range.split_once("..").ok_or_else(wrong)?;
    let primary = rest.split('#').next().ok_or_else(wrong)?;
    Ok([hex(first)?, hex(last)?, hex(primary)?])
}

/// A line of weights, `0041 ; [.1FA2.0020.0008] # ...`.
fn weight_line(line: &str, into: &mut Weights) -> Result<()> {
    let body = line.split('#').next().unwrap_or_default();
    let (codes, elements) = body.split_once(';').ok_or_else(|| Error::new(format!("'{line}' has no weights")))?;
    let codes = codes.split_whitespace().map(hex).collect::<Result<Vec<u32>>>()?;
    let elements = elements.split(['[', ']']).filter(|e| !e.trim().is_empty()).map(element).collect::<Result<Vec<Element>>>()?;
    match codes.as_slice() {
        [one] => {
            into.single.insert(*one, elements);
        }
        many if many.len() <= MOST_CONTRACTED => {
            let mut key = [0; MOST_CONTRACTED];
            key.iter_mut().zip(many).for_each(|(k, c)| *k = *c);
            into.contracted.insert(key, elements);
        }
        _ => return Err(Error::new(format!("'{line}' contracts too many characters"))),
    }
    Ok(())
}

pub fn parsed_weights(text: &str) -> Result<Weights> {
    if !text.lines().any(|l| l.trim() == VERSION_LINE) {
        return Err(Error::new(format!("{ALLKEYS} is not of {VERSION_LINE}")));
    }
    let mut weights = Weights::default();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if line.starts_with(IMPLICIT_LINE) {
            let [first, last, primary] = implicit_range(line)?;
            weights.implicit.push([first, last, primary, first]);
        } else if !line.starts_with('@') {
            weight_line(line, &mut weights)?;
        }
    }
    // [UTS #10 §10.1.3] a script's ranges count from its first.
    let of_primary: Vec<[u32; 4]> = weights.implicit.clone();
    for range in &mut weights.implicit {
        let origin = of_primary.iter().filter(|r| r[2] == range[2]).map(|r| r[0]).min();
        range[3] = origin.unwrap_or(range[0]);
    }
    weights.implicit.sort_unstable();
    Ok(weights)
}

/// Each character's full canonical decomposition, and its combining class
/// if not 0.
#[expect(clippy::type_complexity, reason = "two maps, made together")]
fn parsed_data(text: &str) -> Result<(BTreeMap<u32, Vec<u32>>, BTreeMap<u32, u32>)> {
    let (mut once, mut classes) = (BTreeMap::new(), BTreeMap::new());
    for line in text.lines() {
        let fields: Vec<&str> = line.split(';').collect();
        let [code, _, _, class, _, decomposition, ..] = fields.as_slice() else {
            return Err(Error::new(format!("'{line}' is no line of {UNICODE_DATA}")));
        };
        let code = hex(code)?;
        let class: u32 = class.parse().map_err(|e| Error::new(format!("'{line}': {e}")))?;
        if class != 0 {
            classes.insert(code, class);
        }
        if !decomposition.is_empty() && !decomposition.starts_with('<') {
            once.insert(code, decomposition.split_whitespace().map(hex).collect::<Result<Vec<u32>>>()?);
        }
    }
    let full = once.keys().map(|&c| (c, expanded(c, &once))).collect();
    Ok((full, classes))
}

/// `code` decomposed until nothing is left to decompose.
fn expanded(code: u32, once: &BTreeMap<u32, Vec<u32>>) -> Vec<u32> {
    let (mut done, mut waiting) = (Vec::new(), vec![code]);
    while let Some(next) = waiting.pop() {
        match once.get(&next) {
            Some(parts) => waiting.extend(parts.iter().rev()),
            None => done.push(next),
        }
    }
    done
}

/// Common and simple case folding, which keep a string's character count.
fn parsed_folds(text: &str) -> Result<BTreeMap<u32, u32>> {
    let mut folds = BTreeMap::new();
    for line in text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let fields: Vec<&str> = line.split(';').map(str::trim).collect();
        let [code, status, mapping, ..] = fields.as_slice() else {
            return Err(Error::new(format!("'{line}' is no line of {CASE_FOLDING}")));
        };
        if matches!(*status, "C" | "S") {
            folds.insert(hex(code)?, hex(mapping)?);
        }
    }
    Ok(folds)
}

fn put(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn count(n: usize) -> Result<u32> {
    u32::try_from(n).map_err(|e| Error::new(format!("{n} records: {e}")))
}

/// A record's count and offset, in one number.
fn counted(items: usize, offset: usize) -> Result<u32> {
    let (items, offset) = (count(items)?, count(offset)?);
    if items >= 1 << (u32::BITS - COUNT_SHIFT) || offset >= 1 << COUNT_SHIFT {
        return Err(Error::new(format!("{items} items at {offset} do not fit a record")));
    }
    Ok(items << COUNT_SHIFT | offset)
}

/// `elements` added to `pool`: where they begin.
fn pooled(pool: &mut Vec<Element>, elements: &[Element]) -> usize {
    let at = pool.len();
    pool.extend_from_slice(elements);
    at
}

/// The sections of the weights: the characters, their elements and the
/// contractions, with how many of each.
fn weight_sections(weights: &Weights) -> Result<([Vec<u8>; 3], [usize; 3])> {
    let mut pool = Vec::new();
    let mut codes: BTreeMap<u32, (usize, usize)> = BTreeMap::new();
    for (&code, elements) in &weights.single {
        codes.insert(code, (elements.len(), pooled(&mut pool, elements)));
    }
    let mut contractions = Vec::new();
    for (key, elements) in &weights.contracted {
        let first = key.first().copied().unwrap_or_default();
        codes.entry(first).or_insert((0, 0));
        let at = pooled(&mut pool, elements);
        for c in key {
            put(&mut contractions, *c);
        }
        put(&mut contractions, counted(elements.len(), at)?);
    }
    let begins = |code: u32| weights.contracted.keys().any(|k| k.first() == Some(&code));
    let mut chars = Vec::new();
    for (&code, &(items, at)) in &codes {
        put(&mut chars, if begins(code) { code | BEGINS_CONTRACTION } else { code });
        put(&mut chars, counted(items, at)?);
    }
    let mut elements = Vec::new();
    for e in &pool {
        for weight in e.weights {
            elements.extend_from_slice(&weight.to_le_bytes());
        }
        elements.extend_from_slice(&[u8::from(e.variable), 0]);
    }
    Ok(([chars, elements, contractions], [codes.len(), pool.len(), weights.contracted.len()]))
}

/// The sections of the decompositions, and how many parts they have.
fn decomposition_sections(decompositions: &BTreeMap<u32, Vec<u32>>) -> Result<(Vec<u8>, Vec<u8>, usize)> {
    let (mut records, mut parts) = (Vec::new(), Vec::new());
    let mut held = 0;
    for (&code, full) in decompositions {
        put(&mut records, code);
        put(&mut records, counted(full.len(), held)?);
        held = held.saturating_add(full.len());
        for &part in full {
            put(&mut parts, part);
        }
    }
    Ok((records, parts, held))
}

/// `map` as a section of pairs.
fn pairs(map: &BTreeMap<u32, u32>) -> Vec<u8> {
    let mut out = Vec::new();
    for (&k, &v) in map {
        put(&mut out, k);
        put(&mut out, v);
    }
    out
}

/// The table of `weights` and `sources`, beginning with `magic`.
pub fn table(magic: [u8; 8], weights: &Weights, sources: &Sources) -> Result<Vec<u8>> {
    let Sources { decompositions, classes, folds } = sources;
    let ([chars, elements, contractions], [char_count, element_count, contraction_count]) = weight_sections(weights)?;
    let (decomposed, parts, part_count) = decomposition_sections(decompositions)?;
    let mut implicit = Vec::new();
    for &value in weights.implicit.iter().flatten() {
        put(&mut implicit, value);
    }
    let mut out = magic.to_vec();
    let sizes = [
        char_count,
        element_count,
        contraction_count,
        decompositions.len(),
        part_count,
        classes.len(),
        folds.len(),
        weights.implicit.len(),
    ];
    for size in sizes {
        put(&mut out, count(size)?);
    }
    for section in [&chars, &elements, &contractions, &decomposed, &parts, &pairs(classes), &pairs(folds), &implicit] {
        out.extend_from_slice(section);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [TOOL] R-TOOL-2 a line of weights gives its elements, a
    /// variable one marked; several code points make a contraction.
    #[test]
    fn lines_of_weights_are_read() {
        let mut weights = Weights::default();
        weight_line("0041  ; [.1FA2.0020.0008] # LATIN", &mut weights).unwrap();
        weight_line("0020  ; [*0209.0020.0002] # SPACE", &mut weights).unwrap();
        weight_line("0FB2 0F71 ; [.2D01.0020.0002][.0000.0120.0002] # TIBETAN", &mut weights).unwrap();
        let a = weights.single[&0x41][0];
        assert_eq!((a.weights, a.variable), ([0x1FA2, 0x20, 8], false));
        assert!(weights.single[&0x20][0].variable);
        assert_eq!(weights.contracted[&[0xFB2, 0xF71, 0]].len(), 2);
    }

    /// Verifies: [TOOL] R-TOOL-2 a decomposition is followed to its end,
    /// in order, and a character with none stands for itself.
    #[test]
    fn decompositions_are_followed_to_their_end() {
        let once = BTreeMap::from([(1, vec![2, 3]), (2, vec![4, 5])]);
        assert_eq!(expanded(1, &once), [4, 5, 3]);
        assert_eq!(expanded(9, &once), [9]);
    }

    /// Verifies: [TOOL] R-TOOL-2 only common and simple folds are kept,
    /// and a script's ranges count from its first code point.
    #[test]
    fn folds_and_ranges_are_read() {
        let folds = parsed_folds("# note\n0041; C; 0061; # A\n00DF; F; 0073 0073; # sharp s\n1E9E; S; 00DF; # capital\n").unwrap();
        assert_eq!(folds.len(), 2);
        assert_eq!(folds[&0x1E9E], 0xDF);
        let text = "@version 13.0.0\n@implicitweights 17000..18AFF; FB00 # T\n@implicitweights 18D00..18D8F; FB00 # S\n";
        let weights = parsed_weights(text).unwrap();
        assert_eq!(weights.implicit[1], [0x18D00, 0x18D8F, 0xFB00, 0x17000]);
        assert!(parsed_weights("@version 14.0.0\n").is_err());
    }
}

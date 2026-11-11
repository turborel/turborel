//! The script reordering groups of CLDR 38 \[UTS #35 Part 5 §3.13\], laid
//! over UNICODE's primary weights, and the radical-stroke order of the
//! unified ideographs, both read from CLDR's `FractionalUCA.txt`.
//!
//! CLDR names a group by the first byte of its primary weights, and lists
//! the scripts each such byte holds. A character's group is the one its
//! weight in CLDR's root begins with, and a primary of UNICODE's is in the
//! group of the characters that have it. The groups before the letters are
//! told apart as CLDR's reorder codes tell them: digits by CLDR's own
//! group, then spaces, punctuation, currency and other symbols by general
//! category. Each primary up to the next group's is in the group before,
//! the last letter group runs to the reserved weight before the unified
//! ideographs, and from there the ideographs' group runs to the end of
//! their implicit weights.

use std::collections::{BTreeMap, HashMap};

use crate::error::{Error, Result};
use crate::unicode::{Weights, hex};

/// What a line naming a first byte's group begins with.
const TOP_BYTE: &str = "[top_byte\t";
/// What a line listing a radical's ideographs begins with, and the rest
/// of the line after the last.
const RADICAL: &str = "[radical ";
const RADICALS_END: &str = "end]";
/// The groups before the letters, as CLDR names them.
const SPACE_PUNCTUATION: &str = "SPACE PUNCTUATION";
const SYMBOL_CURRENCY: &str = "SYMBOL CURRENCY";
const DIGIT: &str = "DIGIT";
/// The weight a reset to `[last regular]` names, which begins the group
/// of the unified ideographs, and the last of their implicit weights.
const LAST_REGULAR: u32 = 0xFB3F;
const LAST_IDEOGRAPH: u32 = 0xFBBF;
const IDEOGRAPHS: &str = "Hani Hans Hant";
/// The reorder codes of the groups before the letters \[UTS #35 Part 5
/// §3.13\], and the general category of currency.
const SPACE: &str = "space";
const PUNCT: &str = "punct";
const SYMBOL: &str = "symbol";
const CURRENCY: &str = "currency";
const DIGIT_CODE: &str = "digit";
const CURRENCY_CATEGORY: &str = "Sc";
/// The hexadecimal digits of a weight's first byte.
const BYTE_DIGITS: usize = 2;
/// Where an implicit or ignorable weight is written instead of a first byte.
const NO_BYTE: [&str; 2] = ["U+", ","];

/// A range of primaries and its group.
struct Range {
    first: u32,
    last: u32,
    group: String,
}

/// The group CLDR gives each first byte.
fn top_bytes(fractional: &str) -> Result<BTreeMap<u32, String>> {
    let mut out = BTreeMap::new();
    for line in fractional.lines().filter_map(|l| l.strip_prefix(TOP_BYTE)) {
        let mut fields = line.split(']').next().unwrap_or_default().split('\t');
        let byte = hex(fields.next().unwrap_or_default())?;
        out.insert(byte, fields.next().unwrap_or_default().trim().to_owned());
    }
    Ok(out)
}

/// The group of each character whose root weight CLDR gives on a line of
/// its own.
fn characters(fractional: &str, tops: &BTreeMap<u32, String>) -> Result<HashMap<u32, String>> {
    let mut out = HashMap::new();
    for line in fractional.lines() {
        let Some((code, rest)) = line.split_once(';') else { continue };
        let Some(weight) = rest.trim_start().strip_prefix('[') else { continue };
        if code.contains(' ') || code.starts_with('[') || NO_BYTE.iter().any(|n| weight.starts_with(n)) {
            continue;
        }
        let byte = hex(weight.get(..BYTE_DIGITS).ok_or_else(|| Error::new(format!("a weight too short: {line}")))?)?;
        let group = tops.get(&byte).ok_or_else(|| Error::new(format!("no group for byte {byte:02X}")))?;
        out.insert(hex(code.trim())?, group.clone());
    }
    Ok(out)
}

/// The general category of each character `UnicodeData.txt` lists.
fn categories(data: &str) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    for line in data.lines() {
        let mut fields = line.split(';');
        if let (Some(code), Some(_), Some(category)) = (fields.next(), fields.next(), fields.next())
            && let Ok(code) = hex(code)
        {
            out.insert(code, category.to_owned());
        }
    }
    out
}

/// The reorder code of a character in a group before the letters.
fn special(category: &str) -> &'static str {
    match category.chars().next() {
        Some('Z' | 'C') => SPACE,
        Some('P') => PUNCT,
        _ if category == CURRENCY_CATEGORY => CURRENCY,
        _ => SYMBOL,
    }
}

/// The groups as reorder codes name them, of each primary of `weights`.
fn primaries(fractional: &str, weights: &Weights, data: &str) -> Result<BTreeMap<u32, String>> {
    let tops = top_bytes(fractional)?;
    let groups = characters(fractional, &tops)?;
    let categories = categories(data);
    let mut out: BTreeMap<u32, String> = BTreeMap::new();
    for (code, elements) in &weights.single {
        let Some(primary) = elements.first().map(|e| u32::from(e.weights[0])).filter(|p| *p != 0) else { continue };
        let Some(group) = groups.get(code) else { continue };
        let group = match group.as_str() {
            DIGIT => DIGIT_CODE.to_owned(),
            SPACE_PUNCTUATION | SYMBOL_CURRENCY => special(categories.get(code).map_or("Cn", String::as_str)).to_owned(),
            script => script.split('\t').next().unwrap_or(script).to_owned(),
        };
        // A primary two groups share is the letters', as the special
        // groups' are told apart by category alone.
        let letters = group.chars().next().is_some_and(char::is_uppercase);
        if letters || !out.contains_key(&primary) {
            out.insert(primary, group);
        }
    }
    Ok(out)
}

/// The ranges of primaries each group holds.
fn ranges(primaries: &BTreeMap<u32, String>) -> Result<Vec<Range>> {
    let mut out: Vec<Range> = Vec::new();
    for (primary, group) in primaries.range(..LAST_REGULAR) {
        match out.last_mut() {
            Some(last) if last.group == *group => last.last = *primary,
            Some(last) => {
                last.last = primary.saturating_sub(1);
                out.push(Range { first: *primary, last: *primary, group: group.clone() });
            }
            None => out.push(Range { first: *primary, last: *primary, group: group.clone() }),
        }
    }
    if let Some(last) = out.last_mut() {
        last.last = LAST_REGULAR.saturating_sub(1);
    }
    out.push(Range { first: LAST_REGULAR, last: LAST_IDEOGRAPH, group: IDEOGRAPHS.to_owned() });
    for (n, range) in out.iter().enumerate() {
        let script = range.group.chars().next().is_some_and(char::is_uppercase);
        if script && out.iter().skip(n.saturating_add(1)).any(|r| r.group == range.group) {
            return Err(Error::new(format!("the group {} is not contiguous", range.group)));
        }
    }
    Ok(out)
}

/// The groups' file: a line for each range, its first and last primary
/// and the codes of its group.
pub fn text(header: &str, fractional: &str, weights: &Weights, data: &str) -> Result<String> {
    let lines: Vec<String> =
        ranges(&primaries(fractional, weights, data)?)?.iter().map(|r| format!("{:04X} {:04X} {}\n", r.first, r.last, r.group)).collect();
    Ok([header.to_owned(), lines.concat()].concat())
}

/// The rules putting the unified ideographs in radical-stroke order after
/// `[last regular]`, a list for each radical.
pub fn radicals(fractional: &str) -> Result<Vec<String>> {
    let mut out = vec!["&[last regular]".to_owned()];
    for line in fractional.lines().filter_map(|l| l.strip_prefix(RADICAL)).filter(|l| *l != RADICALS_END) {
        let (_, list) = line.split_once(':').ok_or_else(|| Error::new(format!("a radical with no list: {line}")))?;
        out.push(["<*", list.trim_end().trim_end_matches(']')].concat());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [TOOL] R-TOOL-2 each primary is in its characters' group,
    /// the special groups by category, the last letter group runs to
    /// `[last regular]`, and a script split in two is refused, though a
    /// special group may be. Lines of no character's own weight are passed
    /// over. MC/DC of passing one over, a contraction, an option, or an
    /// implicit or ignorable weight: `0061 0300`, `[first`, `4E00` and
    /// `0300` against `0061`. MC/DC of a split refused, of a script and
    /// split: Latin against punctuation (script), and Latin split against
    /// whole (split).
    #[test]
    fn primaries_fall_in_their_characters_groups() {
        let fractional = "[top_byte\t03\tSPACE PUNCTUATION ]  # 694 primary weights\n[top_byte\t0E\tDIGIT ]\n[top_byte\t29\tLatn ]\n\
                          [top_byte\t2C\tGrek Copt\tCOMPRESS ]\n0020; [03 05, 05, 05]\n0021; [03 07, 05, 05]\n\
                          0030; [0E 02, 05, 05]\n0061; [29 02, 05, 05]\n03B1; [2C 02, 05, 05]\n0300; [, 88, 05]\n\
                          0061 0300; [29 05, 05, 05]\n[first x; [29 02, 05, 05]\n4E00; [U+4E00, 05]\n0022; [03 09, 05, 05]\n";
        let data = "0020;SPACE;Zs\n0021;EXCLAMATION;Po\n0022;QUOTE;Po\n0030;ZERO;Nd\n0061;A;Ll\n03B1;ALPHA;Ll\n";
        let element = |p: u16| crate::unicode::Element { weights: [p, 0x20, 2], variable: false };
        let mut weights = Weights::default();
        for (code, p) in [(0x20, 0x0201), (0x21, 0x0210), (0x22, 0x1FA0), (0x30, 0x1F98), (0x61, 0x1FA2), (0x3B1, 0x231E), (0x300, 0)] {
            weights.single.insert(code, vec![element(p)]);
        }
        let text = text("", fractional, &weights, data).unwrap();
        assert_eq!(
            text,
            "0201 020F space\n0210 1F97 punct\n1F98 1F9F digit\n1FA0 1FA1 punct\n1FA2 231D Latn\n231E FB3E Grek Copt\nFB3F FBBF Hani Hans Hant\n"
        );
        weights.single.insert(0x62, vec![element(0x2400)]);
        assert!(text_of(fractional, &weights, &format!("{data}0062;B;Ll\n"), "0062; [29 03, 05, 05]\n").is_err());
    }

    /// Verifies: [TOOL] R-TOOL-2 a primary two characters share is in the
    /// letters' group if one is a letter, else in the first's. MC/DC of
    /// taking the later, a letter or the first seen: `$` then `b` (letter)
    /// against `!` then `"` (neither), and `!` alone (first seen).
    #[test]
    fn a_shared_primary_is_the_letters() {
        let fractional = "[top_byte\t03\tSPACE PUNCTUATION ]\n[top_byte\t0C\tSYMBOL CURRENCY ]\n[top_byte\t29\tLatn ]\n\
                          0021; [03 05, 05, 05]\n0022; [03 07, 05, 05]\n0024; [0C 05, 05, 05]\n0062; [29 02, 05, 05]\n";
        let data = "0021;EXCLAMATION;Po\n0022;QUOTE;So\n0024;DOLLAR;Sc\n0062;B;Ll\n";
        let element = |p: u16| crate::unicode::Element { weights: [p, 0x20, 2], variable: false };
        let mut weights = Weights::default();
        for (code, p) in [(0x21, 0x0210), (0x22, 0x0210), (0x24, 0x0400), (0x62, 0x0400)] {
            weights.single.insert(code, vec![element(p)]);
        }
        let found = primaries(fractional, &weights, data).unwrap();
        assert_eq!(found.get(&0x0210).map(String::as_str), Some(PUNCT));
        assert_eq!(found.get(&0x0400).map(String::as_str), Some("Latn"));
    }

    fn text_of(fractional: &str, weights: &Weights, data: &str, more: &str) -> Result<String> {
        text("", &[fractional, more].concat(), weights, data)
    }

    /// Verifies: [TOOL] R-TOOL-2 each radical's list follows a reset to
    /// `[last regular]`, its ranges kept.
    #[test]
    fn radicals_list_ideographs_after_last_regular() {
        let fractional = "[radical 1=⼀一:一𪛙丁-丆]\n[radical 2=⼁丨:丨丩]\n[radical end]\n0020; [03 05, 05, 05]\n";
        assert_eq!(radicals(fractional).unwrap(), ["&[last regular]", "<*一𪛙丁-丆", "<*丨丩"]);
    }
}

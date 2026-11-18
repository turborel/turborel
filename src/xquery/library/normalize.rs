//! The functions of Unicode's mappings [XQueryFO §7.4.6 to §7.4.8]:
//! `fn:normalize-unicode`, by the Unicode Normalization Algorithm \[UAX #15
//! §1.3\], and `fn:upper-case` and `fn:lower-case`, by each character's
//! full case mapping, of Unicode 13.0 (`unicode.rs`).
//!
//! \[DESIGN\] RULES.md D106: the normalization forms supported are NFC,
//! NFD, NFKC, NFKD and FULLY-NORMALIZED, and the zero-length string, which
//! normalizes nothing [XQueryFO §7.4.6]. Fully normalized text is defined
//! by [XML 1.1 Appendix B]: include-normalized, which a string, holding no
//! character escape and no include, is when it is in NFC, and with no
//! relevant construct beginning with a composing character. A string has
//! no markup, so it is its one construct: its fully normalized form is its
//! NFC, and one whose NFC begins with a composing character has none to
//! give, err:FOCH0003, as no form of it is supported. A composing
//! character is one of a canonical combining class not zero, or second in
//! the canonical decomposition mapping of a primary composite: of the
//! table's pairs, or a vowel or trailing consonant jamo, second in a
//! Hangul syllable's [Unicode §3.12]. A case mapping is SpecialCasing's
//! unconditional one where it has one, else UnicodeData's simple one, as
//! no language is known;
//! those a context or a language governs are not applied. Hangul
//! syllables are decomposed and composed by the arithmetic of [Unicode
//! §3.12], which \[UAX #15 §14.1.4\] points to, of the jamo \[UAX #15\]'s
//! tables count: 19 leading consonants from U+1100, 21 vowels from
//! U+1161 and 27 trailing consonants from U+11A8.

use super::super::error::{ErrorCode, error};
use super::super::functions::Context;
use super::Builtin;
use super::args::{arg, string, string_or_empty, string_value};
use super::unicode::{Map, Table};
use crate::error::Result;
use crate::xml::Sequence;

/// The functions of [XQueryFO §7.4.6 to §7.4.8].
pub(super) const FUNCTIONS: [Builtin; 3] = [
    Builtin::function("normalize-unicode", 1, 2, normalize_unicode),
    Builtin::function("upper-case", 1, 1, upper_case),
    Builtin::function("lower-case", 1, 1, lower_case),
];

/// [XQueryFO §7.4.6] The name of the fully normalized form.
const FULLY_NORMALIZED: &str = "FULLY-NORMALIZED";

/// The first Hangul syllable, U+AC00, `UnicodeData.txt`'s first of its
/// range of syllables.
const S_BASE: u32 = 0xAC00;
/// The first leading consonant jamo, U+1100 \[UAX #15 §14.1.4\].
const L_BASE: u32 = 0x1100;
/// The first vowel jamo, U+1161.
const V_BASE: u32 = 0x1161;
/// The code point before the first trailing consonant jamo, U+11A8.
const T_BASE: u32 = 0x11A7;
/// How many leading consonants, vowels and trailing consonants there are,
/// a trailing consonant's count with its absence.
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
/// How many syllables of a leading consonant there are.
const N_COUNT: u32 = V_COUNT * T_COUNT;
/// How many syllables there are, to U+D7A3.
const S_COUNT: u32 = L_COUNT * N_COUNT;

/// A normalization form \[UAX #15 §1.2\].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// Canonical decomposition, then composition.
    Nfc,
    /// Canonical decomposition.
    Nfd,
    /// Compatibility decomposition, then canonical composition.
    Nfkc,
    /// Compatibility decomposition.
    Nfkd,
}

/// The syllable `c`'s jamo, if it is a Hangul syllable [Unicode §3.12].
fn hangul_parts(c: char) -> Option<Vec<char>> {
    let index = u32::from(c).checked_sub(S_BASE).filter(|&i| i < S_COUNT)?;
    let l = L_BASE.checked_add(index.checked_div(N_COUNT)?)?;
    let v = V_BASE.checked_add(index.checked_rem(N_COUNT)?.checked_div(T_COUNT)?)?;
    let t = T_BASE.checked_add(index.checked_rem(T_COUNT)?)?;
    let mut parts = vec![char::from_u32(l)?, char::from_u32(v)?];
    if t != T_BASE {
        parts.push(char::from_u32(t)?);
    }
    Some(parts)
}

/// The Hangul syllable `first` and `second` compose to, if they do: a
/// leading consonant and a vowel, or a syllable of no trailing consonant
/// and a trailing one [Unicode §3.12].
fn hangul_composite(first: char, second: char) -> Option<char> {
    let (a, b) = (u32::from(first), u32::from(second));
    if let (Some(l), Some(v)) = (a.checked_sub(L_BASE).filter(|&l| l < L_COUNT), b.checked_sub(V_BASE).filter(|&v| v < V_COUNT)) {
        return char::from_u32(S_BASE.checked_add(l.checked_mul(N_COUNT)?)?.checked_add(v.checked_mul(T_COUNT)?)?);
    }
    let syllable = a.checked_sub(S_BASE).filter(|&s| s < S_COUNT && s.checked_rem(T_COUNT) == Some(0))?;
    let trailing = b.checked_sub(T_BASE).filter(|&t| t > 0 && t < T_COUNT)?;
    char::from_u32(S_BASE.checked_add(syllable)?.checked_add(trailing)?)
}

/// `text` fully decomposed, canonically or for compatibility too, and its
/// non-starters put in canonical order \[UAX #15 §1.3\].
fn decomposed(table: &Table, text: &str, map: Map) -> Vec<char> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        let parts = table.mapped(c, map).unwrap_or_else(|| vec![c]);
        for part in parts {
            out.extend(hangul_parts(part).unwrap_or_else(|| vec![part]));
        }
    }
    let classes: Vec<u32> = out.iter().map(|&c| table.class(c)).collect();
    let mut keyed: Vec<(u32, char)> = classes.into_iter().zip(out).collect();
    // A stable sort of each run of non-starters by class is the Canonical
    // Ordering Algorithm.
    for run in keyed.split_mut(|(class, _)| *class == 0) {
        run.sort_by_key(|(class, _)| *class);
    }
    keyed.into_iter().map(|(_, c)| c).collect()
}

/// `chars`, decomposed and ordered, composed by the Canonical Composition
/// Algorithm \[UAX #15 §1.3\]: each character not blocked from the last
/// starter, by a character between of class zero or of its class or more,
/// replaced with it by their primary composite.
fn composed(table: &Table, chars: Vec<char>) -> Vec<char> {
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut starter: Option<usize> = None;
    let mut last_class = 0;
    for c in chars {
        let class = table.class(c);
        if let Some(at) = starter {
            // What is between the starter and `c` is non-starters in order of
            // class, the last of the greatest, so `c` is blocked unless it
            // follows the starter or that class is less than its own.
            let unblocked = at.saturating_add(1) == out.len() || last_class < class;
            let first = out.get(at).copied().filter(|_| unblocked);
            if let Some(composite) = first.and_then(|f| hangul_composite(f, c).or_else(|| table.composite(f, c))) {
                if let Some(slot) = out.get_mut(at) {
                    *slot = composite;
                }
                continue;
            }
        }
        if class == 0 {
            starter = Some(out.len());
        }
        last_class = class;
        out.push(c);
    }
    out
}

/// `text` in the normalization form `form` \[UAX #15 §1.3\].
///
/// # Errors
///
/// SQLCODE -916 if the table is damaged.
pub fn normalized(text: &str, form: Form) -> Result<String> {
    let table = Table::read()?;
    let map = if matches!(form, Form::Nfkc | Form::Nfkd) { Map::Compatible } else { Map::Canonical };
    let chars = decomposed(&table, text, map);
    let out = if matches!(form, Form::Nfc | Form::Nfkc) { composed(&table, chars) } else { chars };
    Ok(out.into_iter().collect())
}

/// Whether `c` is a composing character [XML 1.1 Appendix B]: of a
/// canonical combining class not zero, or second in the canonical
/// decomposition mapping of a primary composite (D3 of \[UAX #15\]): a
/// pair of the table, or a Hangul syllable's, whose mapping is a leading
/// consonant and a vowel, or a syllable of no trailing consonant and a
/// trailing one [Unicode §3.12].
fn composing(table: &Table, c: char) -> bool {
    let code = u32::from(c);
    let vowel = code.checked_sub(V_BASE).is_some_and(|v| v < V_COUNT);
    let trailing = code.checked_sub(T_BASE).is_some_and(|t| t > 0 && t < T_COUNT);
    table.class(c) != 0 || vowel || trailing || table.composes_second(c)
}

/// `text` in fully normalized form [XQueryFO §7.4.6] [XML 1.1 Appendix B]:
/// its NFC, if that does not begin with a composing character.
///
/// # Errors
///
/// err:FOCH0003 of a string whose NFC begins with a composing character,
/// which has no fully normalized form; SQLCODE -916 if the table is
/// damaged.
fn fully_normalized(text: &str) -> Result<String> {
    let table = Table::read()?;
    let out = normalized(text, Form::Nfc)?;
    if out.chars().next().is_some_and(|first| composing(&table, first)) {
        let why = "a string beginning with a composing character has no fully normalized form [XQueryFO §7.4.6] [XML 1.1 Appendix B]";
        return Err(error(ErrorCode::FOCH0003, why));
    }
    Ok(out)
}

/// [XQueryFO §7.4.6] `fn:normalize-unicode($arg as xs:string?,
/// $normalizationForm as xs:string) as xs:string`: the string in the form
/// the second names, its blanks at either end removed and in upper case,
/// NFC if it is not given; unchanged if it is the zero-length string.
///
/// # Errors
///
/// err:FOCH0003 of a form not supported, and, of FULLY-NORMALIZED, of a
/// string that has no fully normalized form.
fn normalize_unicode(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    let text = string_or_empty(arg(args, 0))?;
    let named = match args.get(1) {
        Some(given) => string(given)?.trim_matches(' ').to_ascii_uppercase(),
        None => "NFC".to_owned(),
    };
    let form = match named.as_str() {
        "" => return string_value(text),
        "NFC" => Form::Nfc,
        "NFD" => Form::Nfd,
        "NFKC" => Form::Nfkc,
        "NFKD" => Form::Nfkd,
        FULLY_NORMALIZED => return string_value(fully_normalized(&text)?),
        other => return Err(error(ErrorCode::FOCH0003, &format!("the normalization form '{other}' is not supported [XQueryFO §7.4.6]"))),
    };
    string_value(normalized(&text, form)?)
}

/// `text` with each character mapped as `map` maps it, or kept.
fn cased(text: &str, map: Map) -> Result<Sequence> {
    let table = Table::read()?;
    string_value(text.chars().flat_map(|c| table.mapped(c, map).unwrap_or_else(|| vec![c])).collect())
}

/// [XQueryFO §7.4.7] `fn:upper-case($arg as xs:string?) as xs:string`:
/// each character's upper case, which may be longer; the zero-length
/// string of the empty sequence.
fn upper_case(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    cased(&string_or_empty(arg(args, 0))?, Map::Upper)
}

/// [XQueryFO §7.4.8] `fn:lower-case($arg as xs:string?) as xs:string`:
/// each character's lower case.
fn lower_case(_: &Context<'_>, args: &[Sequence]) -> Result<Sequence> {
    cased(&string_or_empty(arg(args, 0))?, Map::Lower)
}

#[cfg(test)]
mod tests;

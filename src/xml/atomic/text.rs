//! The literals of the types derived from `xs:token` and of the names
//! [Schema2 §3.3.3 to §3.3.11, §3.2.18, §3.2.19]: a language identifier,
//! an Nmtoken, a Name, an NCName and a QName.
//!
//! \[DESIGN\] RULES.md D86: names are XML 1.1's [XML 1.1 §2.3], the
//! names of XQuery's evaluations under the lexical rules of XML 1.1
//! [ISO/IEC 9075-14:2006 draft §6.6 GR3d vi, GR4j], and those `\i` and
//! `\c` match in a pattern (`regex/class.rs`). Each production is matched
//! as written, not by compiling the pattern [Schema2 Appendix A] that
//! names it, as it is matched for each value cast.

use super::super::chars::{COLON, Version, is_name, is_ncname, is_nmtoken};
use super::types::Production;

/// [Schema2 Appendix A] The most letters of a subtag of a language
/// identifier, and of its first after `x-`.
const SUBTAG: usize = 8;
/// [Schema2 Appendix A] The letters of an ISO 639 code, the first form of
/// a language identifier's first subtag.
const ISO_639: usize = 2;

/// Whether `s` is of `least` to `most` ASCII letters.
fn letters(s: &str, least: usize, most: usize) -> bool {
    (least..=most).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphabetic())
}

/// [Schema2 §3.3.3, Appendix A] Whether `text` matches language's
/// pattern, `([a-zA-Z]{2}|[iI]-[a-zA-Z]+|[xX]-[a-zA-Z]{1,8})(-[a-zA-Z]{1,8})*`.
fn language(text: &str) -> bool {
    let mut tags = text.split('-');
    let first = tags.next().unwrap_or_default();
    let rest: Vec<&str> = tags.collect();
    let (head, tail) = match first {
        "i" | "I" => (rest.first().is_some_and(|t| letters(t, 1, usize::MAX)), rest.get(1..)),
        "x" | "X" => (rest.first().is_some_and(|t| letters(t, 1, SUBTAG)), rest.get(1..)),
        _ => (letters(first, ISO_639, ISO_639), rest.get(..)),
    };
    head && tail.is_some_and(|t| t.iter().all(|tag| letters(tag, 1, SUBTAG)))
}

/// Whether `text` matches `production` [Schema2 §3.3.3, §3.3.4, §3.3.6,
/// §3.3.7]: a language identifier, an Nmtoken, a Name or an NCName.
#[must_use]
pub fn matches(text: &str, production: Production) -> bool {
    match production {
        Production::None => true,
        Production::Language => language(text),
        Production::NmToken => is_nmtoken(Version::V11, text),
        Production::Name => is_name(Version::V11, text),
        Production::NcName => is_ncname(Version::V11, text),
    }
}

/// [Schema2 §3.2.18] [Namespaces 1.1 §3 \[7\]] Whether `text` is a QName:
/// an NCName, or two joined by a colon.
#[must_use]
pub fn is_qname(text: &str) -> bool {
    let (prefix, local) = text.split_once(COLON).map_or((None, text), |(p, l)| (Some(p), l));
    prefix.is_none_or(|p| is_ncname(Version::V11, p)) && is_ncname(Version::V11, local)
}

//! [ISO/IEC 9075-2:2023 §8.5 GR3] LIKE under a collation whose characters
//! do not compare one at a time, as UNICODE's contract and expand. Each run
//! of characters the pattern gives matches a substring of one or more
//! characters equal to it under the collation, with NO PAD [GR3d ii 4].
//!
//! The pattern's parts are matched in turn, each from every place in the
//! value the parts before it can end at, so no part is tried twice from
//! one place. A run tries the substrings from a place until they hold more
//! characters with a first-level weight than a string equal to it can, as
//! each element of a key is made of at most `MOST_CONTRACTED` characters.

use super::{Spec, specifiers};
use crate::error::Result;
use crate::value::collation::{CollationRef, MOST_CONTRACTED, Pad};

/// A substring specifier [GR3c].
enum Part {
    /// An arbitrary character specifier, `_`.
    One,
    /// An arbitrary string specifier, `%`.
    Any,
    /// A sequence of single character specifiers: its sort key.
    Run(Vec<u8>),
}

/// The substring specifiers of `specs` [GR3c], a run keyed by `collation`.
fn parts(specs: &[Spec], collation: &CollationRef) -> Vec<Part> {
    let mut out = Vec::new();
    let mut run = String::new();
    let key = |text: &str| {
        let mut key = Vec::new();
        collation.collation.sort_key(text, Pad::None, &mut key);
        Part::Run(key)
    };
    for spec in specs {
        let part = match spec {
            Spec::Character(c) => {
                run.push(*c);
                continue;
            }
            Spec::AnyCharacter => Part::One,
            Spec::AnyString => Part::Any,
        };
        if !run.is_empty() {
            out.push(key(&run));
            run.clear();
        }
        out.push(part);
    }
    if !run.is_empty() {
        out.push(key(&run));
    }
    out
}

/// [ISO/IEC 9075-2:2023 §8.5 GR3d] Whether `value` matches `pattern` under
/// `collation`, `escape` its escape character if one was given.
///
/// # Errors
///
/// As `like::like`.
pub fn collated(value: &str, pattern: &str, escape: Option<char>, collation: &CollationRef) -> Result<bool> {
    let parts = parts(&specifiers(pattern, escape)?, collation);
    let text: Vec<char> = value.chars().collect();
    // Where the parts matched so far can end.
    let mut reached = vec![false; text.len().saturating_add(1)];
    if let Some(start) = reached.first_mut() {
        *start = true;
    }
    for part in &parts {
        let mut next = vec![false; reached.len()];
        for from in reached.iter().enumerate().filter(|(_, r)| **r).map(|(at, _)| at) {
            match part {
                Part::One => mark(&mut next, from.saturating_add(1)),
                Part::Any => {
                    next.iter_mut().skip(from).for_each(|n| *n = true);
                    break;
                }
                Part::Run(key) => ends(&text, from, key, collation).into_iter().for_each(|end| mark(&mut next, end)),
            }
        }
        reached = next;
    }
    Ok(reached.last().copied().unwrap_or(false))
}

fn mark(reached: &mut [bool], at: usize) {
    if let Some(r) = reached.get_mut(at) {
        *r = true;
    }
}

/// Where a substring of `text` from `from`, of one or more characters,
/// whose key under `collation` is `key`, ends.
fn ends(text: &[char], from: usize, key: &[u8], collation: &CollationRef) -> Vec<usize> {
    let most = key.len().saturating_mul(MOST_CONTRACTED).saturating_add(1);
    let mut out = Vec::new();
    let mut substring = String::new();
    let mut weighing = 0_usize;
    let mut found = Vec::with_capacity(key.len());
    for (end, c) in text.iter().enumerate().skip(from) {
        if collation.collation.weighs(*c) {
            weighing = weighing.saturating_add(1);
            if weighing > most {
                break;
            }
        }
        substring.push(*c);
        found.clear();
        collation.collation.sort_key(&substring, Pad::None, &mut found);
        if found == key {
            out.push(end.saturating_add(1));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::collation::builtin;

    fn under(name: &str) -> CollationRef {
        CollationRef { collation: builtin().into_iter().find(|c| c.name() == name).unwrap(), pad: Pad::None }
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.5 GR3d] under UNICODE a
    /// run of the pattern matches a substring equal to it, of any length,
    /// `_` one character and `%` any; canonically equivalent text matches,
    /// but a different case or accent does not; no padding is added.
    #[test]
    fn a_run_matches_a_substring_equal_to_it_8_5() {
        let unicode = under("UNICODE");
        let m = |v: &str, p: &str| collated(v, p, None, &unicode).unwrap();
        assert!(m("re\u{301}sume\u{301}", "résumé"));
        assert!(m("résumé", "r_sum_"));
        assert!(m("résumé", "r%"));
        assert!(m("résumé", "%sum%"));
        assert!(!m("résumé", "resume"));
        assert!(!m("Résumé", "résumé"));
        assert!(!m("ab ", "ab"));
        assert!(m("", ""));
        assert!(!m("a", ""));
        assert!(m("a", "%"));
        assert!(!m("", "_"));
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.5 GR3d ii 1, 4] under a
    /// tailoring each substring is keyed alone. Traditional Spanish `ch` is
    /// a letter, but the value may be split inside it, so `c_ico` matches
    /// `chico`; one character matches a run of two it equals; a run of
    /// characters a collation ignores is matched however long.
    #[test]
    fn substrings_are_keyed_alone_8_5() {
        let spanish = under("UNICODE_ES_TRADITIONAL");
        let m = |v: &str, p: &str| collated(v, p, None, &spanish).unwrap();
        assert!(m("chico", "ch%"));
        assert!(m("chico", "c_ico"));
        assert!(m("chico", "c%"));
        assert!(!m("chico", "cx%"));
        assert!(m("é", "e\u{301}"));
        // Under Japanese, of three levels, punctuation is ignorable, so a
        // run of it is found however long.
        let japanese = under("UNICODE_JA");
        let dashes = ["a", &"-".repeat(100), "b"].concat();
        assert!(collated(&dashes, "ab", None, &japanese).unwrap());
        assert!(collated(&dashes, "a_b", None, &japanese).unwrap());
        assert!(collated("a_b", "a!_b", Some('!'), &spanish).unwrap());
        assert!(collated("a", "a!", Some('!'), &spanish).is_err());
    }
}

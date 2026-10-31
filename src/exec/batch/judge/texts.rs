//! \[DESIGN\] RULES.md D31: a conjunct judged over a vector of character
//! strings as their text (`storage::Texts`), no value made of each: LIKE
//! a constant pattern of each text in place [X3.135-1986 §5.14]; a
//! comparison with a constant string, IN and NOT IN a list of them, each
//! padded with spaces as strings compare [§5.11 GR5]; IS NULL [§5.15]; of
//! a store's dictionaries, each once of each entry, a row then kept by its
//! entry. A
//! constant not a string is judged of the values instead. Split from
//! judge.rs (RULES.md R-SIZE-2).

use std::cmp::Ordering;

use super::Judged;
use super::ints::holds;
use crate::storage::Texts;
use crate::value::{Value, compare_strings};

/// The rows of `kept` that `judged` keeps of `texts`, if it is judged of
/// text.
pub(super) fn kept(judged: &Judged, texts: &Texts, kept: &[usize]) -> Option<Vec<usize>> {
    let keep = |f: &dyn Fn(&str) -> bool| -> Vec<usize> {
        // Of a store's dictionaries, judged once of each entry, then of
        // each row by its entry.
        if let Some(coded) = &texts.coded {
            let entries: Vec<bool> = (0..coded.entries.len()).map(|e| coded.entries.get(e).is_some_and(f)).collect();
            let of = |n: usize| coded.codes.get(n).and_then(|&c| usize::try_from(c).ok()).and_then(|c| entries.get(c).copied());
            return kept.iter().copied().filter(|&n| !texts.is_null(n) && of(n).unwrap_or(false)).collect();
        }
        kept.iter().copied().filter(|&n| texts.get(n).is_some_and(f)).collect()
    };
    match judged {
        // [X3.135-1986 §5.15 GR2-GR4] never unknown.
        Judged::IsNull(_, not) => Some(kept.iter().copied().filter(|&n| texts.get(n).is_none() != *not).collect()),
        // [X3.135-1986 §5.14 GR4, GR5] unknown of a null.
        Judged::Like(_, pattern) => Some(pattern.as_ref().map_or_else(Vec::new, |pattern| keep(&|s| pattern.matches(s)))),
        Judged::Compare(_, op, Value::Char(constant)) => Some(keep(&|s| holds(*op, Some(compare_strings(s, constant))))),
        // [X3.135-1986 §5.13 GR2, §5.18] an OR of equalities; NOT of it.
        Judged::In(_, set, not) => {
            // A long list of strings by its set, each looked for unpadded.
            if set.hashed() {
                if !set.of_strings() {
                    return None;
                }
                let want = super::wanted(*not);
                return Some(keep(&|s| set.text_truth(s) == Some(want)));
            }
            let list = set.values();
            let strings: Vec<&str> = list
                .iter()
                .filter(|v| !v.is_null())
                .map(|v| if let Value::Char(s) = v { Some(s.as_str()) } else { None })
                .collect::<Option<_>>()?;
            let has_null = list.iter().any(Value::is_null);
            let equal = |s: &str| strings.iter().any(|c| compare_strings(s, c) == Ordering::Equal);
            Some(if *not { keep(&|s| !has_null && !equal(s)) } else { keep(&equal) })
        }
        Judged::Compare(..) | Judged::Compared(..) => None,
    }
}

//! \[DESIGN\] RULES.md D31: a conjunct judged over a vector of character
//! strings as their text (`storage::Texts`), no value made of each: LIKE a
//! constant pattern of each text in place [X3.135-1986 §5.14], or of all
//! at once by its piece between two `%`s (RULES.md D1119); a
//! comparison with a constant string, IN and NOT IN a list of them, each
//! padded with spaces as strings compare [§5.11 GR5]; IS NULL [§5.15]; of
//! a store's dictionaries, each once of each entry, a row then kept by its
//! entry. A constant not a string is judged of the values instead.

use std::cmp::Ordering;

use super::Judged;
use super::ints::holds;
use crate::kernels::Kernels;
use crate::storage::mask::index;
use crate::storage::sel::rows_of;
use crate::storage::{Mask, Texts};
use crate::value::{LikePattern, Value, compare_strings};

/// The rows of `over`, or of all, that `judged` keeps of `texts`, if it is
/// judged of text: of a store's dictionaries, each entry judged once into
/// a mask of the entries kept, the rows then by the kernel `select_codes`.
pub(super) fn kept(judged: &Judged, texts: &Texts, over: Option<&[u32]>, kernels: &dyn Kernels) -> Option<Vec<u32>> {
    let keep = |f: &dyn Fn(&str) -> bool| -> Vec<u32> {
        let mut out = Vec::new();
        if let Some(coded) = &texts.coded {
            let mut entries = Mask::none(coded.entries.len());
            for e in 0..coded.entries.len() {
                if coded.entries.get(e).is_some_and(f) {
                    entries.set(e);
                }
            }
            kernels.select_codes(&coded.codes, entries.words(), (texts.nulls.as_ref(), over), &mut out);
            return out;
        }
        let rows = over.map_or_else(|| rows_of(0..texts.len()), <[u32]>::to_vec);
        rows.into_iter().filter(|&n| texts.get(index(n)).is_some_and(f)).collect()
    };
    match judged {
        // [X3.135-1986 §5.15 GR2-GR4] never unknown.
        Judged::IsNull(_, not) => Some(super::kinds::nulls_kept(texts.nulls.as_ref(), (texts.len(), over), *not, kernels)),
        // [X3.135-1986 §5.14 GR4, GR5] unknown of a null.
        // NOT of it unknown of a null too (RULES.md D1122).
        Judged::Like(_, pattern, not) => Some(pattern.as_ref().map_or_else(Vec::new, |pattern| {
            arena(pattern, texts, over, kernels).map_or_else(
                || keep(&|s| pattern.matches(s) != *not),
                |matched| if *not { unmatched(&matched, texts, over) } else { matched },
            )
        })),
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
        Judged::Compare(..) | Judged::Truth(..) | Judged::LikeOf(..) | Judged::Any(_) => None,
    }
}

/// The rows of `over`, or of all, of `texts` that match `pattern`, the
/// batch's text searched at once by `kernels` (`value/like/arena.rs`,
/// RULES.md D1119): none of a store's dictionaries, whose entries are
/// each matched once, of rows `over` keeps fewer than half of, or of a
/// pattern of no piece between two `%`s.
fn arena(pattern: &LikePattern, texts: &Texts, over: Option<&[u32]>, kernels: &dyn Kernels) -> Option<Vec<u32>> {
    if texts.coded.is_some() || over.is_some_and(|rows| rows.len().saturating_mul(2) < texts.len()) {
        return None;
    }
    let all = pattern.matches_all((texts.text.as_bytes(), &texts.ends), kernels)?;
    let kept = |n: &u32| all.get(index(*n)).copied().unwrap_or(false);
    Some(match over {
        Some(rows) => rows.iter().copied().filter(kept).collect(),
        None => rows_of(0..all.len()).into_iter().filter(kept).collect(),
    })
}

/// The rows of `over`, or of all, of `texts`, not null and not among
/// `matched`, ascending as they are: those NOT LIKE keeps (RULES.md D1122).
fn unmatched(matched: &[u32], texts: &Texts, over: Option<&[u32]>) -> Vec<u32> {
    let rows = over.map_or_else(|| rows_of(0..texts.len()), <[u32]>::to_vec);
    let mut taken = matched.iter().peekable();
    rows.into_iter()
        .filter(|&n| {
            while taken.next_if(|&&m| m < n).is_some() {}
            taken.peek() != Some(&&n) && texts.get(index(n)).is_some()
        })
        .collect()
}

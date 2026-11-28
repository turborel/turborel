//! \[DESIGN\] (RULES.md D512) A conjunct of a column of texts judged of a
//! chunk's dictionary whose entries are in order as strings compare
//! (`encoded/dictionary.rs`), by its codes alone. Between the places
//! where the entries equal to each constant of the conjunct begin and
//! end, found by binary search, every entry orders alike against every
//! constant, so the conjunct is true of all of them or of none: it is
//! judged of one entry of each span, and a row is kept if its code is in
//! a span kept. A comparison, a range, a LIKE prefix (as its bounds), IN
//! and NOT IN judge a few entries, not all of them.

use std::cmp::Ordering;

use super::super::encoded::Body;
use super::super::packed::{dense, get, unpack_words};
use super::super::{Chunk, WORD, read_word};
use super::{Pred, text_keeps};
use crate::index::Probe;
use crate::storage::mask::index;
use crate::storage::{Condition, Conjunct};
use crate::value::{Value, compare_strings};

/// Pushes to `out` the rows of `these`, places from `from`, that `pred`
/// keeps of `chunk`, whose first row is `first`, by the codes of its
/// dictionary in order: none, nothing pushed, if it is not one, or an
/// entry or a code is malformed.
pub(super) fn judged(chunk: &Chunk<'_>, pred: &Pred, (first, from): (u32, u32), these: &[u32], out: &mut Vec<u32>) -> Option<()> {
    let Body::Dictionary { bits, sorted: true, offsets, entries, codes } = &chunk.body else { return None };
    let count = offsets.len().checked_div(WORD)?.checked_sub(1)?;
    let entry = |e: usize| {
        let (at, end) =
            (read_word(offsets, e.saturating_mul(WORD)).ok()?, read_word(offsets, e.saturating_add(1).saturating_mul(WORD)).ok()?);
        std::str::from_utf8(entries.get(at..end)?).ok()
    };
    let spans = kept_codes(pred, count, &entry)?;
    let at = |r: u32| index(from.saturating_add(r).saturating_sub(first));
    let (Some(&low), Some(&high)) = (these.first(), these.last()) else { return Some(()) };
    let (low, high) = (at(low), at(high));
    // Codes dense in their span unpacked at once, else each read alone.
    let mut held = Vec::new();
    let unpacked = dense(these.len(), high.checked_sub(low)?.saturating_add(1));
    if unpacked {
        unpack_words(codes, (*bits, 0), (low, high.checked_add(1)?), &mut held)?;
    }
    let mut kept = Vec::with_capacity(these.len());
    for &r in these {
        let n = at(r);
        if chunk.is_null(n) {
            if pred.of_null {
                kept.push(r);
            }
            continue;
        }
        let code =
            if unpacked { usize::try_from(*held.get(n.checked_sub(low)?)?).ok()? } else { usize::try_from(get(codes, *bits, n)?).ok()? };
        if spans.iter().any(|span| span.contains(&code)) {
            kept.push(r);
        }
    }
    out.extend(kept);
    Some(())
}

/// The spans of codes of the entries `pred` keeps of a dictionary in
/// order of `count` entries, each as `entry` gives it, adjacent spans
/// joined: none if an entry is malformed, or the conjunct is not judged
/// of text (`text_keeps`) or by constants.
fn kept_codes<'e>(pred: &Pred, count: usize, entry: &dyn Fn(usize) -> Option<&'e str>) -> Option<Vec<std::ops::Range<usize>>> {
    let mut constants = Vec::new();
    constants_of(&pred.conjunct, &mut constants)?;
    let mut cuts = vec![0, count];
    for c in constants {
        let order = |e: usize| entry(e).map(|s| compare_strings(s, c));
        cuts.push(first_where(count, &|e| Some(order(e)? != Ordering::Less))?);
        cuts.push(first_where(count, &|e| Some(order(e)? == Ordering::Greater))?);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
    for pair in cuts.windows(2) {
        let (&[from, to], Some(text)) = (pair, pair.first().and_then(|&e| entry(e))) else { return None };
        if !text_keeps(&pred.conjunct, &|c| Some(super::text_order(text, c)))? {
            continue;
        }
        if let Some(last) = spans.last_mut().filter(|last| last.end == from) {
            last.end = to;
        } else {
            spans.push(from..to);
        }
    }
    Some(spans)
}

/// The first of `0..count` `holds` is true of, it being false of those
/// before and true of those after, or `count`: none if it is none of one.
fn first_where(count: usize, holds: &dyn Fn(usize) -> Option<bool>) -> Option<usize> {
    let (mut low, mut high) = (0_usize, count);
    while low < high {
        let mid = low.saturating_add(high.saturating_sub(low).checked_div(2).unwrap_or(0));
        if holds(mid)? {
            high = mid;
        } else {
            low = mid.saturating_add(1);
        }
    }
    Some(low)
}

/// The strings `conjunct` compares a text with, pushed to `out`: none,
/// of a bound a sort raises as it runs, as no constant holds it, and of a
/// constant compared unpadded, which a dictionary in the padded order and
/// a sketch of it place no code by (RULES.md D1183), and of a LIKE, which
/// no order against a constant judges.
pub(super) fn constants_of<'c>(conjunct: &'c Conjunct, out: &mut Vec<&'c str>) -> Option<()> {
    let strings = |values: &'c [Value], into: &mut Vec<&'c str>| {
        into.extend(values.iter().filter_map(|v| if let Value::Char(c) = v { Some(c.as_str()) } else { None }));
    };
    match conjunct {
        Conjunct::Condition(Condition { probe: Probe::Compare { value: Value::Char(c), .. }, .. }) => out.push(c),
        Conjunct::In { values, .. } | Conjunct::NotIn { values, .. } => strings(values, out),
        Conjunct::Any(each) => return each.iter().try_for_each(|c| constants_of(c, out)),
        Conjunct::Condition(_) | Conjunct::Null { .. } | Conjunct::Spans { .. } => {}
        Conjunct::Dynamic { .. } | Conjunct::Among { .. } | Conjunct::Unpadded { .. } | Conjunct::Like { .. } => return None,
    }
    Some(())
}

// Unit tests: dictionaries written in order and judged by their codes
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

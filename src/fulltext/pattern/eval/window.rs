//! Proximity and context over the places leaves were found
//! [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §6.8.2, §6.18.2].
//!
//! - Proximity follows §6.8.2, the normative clause, over the informal
//!   window of §4.2.5.1. Each place has a position in the unit: the word's
//!   ordinal for WORDS, its sentence's for SENTENCES, its paragraph's for
//!   PARAGRAPHS and its first character's offset for CHARACTERS
//!   [§5.1.5 Description 4b]. The distance from a match to one not before
//!   it is the second's position less the first's, less the first's
//!   `corrVal` [§5.1.5 Description 4c], which for CHARACTERS is
//!   implementation-defined: here the first's length, so the distance is
//!   the characters between the two words. IN ORDER takes the matches of
//!   the first list as the first; ANY ORDER either. So WITHIN 0 WORDS is
//!   the same word, WITHIN 1 WORDS the next, and WITHIN 0 SENTENCES the
//!   same sentence. A word may match both lists.
//! - A phrase a thesaurus term expands to is at the place of its first word
//!   and, for the distance after it, its last.
//! - Context holds where some one sentence or paragraph holds a match of
//!   every argument, each match wholly inside it [§6.18.2 Description 2b].

use std::collections::BTreeSet;

use crate::fulltext::pattern::Unit;
use crate::fulltext::pattern::eval::leaf::{Place, Span};

/// The position of `place` in `unit`.
fn position(place: Place, unit: Unit) -> i64 {
    i64::from(match unit {
        Unit::Characters => place.offset,
        Unit::Words => place.ordinal,
        Unit::Sentences => place.sentence,
        Unit::Paragraphs => place.paragraph,
    })
}

/// The position in `unit` a distance after `span` is counted from: its
/// last token's, plus that token's length for CHARACTERS.
fn end(span: Span, unit: Unit) -> i64 {
    let last = position(span.last, unit);
    match unit {
        Unit::Characters => last.saturating_add(i64::from(span.last.length)),
        Unit::Words | Unit::Sentences | Unit::Paragraphs => last,
    }
}

/// Whether a match of `left` and one of `right` are within `distance`
/// units, the one of `left` first if `in_order`.
pub(super) fn near(left: &[Span], right: &[Span], distance: u64, unit: Unit, in_order: bool) -> bool {
    let distance = i64::try_from(distance).unwrap_or(i64::MAX);
    ordered(left, right, distance, unit) || (!in_order && ordered(right, left, distance, unit))
}

/// Whether a match of `second` not before one of `first` is within
/// `distance` units after it. For each of `first`, the nearest of
/// `second` not before it is the one to try.
fn ordered(first: &[Span], second: &[Span], distance: i64, unit: Unit) -> bool {
    let mut starts: Vec<i64> = second.iter().map(|s| position(s.first, unit)).collect();
    starts.sort_unstable();
    first.iter().any(|a| {
        let from = position(a.first, unit);
        let nearest = starts.partition_point(|s| *s < from);
        starts.get(nearest).is_some_and(|s| s.saturating_sub(end(*a, unit)) <= distance)
    })
}

/// Whether some one `unit` holds a match of each of `arguments`.
pub(super) fn context(arguments: &[Vec<Span>], unit: Unit) -> bool {
    let mut common: Option<BTreeSet<i64>> = None;
    for spans in arguments {
        let segments: BTreeSet<i64> = spans
            .iter()
            .filter_map(|s| {
                let segment = position(s.first, unit);
                (segment == position(s.last, unit)).then_some(segment)
            })
            .collect();
        common = Some(match common {
            None => segments,
            Some(common) => common.intersection(&segments).copied().collect(),
        });
    }
    common.is_some_and(|c| !c.is_empty())
}

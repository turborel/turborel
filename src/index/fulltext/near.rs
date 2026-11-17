//! \[DESIGN\] A plan answered as a stream of document numbers (RULES.md
//! D86, D34): each set of terms a union of their cursors, `&` an
//! intersection, `|` a union, every document the number entries
//! (`cursor.rs`); and the places of a phrase, a proximity or a context
//! condition checked in each document their words meet in [ISO/IEC
//! 13249-2:2002 (CD, SC32 N0640) §5.3.1 Description 5e to 5g].
//!
//! The checks keep a superset of the documents Contains finds, so they
//! ask only what any reading of the words' places must give.
//!
//! * A phrase's words are consecutive tokens [§6.6.2]: each word's ordinal
//!   follows the one before's by the tokens between, stop words counted,
//!   a stop word or a word not posted one, an optional word one or none.
//! * A proximity's two words are within its distance [§6.8.2]: their
//!   ordinals, sentence or paragraph numbers apart by at most the
//!   distance; in CHARACTERS, counted from a word's end, whose length the
//!   postings do not keep, only their order; the first list's first if IN
//!   ORDER.
//! * A context condition's words are in one sentence or one paragraph,
//!   by their numbers.

use std::sync::Arc;

use super::FullTextIndex;
use super::block::Places;
use super::cursor::{Cursor, Docs, Every, Meet, Nothing, Terms, Union};
use super::keys::overflow_key;
use super::plan::{Check, Near, Plan};
use super::postings::{Position, positions_in};
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::fulltext::pattern::Unit;

/// The stream of the documents `plan` finds in `index`.
pub(super) fn stream(index: &Arc<FullTextIndex>, plan: Plan) -> Box<dyn Docs> {
    match plan {
        Plan::All => Box::new(Every::new(index)),
        Plan::Nothing => Box::new(Nothing),
        Plan::Terms(mut terms) => {
            if terms.len() == 1
                && let Some(term) = terms.pop()
            {
                return Box::new(Cursor::new(index, term));
            }
            Box::new(Terms::new(index, terms))
        }
        Plan::And(plans) => Box::new(Meet::new(plans.into_iter().map(|part| stream(index, part)).collect())),
        Plan::Or(plans) => Box::new(Union::new(plans.into_iter().map(|part| stream(index, part)).collect())),
        Plan::Near(near) => Box::new(NearDocs::new(index, near)),
    }
}

/// The documents holding a term of each slot whose places pass a check.
struct NearDocs {
    index: Arc<FullTextIndex>,
    check: Check,
    slots: Vec<Terms>,
    rest: Vec<Box<dyn Docs>>,
}

impl NearDocs {
    /// The stream of `near` in `index`.
    fn new(index: &Arc<FullTextIndex>, near: Near) -> NearDocs {
        let slots = near.slots.into_iter().map(|terms| Terms::new(index, terms)).collect();
        let rest = near.rest.into_iter().map(|plan| stream(index, plan)).collect();
        NearDocs { index: Arc::clone(index), check: near.check, slots, rest }
    }

    /// The first number at or past `docno` every slot and plan holds.
    fn meet(&mut self, docno: u64) -> Result<Option<u64>> {
        let mut target = docno;
        'round: loop {
            let streams = self.slots.iter_mut().map(|slot| -> &mut dyn Docs { slot }).chain(self.rest.iter_mut().map(|rest| &mut **rest));
            for stream in streams {
                let Some(found) = stream.advance_to(target)? else { return Ok(None) };
                if found > target {
                    target = found;
                    continue 'round;
                }
            }
            return Ok(Some(target));
        }
    }

    /// Each slot's places in document `docno`, which every slot holds.
    fn places(&self, docno: u64) -> Result<Vec<Vec<Position>>> {
        let mut out = Vec::with_capacity(self.slots.len());
        for slot in &self.slots {
            let mut places = Vec::new();
            for cursor in slot.at(docno) {
                places.extend(self.index.places(cursor.term(), docno)?);
            }
            places.sort_unstable_by_key(|p| (p.ordinal, p.offset));
            out.push(places);
        }
        Ok(out)
    }
}

impl Docs for NearDocs {
    fn advance_to(&mut self, docno: u64) -> Result<Option<u64>> {
        let mut target = docno;
        loop {
            let Some(found) = self.meet(target)? else { return Ok(None) };
            if passes(&self.check, &self.places(found)?) {
                return Ok(Some(found));
            }
            let Some(next) = found.checked_add(1) else { return Ok(None) };
            target = next;
        }
    }
}

/// Whether the places of each slot pass `check`.
fn passes(check: &Check, slots: &[Vec<Position>]) -> bool {
    match check {
        Check::Phrase(gaps) => phrase(slots, gaps),
        Check::Within { unit, distance, in_order } => match slots {
            [left, right] => within(left, right, (*unit, *distance), *in_order),
            _ => true,
        },
        Check::Same(unit) => same(slots, *unit),
    }
}

/// Whether a place of each slot follows one of the slot before by one of
/// its gap's ordinals: the ordinals reached, slot by slot.
fn phrase(slots: &[Vec<Position>], gaps: &[(u32, u32)]) -> bool {
    let Some((first, others)) = slots.split_first() else { return true };
    let mut reached: Vec<u32> = first.iter().map(|p| p.ordinal).collect();
    for (places, (least, most)) in others.iter().zip(gaps) {
        reached.sort_unstable();
        reached = places
            .iter()
            .map(|p| p.ordinal)
            .filter(|ordinal| {
                let from = reached.partition_point(|r| r.saturating_add(*most) < *ordinal);
                reached.get(from).is_some_and(|r| r.saturating_add(*least) <= *ordinal)
            })
            .collect();
        if reached.is_empty() {
            return false;
        }
    }
    true
}

/// A place's position in `unit`, and the most a later one may be past it
/// within `distance`: any for CHARACTERS, which counts from the word's
/// end, as a word's length is not kept.
fn measure(position: &Position, (unit, distance): (Unit, u64)) -> (u64, u64) {
    match unit {
        Unit::Characters => (u64::from(position.offset), u64::MAX),
        Unit::Words => (u64::from(position.ordinal), distance),
        Unit::Sentences => (u64::from(position.sentence), distance),
        Unit::Paragraphs => (u64::from(position.paragraph), distance),
    }
}

/// Whether a place of `left` and one of `right` are within `distance` of
/// `unit`, the left one not after the right if `in_order`.
fn within(left: &[Position], right: &[Position], unit: (Unit, u64), in_order: bool) -> bool {
    let mut rights: Vec<u64> = right.iter().map(|p| measure(p, unit).0).collect();
    rights.sort_unstable();
    left.iter().any(|place| {
        let (at, most) = measure(place, unit);
        let from = if in_order { at } else { at.saturating_sub(most) };
        let first = rights.partition_point(|r| *r < from);
        rights.get(first).is_some_and(|r| *r <= at.saturating_add(most))
    })
}

/// Whether some sentence, or paragraph, holds a place of every slot.
fn same(slots: &[Vec<Position>], unit: Unit) -> bool {
    let number = |p: &Position| match unit {
        Unit::Paragraphs => p.paragraph,
        Unit::Sentences | Unit::Words | Unit::Characters => p.sentence,
    };
    let Some((first, others)) = slots.split_first() else { return true };
    first.iter().map(number).any(|n| others.iter().all(|places| places.iter().any(|p| number(p) == n)))
}

impl FullTextIndex {
    /// The places of `term` in document `docno`, in order; none if it
    /// does not hold it.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if a posting is malformed.
    pub(super) fn places(&self, term: &[u8], docno: u64) -> Result<Vec<Position>> {
        let Some(posting) = self.posting(term, docno)? else { return Ok(Vec::new()) };
        match posting.places {
            Places::Inline(bytes) => positions_in(&bytes),
            Places::Overflow => {
                let (lower, upper) = (overflow_key(term, docno, 0), overflow_key(term, docno, u16::MAX));
                let mut places = Vec::new();
                for entry in self.tree.scan(std::ops::Bound::Included(&lower), std::ops::Bound::Included(&upper), Direction::Forward) {
                    places.extend(positions_in(&entry?.1)?);
                }
                Ok(places)
            }
        }
    }
}

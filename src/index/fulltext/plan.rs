//! \[DESIGN\] A search pattern answered by a full-text index (RULES.md D86,
//! D34) [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.3.1]: the pattern made
//! a plan of the documents that may hold it, a superset of those that do,
//! as the executor rechecks each row Contains finds.
//!
//! * A word reads its term's postings; a word with wild cards, the terms
//!   of the dictionary (the `S` entries) its mask matches, those beginning
//!   with its literal prefix; FORM OF, the terms of its stem (`W`); each
//!   set of terms a union of their postings.
//! * `&` intersects; `|` unites; NOT is every document, so under `&` the
//!   other side alone.
//! * A phrase, a proximity and a context condition intersect their words'
//!   postings and check their places (`near.rs`).
//! * IS ABOUT unites its words that are not stop words.
//! * SOUNDS LIKE and FUZZY FORM OF unite the dictionary's terms alike.
//! * What the index cannot list, a thesaurus's terms (which live in tables
//!   the statement reads), a stop word it may not have posted, is every
//!   document.

use std::ops::Bound;

use super::FullTextIndex;
use super::document::term_of;
use super::keys::{STATISTIC, TermStatistics, statistic_key, stem_key};
use super::matcher::{Glob, Matcher, expansion_matchers, word_matchers};
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::fulltext::pattern::{Argument, Context, Expansion, Literal, Part, Pattern, Proximity, Unit};
use crate::fulltext::text::StopWords;
use crate::fulltext::{Languages, is_stop};

/// The documents that may hold a pattern.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Plan {
    /// Every document.
    All,
    /// No document.
    Nothing,
    /// Those holding any of these terms.
    Terms(Vec<Vec<u8>>),
    /// Those every plan finds.
    And(Vec<Plan>),
    /// Those any plan finds.
    Or(Vec<Plan>),
    /// Those holding a term of each slot whose places pass a check, and
    /// that the other plans find.
    Near(Near),
}

/// Slots of terms whose places are checked.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Near {
    /// The check.
    pub check: Check,
    /// Each slot's terms.
    pub slots: Vec<Vec<Vec<u8>>>,
    /// Plans found too, whose places are not checked.
    pub rest: Vec<Plan>,
}

/// What places of a near plan's slots must be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Check {
    /// A phrase's words: each slot's ordinal after the one before's by
    /// one of a range, the tokens between counted [§6.6.2].
    Phrase(Vec<(u32, u32)>),
    /// The two slots within a distance.
    Within {
        /// Its unit.
        unit: Unit,
        /// Its distance.
        distance: u64,
        /// Whether the first slot's place comes first.
        in_order: bool,
    },
    /// The slots in one sentence or one paragraph.
    Same(Unit),
}

/// The plan of `terms`, none for every document.
fn of_terms(terms: Option<Vec<Vec<u8>>>) -> Plan {
    let Some(terms) = terms else { return Plan::All };
    if terms.is_empty() { Plan::Nothing } else { Plan::Terms(terms) }
}

/// The plan of documents every plan of `plans` finds, simplified.
fn and(plans: Vec<Plan>) -> Plan {
    let mut kept = Vec::new();
    for plan in plans {
        match plan {
            Plan::All => {}
            Plan::Nothing => return Plan::Nothing,
            Plan::And(inner) => kept.extend(inner),
            other @ (Plan::Terms(_) | Plan::Or(_) | Plan::Near(_)) => kept.push(other),
        }
    }
    match kept.len() {
        0 => Plan::All,
        1 => kept.pop().unwrap_or(Plan::All),
        _ => Plan::And(kept),
    }
}

/// The plan of documents any plan of `plans` finds, simplified.
pub(super) fn or(plans: Vec<Plan>) -> Plan {
    let mut kept = Vec::new();
    for plan in plans {
        match plan {
            Plan::All => return Plan::All,
            Plan::Nothing => {}
            Plan::Or(inner) => kept.extend(inner),
            other @ (Plan::Terms(_) | Plan::And(_) | Plan::Near(_)) => kept.push(other),
        }
    }
    match kept.len() {
        0 => Plan::Nothing,
        1 => kept.pop().unwrap_or(Plan::Nothing),
        _ => Plan::Or(kept),
    }
}

impl FullTextIndex {
    /// The plan of `pattern`.
    ///
    /// # Errors
    ///
    /// The tree's.
    pub(super) fn plan(&self, pattern: &Pattern) -> Result<Plan> {
        Ok(match pattern {
            Pattern::Or(left, right) => or(vec![self.plan(left)?, self.plan(right)?]),
            Pattern::And(left, right) => and(vec![self.plan(left)?, self.plan(right)?]),
            Pattern::Not(_) => Plan::All,
            Pattern::Literal(literal) => self.literal_plan(literal)?,
            Pattern::Near(near) => self.near_plan(near)?,
            Pattern::About(literal) => self.about_plan(literal)?,
            Pattern::Expansion(expansion) => self.expansion_plan(expansion)?.1,
            Pattern::Context(context) => self.context_plan(context)?,
        })
    }

    /// The terms of an expansion and its plan [§6.9.5, §6.10.5]: of SOUNDS
    /// LIKE or FUZZY FORM OF, the dictionary's terms alike, as Contains
    /// matches them; nothing of a stop word; every document of a
    /// thesaurus, expanded before it is matched (RULES.md D87).
    fn expansion_plan(&self, expansion: &Expansion) -> Result<(Option<Vec<Vec<u8>>>, Plan)> {
        if let Expansion::Thesaurus(_) = expansion {
            return Ok((None, Plan::All));
        }
        let Some(matcher) = expansion_matchers(expansion).pop() else { return Ok((None, Plan::Nothing)) };
        let terms = self.terms(&matcher)?;
        Ok((terms.clone(), of_terms(terms)))
    }

    /// The plan of a word or a phrase: each word's, its places checked
    /// for a phrase of words of known terms. A word alone that is a stop
    /// word of its language matches nothing [§6.4.2 Description 2a].
    fn literal_plan(&self, literal: &Literal) -> Result<Plan> {
        let mut slots = Vec::new();
        let mut gaps: Vec<(u32, u32)> = Vec::new();
        // The ordinals from the last slot to the next, least and most.
        let mut gap: Option<(u32, u32)> = None;
        let mut rest = Vec::new();
        let words = literal.parts.iter().filter(|part| matches!(part, Part::Word(_))).count();
        let widen =
            |from: Option<(u32, u32)>, (least, most): (u32, u32)| from.map(|(l, m)| (l.saturating_add(least), m.saturating_add(most)));
        for part in &literal.parts {
            let Part::Word(mask) = part else {
                gap = widen(gap, (0, 1));
                continue;
            };
            let stop = mask.text().is_some_and(|text| is_stop(literal.language, &text));
            if stop && words == 1 {
                return Ok(Plan::Nothing);
            }
            let Some(terms) = self.slot(literal, mask, stop, &mut rest)? else {
                gap = widen(gap, (1, 1));
                continue;
            };
            if let Some((least, most)) = gap {
                gaps.push((least.saturating_add(1), most.saturating_add(1)));
            }
            slots.push(terms);
            gap = Some((0, 0));
        }
        Ok(near(Check::Phrase(gaps), slots, rest))
    }

    /// The terms of a phrase's word `mask` if its place can be checked:
    /// none for a stop word, which a stop word of the document matches, or
    /// a word whose terms are not known, whose plan goes to `rest`.
    fn slot(
        &self,
        literal: &Literal,
        mask: &crate::fulltext::pattern::Mask,
        stop: bool,
        rest: &mut Vec<Plan>,
    ) -> Result<Option<Vec<Vec<u8>>>> {
        if stop {
            return Ok(None);
        }
        let matchers = word_matchers(literal, mask);
        if let [matcher] = matchers.as_slice()
            && let Some(terms) = self.terms(matcher)?
        {
            return Ok(Some(terms));
        }
        for matcher in &matchers {
            rest.push(of_terms(self.terms(matcher)?));
        }
        Ok(None)
    }

    /// The plan of IS ABOUT: any of its words not stop words of its
    /// language, none if every one is [§6.17.2 Description 2].
    fn about_plan(&self, literal: &Literal) -> Result<Plan> {
        let mut plans = Vec::new();
        for part in &literal.parts {
            let Part::Word(mask) = part else { continue };
            if mask.text().is_some_and(|text| is_stop(literal.language, &text)) {
                continue;
            }
            for matcher in word_matchers(literal, mask) {
                plans.push(of_terms(self.terms(&matcher)?));
            }
        }
        Ok(or(plans))
    }

    /// The terms an argument of a proximity or context condition may
    /// match, none if they are not all known; and its plan.
    fn argument(&self, argument: &Argument) -> Result<(Option<Vec<Vec<u8>>>, Plan)> {
        let literals: Vec<&Literal> = match argument {
            Argument::Literal(literal) => vec![literal],
            Argument::List(literals) => literals.iter().collect(),
            Argument::Expansion(expansion) => return self.expansion_plan(expansion),
        };
        let mut all = Some(Vec::new());
        let mut plans = Vec::new();
        for literal in literals {
            plans.push(self.literal_plan(literal)?);
            let one = match literal.parts.as_slice() {
                [Part::Word(mask)] => match word_matchers(literal, mask).as_slice() {
                    [matcher] => self.terms(matcher)?,
                    _ => None,
                },
                _ => None,
            };
            all = all.zip(one).map(|(mut all, one)| {
                all.extend(one);
                all
            });
        }
        Ok((all, or(plans)))
    }

    /// The plan of a proximity: both sides', their places checked if
    /// their terms are known.
    fn near_plan(&self, proximity: &Proximity) -> Result<Plan> {
        let (left, left_plan) = self.argument(&proximity.left)?;
        let (right, right_plan) = self.argument(&proximity.right)?;
        let Some((left, right)) = left.zip(right) else { return Ok(and(vec![left_plan, right_plan])) };
        let check = Check::Within { unit: proximity.unit, distance: proximity.distance, in_order: proximity.in_order };
        Ok(near(check, vec![left, right], Vec::new()))
    }

    /// The plan of a context condition: its arguments', those of known
    /// terms checked in one sentence or paragraph.
    fn context_plan(&self, context: &Context) -> Result<Plan> {
        let mut slots = Vec::new();
        let mut rest = Vec::new();
        for argument in &context.arguments {
            match self.argument(argument)? {
                (Some(terms), _) => slots.push(terms),
                (None, plan) => rest.push(plan),
            }
        }
        Ok(near(Check::Same(context.unit), slots, rest))
    }

    /// The terms `matcher` matches, in order; none if they cannot all be
    /// listed, as some may not be posted.
    ///
    /// # Errors
    ///
    /// The tree's.
    pub(super) fn terms(&self, matcher: &Matcher) -> Result<Option<Vec<Vec<u8>>>> {
        if self.unposted(matcher) || *matcher == Matcher::Unknown {
            return Ok(None);
        }
        self.listed(matcher).map(Some)
    }

    /// The terms posted that `matcher` matches, in order: those Rank
    /// counts.
    ///
    /// # Errors
    ///
    /// The tree's.
    pub(super) fn listed(&self, matcher: &Matcher) -> Result<Vec<Vec<u8>>> {
        match matcher {
            Matcher::Exact(word) => Ok(vec![term_of(word, self.most_term).to_vec()]),
            Matcher::Mask(mask) => self.dictionary(matcher, &prefix_of(mask)),
            // A document's word is stemmed in its language: by the stem
            // entries, stemmed in the index's, if every document is in it,
            // else from the dictionary.
            Matcher::Stem(stemmed) => {
                if self.languages() != Languages::of(self.reading.language) {
                    return self.dictionary(matcher, "");
                }
                let mut terms = self.stemmed(stemmed)?;
                terms.push(term_of(stemmed, self.most_term).to_vec());
                terms.sort_unstable();
                terms.dedup();
                Ok(terms)
            }
            Matcher::Sounds { .. } | Matcher::Spelled { .. } => self.dictionary(matcher, ""),
            Matcher::Unknown => Ok(Vec::new()),
        }
    }

    /// Whether a word `matcher` matches may be one the index does not
    /// post while Contains finds it: a word of the index's own stop words
    /// [§4.4.2], or for a word itself, a stop word of any language.
    fn unposted(&self, matcher: &Matcher) -> bool {
        if let Matcher::Exact(word) = matcher
            && self.reading.may_stop(word, self.languages())
        {
            return true;
        }
        match &self.reading.stop {
            StopWords::Words(words) => words.iter().any(|word| matcher.matches(word, self.languages())),
            StopWords::Language | StopWords::None => false,
        }
    }

    /// The terms of the dictionary beginning with `prefix` that `matcher`
    /// matches; a term cut short, if it may.
    fn dictionary(&self, matcher: &Matcher, prefix: &str) -> Result<Vec<Vec<u8>>> {
        let prefix = term_of(prefix, self.most_term);
        let lower = [&[STATISTIC][..], prefix].concat();
        let upper = [&[STATISTIC][..], prefix, &[u8::MAX][..]].concat();
        let mut out = Vec::new();
        let languages = self.languages();
        for entry in self.tree.scan(Bound::Included(&lower), Bound::Excluded(&upper), Direction::Forward) {
            let (key, _) = entry?;
            let term = key.get(1..key.len().saturating_sub(1)).unwrap_or_default();
            let word = String::from_utf8_lossy(term);
            let cut = term.len().saturating_add(CUT_SLACK) > self.most_term;
            if matcher.matches(&word, languages) || (cut && matcher.matches_prefix(&word)) {
                out.push(term.to_vec());
            }
        }
        Ok(out)
    }

    /// The terms of the stem `stemmed`, from the stem entries.
    fn stemmed(&self, stemmed: &str) -> Result<Vec<Vec<u8>>> {
        let stem = term_of(stemmed, self.most_term);
        let lower = stem_key(stem, b"");
        let lower = lower.get(..lower.len().saturating_sub(1)).unwrap_or_default().to_vec();
        let mut upper = lower.clone();
        if let Some(last) = upper.last_mut() {
            *last = 1;
        }
        let mut out = Vec::new();
        for entry in self.tree.scan(Bound::Included(&lower), Bound::Excluded(&upper), Direction::Forward) {
            let (key, _) = entry?;
            let term = key.get(lower.len()..key.len().saturating_sub(1)).unwrap_or_default();
            out.push(term.to_vec());
        }
        Ok(out)
    }

    /// The documents `plan` may find, for `estimate`: a term's from its
    /// statistics, the fewest of an intersection, the sum of a union.
    ///
    /// # Errors
    ///
    /// The tree's.
    pub(super) fn plan_estimate(&self, plan: &Plan, every: u64) -> Result<u64> {
        Ok(match plan {
            Plan::All => every,
            Plan::Nothing => 0,
            Plan::Terms(terms) => {
                let mut sum = 0_u64;
                for term in terms {
                    let found = self.tree.get(&statistic_key(term))?;
                    sum = sum.saturating_add(found.map_or(Ok(TermStatistics::default()), |b| TermStatistics::decode(&b))?.documents);
                }
                sum.min(every)
            }
            Plan::And(plans) => {
                let mut fewest = every;
                for part in plans {
                    fewest = fewest.min(self.plan_estimate(part, every)?);
                }
                fewest
            }
            Plan::Or(plans) => {
                let mut sum = 0_u64;
                for part in plans {
                    sum = sum.saturating_add(self.plan_estimate(part, every)?);
                }
                sum.min(every)
            }
            Plan::Near(near) => {
                let mut fewest = every;
                for slot in &near.slots {
                    fewest = fewest.min(self.plan_estimate(&Plan::Terms(slot.clone()), every)?);
                }
                for part in &near.rest {
                    fewest = fewest.min(self.plan_estimate(part, every)?);
                }
                fewest
            }
        })
    }
}

/// \[DESIGN\] The bytes short of the most a term may have that make it
/// one cut short: a character's most.
const CUT_SLACK: usize = 4;

/// The plan of `slots` checked by `check`, with `rest`: a slot of no term
/// finds nothing; a check of fewer than two slots is no check.
fn near(check: Check, slots: Vec<Vec<Vec<u8>>>, rest: Vec<Plan>) -> Plan {
    if slots.iter().any(Vec::is_empty) {
        return Plan::Nothing;
    }
    if slots.len() < 2 {
        let mut plans: Vec<Plan> = slots.into_iter().map(Plan::Terms).collect();
        plans.extend(rest);
        return and(plans);
    }
    let rest = match and(rest) {
        Plan::All => Vec::new(),
        Plan::And(plans) => plans,
        other @ (Plan::Nothing | Plan::Terms(_) | Plan::Or(_) | Plan::Near(_)) => vec![other],
    };
    Plan::Near(Near { check, slots, rest })
}

/// The literal characters a mask begins with, before its first wild
/// card.
fn prefix_of(mask: &[Glob]) -> String {
    let mut out = String::new();
    for glob in mask {
        let Glob::Char(c) = glob else { break };
        out.push(*c);
    }
    out
}

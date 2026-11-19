//! The leaves of a pattern being evaluated: each word, phrase and
//! expansion, matched against a document's tokens as they are read
//! [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §6.4.2, §6.5.2, §6.6.2, §6.7.2,
//! §6.9.2, §6.10.2, §6.17.2]. A leaf keeps whether it was found and, when a
//! proximity or a context needs them, the places it was found at; nothing
//! else of the document.
//!
//! - A word matches a token by LIKE over normalized characters, the
//!   pattern's `_` and `%` its wild cards [§6.4.2 Description 2]; a FORM OF
//!   word matches by stems, the token's stemmed in the document's language
//!   and the pattern's in its own [§6.5.2 Description 2]. So a literal of
//!   another language than the document's matches by the same comparison,
//!   only its stop words and stems being its language's.
//! - Stop words. Every token of the document is kept with its place, stop
//!   words too, and a stop word compares equal to any stop word and to
//!   nothing else [§5.1.5 DR6, §6.6.2 Description 3]. A word of a phrase
//!   that is a stop word of the phrase's language so matches any stop
//!   word of the document [§4.4.2]. A word by itself that is a stop word
//!   matches nothing [§6.4.2 Description 2a i: R is False], and no other
//!   word matches a stop word of the document.
//! - A phrase matches consecutive tokens, an optional word one token or
//!   none [§6.6.2 Description 5e], across sentence ends.

use crate::fulltext::pattern::fuzzy::{allowed, within};
use crate::fulltext::pattern::literal::is_stop_word;
use crate::fulltext::pattern::soundex::soundex;
use crate::fulltext::pattern::{Literal, MaskChar, Part};
use crate::fulltext::{Language, Token, is_single_letter, stem};

/// Where a token is, in every unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Place {
    /// Its ordinal among the document's words, from 1.
    pub ordinal: u32,
    /// Its sentence, from 1.
    pub sentence: u32,
    /// Its paragraph, from 1.
    pub paragraph: u32,
    /// Its first character's offset.
    pub offset: u32,
    /// Its characters.
    pub length: u32,
}

impl Place {
    /// The place of `token`.
    fn of(token: &Token) -> Place {
        Place { ordinal: token.ordinal, sentence: token.sentence, paragraph: token.paragraph, offset: token.offset, length: token.length }
    }
}

/// Where a leaf was found: its first token and its last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Span {
    /// The first token.
    pub first: Place,
    /// The last token.
    pub last: Place,
}

/// A step of a word or phrase.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Step {
    /// An optional word: one token, any, or none.
    Optional,
    /// A stop word of a phrase: a stop word of the document.
    Stop,
    /// A word: a token, not a stop word, by LIKE.
    Word {
        /// The word, or its stem if `stemmed`.
        mask: Vec<MaskChar>,
        /// Whether it matches the token's stem.
        stemmed: bool,
    },
}

/// What a leaf matches.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Matcher {
    /// Nothing: a stop word by itself.
    Never,
    /// Consecutive tokens matching its steps.
    Sequence(Vec<Step>),
    /// A token matching any of these words: IS ABOUT.
    AnyWord(Vec<Vec<MaskChar>>),
    /// A token that sounds like the word: equal to it, or of its Soundex
    /// code if it has one.
    Sounds {
        /// The word.
        word: String,
        /// Its code.
        code: Option<String>,
    },
    /// A token spelled like the word.
    Spelled {
        /// The word.
        word: Vec<char>,
        /// The edits allowed.
        edits: usize,
    },
}

impl Matcher {
    /// The matcher of the word or phrase `literal`.
    pub(super) fn literal(literal: &Literal) -> Matcher {
        if literal.parts.is_empty() || stop_word(literal) {
            return Matcher::Never;
        }
        let steps = literal
            .parts
            .iter()
            .map(|part| match part {
                Part::Optional => Step::Optional,
                Part::Word(mask) => {
                    if is_stop_word(mask, literal.language) {
                        Step::Stop
                    } else {
                        Step::Word { mask: stemmed(&mask.0, literal), stemmed: literal.stemmed }
                    }
                }
            })
            .collect();
        Matcher::Sequence(steps)
    }

    /// The matcher of IS ABOUT `literal` \[implementation-defined, §6.17.2
    /// Description 2\]: any word of it not a stop word of its language.
    pub(super) fn about(literal: &Literal) -> Matcher {
        let words: Vec<Vec<MaskChar>> = literal
            .parts
            .iter()
            .filter_map(|part| match part {
                Part::Word(mask) => (!is_stop_word(mask, literal.language)).then(|| mask.0.clone()),
                Part::Optional => None,
            })
            .collect();
        if words.is_empty() { Matcher::Never } else { Matcher::AnyWord(words) }
    }

    /// The matcher of SOUNDS LIKE `literal` [§6.9.5].
    pub(super) fn sounds(literal: &Literal) -> Matcher {
        let word = only_word(literal);
        if stop_word(literal) {
            return Matcher::Never;
        }
        let code = soundex(&word);
        Matcher::Sounds { word, code }
    }

    /// The matcher of FUZZY FORM OF `literal` [§6.10.5].
    pub(super) fn spelled(literal: &Literal) -> Matcher {
        let word: Vec<char> = only_word(literal).chars().collect();
        if stop_word(literal) {
            return Matcher::Never;
        }
        let edits = allowed(&word);
        Matcher::Spelled { word, edits }
    }

    /// Whether it needs the stems of the document's tokens.
    pub(super) fn stems(&self) -> bool {
        match self {
            Matcher::Sequence(steps) => steps.iter().any(|s| matches!(s, Step::Word { stemmed: true, .. })),
            Matcher::Never | Matcher::AnyWord(_) | Matcher::Sounds { .. } | Matcher::Spelled { .. } => false,
        }
    }
}

/// Whether `literal` is one word, a stop word of its language.
fn stop_word(literal: &Literal) -> bool {
    match literal.parts.as_slice() {
        [Part::Word(mask)] => is_stop_word(mask, literal.language),
        _ => false,
    }
}

/// The characters of the first word of `literal`, wild cards left out.
fn only_word(literal: &Literal) -> String {
    let mask = literal.parts.iter().find_map(|part| match part {
        Part::Word(mask) => Some(mask),
        Part::Optional => None,
    });
    mask.map(|m| m.0.iter().filter_map(|c| literal_char(*c)).collect()).unwrap_or_default()
}

/// The character `c` is, if no wild card.
fn literal_char(c: MaskChar) -> Option<char> {
    match c {
        MaskChar::Literal(c) => Some(c),
        MaskChar::One | MaskChar::Any => None,
    }
}

/// The word `mask` of `literal` as its steps match it: any word holding
/// it if it is a single letter of a language read as bigrams (RULES.md
/// D323); its stem in the literal's language if the literal is stemmed
/// and the word has no wild card; else as written.
fn stemmed(mask: &[MaskChar], literal: &Literal) -> Vec<MaskChar> {
    let text: Option<String> = mask.iter().map(|c| literal_char(*c)).collect();
    if let Some(letter) = text.as_deref().filter(|text| is_single_letter(literal.language, text)) {
        return [MaskChar::Any].into_iter().chain(letter.chars().map(MaskChar::Literal)).chain([MaskChar::Any]).collect();
    }
    match text.filter(|_| literal.stemmed) {
        Some(text) => stem(literal.language, &text).chars().map(MaskChar::Literal).collect(),
        None => mask.to_vec(),
    }
}

/// A token as the leaves read it.
pub(super) struct Seen<'t> {
    /// The token.
    pub token: &'t Token,
    /// Its characters.
    pub chars: Vec<char>,
    /// The characters of its stem in the document's language, if a leaf
    /// needs them.
    pub stem: Vec<char>,
}

impl<'t> Seen<'t> {
    /// `token` of a document in `language`, its stem taken if `stems`.
    pub(super) fn new(token: &'t Token, language: Language, stems: bool) -> Seen<'t> {
        let stem = if stems { stem(language, &token.word).chars().collect() } else { Vec::new() };
        Seen { token, chars: token.word.chars().collect(), stem }
    }
}

/// A match of a sequence begun: the step it is at and where it began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Thread {
    /// The index of the next step.
    state: usize,
    /// Its first token.
    first: Place,
}

/// A leaf: its matcher and what it found.
#[derive(Debug)]
pub(super) struct Leaf {
    /// What it matches.
    matcher: Matcher,
    /// Whether to keep its spans, for a proximity or a context.
    keep: bool,
    /// Whether it was found.
    pub found: bool,
    /// Where it was found, if `keep`.
    pub spans: Vec<Span>,
    /// The matches of a sequence begun and not yet done: at most one for
    /// each step and each of the last tokens as many as its steps.
    threads: Vec<Thread>,
}

impl Leaf {
    /// A leaf of `matcher`, keeping its spans if `keep`.
    pub(super) fn new(matcher: Matcher, keep: bool) -> Leaf {
        Leaf { matcher, keep, found: false, spans: Vec::new(), threads: Vec::new() }
    }

    /// Whether it needs the stems of the document's tokens.
    pub(super) fn stems(&self) -> bool {
        self.matcher.stems()
    }

    /// Reads the token `seen`.
    pub(super) fn step(&mut self, seen: &Seen<'_>) {
        if self.found && !self.keep {
            return;
        }
        let place = Place::of(seen.token);
        let found = match &self.matcher {
            Matcher::Never => false,
            Matcher::Sequence(steps) => {
                let ends = advance(steps, &mut self.threads, seen, place);
                for first in ends {
                    self.found_at(Span { first, last: place });
                }
                return;
            }
            Matcher::AnyWord(words) => !seen.token.stop && words.iter().any(|w| like(&seen.chars, w)),
            Matcher::Sounds { word, code } => {
                let alike = *word == seen.token.word || code.as_ref().is_some_and(|c| soundex(&seen.token.word).as_ref() == Some(c));
                !seen.token.stop && alike
            }
            Matcher::Spelled { word, edits } => !seen.token.stop && within(&seen.chars, word, *edits),
        };
        if found {
            self.found_at(Span { first: place, last: place });
        }
    }

    /// Notes it found at `span`.
    fn found_at(&mut self, span: Span) {
        self.found = true;
        if self.keep && self.spans.last() != Some(&span) {
            self.spans.push(span);
        }
    }
}

/// Moves the matches `threads` of `steps` past the token `seen` at
/// `place`, a new one begun at it; the first places of those it ends.
fn advance(steps: &[Step], threads: &mut Vec<Thread>, seen: &Seen<'_>, place: Place) -> Vec<Place> {
    let mut current = std::mem::take(threads);
    current.push(Thread { state: 0, first: place });
    let mut ends = Vec::new();
    for thread in current {
        for state in closure(steps, thread.state) {
            let Some(step) = steps.get(state) else { continue };
            if !step_matches(step, seen) {
                continue;
            }
            let next = state.saturating_add(1);
            if closure(steps, next).any(|s| s == steps.len()) && !ends.contains(&thread.first) {
                ends.push(thread.first);
            }
            let moved = Thread { state: next, first: thread.first };
            if next < steps.len() && !threads.contains(&moved) {
                threads.push(moved);
            }
        }
    }
    ends
}

/// The states reached from `state` without reading a token: it and those
/// after the optional words that follow it.
fn closure(steps: &[Step], state: usize) -> impl Iterator<Item = usize> + '_ {
    let optional = steps.iter().skip(state).take_while(|s| **s == Step::Optional).count();
    state..=state.saturating_add(optional)
}

/// Whether `step` matches the token `seen`.
fn step_matches(step: &Step, seen: &Seen<'_>) -> bool {
    match step {
        Step::Optional => true,
        Step::Stop => seen.token.stop,
        Step::Word { mask, stemmed } => !seen.token.stop && like(if *stemmed { &seen.stem } else { &seen.chars }, mask),
    }
}

/// Whether `text` is LIKE `mask`: `_` any one character, `%` any run of
/// characters, none too [§6.4.4].
pub(super) fn like(text: &[char], mask: &[MaskChar]) -> bool {
    let (mut t, mut m) = (0_usize, 0_usize);
    // The last `%` read: the mask's index after it, and the text's index it
    // stands at, which a mismatch moves on by one.
    let mut star: Option<(usize, usize)> = None;
    while let Some(c) = text.get(t) {
        let moved = match mask.get(m) {
            Some(MaskChar::Any) => {
                star = Some((m.saturating_add(1), t));
                m = m.saturating_add(1);
                continue;
            }
            Some(MaskChar::One) => true,
            Some(MaskChar::Literal(x)) => x == c,
            None => false,
        };
        if moved {
            m = m.saturating_add(1);
            t = t.saturating_add(1);
        } else if let Some((after, at)) = star {
            let at = at.saturating_add(1);
            star = Some((after, at));
            m = after;
            t = at;
        } else {
            return false;
        }
    }
    mask.iter().skip(m).all(|c| *c == MaskChar::Any)
}

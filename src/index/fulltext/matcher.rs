//! \[DESIGN\] What a word of a search pattern matches [ISO/IEC 13249-2:2002
//! (CD, SC32 N0640) §5.3.1 Description 5d]: a normalized word, a word
//! with wild cards, or the words of a stem (RULES.md D86). A full-text
//! index finds the terms a word matches from them (`plan.rs`), and Rank
//! counts the tokens they match (`crate::fulltext::rank`), alike, so an
//! index's ranking bounds what Rank gives.
//!
//! A mask's literal characters are normalized as a document's words are
//! [§4.5]; `_` matches one character, `%` any, as LIKE [§6.4.4]. A FORM OF
//! word matches a word whose stem in its document's language is its stem, as a
//! document's word is stemmed in the document's language [§6.5.2]. A
//! pattern's word that the tokenizer reads as several words matches as
//! each of them.

use crate::fulltext::pattern::fuzzy::{allowed, within};
use crate::fulltext::pattern::soundex::soundex;
use crate::fulltext::pattern::{Argument, Expansion, Literal, Mask, MaskChar, Part, Pattern};
use crate::fulltext::{Language, Languages, is_single_letter, is_stop, normalize_in, stem, tokenize};

/// A character of a normalized mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Glob {
    /// A character to match.
    Char(char),
    /// `_`: one character, as written.
    One,
    /// `%`: any characters.
    Any,
}

/// What one word of a pattern matches, in a normalized word.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Matcher {
    /// The word itself.
    Exact(String),
    /// Words the mask matches.
    Mask(Vec<Glob>),
    /// Words whose stem in their document's language is the stem, the
    /// word's in its own.
    Stem(String),
    /// SOUNDS LIKE a word [§6.9.5]: words equal to it, or of its Soundex
    /// code if it has one, as Contains matches them.
    Sounds {
        /// The word.
        word: String,
        /// Its code.
        code: Option<String>,
    },
    /// FUZZY FORM OF a word [§6.10.5]: words within its edits of it.
    Spelled {
        /// The word.
        word: Vec<char>,
        /// The edits allowed.
        edits: usize,
    },
    /// Words nothing here can list, which the index answers by every
    /// document and Rank does not count.
    Unknown,
}

impl Matcher {
    /// Whether it matches the normalized word `word` of a document in one
    /// of `languages`.
    pub(crate) fn matches(&self, word: &str, languages: Languages) -> bool {
        match self {
            Matcher::Exact(exact) => exact == word,
            Matcher::Mask(mask) => glob(mask, &word.chars().collect::<Vec<_>>(), false),
            Matcher::Stem(stemmed) => languages.iter().any(|language| stem(language, word) == *stemmed),
            Matcher::Sounds { word: sounded, code } => {
                sounded == word || code.as_ref().is_some_and(|code| soundex(word).as_ref() == Some(code))
            }
            Matcher::Spelled { word: spelled, edits } => within(&word.chars().collect::<Vec<_>>(), spelled, *edits),
            Matcher::Unknown => false,
        }
    }

    /// Whether it may match a word beginning with the normalized `word`,
    /// as a term cut short keeps only a word's first bytes.
    pub(crate) fn matches_prefix(&self, word: &str) -> bool {
        match self {
            Matcher::Exact(exact) => exact.starts_with(word),
            Matcher::Mask(mask) => glob(mask, &word.chars().collect::<Vec<_>>(), true),
            Matcher::Stem(..) | Matcher::Sounds { .. } | Matcher::Spelled { .. } | Matcher::Unknown => true,
        }
    }
}

/// Whether `mask` matches `word`, or with `prefix` some word beginning
/// with it: whether each place of the mask matches each rest of the word,
/// filled from the ends back.
fn glob(mask: &[Glob], word: &[char], prefix: bool) -> bool {
    // after[w]: the mask past the place at hand matches the word from w.
    let mut after: Vec<bool> = (0..=word.len()).map(|w| w == word.len() || prefix).collect();
    for glob in mask.iter().rev() {
        // At the word's end, only `%` matches, and it matches nothing.
        let end = prefix || (*glob == Glob::Any && after.last().copied().unwrap_or(false));
        let mut here = vec![end; word.len().saturating_add(1)];
        for w in (0..word.len()).rev() {
            let next = after.get(w.saturating_add(1)).copied().unwrap_or(false);
            let matches = match glob {
                Glob::Char(c) => word.get(w) == Some(c) && next,
                Glob::One => next,
                Glob::Any => after.get(w).copied().unwrap_or(false) || here.get(w.saturating_add(1)).copied().unwrap_or(false),
            };
            if let Some(cell) = here.get_mut(w) {
                *cell = matches;
            }
        }
        after = here;
    }
    after.first().copied().unwrap_or(false)
}

/// Whether `c` may stand in a word, as the tokenizer reads one: a mask
/// holding another character is not answered by terms.
fn wordlike(c: char) -> bool {
    c.is_alphanumeric() || c == '\'' || c == '\u{2019}'
}

/// The mask of `mask` normalized in `language`, none if a literal
/// character of it is not one a word holds.
fn normalized(mask: &Mask, language: Language) -> Option<Vec<Glob>> {
    let mut out = Vec::new();
    let mut run = String::new();
    for c in &mask.0 {
        let glob = match c {
            MaskChar::Literal(c) => {
                if !wordlike(*c) {
                    return None;
                }
                run.push(*c);
                continue;
            }
            MaskChar::One => Glob::One,
            MaskChar::Any => Glob::Any,
        };
        out.extend(normalize_in(language, &std::mem::take(&mut run)).chars().map(Glob::Char));
        out.push(glob);
    }
    out.extend(normalize_in(language, &run).chars().map(Glob::Char));
    Some(out)
}

/// The matchers of the word `mask` of `literal`, each of which a document
/// must hold: one for a word the tokenizer reads as one, one for each of
/// several; `Unknown` for a mask it cannot read. A single letter of a
/// language read as bigrams matches each word holding it (RULES.md D323).
pub(crate) fn word_matchers(literal: &Literal, mask: &Mask) -> Vec<Matcher> {
    let Some(text) = mask.text() else {
        return vec![normalized(mask, literal.language).map_or(Matcher::Unknown, Matcher::Mask)];
    };
    let words: Vec<String> = tokenize(text.chars().map(Ok), literal.language).filter_map(|token| token.ok().map(|t| t.word)).collect();
    if words.is_empty() {
        return vec![Matcher::Unknown];
    }
    let each = |word: String| {
        if is_single_letter(literal.language, &word) {
            Matcher::Mask([Glob::Any].into_iter().chain(word.chars().map(Glob::Char)).chain([Glob::Any]).collect())
        } else if literal.stemmed {
            Matcher::Stem(stem(literal.language, &word))
        } else {
            Matcher::Exact(word)
        }
    };
    words.into_iter().map(each).collect()
}

/// The matchers of every word of `literal`, in order.
pub(crate) fn literal_matchers(literal: &Literal) -> Vec<Matcher> {
    let mut out = Vec::new();
    for part in &literal.parts {
        if let Part::Word(mask) = part {
            out.extend(word_matchers(literal, mask));
        }
    }
    out
}

/// The matchers of an expansion: a sound or spelling alike of its word;
/// none of one word that is a stop word of its language, which matches
/// nothing; none of a thesaurus, expanded before it is matched.
pub(super) fn expansion_matchers(expansion: &Expansion) -> Vec<Matcher> {
    let (literal, sounds) = match expansion {
        Expansion::Soundex(literal) => (literal, true),
        Expansion::Fuzzy(literal) => (literal, false),
        Expansion::Thesaurus(_) => return Vec::new(),
    };
    if let [Part::Word(mask)] = literal.parts.as_slice()
        && mask.text().is_some_and(|text| is_stop(literal.language, &text))
    {
        return Vec::new();
    }
    let word: String = literal.parts.iter().find_map(only_mask).map(literal_chars).unwrap_or_default();
    if sounds {
        let code = soundex(&word);
        return vec![Matcher::Sounds { word, code }];
    }
    let word: Vec<char> = word.chars().collect();
    let edits = allowed(&word);
    vec![Matcher::Spelled { word, edits }]
}

/// The mask of a word part.
fn only_mask(part: &Part) -> Option<&Mask> {
    match part {
        Part::Word(mask) => Some(mask),
        Part::Optional => None,
    }
}

/// The characters of `mask` but its wild cards.
fn literal_chars(mask: &Mask) -> String {
    mask.0
        .iter()
        .filter_map(|c| match c {
            MaskChar::Literal(c) => Some(*c),
            MaskChar::One | MaskChar::Any => None,
        })
        .collect()
}

/// The matchers of a context or proximity argument.
fn argument_matchers(argument: &Argument) -> Vec<Matcher> {
    match argument {
        Argument::Literal(literal) => literal_matchers(literal),
        Argument::List(literals) => literals.iter().flat_map(literal_matchers).collect(),
        Argument::Expansion(expansion) => expansion_matchers(expansion),
    }
}

/// The matchers Rank counts of `pattern`: of every word it asks for that
/// is not under NOT, each once [§5.1.3].
pub(crate) fn positive_matchers(pattern: &Pattern) -> Vec<Matcher> {
    let mut out: Vec<Matcher> = Vec::new();
    let mut stack = vec![pattern];
    while let Some(part) = stack.pop() {
        let found = match part {
            Pattern::Or(left, right) | Pattern::And(left, right) => {
                stack.push(right);
                stack.push(left);
                continue;
            }
            Pattern::Not(_) => continue,
            Pattern::Literal(literal) | Pattern::About(literal) => literal_matchers(literal),
            Pattern::Near(near) => argument_matchers(&near.left).into_iter().chain(argument_matchers(&near.right)).collect(),
            Pattern::Expansion(expansion) => expansion_matchers(expansion),
            Pattern::Context(context) => context.arguments.iter().flat_map(argument_matchers).collect(),
        };
        for matcher in found {
            if matcher != Matcher::Unknown && !out.contains(&matcher) {
                out.push(matcher);
            }
        }
    }
    out
}

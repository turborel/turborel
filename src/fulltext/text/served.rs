//! \[EXTENSION\] The languages plug-ins serve (RULES.md D328): a LANGUAGE
//! object of an extension, named as a built-in language is, which cuts
//! text into words, stems words and gives its stop words. A part it does
//! not serve is the built-in language's it is declared LIKE, NONE unless
//! written, so a plug-in may cut Chinese into dictionary words and leave
//! stems and stop words alone.
//!
//! \[DESIGN\] A served language is named in the whole process, as the
//! tokenizer reads text with no database at hand. A name is the first
//! extension's that registers it, while one of its registrations lives,
//! and another database's registration of that extension shares it; a
//! built-in name is never served, as the built-in languages win. At most
//! `MAX_SERVED` names are known in a process, each kept as long as it
//! runs, as a set of languages holds each in a bit.

use std::sync::{Arc, Mutex, Weak};

use super::kind::tables;
use super::segment::Segmenter;
use super::{Language, MAX_WORD_CHARS, StopWords, Token, normalize_in};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::sync::lock_builtin;

/// \[DESIGN\] The served languages a process knows at most: the bits of a
/// set of languages past the built-in ones.
pub const MAX_SERVED: usize = 21;

/// A language a plug-in serves, as the tokenizer calls it: a batch of
/// documents or words a call.
pub trait ServedLanguage: Send + Sync + std::fmt::Debug {
    /// The built-in language whose parts it does not serve.
    fn base(&self) -> Language;
    /// Whether it cuts text into words.
    fn segments(&self) -> bool;
    /// Whether it stems words.
    fn stems(&self) -> bool;
    /// The words of each of `documents`, each the offset of its first
    /// character and its characters, in the order of their offsets.
    ///
    /// # Errors
    ///
    /// The plug-in's, SQLSTATE class 38.
    fn segment(&self, documents: &[&str]) -> Result<Vec<Vec<(u32, u32)>>>;
    /// The stem of each of `words`, each normalized as its words are.
    ///
    /// # Errors
    ///
    /// As `segment`.
    fn stem(&self, words: &[&str]) -> Result<Vec<String>>;
    /// Its stop words, normalized as its words are, sorted, each once;
    /// none if it leaves them to its base.
    fn stop_words(&self) -> Option<&[String]>;
    /// Asks what it serves once, its stop words, as it is registered.
    ///
    /// # Errors
    ///
    /// As `segment`.
    fn prepare(&self) -> Result<()>;
}

/// A name a plug-in serves a language of.
struct Slot {
    name: &'static str,
    /// The extension that serves it.
    owner: String,
    /// Its registrations, one a database.
    served: Vec<Weak<dyn ServedLanguage>>,
}

/// The served languages' names, by slot.
static SLOTS: Mutex<Vec<Slot>> = Mutex::new(Vec::new());

/// The error of a name taken.
fn taken(why: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[EXTENSION] {why}"))
}

/// Registers `served`, the language `name` of extension `owner`, asking
/// its stop words; the slot it is in.
///
/// # Errors
///
/// SQLCODE -910 if the name is a built-in language's, another
/// extension's, or no slot is left; as `ServedLanguage::prepare`.
pub fn register(name: &str, owner: &str, served: &Arc<dyn ServedLanguage>) -> Result<usize> {
    if Language::all().any(|l| l.name().eq_ignore_ascii_case(name)) {
        return Err(taken(&format!("a LANGUAGE {name} is built in, and the built-in language wins")));
    }
    served.prepare()?;
    let mut slots = lock_builtin(&SLOTS);
    for slot in slots.iter_mut() {
        slot.served.retain(|s| s.strong_count() > 0);
    }
    let found = slots.iter().position(|s| s.name.eq_ignore_ascii_case(name));
    let at = if let Some(at) = found {
        at
    } else {
        if slots.len() >= MAX_SERVED {
            return Err(taken(&format!("no LANGUAGE {name}: a process knows at most {MAX_SERVED} served languages")));
        }
        let name = crate::plugin::bridge::leaked(&name.to_ascii_uppercase());
        slots.push(Slot { name, owner: owner.to_owned(), served: Vec::new() });
        slots.len().saturating_sub(1)
    };
    let slot = slots.get_mut(at).ok_or_else(|| taken(&format!("no LANGUAGE {name}")))?;
    if !slot.served.is_empty() && !slot.owner.eq_ignore_ascii_case(owner) {
        return Err(taken(&format!("extension {} serves a LANGUAGE {name} already", slot.owner)));
    }
    owner.clone_into(&mut slot.owner);
    slot.served.push(Arc::downgrade(served));
    Ok(at)
}

/// The slot of the served language named `name`, if one serves it now.
pub(super) fn named(name: &str) -> Option<usize> {
    let slots = lock_builtin(&SLOTS);
    slots.iter().position(|s| s.name.eq_ignore_ascii_case(name) && s.served.iter().any(|w| w.strong_count() > 0))
}

/// The name of slot `slot`.
pub(super) fn name_of(slot: u8) -> &'static str {
    lock_builtin(&SLOTS).get(usize::from(slot)).map_or("NONE", |s| s.name)
}

/// The slots named so far.
pub(super) fn slots() -> usize {
    lock_builtin(&SLOTS).len()
}

/// What serves slot `slot` now, if anything.
pub(super) fn serving(slot: u8) -> Option<Arc<dyn ServedLanguage>> {
    let slots = lock_builtin(&SLOTS);
    slots.get(usize::from(slot))?.served.iter().find_map(Weak::upgrade)
}

/// What serves slot `slot` now.
///
/// # Errors
///
/// SQLSTATE 2FF02 if nothing does, its extension dropped.
pub(super) fn served(slot: u8) -> Result<Arc<dyn ServedLanguage>> {
    serving(slot).ok_or_else(|| {
        SqlError::new(Code::Data, format!("[EXTENSION] no extension serves the LANGUAGE {} now", name_of(slot)))
            .with_state(SqlState::INVALID_LANGUAGE_SPECIFICATION)
    })
}

/// The stems of `words` in the language of slot `slot`: one call.
///
/// # Errors
///
/// As `served` and `ServedLanguage::stem`.
pub(super) fn stems(slot: u8, words: &[&str]) -> Result<Vec<String>> {
    served(slot)?.stem(words)
}

/// That `spans`, a plug-in's words of a document of `chars` characters,
/// each begin where the one before begins or after, and end inside it.
///
/// # Errors
///
/// SQLSTATE 38T03 if one does not.
pub(super) fn checked(spans: &[(u32, u32)], chars: usize) -> Result<()> {
    let mut last = 0_u32;
    for &(offset, length) in spans {
        let end = usize::try_from(offset.saturating_add(length)).unwrap_or(usize::MAX);
        if offset < last || length == 0 || end > chars {
            return Err(crate::plugin::errors::violation(&format!("it gave a word at {offset} of {length} characters")));
        }
        last = offset;
    }
    Ok(())
}

/// The tokens of the document `text` in the served language `language`,
/// marked by the stop words `stop`: its words as one call cuts them, each
/// in the sentence and paragraph of the built-in word it begins in, and
/// their stems asked by one more call, if the plug-in stems, so a
/// document is two calls at most.
///
/// # Errors
///
/// As `served`, `ServedLanguage::segment` and `checked`.
pub(super) fn tokens(text: &str, language: Language, stop: &StopWords) -> Result<Vec<Token>> {
    let Language::Served(slot) = language else { return Ok(Vec::new()) };
    let served = served(slot)?;
    let spans = crate::plugin::bridge::counted(served.segment(&[text])?, 1)?.pop().unwrap_or_default();
    let chars: Vec<char> = text.chars().collect();
    checked(&spans, chars.len())?;
    let places = places(&chars, served.base())?;
    let mut out = Vec::with_capacity(spans.len());
    for (ordinal, (offset, length)) in (1_u32..).zip(spans) {
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let kept = usize::try_from(length.min(MAX_WORD_CHARS)).unwrap_or(0);
        let written: String = chars.iter().skip(start).take(kept).collect();
        let word = normalize_in(language, &written);
        let at = places.partition_point(|p| p.0 <= offset);
        let (sentence, paragraph) = at.checked_sub(1).and_then(|at| places.get(at)).map_or((1, 1), |p| (p.1, p.2));
        let stop = stop.contains(language, &word);
        out.push(Token { word, ordinal, sentence, paragraph, offset, length, stop });
    }
    if served.stems() {
        let mut words: Vec<&str> = out.iter().filter(|t| !t.stop).map(|t| t.word.as_str()).collect();
        words.sort_unstable();
        words.dedup();
        served.stem(&words)?;
    }
    Ok(out)
}

/// The offset, sentence and paragraph of each word the built-in language
/// `base` reads in `chars`.
///
/// # Errors
///
/// As the tokenizer's tables.
fn places(chars: &[char], base: Language) -> Result<Vec<(u32, u32, u32)>> {
    let tables = tables()?;
    let mut segmenter = Segmenter::new(base, StopWords::None);
    let mut out = Vec::new();
    for c in chars {
        out.extend(segmenter.step(tables, *c).map(|t| (t.offset, t.sentence, t.paragraph)));
    }
    out.extend(segmenter.finish(tables).map(|t| (t.offset, t.sentence, t.paragraph)));
    Ok(out)
}

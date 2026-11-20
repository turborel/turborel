//! \[EXTENSION\] A plug-in's language of full-text search (RULES.md D328),
//! a `ServedLanguage` whose calls are batches: a SEGMENT call cuts
//! `batch` documents into words, a STEM call stems `batch` words, and the
//! stop words are asked by one STOP call as the extension is registered.
//! A stem is kept once answered, as a language is deterministic by its
//! contract, so a word asked again makes no call, and so are the words of
//! the last few short documents cut, as an index reads a document twice,
//! for its key's hash and for its postings, and its recheck once more.
//!
//! A part the plug-in does not SERVE is the built-in language's it is
//! LIKE, NONE unless written. A failed call is the statement's error, of
//! class 38, and the process is started again for the next call, as any
//! kind's (D167).

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use crate::error::Result;
use crate::fulltext::text::served::ServedLanguage;
use crate::fulltext::{Language, normalize_in};
use crate::plugin::manifest::{Declared, SERVED_PARTS};
use crate::plugin::payload::language::{put_texts, spans};
use crate::plugin::payload::{LANGUAGE_SEGMENT, LANGUAGE_STEM, LANGUAGE_STOP, strings};
use crate::sync::lock_builtin;

use super::{Link, answer, counted};

/// The documents or words a call carries when the manifest says no BATCH.
pub const DEFAULT_BATCH: usize = 256;

/// \[DESIGN\] The stems kept at most; past it they are forgotten together.
pub const KEPT: usize = 65_536;

/// \[DESIGN\] The documents whose words are kept, the last cut.
pub const RECENT: usize = 32;

/// \[DESIGN\] The bytes of the longest document whose words are kept.
pub const RECENT_BYTES: usize = 16_384;

/// The words of documents cut lately, each with its text.
type Recent = VecDeque<(String, Vec<(u32, u32)>)>;

/// A plug-in's language.
#[derive(Debug)]
pub struct PluginLanguage {
    link: Link,
    base: Language,
    /// Whether it serves each of `SERVED_PARTS`.
    serves: [bool; 3],
    batch: usize,
    /// Its stop words, normalized and sorted, once asked; none if it
    /// leaves them to its base.
    stop: OnceLock<Option<Vec<String>>>,
    kept: Mutex<HashMap<String, String>>,
    recent: Mutex<Recent>,
}

impl PluginLanguage {
    /// The language `d` declares, which `link` names.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF02 if it is LIKE no built-in language.
    pub fn new(d: &Declared, link: Link) -> Result<PluginLanguage> {
        let base = d.like.as_deref().map_or(Ok(Language::None), Language::named)?;
        let serves = SERVED_PARTS.map(|part| d.supports.iter().any(|s| s == part));
        let batch = d.batch.and_then(|b| usize::try_from(b).ok()).unwrap_or(DEFAULT_BATCH).max(1);
        Ok(PluginLanguage { link, base, serves, batch, stop: OnceLock::new(), kept: Mutex::default(), recent: Mutex::default() })
    }

    /// The stems of `words`, none of them kept: a call a batch.
    fn asked(&self, words: &[&str]) -> Result<Vec<String>> {
        let mut all = Vec::with_capacity(words.len());
        for chunk in words.chunks(self.batch) {
            let mut payload = Vec::new();
            put_texts(&mut payload, chunk);
            let got = self.link.plain(LANGUAGE_STEM, payload)?;
            all.extend(counted(answer(&got, strings)?, chunk.len())?);
        }
        Ok(all)
    }
}

impl ServedLanguage for PluginLanguage {
    fn base(&self) -> Language {
        self.base
    }
    fn segments(&self) -> bool {
        self.serves[0]
    }
    fn stems(&self) -> bool {
        self.serves[1]
    }
    fn segment(&self, documents: &[&str]) -> Result<Vec<Vec<(u32, u32)>>> {
        if let [document] = documents
            && let Some((_, found)) = lock_builtin(&self.recent).iter().find(|(text, _)| text == document)
        {
            return Ok(vec![found.clone()]);
        }
        let mut all = Vec::with_capacity(documents.len());
        for chunk in documents.chunks(self.batch) {
            let mut payload = Vec::new();
            put_texts(&mut payload, chunk);
            let got = self.link.plain(LANGUAGE_SEGMENT, payload)?;
            all.extend(counted(answer(&got, spans)?, chunk.len())?);
        }
        let mut recent = lock_builtin(&self.recent);
        for (document, found) in documents.iter().zip(&all).filter(|(d, _)| d.len() <= RECENT_BYTES) {
            if recent.len() >= RECENT {
                recent.pop_front();
            }
            recent.push_back(((*document).to_owned(), found.clone()));
        }
        Ok(all)
    }
    fn stem(&self, words: &[&str]) -> Result<Vec<String>> {
        let mut found: Vec<Option<String>> = {
            let kept = lock_builtin(&self.kept);
            words.iter().map(|w| kept.get(*w).cloned()).collect()
        };
        let missing: Vec<&str> = words.iter().zip(&found).filter(|(_, f)| f.is_none()).map(|(w, _)| *w).collect();
        if !missing.is_empty() {
            let got = self.asked(&missing)?;
            let mut kept = lock_builtin(&self.kept);
            if kept.len().saturating_add(got.len()) > KEPT {
                kept.clear();
            }
            let mut got = missing.iter().zip(got);
            for slot in found.iter_mut().filter(|f| f.is_none()) {
                let Some((word, stem)) = got.next() else { break };
                kept.insert((*word).to_owned(), stem.clone());
                *slot = Some(stem);
            }
        }
        Ok(found.into_iter().zip(words).map(|(f, w)| f.unwrap_or_else(|| (*w).to_owned())).collect())
    }
    fn stop_words(&self) -> Option<&[String]> {
        self.stop.get().and_then(Option::as_deref)
    }
    fn prepare(&self) -> Result<()> {
        if !self.serves[2] || self.stop.get().is_some() {
            return Ok(());
        }
        let got = self.link.plain(LANGUAGE_STOP, Vec::new())?;
        let mut words: Vec<String> = answer(&got, strings)?.iter().map(|w| normalize_in(self.base, w)).collect();
        words.sort_unstable();
        words.dedup();
        // A second registration's answer, were two asked at once, is the
        // same list by the language's contract.
        self.stop.get_or_init(|| Some(words));
        Ok(())
    }
}

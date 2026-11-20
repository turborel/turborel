//! \[DESIGN\] The languages of an index's documents (RULES.md D324). A
//! FullText value names its own language [ISO/IEC 13249-2:2002 (CD, SC32
//! N0640) §5.1.1], so the documents of one index may be in several. A
//! search reads a pattern's words in each language its documents are in,
//! as a word naming no language is of its document's [§5.3.1 Description
//! 3c], and matches a FORM OF word's stem in each.
//!
//! The index keeps an `L` entry for each language of a document it holds
//! besides its own, written with that language's first document and
//! never taken back, so the set read at open is a superset of the
//! languages of the documents it holds. The set is kept in memory too, so
//! a search reads no entry for it.

use std::ops::Bound;
use std::sync::atomic::Ordering;

use super::FullTextIndex;
use super::document::language_of;
use super::keys::{LANGUAGE, language_key};
use crate::engine::tree::{Direction, Update};
use crate::error::{Code, Result, SqlError};
use crate::fulltext::{Language, Languages};
use crate::value::Value;

impl FullTextIndex {
    /// The languages its documents are in: its own, and those of its
    /// FullText values naming another.
    pub(super) fn languages(&self) -> Languages {
        Languages::from_bits(self.languages.load(Ordering::Acquire)).with(self.reading.language)
    }

    /// The language of the document `value` noted, its entry written if
    /// it is a new one.
    ///
    /// # Errors
    ///
    /// As reading the document; the tree's.
    pub(super) fn note_language(&self, value: &Value) -> Result<()> {
        let Some(language) = language_of(value, &self.reading)? else { return Ok(()) };
        if self.languages().contains(language) {
            return Ok(());
        }
        self.tree.update(&language_key(language.name()), |_| Ok(Update::Put(Vec::new())))?;
        self.languages.fetch_or(Languages::of(language).bits(), Ordering::AcqRel);
        Ok(())
    }

    /// The language of the document `value` noted in memory alone, as a
    /// bulk load reads it; `write_languages` writes the entries after.
    ///
    /// # Errors
    ///
    /// As reading the document.
    pub(super) fn note_language_in_memory(&self, value: &Value) -> Result<()> {
        if let Some(language) = language_of(value, &self.reading)? {
            self.languages.fetch_or(Languages::of(language).bits(), Ordering::AcqRel);
        }
        Ok(())
    }

    /// The entries of the languages noted besides its own.
    ///
    /// # Errors
    ///
    /// The tree's.
    pub(super) fn write_languages(&self) -> Result<()> {
        for language in self.languages().iter().filter(|language| *language != self.reading.language) {
            self.tree.update(&language_key(language.name()), |_| Ok(Update::Put(Vec::new())))?;
        }
        Ok(())
    }

    /// The languages of its entries read into memory, as it is opened.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if an entry names no language known.
    pub(super) fn read_languages(&self) -> Result<()> {
        let (lower, upper) = ([LANGUAGE], [LANGUAGE.saturating_add(1)]);
        let mut bits = Languages::default();
        for entry in self.tree.scan(Bound::Included(&lower[..]), Bound::Excluded(&upper[..]), Direction::Forward) {
            let (key, _) = entry?;
            let name = String::from_utf8_lossy(key.get(1..).unwrap_or_default()).into_owned();
            let language = Language::named(&name).map_err(|_| {
                SqlError::new(Code::Corrupt, format!("[DESIGN] a full-text index holds documents in {name}, no language known"))
            })?;
            bits = bits.with(language);
        }
        self.languages.fetch_or(bits.bits(), Ordering::AcqRel);
        Ok(())
    }
}

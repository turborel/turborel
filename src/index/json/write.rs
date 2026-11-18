//! \[DESIGN\] A json index's entries written and taken back (RULES.md
//! D34): a document's count changes in one descent, under its leaf's lock
//! (`BLinkTree::update`); the insert that finds none posts the document's
//! terms and its hash's key, and the remove that takes its last count
//! takes them back. Both run under the row's stripe's lock, so an insert
//! of a row's document never meets the remove of its last count half done,
//! as garbage collection's, which holds no row lock, may be. A term's
//! count changes in one descent too, so writers of one term never lose
//! each other's.

use super::JsonIndex;
use super::document::Document;
use super::keys::{count_key, count_of, count_value, doc, document_key, hashed_key, posting_key, versions_value};
use super::terms::Term;
use crate::engine::tree::Update;
use crate::error::Result;
use crate::index::Key;
use crate::storage::RowId;

impl JsonIndex {
    /// Adds an entry of `row` under `key`: one more version of its
    /// document, posted if it is the first.
    ///
    /// # Errors
    ///
    /// As reading the document; the tree's.
    pub(super) fn add(&self, key: &Key, row: RowId) -> Result<()> {
        let Some(document) = Document::read(key, self.shape)? else { return Ok(()) };
        let _row = self.row_lock(row)?;
        let mut first = false;
        self.tree.update(&document_key(row, &document.hash), |old| {
            let n = old.map(count_of).transpose()?.unwrap_or(0);
            first = n == 0;
            Ok(Update::Put(versions_value(u32::try_from(n.saturating_add(1)).unwrap_or(u32::MAX))))
        })?;
        if !first {
            return Ok(());
        }
        self.tree.insert(&hashed_key(&document.hash, row), &[])?;
        let at = doc(&document.hash, row);
        for term in &document.terms {
            if self.tree.insert(&posting_key(term, &at), &[])? {
                self.counted(term, true)?;
            }
        }
        Ok(())
    }

    /// Takes back one entry of `row` under `key`, if there is one: its
    /// document's postings with its last.
    ///
    /// # Errors
    ///
    /// As reading the document; the tree's.
    pub(super) fn take_back(&self, key: &Key, row: RowId) -> Result<()> {
        let Some(document) = Document::read(key, self.shape)? else { return Ok(()) };
        let _row = self.row_lock(row)?;
        let mut last = false;
        self.tree.update(&document_key(row, &document.hash), |old| {
            let Some(n) = old.map(count_of).transpose()? else { return Ok(Update::Keep) };
            last = n <= 1;
            Ok(if last { Update::Remove } else { Update::Put(versions_value(u32::try_from(n.saturating_sub(1)).unwrap_or(u32::MAX))) })
        })?;
        if !last {
            return Ok(());
        }
        self.tree.delete(&hashed_key(&document.hash, row))?;
        let at = doc(&document.hash, row);
        for term in &document.terms {
            if self.tree.delete(&posting_key(term, &at))?.is_some() {
                self.counted(term, false)?;
            }
        }
        Ok(())
    }

    /// Counts one more document posting `term`, or one fewer.
    ///
    /// # Errors
    ///
    /// The tree's.
    fn counted(&self, term: &Term, more: bool) -> Result<()> {
        self.tree.update(&count_key(term), |old| {
            let n = old.map(count_of).transpose()?.unwrap_or(0);
            let next = if more { n.saturating_add(1) } else { n.saturating_sub(1) };
            Ok(if next == 0 { Update::Remove } else { Update::Put(count_value(next)) })
        })
    }
}

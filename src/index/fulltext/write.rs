//! \[DESIGN\] A full-text index's entries written and taken back (RULES.md
//! D86, D29): each (document hash, row) pair is counted, one for each
//! version holding it. The insert that makes the count one writes the
//! document's postings and adds it to its terms' statistics, their stems'
//! counts and the totals; the remove that takes the last count reads the
//! document again and takes all of that back. Either reads the document
//! as it streams, its places sorted in a work area (`postings.rs`, D38).
//!
//! A document is numbered when first counted, from the totals entry's
//! next number, which is never given again (`keys.rs`), and its postings
//! go in each term's blocks under that number (`block.rs`).
//!
//! A row's count and what is written or taken back with it change under
//! the row's stripe lock (`FullTextIndex::row_lock`), so an insert never
//! meets a remove of the same row half done: garbage collection removes
//! an old version's entry holding no row lock of the table's. A term's
//! blocks are rewritten under its stripe lock (`FullTextIndex::term_lock`),
//! taken after the row's and one at a time, so locks are taken in one
//! order and never wait in a cycle. Each count of a term, a stem or the
//! totals is changed in one descent, under its leaf's lock
//! (`BLinkTree::update`), so threads inserting documents of other rows at
//! once never lose one. A document's writes failing part
//! way are taken back, as far as storage lets them be, before the error is
//! given: a failed change has not been made (`Index`).

use std::cell::Cell;

use super::FullTextIndex;
use super::block::{Places, Posting};
use super::document::{hash_of, single};
use super::keys::{
    COUNTS_KEY, Counts, Doc, DocumentValue, TermStatistics, count_of, count_value, document_key, hashed_key, number_key, number_value,
    overflow_key, statistic_key, stem_key,
};
use super::postings::{Event, Read, Runs, read};
use crate::engine::tree::Update;
use crate::error::{Code, Result, SqlError};
use crate::fulltext::stem;
use crate::index::Key;
use crate::storage::RowId;
use crate::value::Value;

/// A change to a count: one more, or one less.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    /// One more.
    Up,
    /// One less.
    Down,
}

/// How far a document's postings were written: the last term anything
/// was written of, and whether its statistics were counted. Terms come in
/// order, so every term before it was done.
#[derive(Default)]
struct Progress {
    term: Option<Vec<u8>>,
    counted: bool,
}

impl FullTextIndex {
    /// The bytes of the stem of `term`, cut as terms are.
    pub(super) fn stem_of(&self, term: &[u8]) -> Vec<u8> {
        let word = String::from_utf8_lossy(term);
        super::document::term_of(&stem(self.reading.language, &word), self.most_term).to_vec()
    }

    /// The document `value`, read, its places sorted in a work area.
    ///
    /// # Errors
    ///
    /// As `read`; SQLCODE -916 if it is null, which a counted entry's
    /// document never is.
    pub(super) fn read_document(&self, value: &Value) -> Result<Read> {
        let areas = self.spill.areas();
        read(value, &self.reading, self.most_term, (&areas, self.spill.area))?
            .ok_or_else(|| SqlError::new(Code::Corrupt, "[DESIGN] a full-text index's counted document is null"))
    }

    /// The runs of the terms of `read`.
    pub(super) fn runs(&self, read: Read) -> Runs {
        Runs::new(read.places, self.tree.max_entry())
    }

    /// `insert`: one more count of `key`'s document in `row`, its entries
    /// written if it is the first. A null document has none.
    ///
    /// # Errors
    ///
    /// As reading the document; the tree's.
    pub(super) fn add(&self, key: &Key, row: RowId) -> Result<()> {
        let value = single(key)?;
        let Some(hash) = hash_of(value, &self.reading)? else { return Ok(()) };
        let doc = Doc { row, hash };
        let _row = self.row_lock(row)?;
        let entry = document_key(doc);
        if let Some(old) = self.tree.get(&entry)? {
            let found = DocumentValue::decode(&old)?;
            return self.tree.update(&entry, |_| Ok(Update::Put(DocumentValue { count: found.count.saturating_add(1), ..found }.encode())));
        }
        let docno = self.next_docno()?;
        self.tree.update(&entry, |_| Ok(Update::Put(DocumentValue { count: 1, docno, length: 0 }.encode())))?;
        let posted = self.post(value, doc, docno);
        if posted.is_err() {
            // Taken back as far as storage lets it; the error given is the
            // write's, which says why. Its number is not given again.
            let _ = self.tree.delete(&entry);
        }
        posted
    }

    /// The next document number, taken from the totals entry.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if the entry is malformed.
    fn next_docno(&self) -> Result<u64> {
        let given = Cell::new(0_u64);
        self.tree.update(&COUNTS_KEY, |old| {
            let found = old.map(Counts::decode).transpose()?.unwrap_or_default();
            given.set(found.next);
            Ok(Update::Put(Counts { next: found.next.saturating_add(1), ..found }.encode()))
        })?;
        Ok(given.get())
    }

    /// Writes the entries of `value` as `doc`'s, numbered `docno`; taking
    /// back what it wrote if it fails.
    ///
    /// # Errors
    ///
    /// As reading the document; the tree's.
    fn post(&self, value: &Value, doc: Doc, docno: u64) -> Result<()> {
        let read = self.read_document(value)?;
        let length = read.length;
        let mut progress = Progress::default();
        let written = self.post_runs(read, docno, &mut progress).and_then(|()| self.post_document(doc, docno, length));
        if written.is_err() {
            // Taken back as far as storage lets it; the error given is the
            // write's.
            let _ = self.undo(value, docno, &progress);
        }
        written
    }

    /// Writes each run of `read`: its chunks past its block, then its
    /// posting in its term's blocks, counted, keeping `progress`.
    fn post_runs(&self, read: Read, docno: u64, progress: &mut Progress) -> Result<()> {
        for event in self.runs(read) {
            let event = event?;
            let begun = match &event {
                Event::Overflow(of, ..) | Event::End(of, ..) => of,
            };
            if progress.term.as_ref() != Some(begun) {
                *progress = Progress { term: Some(begun.clone()), counted: false };
            }
            match event {
                Event::Overflow(term, chunk, value) => self.tree.update(&overflow_key(&term, docno, chunk), |_| Ok(Update::Put(value)))?,
                Event::End(term, frequency, inline) => {
                    let places = inline.map_or(Places::Overflow, Places::Inline);
                    {
                        let _term = self.term_lock(&term)?;
                        self.put_posting(&term, Posting { docno, frequency, places })?;
                    }
                    self.count_term(&term, Step::Up, frequency)?;
                    progress.counted = true;
                }
            }
        }
        Ok(())
    }

    /// Takes back what `post` wrote of `value` numbered `docno` up to
    /// `progress`: the document read again, each term before the last done
    /// taken back, and the last's postings, and its count if made.
    ///
    /// # Errors
    ///
    /// As reading the document; the first of the tree's.
    fn undo(&self, value: &Value, docno: u64, progress: &Progress) -> Result<()> {
        let Some(last) = progress.term.as_deref() else { return Ok(()) };
        let mut first = Ok(());
        for event in self.runs(self.read_document(value)?) {
            let Event::End(term, ..) = event? else { continue };
            if term.as_slice() > last {
                break;
            }
            let counted = term.as_slice() < last || progress.counted;
            let taken = self.unpost(&term, docno).and_then(|()| if counted { self.count_term(&term, Step::Down, 0) } else { Ok(()) });
            if first.is_ok() {
                first = taken;
            }
        }
        first
    }

    /// Takes the posting of `term` numbered `docno` out of its blocks, and
    /// its places past them.
    fn unpost(&self, term: &[u8], docno: u64) -> Result<()> {
        {
            let _term = self.term_lock(term)?;
            self.take_posting(term, docno)?;
        }
        self.delete_overflow(term, docno)
    }

    /// Adds `doc` to the totals, its length to its entry, and its hashed
    /// and number entries; taking back the totals if what follows fails.
    fn post_document(&self, doc: Doc, docno: u64, length: u32) -> Result<()> {
        self.count_totals(Step::Up, length)?;
        let rest = self
            .tree
            .update(&document_key(doc), |old| {
                let found = old.map(DocumentValue::decode).transpose()?.unwrap_or(DocumentValue { count: 1, docno, length: 0 });
                Ok(Update::Put(DocumentValue { length, ..found }.encode()))
            })
            .and_then(|()| self.tree.update(&hashed_key(doc), |_| Ok(Update::Put(Vec::new()))))
            .and_then(|()| self.tree.update(&number_key(docno), |_| Ok(Update::Put(number_value(doc)))));
        if rest.is_err() {
            // Taken back as far as storage lets it; the error given is the
            // write's.
            let _ = self.tree.delete(&hashed_key(doc));
            let _ = self.count_totals(Step::Down, length);
        }
        rest
    }

    /// Changes `term`'s statistics and its stem's count by `step`, for a
    /// document holding it `frequency` times.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if an entry is malformed.
    fn count_term(&self, term: &[u8], step: Step, frequency: u32) -> Result<()> {
        self.tree.update(&statistic_key(term), |old| {
            let found = old.map(TermStatistics::decode).transpose()?.unwrap_or_default();
            let documents = stepped(found.documents, step);
            if documents == 0 {
                return Ok(Update::Remove);
            }
            Ok(Update::Put(TermStatistics { documents, most: found.most.max(frequency) }.encode()))
        })?;
        self.tree.update(&stem_key(&self.stem_of(term), term), |old| {
            let count = stepped(old.map(count_of).transpose()?.unwrap_or_default(), step);
            Ok(if count == 0 { Update::Remove } else { Update::Put(count_value(count)) })
        })
    }

    /// Changes the totals by one document of `length` words, by `step`.
    ///
    /// # Errors
    ///
    /// The tree's; SQLCODE -916 if the entry is malformed.
    fn count_totals(&self, step: Step, length: u32) -> Result<()> {
        self.tree.update(&COUNTS_KEY, |old| {
            let found = old.map(Counts::decode).transpose()?.unwrap_or_default();
            let mut totals = found.totals;
            totals.documents = stepped(totals.documents, step);
            totals.length = match step {
                Step::Up => totals.length.saturating_add(u64::from(length)),
                Step::Down => totals.length.saturating_sub(u64::from(length)),
            };
            Ok(Update::Put(Counts { totals, ..found }.encode()))
        })
    }

    /// `remove`: one less count of `key`'s document in `row`, its entries
    /// taken back with the last.
    ///
    /// # Errors
    ///
    /// As reading the document; the tree's.
    pub(super) fn take_back(&self, key: &Key, row: RowId) -> Result<()> {
        let value = single(key)?;
        let Some(hash) = hash_of(value, &self.reading)? else { return Ok(()) };
        let doc = Doc { row, hash };
        let _row = self.row_lock(row)?;
        let last = Cell::new(None);
        self.tree.update(&document_key(doc), |old| {
            let Some(old) = old else { return Ok(Update::Keep) };
            let found = DocumentValue::decode(old)?;
            if found.count > 1 {
                return Ok(Update::Put(DocumentValue { count: found.count.saturating_sub(1), ..found }.encode()));
            }
            last.set(Some(found));
            Ok(Update::Remove)
        })?;
        let Some(found) = last.get() else { return Ok(()) };
        let read = match self.read_document(value) {
            Ok(read) => read,
            Err(e) => {
                // Its count put back, so the remove has not been made.
                let _ = self.tree.update(&document_key(doc), |_| Ok(Update::Put(found.encode())));
                return Err(e);
            }
        };
        self.tree.delete(&hashed_key(doc))?;
        self.tree.delete(&number_key(found.docno))?;
        self.count_totals(Step::Down, found.length)?;
        self.retract(read, found.docno)
    }

    /// Takes back the postings of each term of `read` numbered `docno`, and the
    /// document from their statistics; a term's errors do not stop the
    /// others'.
    ///
    /// # Errors
    ///
    /// The first of reading the runs and the tree's.
    fn retract(&self, read: Read, docno: u64) -> Result<()> {
        let mut first = Ok(());
        for event in self.runs(read) {
            let taken = match event? {
                Event::End(term, ..) => self.unpost(&term, docno).and_then(|()| self.count_term(&term, Step::Down, 0)),
                Event::Overflow(..) => Ok(()),
            };
            if first.is_ok() {
                first = taken;
            }
        }
        first
    }
}

/// `count` changed by `step`, never below 0.
fn stepped(count: u64, step: Step) -> u64 {
    match step {
        Step::Up => count.saturating_add(1),
        Step::Down => count.saturating_sub(1),
    }
}

//! \[DESIGN\] A full-text index built whole (RULES.md D86, D31): each
//! entry's document read once, and the tree loaded in key order
//! (`BLinkTree::load`), as inserting each entry in that order would leave
//! it, its blocks full. Two sorts in spill areas (`spill::sorted_records`)
//! do it, so no more of the index is held than an area's bytes.
//!
//! The first sorts records by document, its row and hash, each one of
//!
//! * the document's: then 0 and its length, four bytes, one for each
//!   version, so those alike, adjacent, are counted;
//! * a chunk of a term's places past its block: then 1, the term, its
//!   end, the chunk's number and its bytes;
//! * a term's posting: then 2, the term, its end, its frequency, four
//!   bytes, 1 if its places follow, 0 if they are past its block, and
//!   them.
//!
//! Read in order, versions alike are kept once, and each document is
//! numbered as it comes, so documents are numbered in row order. Each
//! record then makes records of the tree's own keys, sorted by the second:
//! a document's, hashed and number entries, a chunk's entry with its
//! bytes' length at the end, four bytes, a term's posting key with what
//! it holds, and its statistic and stem keys with the document's number,
//! and frequency for the statistic. The postings of a term then come in
//! order, packed into full blocks as they pass (`block::Builder`); the
//! statistics and stems of a term are counted. The totals' key sorts
//! first, before they are known, so it is written once the tree is loaded.

use std::iter::Peekable;

use super::FullTextIndex;
use super::block::{Builder, Places, Posting, encode};
use super::document::single;
use super::keys::{
    COUNTS_KEY, Counts, DOCNO_BYTES, DOCUMENT, DOCUMENT_KEY, Doc, DocumentValue, HASH_BYTES, HASHED, NUMBER, OVERFLOW, POSTING, STATISTIC,
    STEM, TermStatistics, block_key, corrupt, count_value, document_key, hashed_key, number_key, number_value, overflow_key, statistic_key,
    stem_key,
};
use super::postings::{Event, read};
use crate::engine::tree::Update;
use crate::error::Result;
use crate::index::Key;
use crate::spill::{Area, Records, WorkAreas, sorted_records};
use crate::storage::RowId;

/// Bytes of a length or frequency in a record.
const FOUR: usize = 4;
/// Bytes of a document's row and hash at a first record's head.
const DOC_BYTES: usize = 8 + HASH_BYTES;
/// What a first record holds after its document.
const OF_DOCUMENT: u8 = 0;
const OF_CHUNK: u8 = 1;
const OF_POSTING: u8 = 2;
/// The flag of a posting record whose places follow.
const INLINE: u8 = 1;

/// The four bytes at the end of `record`, big-endian, and the rest.
fn number_at_end(record: &[u8]) -> Result<(&[u8], u32)> {
    let (rest, number) = record.split_at_checked(record.len().checked_sub(FOUR).ok_or_else(corrupt)?).ok_or_else(corrupt)?;
    Ok((rest, u32::from_be_bytes(number.try_into().map_err(|_| corrupt())?)))
}

/// The four bytes at the head of `bytes`, big-endian, and the rest.
fn number_at_head(bytes: &[u8]) -> Result<(u32, &[u8])> {
    let (number, rest) = bytes.split_at_checked(FOUR).ok_or_else(corrupt)?;
    Ok((u32::from_be_bytes(number.try_into().map_err(|_| corrupt())?), rest))
}

/// The term at the head of `bytes`, up to its end, and what follows it.
fn term_at(bytes: &[u8]) -> Result<(&[u8], &[u8])> {
    let end = bytes.iter().position(|&b| b == 0).ok_or_else(corrupt)?;
    let (term, rest) = bytes.split_at_checked(end).ok_or_else(corrupt)?;
    Ok((term, rest.get(1..).unwrap_or_default()))
}

/// A first record of `doc`: its row and hash, then `kind`.
fn doc_record(doc: Doc, kind: u8) -> Vec<u8> {
    let mut record = doc.row.to_be_bytes().to_vec();
    record.extend_from_slice(&doc.hash);
    record.push(kind);
    record
}

impl FullTextIndex {
    /// The first record of one event of `doc`'s runs.
    fn event_record(doc: Doc, event: Result<Event>) -> Result<Vec<u8>> {
        Ok(match event? {
            Event::Overflow(term, chunk, bytes) => {
                let mut record = doc_record(doc, OF_CHUNK);
                record.extend_from_slice(&term);
                record.push(0);
                record.extend_from_slice(&chunk.to_be_bytes());
                record.extend_from_slice(&bytes);
                record
            }
            Event::End(term, frequency, inline) => {
                let mut record = doc_record(doc, OF_POSTING);
                record.extend_from_slice(&term);
                record.push(0);
                record.extend_from_slice(&frequency.to_be_bytes());
                record.push(u8::from(inline.is_some()));
                record.extend_from_slice(&inline.unwrap_or_default());
                record
            }
        })
    }

    /// The first records of `entry`, as its document's runs are read: its
    /// document's, then its runs'; none for a null document. Its places
    /// are sorted in an area of `work`.
    fn records<'a>(&'a self, entry: Result<(Key, RowId)>, work: (&WorkAreas, Area)) -> impl Iterator<Item = Result<Vec<u8>>> + 'a {
        let made = entry.and_then(|(key, row)| {
            let value = single(&key)?;
            self.note_language_in_memory(value)?;
            Ok(read(value, &self.reading, self.most_term, work)?.map(|r| (row, r)))
        });
        let (head, runs) = match made {
            Ok(Some((row, read))) => {
                let doc = Doc { row, hash: read.hash };
                let mut document = doc_record(doc, OF_DOCUMENT);
                document.extend_from_slice(&read.length.to_be_bytes());
                (Some(Ok(document)), Some((doc, self.runs(read))))
            }
            Ok(None) => (None, None),
            Err(e) => (Some(Err(e)), None),
        };
        let runs = runs.into_iter().flat_map(|(doc, runs)| runs.map(move |event| Self::event_record(doc, event)));
        head.into_iter().chain(runs)
    }

    /// `load`: every entry's records sorted by document, numbered, sorted
    /// again by key, then the tree loaded from them, then the totals
    /// written.
    ///
    /// # Errors
    ///
    /// As reading the entries and their documents; the spill area's; the
    /// tree's.
    pub(super) fn load_whole(
        &self,
        entries: &mut dyn Iterator<Item = Result<(Key, RowId)>>,
        (areas, area): (&WorkAreas, Area),
    ) -> Result<()> {
        let mut records = entries.flat_map(|entry| self.records(entry, (areas, area)));
        let by_document = sorted_records(areas, area, &mut records)?;
        let mut numbered =
            Numbered { index: self, records: by_document.peekable(), last: Vec::new(), doc: None, counts: Counts::default() };
        let mut keyed = std::iter::from_fn(|| numbered.next_records()).flatten();
        let by_key = sorted_records(areas, area, &mut keyed)?;
        let counts = numbered.counts;
        let mut merged = Merged { index: self, records: by_key, back: None };
        self.tree.load(&mut merged)?;
        self.write_languages()?;
        if counts.totals.documents == 0 {
            return Ok(());
        }
        self.tree.update(&COUNTS_KEY, |_| Ok(Update::Put(counts.encode())))
    }
}

/// The first records read in order, versions alike kept once and each
/// document numbered, making the second's.
struct Numbered<'a> {
    index: &'a FullTextIndex,
    records: Peekable<Records>,
    /// The record before, so one alike is passed over.
    last: Vec<u8>,
    /// The document being read, and its number.
    doc: Option<(Doc, u64)>,
    /// The totals, and the next number.
    counts: Counts,
}

impl Numbered<'_> {
    /// The second records of the next first record, none past the last.
    fn next_records(&mut self) -> Option<Vec<Result<Vec<u8>>>> {
        loop {
            let record = match self.records.next()? {
                Ok(record) => record,
                Err(e) => return Some(vec![Err(e)]),
            };
            if record == self.last {
                continue;
            }
            let made = self.made(&record);
            self.last = record;
            return Some(made.unwrap_or_else(|e| vec![Err(e)]));
        }
    }

    /// The second records of first record `record`.
    fn made(&mut self, record: &[u8]) -> Result<Vec<Result<Vec<u8>>>> {
        let (head, rest) = record.split_at_checked(DOC_BYTES).ok_or_else(corrupt)?;
        let (row, hash) = head.split_at_checked(8).ok_or_else(corrupt)?;
        let doc = Doc { row: RowId::from_be_bytes(row.try_into().map_err(|_| corrupt())?), hash: hash.try_into().map_err(|_| corrupt())? };
        let (&kind, rest) = rest.split_first().ok_or_else(corrupt)?;
        if kind == OF_DOCUMENT {
            return self.document(doc, record, rest);
        }
        let Some((_, docno)) = self.doc.filter(|(at, _)| *at == doc) else { return Err(corrupt()) };
        let (term, rest) = term_at(rest)?;
        if kind == OF_CHUNK {
            let (chunk, bytes) = rest.split_at_checked(2).ok_or_else(corrupt)?;
            let mut out = overflow_key(term, docno, u16::from_be_bytes(chunk.try_into().map_err(|_| corrupt())?));
            out.extend_from_slice(bytes);
            out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_be_bytes());
            return Ok(vec![Ok(out)]);
        }
        let mut posting = block_key(term, docno);
        posting.extend_from_slice(rest);
        let mut statistic = statistic_key(term);
        statistic.extend_from_slice(&docno.to_be_bytes());
        statistic.extend_from_slice(rest.get(..FOUR).ok_or_else(corrupt)?);
        let mut stem = stem_key(&self.index.stem_of(term), term);
        stem.extend_from_slice(&docno.to_be_bytes());
        Ok(vec![Ok(posting), Ok(statistic), Ok(stem)])
    }

    /// The second records of a document of `doc` whose first record is
    /// `record`, holding its length in `rest`: its versions counted, and it
    /// numbered.
    fn document(&mut self, doc: Doc, record: &[u8], rest: &[u8]) -> Result<Vec<Result<Vec<u8>>>> {
        let mut count = 1_u32;
        while self.records.next_if(|next| next.as_ref().is_ok_and(|n| n.as_slice() == record)).is_some() {
            count = count.saturating_add(1);
        }
        let (length, _) = number_at_head(rest)?;
        let docno = self.counts.next;
        self.counts.next = docno.saturating_add(1);
        self.counts.totals.documents = self.counts.totals.documents.saturating_add(1);
        self.counts.totals.length = self.counts.totals.length.saturating_add(u64::from(length));
        self.doc = Some((doc, docno));
        let mut document = document_key(doc);
        document.extend_from_slice(&DocumentValue { count, docno, length }.encode());
        let mut number = number_key(docno);
        number.extend_from_slice(&number_value(doc, length));
        Ok(vec![Ok(document), Ok(hashed_key(doc)), Ok(number)])
    }
}

/// The tree's entries of the second records: postings packed into
/// blocks, statistics and stems counted.
struct Merged<'a> {
    index: &'a FullTextIndex,
    records: Records,
    /// A record read past a block it did not fit, to be read again.
    back: Option<Vec<u8>>,
}

/// The posting a second posting record holds, after its tag: its term
/// and the posting.
fn posting_of(record: &[u8]) -> Result<(&[u8], Posting)> {
    let (term, rest) = term_at(record.get(1..).unwrap_or_default())?;
    let (docno, rest) = rest.split_at_checked(DOCNO_BYTES).ok_or_else(corrupt)?;
    let (frequency, rest) = number_at_head(rest)?;
    let (&flag, places) = rest.split_first().ok_or_else(corrupt)?;
    let places = if flag == INLINE { Places::Inline(places.to_vec()) } else { Places::Overflow };
    Ok((term, Posting { docno: u64::from_be_bytes(docno.try_into().map_err(|_| corrupt())?), frequency, places }))
}

impl Merged<'_> {
    /// The next record: the one given back, or the sort's.
    fn pull(&mut self) -> Option<Result<Vec<u8>>> {
        self.back.take().map(Ok).or_else(|| self.records.next())
    }

    /// The records after `first` whose first `prefix` bytes are its,
    /// taken; the first other given back.
    fn alike(&mut self, first: &[u8], prefix: usize) -> Result<Vec<Vec<u8>>> {
        let head = first.get(..prefix).unwrap_or(first);
        let mut out = Vec::new();
        while let Some(next) = self.pull() {
            let next = next?;
            if next.get(..prefix) != Some(head) {
                self.back = Some(next);
                break;
            }
            out.push(next);
        }
        Ok(out)
    }

    /// The entry of the records beginning with `first`.
    fn entry(&mut self, first: Vec<u8>) -> Result<(Vec<u8>, Vec<u8>)> {
        let split = |at: usize| first.split_at_checked(at).map(|(k, v)| (k.to_vec(), v.to_vec())).ok_or_else(corrupt);
        match first.first().copied() {
            Some(DOCUMENT) => split(DOCUMENT_KEY),
            Some(HASHED) => Ok((first, Vec::new())),
            Some(NUMBER) => split(1_usize.saturating_add(DOCNO_BYTES)),
            Some(POSTING) => self.block(&first),
            Some(OVERFLOW) => {
                let (rest, length) = number_at_end(&first)?;
                let at = rest.len().checked_sub(usize::try_from(length).map_err(|_| corrupt())?).ok_or_else(corrupt)?;
                let (key, value) = rest.split_at_checked(at).ok_or_else(corrupt)?;
                Ok((key.to_vec(), value.to_vec()))
            }
            Some(STATISTIC) => self.statistic(&first),
            Some(STEM) => {
                let key = first.len().checked_sub(DOCNO_BYTES).ok_or_else(corrupt)?;
                let documents = self.alike(&first, key)?.len();
                let count = u64::try_from(documents).unwrap_or(u64::MAX).saturating_add(1);
                Ok((first.get(..key).unwrap_or_default().to_vec(), count_value(count)))
            }
            Some(_) | None => Err(corrupt()),
        }
    }

    /// A full block of the postings of one term beginning with `first`'s.
    fn block(&mut self, first: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        let (term, posting) = posting_of(first)?;
        let room = self.index.room(term);
        let mut builder = Builder::new(posting.docno);
        if builder.add(posting, room).is_some() {
            return Err(corrupt());
        }
        while let Some(next) = self.pull() {
            let next = next?;
            let fitted = match next.first() {
                Some(&POSTING) => {
                    let (next_term, next_posting) = posting_of(&next)?;
                    next_term == term && builder.add(next_posting, room).is_none()
                }
                Some(_) | None => false,
            };
            if !fitted {
                self.back = Some(next);
                break;
            }
        }
        Ok((block_key(term, builder.first()), encode(builder.first(), &builder.postings)))
    }

    /// A term's statistics: its documents, and the most of their
    /// frequencies.
    fn statistic(&mut self, first: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        let key = first.len().checked_sub(DOCNO_BYTES.saturating_add(FOUR)).ok_or_else(corrupt)?;
        let mut stats = TermStatistics { documents: 1, most: number_at_end(first)?.1 };
        for next in self.alike(first, key)? {
            stats.documents = stats.documents.saturating_add(1);
            stats.most = stats.most.max(number_at_end(&next)?.1);
        }
        Ok((first.get(..key).unwrap_or_default().to_vec(), stats.encode()))
    }
}

impl Iterator for Merged<'_> {
    type Item = Result<(Vec<u8>, Vec<u8>)>;

    fn next(&mut self) -> Option<Result<(Vec<u8>, Vec<u8>)>> {
        Some(self.pull()?.and_then(|first| self.entry(first)))
    }
}

//! \[DESIGN\] A sparse inverted index built whole (`Index::load`), on an
//! empty index nothing else reads or writes meanwhile. Each entry gives a
//! record of its count and one of each posting, sorted together bytewise
//! in a work area (`spill::sorted_records`, RULES.md D38), so a load of
//! more postings than memory holds spills; then the tree is written in key
//! order (`BLinkTree::load`): the counts, each run of equal records one
//! count; the blocks, each dimension's postings in rows ascending, filled
//! to the block size; and last, each dimension's bounds.
//!
//! A posting record is the block key's tag, dimension and row, then the
//! vector's hash and the weight. An entry given twice gives equal records,
//! written once, as an insert of a vector already counted writes no
//! postings; two vectors of one row give two postings of it.

use std::collections::VecDeque;
use std::iter::Peekable;

use super::block::{Postings, Quantization};
use super::keys::{self, BLOCK_KEY, DOCUMENT, POSTINGS};
use super::vector::HASH_BYTES;
use super::{SparseTree, SparseVector};
use crate::error::Result;
use crate::spill::{Area, Records, WorkAreas, sorted_records};
use crate::storage::RowId;
use crate::sync::lock_builtin;

/// The bytes of a posting record: block key, hash, weight.
const POSTING_RECORD: usize = 33;
/// The bytes of a weight.
const WEIGHT_BYTES: usize = 4;

/// A tree entry: its key and value.
type Entry = (Vec<u8>, Vec<u8>);

/// The records of `row`'s `vector`: its count's, then a posting's for each
/// dimension it holds.
fn records_of(vector: &SparseVector, row: RowId) -> Vec<Vec<u8>> {
    let hash = vector.hash();
    let mut out = Vec::with_capacity(vector.nnz().saturating_add(1));
    out.push(keys::document_key(&hash, row));
    for (dimension, weight) in vector.entries() {
        let mut record = keys::block_key(dimension, row);
        record.reserve(POSTING_RECORD.saturating_sub(BLOCK_KEY));
        record.extend_from_slice(&hash);
        record.extend_from_slice(&weight.to_le_bytes());
        out.push(record);
    }
    out
}

/// The dimension, row and weight of a posting record.
fn posting_of(record: &[u8]) -> Result<(u32, RowId, f32)> {
    let (key, rest) = record.split_at_checked(BLOCK_KEY).ok_or_else(|| super::block::corrupt("a load's record is cut short"))?;
    let (dimension, row) = keys::block_of(key)?;
    let weight = rest.get(HASH_BYTES..).and_then(|w| <[u8; WEIGHT_BYTES]>::try_from(w).ok());
    let weight = weight.ok_or_else(|| super::block::corrupt("a load's record is cut short"))?;
    Ok((dimension, row, f32::from_le_bytes(weight)))
}

/// A dimension's postings being gathered into blocks.
struct Gathering {
    dimension: u32,
    postings: Postings,
    max: f32,
    min: f32,
}

/// The tree's entries, made from the sorted records as they are taken.
struct Loader<'a> {
    index: &'a SparseTree,
    sorted: Peekable<Records>,
    quantization: Quantization,
    block: usize,
    ready: VecDeque<Entry>,
    gathering: Option<Gathering>,
    /// The last posting record, so one given twice is written once.
    last: Vec<u8>,
    bounds: Vec<(u32, f32, f32)>,
    documents: u64,
    done: bool,
}

impl Loader<'_> {
    /// Takes one record, or at the end what is gathered and the bounds.
    fn step(&mut self) -> Result<()> {
        let Some(record) = self.sorted.next().transpose()? else {
            self.finish()?;
            for (dimension, max, min) in std::mem::take(&mut self.bounds) {
                self.ready.push_back((keys::summary_key(dimension), keys::summary_value(max, min)));
            }
            self.done = true;
            return Ok(());
        };
        match record.first() {
            Some(&DOCUMENT) => {
                self.document(record);
                Ok(())
            }
            Some(&POSTINGS) => self.posting(record),
            Some(_) | None => Err(super::block::corrupt("a load's record has no known tag")),
        }
    }

    /// A count's record, and those equal after it.
    fn document(&mut self, record: Vec<u8>) {
        let mut count = 1_u32;
        while self.sorted.next_if(|next| next.as_ref().is_ok_and(|n| *n == record)).is_some() {
            count = count.saturating_add(1);
        }
        self.documents = self.documents.saturating_add(1);
        self.ready.push_back((record, keys::count_value(count)));
    }

    /// A posting's record: added to its dimension's postings, a block
    /// written first if they are full and the row is a new one.
    fn posting(&mut self, record: Vec<u8>) -> Result<()> {
        if record == self.last {
            return Ok(());
        }
        let (dimension, row, weight) = posting_of(&record)?;
        self.last = record;
        if self.gathering.as_ref().is_some_and(|g| g.dimension != dimension) {
            self.finish()?;
        }
        let Some(mut gathered) = self.gathering.take() else {
            self.gathering = Some(Gathering { dimension, postings: Postings::single(row, weight), max: weight, min: weight });
            return Ok(());
        };
        if gathered.postings.len() >= self.block && gathered.postings.rows.last() != Some(&row) {
            let full = std::mem::replace(&mut gathered.postings, Postings::single(row, weight));
            self.write(dimension, full)?;
        } else {
            gathered.postings.insert(row, weight);
        }
        gathered.max = gathered.max.max(weight);
        gathered.min = gathered.min.min(weight);
        self.gathering = Some(gathered);
        Ok(())
    }

    /// Writes the dimension gathered, and keeps its bounds.
    fn finish(&mut self) -> Result<()> {
        let Some(gathered) = self.gathering.take() else { return Ok(()) };
        self.bounds.push((gathered.dimension, gathered.max, gathered.min));
        self.write(gathered.dimension, gathered.postings)
    }

    /// Writes `postings` of `dimension` as blocks.
    fn write(&mut self, dimension: u32, postings: Postings) -> Result<()> {
        for (first, bytes) in self.index.pieces(postings, self.quantization)? {
            self.ready.push_back((keys::block_key(dimension, first), bytes));
        }
        Ok(())
    }
}

impl Iterator for Loader<'_> {
    type Item = Result<Entry>;

    fn next(&mut self) -> Option<Result<Entry>> {
        loop {
            if let Some(entry) = self.ready.pop_front() {
                return Some(Ok(entry));
            }
            if self.done {
                return None;
            }
            if let Err(error) = self.step() {
                self.done = true;
                return Some(Err(error));
            }
        }
    }
}

impl SparseTree {
    /// Adds every entry of `entries`, in any order, to the index, which is
    /// empty and which nothing else reads or writes until it returns: as
    /// inserting each would, sorting them in a work area of `work`.
    ///
    /// # Errors
    ///
    /// The entries' own; the work area's; the tree's: SQLCODE -910 if the
    /// index is not empty.
    pub fn load(&self, entries: &mut dyn Iterator<Item = Result<(SparseVector, RowId)>>, (areas, area): (&WorkAreas, Area)) -> Result<()> {
        let mut records = entries.flat_map(|entry| match entry {
            Ok((vector, row)) => records_of(&vector, row).into_iter().map(Ok).collect::<Vec<_>>(),
            Err(error) => vec![Err(error)],
        });
        let sorted = sorted_records(areas, area, &mut records)?.peekable();
        let layout = self.layout();
        let mut loader = Loader {
            index: self,
            sorted,
            quantization: layout.quantization,
            block: layout.block,
            ready: VecDeque::new(),
            gathering: None,
            last: Vec::new(),
            bounds: Vec::new(),
            documents: 0,
            done: false,
        };
        self.inner.tree.load(&mut loader)?;
        *lock_builtin(&self.inner.documents) = Some(loader.documents);
        Ok(())
    }
}

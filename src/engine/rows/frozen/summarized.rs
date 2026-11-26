//! \[DESIGN\] A table that holds groups read for set functions its groups'
//! summaries answer (RULES.md D1181, `TableStorage::summarized`): the
//! rows a scan by batch with no condition would give (`batches.rs`), each
//! group the tree has no entry over given whole, by its summary, its
//! rows and of each column asked for its nulls and its least and greatest
//! values, with none of its values read. The directory is taken before
//! the tree is read, as the scan's is. A group whose shadow bits say the
//! tree may have entries for its rows is read by batch, the tree's
//! entries over it with it, as the scan reads it; then the tree's entries
//! outside every group's range. So each row the scan gives is given once,
//! of a summary or of a batch.

use std::sync::Arc;

use super::Directory;
use super::batches::Batched;
use super::counts::outside;
use crate::engine::rows::RowTable;
use crate::engine::rows::mvcc::Read;
use crate::storage::{Piece, Pieces};
use crate::txn::TxId;

impl RowTable {
    /// The pieces `TableStorage::summarized` gives of the rows `tx` sees, of
    /// `columns`; none of a table that holds no group and freezes none.
    /// Nothing is read, nor the read noted, until the first is asked for.
    pub(in crate::engine::rows) fn summarized_of(self: Arc<Self>, tx: TxId, columns: &[usize]) -> Option<Pieces> {
        if !self.holds_groups() {
            return None;
        }
        let columns = columns.to_vec();
        let mut started: Option<Pieces> = None;
        Some(Box::new(std::iter::from_fn(move || started.get_or_insert_with(|| Arc::clone(&self).pieces(tx, &columns)).next())))
    }

    /// The pieces of the rows `tx` sees, of `columns`, the read noted first,
    /// as a scan's of those columns with no condition is (`rows/mvcc.rs`).
    fn pieces(self: Arc<Self>, tx: TxId, columns: &[usize]) -> Pieces {
        let opened = self.reader(tx).and_then(|reader| self.note_read(tx, || Read::scan(None, columns)).map(|()| reader));
        let (reader, directory) = match opened.and_then(|reader| self.directory_for(reader).map(|directory| (reader, directory))) {
            Ok(found) => found,
            Err(e) => return Box::new(std::iter::once(Err(e))),
        };
        let (whole, shadowed): (Vec<_>, Vec<_>) = directory.groups.iter().cloned().partition(|e| self.shadow_ranges(e).is_empty());
        let summaries: Vec<Piece> = whole
            .iter()
            .map(|e| Piece::Whole {
                rows: u64::from(e.summary.rows),
                columns: columns
                    .iter()
                    .map(|&c| e.summary.columns.get(c).map_or((0, None), |o| (u64::from(o.nulls), o.bounds.clone())))
                    .collect(),
            })
            .collect();
        let part = Arc::new(Directory::part(&directory, shadowed));
        let rest = Batched::within(Arc::clone(&self), (reader, part), (columns.to_vec(), None), outside(&directory));
        Box::new(summaries.into_iter().map(Ok).chain(rest.map(|batch| batch.map(Piece::Batch))))
    }
}

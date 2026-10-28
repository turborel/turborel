//! A row table's scans: the rows a transaction sees, read through the table
//! they come from, so a scan owns what it reads and outlives the call that
//! started it (`TableStorage::scan`). Split from rows.rs (RULES.md
//! R-SIZE-2).

use std::iter;
use std::ops::{Bound, Deref};
use std::sync::Arc;

use super::mvcc::Read;
use super::{RowTable, row_id};
use crate::engine::tree::{BLinkTree, Direction, Scan, scan_of};
use crate::storage::{Condition, Rows};
use crate::txn::TxId;

/// A table's tree, reached through the table, so a scan that owns the
/// table owns the tree it reads.
pub(super) struct TreeOf(Arc<RowTable>);

impl Deref for TreeOf {
    type Target = BLinkTree;

    fn deref(&self) -> &BLinkTree {
        &self.0.tree
    }
}

impl RowTable {
    /// Every entry of the table's tree, in row id order, read through
    /// `table`, which the scan holds.
    pub(super) fn entries(table: &Arc<Self>) -> Scan<TreeOf> {
        scan_of(TreeOf(Arc::clone(table)), Bound::Unbounded, Bound::Unbounded, Direction::Forward)
    }

    /// The rows `tx` sees, `columns` decoded, and only those `condition`
    /// matches, if given, which is then what counts as read.
    pub(super) fn scan_rows(self: Arc<Self>, tx: TxId, columns: &[usize], condition: Option<Condition>) -> Rows {
        let read = condition.clone().map_or(Read::All, Read::Condition);
        let reader = match self.reader(tx).and_then(|reader| self.note_read(tx, read).map(|()| reader)) {
            Ok(reader) => reader,
            Err(e) => return Box::new(iter::once(Err(e))),
        };
        // The condition's columns are decoded too, to check it.
        let mut columns = columns.to_vec();
        columns.extend(condition.iter().flat_map(|c| c.columns.iter().copied()));
        Box::new(Self::entries(&self).filter_map(move |entry| {
            let found = entry.and_then(|(key, newest)| {
                let id = row_id(&key)?;
                let Some(record) = self.visible_from(newest, reader)? else { return Ok(None) };
                let row = self.decode(id, &record, Some(&columns))?;
                Ok(condition.as_ref().is_none_or(|c| c.matches(&row)).then_some((id, row)))
            });
            found.transpose()
        }))
    }
}

//! \[EXTENSION\] A load's index entries held back (RULES.md D532). Of each
//! index of a table that is empty as a load into it begins, under the
//! table's lock held alone, the entries its rows give are not inserted as
//! they go in but kept, as the undo entries that would take them out, from
//! the load's mark in the transaction's undo log on. As the load ends they
//! are read back and the index is built whole from them (`Index::load`),
//! or inserted in their rows' order if it is no longer empty. A statement
//! or savepoint undone back past the load's mark undoes the load: its
//! entries go with the undo entries taken, and it is forgotten
//! (`UndoLog::take_from`).

use std::sync::Arc;

use super::{UndoEntry, entry_of};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::index::{Index, Key};
use crate::spill::{Decode, SpillList};
use crate::storage::RowId;
use crate::txn::Txn;

/// A load under way: into which table, from which place of the undo log
/// on, and the indexes whose entries it holds back.
pub(crate) struct Loading {
    table: QualifiedName,
    pub(super) mark: usize,
    indexes: Vec<Arc<dyn Index>>,
}

/// What ends a load: called with each index held back and its entries, in
/// their rows' order.
pub(crate) type Ending<'a> = &'a mut dyn FnMut(&Arc<dyn Index>, &mut dyn Iterator<Item = Result<(Key, RowId)>>) -> Result<()>;

impl super::UndoLog {
    /// Whether a load is under way.
    pub(crate) fn loads(&self) -> bool {
        self.loading.is_some()
    }
}

impl Txn {
    /// Whether a load is under way in it.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the transaction's state is poisoned.
    pub(crate) fn loads(&self) -> Result<bool> {
        Ok(self.state()?.undo.loads())
    }

    /// Begins a load into `table`, holding back the entries of `indexes`
    /// from here on, unless one is under way: whether it began.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the transaction's state is poisoned.
    pub(crate) fn begin_load(&self, table: &QualifiedName, indexes: Vec<Arc<dyn Index>>) -> Result<bool> {
        let mut st = self.state()?;
        if st.undo.loading.is_some() {
            return Ok(false);
        }
        let mark = st.undo.mark();
        st.undo.loading = Some(Loading { table: table.clone(), mark, indexes });
        Ok(true)
    }

    /// The indexes whose entries a load into `table` holds back, if one is
    /// under way; none if none is, or one into another table.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the transaction's state is poisoned.
    pub(crate) fn held_back(&self, table: &QualifiedName) -> Result<Option<Vec<Arc<dyn Index>>>> {
        let st = self.state()?;
        Ok(st.undo.loading.as_ref().filter(|l| l.table == *table).map(|l| l.indexes.clone()))
    }

    /// Ends the load under way, if one is: `ending` called with each index
    /// it held back and its entries, read back from the undo log, which
    /// keeps them.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the transaction's state is poisoned; the spill
    /// area's, reading the entries back; `ending`'s.
    pub(crate) fn end_load(&self, ending: Ending<'_>) -> Result<()> {
        let (loading, mut tail, targets, types) = {
            let mut st = self.state()?;
            let Some(loading) = st.undo.loading.take() else { return Ok(()) };
            let (targets, types) = (st.undo.targets.clone(), st.undo.types.clone());
            let decode = |record: &[u8]| entry_of((&targets, &types), record);
            let tail = st.undo.list.take_from(loading.mark, &decode)?;
            (loading, tail, targets, types)
        };
        let decode = |record: &[u8]| entry_of((&targets, &types), record);
        let ended = loading.indexes.iter().try_for_each(|index| ending(index, &mut held_entries(&mut tail, index, &decode)));
        self.state()?.undo.list.append(tail);
        ended
    }
}

/// The entries of `index` among the undo entries of `tail`, in order: its
/// segments read back, or copied, one at a time.
fn held_entries<'a>(
    tail: &'a mut SpillList<UndoEntry>,
    index: &'a Arc<dyn Index>,
    decode: Decode<'a, UndoEntry>,
) -> impl Iterator<Item = Result<(Key, RowId)>> + 'a {
    let (segments, mut at) = (tail.segments(), 0_usize);
    let mut read = Vec::new().into_iter();
    std::iter::from_fn(move || {
        loop {
            if let Some(entry) = read.next() {
                return Some(Ok(entry));
            }
            if at >= segments {
                return None;
            }
            let segment = tail.segment(at, decode);
            at = at.saturating_add(1);
            match segment {
                Ok(entries) => read = entries.into_iter().filter_map(|e| of(index, e)).flatten().collect::<Vec<_>>().into_iter(),
                Err(e) => return Some(Err(e)),
            }
        }
    })
}

/// The entries `entry` holds of `index`, if it is an index's of rows
/// inserted together.
fn of(index: &Arc<dyn Index>, entry: UndoEntry) -> Option<Vec<(Key, RowId)>> {
    match entry {
        UndoEntry::IndexInsertedRows { index: of, entries, .. } => {
            std::ptr::addr_eq(Arc::as_ptr(&of), Arc::as_ptr(index)).then_some(entries)
        }
        UndoEntry::Inserted { .. }
        | UndoEntry::Deleted { .. }
        | UndoEntry::Updated { .. }
        | UndoEntry::IndexInserted { .. }
        | UndoEntry::IndexRemoved { .. } => None,
    }
}

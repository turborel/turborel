//! \[DESIGN\] A frozen row read by its id (RULES.md D31, D1091): its
//! group found in the directory, its header as its entry holds it
//! (`columns/cache.rs`), its place in the group, and then of each column
//! asked for the one chunk that holds the row, found by halving, and the
//! one value read of it. Nothing of the other columns is read, and a
//! read of listed columns makes no row of the table's width.

use super::Directory;
use crate::engine::rows::RowTable;
use crate::engine::rows::mvcc::Reader;
use crate::error::Result;
use crate::storage::{Row, RowId};
use crate::value::Value;

impl RowTable {
    /// Row `id`'s frozen values, `columns` of them or all, if a group of
    /// `directory` holds it.
    pub(in crate::engine::rows) fn frozen_in(&self, directory: &Directory, id: RowId, columns: Option<&[usize]>) -> Result<Option<Row>> {
        let Some(entry) = directory.holding(id) else { return Ok(None) };
        let pages = self.group_pages()?;
        let group = self.group_of(entry)?;
        let Some(position) = group.position(&pages, id)? else { return Ok(None) };
        let types = self.format.types();
        let mut row = vec![Value::Null; types.len()];
        let mut read = |c: usize| -> Result<()> {
            if let (Some(slot), Some(t)) = (row.get_mut(c), types.get(c)) {
                *slot = group.value(&pages, (c, t), position)?;
            }
            Ok(())
        };
        match columns {
            Some(columns) => columns.iter().try_for_each(|&c| read(c))?,
            None => (0..types.len()).try_for_each(read)?,
        }
        Ok(Some(row))
    }

    /// Appends to `out` the frozen values of `columns` of row `id`, in
    /// their order, if a group `reader` sees holds it: whether one does,
    /// `out` as it was if not, or if a value cannot be read.
    pub(in crate::engine::rows) fn frozen_listed(&self, (id, columns): (RowId, &[usize]), reader: Reader, out: &mut Row) -> Result<bool> {
        let directory = self.frozen.directory()?;
        let seen = |e: &&std::sync::Arc<super::Entry>| e.stamp() == 0 || reader.sees(self.group_maker(e));
        let Some(entry) = directory.holding(id).filter(seen) else { return Ok(false) };
        let pages = self.group_pages()?;
        let group = self.group_of(entry)?;
        let Some(position) = group.position(&pages, id)? else { return Ok(false) };
        let types = self.format.types();
        let before = out.len();
        for &c in columns {
            let value = types.get(c).map_or(Ok(Value::Null), |t| group.value(&pages, (c, t), position));
            match value {
                Ok(value) => out.push(value),
                Err(e) => {
                    out.truncate(before);
                    return Err(e);
                }
            }
        }
        Ok(true)
    }
}

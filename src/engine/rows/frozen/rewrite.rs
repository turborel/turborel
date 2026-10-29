//! \[DESIGN\] Frozen row groups written again (RULES.md D31): a group whose
//! range of row ids the tree has entries for, of COLUMNSTORE.REWRITE_FRACTION
//! of its rows or more (frozen rows changed or deleted, or rows given ids
//! in its range late), is written again with them: each entry committed
//! before every snapshot folded in, a deletion dropping its row, an update
//! taking its values, a row added taking its place; the group is then
//! replaced in the directory, and the entries folded taken out of the tree
//! as a freeze takes out its rows' (`freeze.rs`). An entry not yet
//! committed so stays, before the group's values, which hold its row as it
//! was. Split from frozen.rs (RULES.md R-SIZE-2).

use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::Entry;
use crate::engine::columns::group::Group;
use crate::engine::columns::scan::rows;
use crate::engine::rows::options::Columns;
use crate::engine::rows::version::{Maker, Version};
use crate::engine::rows::{RowTable, key, row_id};
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::storage::{Row, RowId};
use crate::txn::Timestamp;

/// \[DESIGN\] The most groups one transaction's end writes again.
const MOST_REWRITES: usize = 2;

/// Whether `entries` of a group of `rows` rows are COLUMNSTORE.REWRITE_FRACTION
/// of them or more, as `columns` says.
fn due(entries: usize, rows: u32, columns: &Columns) -> bool {
    let entries = f64::from(u32::try_from(entries).unwrap_or(u32::MAX));
    columns.rewrite_fraction > 0.0 && entries >= columns.rewrite_fraction * f64::from(rows)
}

impl RowTable {
    /// Writes again the groups due, if enough frozen rows have been given
    /// entries since they were last looked at, folding in the entries
    /// committed at or before `cutoff`.
    pub(in crate::engine::rows) fn rewrite_due(&self, columns: Columns, cutoff: Timestamp) -> Result<()> {
        if !due(usize::try_from(self.frozen.shadowed.load(Ordering::Acquire)).unwrap_or(usize::MAX), columns.group_rows, &columns) {
            return Ok(());
        }
        self.frozen.shadowed.store(0, Ordering::Release);
        let directory = self.frozen.directory()?;
        let mut written = 0_usize;
        for entry in &directory.groups {
            if written >= MOST_REWRITES {
                // Looked at again at a later transaction's end.
                self.frozen.shadow();
                break;
            }
            let entries = self.entries_in(&entry.group)?;
            if due(entries.len(), entry.group.rows, &columns) {
                self.rewrite(entry, (entries, cutoff))?;
                written = written.saturating_add(1);
            }
        }
        Ok(())
    }

    /// The tree's entries in `group`'s range of row ids.
    fn entries_in(&self, group: &Group) -> Result<Vec<(RowId, Vec<u8>)>> {
        let (low, high) = (key(group.first), key(group.last));
        let mut found = Vec::new();
        for entry in self.tree.scan(Bound::Included(&low[..]), Bound::Included(&high[..]), Direction::Forward) {
            let (k, bytes) = entry?;
            found.push((row_id(&k)?, bytes));
        }
        Ok(found)
    }

    /// Writes `entry`'s group again, `entries` of its range committed at or
    /// before `cutoff` folded in.
    fn rewrite(&self, entry: &Arc<Entry>, (entries, cutoff): (Vec<(RowId, Vec<u8>)>, Timestamp)) -> Result<()> {
        let pages = self.group_pages()?;
        let types = self.format.types();
        let all: Vec<usize> = (0..types.len()).collect();
        let old_ids = entry.group.row_ids(&pages)?;
        let frozen = rows(&entry.group, &pages, types, &all, (0, entry.group.rows))?;
        // Each row id's row, or none if deleted: the group's, then the
        // entries folded in over them.
        let mut kept: BTreeMap<RowId, Option<Row>> = old_ids.into_iter().zip(frozen.into_iter().map(Some)).collect();
        let mut folded = Vec::new();
        for (id, bytes) in entries {
            let version = Version::parse(&bytes)?;
            let Maker::Committed(at) = version.maker else { continue };
            if at > cutoff {
                continue;
            }
            let row = if version.deleted { None } else { Some(self.decode(id, version.record, None)?) };
            kept.insert(id, row);
            folded.push((id, bytes));
        }
        let (new_ids, new_rows): (Vec<RowId>, Vec<Row>) = kept.into_iter().filter_map(|(id, row)| row.map(|r| (id, r))).unzip();
        let group = if new_ids.is_empty() { None } else { Some(Group::write(&pages, types, &new_ids, &new_rows)?) };
        self.publish((Some(entry), group), folded)
    }
}

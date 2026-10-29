//! \[DESIGN\] Frozen row groups written again (RULES.md D31): a group whose
//! range of row ids the tree has entries for, of COLUMNSTORE.REWRITE_FRACTION
//! of its rows or more (frozen rows changed or deleted, or rows given ids
//! in its range late), is written again with them: each entry committed
//! before every snapshot folded in, a deletion dropping its row, an update
//! taking its values, a row added taking its place; the group is then
//! replaced in the directory, and the entries folded taken out of the tree
//! as a freeze takes out its rows' (`freeze.rs`). An entry not yet
//! committed so stays, before the group's values, which hold its row as it
//! was. A group whose rows are sorted by other columns than COLUMNSTORE.ORDER
//! names now, as it was changed, is written again so, sorted as it says,
//! MOST_REWRITES of them a call, as the maintenance thread calls again
//! while any is left (`unsorted`). Split from frozen.rs (RULES.md
//! R-SIZE-2).

use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::Entry;
use super::freeze::ordered;
use crate::engine::columns::group::Group;
use crate::engine::columns::scan::rows;
use crate::engine::rows::options::{Columns, Order};
use crate::engine::rows::version::{Maker, Version};
use crate::engine::rows::{RowTable, key, row_id};
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::storage::{Row, RowId};
use crate::txn::Timestamp;

/// \[DESIGN\] The most groups one call writes again.
const MOST_REWRITES: usize = 2;

/// Whether one of `groups` is sorted by other columns than `columns`'s
/// COLUMNSTORE.ORDER names.
pub(super) fn unsorted(groups: &[Arc<Entry>], columns: &Columns) -> bool {
    groups.iter().any(|entry| entry.group.order.as_slice() != columns.order.places())
}

/// Whether `entries` of a group of `rows` rows are COLUMNSTORE.REWRITE_FRACTION
/// of them or more, as `columns` says.
pub(super) fn due(entries: usize, rows: u32, columns: &Columns) -> bool {
    let entries = f64::from(u32::try_from(entries).unwrap_or(u32::MAX));
    columns.rewrite_fraction > 0.0 && entries >= columns.rewrite_fraction * f64::from(rows)
}

impl RowTable {
    /// Writes again the groups due, if enough frozen rows have been given
    /// entries since they were last looked at, folding in the entries
    /// committed at or before `cutoff`: whether it wrote any.
    pub(in crate::engine::rows) fn rewrite_due(&self, columns: Columns, cutoff: Timestamp) -> Result<bool> {
        let shadowed =
            due(usize::try_from(self.frozen.shadowed.load(Ordering::Acquire)).unwrap_or(usize::MAX), columns.group_rows, &columns);
        let directory = self.frozen.directory()?;
        if !shadowed && !unsorted(&directory.groups, &columns) {
            return Ok(false);
        }
        if shadowed {
            self.frozen.shadowed.store(0, Ordering::Release);
        }
        let mut written = 0_usize;
        for entry in &directory.groups {
            if written >= MOST_REWRITES {
                // Looked at again at a later call.
                self.frozen.shadow();
                break;
            }
            let resorted = entry.group.order.as_slice() != columns.order.places();
            if !shadowed && !resorted {
                continue;
            }
            let entries = self.entries_in(&entry.group)?;
            if resorted || due(entries.len(), entry.group.rows, &columns) {
                self.rewrite(entry, (entries, cutoff), &columns.order)?;
                written = written.saturating_add(1);
            }
        }
        Ok(written > 0)
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
    /// before `cutoff` folded in, its rows sorted as `order` says.
    fn rewrite(&self, entry: &Arc<Entry>, (entries, cutoff): (Vec<(RowId, Vec<u8>)>, Timestamp), order: &Order) -> Result<()> {
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
        let (new_ids, new_rows) = ordered(order, kept.into_iter().filter_map(|(id, row)| row.map(|r| (id, r))).collect());
        let group = if new_ids.is_empty() { None } else { Some(Group::write(&pages, types, (&new_ids, &new_rows), order.places())?) };
        self.publish((Some(entry), group), folded)
    }
}

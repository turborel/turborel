//! \[DESIGN\] The bytes a row table takes, as TABLE_STATISTICS shows them
//! (`TableStorage::sizes`, RULES.md D522). Its frozen groups' pages are
//! counted (`rows/frozen.rs`), each group's from its header, held or read;
//! the rows of its tree, which shares its pages with the store's other
//! tables and indexes, are estimated as their number times the mean width
//! its statistics last measured, none if they never did.

use super::RowTable;
use crate::storage::{TableSizes, TableStorage};

impl RowTable {
    /// Its bytes: those of its frozen groups' pages, and those of the rows
    /// of its tree, estimated; none if a group cannot be read, or the rows
    /// of its tree have no width measured.
    pub(super) fn measured(&self) -> Option<TableSizes> {
        let directory = self.frozen.directory().ok()?;
        let (_, page) = self.pool.page_count(self.file).ok()?;
        let page = u64::try_from(page).ok()?;
        let mut frozen = 0_u64;
        for entry in &directory.groups {
            let pages = u64::try_from(self.group_of(entry).ok()?.pages().len()).ok()?;
            frozen = frozen.saturating_add(pages.saturating_mul(page));
        }
        let live = u64::try_from(self.row_count()).ok()?;
        let in_tree = live.saturating_sub(directory.rows());
        if in_tree == 0 {
            return Some(TableSizes { bytes: frozen, estimated: false });
        }
        let width = self.statistics()?.width(&[])?;
        Some(TableSizes { bytes: frozen.saturating_add(estimate(in_tree, width)), estimated: true })
    }
}

/// `rows` rows of `width` bytes each, rounded up.
fn estimate(rows: u64, width: f64) -> u64 {
    let rows = f64::from(u32::try_from(rows).unwrap_or(u32::MAX));
    let bytes = crate::physical::parallel_rows((rows * width.max(0.0_f64)).ceil());
    u64::try_from(bytes).unwrap_or(u64::MAX)
}

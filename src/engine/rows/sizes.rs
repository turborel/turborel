//! \[DESIGN\] The bytes a row table takes, as TABLE_STATISTICS and
//! COLUMN_STORAGE show them (`TableStorage::sizes`, RULES.md D522, D1165).
//! Its frozen groups' pages are counted (`rows/frozen.rs`), each group's
//! from its header, held or read, and so are its tree's
//! (`BLinkTree::pages`), its nodes above the leaves read; the pages of its
//! long values, its zones and its statistics are not.
//!
//! Of each column, if asked, the chunks of each encoding, each chunk's
//! kind and bytes read from the chunk itself (`columns/encoded.rs`), so
//! every group shows them, whichever build wrote it: the rows they hold,
//! their bytes and the bytes of the pages they take; and of the rows of
//! its tree their number, no bytes: a tree keeps each row whole, so no
//! column of it takes bytes of its own, and the tree's are the table's.
//!
//! Every sum is checked. One past what a count holds (`BIGINT`) is none,
//! never a bound reached and shown as bytes.

use std::collections::BTreeMap;

use super::RowTable;
use crate::engine::columns::group::{Pages, Zone};
use crate::engine::columns::kind_name;
use crate::engine::rows::sealing::Sealing;
use crate::error::Result;
use crate::storage::{ColumnSizes, TableSizes, TableStorage};

impl RowTable {
    /// Its bytes, and its columns' if `by_column`; none if a group, a chunk
    /// or a node of its tree cannot be read, or its bytes pass a count's.
    pub(super) fn measured(&self, by_column: bool) -> Option<TableSizes> {
        let pages = self.group_pages().ok()?;
        let directory = self.frozen.directory().ok()?;
        let (_, page) = self.pool.page_count(self.file).ok()?;
        let page = u64::try_from(page).ok()?;
        let mut frozen = 0_u64;
        let mut held: BTreeMap<(usize, Option<&'static str>), ColumnSizes> = BTreeMap::new();
        for entry in &directory.groups {
            let group = self.group_of(entry).ok()?;
            frozen = within(frozen.checked_add(u64::try_from(group.pages().len()).ok()?.checked_mul(page)?))?;
            for (column, zones) in group.columns.iter().enumerate().filter(|_| by_column) {
                for zone in zones {
                    add(&mut held, (column, zone), (&pages, page)).ok()?;
                }
            }
        }
        if !by_column {
            return Some(TableSizes {
                bytes: within(frozen.checked_add(self.tree.pages().ok()?.checked_mul(page)?))?,
                ..TableSizes::default()
            });
        }
        let bytes = within(frozen.checked_add(self.tree.pages().ok()?.checked_mul(page)?))?;
        let in_tree = u64::try_from(self.row_count()).ok()?.saturating_sub(directory.rows());
        let mut columns: Vec<ColumnSizes> = held.into_values().collect();
        if in_tree > 0 {
            for column in 0..self.format.columns() {
                columns.push(ColumnSizes { column, encoding: Some("ROW"), chunks: None, rows: in_tree, bytes: None, page_bytes: None });
            }
            columns.sort_by_key(|c| c.column);
        }
        Some(TableSizes { bytes, estimated: false, columns })
    }
}

/// Adds `zone`, a chunk of the column at `column`, to what `columns` holds
/// of its encoding, its kind and bytes read from `pages`, a chunk on a page
/// of its own taking the `page` bytes.
///
/// # Errors
///
/// As `Pages::with_run`.
fn add(
    columns: &mut BTreeMap<(usize, Option<&'static str>), ColumnSizes>,
    (column, zone): (usize, &Zone),
    (pages, page): (&Pages<'_, Sealing>, u64),
) -> Result<()> {
    let (kind, bytes) = pages.with_run(&zone.run, |chunk| Ok((chunk.first().copied(), chunk.len())))?;
    let encoding = kind.and_then(kind_name);
    let sizes = columns.entry((column, encoding)).or_insert(ColumnSizes {
        column,
        encoding,
        chunks: Some(0),
        rows: 0,
        bytes: Some(0),
        page_bytes: Some(0),
    });
    sizes.chunks = sizes.chunks.map(|n| n.saturating_add(1));
    sizes.rows = sizes.rows.saturating_add(u64::from(zone.run.rows));
    sizes.bytes = sizes.bytes.and_then(|n| within(n.checked_add(u64::try_from(bytes).ok()?)));
    let taken = if zone.run.len == 0 { page } else { u64::from(zone.run.len) };
    sizes.page_bytes = sizes.page_bytes.and_then(|n| within(n.checked_add(taken)));
    Ok(())
}

/// `n`, if a count (`BIGINT`) holds it.
fn within(n: Option<u64>) -> Option<u64> {
    n.filter(|n| i64::try_from(*n).is_ok())
}

#[cfg(test)]
mod tests;

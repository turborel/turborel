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
//! their bytes and the bytes of the pages they take; and its values in the
//! rows of the tree, their number times the column's mean width its
//! statistics last measured, none if they never did.

use std::collections::BTreeMap;

use super::RowTable;
use crate::engine::columns::group::{Pages, Zone};
use crate::engine::columns::kind_name;
use crate::engine::rows::sealing::Sealing;
use crate::error::Result;
use crate::stats::Statistics;
use crate::storage::{ColumnSizes, TableSizes, TableStorage};

impl RowTable {
    /// Its bytes, and its columns' if `by_column`; none if a group, a chunk
    /// or a node of its tree cannot be read.
    pub(super) fn measured(&self, by_column: bool) -> Option<TableSizes> {
        let pages = self.group_pages().ok()?;
        let directory = self.frozen.directory().ok()?;
        let (_, page) = self.pool.page_count(self.file).ok()?;
        let page = u64::try_from(page).ok()?;
        let mut frozen = 0_u64;
        let mut held: BTreeMap<(usize, Option<&'static str>), ColumnSizes> = BTreeMap::new();
        for entry in &directory.groups {
            let group = self.group_of(entry).ok()?;
            frozen = frozen.saturating_add(u64::try_from(group.pages().len()).ok()?.saturating_mul(page));
            for (column, zones) in group.columns.iter().enumerate().filter(|_| by_column) {
                for zone in zones {
                    add(&mut held, (column, zone), (&pages, page)).ok()?;
                }
            }
        }
        if !by_column {
            return Some(TableSizes {
                bytes: frozen.saturating_add(self.tree.pages().ok()?.saturating_mul(page)),
                ..TableSizes::default()
            });
        }
        let tree = self.tree.pages().ok()?.saturating_mul(page);
        let in_tree = u64::try_from(self.row_count()).ok()?.saturating_sub(directory.rows());
        let mut columns: Vec<ColumnSizes> = held.into_values().collect();
        if in_tree > 0 {
            // The last pass's, read as no plan reads them, so showing sizes
            // makes no pass wanted (RULES.md D473).
            let stats = self.gathered.get();
            for column in 0..self.format.columns() {
                let width = stats.as_ref().and_then(|s| s.width(&[column]));
                let bytes = width.map(|w| estimate(in_tree, w));
                columns.push(ColumnSizes { column, encoding: Some("ROW"), chunks: None, rows: in_tree, bytes, page_bytes: None });
            }
            columns.sort_by_key(|c| c.column);
        }
        Some(TableSizes { bytes: frozen.saturating_add(tree), estimated: false, columns })
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
    sizes.bytes = sizes.bytes.map(|n| n.saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX)));
    let taken = if zone.run.len == 0 { page } else { u64::from(zone.run.len) };
    sizes.page_bytes = sizes.page_bytes.map(|n| n.saturating_add(taken));
    Ok(())
}

/// `rows` rows of `width` bytes each, rounded up.
fn estimate(rows: u64, width: f64) -> u64 {
    let rows = f64::from(u32::try_from(rows).unwrap_or(u32::MAX));
    let bytes = crate::physical::parallel_rows((rows * width.max(0.0_f64)).ceil());
    u64::try_from(bytes).unwrap_or(u64::MAX)
}

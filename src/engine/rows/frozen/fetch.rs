//! \[DESIGN\] (RULES.md D513) Rows fetched by id in batches, as an index
//! scan reads them. A row the tree holds is read from it, as `get` reads
//! it. The frozen ones are taken by group, their places in it found, and
//! each column read at those places in order (`values_at`), so a chunk
//! is opened once for every row of it fetched, not once a value, and the
//! directory and each group once a batch. Of fewer than eight rows to a
//! chunk, each value is read alone, as `get` reads it.

use crate::engine::columns::prepared::Prepared;
use crate::engine::columns::scan::values_at;
use crate::engine::rows::{RowTable, key};
use crate::error::Result;
use crate::storage::{Near, Row, RowId};
use crate::txn::TxId;
use crate::value::Value;

/// The rows to a chunk, on average, from which a batch reads a column's
/// values chunk by chunk, each chunk prepared once.
const DENSE: usize = 8;

impl RowTable {
    /// Rows `ids` as `tx` sees them, in their order, `columns` decoded,
    /// or all, as `get_of` reads each.
    pub(in crate::engine::rows) fn get_many_of(
        &self,
        tx: TxId,
        ids: &[RowId],
        columns: Option<&[usize]>,
        near: &mut Near,
    ) -> Result<Vec<Option<Row>>> {
        let mut rows = Vec::with_capacity(ids.len());
        let mut frozen = Vec::new();
        for (n, &id) in ids.iter().enumerate() {
            let reader = self.read_row(tx, id, near)?;
            if self.tree.get_near(&key(id), near)? {
                rows.push(self.visible_in(&near.bytes, reader)?.map(|record| self.decode(id, &record, columns)).transpose()?);
            } else {
                // Frozen, so committed before every snapshot (`frozen.rs`).
                rows.push(None);
                frozen.push((id, n));
            }
        }
        self.frozen_many(&mut frozen, columns, &mut rows)?;
        Ok(rows)
    }

    /// Puts in `rows` at its place each row of `frozen`, an id and its
    /// place, a group holds, `columns` of it or all; group by group.
    fn frozen_many(&self, frozen: &mut [(RowId, usize)], columns: Option<&[usize]>, rows: &mut [Option<Row>]) -> Result<()> {
        if frozen.is_empty() {
            return Ok(());
        }
        frozen.sort_unstable();
        let directory = self.frozen.directory()?;
        let mut rest: &[(RowId, usize)] = frozen;
        while let Some(&(first, _)) = rest.first() {
            let Some(entry) = directory.holding(first) else {
                rest = rest.get(1..).unwrap_or_default();
                continue;
            };
            let last = entry.summary.last;
            let (these, after) = rest.split_at(rest.partition_point(|&(id, _)| id <= last));
            rest = after;
            self.of_group(&*self.group_of(entry)?, these, columns, rows)?;
        }
        Ok(())
    }

    /// As `frozen_many`, of the rows `these` of `group`.
    fn of_group(
        &self,
        group: &crate::engine::columns::group::Group,
        these: &[(RowId, usize)],
        columns: Option<&[usize]>,
        rows: &mut [Option<Row>],
    ) -> Result<()> {
        let pages = self.group_pages()?;
        let mut at = Vec::with_capacity(these.len());
        for &(id, n) in these {
            if let Some(position) = group.position(&pages, id)? {
                at.push((position, n));
            }
        }
        at.sort_unstable();
        let positions: Vec<u32> = at.iter().map(|&(p, _)| p).collect();
        let types = self.format.types();
        for &(_, n) in &at {
            if let Some(slot) = rows.get_mut(n) {
                *slot = Some(vec![Value::Null; types.len()]);
            }
        }
        let all: Vec<usize> = (0..types.len()).collect();
        let mut prepared = Prepared::default();
        for &c in columns.unwrap_or(&all) {
            let Some(t) = types.get(c) else { continue };
            // Of few rows to a chunk, each value read alone, as `get` reads
            // it: preparing a chunk for one value costs more than it saves.
            let chunks = group.columns.get(c).map_or(0, Vec::len);
            let values = if positions.len() >= chunks.saturating_mul(DENSE) {
                values_at(group, (&pages, &mut prepared), (c, t), &positions)?
            } else {
                positions.iter().map(|&p| group.value(&pages, (c, t), p)).collect::<Result<Vec<_>>>()?
            };
            for (&(_, n), value) in at.iter().zip(values) {
                if let Some(slot) = rows.get_mut(n).and_then(Option::as_mut).and_then(|row| row.get_mut(c)) {
                    *slot = value;
                }
            }
        }
        Ok(())
    }
}

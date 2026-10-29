//! \[DESIGN\] The row store's side of recovery (RULES.md D25, D31). After a
//! crash a table is reopened as its last checkpoint left it, which may hold
//! versions made by transactions open then: they name older versions in
//! the undo store, which did not survive the process. So a table opened
//! counts its rows by their newest versions alone, and gathers its
//! statistics only when first asked, once recovery is done; and recovery
//! writes each row's newest version outright, committed, never reading the
//! version it replaces (`apply`).

use std::ops::Bound;
use std::sync::atomic::Ordering;

use super::version::{Maker, Version};
use super::{RowTable, key, row_id};
use crate::engine::tree::Direction;
use crate::error::Result;
use crate::storage::{Row, RowId};

impl RowTable {
    /// The rows whose newest version is not a deletion: the tree's, and
    /// the frozen rows no entry of the tree stands for.
    pub(super) fn count_newest(&self) -> Result<usize> {
        let directory = self.frozen.directory()?;
        let mut rows = usize::try_from(directory.rows()).unwrap_or(usize::MAX);
        for entry in self.tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward) {
            let (k, newest) = entry?;
            if directory.holding(row_id(&k)?).is_some() {
                rows = rows.saturating_sub(1);
            }
            if !Version::parse(&newest)?.deleted {
                rows = rows.saturating_add(1);
            }
        }
        Ok(rows)
    }

    /// Makes row `id` hold `row`, or be absent, as a version committed
    /// before every snapshot, whatever version is there; ids given from now
    /// on are past it.
    pub(super) fn apply_newest(&self, id: RowId, row: Option<Row>) -> Result<()> {
        self.next.fetch_max(id.saturating_add(1), Ordering::AcqRel);
        self.blocks.clear();
        self.reserve_past(id)?;
        let old = match row {
            Some(row) => self.put_newest(id, &row)?,
            None => self.remove_newest(id)?,
        };
        if let Some(old) = old {
            self.free_values(id, Version::parse(&old)?.record);
        }
        Ok(())
    }

    /// Makes `row` row `id`'s only version, committed; the version it
    /// replaced, if any.
    fn put_newest(&self, id: RowId, row: &Row) -> Result<Option<Vec<u8>>> {
        let k = key(id);
        let (record, pages) = self.encode(id, row)?;
        let version = Version { maker: Maker::Committed(0), deleted: false, older: None, record: &record }.encode();
        let written = self.tree.replace(&k, &version).and_then(|old| match old {
            Some(old) => Ok(Some(old)),
            None => self.tree.insert(&k, &version).map(|_| None),
        });
        let old = written.inspect_err(|_| self.free(&pages))?;
        // A frozen row with no entry was there already.
        let was = match &old {
            Some(o) => Version::parse(o).is_ok_and(|v| !v.deleted),
            None => self.frozen_row(id, Some(&[]))?.is_some(),
        };
        if !was {
            self.live.fetch_add(1, Ordering::AcqRel);
        }
        Ok(old)
    }

    /// Removes row `id`; its version, if it had one. A frozen row keeps a
    /// deletion committed before every snapshot (`rows/frozen.rs`).
    fn remove_newest(&self, id: RowId) -> Result<Option<Vec<u8>>> {
        let k = key(id);
        let frozen = self.frozen_row(id, Some(&[]))?.is_some();
        let old = if frozen {
            let deletion = Version { maker: Maker::Committed(0), deleted: true, older: None, record: &[] }.encode();
            let old = self.tree.replace(&k, &deletion)?;
            if old.is_none() {
                self.tree.insert(&k, &deletion)?;
                self.frozen.shadow();
            }
            old
        } else {
            self.tree.delete(&k)?
        };
        let was = match &old {
            Some(o) => Version::parse(o).is_ok_and(|v| !v.deleted),
            None => frozen,
        };
        if was {
            self.live.fetch_sub(1, Ordering::AcqRel);
        }
        Ok(old)
    }
}

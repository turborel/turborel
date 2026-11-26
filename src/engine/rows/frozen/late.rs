//! \[DESIGN\] (RULES.md D361) The source a column store's late columns are
//! decoded of. It holds the frozen group a scan read them of, with the
//! directory's entry of it, so its pages are not freed while a batch holding its rows
//! lives (`frozen.rs`, `Entry`), as the scan's own directory holds them. A
//! group is replaced, never changed, so its rows decoded later are those
//! the scan would have decoded then.

use std::sync::{Arc, Mutex};

use super::Entry;
use crate::engine::columns::group::Group;
use crate::engine::columns::prepared::Prepared;
use crate::engine::columns::scan::{column_at, liked_at};
use crate::engine::rows::RowTable;
use crate::error::Result;
use crate::storage::{Column, LateSource};
use crate::value::{LikePattern, Value};

/// \[DESIGN\] A frozen group, the source of late columns of its rows.
pub(super) struct FrozenLate {
    table: Arc<RowTable>,
    /// Held so the group's pages are not freed while it is.
    _entry: Arc<Entry>,
    group: Arc<Group>,
    /// The chunks its last decoding opened, taken by the next one, so the
    /// batches of a scan, decoded in its order, open each chunk about
    /// once (`prepared.rs`). Taken out to decode, never held locked
    /// across a page's unsealing (RULES.md R-ERR-4).
    prepared: Mutex<Option<Prepared>>,
}

impl FrozenLate {
    /// The group `group` of `table`, `entry` its directory's.
    pub(super) fn new(table: Arc<RowTable>, entry: Arc<Entry>, group: Arc<Group>) -> FrozenLate {
        FrozenLate { table, _entry: entry, group, prepared: Mutex::new(None) }
    }
}

impl LateSource for FrozenLate {
    /// The chunks the last decoding opened read again, or, if another
    /// thread decodes at once, opened anew.
    fn decode(&self, column: usize, positions: &[u32]) -> Result<Column> {
        let pages = self.table.group_pages()?;
        let Some(t) = self.table.format.types().get(column) else { return Ok(Column::from(vec![Value::Null; positions.len()])) };
        let mut prepared = self.prepared.lock().ok().and_then(|mut held| held.take()).unwrap_or_default();
        let decoded = column_at(&self.group, (&pages, &mut prepared), (column, t), positions);
        if let Ok(mut held) = self.prepared.lock() {
            *held = Some(prepared);
        }
        decoded
    }

    /// \[DESIGN\] (RULES.md D1122) Judged of the chunks' codes
    /// (`columns/scan/liked.rs`), as `decode` reads them.
    fn like(&self, column: usize, positions: &[u32], pattern: &LikePattern) -> Result<Option<Vec<Option<bool>>>> {
        let pages = self.table.group_pages()?;
        let Some(t) = self.table.format.types().get(column) else { return Ok(None) };
        let mut prepared = self.prepared.lock().ok().and_then(|mut held| held.take()).unwrap_or_default();
        let judged = liked_at(&self.group, (&pages, &mut prepared), (column, t), (positions, pattern));
        if let Ok(mut held) = self.prepared.lock() {
            *held = Some(prepared);
        }
        judged
    }
}

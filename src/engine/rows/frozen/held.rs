//! \[DESIGN\] (RULES.md D1173) A group's header as the table holds it,
//! read for the columns its reader asks for (`group/wanted.rs`).

use std::sync::Arc;

use super::Entry;
use crate::engine::columns::group::{self, Group};
use crate::engine::rows::RowTable;
use crate::error::Result;

impl RowTable {
    /// The group of `entry`, read for the columns `wanted`, or for every
    /// column if none is given (RULES.md D1173): its header as held if it
    /// was read for them, else read for them and those it was read for,
    /// and held in its place.
    ///
    /// # Errors
    ///
    /// As `group::read`.
    pub(in crate::engine::rows) fn group_of(&self, entry: &Entry, wanted: Option<&[usize]>) -> Result<Arc<Group>> {
        let held = entry.slot.get();
        if let Some(group) = held.as_ref().filter(|g| g.holds(wanted)) {
            return Ok(Arc::clone(group));
        }
        let all = match (&held, wanted) {
            (Some(group), Some(wanted)) => Some(group.with_columns(wanted)),
            (None, Some(wanted)) => Some(wanted.to_vec()),
            (_, None) => None,
        };
        let group = Arc::new(group::read(&self.pages_of(entry)?, entry.summary.header, self.format.types(), all.as_deref())?);
        self.headers.put(entry.header_key(), &entry.slot, &group);
        Ok(group)
    }
}

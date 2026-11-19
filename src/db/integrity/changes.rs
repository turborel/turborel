//! The changes a statement makes, noted in its effects as each is made:
//! for its constraints, its triggers' transition tables and its delta
//! table [ISO/IEC 9075-2:2023 §7.6 GR5] (RULES.md D174).

use super::{Effect, Effects};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::storage::{Row, RowId};

impl Effects<'_> {
    /// Of a delta table's statement, the row of `table` changed from `old`
    /// to `new` recorded; a change of a table none may change refused.
    fn noted(&self, table: &QualifiedName, old: Option<&Row>, new: Option<&Row>) -> Result<()> {
        if let Some((names, state)) = &self.restricted
            && names.contains(table)
        {
            return Err(crate::db::delta::restricted(table, *state));
        }
        self.delta.as_ref().map_or(Ok(()), |c| c.record(table, old, new))
    }

    /// Row `id` of `table`, `row`, inserted: a transition of its state
    /// change, if a trigger may read it [ISO/IEC 9075-2:2023 §15.11 GR5].
    ///
    /// # Errors
    ///
    /// The spill area's, SQLCODE -918 if it cannot be reserved.
    pub(in crate::db) fn inserted(&mut self, table: &QualifiedName, id: RowId, row: &Row) -> Result<()> {
        self.noted(table, None, Some(row))?;
        if let Some(t) = &mut self.triggered {
            t.record(table, &[], &(id, Row::new(), row.clone()))?;
        }
        self.done.push(Effect::Inserted { table: table.clone(), id })
    }

    /// Row `id` of `table` updated from `old` to `new`, giving values to
    /// `columns`: a transition of its state change, if a trigger may read
    /// it [ISO/IEC 9075-2:2023 §15.14 GR8].
    ///
    /// # Errors
    ///
    /// As `inserted`.
    pub(in crate::db) fn updated(&mut self, table: &QualifiedName, id: RowId, (old, new): (Row, &Row), columns: &[usize]) -> Result<()> {
        self.noted(table, Some(&old), Some(new))?;
        if let Some(t) = &mut self.triggered {
            t.record(table, columns, &(id, old.clone(), new.clone()))?;
        }
        self.done.push(Effect::Updated { table: table.clone(), id, old, columns: columns.to_vec() })
    }

    /// \[EXTENSION\] Row `id` of the partition `from`, `old`, moved by an
    /// UPDATE giving values to `columns` to the partition `to`, as its row
    /// `moved`, `new`: deleted from one and inserted in the other, each
    /// checked so, and of the tables both are partitions of, an update
    /// (RULES.md D155).
    ///
    /// # Errors
    ///
    /// As `inserted`.
    pub(in crate::db) fn moved(
        &mut self,
        (from, id, old): (&QualifiedName, RowId, Row),
        (to, moved, new): (&QualifiedName, RowId, &Row),
        columns: &[usize],
    ) -> Result<()> {
        // [EXTENSION] of a delta table, one row updated (RULES.md D174).
        self.noted(to, Some(&old), Some(new))?;
        if let Some(t) = &mut self.triggered {
            t.record_moved((from, to), columns, (moved, &old, new))?;
        }
        self.moved_in.insert((to.clone(), moved));
        self.done.push(Effect::Deleted { table: from.clone(), id, old })?;
        self.done.push(Effect::Inserted { table: to.clone(), id: moved })
    }

    /// Whether row `id` of `table` is one it moved there.
    pub(in crate::db) fn moved_in(&self, table: &QualifiedName, id: RowId) -> bool {
        self.moved_in.contains(&(table.clone(), id))
    }

    /// Row `id` of `table`, `old`, deleted: a transition of its state
    /// change, if a trigger may read it [ISO/IEC 9075-2:2023 §15.8 GR3].
    ///
    /// # Errors
    ///
    /// As `inserted`.
    pub(in crate::db) fn deleted(&mut self, table: &QualifiedName, id: RowId, old: Row) -> Result<()> {
        self.noted(table, Some(&old), None)?;
        if let Some(t) = &mut self.triggered {
            t.record(table, &[], &(id, old.clone(), Row::new()))?;
        }
        self.done.push(Effect::Deleted { table: table.clone(), id, old })
    }
}

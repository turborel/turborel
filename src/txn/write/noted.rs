//! What a transaction notes of the rows it writes, for its statements'
//! ends: the row it wrote of each form's change to an assertion's kept
//! totals (`db/assertions/kept.rs`), and the rows whose entry in a unique
//! index was its key's only one (`db/assignment.rs`).

use std::collections::HashSet;

use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::storage::RowId;
use crate::txn::Txn;

/// Notes in `noted` each row of `rows` that was alone, and takes out each
/// that was not.
fn note_rows(noted: &mut HashSet<RowId>, rows: &mut dyn Iterator<Item = (RowId, bool)>) {
    for (row, alone) in rows {
        if alone {
            noted.insert(row);
        } else {
            noted.remove(&row);
        }
    }
}

impl Txn {
    /// The row it wrote of form `form`'s change to the kept totals of
    /// `table`, if it wrote one; a statement undone may have taken it.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the transaction's state is poisoned.
    pub(crate) fn kept_row(&self, table: &QualifiedName, form: usize) -> Result<Option<RowId>> {
        Ok(self.state()?.kept_rows.get(&(table.clone(), form)).copied())
    }

    /// Notes `row` as the one it writes of form `form`'s change to the kept
    /// totals of `table`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the transaction's state is poisoned.
    pub(crate) fn note_kept_row(&self, table: &QualifiedName, form: usize, row: RowId) -> Result<()> {
        self.state()?.kept_rows.insert((table.clone(), form), row);
        Ok(())
    }

    /// Notes whether row `row`'s entry in the unique index `index`, as it
    /// was inserted, was its key's only one.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the transaction's state is poisoned.
    pub(crate) fn note_alone(&self, index: &str, row: RowId, alone: bool) -> Result<()> {
        self.note_alone_rows(index, &mut std::iter::once((row, alone)))
    }

    /// As `note_alone` of each row and whether it was alone, of `rows`, the
    /// transaction's state taken once.
    ///
    /// # Errors
    ///
    /// As `note_alone`.
    pub(crate) fn note_alone_rows(&self, index: &str, rows: &mut dyn Iterator<Item = (RowId, bool)>) -> Result<()> {
        let mut state = self.state()?;
        // Found by one hash of its name; the name is copied only the first time.
        if let Some(noted) = state.alone.get_mut(index) {
            note_rows(noted, rows);
            return Ok(());
        }
        let mut noted = HashSet::new();
        note_rows(&mut noted, rows);
        state.alone.insert(index.to_owned(), noted);
        Ok(())
    }

    /// Whether row `row`'s entry in the unique index `index`, as it was
    /// last inserted, was its key's only one: another row of its key,
    /// inserted after, was not, and is looked for itself.
    pub(crate) fn alone(&self, index: &str, row: RowId) -> bool {
        self.state().is_ok_and(|s| s.alone.get(index).is_some_and(|rows| rows.contains(&row)))
    }
}

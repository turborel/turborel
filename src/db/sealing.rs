//! \[EXTENSION\] A table's encodings, as the database applies them (RULES.md
//! D41, D48): its option ENCODINGS, the database's own, of every table of
//! any engine. Its rows' log records are sealed through them
//! (`log/sealed.rs`), and an engine that seals its storage
//! (`StorageEngine::seals`) seals its rows too; a table of an engine that
//! does not cannot name any.
//!
//! An encryption hides a table's rows, but an index's keys stay in order,
//! in the clear: so a table whose encodings are confidential may have an
//! index only where the database's files are encrypted too
//! (STORAGE.ENCODINGS), or kept nowhere.

use std::sync::Arc;

use super::State;
use crate::catalog::{QualifiedName, TableDesc};
use crate::disk::encoding::Pipeline;
use crate::error::{Code, Result, SqlError};
use crate::storage::{ENCODINGS, encodings_of};
use crate::sync::lock_builtin;

/// The name the database declares its own table options under.
pub(crate) const DATABASE_OPTIONS: &str = "database";

fn refused(message: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {message}"))
}

impl State {
    /// The pipeline the table `desc` describes passes its rows through, if
    /// it names any: made once for each list of names.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an encoding it names is not registered, or refuses
    /// its argument.
    pub(crate) fn seal_of_desc(&self, desc: &TableDesc) -> Result<Option<Arc<Pipeline>>> {
        let names = encodings_of(&desc.options);
        if names.is_empty() {
            return Ok(None);
        }
        if let Some(made) = lock_builtin(&self.seals).get(names) {
            return Ok(Some(Arc::clone(made)));
        }
        let made = Arc::new(self.encodings.named(names)?);
        lock_builtin(&self.seals).insert(names.to_owned(), Arc::clone(&made));
        Ok(Some(made))
    }

    /// As `seal_of_desc`, for the table `table`; none if it is not one.
    ///
    /// # Errors
    ///
    /// As `seal_of_desc`.
    pub(crate) fn seal_of(&self, table: &QualifiedName) -> Result<Option<Arc<Pipeline>>> {
        match self.catalog.table(table) {
            Some(desc) => self.seal_of_desc(desc),
            None => Ok(None),
        }
    }

    /// The encodings spill areas' files pass through
    /// (`TempFiles::pipeline`): the first confidential encoding the
    /// database's files or a table use, with no argument, its ephemeral key;
    /// none if nothing is encrypted.
    pub(crate) fn spill_encodings(&self) -> Pipeline {
        let files = self.created.then(|| self.layout(&self.settings).ok()).flatten().map(|l| l.formats.pipeline);
        let tables: Vec<Arc<Pipeline>> = self.catalog.tables().into_iter().filter_map(|t| self.seal_of_desc(t).ok().flatten()).collect();
        let used = files.iter().chain(tables.iter().map(|p| &**p)).flat_map(Pipeline::stages).find(|s| s.confidential()).map(|s| s.id());
        let unbound = used.and_then(|id| self.encodings.all().ok()?.into_iter().find(|e| e.id() == id));
        unbound.and_then(|e| Pipeline::new(vec![e]).ok()).unwrap_or_default()
    }

    /// Whether the database's files are encrypted, or kept nowhere: where
    /// an index of an encrypted table may be.
    fn files_hidden(&self) -> Result<bool> {
        if !self.created {
            return Ok(true);
        }
        Ok(self.layout(&self.settings)?.formats.pipeline.confidential())
    }

    /// Refuses `desc`, a table defined or changed, if it names encodings its
    /// engine does not seal its storage through, or confidential ones while
    /// it has an index where the database's files are not encrypted.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, saying why.
    pub(crate) fn check_encodings(&self, desc: &TableDesc) -> Result<()> {
        if encodings_of(&desc.options).is_empty() {
            return Ok(());
        }
        let seals = self.engines.get(&desc.engine).is_some_and(|e| e.seals());
        if !seals {
            return Err(refused(&format!("the engine {} does not pass its tables' rows through {ENCODINGS}", desc.engine)));
        }
        if !self.catalog.indexes(&desc.name).is_empty() {
            self.check_index_hidden(desc)?;
        }
        Ok(())
    }

    /// Refuses an index of `desc` if its encodings are confidential and
    /// the database's files are not encrypted: its keys would be in the
    /// clear.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, saying why.
    pub(crate) fn check_index_hidden(&self, desc: &TableDesc) -> Result<()> {
        let confidential = self.seal_of_desc(desc)?.is_some_and(|p| p.confidential());
        if confidential && !self.files_hidden()? {
            return Err(refused(&format!(
                "{} is encrypted, but an index keeps its keys in order, in the clear: encrypt the database's files too (STORAGE.ENCODINGS) to index it",
                desc.name
            )));
        }
        Ok(())
    }
}

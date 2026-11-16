//! \[DESIGN\] The large objects of a row written (RULES.md D79): each one
//! the table does not keep for the row is copied into objects of its own
//! before the row is, a chunk at a time (`TableStorage::keep_lobs`). Each
//! object is kept and logged in one section of the checkpoint gate, so a
//! checkpoint holds both or neither, and never waits for more than one
//! object; and its record comes before the row change naming it.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{Reading, Txn};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::error::Result;
use crate::log::record::Change;
use crate::storage::{Row, RowId};
use crate::sync::lock_builtin;

impl State {
    /// Makes each large object of `row`, to be inserted into `table` by
    /// `tx`, or to replace its row `replacing`, one the table keeps for
    /// it, logging each object it makes.
    pub(crate) fn keep_lobs(&self, tx: &Txn, table: &QualifiedName, replacing: Option<RowId>, row: &mut Row) -> Result<()> {
        if !row.iter().any(|v| matches!(v, crate::value::Value::Lob(_))) {
            return Ok(());
        }
        let storage = Arc::clone(self.storage_of(table)?);
        let logs = Self::logs_changes(self.capabilities(table)?.persistence);
        let generation = self.generation_of(table);
        let seal = self.seal_of(table)?;
        let mut log = |id: u64, body: &[u8], put: &mut dyn FnMut() -> Result<()>| -> Result<()> {
            let _gate = self.tx.gate.enter();
            put()?;
            if logs {
                let change = Change::LobPut { table: table.clone(), generation, id, body: body.to_vec() };
                tx.log_sealed(change, seal.as_deref().map(|p| (p, id)))?;
            }
            Ok(())
        };
        storage.keep_lobs(tx.id, replacing, row, &mut log)
    }
}

impl Txn {
    /// \[DESIGN\] Keeps its current snapshot counted open until it ends,
    /// though it reads at a new one from its next statement on (READ
    /// COMMITTED): what a value read at it names is not collected
    /// meanwhile, as a locator of the value holds it [ISO/IEC 9075-2:2023
    /// §4.37.5] (RULES.md D79). Of a transaction reading at one snapshot
    /// throughout, nothing: it holds it already.
    pub(crate) fn pin_snapshot(&self) {
        if self.reading != Reading::Statement || !self.open.load(Ordering::Acquire) {
            return;
        }
        let at = self.snapshot();
        let mut st = lock_builtin(&self.state);
        if !st.pinned.contains(&at) {
            st.pinned.push(at);
            self.shared.clock.hold(self.id, at);
        }
    }
}

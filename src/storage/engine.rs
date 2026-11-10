//! \[DESIGN\] A kind of table storage (`StorageEngine`), which makes and
//! opens the `TableStorage` of each table placed in it.

use super::{TableOption, TableStorage};
use crate::catalog::TableDesc;
use crate::error::Result;
use crate::log::Lsn;
use crate::memory::Host;
use crate::settings::Configurable;

/// \[DESIGN\] A kind of table storage: creates the `TableStorage` of each
/// table placed in it.
///
/// Contract (RULES.md R-SAFE-3; checked for the built-in engines by
/// `test/integration/contracts.rs`):
/// * `name` is unique among the registered engines, and constant.
/// * `create_table` returns an empty table for the columns of `desc`, or an
///   error, and then makes nothing.
/// * `open_table` returns the table whose `locator` it gave, holding every
///   change up to its `durable_through`, or `None` if it keeps nothing
///   between runs; the database then makes it anew and replays the log.
/// * `checkpoint(at)` is called with no change half made, and captures
///   every change logged before `at`; after `checkpoint_finish` then
///   succeeds, they are durable in the engine, and each table's
///   `durable_through` is at least `at`. Changes made between the two are
///   not part of the checkpoint.
/// * `attach` is called once, as the engine is registered, before any other
///   method but `name`; the memory its caches hold it reserves of the
///   `Host`'s budget, giving it back as they are dropped (RULES.md D38).
/// * `settings` names each setting under the engine's name (RULES.md D36);
///   `configure` is called with no lock of the database held, and takes
///   effect for what the engine does after it returns.
/// * Methods may be called from several threads at once (`Send + Sync`): an
///   implementation guards its own state.
/// * No method may call back into the `Database` or its sessions (RULES.md
///   R-ERR-4); the `Database` docs say what happens if one does.
pub trait StorageEngine: Send + Sync {
    /// The name the engine is chosen by.
    fn name(&self) -> &'static str;
    /// \[EXTENSION\] The options its tables take (RULES.md D41), each with
    /// its kind and default, set by `CREATE TABLE ... WITH (...)` and changed
    /// by ALTER TABLE; `create_table` and `open_table` find a table's in
    /// its descriptor (`TableDesc::options`). Default: none.
    fn table_options(&self) -> Vec<TableOption> {
        Vec::new()
    }
    /// \[EXTENSION\] Whether it passes its tables' rows through their
    /// ENCODINGS as it stores them (RULES.md D48), each with its encodings
    /// named (`Pipeline::seal`); a table of an engine that does not may name
    /// none. Default: no.
    fn seals(&self) -> bool {
        false
    }
    /// Takes what the database registering it provides: its memory budget.
    /// Default: nothing, for an engine that holds no cache.
    ///
    /// # Errors
    ///
    /// The engine cannot work with it; it is not registered.
    fn attach(&self, _host: &Host) -> Result<()> {
        Ok(())
    }
    /// \[DESIGN\] Itself as it declares settings and is told them
    /// (`Configurable`, RULES.md D36), if it has any. Default: none.
    fn configurable(&self) -> Option<&dyn Configurable> {
        None
    }
    /// The storage for a new, empty table described by `desc`.
    ///
    /// # Errors
    ///
    /// The engine cannot make the table (an engine on disk, its file); the
    /// definition fails.
    fn create_table(&self, desc: &TableDesc) -> Result<Box<dyn TableStorage>>;
    /// The storage of the table described by `desc` that this engine made
    /// before, found by the `locator` it gave (`TableStorage::locator`), or
    /// `None` if it keeps nothing between runs. Default: `None`.
    ///
    /// # Errors
    ///
    /// The engine cannot read the table; the database is not opened.
    fn open_table(&self, _desc: &TableDesc, _locator: &[u8]) -> Result<Option<Box<dyn TableStorage>>> {
        Ok(None)
    }
    /// Captures every change logged before position `at`, for
    /// `checkpoint_finish` to make durable (RULES.md D25,
    /// `db/checkpoint.rs`). Called with writers held, so it should be quick.
    /// Default: nothing, for an engine that keeps nothing.
    ///
    /// # Errors
    ///
    /// The engine cannot write its files; the checkpoint fails, and the
    /// last one stands.
    fn checkpoint(&self, _at: Lsn) -> Result<()> {
        Ok(())
    }
    /// Makes what the last `checkpoint` captured durable, writers going on
    /// meanwhile; what it wrote to its directory declared shadowed
    /// (`PluginFiles::declare`, RULES.md D56) is durable already. Default:
    /// nothing.
    ///
    /// # Errors
    ///
    /// The engine cannot write or sync its files; the checkpoint fails, and
    /// the last one stands.
    fn checkpoint_finish(&self) -> Result<()> {
        Ok(())
    }
}

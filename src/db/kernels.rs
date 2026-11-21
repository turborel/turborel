//! \[DESIGN\] A database's kernels (RULES.md D354, D460): the portable
//! ones built in, or one set a crate registers in their place, such as
//! `turborel-simd`'s, before the database opens its tables. A second
//! registration is refused. EXECUTOR.KERNELS names the set a statement
//! runs on, read as it begins: `PORTABLE` forces the built-in bodies, to
//! diagnose a registered set; any other value, the default empty among
//! them, runs the registered set, or the portable one if none is. The
//! setting is the executor's: a table of the column store decodes and
//! judges on the set registered, which it takes as it opens (RULES.md
//! D570), and SET leaves it as it is.

use std::sync::Arc;

use super::{Database, State, api};
use crate::error::Result;
use crate::kernels::{Kernels, PORTABLE, Portable};
use crate::settings::EXECUTOR_KERNELS;
use crate::value::Value;

impl Database {
    /// \[EXTENSION\] Registers `kernels`, the batches' primitives, in place
    /// of the portable ones, before the database is opened from its files,
    /// as an encoding is registered (RULES.md D354, D460).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a set is registered already, or the database is
    /// closed.
    pub fn register_kernels(&self, kernels: Box<dyn Kernels>) -> Result<()> {
        let kernels: Arc<dyn Kernels> = Arc::from(kernels);
        self.change(|st| {
            let held = st.functions.kernels().name();
            if held != PORTABLE {
                return Err(api(format!("[DESIGN] the kernels {held} are registered; a database takes one set")));
            }
            st.functions.set_kernels(Arc::clone(&kernels));
            for engine in st.engines.values() {
                engine.use_kernels(Arc::clone(&kernels));
            }
            Ok(())
        })
    }
}

/// The kernels a statement of `st` runs on, as EXECUTOR.KERNELS chooses:
/// the portable ones if it says `PORTABLE`, else the registered set.
pub(crate) fn chosen(st: &State) -> Arc<dyn Kernels> {
    if let Value::Char(name) = st.settings.get(EXECUTOR_KERNELS)
        && name.trim().eq_ignore_ascii_case(PORTABLE)
    {
        return Arc::new(Portable);
    }
    st.functions.kernels_shared()
}

//! \[EXTENSION\] Functions registered with the database, as plug-ins
//! (`functions.rs`): each with its settings declared and their values told
//! it (`Configurable`, RULES.md D36). Split from database.rs (RULES.md
//! R-SIZE-2).

use std::sync::Arc;

use super::Database;
use crate::diagnostics::Extension;
use crate::error::Result;
use crate::functions::{AggregateFunction, ScalarFunction};

impl Database {
    /// \[EXTENSION\] Register a scalar function. Callable only from sessions
    /// that take the function call syntax (SQL.FUNCTION_CALLS ON).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if an aggregate function has the same name, or a
    /// scalar function, a standard one too: none is replaced (RULES.md
    /// D53); or a setting it declares is refused (D36).
    pub fn register_scalar_function(&self, f: Box<dyn ScalarFunction>) -> Result<()> {
        let (name, f): (String, Arc<dyn ScalarFunction>) = (f.name().to_owned(), Arc::from(f));
        let done = self.change(|st| {
            st.declare_plugin(&name, f.configurable())?;
            st.functions.add_scalar(Arc::clone(&f))
        });
        self.registered((Extension::Function, &name), done.and_then(|()| self.tell_plugin(f.configurable())))
    }

    /// \[EXTENSION\] Register an aggregate. Registering COUNT, SUM, AVG, MAX or
    /// MIN replaces that set function's implementation [X3.135-1986 §5.8];
    /// any other name is callable only through the function call syntax.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a scalar function has the same name, or a setting it
    /// declares is refused (RULES.md D36).
    pub fn register_aggregate_function(&self, f: Box<dyn AggregateFunction>) -> Result<()> {
        let (name, f): (String, Arc<dyn AggregateFunction>) = (f.name().to_owned(), Arc::from(f));
        let done = self.change(|st| {
            st.declare_plugin(&name, f.configurable())?;
            st.functions.add_aggregate(Arc::clone(&f))
        });
        self.registered((Extension::Function, &name), done.and_then(|()| self.tell_plugin(f.configurable())))
    }
}

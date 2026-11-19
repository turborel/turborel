//! \[EXTENSION\] What the database's data file's pool evicts by (RULES.md
//! D283): CLOCK, or the policy a host plug-in's BUFFER EVICTION pushes,
//! set before the database is opened from its files.

use super::Database;
use crate::disk::buffer::MakeEviction;
use crate::error::Result;

impl Database {
    /// \[EXTENSION\] The pool of the data file made after this evicts by
    /// what `make` makes for its frames, in place of CLOCK: set before
    /// the database is opened from its files, as a host plug-in's BUFFER
    /// EVICTION is (RULES.md D283). A pool made already keeps its own.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed.
    pub fn set_eviction(&self, make: MakeEviction) -> Result<()> {
        self.snapshot()?.eviction.set(make);
        Ok(())
    }
}

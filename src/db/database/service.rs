//! \[DESIGN\] A database's services (RULES.md D57): opened from its files,
//! it keeps how, so a replica's database deposed is opened again in place
//! (`db/standby/reopen.rs`), the same handle, its jobs, settings and sinks
//! kept; each time it begins another service, and a session of an earlier
//! one is refused.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::super::standby::Opening;
use super::{Database, State};
use crate::error::{Code, Result, SqlError};
use crate::sync::{lock, write};

impl Database {
    /// The service it is in: 0 until it is first opened again in place.
    pub(in crate::db) fn service(&self) -> u64 {
        self.0.service.load(Ordering::Acquire)
    }

    /// Refuses a call of a session of service `of`, begun before the
    /// database was opened again in place.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `of` is not the service it is in.
    pub(in crate::db) fn refuse_earlier(&self, of: u64) -> Result<()> {
        if of == self.service() {
            return Ok(());
        }
        Err(SqlError::new(Code::Api, "[DESIGN] RULES.md D57: the session's database was opened again since it began: begin another"))
    }

    /// Keeps how it was first opened from its files, unless it keeps one.
    pub(in crate::db) fn keep_opening(&self, opening: Opening) -> Result<()> {
        lock(&self.0.opening)?.get_or_insert_with(|| Arc::new(opening));
        Ok(())
    }

    /// How it was first opened from its files, if it was.
    pub(in crate::db) fn opening(&self) -> Result<Option<Arc<Opening>>> {
        Ok(lock(&self.0.opening)?.clone())
    }

    /// Begins another service over `state`, the database as it was before
    /// it was first opened from its files: its files given up, its
    /// transactions in doubt let go, as recovery from its files decides
    /// them, and the sessions of earlier services refused from now on.
    pub(in crate::db) fn restart_service(&self, state: State) -> Result<()> {
        let _turn = self.take_turn()?;
        *lock(&self.0.files)? = None;
        lock(&self.0.in_doubt)?.clear();
        self.0.service.fetch_add(1, Ordering::AcqRel);
        *write(&self.0.current)? = Arc::new(state);
        Ok(())
    }
}

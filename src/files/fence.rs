//! \[DESIGN\] The fence of a database's files (RULES.md D56). The lock of
//! its directory keeps every other opener out while a database runs, but
//! a file system may give a lock to another opener while its holder still
//! runs, as one gives a crashed process's locks up, or the tests' does.
//! Then the first database's sessions and jobs still hold its log and
//! files, and a session ended would commit at the log's old end, over
//! what the second wrote. So before each write, change of size, sync,
//! creation or removal of a file, the file manager asks whether the
//! database still holds its lock, and from the first time it does not,
//! or once the database has closed, refuses every one (SQLCODE -915, or
//! -910 once closed, as a file opened then is refused).
//! The transactions such a database ends are undone in memory only, and
//! its log and files are left as the next opener has them.

use std::path::Path;
use std::sync::PoisonError;
use std::sync::atomic::Ordering;

use super::Registry;
use crate::disk::io::Holding;
use crate::error::{Code, Result, SqlError};

impl Registry {
    /// Keeps the test of whether the database still holds its directory's
    /// lock, of a lock another opener may take.
    pub(super) fn hold(&self, holding: Option<Holding>) {
        *self.holding.write().unwrap_or_else(PoisonError::into_inner) = holding;
    }

    /// Refuses a change to `path` once the database is closed, or its
    /// lock was taken by another opener, which it never gets back.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is closed; -915 if its lock was taken.
    pub(super) fn fenced(&self, path: &Path) -> Result<()> {
        if !self.fenced.load(Ordering::Acquire) {
            let lost = self.holding.read().unwrap_or_else(PoisonError::into_inner).as_ref().is_some_and(|held| !held());
            if !lost && !self.shut.load(Ordering::Acquire) {
                return Ok(());
            }
            self.fenced.store(true, Ordering::Release);
        }
        // Closed, as a file opened after `close_all` is refused; taken, as a
        // file the system no longer lets it have.
        let code = if self.shut.load(Ordering::Acquire) { Code::Api } else { Code::Io };
        let why = "its database is closed, or another opened its files since, so it changes them no more";
        Err(SqlError::new(code, format!("[DESIGN] RULES.md D56: {} is not changed: {why}", path.display())))
    }

    /// Whether the database is closed, or another opener took its lock:
    /// so whether `fenced` refuses a change.
    pub fn is_fenced(&self) -> bool {
        self.fenced(Path::new(".")).is_err()
    }
}

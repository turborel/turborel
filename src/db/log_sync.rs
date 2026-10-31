//! \[DESIGN\] The log synced at a pace of its own (RULES.md D25, D55): a
//! commit of LOG.COMMIT_SYNC OFF is answered once its commit record is
//! written, and is durable only at the log's next sync. So that a trickle
//! of such commits, then a database at rest, is not left unsynced for
//! good, the first commit left unsynced asks for the log sync job
//! (`turborel-log-sync`) LOG.SYNC_AFTER_MS later, which the thread manager
//! runs as JOBS.POLICY says (`db/background.rs`). The job syncs the log
//! through the last commit left unsynced, if another sync (a commit that
//! waits, a definition, a checkpoint) has not; a commit after it asks
//! anew. LOG.SYNC_AFTER_MS 0 asks for none, the commits then synced only
//! by the log's next sync for another reason.
//!
//! The job syncs through `Log::sync`, as every sync of the log, so a
//! memory log, the file log and a log plug-in alike; a failed sync stops
//! the database taking changes, as any failed sync of the log does
//! (`txn/logging.rs`), and is told (`Event::Thread`). Closing stops the
//! job's thread before the close syncs the log itself.

use std::time::Duration;

use super::Database;
use crate::error::Result;
use crate::settings::LOG_SYNC_AFTER_MS;

/// \[DESIGN\] The milliseconds after a commit not waiting for the log
/// within which its job syncs the log, by default: LOG.SYNC_AFTER_MS. A
/// second bounds by time what a failure of the system loses of such
/// commits, at the cost of at most one sync a second while they come.
pub const SYNC_AFTER_MS: u64 = 1000;

impl Database {
    /// Asks for the log sync job, LOG.SYNC_AFTER_MS from now, if a commit
    /// left the log unsynced since it was last asked for, and the setting
    /// is not 0.
    pub(super) fn sync_log_if_due(&self) {
        let Ok(st) = self.snapshot() else { return };
        let Ok(after) = u64::try_from(st.settings.integer(LOG_SYNC_AFTER_MS)) else { return };
        if after == 0 {
            return;
        }
        let asked = st.tx.ask_sync();
        if asked {
            self.request_log_sync(Duration::from_millis(after));
        }
    }
}

/// The log sync job's work: syncs the log through the last commit left
/// unsynced, if one is, and LOG.SYNC_AFTER_MS is not 0. It is asked for
/// again by the next commit left unsynced. A change of the setting needs
/// no asking: it is a definition, logged and synced (RULES.md D36), which
/// syncs every commit before it.
///
/// # Errors
///
/// SQLCODE -910 if the database is closed; SQLCODE -915 if the database
/// takes no changes, a sync of its log having failed, or the sync fails,
/// which then stops it taking changes (RULES.md D25).
pub(super) fn log_synced(db: &Database) -> Result<Option<Duration>> {
    let st = db.snapshot()?;
    if st.settings.integer(LOG_SYNC_AFTER_MS) == 0 {
        return Ok(None);
    }
    if let Some((log, through)) = st.tx.take_unsynced() {
        st.tx.sync(&*log, through)?;
    }
    Ok(None)
}

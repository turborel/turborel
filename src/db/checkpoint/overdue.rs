//! \[DESIGN\] Checkpoints overdue (RULES.md D25, D40): while none is taken,
//! or one is but an open transaction keeps the log from its first record,
//! the log, and the tables file's side file, only grow. As a transaction
//! ends, once either has grown past `OVERDUE` times what makes a
//! checkpoint due, it is told, at most once a minute, with why: each
//! checkpoint refused (a damaged table, a failed sync of the log, a
//! failure of its own), the oldest open transaction and how long ago it
//! began logging, or none taken as JOBS.POLICY stands. Split from
//! checkpoint.rs (RULES.md R-SIZE-2).

use std::time::{Duration, Instant};

use super::super::Database;
use crate::diagnostics::{Event, Overdue};
use crate::settings::{CHECKPOINT_AFTER_BYTES, CHECKPOINT_AFTER_CHANGES};
use crate::sync::lock_builtin;

/// \[DESIGN\] How many times what makes a checkpoint due
/// (CHECKPOINT.AFTER_CHANGES, CHECKPOINT.AFTER_BYTES) the log, or the side
/// file, grows to with none letting it be given up before it is told:
/// four, so a checkpoint merely slow, or taken a little late, is not.
pub const OVERDUE: u64 = 4;

/// \[DESIGN\] How seldom it is told: once a minute at most, as files
/// opened again too often are (`Event::FilesReopened`).
const TOLD_EVERY: Duration = Duration::from_secs(60);

impl Database {
    /// Tells `Event::CheckpointOverdue` if the log has grown past
    /// `OVERDUE` times what makes a checkpoint due, unless it was told in
    /// the last minute.
    pub(in crate::db) fn tell_if_overdue(&self) {
        let (Some(event), Ok(Some(files))) = (self.overdue(), self.files()) else { return };
        let now = Instant::now();
        {
            let mut told = lock_builtin(&files.checkpointer.overdue_told);
            if told.is_some_and(|at| now.saturating_duration_since(at) < TOLD_EVERY) {
                return;
            }
            *told = Some(now);
        }
        // Told with no lock held: the sink is a plug-in (RULES.md R-ERR-4).
        self.emit(&event);
    }

    /// The event of checkpoints overdue, if they are, with its cause.
    fn overdue(&self) -> Option<Event> {
        let (Ok(Some(files)), Ok(st)) = (self.files(), self.snapshot()) else { return None };
        let limit = |key| u64::try_from(st.settings.integer(key)).unwrap_or(u64::MAX).saturating_mul(OVERDUE);
        let since = st.tx.since_checkpoint();
        let behind = since >= limit(CHECKPOINT_AFTER_CHANGES) || files.shadows.pending_bytes() >= limit(CHECKPOINT_AFTER_BYTES);
        let (held, age) = st.tx.pinned().unwrap_or_default();
        let cause = if behind {
            match st.checkpoint_refused().err().or_else(|| self.checkpoint_failure()) {
                Some(e) => Overdue::Refused(e),
                None => Overdue::NotTaken(self.job_policy()),
            }
        } else if held >= limit(CHECKPOINT_AFTER_CHANGES) {
            Overdue::Transaction(age)
        } else {
            return None;
        };
        Some(Event::CheckpointOverdue { changes: since.max(held), cause })
    }
}

//! \[DESIGN\] A checkpoint of a following database (RULES.md D25, D57): its
//! log is its replica's, which it never writes, so it logs nothing. Taking
//! turns with its replay, it collects every table's garbage, so no entry of
//! a version awaiting collection needs logging (`Change::Stale`), and is
//! refused while a reader still holds one; then every engine captures what
//! it holds through the position after the last record replayed, and the
//! image is recorded, recovery to read the log from the first record of the
//! oldest transaction it holds, whose records it applies only as the
//! transaction ends (`db/standby.rs`). Opened again, it recovers from that
//! image, and follows from there (`db/standby/reopen.rs`). Not before it
//! has replayed again what the checkpoint it was opened from holds.

use super::super::files::DatabaseFiles;
use super::super::{Database, api};
use crate::error::{Code, Result, SqlError};
use crate::log::Lsn;
use crate::sync::lock_builtin;
use crate::txn::FollowedRecords;

impl Database {
    /// `take_checkpoint`'s work for a following database, its turn taken:
    /// see the module's documentation.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 before it replayed again what the checkpoint it was
    /// opened from holds; -911 while a reader holds a version awaiting
    /// collection; -914 while a table is damaged; an engine's, or the
    /// catalog's, errors.
    pub(super) fn take_following_checkpoint(&self, files: &DatabaseFiles) -> Result<()> {
        let current = self.snapshot()?;
        let _replaying = current.tx.replaying.take("a checkpoint")?;
        let st = self.snapshot()?;
        let following = *lock_builtin(&st.tx.following);
        let through = Lsn(following.replayed.0.saturating_add(1));
        if through < following.image {
            return Err(api(
                "[DESIGN] RULES.md D57: a following database takes no checkpoint until it replays again what it was opened from",
            ));
        }
        if let Some(table) = st.damaged_tables().first() {
            return Err(SqlError::new(Code::Damaged, format!("[DESIGN] {table} is damaged: no checkpoint is taken until it is repaired")));
        }
        let tables = st.catalog.tables();
        st.gc(tables.iter().map(|t| &t.name));
        if let Some(table) = st.owing() {
            return Err(SqlError::new(
                Code::Io,
                format!(
                    "[DESIGN] RULES.md D57: index entries of {table} could not be taken back: a following database's checkpoint waits for them"
                ),
            ));
        }
        let as_of = st.tx.stable();
        for table in tables.iter().map(|t| &t.name) {
            let storage = st.storage_of(table)?;
            if std::sync::Arc::clone(storage).garbage(as_of).next().is_some() {
                return Err(SqlError::new(
                    Code::Conflict,
                    format!("[DESIGN] RULES.md D57: a reader holds versions of {table}: a following database's checkpoint waits for it"),
                ));
            }
        }
        let counted = st.tx.logged.load(std::sync::atomic::Ordering::Relaxed);
        st.capture_engines(through)?;
        let held = lock_builtin(&st.tx.followed).values().filter_map(FollowedRecords::first).min();
        let image = st.image_of(false, (through, held.map_or(through, |first| first.min(through))));
        let image = st.record_image(files, image)?;
        st.tx.checkpointed(counted);
        // The group's log before what the image holds may go, as the
        // replica's retention says (RULES.md D57).
        let log = st.log.clone().ok_or_else(|| api("[DESIGN] a database opened from its files has a log"))?;
        log.truncate_before(image.start)
    }
}

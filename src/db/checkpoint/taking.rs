//! \[DESIGN\] A checkpoint taken, and timed (RULES.md D25, D810): the
//! steps of the module above it, and what they took, for the diagnostics
//! (`Event::CheckpointTaken`): the whole, the writers held, the record's
//! sync, the captures made durable, and the pages written to the tables
//! file meanwhile, so an operator sees what a checkpoint costs the work
//! going on.

use std::sync::atomic::Ordering;
use std::time::Instant;

use super::super::files::DatabaseFiles;
use super::super::{Database, api};
use crate::diagnostics::CheckpointTook;
use crate::error::Result;
use crate::threads::Turned;

impl Database {
    /// `checkpoint`'s work, and what it took.
    pub(super) fn take_checkpoint(&self) -> Result<CheckpointTook> {
        let began = Instant::now();
        let files = self.files()?.ok_or_else(|| api("[DESIGN] only a database opened from its files takes checkpoints"))?;
        let written = pages_written(&files);
        let _whole = files.checkpointer.whole.take("a checkpoint")?;
        let turn = files.checkpointer.taking.take("a checkpoint")?;
        let current = self.snapshot()?;
        let mut took = CheckpointTook::default();
        if matches!(current.tx.held(), Some(crate::txn::Held::Following)) {
            self.take_following_checkpoint(&files)?;
        } else {
            self.take_shut(&files, turn, &mut took)?;
        }
        took.micros = micros(began);
        took.pages = pages_written(&files).saturating_sub(written);
        Ok(took)
    }

    /// Steps 1 to 7 of a checkpoint not following, the checkpoints' `turn`
    /// held, timed into `took`.
    fn take_shut(&self, files: &DatabaseFiles, turn: Turned<'_>, took: &mut CheckpointTook) -> Result<()> {
        let current = self.snapshot()?;
        // Not while an index is built: a checkpoint records only indexes
        // ready to be read (`Database::create_index`).
        let shutting = Instant::now();
        let shut = loop {
            current.tx.wait_for_builds();
            let shut = current.tx.gate.shut();
            if !current.tx.building() {
                break shut;
            }
        };
        // Taken shut: no definition is half published.
        let st = self.snapshot()?;
        let (image, counted) = st.capture_checkpoint(false)?;
        let stale = st.hold_stale();
        drop(shut);
        took.shut_micros = micros(shutting);
        // Writers going on: the entries of what awaited collection as the
        // pages were captured, logged before the record that syncs them.
        let logged = st.log_stale(&stale, image.through);
        drop(stale);
        // The turn let go while the record is synced (step 4), then taken
        // again to record the image.
        let syncing = Instant::now();
        let synced = logged.and_then(|()| st.sync_checkpoint_record(turn, &image));
        took.sync_micros = micros(syncing);
        let finishing = Instant::now();
        let recorded = synced.and_then(|()| {
            let _turn = files.checkpointer.taking.take("a checkpoint")?;
            st.record_and_truncate(files, image)
        });
        took.finish_micros = micros(finishing);
        st.counted_if(recorded, counted)?;
        files.checkpointer.taken.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// The microseconds since `from`.
fn micros(from: Instant) -> u64 {
    u64::try_from(from.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// The pages written to the tables file of `files` since it was opened:
/// written back, or captured (`Pager::captured`); none of a database with
/// no tables file.
fn pages_written(files: &DatabaseFiles) -> u64 {
    files.data.as_ref().and_then(|(pool, file)| pool.pager_counts(*file).ok()).map_or(0, |counts| counts.writes)
}

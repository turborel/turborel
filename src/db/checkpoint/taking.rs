//! \[DESIGN\] A checkpoint taken, and timed (RULES.md D25, D810): the
//! steps of the module above it, and what they took, for the diagnostics
//! (`Event::CheckpointTaken`): the whole, the writers held, the record's
//! sync, the captures made durable, and the pages written to the tables
//! file meanwhile, so an operator sees what a checkpoint costs the work
//! going on.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::super::files::DatabaseFiles;
use super::super::{Database, api};
use crate::diagnostics::CheckpointTook;
use crate::error::Result;
use crate::sync::lock_builtin;
use crate::threads::Turned;

/// \[DESIGN\] The share of the time between checkpoints a checkpoint's
/// commits are spread over, as a fraction (RULES.md D812). Three quarters
/// by a sweep of none, a quarter, a half and three quarters on a drive
/// with power-loss protection and one without: the flattest work going on
/// on both, a quarter of the time left before the next is due.
const SPREAD: (u32, u32) = (3, 4);

impl Database {
    /// `checkpoint`'s work, and what it took; its commits spread if
    /// `spread` (RULES.md D812).
    pub(super) fn take_checkpoint(&self, spread: bool) -> Result<CheckpointTook> {
        let began = Instant::now();
        let files = self.files()?.ok_or_else(|| api("[DESIGN] only a database opened from its files takes checkpoints"))?;
        let written = pages_written(&files);
        let _whole = files.checkpointer.whole.take("a checkpoint")?;
        let turn = files.checkpointer.taking.take("a checkpoint")?;
        let over = spread_over(&files.checkpointer, began).filter(|_| spread);
        let current = self.snapshot()?;
        let mut took = CheckpointTook::default();
        if matches!(current.tx.held(), Some(crate::txn::Held::Following)) {
            self.take_following_checkpoint(&files)?;
        } else {
            self.take_shut(&files, (turn, over), &mut took)?;
        }
        took.micros = micros(began);
        took.pages = pages_written(&files).saturating_sub(written);
        Ok(took)
    }

    /// Steps 1 to 7 of a checkpoint not following, the checkpoints' `turn`
    /// held, timed into `took`; the commits of the captures spread `over`
    /// a span if one is given and the database is not closing.
    fn take_shut(&self, files: &DatabaseFiles, (turn, over): (Turned<'_>, Option<Duration>), took: &mut CheckpointTook) -> Result<()> {
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
        spread_commits(files, over);
        let recorded = synced.and_then(|()| {
            let _turn = files.checkpointer.taking.take("a checkpoint")?;
            st.record_and_truncate(files, image)
        });
        files.shadows.spread_commits(None);
        took.finish_micros = micros(finishing);
        st.counted_if(recorded, counted)?;
        files.checkpointer.taken.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

impl Database {
    /// Whether checkpoints are to hurry, the database closing: a commit
    /// spread goes on at once.
    pub(super) fn hurry_checkpoints(&self, hurry: bool) {
        if let Ok(Some(files)) = self.files() {
            files.checkpointer.hurry.store(hurry, Ordering::SeqCst);
            if hurry {
                files.shadows.spread_commits(None);
            }
        }
    }
}

/// Spreads the commits of `files` `over` a span, unless the database is
/// closing: told so after the span is set, a close told to hurry before
/// it was set clears it; one told after, it clears it itself.
fn spread_commits(files: &DatabaseFiles, over: Option<Duration>) {
    files.shadows.spread_commits(over);
    if files.checkpointer.hurry.load(Ordering::SeqCst) {
        files.shadows.spread_commits(None);
    }
}

/// \[DESIGN\] The span a checkpoint begun at `now` spreads its commits
/// over (RULES.md D812): `SPREAD` of the time since the last began, so it
/// is done before the next is due; none for the first. `now` is
/// kept, for the next.
fn spread_over(checkpointer: &super::Checkpointer, now: Instant) -> Option<Duration> {
    let last = lock_builtin(&checkpointer.began).replace(now)?;
    let since = now.saturating_duration_since(last);
    since.saturating_mul(SPREAD.0).checked_div(SPREAD.1)
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

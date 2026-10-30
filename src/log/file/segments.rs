//! \[DESIGN\] A file log's segments begun, sealed and synced, a failed
//! sync kept (RULES.md D25, D30): see the parent module. Split from
//! file.rs (RULES.md R-SIZE-2).

use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

use super::{FileLog, Segment, add, segment_file, segment_name};
use crate::diagnostics::Event;
use crate::disk::block::BlockId;
use crate::disk::io::Durability;
use crate::error::{Result, SqlError};
use crate::sync::lock_builtin;

impl FileLog {
    /// Begins the segment after `full`, if `full` is still the current one
    /// and no other thread is beginning one; then seals `full`. A segment
    /// that cannot be begun is told, once until one is; a seal that fails
    /// is kept, and told, by `failed`.
    pub(super) fn rotate(&self, full: &Arc<Segment>) {
        if self.rotating.swap(true, Ordering::AcqRel) {
            return;
        }
        let current = Arc::ptr_eq(&self.active.read().unwrap_or_else(PoisonError::into_inner), full);
        let begun = if current { self.begin_after(full) } else { Ok(()) };
        self.rotating.store(false, Ordering::Release);
        if let Err(e) = begun {
            self.tell_unbegun(&e, full);
            return;
        }
        self.told.store(false, Ordering::Release);
        if current {
            // Its failure fails every later sync; nothing more to do here.
            drop(self.seal(full));
        }
    }

    /// Tells failure `e` to begin the segment after `full`, once until one
    /// is begun.
    fn tell_unbegun(&self, e: &SqlError, full: &Segment) {
        if !self.told.swap(true, Ordering::AcqRel) {
            let why = format!(
                "[DESIGN] the log in {} could not begin a segment after segment {}, which grows past its size: {}",
                self.dir.display(),
                full.number,
                e.msg
            );
            self.events.event(&Event::Failed(SqlError::new(e.code, why)));
        }
    }

    /// Fails if a sync of the log failed before: it may have lost what it
    /// was to make durable, and a sync tried again may succeed without it.
    ///
    /// # Errors
    ///
    /// The failure kept, as SQLCODE -915 or its own.
    pub(super) fn healthy(&self) -> Result<()> {
        let Some(e) = lock_builtin(&self.failure).clone() else { return Ok(()) };
        Err(SqlError::new(
            e.code,
            format!(
                "[DESIGN] a sync of the log in {} failed, so it syncs nothing more until it is opened again: {}",
                self.dir.display(),
                e.msg
            ),
        ))
    }

    /// Keeps `e`, the failure of a sync of segment `number`, if it is the
    /// first, and tells it; returns it.
    fn failed(&self, e: SqlError, number: u64) -> SqlError {
        let mut failure = lock_builtin(&self.failure);
        if failure.is_none() {
            *failure = Some(e.clone());
            drop(failure);
            let why = format!(
                "[DESIGN] the log in {} could not sync segment {number}, so every later sync of it fails, and the database takes no change until it is opened again: {}",
                self.dir.display(),
                e.msg
            );
            self.events.event(&Event::Failed(SqlError::new(e.code, why)));
        }
        e
    }

    /// Creates the segment after `full` and makes it current.
    fn begin_after(&self, full: &Segment) -> Result<()> {
        let number = full.number.saturating_add(1);
        let path = self.dir.join(segment_name(number));
        // Left by a rotation that failed, half made, holding no record.
        if self.fs.exists(&path)? {
            self.fs.remove(&path)?;
        }
        let blocks = segment_file(&*self.fs, &path, Arc::clone(&self.hash), &self.options)?;
        let next = Segment::new(number, blocks);
        lock_builtin(&self.segments).insert(number, Arc::clone(&next));
        *self.active.write().unwrap_or_else(PoisonError::into_inner) = next;
        add(&self.counters.rotations, 1);
        Ok(())
    }

    /// Seals `segment`, no longer current: waits for its appends, syncs it.
    pub(super) fn seal(&self, segment: &Segment) -> Result<()> {
        if segment.sealed.load(Ordering::Acquire) {
            return Ok(());
        }
        self.healthy()?;
        segment.drain();
        // A log never synced asks for no sync.
        if self.options.durability != Durability::None {
            segment.blocks.sync(self.options.durability).map_err(|e| self.failed(e, segment.number))?;
        }
        segment.durable_end.fetch_max(segment.blocks.end().0, Ordering::AcqRel);
        segment.sealed.store(true, Ordering::Release);
        Ok(())
    }

    /// Makes block `id` of `segment` durable, grouping with other syncs: if
    /// one is running, wait for it, then check again; otherwise sync for
    /// everyone, through the blocks published when the sync began.
    pub(super) fn sync_through(&self, segment: &Segment, id: BlockId) -> Result<()> {
        if id.0 < segment.durable_end.load(Ordering::Acquire) {
            return Ok(());
        }
        add(&self.counters.sync_requests, 1);
        let mut syncing = lock_builtin(&self.syncing);
        loop {
            if id.0 < segment.durable_end.load(Ordering::Acquire) {
                return Ok(());
            }
            // The group's sync failed: none is tried again.
            self.healthy()?;
            if !*syncing {
                break;
            }
            syncing = self.synced.wait(syncing).unwrap_or_else(PoisonError::into_inner);
        }
        *syncing = true;
        drop(syncing);
        let end = segment.blocks.end();
        let synced = segment.blocks.sync(self.options.durability).map_err(|e| self.failed(e, segment.number));
        if synced.is_ok() {
            segment.durable_end.fetch_max(end.0, Ordering::AcqRel);
            add(&self.counters.syncs, 1);
        }
        *lock_builtin(&self.syncing) = false;
        self.synced.notify_all();
        synced
    }
}

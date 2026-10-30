//! \[DESIGN\] A file log's segments begun, sealed and synced (RULES.md D25,
//! D30): see the parent module. Split from file.rs (RULES.md R-SIZE-2).

use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

use super::{FileLog, Segment, add, segment_name};
use crate::diagnostics::Event;
use crate::disk::block::{BlockFile, BlockId};
use crate::error::{Result, SqlError};
use crate::sync::lock_builtin;

impl FileLog {
    /// Tells a failure `rotated` of `full`'s rotation, once until one
    /// succeeds.
    pub(super) fn tell(&self, rotated: Result<()>, full: &Segment) {
        let Err(e) = rotated else {
            self.told.store(false, Ordering::Release);
            return;
        };
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

    /// Begins the segment after `full`, if `full` is still the current one
    /// and no other thread is beginning one; then seals `full`.
    pub(super) fn rotate(&self, full: &Arc<Segment>) -> Result<()> {
        if self.rotating.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let current = Arc::ptr_eq(&self.active.read().unwrap_or_else(PoisonError::into_inner), full);
        let begun = if current { self.begin_after(full) } else { Ok(()) };
        self.rotating.store(false, Ordering::Release);
        begun?;
        if current {
            self.seal(full)?;
        }
        Ok(())
    }

    /// Creates the segment after `full` and makes it current.
    fn begin_after(&self, full: &Segment) -> Result<()> {
        let number = full.number.saturating_add(1);
        let path = self.dir.join(segment_name(number));
        // Left by a rotation that failed, half made, holding no record.
        if self.fs.exists(&path)? {
            self.fs.remove(&path)?;
        }
        let blocks = BlockFile::create(&*self.fs, &path, Arc::clone(&self.hash), self.options.pipeline.clone(), self.options.blocks)?;
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
        segment.drain();
        segment.blocks.sync(self.options.durability)?;
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
            if !*syncing {
                break;
            }
            syncing = self.synced.wait(syncing).unwrap_or_else(PoisonError::into_inner);
        }
        *syncing = true;
        drop(syncing);
        let end = segment.blocks.end();
        let synced = segment.blocks.sync(self.options.durability);
        if synced.is_ok() {
            segment.durable_end.fetch_max(end.0, Ordering::AcqRel);
            add(&self.counters.syncs, 1);
        }
        *lock_builtin(&self.syncing) = false;
        self.synced.notify_all();
        synced
    }
}

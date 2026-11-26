//! \[DESIGN\] A file log's segments begun, sealed and synced, a failed
//! sync kept (RULES.md D25, D30): see the parent module.

use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

use super::{FileLog, Segment, add, lsn, segment_file, segment_name};
use crate::diagnostics::Event;
use crate::disk::block::{BlockFile, BlockId};
use crate::disk::io::Durability;
use crate::error::{Result, SqlError};
use crate::log::Lsn;
use crate::sync::lock_builtin;

impl FileLog {
    /// Writes in `hash` and `pipeline` from now on: a segment in them begun
    /// before the next record appended, if they are not those of now.
    pub(super) fn formats_from_now(&self, hash: Arc<dyn crate::hash::HashFunction>, pipeline: crate::disk::encoding::Pipeline) {
        let changed = {
            let mut formats = lock_builtin(&self.formats);
            let changed = formats.0.key_name() != hash.key_name() || formats.1.ids() != pipeline.ids();
            *formats = (hash, pipeline);
            changed
        };
        if changed {
            self.rotate_asked.store(true, Ordering::Release);
        }
    }

    /// Writes `count` records of `bytes` bytes to the active segment with
    /// `write`: its number, and what `write` returned. A segment that
    /// cannot be begun leaves this one growing past its size; the records
    /// are appended either way, a failure told, and a failed seal kept to
    /// fail the next sync.
    pub(super) fn on_active<T>(&self, write: impl FnOnce(&BlockFile) -> Result<T>, count: usize, bytes: usize) -> Result<(u64, T)> {
        if self.rotate_asked.load(Ordering::Acquire) {
            self.rotate_for_formats();
        }
        let segment = self.take_active();
        let written = write(&segment.blocks);
        segment.finished();
        let written = written?;
        add(&self.counters.appends, u64::try_from(count).unwrap_or(u64::MAX));
        add(&self.counters.appended_bytes, u64::try_from(bytes).unwrap_or(u64::MAX));
        if segment.blocks.end().0 >= self.options.segment_size {
            self.rotate(&segment);
        }
        Ok((segment.number, written))
    }

    /// Appends `records`, in order: their positions. As many as the
    /// active segment has room for go in one write
    /// (`BlockFile::append_each`), at least one, then the rest, so a run
    /// grows a segment past its size no more than one record does. A run
    /// written whole, as a transaction's at its commit nearly always is,
    /// gives back the vector of its blocks' ids made positions in place.
    pub(super) fn append_records<'r>(&self, records: impl ExactSizeIterator<Item = &'r [u8]> + Clone) -> Result<Vec<Lsn>> {
        let mut lsns = Vec::new();
        let mut rest = records;
        while rest.len() > 0 {
            let fit = self.fitting(rest.clone());
            let now = rest.clone().take(fit);
            let bytes = now.clone().map(<[u8]>::len).sum();
            let (number, ids) = self.on_active(|blocks| blocks.append_each(now), fit, bytes)?;
            let positions = ids.into_iter().map(|id| lsn(number, id));
            if lsns.is_empty() && fit >= rest.len() {
                return Ok(positions.collect());
            }
            lsns.extend(positions);
            for _ in 0..fit {
                rest.next();
            }
        }
        Ok(lsns)
    }

    /// How many of `records`, at least one, the active segment has room
    /// for, by their lengths.
    fn fitting<'r>(&self, records: impl Iterator<Item = &'r [u8]>) -> usize {
        let end = self.active.read().unwrap_or_else(PoisonError::into_inner).blocks.end().0;
        let mut room = self.options.segment_size.saturating_sub(end);
        let fit = records
            .take_while(|record| {
                let len = u64::try_from(record.len()).unwrap_or(u64::MAX);
                room.checked_sub(len).map(|left| room = left).is_some()
            })
            .count();
        fit.max(1)
    }

    /// Begins the segment after `full`, if `full` is still the current one
    /// and no other thread is beginning one; then seals `full`. A segment
    /// that cannot be begun is told, once until one is; a seal that fails
    /// is kept, and told, by `failed`. Whether a segment was begun.
    pub(super) fn rotate(&self, full: &Arc<Segment>) -> bool {
        if self.rotating.swap(true, Ordering::AcqRel) {
            return false;
        }
        let current = Arc::ptr_eq(&self.active.read().unwrap_or_else(PoisonError::into_inner), full);
        let begun = if current { self.begin_after(full) } else { Ok(()) };
        self.rotating.store(false, Ordering::Release);
        if let Err(e) = begun {
            self.tell_unbegun(&e, full);
            return false;
        }
        self.told.store(false, Ordering::Release);
        if current {
            // Its failure fails every later sync; nothing more to do here.
            drop(self.seal(full));
        }
        current
    }

    /// Begins a segment in the formats set (`Log::set_formats`), before
    /// the record appended next: asked again at the next append while none
    /// is begun, as another thread was beginning one, or none could be.
    pub(super) fn rotate_for_formats(&self) {
        let current = Arc::clone(&self.active.read().unwrap_or_else(PoisonError::into_inner));
        if self.rotate(&current) {
            self.rotate_asked.store(false, Ordering::Release);
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

    /// Keeps `e`, the failure of a sync of `what` (segment N, or the
    /// directory), if it is the first, and tells it; returns it.
    fn failed(&self, e: SqlError, what: &str) -> SqlError {
        let mut failure = lock_builtin(&self.failure);
        if failure.is_none() {
            *failure = Some(e.clone());
            drop(failure);
            let why = format!(
                "[DESIGN] the log in {} could not sync {what}, so every later sync of it fails, and the database takes no change until it is opened again: {}",
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
        let durability = self.durability();
        let formats = lock_builtin(&self.formats).clone();
        let blocks = segment_file(&*self.fs, &path, formats, (&self.options, durability))?;
        // Its entry made, not synced: the first sync that syncs syncs it.
        if durability == Durability::None {
            self.entries_unsynced.store(true, Ordering::Release);
        }
        let next = Segment::new(number, blocks);
        lock_builtin(&self.segments).insert(number, Arc::clone(&next));
        *self.active.write().unwrap_or_else(PoisonError::into_inner) = next;
        add(&self.counters.rotations, 1);
        Ok(())
    }

    /// Seals `segment`, no longer current: waits for its appends, syncs it
    /// as the log's durability says now. One sealed while that was `None`
    /// is synced by the first seal after it is not.
    pub(super) fn seal(&self, segment: &Segment) -> Result<()> {
        if segment.settled() {
            return Ok(());
        }
        // [DESIGN] One sealed while the log syncs nothing
        // was drained and cut as it was sealed, and is asked no sync now,
        // so it is left as it is until the log syncs again.
        if segment.sealed.load(Ordering::Acquire) && self.durability() == Durability::None {
            return Ok(());
        }
        self.healthy()?;
        segment.drain();
        // The records it holds in memory are written, whatever the log
        // syncs (RULES.md D921).
        self.written(segment).map_err(|e| self.failed(e, &format!("segment {}", segment.number)))?;
        // Its zeros filled ahead go before the sync, which then makes its
        // size the end of its records durably (`known_through`).
        segment.blocks.trim().map_err(|e| self.failed(e, &format!("segment {}", segment.number)))?;
        let durability = self.durability();
        // A log syncing nothing asks for no sync, and claims nothing durable.
        if durability != Durability::None {
            segment.blocks.sync(durability).map_err(|e| self.failed(e, &format!("segment {}", segment.number)))?;
            segment.durable_end.fetch_max(segment.blocks.end().0, Ordering::AcqRel);
        }
        segment.sealed.store(true, Ordering::Release);
        Ok(())
    }

    /// Writes to its file the records the current segment holds in memory
    /// (`Log::write_buffered`, RULES.md D921): a segment before it wrote
    /// its own as it was sealed.
    pub(super) fn write_held(&self) -> Result<()> {
        let active = Arc::clone(&self.active.read().unwrap_or_else(PoisonError::into_inner));
        self.written(&active)
    }

    /// Has `segment` write the records it holds in memory, if the log
    /// buffers any: a sync that syncs nothing, which its buffer takes as
    /// the asking and passes to no file (`buffered.rs`).
    fn written(&self, segment: &Segment) -> Result<()> {
        if self.options.buffer == 0 {
            return Ok(());
        }
        segment.blocks.sync(Durability::None)
    }

    /// Syncs the directory, and the one it is in, if a segment's entry in
    /// it may not be durable, made while the log synced nothing; a failure
    /// fails every later sync, as a segment's does. `number` is the
    /// segment synced after it, for the message.
    pub(super) fn sync_entries(&self, number: u64) -> Result<()> {
        // Taken before the sync: an entry made during it asks again.
        if !self.entries_unsynced.swap(false, Ordering::AcqRel) {
            return Ok(());
        }
        let parent = self.dir.parent().filter(|p| !p.as_os_str().is_empty());
        let synced = self.fs.sync_dir(&self.dir).and_then(|()| parent.map_or(Ok(()), |p| self.fs.sync_dir(p)));
        synced.map_err(|e| {
            self.entries_unsynced.store(true, Ordering::Release);
            self.failed(e, &format!("its directory before segment {number}"))
        })
    }

    /// Makes block `id` of `segment` durable as `durability`, not `None`,
    /// says, grouping with other syncs: if one is running, wait for it,
    /// then check again; otherwise sync for everyone, through the blocks
    /// published when the sync began: the end of the segment's blocks
    /// durable then, past `id`, as the group's sync may reach further.
    pub(super) fn sync_through(&self, segment: &Segment, id: BlockId, durability: Durability) -> Result<BlockId> {
        let reached = || BlockId(segment.durable_end.load(Ordering::Acquire));
        if id < reached() {
            return Ok(reached());
        }
        add(&self.counters.sync_requests, 1);
        let mut syncing = lock_builtin(&self.syncing);
        loop {
            if id < reached() {
                return Ok(reached());
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
        let synced = segment.blocks.sync(durability).map_err(|e| self.failed(e, &format!("segment {}", segment.number)));
        if synced.is_ok() {
            segment.durable_end.fetch_max(end.0, Ordering::AcqRel);
            add(&self.counters.syncs, 1);
        }
        *lock_builtin(&self.syncing) = false;
        self.synced.notify_all();
        synced.map(|()| reached())
    }
}

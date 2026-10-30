//! \[DESIGN\] `Pager::sync` (RULES.md D30): the pages written made durable,
//! and the free list recorded: on new trunk pages, then in the meta slot that
//! is not current, so a crash at any point leaves the previous record, and
//! the trunk pages it names, whole. Free pages at the end of the file are
//! cut off once the record is durable. Split from pager.rs (RULES.md R-SIZE-2).

use std::sync::PoisonError;
use std::sync::atomic::Ordering;

use super::format::{self, KIND_TRUNK, META_AT, Meta, NONE};
use super::{Extent, PageId, Pager, add};
use crate::disk::io::Durability;
use crate::error::Result;
use crate::sync::lock_builtin;

impl Pager {
    /// Makes the pages written durable as `durability` says, and records
    /// the free list: on new trunk pages, then in the meta slot not current,
    /// so a crash at any point leaves the previous record whole. Free pages
    /// at the end of the file are cut off. Syncs take turns.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if a write or sync fails; the previous record stays
    /// current.
    pub fn sync(&self, durability: Durability) -> Result<()> {
        let (sequence, old_trunk) = {
            let mut s = lock_builtin(&self.sync_state);
            while s.running {
                s = self.synced.wait(s).unwrap_or_else(PoisonError::into_inner);
            }
            s.running = true;
            (s.sequence, s.trunk.clone())
        };
        let outcome = self.record(sequence, &old_trunk, durability);
        let mut s = lock_builtin(&self.sync_state);
        s.running = false;
        let recorded = matches!(outcome, Ok(Some(_)));
        if let Ok(Some((meta, trunk))) = &outcome {
            s.sequence = meta.sequence;
            s.trunk.clone_from(trunk);
        }
        drop(s);
        self.synced.notify_all();
        if recorded {
            // The new record already lists the old trunk pages as free; if
            // they cannot be zeroed they stay in use: lost space, not damage.
            if self.zero_all(&old_trunk).is_ok() {
                lock_builtin(&self.free).free_recorded(&old_trunk);
            }
        }
        add(&self.counters.syncs, 1);
        outcome.map(|_| ())
    }

    /// Syncs the pages written and, if the free list changed since it was
    /// last recorded, writes it and the meta slot after `sequence`: the meta
    /// written and the new trunk pages, or `None` if nothing changed. The
    /// free run that ends the file is set aside first, recorded as cut, and
    /// cut once the record is durable. The new trunk pages come from the
    /// other free runs, so recording never grows the file, and the record
    /// lists `old_trunk` as free, so freeing it afterwards changes nothing.
    fn record(&self, sequence: u64, old_trunk: &[Extent], durability: Durability) -> Result<Option<(Meta, Vec<Extent>)>> {
        let per_trunk = self.body_size().saturating_sub(format::LIST_HEADER).checked_div(format::ENTRY).unwrap_or(1).max(1);
        let (trailing, pages, trunk, runs) = {
            let mut free = lock_builtin(&self.free);
            if !free.changed() {
                drop(free);
                self.file.sync(durability)?;
                return Ok(None);
            }
            let trailing = free.take_trailing();
            let needed = u64::try_from(free.runs().len().saturating_add(old_trunk.len()).div_ceil(per_trunk)).unwrap_or(u64::MAX);
            let trunk: Vec<Extent> = free.allocate(needed);
            let mut planned = free.clone();
            // Taken from no list, so they cannot overlap a free page.
            let _ = planned.free(old_trunk);
            free.recorded();
            (trailing, trailing.map_or(free.pages(), |t| t.start.0), trunk, planned.runs())
        };
        let trunk_pages: Vec<u64> = trunk.iter().flat_map(|e| e.start.0..e.start.0.saturating_add(e.count)).collect();
        let written = self.write_trunk(&trunk_pages, &runs, per_trunk).and_then(|()| self.file.sync(durability)).and_then(|()| {
            let meta = Meta { sequence: sequence.saturating_add(1), pages, trunk: trunk_pages.first().copied().unwrap_or(NONE) };
            let slot = META_AT.get(usize::try_from(meta.sequence % 2).unwrap_or_default()).copied().unwrap_or_default();
            self.file.write_all_at(&format::encode_meta(&*self.hash, meta), u64::try_from(slot).unwrap_or_default())?;
            self.file.sync(durability)?;
            Ok(meta)
        });
        match written {
            Ok(meta) => {
                if let Some(run) = trailing {
                    self.cut(run)?;
                }
                Ok(Some((meta, trunk)))
            }
            Err(e) => {
                // Nothing refers to the new trunk pages; as in `write`.
                let _ = self.release(&trunk);
                let mut free = lock_builtin(&self.free);
                if let Some(run) = trailing {
                    // Taken from this list, so returning it cannot overlap.
                    let _ = free.free(&[run]);
                }
                free.mark_changed();
                Err(e)
            }
        }
    }

    /// Writes `runs` on the trunk pages `pages`, `per_trunk` to a page, each
    /// pointing to the next. Every trunk page is written, even one left with
    /// no runs, since the record names them all.
    fn write_trunk(&self, pages: &[u64], runs: &[Extent], per_trunk: usize) -> Result<()> {
        let mut chunks = runs.chunks(per_trunk);
        for (n, &id) in pages.iter().enumerate() {
            let next = pages.get(n.saturating_add(1)).copied().unwrap_or(NONE);
            let mut sealed = Vec::with_capacity(self.page_size);
            self.seal(id, &format::list_body(KIND_TRUNK, chunks.next().unwrap_or_default(), next), &mut sealed);
            self.write_run(id, &sealed)?;
        }
        Ok(())
    }

    /// Holds room in its file for page `id`, where the file is kept in
    /// memory of a budget (`DiskFile::reserve`, RULES.md D56): so a page is
    /// refused as it is allocated, never as it is written back.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the budget has no room for it.
    pub fn reserve(&self, id: PageId) -> Result<()> {
        self.offset(id.0.saturating_add(1)).and_then(|end| self.file.reserve(end))
    }

    /// Cuts the file at the start of `run`, the free run that ended it, set
    /// aside by `take_trailing` so no allocation used its pages meanwhile;
    /// no lock is held during the cut.
    fn cut(&self, run: Extent) -> Result<()> {
        let cut = self.offset(run.start.0).and_then(|at| self.file.set_size(at));
        let mut free = lock_builtin(&self.free);
        if cut.is_ok() {
            self.on_file.fetch_min(run.start.0, Ordering::AcqRel);
            free.release_trailing(run);
        } else {
            // Taken from this list, so returning it cannot overlap.
            let _ = free.free(&[run]);
        }
        cut
    }
}

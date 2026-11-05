//! \[DESIGN\] Read-ahead (RULES.md D30): pages a scan will read next
//! brought into the pool before it asks for them, several in one read of
//! the file (`Pager::read_ahead`), so a scan of more pages than the pool
//! holds makes one read of the file where it would make many.
//!
//! A scan asks for the pages it will read (`ahead_of`, reading pages in
//! order; `ahead_pages`, pages it knows). In a pool a database serves
//! (`AheadReader`), they are queued, and its read-ahead job reads them
//! while the scan goes on (`read_queued`); in any other, they are read at
//! once, in the scan's thread. Pages queued or being read are at most a
//! window of the pool's: the pages it reads ahead at once, at most a
//! `READ_AHEAD_SHARE`th of its frames, so a scan never takes the pool from
//! the pages others use, nor queues more than the job can read.
//!
//! Pages in the pool already are left as they are, and it never fails a
//! read: a frame it cannot have, or a page it cannot take whole, it leaves
//! to the read that wants it, and a read of the file that fails reads
//! nothing. Pages read ahead count as `PoolCounts::read_ahead`, not as
//! misses; reading them later is a hit.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};

use super::Claim;
use crate::disk::buffer::{Ahead, AheadReader, BufferPool, FileId, Key, Pin, count};
use crate::disk::pager::PageId;
use crate::memory::Undoing;
use crate::sync::lock_builtin;

/// \[DESIGN\] The share of a pool's frames one read-ahead may fill: an
/// eighth, so a scan reading ahead leaves the rest to everything else.
const READ_AHEAD_SHARE: usize = 8;

/// \[DESIGN\] The pages a pool reads ahead at once, unless told otherwise
/// (`BufferPool::set_read_ahead`), as a database tells its pools its
/// setting STORAGE.READ_AHEAD_PAGES, of which this is the default: 128 KiB
/// of 4 KiB pages, one read of the file for as many pages.
pub const READ_AHEAD_PAGES: usize = 32;

/// The pages a pool's scans asked to be read ahead, not yet read, and
/// the reader that reads them, if a database serves the pool.
#[derive(Default)]
pub(in crate::disk::buffer) struct Queued {
    reader: RwLock<Option<Weak<AheadReader>>>,
    state: Mutex<Requests>,
}

/// The requests queued, and the pages of those being read.
#[derive(Default)]
struct Requests {
    queue: VecDeque<Request>,
    reading: usize,
}

/// Pages of a file a scan asked for, while the scan goes on.
struct Request {
    file: FileId,
    pages: Vec<u64>,
    alive: Weak<()>,
}

impl Queued {
    /// Has `reader` read what is queued from now on.
    pub(in crate::disk::buffer) fn serve(&self, reader: Weak<AheadReader>) {
        *self.reader.write().unwrap_or_else(PoisonError::into_inner) = Some(reader);
    }

    /// The reader serving the pool, if one does.
    fn reader(&self) -> Option<Arc<AheadReader>> {
        self.reader.read().unwrap_or_else(PoisonError::into_inner).as_ref().and_then(Weak::upgrade)
    }
}

impl Requests {
    /// The pages queued, of scans going on, or being read: those of scans
    /// ended taken out, and those below `passed` of the scan `ahead`, which
    /// read them itself.
    fn in_flight(&mut self, ahead: &Ahead, passed: u64) -> usize {
        for request in &mut self.queue {
            if ahead.alive.as_ref().is_some_and(|alive| request.alive.ptr_eq(&Arc::downgrade(alive))) {
                request.pages.retain(|&page| page >= passed);
            }
        }
        self.queue.retain(|r| r.alive.strong_count() > 0 && !r.pages.is_empty());
        self.queue.iter().fold(self.reading, |n, r| n.saturating_add(r.pages.len()))
    }
}

impl BufferPool {
    /// Reads ahead the pages of pager file `file` from `first`, if page
    /// `first` is not in the pool: those of the next `read_ahead_pages`
    /// not in it, at most a `READ_AHEAD_SHARE`th of its frames, each into
    /// a frame of its own, unpinned, in the calling thread. Nothing during
    /// an undo, which reads a few pages, never in order. The pages read.
    pub fn read_ahead(&self, file: FileId, first: u64) -> usize {
        let most = self.window();
        if most == 0 || Undoing::active() || self.mapped(Key { file, id: first }) {
            return 0;
        }
        let ids: Vec<u64> = (first..).take(most).collect();
        self.read_run(file, &ids)
    }

    /// For a scan reading pages of pager file `file` in order, reading
    /// page `id` now: asks for the window of pages after it, as
    /// `ahead_pages` does, once fewer than half a window of those it asked
    /// for are left ahead of it.
    pub fn ahead_of(&self, ahead: &mut Ahead, file: FileId, id: u64) {
        let window = self.window();
        let next = id.saturating_add(1);
        let half = u64::try_from(window.checked_div(2).unwrap_or_default()).unwrap_or(u64::MAX);
        if window == 0 || ahead.until > next.saturating_add(half) {
            return;
        }
        let from = ahead.until.max(next);
        // None past the file's end, where there is nothing to read.
        let end = self.pager(file).map_or(0, |pager| pager.page_count());
        let to = next.saturating_add(u64::try_from(window).unwrap_or(u64::MAX)).min(end);
        let pages: Vec<u64> = (from..to).collect();
        let used = self.ask(ahead, file, &pages, next);
        ahead.until = from.saturating_add(u64::try_from(used).unwrap_or(u64::MAX));
    }

    /// For a scan that will read `pages` of pager file `file`, in that
    /// order: asks for them to be read ahead, from the first, as many as
    /// its window has room for, those in the pool passed over. Queued for
    /// the database's read-ahead job, if one serves the pool, else read
    /// now. How many of `pages` it took, to be asked for no more; all of
    /// them if it reads none ahead, or during an undo.
    pub fn ahead_pages(&self, ahead: &mut Ahead, file: FileId, pages: &[u64]) -> usize {
        self.ask(ahead, file, pages, 0)
    }

    /// As `ahead_pages`, of a scan that has read pages below `passed`
    /// itself: those it asked for, not yet read, are taken out.
    fn ask(&self, ahead: &mut Ahead, file: FileId, pages: &[u64], passed: u64) -> usize {
        let window = self.window();
        if window == 0 || Undoing::active() {
            return pages.len();
        }
        let Some(reader) = self.queued.reader() else {
            let (used, mut wanted) = self.wanted(file, pages, window);
            wanted.sort_unstable();
            self.read_listed(file, &wanted);
            return used;
        };
        let (used, queued) = {
            let mut state = lock_builtin(&self.queued.state);
            let room = window.saturating_sub(state.in_flight(ahead, passed));
            let (used, wanted) = self.wanted(file, pages, room);
            let queued = !wanted.is_empty();
            if queued {
                let alive = ahead.alive.get_or_insert_with(Arc::default);
                state.queue.push_back(Request { file, pages: wanted, alive: Arc::downgrade(alive) });
            }
            (used, queued)
        };
        if queued {
            reader.ask();
        }
        used
    }

    /// Of `pages` of `file`, from the first: how many it takes, up to the
    /// `most`th not in the pool, and those not in the pool.
    fn wanted(&self, file: FileId, pages: &[u64], most: usize) -> (usize, Vec<u64>) {
        let mut wanted = Vec::new();
        let mut used = 0_usize;
        for &id in pages {
            if wanted.len() >= most {
                break;
            }
            used = used.saturating_add(1);
            if !self.mapped(Key { file, id }) {
                wanted.push(id);
            }
        }
        (used, wanted)
    }

    /// Reads the pages queued, of scans not yet ended, each request's in
    /// as few reads of the file as its runs of pages: the pages read.
    pub(in crate::disk::buffer) fn read_queued(&self) -> usize {
        let mut read = 0_usize;
        loop {
            let mut request = {
                let mut state = lock_builtin(&self.queued.state);
                let Some(request) = state.queue.pop_front() else { break };
                state.reading = state.reading.saturating_add(request.pages.len());
                request
            };
            if request.alive.strong_count() > 0 {
                request.pages.sort_unstable();
                read = read.saturating_add(self.read_listed(request.file, &request.pages));
            }
            let mut state = lock_builtin(&self.queued.state);
            state.reading = state.reading.saturating_sub(request.pages.len());
        }
        read
    }

    /// The pages it reads ahead at once, within its share of the frames.
    fn window(&self) -> usize {
        self.read_ahead_pages().min(self.frames.len().checked_div(READ_AHEAD_SHARE).unwrap_or_default())
    }

    /// Reads `pages` of `file`, in ascending order, ahead: each run of
    /// pages one after another in one read of the file. The pages read.
    fn read_listed(&self, file: FileId, pages: &[u64]) -> usize {
        let mut read = 0_usize;
        let mut rest = pages;
        while !rest.is_empty() {
            let run = rest.windows(2).take_while(|pair| pair.first().and_then(|a| a.checked_add(1)) == pair.get(1).copied()).count();
            let (now, later) = rest.split_at(run.saturating_add(1).min(rest.len()));
            read = read.saturating_add(self.read_run(file, now));
            rest = later;
        }
        read
    }

    /// Reads ahead those of `ids`, pages of pager file `file` in ascending
    /// order, not in the pool, in one read of the file, each into a frame
    /// of its own, unpinned: up to the first whose frame cannot be had.
    /// The pages read.
    fn read_run(&self, file: FileId, ids: &[u64]) -> usize {
        let Ok(pager) = self.pager(file) else { return 0 };
        let claims = self.claim_ahead(file, ids);
        let claimed: Vec<PageId> = claims.iter().map(|&(id, _)| PageId(id)).collect();
        let contents = pager.read_ahead(&claimed).unwrap_or_default();
        let mut read = 0_usize;
        let mut contents = contents.into_iter();
        for (id, claim) in claims {
            let key = Key { file, id };
            match contents.next().flatten() {
                Some(content) => {
                    self.filled_ahead(key, claim, content);
                    read = read.saturating_add(1);
                }
                None => self.restore_empty(claim.frame, key),
            }
        }
        read
    }

    /// The pages it reads ahead at once: `READ_AHEAD_PAGES` unless set.
    pub fn read_ahead_pages(&self) -> usize {
        self.read_ahead_pages.load(Ordering::Relaxed)
    }

    /// Reads ahead `pages` at once from now on; 0 reads none ahead.
    pub fn set_read_ahead(&self, pages: usize) {
        self.read_ahead_pages.store(pages, Ordering::Relaxed);
    }

    /// Whether `key` is mapped to a frame.
    fn mapped(&self, key: Key) -> bool {
        self.shared_map(key).is_ok_and(|map| map.contains_key(&key))
    }

    /// Frames claimed, and emptied, for those of pages `ids` of `file`
    /// not in the pool: up to the first that cannot be had.
    fn claim_ahead(&self, file: FileId, ids: &[u64]) -> Vec<(u64, Claim)> {
        let mut claims = Vec::with_capacity(ids.len());
        for &id in ids {
            let key = Key { file, id };
            let claim = match self.claim(key) {
                Ok(Some(claim)) => claim,
                Ok(None) => continue,
                Err(_) => break,
            };
            // What it held, written back; if that fails, the frame is put
            // back as it was, and the read ahead stops here.
            if self.emptied(key, claim).is_err() {
                break;
            }
            claims.push((id, claim));
        }
        claims
    }

    /// Fills `claim`'s frame, emptied, with `content`, page `key`'s, and
    /// lets it go.
    fn filled_ahead(&self, key: Key, claim: Claim, content: Vec<u8>) {
        let Ok(frame) = self.frame(claim.frame) else { return };
        *frame.content.write().unwrap_or_else(PoisonError::into_inner) = content;
        frame.dirty.store(false, Ordering::Release);
        frame.ready();
        self.eviction.accessed(claim.frame);
        self.hints.note(key, claim.frame);
        count(&self.counters.read_ahead);
        drop(Pin(frame));
    }
}

// Unit tests: an undo reads nothing ahead; a scan's requests bounded,
// passed over once it ends (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

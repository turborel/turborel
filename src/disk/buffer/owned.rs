//! \[DESIGN\] A page owned by one writer at a time (RULES.md D503). A
//! structure whose readers lock nothing keeps its writers apart by node,
//! and the frame holding the node is the place for that lock. It is one
//! word beside the latch, taken by one atomic exchange when no one else holds
//! it, no table of held locks shared by every writer of the file. The
//! frame stays pinned while owned, so it holds the same page all along.
//!
//! Ownership is not the latch. A writer owns a node across its reads and
//! its write, and readers read it meanwhile under the latch as before.
//! One that finds the page owned waits on the frame's signal; the owner
//! wakes it only if someone waits, counted before the wait is checked,
//! so a release never misses a writer and costs nothing when none waits.

use std::sync::PoisonError;
use std::sync::atomic::Ordering;

use super::{BufferPool, FileId, Frame, Key, Pin};
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::sync::lock_builtin;

/// A page owned, released when dropped, the ownership before the pin.
pub struct Owned<'a> {
    frame: &'a Frame,
    _pin: Pin<'a>,
    /// The page owned.
    pub id: PageId,
    waited: bool,
}

impl Owned<'_> {
    /// Whether another writer held the page when it was asked for.
    pub fn waited(&self) -> bool {
        self.waited
    }

    /// The waits for the page counted since it was last `settle`d, the
    /// frame's count, which stays with the frame.
    pub fn contended(&self) -> u32 {
        self.frame.contended.load(Ordering::Relaxed)
    }

    /// Clears the count of waits for the page.
    pub fn settle(&self) {
        self.frame.contended.store(0, Ordering::Relaxed);
    }
}

impl BufferPool {
    /// Owns page `id` of pager file `file`, waiting while another writer
    /// owns it, its frame pinned meanwhile.
    ///
    /// # Errors
    ///
    /// As `write`.
    pub fn own(&self, file: FileId, id: PageId) -> Result<Owned<'_>> {
        self.pager(file)?;
        let (frame, _) = self.hold(Key { file, id: id.0 }, super::pool::Fill::Read)?;
        let pin = Pin(frame);
        let waited = frame.own();
        Ok(Owned { frame, _pin: pin, id, waited })
    }
}

impl Frame {
    /// Takes the frame's ownership; whether it waited for it.
    fn own(&self) -> bool {
        if self.owned.compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed).is_ok() {
            return false;
        }
        self.owners_waiting.fetch_add(1, Ordering::SeqCst);
        self.contended.fetch_add(1, Ordering::Relaxed);
        let waiting = lock_builtin(&self.waiting);
        let waiting = self
            .changed
            .wait_while(waiting, |()| self.owned.compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed).is_err())
            .unwrap_or_else(PoisonError::into_inner);
        drop(waiting);
        self.owners_waiting.fetch_sub(1, Ordering::SeqCst);
        true
    }

    /// Gives the ownership up, waking those waiting for it. A waiter
    /// counted itself before it looked, and looks under the lock taken
    /// here, so it either sees the page free or is woken.
    fn disown(&self) {
        self.owned.store(false, Ordering::SeqCst);
        if self.owners_waiting.load(Ordering::SeqCst) > 0 {
            drop(lock_builtin(&self.waiting));
            self.changed.notify_all();
        }
    }
}

impl Drop for Owned<'_> {
    fn drop(&mut self) {
        self.frame.disown();
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    use crate::disk::buffer::{BufferPool, Clock};
    use crate::disk::encoding::Pipeline;
    use crate::disk::memory::MemoryFileSystem;
    use crate::disk::pager::Pager;
    use crate::hash::HashFunction;
    use crate::hash::crc::{CRC32_ISCSI, Crc};

    /// Writers owning one page take turns: each change they make under
    /// ownership, read then written, is kept, and the waits are counted.
    #[test]
    fn owners_of_a_page_take_turns() {
        let hash: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
        let pager = Pager::create(&MemoryFileSystem::new(), Path::new("/o"), hash, Pipeline::none(), 4096).unwrap();
        let pool = BufferPool::new(8, Box::new(Clock::new(8))).unwrap();
        let file = pool.add_pager(Arc::new(pager));
        let id = {
            let (id, mut page) = pool.new_page(file).unwrap();
            *page = vec![0; 8];
            id
        };
        let waits = AtomicU64::new(0);
        let add = || {
            let owned = pool.own(file, id).unwrap();
            let n = u64::from_le_bytes(pool.read(file, id.0).unwrap()[..8].try_into().unwrap());
            thread::yield_now();
            pool.write(file, id).unwrap()[..8].copy_from_slice(&(n + 1).to_le_bytes());
            waits.fetch_add(u64::from(owned.waited()), Ordering::Relaxed);
        };
        thread::scope(|s| {
            for _ in 0..4_u8 {
                s.spawn(|| (0..500_u16).for_each(|_| add()));
            }
        });
        assert_eq!(u64::from_le_bytes(pool.read(file, id.0).unwrap()[..8].try_into().unwrap()), 2000);
        let owned = pool.own(file, id).unwrap();
        assert!(!owned.waited());
        assert_eq!(u64::from(owned.contended()), waits.load(Ordering::Relaxed));
        owned.settle();
        assert_eq!(owned.contended(), 0);
    }
}

//! \[DESIGN\] The batches of the undo store being gathered, one a stripe of
//! threads, and the pins of each batch, counted by batch (RULES.md D31,
//! D873). Every first change of a row by a transaction appends a version,
//! in every table of the store. With one batch gathered at a time, each
//! append took the store's one lock of its batches, and then the one lock
//! of its pins to add the position to a map of them all.
//!
//! So each stripe of threads gathers a batch of its own, under a lock only
//! its threads take, and a batch's number is taken from the store as the
//! batch begins. A version appended is pinned by being counted in its
//! batch, which the append does anyway, so pinning takes no lock. A
//! release marks the version released in its batch, and a batch written
//! whose every version is released is needed no more. Only the beginning
//! and the end of a batch, and a truncation, take a lock the whole store
//! shares.
//!
//! A release is marked by the version's place in its batch, a bit each,
//! so releasing a position twice counts once, as taking a position out of
//! a map did.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::sync::{Alone, lock_builtin, thread_stripe};

/// \[DESIGN\] The stripes batches are gathered in. Threads past them share
/// one, and wait on each other only for the moment of an append, and a
/// truncation, which holds them all, takes few locks.
const STRIPES: usize = 16;
/// The bits of a word of a batch's released versions.
const WORD_BITS: usize = 64;

/// The batch a stripe gathers: its number, none before its first version
/// and once it is taken to be written, how many versions it holds, and
/// their bytes.
#[derive(Debug, Default)]
pub(super) struct Gathering {
    pub(super) number: Option<u64>,
    pub(super) count: usize,
    pub(super) bytes: usize,
    /// Its count as its pins read it, with no lock of the stripe.
    appended: Arc<AtomicUsize>,
}

impl Gathering {
    /// Counts one more version, which pins it.
    pub(super) fn count_one(&mut self, bytes: usize) {
        self.count = self.count.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
        self.appended.store(self.count, Ordering::Release);
    }
}

/// The batches being gathered, one a stripe.
pub(super) struct Stripes {
    stripes: Box<[Alone<Mutex<Gathering>>]>,
}

impl Stripes {
    /// None gathering yet.
    pub(super) fn new() -> Stripes {
        Stripes { stripes: (0..STRIPES).map(|_| Alone::default()).collect() }
    }

    /// This thread's stripe: for tests of threads on stripes apart.
    #[cfg(test)]
    pub(super) fn of_thread() -> usize {
        thread_stripe(STRIPES)
    }

    /// The batch this thread's stripe gathers, held; none if the stripe is
    /// missing, which `new` prevents.
    pub(super) fn mine(&self) -> Option<MutexGuard<'_, Gathering>> {
        self.stripes.get(thread_stripe(STRIPES)).map(|stripe| lock_builtin(&stripe.0))
    }

    /// Every stripe's batch, held, in the stripes' order: while they are,
    /// no version is appended and no batch begun or taken.
    pub(super) fn all(&self) -> Vec<MutexGuard<'_, Gathering>> {
        self.stripes.iter().map(|stripe| lock_builtin(&stripe.0)).collect()
    }
}

/// The pins of one batch: which of its versions are released, how many,
/// how many it holds, once it is taken to be written, and until then the
/// count its stripe keeps of them.
#[derive(Debug, Default)]
struct Pins {
    released: Vec<u64>,
    count: usize,
    total: Option<usize>,
    appended: Arc<AtomicUsize>,
}

impl Pins {
    /// Marks the version at `index` released: whether it was not already.
    fn release(&mut self, index: usize) -> bool {
        let (word, bit) = (index.checked_div(WORD_BITS).unwrap_or(0), index.checked_rem(WORD_BITS).unwrap_or(0));
        if self.released.len() <= word {
            self.released.resize(word.saturating_add(1), 0);
        }
        let mask = 1_u64.checked_shl(u32::try_from(bit).unwrap_or(0)).unwrap_or(0);
        let Some(slot) = self.released.get_mut(word).filter(|slot| **slot & mask == 0) else { return false };
        *slot |= mask;
        self.count = self.count.saturating_add(1);
        true
    }
}

/// The pins of the store's batches, by batch number: of each batch begun
/// and still needed. A batch written with every version released is taken
/// out, so the batches written here each hold a pinned version.
#[derive(Default)]
pub(super) struct Pinned {
    batches: Mutex<BTreeMap<u64, Pins>>,
}

impl Pinned {
    /// Batch `number` begins in `stripe`, none of its versions released.
    pub(super) fn begin(&self, number: u64, stripe: &mut Gathering) {
        *stripe = Gathering { number: Some(number), ..Gathering::default() };
        lock_builtin(&self.batches).insert(number, Pins { appended: Arc::clone(&stripe.appended), ..Pins::default() });
    }

    /// Batch `number` is taken to be written, holding `total` versions:
    /// whether every one is released already, so it is needed no more.
    pub(super) fn close(&self, number: u64, total: usize) -> bool {
        let mut batches = lock_builtin(&self.batches);
        let Some(pins) = batches.get_mut(&number) else { return false };
        pins.total = Some(total);
        let done = pins.count >= total;
        if done {
            batches.remove(&number);
        }
        done
    }

    /// Releases the version at `index` of batch `number`: whether the
    /// batch is needed no more for it, one written then taken out, one
    /// still gathered left for a truncation to give up. A version released
    /// already, or of a batch no longer kept, changes nothing.
    pub(super) fn release(&self, number: u64, index: usize) -> bool {
        let mut batches = lock_builtin(&self.batches);
        let Some(pins) = batches.get_mut(&number) else { return false };
        if !pins.release(index) {
            return false;
        }
        let Some(total) = pins.total else { return pins.count >= pins.appended.load(Ordering::Acquire) };
        let done = pins.count >= total;
        if done {
            batches.remove(&number);
        }
        done
    }

    /// How many of the `count` versions batch `number`, being gathered,
    /// holds are still pinned.
    pub(super) fn live(&self, number: u64, count: usize) -> usize {
        count.saturating_sub(lock_builtin(&self.batches).get(&number).map_or(count, |pins| pins.count))
    }

    /// Forgets batch `number`, gathered and given up with no version
    /// pinned.
    pub(super) fn forget(&self, number: u64) {
        lock_builtin(&self.batches).remove(&number);
    }

    /// The oldest batch written that holds a pinned version.
    pub(super) fn oldest_written(&self) -> Option<u64> {
        lock_builtin(&self.batches).iter().find(|(_, pins)| pins.total.is_some()).map(|(&number, _)| number)
    }

    /// The versions pinned in the batches written.
    pub(super) fn written(&self) -> usize {
        let batches = lock_builtin(&self.batches);
        batches.values().filter_map(|pins| Some(pins.total?.saturating_sub(pins.count))).fold(0, usize::saturating_add)
    }
}

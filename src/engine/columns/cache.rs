//! \[DESIGN\] The headers of frozen row groups held in memory (RULES.md
//! D31, D1090): a group's header, read from its pages, is kept while it
//! was read recently, so a group a query reads again is not read again; of
//! a store's tables together, as many bytes as its setting says at most
//! (`COLUMNSTORE.HEADER_CACHE_BYTES`, by default `BUDGET_SHARE` of the
//! memory budget), as `Group::bytes` reckons them. What it holds is
//! reserved of the database's memory budget, a `CHUNK` at a time; a header
//! the budget has no room for is read each time it is needed.
//!
//! A header held is kept in its group's own `Slot`, which the group's
//! entry of its table's directory holds, so reading one held takes no lock
//! of the cache and looks nothing up: scans and fetches of many threads
//! share no word but the group's own. The cache keeps the bytes, and gives
//! headers up in the order they were put, passing over once each one read
//! since it was last passed over, so those read lately stay.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use super::group::Group;
use crate::disk::buffer::FileId;
use crate::disk::pager::PageId;
use crate::memory::{MemoryBudget, Reservation};
use crate::settings::{Definition, Effect, Kind};
use crate::sync::lock_builtin;
use crate::value::{Decimal, Value};

/// \[DESIGN\] The most bytes of headers a store holds beside a budget of
/// the default size: 16 MiB, the headers of about 90 groups of 65,536 rows
/// of sixteen columns.
pub const HEADER_CACHE_BYTES: usize = 16 << 20;
/// \[DESIGN\] The share of the memory budget a store's headers take at
/// most by default, as a divisor: a 64th, which `HEADER_CACHE_BYTES` is of
/// the default budget (`memory::DEFAULT_BUDGET`), so a database given more
/// memory holds the headers of as much more data (RULES.md D1090).
pub const BUDGET_SHARE: usize = 64;
/// The value of the setting that says the bound is `BUDGET_SHARE` of the
/// budget.
const BY_BUDGET: i128 = -1;
/// \[DESIGN\] The name of the setting of the most bytes of headers a store
/// holds, under the store's own name (RULES.md D36).
const HEADER_CACHE_SETTING: &str = "HEADER_CACHE_BYTES";

/// The name of the setting of the most bytes of headers the store `store`
/// holds: `COLUMNSTORE.HEADER_CACHE_BYTES`.
pub fn setting_name(store: &str) -> String {
    format!("{}.{HEADER_CACHE_SETTING}", store.to_ascii_uppercase())
}

/// The setting of the most bytes of headers the store `store` holds, a
/// database's, taking effect at once.
pub fn declared(store: &str) -> Definition {
    Definition {
        name: setting_name(store),
        kind: Kind::Integer { min: BY_BUDGET, max: i128::from(i64::MAX) },
        default: Value::Exact(Decimal::new(BY_BUDGET, 0)),
        per_session: false,
        node: false,
        effect: Effect::Statement,
        doc: "The bytes of the headers of frozen row groups the column store holds in memory at most, of its tables together, reserved of MEMORY.BUDGET_BYTES as they are held; 0 for none, each read as it is needed; -1 for a 64th of MEMORY.BUDGET_BYTES.".to_owned(),
    }
}

/// The most bytes of headers a store holds, its setting `said` and the
/// memory budget of `budget` bytes: what the setting says, or of -1
/// `BUDGET_SHARE` of the budget.
pub fn bound(said: i128, budget: usize) -> usize {
    if said < 0 {
        return budget.checked_div(BUDGET_SHARE).unwrap_or(0);
    }
    usize::try_from(said).unwrap_or(usize::MAX)
}

/// \[DESIGN\] The bytes reserved of the budget at once: 1 MiB.
const CHUNK: usize = 1 << 20;

/// \[DESIGN\] Where a group's header is held, if it is: the group's own,
/// read with no lock of the cache (see the module's documentation).
#[derive(Default)]
pub struct Slot {
    group: RwLock<Option<Arc<Group>>>,
    /// Set as it is read, until the cache passes over it.
    read: AtomicBool,
}

impl Slot {
    /// The header, if it is held: as read now.
    pub fn get(&self) -> Option<Arc<Group>> {
        let group = self.group.read().ok()?.clone()?;
        // Written only as it changes, so readers of one group at once
        // share its line and do not take it from each other's cores.
        if !self.read.load(Ordering::Relaxed) {
            self.read.store(true, Ordering::Relaxed);
        }
        Some(group)
    }

    /// The header, if it is held: not counted as a read.
    pub fn peek(&self) -> Option<Arc<Group>> {
        self.group.read().ok()?.clone()
    }

    fn set(&self, group: Option<Arc<Group>>) {
        if let Ok(mut held) = self.group.write() {
            *held = group;
        }
    }
}

/// A header held: its slot, its bytes, and when it was put.
struct Held {
    slot: Arc<Slot>,
    bytes: usize,
    tick: u64,
}

/// What the cache holds, under its lock.
#[derive(Default)]
struct Headers {
    by_page: HashMap<(FileId, PageId), Held>,
    /// The headers in the order they were put, or passed over, each with
    /// when it was put: one given up or put again since is not there.
    queue: VecDeque<(u64, (FileId, PageId))>,
    /// The bytes of the headers held.
    bytes: usize,
    /// The bytes reserved of the budget for them.
    reserved: Vec<Reservation>,
    tick: u64,
}

/// \[DESIGN\] The headers of a store's groups held in memory.
pub struct HeaderCache {
    headers: Mutex<Headers>,
    most: AtomicUsize,
    budget: RwLock<Option<Arc<MemoryBudget>>>,
}

impl Default for HeaderCache {
    fn default() -> Self {
        HeaderCache::new(HEADER_CACHE_BYTES)
    }
}

impl HeaderCache {
    /// A cache of `most` bytes at most, holding none yet.
    pub fn new(most: usize) -> HeaderCache {
        HeaderCache { headers: Mutex::default(), most: AtomicUsize::new(most), budget: RwLock::new(None) }
    }

    /// Makes `most` the bytes it holds at most from now on, giving up
    /// headers until it holds no more.
    pub fn set_most(&self, most: usize) {
        self.most.store(most, Ordering::Relaxed);
        let mut headers = lock_builtin(&self.headers);
        while headers.bytes > most && headers.oldest() {}
    }

    /// Makes `budget` the one what it holds is reserved of.
    pub fn use_budget(&self, budget: Arc<MemoryBudget>) {
        if let Ok(mut held) = self.budget.write() {
            *held = Some(budget);
        }
    }

    /// Holds `group`, the header of page `page` of file `file`, in `slot`,
    /// giving up others as it must; none if it is larger than the cache,
    /// or the budget has no room for it.
    pub fn put(&self, (file, page): (FileId, PageId), slot: &Arc<Slot>, group: &Arc<Group>) {
        let (bytes, most) = (group.bytes(), self.most.load(Ordering::Relaxed));
        if bytes > most {
            return;
        }
        let mut headers = lock_builtin(&self.headers);
        headers.forget((file, page));
        while headers.bytes.saturating_add(bytes) > most && headers.oldest() {}
        let budget = self.budget.read().ok().and_then(|b| b.clone());
        while headers.reserved_bytes() < headers.bytes.saturating_add(bytes) {
            match budget.as_ref().map(|b| b.reserve(CHUNK, "the headers of frozen row groups")) {
                Some(Ok(more)) => headers.reserved.push(more),
                // No budget yet: held within the cache's bound alone.
                None => break,
                Some(Err(_)) => {
                    if !headers.oldest() {
                        return;
                    }
                }
            }
        }
        let tick = headers.tick.saturating_add(1);
        headers.tick = tick;
        headers.bytes = headers.bytes.saturating_add(bytes);
        headers.queue.push_back((tick, (file, page)));
        slot.set(Some(Arc::clone(group)));
        headers.by_page.insert((file, page), Held { slot: Arc::clone(slot), bytes, tick });
    }

    /// Gives up the header of page `page` of file `file`, if it holds it:
    /// its group is no more.
    pub fn forget(&self, key: (FileId, PageId)) {
        lock_builtin(&self.headers).forget(key);
    }

    /// The bytes of the headers it holds, and how many.
    pub fn held(&self) -> (usize, usize) {
        let headers = lock_builtin(&self.headers);
        (headers.bytes, headers.by_page.len())
    }
}

impl Headers {
    fn reserved_bytes(&self) -> usize {
        self.reserved.len().saturating_mul(CHUNK)
    }

    /// Gives up the header put longest ago of those not read since they
    /// were last passed over, each read passed over once: whether there
    /// was one to give up.
    fn oldest(&mut self) -> bool {
        // Each header is passed over at most once, so twice their number
        // of looks finds one, whatever is read meanwhile.
        let mut looks = self.queue.len().saturating_mul(2);
        while let Some((tick, key)) = self.queue.pop_front() {
            let Some(held) = self.by_page.get(&key).filter(|h| h.tick == tick) else { continue };
            looks = looks.saturating_sub(1);
            if looks > 0 && held.slot.read.swap(false, Ordering::Relaxed) {
                self.queue.push_back((tick, key));
                continue;
            }
            self.drop_held(key);
            return true;
        }
        false
    }

    fn forget(&mut self, key: (FileId, PageId)) {
        self.drop_held(key);
    }

    /// Takes out the header of `key`, and gives back what the budget no
    /// longer needs. Its place in the queue is passed by when it is come
    /// to.
    fn drop_held(&mut self, key: (FileId, PageId)) {
        if let Some(held) = self.by_page.remove(&key) {
            held.slot.set(None);
            self.bytes = self.bytes.saturating_sub(held.bytes);
        }
        while self.reserved_bytes().saturating_sub(CHUNK) >= self.bytes && self.reserved.pop().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A group of no columns, its least row id `first`.
    fn group(first: u64) -> Arc<Group> {
        Arc::new(Group {
            first,
            last: first,
            rows: 1,
            ids: Vec::new(),
            by_id: Vec::new(),
            columns: Vec::new(),
            order: Vec::new(),
            pax: false,
            edges: Vec::new(),
            sketched: Vec::new(),
            sketches: Vec::new(),
            header: vec![PageId(first)],
            place: (FileId(0), 0),
        })
    }

    const FILE: FileId = FileId(1);

    /// A slot for each of pages 0 to 9.
    fn slots() -> Vec<Arc<Slot>> {
        (0..10).map(|_| Arc::new(Slot::default())).collect()
    }

    /// Puts the header of page `page` in its slot of `slots`.
    fn put(cache: &HeaderCache, slots: &[Arc<Slot>], page: u64) {
        cache.put((FILE, PageId(page)), &slots[usize::try_from(page).unwrap()], &group(page));
    }

    /// Verifies: [DESIGN] RULES.md D31, D1090, headers held: a header put
    /// is given back by its slot, and none other; past its bound, the
    /// header put longest ago and not read since is given up first, one
    /// read since kept; a header larger than the bound is not held; one
    /// forgotten is given up, its bytes too.
    #[test]
    fn headers_not_read_lately_go_first() {
        let size = group(0).bytes();
        let (cache, slots) = (HeaderCache::new(size.saturating_mul(2)), slots());
        put(&cache, &slots, 1);
        put(&cache, &slots, 2);
        assert_eq!(slots[1].get().map(|g| g.first), Some(1));
        assert!(slots[9].get().is_none());
        put(&cache, &slots, 3);
        assert!(slots[2].get().is_none(), "not read since it was put");
        assert!(slots[1].get().is_some() && slots[3].get().is_some());
        assert_eq!(cache.held(), (size.saturating_mul(2), 2));
        cache.forget((FILE, PageId(1)));
        assert_eq!(cache.held(), (size, 1));
        assert!(slots[1].get().is_none(), "forgotten");
        let small = HeaderCache::new(size.saturating_sub(1));
        put(&small, &slots, 4);
        assert_eq!(small.held(), (0, 0), "larger than the bound");
        assert!(slots[4].get().is_none());
    }

    /// Verifies: [DESIGN] RULES.md D1090, headers held: a header read is
    /// passed over once, not for ever, so when every header held was read
    /// the one put longest ago is given up; a header looked at without
    /// being read (`Slot::peek`) is not kept by it; and a header put
    /// again is held once.
    #[test]
    fn a_header_read_is_passed_over_once() {
        let size = group(0).bytes();
        let (cache, slots) = (HeaderCache::new(size.saturating_mul(2)), slots());
        put(&cache, &slots, 1);
        put(&cache, &slots, 2);
        assert!(slots[1].get().is_some() && slots[2].get().is_some());
        put(&cache, &slots, 3);
        assert!(slots[1].peek().is_none(), "every one read: the one put longest ago");
        assert!(slots[2].peek().is_some() && slots[3].peek().is_some());
        put(&cache, &slots, 4);
        assert!(slots[2].peek().is_none(), "passed over once already, looked at but not read since");
        put(&cache, &slots, 4);
        assert_eq!(cache.held(), (size.saturating_mul(2), 2), "put again, held once");
        assert!(slots[3].peek().is_some() && slots[4].peek().is_some());
    }

    /// Verifies: [DESIGN] RULES.md D31, D36, D1090, headers held: a bound
    /// lowered gives up headers until the cache holds no more, and bounds
    /// those put after; raised, it holds more again; 0 holds none. Its
    /// setting is named under the store's name, and says its bytes, or of
    /// -1 a 64th of the memory budget.
    #[test]
    fn a_bound_changed_takes_effect_at_once() {
        let size = group(0).bytes();
        let (cache, slots) = (HeaderCache::new(size.saturating_mul(3)), slots());
        for page in 1..=3_u64 {
            put(&cache, &slots, page);
        }
        cache.set_most(size);
        assert_eq!(cache.held(), (size, 1));
        assert!(slots[3].get().is_some(), "the one put last kept");
        put(&cache, &slots, 4);
        assert_eq!(cache.held(), (size, 1), "bounded after");
        cache.set_most(size.saturating_mul(2));
        put(&cache, &slots, 5);
        assert_eq!(cache.held(), (size.saturating_mul(2), 2), "raised");
        cache.set_most(0);
        assert_eq!(cache.held(), (0, 0), "none");
        assert!(slots.iter().all(|s| s.peek().is_none()));
        assert_eq!(setting_name("columnstore"), "COLUMNSTORE.HEADER_CACHE_BYTES");
        assert_eq!(bound(4096, 1 << 30), 4096);
        assert_eq!(bound(0, 1 << 30), 0);
        assert_eq!(bound(-1, 1 << 30), HEADER_CACHE_BYTES, "of the default budget, as before");
        assert_eq!(bound(-1, 64_usize << 30_u32), 1_usize << 30_u32);
    }

    /// Verifies: [DESIGN] RULES.md D31, D38, headers held: what the cache
    /// holds is reserved of the budget a chunk at a time, and given back
    /// as it gives headers up; with no room in the budget, a header is not
    /// held.
    #[test]
    fn headers_are_reserved_of_the_budget() {
        let budget = MemoryBudget::new(CHUNK.saturating_mul(4));
        let (cache, slots) = (HeaderCache::new(HEADER_CACHE_BYTES), slots());
        cache.use_budget(Arc::clone(&budget));
        put(&cache, &slots, 1);
        assert_eq!(budget.reserved(), CHUNK, "a chunk");
        cache.forget((FILE, PageId(1)));
        assert_eq!(budget.reserved(), 0, "given back");
        let _all = budget.reserve(CHUNK.saturating_mul(4), "the rest").unwrap();
        put(&cache, &slots, 2);
        assert_eq!(cache.held(), (0, 0), "no room in the budget");
        assert!(slots[2].peek().is_none());
    }
}

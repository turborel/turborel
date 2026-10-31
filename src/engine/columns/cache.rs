//! \[DESIGN\] The headers of frozen row groups held in memory (RULES.md
//! D31): a group's header, read from its pages, is kept while it was read
//! recently, so a group a query reads again is not read again; of a store's
//! tables together, as many bytes as its setting says at most
//! (`COLUMNSTORE.HEADER_CACHE_BYTES`, `HEADER_CACHE_BYTES` by default), as
//! `Group::bytes` reckons them, the one read longest ago given up first. What it holds is
//! reserved of the database's memory budget, a `CHUNK` at a time; a header
//! the budget has no room for is read each time it is needed.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use super::group::Group;
use crate::disk::buffer::FileId;
use crate::disk::pager::PageId;
use crate::memory::{MemoryBudget, Reservation};
use crate::settings::{Definition, Effect, Kind};
use crate::sync::lock_builtin;
use crate::value::{Decimal, Value};

/// \[DESIGN\] The most bytes of headers a store holds by default: 16 MiB,
/// the headers of about 500 groups of 65,536 rows of three columns.
pub const HEADER_CACHE_BYTES: usize = 16 << 20;
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
        kind: Kind::Integer { min: 0, max: i128::from(i64::MAX) },
        default: Value::Exact(Decimal::new(i128::try_from(HEADER_CACHE_BYTES).unwrap_or(i128::MAX), 0)),
        per_session: false,
        node: false,
        effect: Effect::Statement,
        doc: "The bytes of the headers of frozen row groups the column store holds in memory at most, of its tables together, reserved of MEMORY.BUDGET_BYTES as they are held; 0 for none, each read as it is needed.".to_owned(),
    }
}
/// \[DESIGN\] The bytes reserved of the budget at once: 1 MiB.
const CHUNK: usize = 1 << 20;

/// A header held: the group, its bytes, and when it was last read.
struct Held {
    group: Arc<Group>,
    bytes: usize,
    tick: u64,
}

/// What the cache holds, under its lock.
#[derive(Default)]
struct Headers {
    by_page: HashMap<(FileId, PageId), Held>,
    /// The headers by when each was last read, the longest ago first.
    by_tick: BTreeMap<u64, (FileId, PageId)>,
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

    /// Makes `most` the bytes it holds at most from now on, giving up the
    /// headers read longest ago until it holds no more.
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

    /// The header of page `page` of file `file`, if it holds it: as read
    /// now.
    pub fn get(&self, (file, page): (FileId, PageId)) -> Option<Arc<Group>> {
        let mut headers = lock_builtin(&self.headers);
        let next = headers.tick.saturating_add(1);
        let held = headers.by_page.get_mut(&(file, page))?;
        let (was, group) = (held.tick, Arc::clone(&held.group));
        held.tick = next;
        headers.tick = next;
        headers.by_tick.remove(&was);
        headers.by_tick.insert(next, (file, page));
        Some(group)
    }

    /// Holds `group`, the header of page `page` of file `file`, giving up
    /// those read longest ago as it must; none if it is larger than the
    /// cache, or the budget has no room for it.
    pub fn put(&self, (file, page): (FileId, PageId), group: &Arc<Group>) {
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
        headers.by_tick.insert(tick, (file, page));
        headers.by_page.insert((file, page), Held { group: Arc::clone(group), bytes, tick });
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

    /// Gives up the header read longest ago: whether there was one.
    fn oldest(&mut self) -> bool {
        let Some((_, key)) = self.by_tick.pop_first() else { return false };
        self.drop_held(key);
        true
    }

    fn forget(&mut self, key: (FileId, PageId)) {
        if let Some(tick) = self.by_page.get(&key).map(|h| h.tick) {
            self.by_tick.remove(&tick);
            self.drop_held(key);
        }
    }

    /// Takes out the header of `key`, and gives back what the budget no
    /// longer needs.
    fn drop_held(&mut self, key: (FileId, PageId)) {
        if let Some(held) = self.by_page.remove(&key) {
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
            header: vec![PageId(first)],
        })
    }

    const FILE: FileId = FileId(1);

    /// Verifies: [DESIGN] RULES.md D31, headers held: a header put is
    /// given back, and none other; past its bound, the header read longest
    /// ago is given up first, one read since kept; a header larger than
    /// the bound is not held; one forgotten is given up, its bytes too.
    #[test]
    fn headers_read_longest_ago_go_first() {
        let size = group(0).bytes();
        let cache = HeaderCache::new(size.saturating_mul(2));
        cache.put((FILE, PageId(1)), &group(1));
        cache.put((FILE, PageId(2)), &group(2));
        assert_eq!(cache.get((FILE, PageId(1))).map(|g| g.first), Some(1));
        assert!(cache.get((FILE, PageId(9))).is_none());
        cache.put((FILE, PageId(3)), &group(3));
        assert!(cache.get((FILE, PageId(2))).is_none(), "read longest ago");
        assert!(cache.get((FILE, PageId(1))).is_some() && cache.get((FILE, PageId(3))).is_some());
        assert_eq!(cache.held(), (size.saturating_mul(2), 2));
        cache.forget((FILE, PageId(1)));
        assert_eq!(cache.held(), (size, 1));
        let small = HeaderCache::new(size.saturating_sub(1));
        small.put((FILE, PageId(1)), &group(1));
        assert_eq!(small.held(), (0, 0), "larger than the bound");
    }

    /// Verifies: [DESIGN] RULES.md D31, D36, headers held: a bound lowered
    /// gives up the headers read longest ago until the cache holds no
    /// more, and bounds those put after; raised, it holds more again; 0
    /// holds none. Its setting is named under the store's name.
    #[test]
    fn a_bound_changed_takes_effect_at_once() {
        let size = group(0).bytes();
        let cache = HeaderCache::new(size.saturating_mul(3));
        for page in 1..=3_u64 {
            cache.put((FILE, PageId(page)), &group(page));
        }
        cache.set_most(size);
        assert_eq!(cache.held(), (size, 1));
        assert!(cache.get((FILE, PageId(3))).is_some(), "the one read last kept");
        cache.put((FILE, PageId(4)), &group(4));
        assert_eq!(cache.held(), (size, 1), "bounded after");
        cache.set_most(size.saturating_mul(2));
        cache.put((FILE, PageId(5)), &group(5));
        assert_eq!(cache.held(), (size.saturating_mul(2), 2), "raised");
        cache.set_most(0);
        assert_eq!(cache.held(), (0, 0), "none");
        assert_eq!(setting_name("columnstore"), "COLUMNSTORE.HEADER_CACHE_BYTES");
    }

    /// Verifies: [DESIGN] RULES.md D31, D38, headers held: what the cache
    /// holds is reserved of the budget a chunk at a time, and given back
    /// as it gives headers up; with no room in the budget, a header is not
    /// held.
    #[test]
    fn headers_are_reserved_of_the_budget() {
        let budget = MemoryBudget::new(CHUNK.saturating_mul(4));
        let cache = HeaderCache::new(HEADER_CACHE_BYTES);
        cache.use_budget(Arc::clone(&budget));
        cache.put((FILE, PageId(1)), &group(1));
        assert_eq!(budget.reserved(), CHUNK, "a chunk");
        cache.forget((FILE, PageId(1)));
        assert_eq!(budget.reserved(), 0, "given back");
        let _all = budget.reserve(CHUNK.saturating_mul(4), "the rest").unwrap();
        cache.put((FILE, PageId(2)), &group(2));
        assert_eq!(cache.held(), (0, 0), "no room in the budget");
    }
}

//! \[DESIGN\] A hash table over the buffer pool, by linear hashing (Litwin,
//! "Linear Hashing: A New Tool for File and Table Addressing", VLDB 1980):
//! keys and values are bytes, a key may have many values, and the table
//! grows and shrinks a bucket at a time as it fills and empties, with no
//! directory to double. Its pages are those of one pager file, read and
//! written through the pool, so a table larger than the pool's frames
//! lives in its file: what the hash index method keeps, and where a
//! statement's operators spill (RULES.md D38).
//!
//! * **Buckets.** Bucket b is a chain of pages (`hash/bucket.rs`), its
//!   entries in the order added, found in a page by a binary search of
//!   their hashes. With 2^L + s buckets, at level L with split pointer s, a
//!   key whose hash is h is in bucket h mod 2^L, or, if that is below s,
//!   already split, in h mod 2^(L+1).
//! * **Growing.** When an insert overflows its bucket's page and the
//!   entries take more than three quarters of a page a bucket, bucket s
//!   splits: its entries for which h mod 2^(L+1) is s + 2^L move, in order,
//!   to a new bucket of that number, and s goes on, to 0 at the next level
//!   once every bucket of this one has split.
//! * **Shrinking.** When a remove empties a page and the entries take less
//!   than a quarter of a page a bucket, the last bucket merges back into
//!   the one it split from, its entries after those there, and its pages
//!   are freed.
//! * **Order.** All a key's entries are in one bucket, in the order added,
//!   through splits and merges: `get` gives them so.
//! * **Readers lock nothing.** They read the shape (level and split
//!   pointer, one word) and the bucket's first page (the directory, in
//!   memory), then the chain a page at a time under the pool's latch, and
//!   check the shape's version is as it was: a split or merge, which moves
//!   entries between pages and frees pages, makes the version odd while it
//!   does, and a reader that overlapped one reads again.
//! * **Writers** of a bucket hold its stripe's lock, so writers of
//!   different stripes go on together; a split or merge holds the locks of
//!   the buckets it changes, and one change of shape is made at a time,
//!   none waited for: a writer that finds one under way leaves the next
//!   to the writer after it. The entry and byte counts are kept by stripe.
//!
//! The directory, bucket number to first page, is kept in directory pages
//! that the meta page lists, and in memory. It holds as many buckets as
//! the meta page lists directory pages of page numbers; past that the
//! table stops splitting, its chains growing longer instead.

mod bucket;
mod directory;
mod shape;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use bucket::{Entry, fragment};
use shape::Shape;

use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::hash::murmur::murmur3_x64_128;
use crate::sync::lock_builtin;

/// The format of the meta page.
const FORMAT: u8 = 1;
/// Bytes of the meta page before its directory pages: format u8, level u8,
/// split u64, entries u64, bytes u64, directory pages u32.
const META_HEADER: usize = 30;
/// Stripes of bucket locks and counts.
const STRIPES: u64 = 64;
/// Buckets of a segment of the directory in memory.
const SEGMENT: usize = 1_024;

/// The error for a page that is not one of a hash table's.
fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a hash table is malformed: {why}"))
}

/// The hash of `key`: the same on every platform and run, as it places
/// entries in a file.
fn hash(key: &[u8]) -> u64 {
    murmur3_x64_128(key, 0).0
}

/// Adds `n` to `counter`, which wraps: the counts are summed over stripes.
fn add(counter: &AtomicU64, n: i64) {
    if n >= 0 {
        counter.fetch_add(n.unsigned_abs(), Ordering::AcqRel);
    } else {
        counter.fetch_sub(n.unsigned_abs(), Ordering::AcqRel);
    }
}

/// A stripe: the lock its buckets' writers hold, and the counts it keeps,
/// alone on its cache lines.
#[repr(align(128))]
#[derive(Default)]
struct Stripe {
    lock: Mutex<()>,
    entries: AtomicU64,
    bytes: AtomicU64,
}

/// \[DESIGN\] A linear hash table in pager file `file` of `pool`. It may be
/// shared between threads.
pub struct HashTable {
    pool: Arc<BufferPool>,
    file: FileId,
    meta: PageId,
    /// The bytes a page holds.
    capacity: usize,
    /// The shape, packed (`Shape::packed`).
    shape: AtomicU64,
    /// Odd while a change of shape is made.
    version: AtomicU64,
    /// The first page of each bucket, in segments of `SEGMENT`.
    heads: Box<[OnceLock<Box<[AtomicU64]>>]>,
    /// The directory pages; held while the shape changes.
    directory: Mutex<Vec<PageId>>,
    stripes: Box<[Stripe]>,
}

/// Entries, each with its key's hash.
type Hashed = Vec<(u64, Entry)>;

/// What a write of a bucket did: its result, and the entries and bytes it
/// added, or removed.
type Written<T> = (T, i64, i64);

impl HashTable {
    /// Creates an empty table in pager file `file` of `pool`: its meta page,
    /// a directory page and one empty bucket.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `file` is not a registered pager file; the buffer
    /// pool's errors.
    pub fn create(pool: Arc<BufferPool>, file: FileId) -> Result<HashTable> {
        let head = Self::new_page(&pool, file, bucket::empty())?;
        let directory = Self::new_page(&pool, file, head.0.to_le_bytes().to_vec())?;
        let meta = Self::new_page(&pool, file, Vec::new())?;
        let table = HashTable::with(pool, (file, meta), Shape { level: 0, split: 0 }, vec![directory])?;
        table.set_head(0, head);
        table.persist()?;
        Ok(table)
    }

    /// Opens the table whose meta page is `meta` in pager file `file`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `meta` is not a table's meta page, or its directory
    /// does not hold its buckets; as `create`.
    pub fn open(pool: Arc<BufferPool>, file: FileId, meta: PageId) -> Result<HashTable> {
        let page = pool.read(file, meta.0)?.to_vec();
        let u64_at = |at: usize| page.get(at..at.saturating_add(8)).and_then(|b| b.try_into().ok()).map(u64::from_le_bytes);
        let (Some(&FORMAT), Some(&level), Some(split), Some(entries), Some(bytes)) =
            (page.first(), page.get(1), u64_at(2), u64_at(10), u64_at(18))
        else {
            return Err(corrupt(&format!("page {} is not a meta page of this format", meta.0)));
        };
        let listed = page.get(26..30).and_then(|b| b.try_into().ok()).map_or(0, u32::from_le_bytes);
        let at = |d: u32| META_HEADER.saturating_add(usize::try_from(d).unwrap_or(usize::MAX).saturating_mul(8));
        let directory: Vec<PageId> = (0..listed)
            .map(|d| u64_at(at(d)).map(PageId))
            .collect::<Option<_>>()
            .ok_or_else(|| corrupt("its meta page lists directory pages it has not"))?;
        let shape = Shape { level, split };
        let table = HashTable::with(pool, (file, meta), shape, directory.clone())?;
        let mut b = 0_u64;
        for d in &directory {
            let ids = table.pool.read(file, d.0)?;
            for id in ids.as_chunks::<8>().0 {
                table.set_head(b, PageId(u64::from_le_bytes(*id)));
                b = b.saturating_add(1);
            }
        }
        if b != shape.buckets() {
            return Err(corrupt(&format!("its directory holds {b} buckets, not {}", shape.buckets())));
        }
        if let Some(first) = table.stripes.first() {
            first.entries.store(entries, Ordering::Release);
            first.bytes.store(bytes, Ordering::Release);
        }
        Ok(table)
    }

    fn with(pool: Arc<BufferPool>, (file, meta): (FileId, PageId), shape: Shape, directory: Vec<PageId>) -> Result<HashTable> {
        let capacity = pool.capacity(file)?;
        let per_page = capacity.checked_div(8).unwrap_or(0);
        let max_buckets = capacity.saturating_sub(META_HEADER).checked_div(8).unwrap_or(0).saturating_mul(per_page);
        let heads = (0..max_buckets.div_ceil(SEGMENT).max(1)).map(|_| OnceLock::new()).collect();
        let stripes = (0..STRIPES).map(|_| Stripe::default()).collect();
        let (shape, version, directory) = (AtomicU64::new(shape.packed()), AtomicU64::new(0), Mutex::new(directory));
        Ok(HashTable { pool, file, meta, capacity, shape, version, heads, directory, stripes })
    }

    fn new_page(pool: &BufferPool, file: FileId, content: Vec<u8>) -> Result<PageId> {
        let (id, mut page) = pool.new_page(file)?;
        *page = content;
        Ok(id)
    }

    /// Its meta page, by which `open` finds it.
    pub fn meta(&self) -> PageId {
        self.meta
    }

    /// The most bytes a key and its value may take together: an entry fits
    /// a page.
    pub fn max_entry(&self) -> usize {
        self.capacity.saturating_sub(bucket::HEADER).saturating_sub(bucket::ENTRY_OVERHEAD).min(usize::from(u16::MAX))
    }

    /// Its entries.
    pub fn len(&self) -> u64 {
        self.stripes.iter().fold(0_u64, |n, s| n.wrapping_add(s.entries.load(Ordering::Acquire)))
    }

    /// Whether it has none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The bytes its entries take.
    fn bytes(&self) -> u64 {
        self.stripes.iter().fold(0_u64, |n, s| n.wrapping_add(s.bytes.load(Ordering::Acquire)))
    }

    /// Its buckets.
    pub fn buckets(&self) -> u64 {
        self.current().buckets()
    }

    fn current(&self) -> Shape {
        Shape::unpacked(self.shape.load(Ordering::Acquire))
    }

    /// The most buckets its directory holds.
    fn max_buckets(&self) -> u64 {
        u64::try_from(self.heads.len().saturating_mul(SEGMENT)).unwrap_or(u64::MAX)
    }

    fn segment(&self, b: u64) -> Option<(&OnceLock<Box<[AtomicU64]>>, usize)> {
        let b = usize::try_from(b).ok()?;
        Some((self.heads.get(b.checked_div(SEGMENT)?)?, b.checked_rem(SEGMENT)?))
    }

    /// The first page of bucket `b`.
    fn head(&self, b: u64) -> Result<PageId> {
        let missing = || corrupt(&format!("it has no bucket {b}"));
        let (segment, at) = self.segment(b).ok_or_else(missing)?;
        let slot = segment.get().and_then(|s| s.get(at)).ok_or_else(missing)?;
        Ok(PageId(slot.load(Ordering::Acquire)))
    }

    fn set_head(&self, b: u64, page: PageId) {
        if let Some((segment, at)) = self.segment(b) {
            let slots = segment.get_or_init(|| (0..SEGMENT).map(|_| AtomicU64::new(0)).collect());
            if let Some(slot) = slots.get(at) {
                slot.store(page.0, Ordering::Release);
            }
        }
    }

    fn stripe(&self, b: u64) -> Option<&Stripe> {
        self.stripes.get(usize::try_from(b.checked_rem(STRIPES)?).ok()?)
    }

    /// The locks of the stripes of `buckets`, each once, in order.
    fn lock(&self, buckets: &[u64]) -> Vec<MutexGuard<'_, ()>> {
        let mut stripes: Vec<u64> = buckets.iter().filter_map(|b| b.checked_rem(STRIPES)).collect();
        stripes.sort_unstable();
        stripes.dedup();
        stripes.into_iter().filter_map(|s| self.stripe(s)).map(|s| lock_builtin(&s.lock)).collect()
    }

    /// Runs `write` on the first page of the bucket of a key whose hash is
    /// `h`, under its stripe's lock, the bucket found again if a split
    /// moved the key while it waited; and adds to its stripe's counts what
    /// `write` says.
    fn in_bucket<T>(&self, h: u64, write: impl Fn(PageId) -> Result<Written<T>>) -> Result<T> {
        loop {
            let b = self.current().bucket(h);
            let _locked = self.lock(&[b]);
            if self.current().bucket(h) != b {
                continue;
            }
            let (done, entries, bytes) = write(self.head(b)?)?;
            if let Some(s) = self.stripe(b) {
                add(&s.entries, entries);
                add(&s.bytes, bytes);
            }
            return Ok(done);
        }
    }

    /// Adds the entry of `key` and `value`, after any of `key` there.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it takes more than `max_entry` bytes; the buffer
    /// pool's errors.
    pub fn insert(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let n = key.len().saturating_add(value.len());
        if n > self.max_entry() {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] a hash table entry of {n} bytes is larger than a page holds")));
        }
        let (h, size) = (hash(key), i64::try_from(bucket::size(key, value)).unwrap_or(i64::MAX));
        let grew = self.in_bucket(h, |head| Ok((self.append(head, (key, value), fragment(h))?, 1, size)))?;
        if grew { self.grow() } else { Ok(()) }
    }

    /// Appends the entry to the chain that begins at `head`: to its last
    /// page, or to a new page after it; whether one was added.
    fn append(&self, head: PageId, entry: (&[u8], &[u8]), f: u32) -> Result<bool> {
        let mut at = head;
        while let Some(n) = bucket::next(&self.pool.read(self.file, at.0)?)? {
            at = n;
        }
        let mut page = self.pool.write(self.file, at)?;
        if bucket::append(&mut page, entry, (f, self.capacity))? {
            return Ok(false);
        }
        let mut fresh = bucket::empty();
        bucket::append(&mut fresh, entry, (f, self.capacity))?;
        let added = Self::new_page(&self.pool, self.file, fresh)?;
        bucket::set_next(&mut page, Some(added))?;
        Ok(true)
    }

    /// The values of `key`, in the order added.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors; SQLCODE -916 if a page is malformed.
    pub fn get(&self, key: &[u8]) -> Result<Vec<Vec<u8>>> {
        let h = hash(key);
        loop {
            let before = self.version.load(Ordering::Acquire);
            if before & 1 == 1 {
                std::thread::yield_now();
                continue;
            }
            let found = self.read_bucket(self.current().bucket(h), (key, fragment(h)), before);
            if self.version.load(Ordering::Acquire) == before {
                return found;
            }
        }
    }

    /// The values of `key` of fragment `f` in bucket `b`, read while the
    /// version is `before`: what is read after it changes is not kept.
    fn read_bucket(&self, b: u64, (key, f): (&[u8], u32), before: u64) -> Result<Vec<Vec<u8>>> {
        let mut at = Some(self.head(b)?);
        let mut out = Vec::new();
        while let Some(p) = at {
            let page = self.pool.read(self.file, p.0)?;
            out.extend(bucket::find(&page, key, f)?);
            at = bucket::next(&page)?;
            if self.version.load(Ordering::Acquire) != before {
                break;
            }
        }
        Ok(out)
    }

    /// Removes the first entry of `key` and `value`; whether there was one.
    ///
    /// # Errors
    ///
    /// As `get`.
    pub fn remove(&self, key: &[u8], value: &[u8]) -> Result<bool> {
        let (h, size) = (hash(key), i64::try_from(bucket::size(key, value)).unwrap_or(i64::MAX));
        let (removed, emptied) = self.in_bucket(h, |head| {
            let mut at = Some(head);
            while let Some(p) = at.take() {
                let mut page = self.pool.write(self.file, p)?;
                if bucket::remove(&mut page, (key, value), fragment(h))? {
                    return Ok(((true, bucket::is_empty(&page)?), -1, size.saturating_neg()));
                }
                at = bucket::next(&page)?;
            }
            Ok(((false, false), 0, 0))
        })?;
        if emptied {
            self.shrink()?;
        }
        Ok(removed)
    }

    /// Every entry, bucket by bucket, each bucket's in the order added.
    ///
    /// # Errors
    ///
    /// As `get`.
    pub fn scan(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let _changing = lock_builtin(&self.directory);
        let all: Vec<u64> = (0..STRIPES).collect();
        let _locked = self.lock(&all);
        let mut out = Vec::new();
        for b in 0..self.current().buckets() {
            out.extend(self.chain(self.head(b)?)?.1.into_iter().map(|(_, e)| e));
        }
        Ok(out)
    }

    /// Every entry's key, bucket by bucket, each bucket read under its
    /// stripe's lock alone, so writers go on meanwhile: for statistics, as
    /// a bucket split during the scan may move an entry past it, or before.
    pub fn keys(&self) -> impl Iterator<Item = Result<Vec<u8>>> + '_ {
        let (mut b, mut pending) = (0_u64, Vec::new().into_iter());
        std::iter::from_fn(move || {
            loop {
                if let Some(key) = pending.next() {
                    return Some(Ok(key));
                }
                if b >= self.current().buckets() {
                    return None;
                }
                let read = {
                    let _locked = self.lock(&[b]);
                    self.head(b).and_then(|head| self.chain(head))
                };
                b = b.saturating_add(1);
                match read {
                    Ok((_, entries)) => pending = entries.into_iter().map(|(_, (key, _))| key).collect::<Vec<_>>().into_iter(),
                    Err(e) => return Some(Err(e)),
                }
            }
        })
    }

    /// The pages of the chain that begins at `head`, and its entries, each
    /// with its key's hash.
    fn chain(&self, head: PageId) -> Result<(Vec<PageId>, Hashed)> {
        let (mut pages, mut entries) = (Vec::new(), Vec::new());
        let mut at = Some(head);
        while let Some(p) = at {
            if pages.contains(&p) {
                return Err(corrupt(&format!("page {} is in its chain twice", p.0)));
            }
            let page = self.pool.read(self.file, p.0)?;
            entries.extend(bucket::entries(&page)?.into_iter().map(|e| (hash(&e.0), e)));
            at = bucket::next(&page)?;
            pages.push(p);
        }
        Ok((pages, entries))
    }

    /// Writes `entries`, in order, over the chain whose pages are `pages`,
    /// its first page staying first, pages added as needed; the pages left
    /// over, to be freed.
    fn rewrite(&self, pages: &[PageId], entries: &[(u64, Entry)]) -> Result<Vec<PageId>> {
        let mut contents: Vec<Vec<(u32, &Entry)>> = vec![Vec::new()];
        let mut used = bucket::HEADER;
        for (h, entry) in entries {
            let size = bucket::size(&entry.0, &entry.1);
            if used.saturating_add(size) > self.capacity || contents.last().is_some_and(|c| c.len() == usize::from(u16::MAX)) {
                contents.push(Vec::new());
                used = bucket::HEADER;
            }
            used = used.saturating_add(size);
            if let Some(last) = contents.last_mut() {
                last.push((fragment(*h), entry));
            }
        }
        let mut ids: Vec<PageId> = pages.iter().copied().take(contents.len()).collect();
        while ids.len() < contents.len() {
            ids.push(Self::new_page(&self.pool, self.file, bucket::empty())?);
        }
        for (n, content) in contents.iter().enumerate() {
            let id = ids.get(n).copied().ok_or_else(|| corrupt("a chain lost a page"))?;
            *self.pool.write(self.file, id)? = bucket::build(content, ids.get(n.saturating_add(1)).copied())?;
        }
        Ok(pages.iter().skip(ids.len()).copied().collect())
    }

    /// Whether the entries take more than three quarters of a page a
    /// bucket, or less than a quarter.
    fn fill(&self, buckets: u64) -> (bool, bool) {
        let bytes = u128::from(self.bytes()).saturating_mul(4);
        let room = u128::from(buckets).saturating_mul(u128::try_from(self.capacity).unwrap_or(u128::MAX));
        (bytes > room.saturating_mul(3), bytes < room)
    }

    /// Makes `change` with the version odd, so readers that overlap it
    /// read again.
    fn changing<T>(&self, change: impl FnOnce() -> Result<T>) -> Result<T> {
        self.version.fetch_add(1, Ordering::AcqRel);
        let done = change();
        self.version.fetch_add(1, Ordering::AcqRel);
        done
    }

    /// Splits buckets while the entries fill too many pages, unless another
    /// change of shape is under way.
    fn grow(&self) -> Result<()> {
        let Ok(mut directory) = self.directory.try_lock() else { return Ok(()) };
        while self.fill(self.buckets()).0 && self.buckets() < self.max_buckets() {
            self.split(&mut directory)?;
        }
        Ok(())
    }

    /// Splits bucket s.
    fn split(&self, directory: &mut Vec<PageId>) -> Result<()> {
        let shape = self.current();
        let (from, image, grown) = (shape.split, shape.buckets(), shape.grown());
        let _locked = self.lock(&[from, image]);
        let (pages, entries) = self.chain(self.head(from)?)?;
        let wide = shape.round().saturating_mul(2).saturating_sub(1);
        let (moved, stay): (Hashed, Hashed) = entries.into_iter().partition(|(h, _)| h & wide == image);
        let head = Self::new_page(&self.pool, self.file, bucket::empty())?;
        self.rewrite(&[head], &moved)?;
        self.set_head(image, head);
        self.changing(|| {
            self.shape.store(grown.packed(), Ordering::Release);
            self.rewrite(&pages, &stay)?.iter().try_for_each(|p| self.pool.free_when_released(self.file, *p))
        })?;
        self.write_directory(directory, grown)?;
        self.write_meta(directory, grown)
    }

    /// Merges buckets back while the entries fill too few pages, unless
    /// another change of shape is under way.
    fn shrink(&self) -> Result<()> {
        let Ok(mut directory) = self.directory.try_lock() else { return Ok(()) };
        while self.fill(self.buckets()).1 && self.buckets() > 1 {
            self.merge(&mut directory)?;
        }
        Ok(())
    }

    /// Merges the last bucket into the one it split from.
    fn merge(&self, directory: &mut Vec<PageId>) -> Result<()> {
        let shape = self.current();
        let after = shape.shrunk();
        let (into, last) = (after.split, shape.buckets().saturating_sub(1));
        let _locked = self.lock(&[into, last]);
        let (gone, moved) = self.chain(self.head(last)?)?;
        let (pages, mut entries) = self.chain(self.head(into)?)?;
        entries.extend(moved);
        self.changing(|| {
            let freed = self.rewrite(&pages, &entries)?;
            self.shape.store(after.packed(), Ordering::Release);
            freed.iter().chain(&gone).try_for_each(|p| self.pool.free_when_released(self.file, *p))
        })?;
        self.write_directory(directory, after)?;
        self.write_meta(directory, after)
    }

    /// Frees every page of the table, its meta page too: for a table no one
    /// reads or writes again.
    ///
    /// # Errors
    ///
    /// The buffer pool's errors; SQLCODE -916 if a chain is malformed.
    pub fn free(&self) -> Result<()> {
        let directory = lock_builtin(&self.directory);
        for b in 0..self.current().buckets() {
            self.chain(self.head(b)?)?.0.iter().try_for_each(|p| self.pool.free(self.file, *p))?;
        }
        directory.iter().chain(std::iter::once(&self.meta)).try_for_each(|p| self.pool.free(self.file, *p))
    }
}

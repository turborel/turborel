//! \[DESIGN\] The pager (RULES.md D30): one file of fixed-size pages, read
//! and written in place by number, whose content may be of any length. What
//! does not fit in its page the pager keeps in overflow pages, listed by
//! extents (runs of consecutive pages) so it is read with few, large reads;
//! the layer above only ever reads and writes whole contents. The pager
//! allocates and frees pages (`free.rs`), using free pages before it grows
//! the file and shrinking it when its last pages are free.
//!
//! It knows nothing of what the pages hold, of caching (the buffer pool,
//! above it), or of logging: two writers of one page are coordinated above
//! it, and torn pages are kept out above it: the database's data file is
//! opened on a shadow file system (`shadow.rs`), which changes the file
//! only by a commit, made again whole after a crash. The free list on disk
//! is the one of the last `sync`; pages allocated or freed since are, after
//! a crash, as they were then, and the layers above keep the pages that
//! refer to them consistent with that.
//!
//! Every page ends with its number and a digest of its body and number:
//! this catches torn and corrupted pages and writes that landed in the
//! wrong place. The format is in `format.rs`.

mod format;
mod free;
mod known;
mod overflow;
mod sync;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

pub use format::FORMAT_VERSION;
use format::{KIND_DATA, KIND_NEW, KIND_TRUNK, META_AT, Meta, NONE, PAGE_NUMBER_BYTES};
use free::FreeList;

use super::encoding::{Encoding, Pipeline};
use super::io::{DiskFile, Durability, FileSystem, OpenMode};
use crate::error::{Code, Result, SqlError};
use crate::hash::{HashFunction, HashId};
use crate::sync::lock_builtin;

/// \[DESIGN\] The smallest page size: a modern disk sector, and room in the
/// header page for the two meta slots in sectors of their own.
pub const MIN_PAGE_SIZE: u32 = 4096;
/// \[DESIGN\] The largest page size, past which a page is more than any
/// structure reads at once.
pub const MAX_PAGE_SIZE: u32 = 64 * 1024;
/// \[DESIGN\] The default page size.
pub const DEFAULT_PAGE_SIZE: u32 = 8 * 1024;
/// \[DESIGN\] Pages zeroed per write when pages are freed: large writes,
/// with a bounded buffer.
const ZERO_CHUNK_PAGES: u64 = 128;

/// A page's number: page `n` is the `n + 1`-th page-sized slot of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageId(pub u64);

/// A run of `count` consecutive pages from `start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    /// Its first page.
    pub start: PageId,
    /// Its pages.
    pub count: u64,
}

/// What opening a pager file found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagerRecovery {
    /// Pages in the file.
    pub pages: u64,
    /// Of those, free.
    pub free: u64,
    /// Bytes of a partial page at the end, cut off: an extension a crash
    /// interrupted.
    pub cut: u64,
}

/// The counters of a pager, for the heuristics above it (RULES.md D30).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PagerCounts {
    /// Pages read with `read`.
    pub reads: u64,
    /// Of those, never written: new, with no content.
    pub new_reads: u64,
    /// Of those, whose content overflowed.
    pub overflow_reads: u64,
    /// Pages written with `write`.
    pub writes: u64,
    /// Overflow and directory pages written for them.
    pub overflow_pages_written: u64,
    /// Content bytes written with `write`.
    pub content_bytes: u64,
    /// Bytes they took once encoded: with compression, fewer.
    pub stored_bytes: u64,
    /// Syncs.
    pub syncs: u64,
    /// Pages read that failed their checksum or held another page's number.
    pub corrupt_reads: u64,
}

/// `PagerCounts`, counted lock-free.
#[derive(Debug, Default)]
struct Counters {
    reads: AtomicU64,
    new_reads: AtomicU64,
    overflow_reads: AtomicU64,
    writes: AtomicU64,
    overflow_pages_written: AtomicU64,
    content_bytes: AtomicU64,
    stored_bytes: AtomicU64,
    syncs: AtomicU64,
    corrupt_reads: AtomicU64,
}

/// Adds `n` to `counter`; it would wrap only after 2^64, which no file reaches.
fn add(counter: &AtomicU64, n: u64) {
    counter.fetch_add(n, Ordering::Relaxed);
}

/// What `sync` keeps between runs.
struct SyncState {
    /// Whether a sync is running; syncs take turns.
    running: bool,
    /// The sequence number of the current meta slot.
    sequence: u64,
    /// The trunk pages of the free list on disk.
    trunk: Vec<Extent>,
}

/// \[DESIGN\] An open pager file. It may be shared between threads: reads
/// and writes of different pages run at once; those of one page are for the
/// caller to keep apart.
pub struct Pager {
    file: Box<dyn DiskFile>,
    path: PathBuf,
    hash: Arc<dyn HashFunction>,
    /// The encodings every page's content passes through.
    pipeline: Pipeline,
    page_size: usize,
    free: Mutex<FreeList>,
    /// Pages written to the file: past them a page reads as new.
    on_file: AtomicU64,
    sync_state: Mutex<SyncState>,
    /// Signalled when a sync ends.
    synced: Condvar,
    counters: Counters,
    /// The overflow of the pages it read or wrote (`known.rs`).
    known: known::Known,
}

impl Pager {
    /// Creates a pager file at `path` with pages of `page_size` bytes,
    /// checksummed with `hash`, their content encoded by `pipeline`, and
    /// makes its creation durable.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `page_size` is not a power of two from
    /// `MIN_PAGE_SIZE` to `MAX_PAGE_SIZE`; SQLCODE -915 if the file exists
    /// or cannot be written.
    pub fn create(fs: &dyn FileSystem, path: &Path, hash: Arc<dyn HashFunction>, pipeline: Pipeline, page_size: u32) -> Result<Pager> {
        Pager::made(fs, path, hash, (pipeline, page_size), true)
    }

    /// \[DESIGN\] As `create`, its creation not made durable: a scratch
    /// file, as a spill area's is, that nothing reads once its maker is
    /// gone, and that a crash leaves among the temporary files cleared as
    /// the database opens.
    ///
    /// # Errors
    ///
    /// As `create`'s.
    pub fn create_scratch(
        fs: &dyn FileSystem,
        path: &Path,
        hash: Arc<dyn HashFunction>,
        pipeline: Pipeline,
        page_size: u32,
    ) -> Result<Pager> {
        Pager::made(fs, path, hash, (pipeline, page_size), false)
    }

    /// `create`'s work, made durable if `durable`.
    fn made(
        fs: &dyn FileSystem,
        path: &Path,
        hash: Arc<dyn HashFunction>,
        (pipeline, page_size): (Pipeline, u32),
        durable: bool,
    ) -> Result<Pager> {
        let size = check_page_size(page_size).ok_or_else(|| {
            SqlError::new(
                Code::Api,
                format!("[DESIGN] a page size must be a power of two from {MIN_PAGE_SIZE} to {MAX_PAGE_SIZE} bytes, not {page_size}"),
            )
        })?;
        let file = fs.open(path, OpenMode::CreateNew)?;
        file.write_all_at(&format::header(&*hash, &pipeline, page_size, size), 0)?;
        if durable {
            file.sync(Durability::Full)?;
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                fs.sync_dir(dir)?;
            }
        }
        let mut pager = Pager::new(file, path, hash, size);
        pager.pipeline = pipeline;
        Ok(pager)
    }

    /// Opens the pager file at `path`, whose hash function must be among
    /// `hashes` and whose encodings among `encodings`. The free list is that of the last `sync`; pages past the
    /// pages it knew are free; a partial page at the end is cut off,
    /// durably.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is not a pager file of this format, or its free
    /// list is damaged; SQLCODE -910 if its hash function or one of its
    /// encodings is not given;
    /// SQLCODE -915 if it cannot be read or cut.
    pub fn open(
        fs: &dyn FileSystem,
        path: &Path,
        hashes: &[Arc<dyn HashFunction>],
        encodings: &[Arc<dyn Encoding>],
    ) -> Result<(Pager, PagerRecovery)> {
        let file = fs.open(path, OpenMode::ReadWrite)?;
        let corrupt = |why: &str| SqlError::new(Code::Corrupt, format!("[DESIGN] {} is not a pager file: {why}", path.display()));
        let mut header = vec![0; usize::try_from(MIN_PAGE_SIZE).unwrap_or_default()];
        file.read_exact_at(&mut header, 0).map_err(|_| corrupt("it is too short"))?;
        let format::Header { hash, page_size, encodings: ids } = format::read_header(&header, hashes, path)?;
        let pipeline = Pipeline::resolve(&ids, encodings, path)?;
        let size = check_page_size(page_size).ok_or_else(|| corrupt(&format!("its page size, {page_size}, is not valid")))?;
        let meta = META_AT
            .iter()
            .filter_map(|&at| format::decode_meta(&*hash, &header, at))
            .max_by_key(|m| m.sequence)
            .ok_or_else(|| corrupt("neither meta slot is whole"))?;
        let page = u64::try_from(size).unwrap_or(u64::MAX);
        let bytes = file.size()?;
        let on_file = bytes.checked_div(page).unwrap_or_default().saturating_sub(1);
        let whole = on_file.saturating_add(1).saturating_mul(page);
        let cut = bytes.saturating_sub(whole);
        if cut > 0 {
            file.set_size(whole)?;
            file.sync(Durability::Full)?;
        }
        let mut pager = Pager::new(file, path, hash, size);
        pager.pipeline = pipeline;
        *lock_builtin(&pager.free) = FreeList::new(meta.pages);
        lock_builtin(&pager.sync_state).sequence = meta.sequence;
        pager.on_file.store(on_file, Ordering::Release);
        pager.load_free_list(meta, on_file)?;
        let recovery = PagerRecovery { pages: pager.page_count(), free: pager.free_pages(), cut };
        Ok((pager, recovery))
    }

    /// A pager of an empty file.
    fn new(file: Box<dyn DiskFile>, path: &Path, hash: Arc<dyn HashFunction>, page_size: usize) -> Pager {
        Pager {
            file,
            path: path.to_path_buf(),
            hash,
            pipeline: Pipeline::none(),
            page_size,
            free: Mutex::new(FreeList::new(0)),
            on_file: AtomicU64::new(0),
            sync_state: Mutex::new(SyncState { running: false, sequence: 0, trunk: Vec::new() }),
            synced: Condvar::new(),
            counters: Counters::default(),
            known: known::Known::new(),
        }
    }

    /// Loads the free list `meta` names, and frees pages written past the
    /// pages it knew (`on_file` are on file).
    fn load_free_list(&self, meta: Meta, on_file: u64) -> Result<()> {
        let mut runs = Vec::new();
        let mut trunk = Vec::new();
        let mut next = meta.trunk;
        while next != NONE {
            if u64::try_from(trunk.len()).unwrap_or(u64::MAX) >= meta.pages {
                return Err(self.damaged("its free list loops"));
            }
            let body =
                self.read_body(next)?.filter(|b| b.first() == Some(&KIND_TRUNK)).ok_or_else(|| self.damaged("a trunk page is not one"))?;
            let (entries, after) = format::list_entries(&body).ok_or_else(|| self.damaged("a trunk page is malformed"))?;
            trunk.push(Extent { start: PageId(next), count: 1 });
            runs.extend(entries);
            next = after;
        }
        let mut free = lock_builtin(&self.free);
        free.free(&runs).map_err(|why| self.damaged(&format!("its free list is wrong: {why}")))?;
        free.grow_free(on_file).map_err(|why| self.damaged(&format!("its free list is wrong: {why}")))?;
        drop(free);
        lock_builtin(&self.sync_state).trunk = trunk;
        Ok(())
    }

    /// The error for a damaged free list.
    fn damaged(&self, why: &str) -> SqlError {
        SqlError::new(Code::Corrupt, format!("[DESIGN] {}: {why}", self.path.display()))
    }

    /// Bytes of each page, trailer included.
    pub fn page_size(&self) -> usize {
        self.page_size
    }

    /// The most content a page holds without overflow pages: its body,
    /// less the room its pipeline's encodings may add (`Encoding::overhead`),
    /// so content of this length, encoded, still fits.
    pub fn inline_capacity(&self) -> usize {
        self.body_size().saturating_sub(format::DATA_HEADER).saturating_sub(self.pipeline.overhead())
    }

    /// Pages in the file, free ones included.
    pub fn page_count(&self) -> u64 {
        lock_builtin(&self.free).pages()
    }

    /// Free pages.
    pub fn free_pages(&self) -> u64 {
        lock_builtin(&self.free).free_pages()
    }

    /// The id of the hash function that checksums the pages.
    pub fn hash_id(&self) -> HashId {
        self.hash.id()
    }

    /// The counters now.
    pub fn counts(&self) -> PagerCounts {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        let c = &self.counters;
        PagerCounts {
            reads: get(&c.reads),
            new_reads: get(&c.new_reads),
            overflow_reads: get(&c.overflow_reads),
            writes: get(&c.writes),
            overflow_pages_written: get(&c.overflow_pages_written),
            content_bytes: get(&c.content_bytes),
            stored_bytes: get(&c.stored_bytes),
            syncs: get(&c.syncs),
            corrupt_reads: get(&c.corrupt_reads),
        }
    }
}

impl Pager {
    /// Allocates `count` pages for the layer above: free pages where there
    /// are, consecutive where it can, new ones at the end otherwise. They
    /// read as new, with no content, until written.
    pub fn allocate(&self, count: u64) -> Vec<Extent> {
        lock_builtin(&self.free).allocate(count)
    }

    /// Frees the pages of `extents`, and any overflow pages of their
    /// content. A page that can no longer be read keeps its overflow pages
    /// in use, which is safe: they are only lost space.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a page is past the end of the file, already free, or
    /// one of the pager's own (overflow, directory or free-list pages);
    /// nothing is freed then. SQLCODE -915 if a freed page cannot be zeroed.
    pub fn free(&self, extents: &[Extent]) -> Result<()> {
        lock_builtin(&self.free).clone().free(extents).map_err(|why| SqlError::new(Code::Api, format!("[DESIGN] cannot free: {why}")))?;
        let mut all = extents.to_vec();
        for e in extents {
            for id in e.start.0..e.start.0.saturating_add(e.count) {
                if let Some(known) = self.known.of(id) {
                    all.extend(known);
                    continue;
                }
                // A page that can no longer be read is freed without its
                // overflow pages, if any: lost space, not damage.
                let Ok(Some(body)) = self.read_body(id) else { continue };
                self.check_data(PageId(id), &body)?;
                all.extend(self.overflow_pages(&body).unwrap_or_default());
            }
        }
        for e in extents {
            (e.start.0..e.start.0.saturating_add(e.count)).for_each(|id| self.known.forget(id));
        }
        self.release(&all)
    }

    /// Zeroes the pages of `extents`, one write per extent, and returns them
    /// to the free list. Zeroed, a page reused later reads as new: nothing
    /// it held before, such as references to overflow pages since given to
    /// other pages, can be followed, and freed content does not linger.
    fn release(&self, extents: &[Extent]) -> Result<()> {
        self.zero_all(extents)?;
        lock_builtin(&self.free).free(extents).map_err(|why| SqlError::new(Code::Api, format!("[DESIGN] cannot free: {why}")))
    }

    /// Zeroes the pages of `extents`.
    fn zero_all(&self, extents: &[Extent]) -> Result<()> {
        for e in extents {
            self.zero(*e)?;
        }
        Ok(())
    }

    /// Zeroes the pages of `e` that are on file, `ZERO_CHUNK_PAGES` per write.
    fn zero(&self, e: Extent) -> Result<()> {
        let end = e.start.0.saturating_add(e.count).min(self.on_file.load(Ordering::Acquire));
        let zeros = vec![0; self.page_size.saturating_mul(usize::try_from(ZERO_CHUNK_PAGES).unwrap_or(1))];
        let mut at = e.start.0;
        while at < end {
            let pages = end.saturating_sub(at).min(ZERO_CHUNK_PAGES);
            let len = usize::try_from(pages).unwrap_or_default().saturating_mul(self.page_size);
            self.file.write_all_at(zeros.get(..len).unwrap_or_default(), self.offset(at)?)?;
            at = at.saturating_add(pages);
        }
        Ok(())
    }

    /// Checks that page `id` is in use, as `read` and `write` need.
    fn check_in_use(&self, id: PageId) -> Result<()> {
        let free = lock_builtin(&self.free);
        if id.0 >= free.pages() || free.is_free(id.0) {
            return Err(SqlError::new(
                Code::Api,
                format!("[DESIGN] page {} of {} is not allocated: its file has {} pages", id.0, self.path.display(), free.pages()),
            ));
        }
        Ok(())
    }

    /// Reads the content of page `id` into `content`, replacing what it
    /// held: from the page, and from its overflow pages if it has any,
    /// decoded by the file's pipeline. A
    /// page never written has no content.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the page is not allocated, or is one of the pager's
    /// own; SQLCODE -916 if a page fails its checksum, holds another page's
    /// number, or is malformed; SQLCODE -915 if the file cannot be read.
    pub fn read(&self, id: PageId, content: &mut Vec<u8>) -> Result<()> {
        self.check_in_use(id)?;
        add(&self.counters.reads, 1);
        content.clear();
        let Some(body) = self.read_body(id.0)? else {
            add(&self.counters.new_reads, 1);
            self.known.set(id.0, &[]);
            return Ok(());
        };
        self.check_data(id, &body)?;
        // Known without reading anything else: no overflow, or only what it
        // lists itself.
        if let Some((_, listed, NONE, _)) = format::data_header(&body) {
            self.known.set(id.0, &listed);
        }
        if self.pipeline.is_empty() {
            return self.load(&body, content);
        }
        let mut stored = Vec::new();
        self.load(&body, &mut stored)?;
        content.extend_from_slice(&self.pipeline.decode(id.0, &stored)?);
        Ok(())
    }

    /// Writes `content`, of any length, as the content of page `id`, in
    /// place, encoded by the file's pipeline: what does not fit goes to
    /// overflow pages, allocated here, and
    /// the page's old overflow pages are freed. Durable after `sync`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the page is not allocated, or is one of the pager's
    /// own; SQLCODE -915 if a write fails; the page is then as it was, or
    /// torn, and no new page stays allocated.
    pub fn write(&self, id: PageId, content: &[u8]) -> Result<()> {
        self.check_in_use(id)?;
        // The old overflow pages, if the old page can be read: a torn page
        // is being repaired, and its overflow pages, if any, stay in use.
        let old = match self.known.of(id.0) {
            Some(known) => known,
            None => match self.read_body(id.0) {
                Ok(Some(body)) => {
                    self.check_data(id, &body)?;
                    self.overflow_pages(&body).unwrap_or_default()
                }
                Ok(None) | Err(_) => Vec::new(),
            },
        };
        let stored = self.pipeline.encode(id.0, content)?;
        let (body, allocated) = self.store(&stored)?;
        let mut sealed = Vec::with_capacity(self.page_size);
        self.seal(id.0, &body, &mut sealed);
        if let Err(e) = self.write_run(id.0, &sealed) {
            // Torn, perhaps: read before it is written again.
            self.known.forget(id.0);
            // Nothing refers to the new overflow pages. If they cannot be
            // released they stay in use: lost space, not damage.
            let _ = self.release(&allocated);
            return Err(e);
        }
        self.known.set(id.0, &allocated);
        add(&self.counters.writes, 1);
        add(&self.counters.content_bytes, u64::try_from(content.len()).unwrap_or(u64::MAX));
        add(&self.counters.stored_bytes, u64::try_from(stored.len()).unwrap_or(u64::MAX));
        // Nothing refers to the old overflow pages any longer.
        self.release(&old)
    }

    /// Checks that `body`, read as page `id`, is a data page.
    fn check_data(&self, id: PageId, body: &[u8]) -> Result<()> {
        match body.first() {
            Some(&KIND_DATA | &KIND_NEW) => Ok(()),
            kind => Err(SqlError::new(
                Code::Api,
                format!("[DESIGN] page {} of {} is one of the pager's own (kind {kind:?}), not content", id.0, self.path.display()),
            )),
        }
    }
}

impl Pager {
    /// Bytes of a page's body: the page less its trailer.
    fn body_size(&self) -> usize {
        self.page_size.saturating_sub(PAGE_NUMBER_BYTES).saturating_sub(self.hash.digest_len())
    }

    /// The byte offset of page `id`.
    fn offset(&self, id: u64) -> Result<u64> {
        let page = u64::try_from(self.page_size).unwrap_or(u64::MAX);
        id.checked_add(1)
            .and_then(|slot| slot.checked_mul(page))
            .ok_or_else(|| SqlError::new(Code::Io, format!("[DESIGN] page {id} of {} would be beyond 2^64 bytes", self.path.display())))
    }

    /// Appends page `id` holding `body`, padded, with its trailer, to `out`.
    fn seal(&self, id: u64, body: &[u8], out: &mut Vec<u8>) {
        let start = out.len();
        out.extend_from_slice(body);
        out.resize(start.saturating_add(self.body_size()), 0);
        let digest = self.hash.hash_parts(&[out.get(start..).unwrap_or_default(), &id.to_le_bytes()]);
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(digest.as_bytes());
    }

    /// The body of page `id` within its bytes `page`; `None` if it was never
    /// written.
    fn unseal<'a>(&self, id: u64, page: &'a [u8]) -> Result<Option<&'a [u8]>> {
        if page.iter().all(|&b| b == 0) {
            return Ok(None);
        }
        let (body, trailer) = page.split_at(self.body_size().min(page.len()));
        let stored = format::le_u64(trailer, 0);
        let digest = trailer.get(PAGE_NUMBER_BYTES..).unwrap_or_default();
        if stored == Some(id) && self.hash.hash_parts(&[body, &id.to_le_bytes()]).as_bytes() == digest {
            return Ok(Some(body));
        }
        add(&self.counters.corrupt_reads, 1);
        let why = match stored.filter(|&other| other != id) {
            Some(other) => format!("it holds page {other}, written in the wrong place"),
            None => "it fails its checksum: torn or corrupted".to_owned(),
        };
        Err(SqlError::new(Code::Corrupt, format!("[DESIGN] page {id} of {}: {why}", self.path.display())))
    }

    /// The body of page `id`; `None` if it was never written.
    fn read_body(&self, id: u64) -> Result<Option<Vec<u8>>> {
        if id >= self.on_file.load(Ordering::Acquire) {
            return Ok(None);
        }
        let page = self.read_run(id, 1)?;
        Ok(self.unseal(id, &page)?.map(<[u8]>::to_vec))
    }

    /// The bytes of `count` pages from `start`, in one read.
    fn read_run(&self, start: u64, count: u64) -> Result<Vec<u8>> {
        let len =
            usize::try_from(count).ok().and_then(|n| n.checked_mul(self.page_size)).ok_or_else(|| self.damaged("an extent is too long"))?;
        let mut bytes = vec![0; len];
        self.file.read_exact_at(&mut bytes, self.offset(start)?)?;
        Ok(bytes)
    }

    /// Writes `sealed`, whole pages from `start`, in one write.
    fn write_run(&self, start: u64, sealed: &[u8]) -> Result<()> {
        self.file.write_all_at(sealed, self.offset(start)?)?;
        let pages = u64::try_from(sealed.len().checked_div(self.page_size).unwrap_or_default()).unwrap_or(u64::MAX);
        self.on_file.fetch_max(start.saturating_add(pages), Ordering::AcqRel);
        Ok(())
    }
}

/// `page_size` as a buffer size, if it is a power of two from
/// `MIN_PAGE_SIZE` to `MAX_PAGE_SIZE`.
fn check_page_size(page_size: u32) -> Option<usize> {
    let valid = page_size.is_power_of_two() && (MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&page_size);
    valid.then(|| usize::try_from(page_size).ok()).flatten()
}

//! \[DESIGN\] The segments a table's loads write their groups to (RULES.md
//! D1146). A load writes each group it makes as an extent appended to a
//! segment of its own, a block file of the database's `groups` directory,
//! never through the buffer pool as dirty pages, so neither the pool nor a
//! checkpoint carries a load's pages: a checkpoint syncs the segments
//! appended to since the last, before its batch is committed, and the
//! pool reads their blocks as it reads any block file's, clean.
//!
//! A segment is named for its table and a number the database gives once:
//! `g<table>-<n>`, its table its meta page's number. Each loading stream
//! takes a segment no other is appending to, so its extents follow one
//! another, and gives it back as its load ends; a segment past
//! `SEGMENT_BYTES` takes no more. A segment no group of its table's stored
//! directory names, and no stream holds, is removed after a checkpoint
//! and as its table is opened. Opened, a segment is read from the end of
//! the last extent its table's stored directory names, so what a crash
//! tore past it is cut off; a committed load's groups written since come
//! back from the log.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};

use super::super::RowTable;
use super::super::sealing::Sealing;
use super::Entry;
use crate::disk::block::{BlockFile, BlockId, BlockOptions, HEADER_LEN};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::encoding::Pipeline;
use crate::disk::io::{Durability, FileSystem};
use crate::engine::columns::group::{Group, Pages, Summary};
use crate::error::Result;
use crate::hash::HashFunction;
use crate::memory::Reservation;
use crate::spill::WorkAreas;
use crate::sync::lock_builtin;

/// \[DESIGN\] The bytes past which a segment takes no more extents: 1 GiB.
/// Bounded below by the groups it should hold, as a group of 65,536 rows
/// of a wide table takes some 20 MB, so about fifty; and above by the
/// space a segment holds once most of its groups are gone, as a segment
/// is given back only whole, when no group names it. At this size a table
/// of a billion rows of 300 bytes is a few hundred files, and the syncs of
/// a checkpoint are one a stream that wrote since, whatever the size.
pub const SEGMENT_BYTES: u64 = 1 << 30;

/// \[DESIGN\] Where a database's segments are: its `groups` directory, on
/// the file system its files are written to as they are, not shadowed.
pub struct SegmentPlace {
    fs: Arc<dyn FileSystem>,
    dir: PathBuf,
    hash: RwLock<Arc<dyn HashFunction>>,
    /// The functions a segment written before may be checksummed with.
    hashes: Vec<Arc<dyn HashFunction>>,
    /// The next segment number to give.
    next: AtomicU64,
    /// The segments of the tables open.
    tables: Mutex<Vec<Weak<Extents>>>,
    /// Of each, the segments its directory named as a checkpoint was
    /// captured, kept once it is committed.
    captured: Mutex<Vec<(Weak<Extents>, Vec<u64>)>>,
    /// The segments of tables dropped, removed once a checkpoint captured
    /// after the drop is committed: those dropped before the capture under
    /// way, and those since.
    dropped: Mutex<(Vec<PathBuf>, Vec<PathBuf>)>,
}

impl SegmentPlace {
    /// The functions a segment's blocks may be checksummed with.
    pub(in crate::engine::rows) fn hashes(&self) -> Vec<Arc<dyn HashFunction>> {
        let mut all = vec![self.hash()];
        all.extend(self.hashes.iter().cloned());
        all
    }

    /// The segments of `dir`, checksummed with `hash`, numbered on from the
    /// highest found there.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the directory cannot be made or read.
    pub fn new(fs: Arc<dyn FileSystem>, dir: PathBuf, hash: Arc<dyn HashFunction>) -> Result<SegmentPlace> {
        fs.create_dir_durably(&dir)?;
        let highest = fs.read_dir(&dir)?.iter().filter_map(|p| named(p)).map(|(_, n)| n).max().unwrap_or(0);
        let mut hashes = crate::hash::builtin();
        hashes.push(Arc::clone(&hash));
        let next = AtomicU64::new(highest.saturating_add(1));
        Ok(SegmentPlace {
            fs,
            dir,
            hash: RwLock::new(hash),
            hashes,
            next,
            tables: Mutex::default(),
            captured: Mutex::default(),
            dropped: Mutex::default(),
        })
    }

    /// Checksums segments made from now on with `hash`.
    pub fn set_hash(&self, hash: Arc<dyn HashFunction>) {
        if let Ok(mut held) = self.hash.write() {
            *held = hash;
        }
    }

    /// The function new extents are checksummed with.
    pub fn hash(&self) -> Arc<dyn HashFunction> {
        self.hash.read().map_or_else(|e| Arc::clone(&e.into_inner()), |h| Arc::clone(&h))
    }

    /// The tables open, those gone forgotten.
    fn open_tables(&self) -> Vec<Arc<Extents>> {
        let mut tables = lock_builtin(&self.tables);
        tables.retain(|t| t.strong_count() > 0);
        tables.iter().filter_map(Weak::upgrade).collect()
    }

    /// A checkpoint's capture, writers held: the segments each table's
    /// directory names, kept by the sweep after it is committed.
    pub fn capture(&self) {
        let named = self.open_tables().iter().map(|t| (Arc::downgrade(t), lock_builtin(&t.named).clone())).collect();
        *lock_builtin(&self.captured) = named;
        let mut dropped = lock_builtin(&self.dropped);
        let since = std::mem::take(&mut dropped.1);
        dropped.0.extend(since);
    }

    /// Syncs every segment appended to since it was last, as `durability`
    /// says: before a checkpoint's batch, whose directories may name their
    /// extents, is committed.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if a sync fails.
    pub fn sync(&self, durability: Durability) -> Result<()> {
        self.open_tables().iter().try_for_each(|t| t.sync(durability))
    }

    /// Removes, once a checkpoint is committed, each table's segments its
    /// captured directory did not name, that no entry holds and no stream
    /// appends to.
    pub fn sweep(&self) {
        for path in std::mem::take(&mut lock_builtin(&self.dropped).0) {
            self.fs.discard(&path);
        }
        for (table, kept) in std::mem::take(&mut *lock_builtin(&self.captured)) {
            if let Some(table) = table.upgrade() {
                let _ = table.sweep(&kept, false);
            }
        }
    }

    fn path(&self, table: u64, n: u64) -> PathBuf {
        self.dir.join(format!("g{table}-{n}"))
    }
}

/// The table and number a segment's file name `path` gives, if it is one.
fn named(path: &std::path::Path) -> Option<(u64, u64)> {
    let name = path.file_name()?.to_str()?.strip_prefix('g')?;
    let (table, n) = name.split_once('-')?;
    Some((table.parse().ok()?, n.parse().ok()?))
}

/// \[DESIGN\] A segment open: its file, as the pool reads it.
pub(in crate::engine::rows) struct Segment {
    pub(in crate::engine::rows) n: u64,
    pub(in crate::engine::rows) blocks: Arc<BlockFile>,
    pub(in crate::engine::rows) file: FileId,
    /// Whether a stream is appending to it.
    taken: AtomicBool,
    /// Whether it was appended to since it was last synced.
    dirty: AtomicBool,
}

/// A table's segments.
#[derive(Default)]
pub(in crate::engine::rows) struct Extents {
    place: OnceLock<(Arc<SegmentPlace>, u64)>,
    open: Mutex<BTreeMap<u64, Arc<Segment>>>,
    /// The segments its directory names as last stored.
    named: Mutex<Vec<u64>>,
    /// The database's work areas, whose budget a group in memory is
    /// charged to until it is written and logged (RULES.md D1149).
    work: OnceLock<Work>,
}

/// The work areas of a store, once it is attached.
pub(in crate::engine::rows) type Work = Arc<RwLock<Option<Arc<WorkAreas>>>>;

impl Extents {
    /// Places its segments in `place`, as those of table `table`, which a
    /// checkpoint syncs and sweeps.
    pub(in crate::engine::rows) fn place(self: &Arc<Self>, place: Arc<SegmentPlace>, table: u64) {
        lock_builtin(&place.tables).push(Arc::downgrade(self));
        let _ = self.place.set((place, table));
    }

    /// Notes the segments `summaries`, its directory's as stored, name.
    pub(in crate::engine::rows) fn name<'s>(&self, summaries: impl Iterator<Item = &'s Summary>) {
        let mut named: Vec<u64> = summaries.filter_map(|s| s.extent.map(|p| p.segment)).collect();
        named.sort_unstable();
        named.dedup();
        *lock_builtin(&self.named) = named;
    }

    /// Notes its table dropped: each of its segments removed once a
    /// checkpoint captured from now on is committed, as the last one
    /// committed may name them.
    pub(in crate::engine::rows) fn dropped(&self) {
        let Some((place, table)) = self.placed() else { return };
        let Ok(listed) = place.fs.read_dir(&place.dir) else { return };
        let own = listed.into_iter().filter(|p| named(p).is_some_and(|(t, _)| t == *table));
        lock_builtin(&place.dropped).1.extend(own);
    }

    /// Where its segments are, if anywhere: a table of a store in memory
    /// writes its loads' groups to pages.
    pub(in crate::engine::rows) fn placed(&self) -> Option<&(Arc<SegmentPlace>, u64)> {
        self.place.get()
    }

    /// A charge of `bytes` of a group held in memory to the work areas'
    /// budget, none before the store is attached (RULES.md D1149).
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the budget cannot hold them.
    pub(in crate::engine::rows) fn charge(&self, bytes: usize) -> Result<Option<Reservation>> {
        let areas = self.work.get().and_then(|w| w.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone());
        areas.map(|a| a.budget().reserve(bytes, "a load's group as it is written")).transpose()
    }

    /// Syncs each of its segments appended to since it was last, as
    /// `durability` says: before a checkpoint's batch, whose directories
    /// may name their extents, is committed.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if a sync fails.
    fn sync(&self, durability: Durability) -> Result<()> {
        let open: Vec<Arc<Segment>> = lock_builtin(&self.open).values().cloned().collect();
        for segment in open.iter().filter(|s| s.dirty.swap(false, Ordering::AcqRel)) {
            if let Err(e) = segment.blocks.sync(durability) {
                segment.dirty.store(true, Ordering::Release);
                return Err(e);
            }
        }
        Ok(())
    }

    /// Removes its segments none of `kept` is, no entry holds and no stream
    /// appends to: those open, and, if `all`, every other file of its own in
    /// the directory.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the directory cannot be read.
    pub(in crate::engine::rows) fn sweep(&self, kept: &[u64], all: bool) -> Result<()> {
        let Some((place, table)) = self.placed() else { return Ok(()) };
        let mut open = lock_builtin(&self.open);
        // One no entry holds, here or with a reader, and no stream.
        let unheld = |s: &Arc<Segment>| Arc::strong_count(s) == 1 && !s.taken.load(Ordering::Acquire);
        let gone: Vec<u64> = open.iter().filter(|(n, s)| !kept.contains(n) && unheld(s)).map(|(n, _)| *n).collect();
        for n in &gone {
            open.remove(n);
            place.fs.discard(&place.path(*table, *n));
        }
        if all {
            for path in place.fs.read_dir(&place.dir)? {
                if named(&path).is_some_and(|(t, n)| t == *table && !kept.contains(&n) && !open.contains_key(&n)) {
                    place.fs.discard(&path);
                }
            }
        }
        Ok(())
    }
}

impl RowTable {
    /// Where the pages of `entry`'s group are: its segment's extent, or
    /// its table's file.
    ///
    /// # Errors
    ///
    /// As `group_pages`.
    pub(in crate::engine::rows) fn pages_of(&self, entry: &Entry) -> Result<Pages<'_, Sealing>> {
        Ok(Pages { file: entry.file, base: entry.base(), ..self.group_pages()? })
    }

    /// Where the pages of `group` are, as it was read or written.
    ///
    /// # Errors
    ///
    /// As `group_pages`.
    pub(in crate::engine::rows) fn pages_for(&self, group: &Group) -> Result<Pages<'_, Sealing>> {
        let (file, base) = group.place;
        Ok(Pages { file, base, ..self.group_pages()? })
    }

    /// The entries of `summaries`, of a directory as stored: each segment
    /// they name opened as read from the end of the last extent they name
    /// in it, so what was written past it is cut off or passed over, and
    /// every other segment of the table removed.
    ///
    /// # Errors
    ///
    /// As `segment` and `Extents::sweep`.
    pub(in crate::engine::rows) fn placed_entries(&self, summaries: Vec<Summary>) -> Result<Vec<Arc<Entry>>> {
        let mut ends: BTreeMap<u64, u64> = BTreeMap::new();
        for p in summaries.iter().filter_map(|s| s.extent) {
            let end = ends.entry(p.segment).or_default();
            *end = (*end).max(p.base.saturating_add(p.length));
        }
        let mut entries = Vec::with_capacity(summaries.len());
        for summary in summaries {
            let extent = match summary.extent {
                Some(p) => Some((self.segment(p.segment, ends.get(&p.segment).copied().unwrap_or_default())?, p.base)),
                None => None,
            };
            entries.push(self.entry_in(summary, extent));
        }
        self.frozen.extents.sweep(&ends.keys().copied().collect::<Vec<_>>(), true)?;
        Ok(entries)
    }

    /// Segment `n`, opened if it is not, as read from `valid`, where the
    /// last extent its stored directory names ends.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it has no segments; as `BlockFile::open_from`.
    pub(in crate::engine::rows) fn segment(&self, n: u64, valid: u64) -> Result<Arc<Segment>> {
        let mut open = lock_builtin(&self.frozen.extents.open);
        if let Some(segment) = open.get(&n) {
            return Ok(Arc::clone(segment));
        }
        let (place, table) = self.frozen.extents.placed().ok_or_else(|| super::super::corrupt("it names a segment, and has none"))?;
        let options = BlockOptions::default();
        let (blocks, _) =
            BlockFile::open_from(&*place.fs, &place.path(*table, n), (&place.hashes, &[]), options, BlockId(valid.max(HEADER_LEN)))?;
        Ok(Arc::clone(open.entry(n).or_insert(self.opened(n, blocks, false))))
    }

    /// Segment `n`, its file `blocks`, registered with the pool.
    fn opened(&self, n: u64, blocks: BlockFile, taken: bool) -> Arc<Segment> {
        let blocks = Arc::new(blocks);
        let file = BufferPool::add_held_blocks(&self.pool, &blocks);
        Arc::new(Segment { n, blocks, file, taken: AtomicBool::new(taken), dirty: AtomicBool::new(false) })
    }

    /// A segment for a stream to append extents of about `bytes` to, none
    /// other appending to it: one with room, else a new one. None if the
    /// table has no segments.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if a new one cannot be made.
    pub(in crate::engine::rows) fn take_segment(&self, bytes: u64) -> Result<Option<Arc<Segment>>> {
        let Some((place, table)) = self.frozen.extents.placed() else { return Ok(None) };
        let mut open = lock_builtin(&self.frozen.extents.open);
        let room = |s: &Arc<Segment>| s.blocks.end().0.saturating_add(bytes) <= SEGMENT_BYTES;
        if let Some(free) = open.values().find(|s| room(s) && !s.taken.swap(true, Ordering::AcqRel)) {
            return Ok(Some(Arc::clone(free)));
        }
        let n = place.next.fetch_add(1, Ordering::AcqRel);
        let blocks = BlockFile::create(&*place.fs, &place.path(*table, n), place.hash(), Pipeline::none(), BlockOptions::default())?;
        let made = self.opened(n, blocks, true);
        open.insert(n, Arc::clone(&made));
        Ok(Some(made))
    }
}

impl Segment {
    /// Gives it back, its stream done with it.
    pub(in crate::engine::rows) fn give_back(&self) {
        self.taken.store(false, Ordering::Release);
    }

    /// Appends `extent`: where it starts.
    ///
    /// # Errors
    ///
    /// As `BlockFile::append_frames`.
    pub(in crate::engine::rows) fn append(&self, extent: &[u8]) -> Result<u64> {
        let at = self.blocks.append_frames(extent)?;
        self.dirty.store(true, Ordering::Release);
        Ok(at.0)
    }
}

impl super::Frozen {
    /// Its segments placed in `place`, if any, as of the table of meta
    /// page `table`.
    pub(in crate::engine::rows) fn placed(self, (place, work): (Option<&Arc<SegmentPlace>>, &Work), table: u64) -> super::Frozen {
        if let Some(place) = place {
            self.extents.place(Arc::clone(place), table);
        }
        let _ = self.extents.work.set(Arc::clone(work));
        self
    }
}

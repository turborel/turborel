//! \[DESIGN\] The large objects of a row-store table (RULES.md D31, D79): a
//! space of objects that never change once made, each named by an id the
//! table gives as it gives row ids (`rows/ids.rs`), so no two share one and
//! none is given twice, and found by a B-link tree of its own, id to page
//! (`engine/tree.rs`), made with the table's first object. The log names an
//! object by its id, never by its page, which a crash may give again.
//!
//! * A **segment** holds at most a chunk of a value's octets
//!   (`value::lob::CHUNK_OCTETS`), a character large object's cut at a
//!   character's boundary, in one page.
//! * A **directory** holds a value's kind and its segments' ids and sizes,
//!   in order: the value's id is its directory's.
//! * Each object's page begins with its id, checked on every read, and its
//!   body is sealed as the table's records are (`rows/sealing.rs`), so a
//!   page freed and given to another object is refused, never read as this
//!   one.
//!
//! Objects pass through the buffer pool, never kept in a frame
//! (`disk/buffer/pool/passing.rs`): a frame is charged one page, and a
//! chunk is many.
//!
//! A record names a value by its directory's id, octets and characters
//! (`value/types/lob.rs`), and a value read from a record is a slice of a
//! source that reads its directory only when first asked for its chunks,
//! so a scan that reads no large object's octets reads none of its pages.
//!
//! ```text
//! page:       object id u64 | the body, sealed
//! segment:    0 u8 | octets
//! directory:  1 u8 | kind u8 (0 character, 1 binary) | segments u32 |
//!             each segment's id u64, octets u32, characters u32
//! ```
//!
//! Integers are little-endian. Which version owns which object, and when
//! one is freed, is `rows/lobs/owned.rs`'s.

mod owned;
// Unit tests: objects kept, shared, copied and freed (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::corrupt;
use super::ids::key;
use super::sealing::Sealing;
use crate::bytes::{Put, Reader};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::pager::PageId;
use crate::engine::tree::BLinkTree;
use crate::error::{Code, Result, SqlError};
use crate::value::DataType;
use crate::value::lob::{Chunk, Directory, Kept, Lob, LobKind, LobSource, Part};

/// Bytes of an object's page before its body: its id.
const HEAD: usize = 8;
/// The first octet of a segment's body.
const SEGMENT: u8 = 0;
/// The first octet of a directory's body.
const DIRECTORY: u8 = 1;
/// Bytes of a directory's entry for one segment.
const ENTRY: usize = 16;

/// \[DESIGN\] The longest large object a record holds itself, not as
/// objects of its own (RULES.md D79): one longer, or one not held in
/// memory, is kept as objects.
pub(super) const IN_RECORD_OCTETS: u64 = 4096;

/// What tells the spaces of tables open apart (`Kept::table`).
static SPACES: AtomicU64 = AtomicU64::new(1);

/// \[DESIGN\] A table's space of large objects.
pub(super) struct LobSpace {
    pool: Arc<BufferPool>,
    file: FileId,
    /// Its tree, once made: by the table's first object, or as it opens.
    tree: OnceLock<BLinkTree>,
    /// Held while the tree is made.
    making: Mutex<()>,
    sealing: Arc<Sealing>,
    /// What tells it apart from the spaces of other tables open.
    tag: u64,
    /// The table's columns of large objects, and their kinds.
    columns: Vec<(usize, LobKind)>,
    /// Set when the table is dropped: its objects are freed when the last
    /// value reading them goes.
    discarded: AtomicBool,
    /// What recovery may free (`owned.rs`).
    candidates: Mutex<owned::Candidates>,
}

impl fmt::Debug for LobSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LobSpace").field("tag", &self.tag).field("columns", &self.columns).finish_non_exhaustive()
    }
}

impl LobSpace {
    /// The space of a table of columns of `types`, in `file` of `pool`, its
    /// tree the one whose meta page is `meta`, 0 for none yet.
    pub(super) fn open((pool, file): (&Arc<BufferPool>, FileId), types: &[DataType], meta: u64, sealing: Arc<Sealing>) -> Result<LobSpace> {
        let tree = OnceLock::new();
        if meta != 0 {
            let _ = tree.set(BLinkTree::open(Arc::clone(pool), file, PageId(meta))?);
        }
        let columns = types.iter().enumerate().filter_map(|(c, t)| t.lob_kind().map(|kind| (c, kind))).collect();
        Ok(LobSpace {
            pool: Arc::clone(pool),
            file,
            tree,
            making: Mutex::new(()),
            sealing,
            tag: SPACES.fetch_add(1, Ordering::Relaxed),
            columns,
            discarded: AtomicBool::new(false),
            candidates: Mutex::new(owned::Candidates::default()),
        })
    }

    /// The table's columns of large objects, and their kinds.
    pub(super) fn columns(&self) -> &[(usize, LobKind)] {
        &self.columns
    }

    /// Its tree's meta page, 0 if it has none yet.
    pub(super) fn meta(&self) -> u64 {
        self.tree.get().map_or(0, |t| t.meta().0)
    }

    /// Seals its objects as `sealing` says, before any is read.
    pub(super) fn seal_with(&mut self, sealing: Arc<Sealing>) {
        self.sealing = sealing;
    }

    /// Marks the table dropped.
    pub(super) fn discard(&self) {
        self.discarded.store(true, Ordering::Release);
    }

    /// Makes its tree if it has none: whether it made it, so the table's
    /// meta page is written to name it.
    pub(super) fn make_tree(&self) -> Result<bool> {
        if self.tree.get().is_some() {
            return Ok(false);
        }
        let _making = crate::sync::lock(&self.making)?;
        if self.tree.get().is_some() {
            return Ok(false);
        }
        let tree = BLinkTree::create(Arc::clone(&self.pool), self.file)?;
        let _ = self.tree.set(tree);
        Ok(true)
    }

    /// Its tree, which it has once it has an object.
    fn tree(&self) -> Result<&BLinkTree> {
        self.tree.get().ok_or_else(|| corrupt("a large object is named where the table keeps none"))
    }

    /// Keeps `body` as object `id`, if no object of that id is kept: the
    /// log's record of it may be read again (`rows/recovery.rs`).
    pub(super) fn put(&self, id: u64, body: &[u8]) -> Result<()> {
        let mut stored = Vec::with_capacity(HEAD.saturating_add(body.len()).saturating_add(self.sealing.overhead()));
        stored.extend_from_slice(&id.to_le_bytes());
        self.sealing.value(id, body, &mut stored)?;
        let page = self.pool.write_passing(self.file, &stored)?;
        match self.tree()?.insert(&key(id), &page.0.to_le_bytes()) {
            Ok(true) => Ok(()),
            Ok(false) => self.pool.free(self.file, page),
            Err(e) => {
                let _ = self.pool.free(self.file, page);
                Err(e)
            }
        }
    }

    /// The body of object `id`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is none, as one freed by a transaction's
    /// rollback has none, or its page is not its own.
    fn get(&self, id: u64) -> Result<Vec<u8>> {
        let Some(entry) = self.tree()?.get(&key(id))? else { return Err(gone(id)) };
        let page = <[u8; 8]>::try_from(entry.as_slice()).map(u64::from_le_bytes).map_err(|_| corrupt("a large object's entry"))?;
        let content = self.pool.read_passing(self.file, PageId(page))?;
        let owner = content.get(..HEAD).and_then(|b| <[u8; HEAD]>::try_from(b).ok()).map(u64::from_le_bytes);
        if owner != Some(id) {
            return Err(corrupt(&format!("page {page} does not hold large object {id}")));
        }
        Ok(self.sealing.plain_value(id, content.get(HEAD..).unwrap_or_default())?.into_owned())
    }

    /// Frees object `id`, a directory's segments first, if it is kept: a
    /// free again finds nothing, so it may be asked for more than once.
    /// One that cannot be freed is left: what freed it never fails for it.
    pub(super) fn free(&self, id: u64) {
        let Some(tree) = self.tree.get() else { return };
        if let Ok(body) = self.get(id)
            && let Ok((_, segments)) = parse_directory(&body)
        {
            for (segment, _) in segments {
                self.free_one(tree, segment);
            }
        }
        self.free_one(tree, id);
    }

    /// The segments of directory `id`.
    pub(super) fn segments(&self, id: u64) -> Result<Vec<u64>> {
        parse_directory(&self.get(id)?).map(|(_, s)| s.into_iter().map(|(segment, _)| segment).collect())
    }

    /// Frees the one object `id`, if kept.
    fn free_one(&self, tree: &BLinkTree, id: u64) {
        if let Ok(Some(entry)) = tree.delete(&key(id))
            && let Ok(page) = <[u8; 8]>::try_from(entry.as_slice()).map(u64::from_le_bytes)
        {
            let _ = self.pool.free(self.file, PageId(page));
        }
    }

    /// The value of the large object of directory `id`, of `kind`, `octets`
    /// and `chars` long, read from this space when asked for its chunks.
    pub(super) fn stored(self: &Arc<Self>, kind: LobKind, id: u64, (octets, chars): (u64, u64)) -> Lob {
        let source = Stored { space: Arc::clone(self), id, totals: (octets, chars), directory: OnceLock::new() };
        Lob::of_source(kind, Arc::new(source))
    }

    /// Of `lob`, the id of the directory this space keeps it as, if it is
    /// the whole of one.
    pub(super) fn kept_here(&self, lob: &Lob) -> Option<u64> {
        let [Part::Slice(s)] = lob.parts() else { return None };
        let kept = s.source.kept()?;
        (kept.table == self.tag && s.whole()).then_some(kept.id)
    }

    /// Makes the large objects `row` names by reference values this space
    /// reads, each when first asked for its chunks.
    pub(super) fn resolve(self: &Arc<Self>, row: &mut [crate::value::Value]) {
        for &(c, kind) in &self.columns {
            if let Some(l) = row.get_mut(c).and_then(crate::value::Value::as_lob_mut)
                && let [Part::Reference(id)] = l.parts()
            {
                *l = self.stored(kind, *id, (l.octets(), l.chars()));
            }
        }
    }

    /// Makes the large object `value`, column `column`'s, if a reference,
    /// a value this space reads.
    pub(super) fn resolve_column(self: &Arc<Self>, column: usize, value: &mut [crate::value::Value; 1]) {
        if let Some(&(_, kind)) = self.columns.iter().find(|(c, _)| *c == column)
            && let Some(l) = value[0].as_lob_mut()
            && let [Part::Reference(id)] = l.parts()
        {
            *l = self.stored(kind, *id, (l.octets(), l.chars()));
        }
    }

    /// Frees every object of its tree, and the tree's pages.
    fn free_all(&self) -> Result<()> {
        let Some(tree) = self.tree.get() else { return Ok(()) };
        for entry in tree.scan(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded, crate::engine::tree::Direction::Forward) {
            let (_, page) = entry?;
            if let Ok(page) = <[u8; 8]>::try_from(page.as_slice()).map(u64::from_le_bytes) {
                let _ = self.pool.free(self.file, PageId(page));
            }
        }
        tree.free()
    }
}

impl Drop for LobSpace {
    fn drop(&mut self) {
        if self.discarded.load(Ordering::Acquire) {
            let _ = self.free_all();
        }
    }
}

/// [ISO/IEC 9075-2:2023 §4.37.5] SQLSTATE 0F001: a large object no
/// longer kept, read as a locator is once invalid, by a value read in a
/// transaction that has ended, or of a transaction rolled back since.
fn gone(id: u64) -> SqlError {
    SqlError::new(
        Code::Data,
        format!(
            "[ISO/IEC 9075-2:2023 §4.37.5] large object {id} is no longer kept: it was read in a transaction that has ended, or made by one rolled back"
        ),
    )
    .with_state(crate::error::SqlState::LOCATOR_INVALID_SPECIFICATION)
}

/// The body of a segment of `octets`.
pub(super) fn segment_body(octets: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(octets.len().saturating_add(1));
    body.push(SEGMENT);
    body.extend_from_slice(octets);
    body
}

/// The body of a directory of a value of `kind` whose segments are
/// `segments`, each its id and size.
pub(super) fn directory_body(kind: LobKind, segments: &[(u64, Chunk)]) -> Result<Vec<u8>> {
    let count =
        u32::try_from(segments.len()).map_err(|_| SqlError::new(Code::Unsupported, "[DESIGN] a large object of too many chunks"))?;
    let mut body = Vec::with_capacity(segments.len().saturating_mul(ENTRY).saturating_add(6));
    body.push(DIRECTORY);
    body.push(match kind {
        LobKind::Character => 0,
        LobKind::Binary => 1,
    });
    body.put_u32(count);
    for (id, size) in segments {
        body.put_u64(*id);
        body.put_u32(size.octets);
        body.put_u32(size.chars);
    }
    Ok(body)
}

/// The kind and segments of a directory's body.
fn parse_directory(body: &[u8]) -> Result<(LobKind, Vec<(u64, Chunk)>)> {
    let mut r = Reader::new(body, "large object directory");
    if r.u8()? != DIRECTORY {
        return Err(r.corrupt("not a directory"));
    }
    let kind = match r.u8()? {
        0 => LobKind::Character,
        1 => LobKind::Binary,
        _ => return Err(r.corrupt("not a kind of large object")),
    };
    let count = usize::try_from(r.u32()?).map_err(|_| r.corrupt("too many segments"))?;
    let mut segments = Vec::with_capacity(count.min(body.len().checked_div(ENTRY).unwrap_or(0)));
    for _ in 0..count {
        let id = r.u64()?;
        segments.push((id, Chunk { octets: r.u32()?, chars: r.u32()? }));
    }
    r.finish()?;
    Ok((kind, segments))
}

/// A large object a table keeps, its directory read when first asked for.
struct Stored {
    space: Arc<LobSpace>,
    id: u64,
    totals: (u64, u64),
    /// Its directory, and its segments' ids, once read.
    directory: OnceLock<(Directory, Box<[u64]>)>,
}

impl fmt::Debug for Stored {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stored").field("space", &self.space.tag).field("id", &self.id).field("totals", &self.totals).finish_non_exhaustive()
    }
}

impl Stored {
    /// Its directory and its segments' ids, read now if not yet.
    fn loaded(&self) -> Result<&(Directory, Box<[u64]>)> {
        if let Some(loaded) = self.directory.get() {
            return Ok(loaded);
        }
        let (_, segments) = parse_directory(&self.space.get(self.id)?)?;
        let ids = segments.iter().map(|(id, _)| *id).collect();
        let directory = Directory::new(segments.into_iter().map(|(_, size)| size).collect());
        if directory.totals() != self.totals {
            return Err(corrupt(&format!("large object {} is not as long as its record says", self.id)));
        }
        let _ = self.directory.set((directory, ids));
        self.directory.get().ok_or_else(|| corrupt("a large object's directory went missing"))
    }
}

impl LobSource for Stored {
    fn totals(&self) -> (u64, u64) {
        self.totals
    }

    fn directory(&self) -> Result<&Directory> {
        self.loaded().map(|(d, _)| d)
    }

    fn chunk(&self, k: usize) -> Result<Arc<[u8]>> {
        let (directory, ids) = self.loaded()?;
        let (Some(&id), Some(size)) = (ids.get(k), directory.chunks().get(k)) else {
            return Err(corrupt(&format!("large object {} has no chunk {k}", self.id)));
        };
        let body = self.space.get(id)?;
        match body.split_first() {
            Some((&SEGMENT, octets)) if u32::try_from(octets.len()).ok() == Some(size.octets) => Ok(Arc::from(octets)),
            Some(_) | None => Err(corrupt(&format!("object {id} is not chunk {k} of large object {}", self.id))),
        }
    }

    fn kept(&self) -> Option<Kept> {
        Some(Kept { table: self.space.tag, id: self.id })
    }
}

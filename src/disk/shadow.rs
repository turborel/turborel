//! \[DESIGN\] Shadow files (RULES.md D30): a file whose writes reach it only
//! a batch at a time, at a commit, so after a crash it holds exactly what
//! the last commit left. `ShadowFileSystem` wraps a `FileSystem`; each file
//! it opens is shadowed.
//!
//! What is written goes to a side file beside it (its name and `.shadow0`
//! or `.shadow1`), a block file of checksummed blocks (`disk/block.rs`),
//! never to the file itself: each write appends, for every granule of
//! `GRANULE` bytes it touches, the granule's new content, and reads see the
//! newest. Nothing about a side file needs syncing until its commit, as
//! what is not committed is thrown away.
//!
//! * **Hold** (`ShadowFileSystem::hold`): until the next seal, writes are
//!   kept in memory, not appended to the side file, so what a checkpoint
//!   writes with writers held (`db/checkpoint.rs`) costs no I/O.
//! * **Seal** (`ShadowFileSystem::seal`): the side file written to, with
//!   what is held in memory, becomes the batch to commit, and writes go to
//!   the other side file from then on. Quick: it writes nothing.
//! * **Commit** (`commit_sealed`): the granules the sealed batch held in
//!   memory, its limit, its size and a commit block appended to its side
//!   file, which is synced; then the
//!   file cut to the limit, every granule of the batch copied into it, its
//!   size set, the file synced; then the batch forgotten. Writers go on
//!   meanwhile, into the other side file. A crash before the commit block
//!   is durable leaves the file as it was; after it, the copy is made again
//!   on opening.
//! * **Opening**: a side file ending in a commit block is copied into the
//!   file; the others are thrown away. Only a sealed side file is ever
//!   committed, and a batch is committed before the next is sealed, so at
//!   most one has a commit block.
//!
//! A side file's *limit* is the least size the file had while it was
//! written to: granules at or past it read as zeros unless written since,
//! so a file cut and grown again reads zeros where it was cut. Each file
//! commits on its own: two files are not committed together.
//!
//! ```text
//! block:  kind u8, then
//!   0 granule:  number u64, content (GRANULE bytes)
//!   1 size:     size u64
//!   2 commit:   (nothing)
//!   3 limit:    size u64
//! ```

use std::collections::HashMap;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock, Weak};

use super::block::{BlockFile, BlockId, BlockOptions};
use super::encoding::Pipeline;
use super::io::{DiskFile, Durability, FileLock, FileSystem, OpenMode};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::hash::HashFunction;
use crate::sync::{lock_builtin, read, write};

/// Bytes of a granule: the unit a write is shadowed in.
pub const GRANULE: usize = 1 << GRANULE_BITS;
const GRANULE_BITS: u32 = 12;
const GRANULE_BYTES: u64 = 1 << GRANULE_BITS;
const KIND_GRANULE: u8 = 0;
const KIND_SIZE: u8 = 1;
const KIND_COMMIT: u8 = 2;
const KIND_LIMIT: u8 = 3;

/// Side file `which` (0 or 1) of the file at `path`.
fn side_path(path: &Path, which: usize) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(if which == 0 { ".shadow0" } else { ".shadow1" });
    PathBuf::from(name)
}

/// What a side file's block says.
enum Op {
    Granule(u64, Vec<u8>),
    Size(u64),
    Commit,
    Limit(u64),
}

fn encode(op: &Op) -> Vec<u8> {
    let mut out = Vec::new();
    match op {
        Op::Granule(number, content) => {
            out.put_u8(KIND_GRANULE);
            out.put_u64(*number);
            out.extend_from_slice(content);
        }
        Op::Size(size) => {
            out.put_u8(KIND_SIZE);
            out.put_u64(*size);
        }
        Op::Commit => out.put_u8(KIND_COMMIT),
        Op::Limit(limit) => {
            out.put_u8(KIND_LIMIT);
            out.put_u64(*limit);
        }
    }
    out
}

fn decode(bytes: &[u8]) -> Result<Op> {
    let mut r = Reader::new(bytes, "shadow block");
    let op = match r.u8()? {
        KIND_GRANULE => Op::Granule(r.u64()?, r.take(GRANULE)?.to_vec()),
        KIND_SIZE => Op::Size(r.u64()?),
        KIND_COMMIT => Op::Commit,
        KIND_LIMIT => Op::Limit(r.u64()?),
        _ => return Err(r.corrupt("its kind is not known")),
    };
    r.finish()?;
    Ok(op)
}

/// The granule `number`'s offset in the file.
fn offset(number: u64) -> u64 {
    number.saturating_mul(GRANULE_BYTES)
}

/// The granule a byte's `position` is in, and where in it.
fn granule_of(position: u64) -> (u64, usize) {
    (position >> GRANULE_BITS, usize::try_from(position & (GRANULE_BYTES - 1)).unwrap_or_default())
}

/// Copies the operations of a committed side file into `base`: cut to the
/// limit, the granules in order, the size.
fn apply(base: &dyn DiskFile, ops: Vec<Op>) -> Result<()> {
    if let Some(limit) = ops.iter().find_map(|op| if let Op::Limit(limit) = op { Some(*limit) } else { None }) {
        base.set_size(limit)?;
    }
    for op in ops {
        match op {
            Op::Granule(number, content) => base.write_all_at(&content, offset(number))?,
            Op::Size(to) => base.set_size(to)?,
            Op::Commit | Op::Limit(_) => {}
        }
    }
    base.sync(Durability::Full)
}

/// A side file and the granules whose newest content it holds, in it or
/// in memory.
struct Side {
    blocks: BlockFile,
    granules: HashMap<u64, BlockId>,
    /// Granules written while held: newer than those of `granules`.
    held: HashMap<u64, Vec<u8>>,
    /// Which of the two side files it is.
    which: usize,
    /// The least size the file had while this side file was written to.
    limit: u64,
}

/// A batch sealed for its commit: its side file, and the file's size when
/// sealed.
struct Sealed {
    side: Side,
    size: u64,
}

/// What a shadowed file has written since its last commit.
struct Pending {
    active: Side,
    sealed: Option<Arc<Sealed>>,
    /// The file's size as written.
    size: u64,
    /// Whether writes are held in memory until the next seal.
    holding: bool,
}

impl Pending {
    /// Makes `content` granule `number`'s newest, in memory if held, else
    /// appended to the side file.
    fn put(&mut self, number: u64, content: Vec<u8>) -> Result<()> {
        if self.holding {
            self.active.held.insert(number, content);
        } else {
            let block = self.active.blocks.append(&encode(&Op::Granule(number, content)))?;
            self.active.held.remove(&number);
            self.active.granules.insert(number, block);
        }
        Ok(())
    }
}

/// \[DESIGN\] A shadowed file and its side files.
struct Shadow {
    base: Box<dyn DiskFile>,
    path: PathBuf,
    fs: Arc<dyn FileSystem>,
    hash: Arc<dyn HashFunction>,
    pending: RwLock<Pending>,
}

/// The content of granule `number` as `side` holds it, if it does.
fn held(side: &Side, number: u64) -> Result<Option<Vec<u8>>> {
    if let Some(content) = side.held.get(&number) {
        return Ok(Some(content.clone()));
    }
    let Some(&at) = side.granules.get(&number) else { return Ok(None) };
    let Op::Granule(_, content) = decode(&side.blocks.read(at)?)? else {
        return Err(SqlError::new(Code::Corrupt, "[DESIGN] a shadow block of a granule holds something else"));
    };
    Ok(Some(content))
}

impl Shadow {
    /// Shadows `base`, at `path`: a side file committed is copied into it
    /// first, the others thrown away.
    fn open(base: Box<dyn DiskFile>, path: &Path, fs: Arc<dyn FileSystem>, hash: Arc<dyn HashFunction>) -> Result<Shadow> {
        for which in 0..2 {
            let side = side_path(path, which);
            if !fs.exists(&side)? {
                continue;
            }
            let (blocks, _) = BlockFile::open(&*fs, &side, &[Arc::clone(&hash)], &[], BlockOptions::default())?;
            let (ops, committed) = read_ops(&blocks)?;
            if committed {
                apply(&*base, ops)?;
            }
            drop(blocks);
            fs.remove(&side)?;
        }
        let size = base.size()?;
        let blocks = fresh_side(&*fs, &side_path(path, 0), &hash)?;
        let active = Side { blocks, granules: HashMap::new(), held: HashMap::new(), which: 0, limit: size };
        Ok(Shadow {
            base,
            path: path.to_path_buf(),
            fs,
            hash,
            pending: RwLock::new(Pending { active, sealed: None, size, holding: false }),
        })
    }

    /// The content of granule `number`, as written: from the side file
    /// written to, or the sealed one, or the file, zeros past the end or a
    /// limit.
    fn granule(&self, pending: &Pending, number: u64) -> Result<Vec<u8>> {
        let start = offset(number);
        if let Some(content) = held(&pending.active, number)? {
            return Ok(content);
        }
        let mut limit = pending.active.limit;
        if let Some(sealed) = pending.sealed.as_ref().filter(|_| start < limit) {
            if let Some(content) = held(&sealed.side, number)? {
                return Ok(content);
            }
            limit = limit.min(sealed.side.limit);
        }
        let mut content = vec![0; GRANULE];
        if start < limit && start < self.base.size()? {
            let _ = self.base.read_at(&mut content, start)?;
        }
        Ok(content)
    }

    /// Seals what was written for its commit, writes going to the other
    /// side file from now on; a batch sealed before and not committed is
    /// committed first.
    fn seal(&self) -> Result<()> {
        if read(&self.pending)?.sealed.is_some() {
            self.commit_sealed()?;
        }
        let mut pending = write(&self.pending)?;
        let which = 1_usize.saturating_sub(pending.active.which);
        let blocks = fresh_side(&*self.fs, &side_path(&self.path, which), &self.hash)?;
        let now = pending.size;
        let fresh = Side { blocks, granules: HashMap::new(), held: HashMap::new(), which, limit: now };
        let side = mem::replace(&mut pending.active, fresh);
        pending.sealed = Some(Arc::new(Sealed { side, size: now }));
        pending.holding = false;
        Ok(())
    }

    /// Copies the sealed batch into the file, durably, then forgets it and
    /// removes its side file. Writers go on meanwhile.
    fn commit_sealed(&self) -> Result<()> {
        let Some(sealed) = read(&self.pending)?.sealed.clone() else { return Ok(()) };
        let side = &sealed.side;
        let mut kept: Vec<(&u64, &Vec<u8>)> = side.held.iter().collect();
        kept.sort_unstable_by_key(|(number, _)| **number);
        for (number, content) in kept {
            side.blocks.append(&encode(&Op::Granule(*number, content.clone())))?;
        }
        side.blocks.append(&encode(&Op::Limit(side.limit)))?;
        side.blocks.append(&encode(&Op::Size(sealed.size)))?;
        side.blocks.append(&encode(&Op::Commit))?;
        side.blocks.sync(Durability::Full)?;
        self.base.set_size(side.limit.min(self.base.size()?))?;
        let mut numbers: Vec<u64> = side.granules.keys().chain(side.held.keys()).copied().collect();
        numbers.sort_unstable();
        numbers.dedup();
        for number in numbers {
            if let Some(content) = held(side, number)? {
                self.base.write_all_at(&content, offset(number))?;
            }
        }
        self.base.set_size(sealed.size)?;
        self.base.sync(Durability::Full)?;
        let which = side.which;
        write(&self.pending)?.sealed = None;
        drop(sealed);
        let path = side_path(&self.path, which);
        if self.fs.exists(&path)? {
            self.fs.remove(&path)?;
        }
        Ok(())
    }
}

/// The operations of a side file up to its commit, and whether it has one.
fn read_ops(blocks: &BlockFile) -> Result<(Vec<Op>, bool)> {
    let mut cursor = blocks.cursor(blocks.first());
    let mut payload = Vec::new();
    let mut ops = Vec::new();
    while cursor.next(&mut payload)?.is_some() {
        match decode(&payload)? {
            Op::Commit => return Ok((ops, true)),
            op @ (Op::Granule(..) | Op::Size(_) | Op::Limit(_)) => ops.push(op),
        }
    }
    Ok((ops, false))
}

/// An empty side file at `path`, replacing any there.
fn fresh_side(fs: &dyn FileSystem, path: &Path, hash: &Arc<dyn HashFunction>) -> Result<BlockFile> {
    if fs.exists(path)? {
        fs.remove(path)?;
    }
    BlockFile::create(fs, path, Arc::clone(hash), Pipeline::none(), BlockOptions::default())
}

/// \[DESIGN\] A file of a `ShadowFileSystem`.
struct ShadowFile(Arc<Shadow>);

impl DiskFile for ShadowFile {
    fn read_at(&self, buf: &mut [u8], at: u64) -> Result<usize> {
        let pending = read(&self.0.pending)?;
        let wanted = u64::try_from(buf.len()).unwrap_or(u64::MAX);
        let available = usize::try_from(pending.size.saturating_sub(at).min(wanted)).unwrap_or_default();
        let mut done = 0_usize;
        while done < available {
            let position = at.saturating_add(u64::try_from(done).unwrap_or(u64::MAX));
            let (number, within) = granule_of(position);
            let content = self.0.granule(&pending, number)?;
            let n = (GRANULE.saturating_sub(within)).min(available.saturating_sub(done));
            if let (Some(to), Some(from)) = (buf.get_mut(done..done.saturating_add(n)), content.get(within..within.saturating_add(n))) {
                to.copy_from_slice(from);
            }
            done = done.saturating_add(n);
        }
        Ok(available)
    }

    fn write_all_at(&self, data: &[u8], at: u64) -> Result<()> {
        let mut pending = write(&self.0.pending)?;
        let mut done = 0_usize;
        while done < data.len() {
            let position = at.saturating_add(u64::try_from(done).unwrap_or(u64::MAX));
            let (number, within) = granule_of(position);
            let n = GRANULE.saturating_sub(within).min(data.len().saturating_sub(done));
            let mut content = if within == 0 && n == GRANULE { vec![0; GRANULE] } else { self.0.granule(&pending, number)? };
            if let (Some(to), Some(from)) = (content.get_mut(within..within.saturating_add(n)), data.get(done..done.saturating_add(n))) {
                to.copy_from_slice(from);
            }
            pending.put(number, content)?;
            done = done.saturating_add(n);
        }
        let end = at.saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
        pending.size = pending.size.max(end);
        Ok(())
    }

    fn size(&self) -> Result<u64> {
        Ok(read(&self.0.pending)?.size)
    }

    /// Granules wholly past the new size are forgotten, and the limit
    /// lowered to it; the one it falls in has its bytes past it zeroed, so
    /// growing again reads zeros.
    fn set_size(&self, size: u64) -> Result<()> {
        let mut pending = write(&self.0.pending)?;
        if size < pending.size {
            let (number, within) = granule_of(size);
            if within > 0 {
                let mut content = self.0.granule(&pending, number)?;
                if let Some(tail) = content.get_mut(within..) {
                    tail.fill(0);
                }
                pending.put(number, content)?;
            }
            let kept = if within > 0 { number } else { number.saturating_sub(1) };
            pending.active.granules.retain(|&n, _| n <= kept && offset(n) < size);
            pending.active.held.retain(|&n, _| n <= kept && offset(n) < size);
            pending.active.limit = pending.active.limit.min(size);
        }
        pending.size = size;
        Ok(())
    }

    /// Nothing: what is written becomes durable only at a commit.
    fn sync(&self, _durability: Durability) -> Result<()> {
        Ok(())
    }
}

/// \[DESIGN\] A file system whose files are shadowed (see the module's
/// documentation).
pub struct ShadowFileSystem {
    inner: Arc<dyn FileSystem>,
    hash: Arc<dyn HashFunction>,
    files: Mutex<Vec<Weak<Shadow>>>,
}

impl ShadowFileSystem {
    /// Shadows the files of `inner`, side files checksummed with `hash`.
    pub fn new(inner: Arc<dyn FileSystem>, hash: Arc<dyn HashFunction>) -> ShadowFileSystem {
        ShadowFileSystem { inner, hash, files: Mutex::new(Vec::new()) }
    }

    /// The bytes appended to the side files the files open write to, since
    /// they were last sealed; writes held in memory are not counted.
    pub fn pending_bytes(&self) -> u64 {
        let open: Vec<Arc<Shadow>> = lock_builtin(&self.files).iter().filter_map(Weak::upgrade).collect();
        open.iter().filter_map(|s| read(&s.pending).ok().map(|p| p.active.blocks.end().0.saturating_sub(p.active.blocks.first().0))).sum()
    }

    /// The files open.
    fn open_files(&self) -> Vec<Arc<Shadow>> {
        let mut files = lock_builtin(&self.files);
        files.retain(|f| f.strong_count() > 0);
        files.iter().filter_map(Weak::upgrade).collect()
    }

    /// Holds what every file open writes in memory until it is sealed (see
    /// the module's documentation).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn hold(&self) -> Result<()> {
        self.open_files().iter().try_for_each(|s| write(&s.pending).map(|mut p| p.holding = true))
    }

    /// Stops holding writes in memory, for a capture given up: what was held
    /// goes to the side files.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if a side file cannot be written.
    pub fn release(&self) -> Result<()> {
        for shadow in self.open_files() {
            let mut pending = write(&shadow.pending)?;
            pending.holding = false;
            let held: Vec<(u64, Vec<u8>)> = pending.active.held.drain().collect();
            for (number, content) in held {
                pending.put(number, content)?;
            }
        }
        Ok(())
    }

    /// Seals what every file open has written, for its commit (see the
    /// module's documentation).
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if a side file cannot be made, or a batch sealed before
    /// committed.
    pub fn seal(&self) -> Result<()> {
        self.open_files().iter().try_for_each(|s| s.seal())
    }

    /// Commits the batch every file open sealed, each at once.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if a side file or a file cannot be written or synced;
    /// a file whose commit block was made durable is committed when opened,
    /// one whose was not by the next commit.
    pub fn commit_sealed(&self) -> Result<()> {
        self.open_files().iter().try_for_each(|s| s.commit_sealed())
    }

    /// Seals and commits what every file open has written.
    ///
    /// # Errors
    ///
    /// As `seal` and `commit_sealed`.
    pub fn commit(&self) -> Result<()> {
        self.seal()?;
        self.commit_sealed()
    }
}

impl FileSystem for ShadowFileSystem {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn DiskFile>> {
        let base = self.inner.open(path, mode)?;
        let shadow = Arc::new(Shadow::open(base, path, Arc::clone(&self.inner), Arc::clone(&self.hash))?);
        lock_builtin(&self.files).push(Arc::downgrade(&shadow));
        Ok(Box::new(ShadowFile(shadow)))
    }
    /// Removes its side files too.
    fn remove(&self, path: &Path) -> Result<()> {
        self.inner.remove(path)?;
        for which in 0..2 {
            let side = side_path(path, which);
            if self.inner.exists(&side)? {
                self.inner.remove(&side)?;
            }
        }
        Ok(())
    }
    fn exists(&self, path: &Path) -> Result<bool> {
        self.inner.exists(path)
    }
    fn lock(&self, path: &Path) -> Result<FileLock> {
        self.inner.lock(path)
    }
    fn create_dir_all(&self, path: &Path) -> Result<()> {
        self.inner.create_dir_all(path)
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        self.inner.read_dir(path)
    }
    fn sync_dir(&self, path: &Path) -> Result<()> {
        self.inner.sync_dir(path)
    }
}

// Unit tests: reads see writes, a commit makes them the file's, a crash
// before a commit loses them and after one keeps them (RULES.md R-TEST-1,
// R-TEST-2, R-TEST-7).
#[cfg(test)]
mod tests;

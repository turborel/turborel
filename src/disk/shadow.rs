//! \[DESIGN\] Shadow files (RULES.md D30): a file whose writes reach it only
//! a batch at a time, at a commit, so after a crash it holds exactly what
//! the last commit left. `ShadowFileSystem` wraps a `FileSystem`; each file
//! it opens is shadowed, and each file it makes or removes is made or
//! removed at a commit.
//!
//! What is written goes to a side file beside it (its name and `.shadow0`
//! or `.shadow1`), a block file of checksummed blocks (`disk/block.rs`),
//! never to the file itself: each write appends, for every granule of
//! `GRANULE` bytes it touches, the granule's new content, and reads see the
//! newest. A side file is made at the first block appended to it. Nothing
//! about it needs syncing until its commit, as what is not committed is
//! thrown away.
//!
//! * **Hold** (`ShadowFileSystem::hold`): until the next seal, writes are
//!   kept in memory, not appended to the side file, so what a checkpoint
//!   writes with writers held (`db/checkpoint.rs`) costs no I/O.
//! * **Seal** (`ShadowFileSystem::seal`): the side file written to, with
//!   what is held in memory, becomes the batch to commit, and writes go to
//!   the other side file from then on. Quick: it writes nothing. A file
//!   with nothing to commit seals nothing.
//! * **Commit** (`commit_sealed`): the granules the sealed batch held in
//!   memory, its limit, its size and a commit block appended to its side
//!   file, which is synced; then the file, made if it is not there, cut to
//!   the limit, every granule of the batch copied into it, its size set,
//!   the file synced; then the batch forgotten. Writers go on meanwhile,
//!   into the other side file. A crash before the commit block is durable
//!   leaves the file as it was; after it, the copy is made again on
//!   opening.
//! * **Made and removed**: a file made (`OpenMode::CreateNew`) is made at
//!   its first commit, synced in its directory; one removed reads as gone
//!   at once, and is removed at the next commit, its batch a removal
//!   block and a commit block, so a crash before leaves it as the last
//!   commit did. Directories are made and removed at once.
//! * **Opening**: a side file ending in a commit block is copied into the
//!   file, or removes it; the others are thrown away. Only a sealed side
//!   file is ever committed, and a batch is committed before the next is
//!   sealed, so at most one has a commit block. A file is recovered as it
//!   is first opened, or, for every file under a directory, by `recover`.
//!
//! A side file's *limit* is the least size the file had while it was
//! written to: granules at or past it read as zeros unless written since,
//! so a file cut and grown again reads zeros where it was cut. Each file
//! commits on its own: two files are not committed together. A file is
//! kept, with what it holds to commit, from its first opening until a
//! commit leaves it nothing to commit and no handle to it is left, so a
//! handle dropped loses no write, and opening it again gives the same
//! file. A name ending as a side file's is refused.
//!
//! ```text
//! block:  kind u8, then
//!   0 granule:  number u64, content (GRANULE bytes)
//!   1 size:     size u64
//!   2 commit:   (nothing)
//!   3 limit:    size u64
//!   4 remove:   (nothing)
//! ```

mod file;
mod side;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use self::file::{Shadow, ShadowFile};
use self::side::{At, SIDES, base_of, side_path};
use super::io::{DiskFile, FileLock, FileSystem, OpenMode, walk};
use crate::error::{Code, Result, SqlError};
use crate::hash::HashFunction;
use crate::sync::{lock_builtin, write};

/// Bytes of a granule: the unit a write is shadowed in.
pub const GRANULE: usize = 1 << side::GRANULE_BITS;

/// \[DESIGN\] A file system whose files are shadowed (see the module's
/// documentation).
pub struct ShadowFileSystem {
    inner: Arc<dyn FileSystem>,
    hash: Arc<dyn HashFunction>,
    /// The files known, by path: opened, and not yet left with nothing to
    /// commit and no handle.
    files: Mutex<HashMap<PathBuf, Arc<Shadow>>>,
    /// Whether writes are held, files opened meanwhile too.
    holding: AtomicBool,
}

impl ShadowFileSystem {
    /// Shadows the files of `inner`, side files checksummed with `hash`.
    pub fn new(inner: Arc<dyn FileSystem>, hash: Arc<dyn HashFunction>) -> ShadowFileSystem {
        ShadowFileSystem { inner, hash, files: Mutex::new(HashMap::new()), holding: AtomicBool::new(false) }
    }

    /// Where the file at `path` is.
    fn at(&self, path: &Path) -> At {
        At { fs: Arc::clone(&self.inner), path: path.to_path_buf(), hash: Arc::clone(&self.hash) }
    }

    /// The file at `path` opened as `mode` says, known from now on.
    fn known_as(&self, files: &mut HashMap<PathBuf, Arc<Shadow>>, path: &Path, mode: OpenMode) -> Result<Arc<Shadow>> {
        let shadow = Arc::new(Shadow::open(self.at(path), mode, self.holding.load(Ordering::Acquire))?);
        files.insert(path.to_path_buf(), Arc::clone(&shadow));
        Ok(shadow)
    }

    /// The bytes appended to the side files the files known write to, since
    /// they were last sealed; writes held in memory are not counted.
    pub fn pending_bytes(&self) -> u64 {
        self.known().iter().map(|s| s.pending_bytes()).sum()
    }

    /// The files known.
    fn known(&self) -> Vec<Arc<Shadow>> {
        lock_builtin(&self.files).values().cloned().collect()
    }

    /// Recovers every file under directory `dir` from its side files, as
    /// opening each would (see the module's documentation), but those
    /// known: so the directory is as the last commit left it.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if a file or side file cannot be read, written or
    /// removed; -916 if a side file is malformed.
    pub fn recover(&self, dir: &Path) -> Result<()> {
        let files = lock_builtin(&self.files);
        if !self.inner.exists(dir)? {
            return Ok(());
        }
        for path in walk(&*self.inner, dir)? {
            if let Some(base) = base_of(&path).filter(|base| !files.contains_key(base)) {
                side::recover(&self.at(&base))?;
            }
        }
        Ok(())
    }

    /// Holds what every file known writes in memory until it is sealed (see
    /// the module's documentation), and what files opened meanwhile write.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn hold(&self) -> Result<()> {
        self.holding.store(true, Ordering::Release);
        self.known().iter().try_for_each(|s| write(&s.pending).map(|mut p| p.holding = true))
    }

    /// Stops holding writes in memory, for a capture given up: what was held
    /// goes to the side files.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if a side file cannot be written.
    pub fn release(&self) -> Result<()> {
        self.holding.store(false, Ordering::Release);
        self.known().iter().try_for_each(|s| s.release())
    }

    /// Seals what every file known has written, for its commit (see the
    /// module's documentation).
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if a batch sealed before cannot be committed.
    pub fn seal(&self) -> Result<()> {
        self.holding.store(false, Ordering::Release);
        self.known().iter().try_for_each(|s| s.seal())
    }

    /// Commits the batch every file known sealed, each at once; then
    /// forgets the files left with nothing to commit and no handle.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if a side file or a file cannot be written or synced;
    /// a file whose commit block was made durable is committed when opened,
    /// one whose was not by the next commit.
    pub fn commit_sealed(&self) -> Result<()> {
        self.known().iter().try_for_each(|s| s.commit_sealed())?;
        lock_builtin(&self.files).retain(|_, s| Arc::strong_count(s) > 1 || !s.clean());
        Ok(())
    }

    /// Seals and commits what every file known has written.
    ///
    /// # Errors
    ///
    /// As `seal` and `commit_sealed`.
    pub fn commit(&self) -> Result<()> {
        self.seal()?;
        self.commit_sealed()
    }
}

/// \[DESIGN\] Whether the file at `path` is named as a side file of shadow
/// files: its name ending `.shadow0` or `.shadow1`, in either case.
pub fn is_side_file(path: &Path) -> bool {
    base_of(path).is_some()
}

/// Refuses `path` if its name ends as a side file's.
fn refuse_side(path: &Path) -> Result<()> {
    match base_of(path) {
        Some(_) => Err(SqlError::new(
            Code::Api,
            format!("[DESIGN] RULES.md D30: {} is named as a side file of shadow files ({})", path.display(), SIDES.join(", ")),
        )),
        None => Ok(()),
    }
}

impl FileSystem for ShadowFileSystem {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn DiskFile>> {
        refuse_side(path)?;
        let mut files = lock_builtin(&self.files);
        if let Some(shadow) = files.get(path) {
            shadow.reopen(mode)?;
            return Ok(Box::new(ShadowFile(Arc::clone(shadow))));
        }
        Ok(Box::new(ShadowFile(self.known_as(&mut files, path, mode)?)))
    }
    /// Removed at the next commit; at once, with its side files, if no
    /// commit made it and no batch sealed will.
    fn remove(&self, path: &Path) -> Result<()> {
        refuse_side(path)?;
        let mut files = lock_builtin(&self.files);
        let shadow = match files.get(path) {
            Some(shadow) => Arc::clone(shadow),
            None => self.known_as(&mut files, path, OpenMode::ReadWrite)?,
        };
        if !shadow.remove()? {
            files.remove(path);
            for which in 0..SIDES.len() {
                let side = side_path(path, which);
                if self.inner.exists(&side)? {
                    self.inner.discard(&side);
                }
            }
        }
        Ok(())
    }
    fn exists(&self, path: &Path) -> Result<bool> {
        match lock_builtin(&self.files).get(path) {
            Some(shadow) => Ok(!shadow.removed()),
            None => self.inner.exists(path),
        }
    }
    fn lock(&self, path: &Path) -> Result<FileLock> {
        self.inner.lock(path)
    }
    fn create_dir_all(&self, path: &Path) -> Result<()> {
        self.inner.create_dir_all(path)
    }
    /// The files there as written: those made and not yet committed with
    /// them, those removed and not yet committed without, side files not.
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        let files = lock_builtin(&self.files);
        let there = self.inner.read_dir(path)?.into_iter().filter(|p| base_of(p).is_none());
        let mut found: Vec<PathBuf> = there.filter(|p| files.get(p).is_none_or(|s| !s.removed())).collect();
        found.extend(files.iter().filter(|(p, s)| p.parent() == Some(path) && !s.removed()).map(|(p, _)| p.clone()));
        found.sort();
        found.dedup();
        Ok(found)
    }
    fn sync_dir(&self, path: &Path) -> Result<()> {
        self.inner.sync_dir(path)
    }
}

// Unit tests: reads see writes, a commit makes them the file's, a crash
// before a commit loses them and after one keeps them, files made and
// removed at commits (RULES.md R-TEST-1, R-TEST-2, R-TEST-7).
#[cfg(test)]
mod tests;

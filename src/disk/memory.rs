//! \[DESIGN\] A file system in memory (RULES.md D30): files and directories
//! kept for as long as the process runs, behind the same `FileSystem`
//! contract as the operating system's, so every component on files runs on
//! it unchanged. It is what a database with no directory keeps its tables
//! in (`Database::new`), as SQLite's `:memory:` does: nothing survives the
//! process, and syncing does nothing more.
//!
//! It behaves as `OsFileSystem` does where the contract says: a file is
//! created only in a directory that exists, and only if absent; it is
//! opened only if present; a file opened for reading only refuses writes;
//! removing a file that is not there is an error; directories list their
//! entries sorted. Every failure is SQLCODE -915.
//!
//! Given a memory budget (`within`, RULES.md D56), each file's bytes are
//! reserved of it, and given back as it shrinks or goes. Room is held
//! ahead (`DiskFile::reserve`) or as the size is set, refused past the
//! budget's limit with SQLCODE -918; a write past the room held is never
//! refused, what it holds counted even past the limit, so a page written
//! back is not lost.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use super::io::{DiskFile, Durability, FileLock, FileSystem, OpenMode, locked};
use crate::error::{Code, Result, SqlError};
use crate::memory::{MemoryBudget, Reservation};
use crate::sync::lock_builtin;

/// What a path names.
#[derive(Clone)]
enum Entry {
    Directory,
    File(Arc<RwLock<Content>>),
}

/// A file's bytes, and what they hold of the budget, if one is given: at
/// least its size, more if room is held ahead.
struct Content {
    bytes: Vec<u8>,
    held: Option<Reservation>,
}

impl Content {
    /// Makes it of `size` bytes, as `set_size` does: its reservation first,
    /// refused past the limit, and no more than it now needs.
    fn set_size(&mut self, size: usize) -> Result<()> {
        if let Some(held) = &mut self.held {
            held.resize(size)?;
        }
        self.bytes.resize(size, 0);
        Ok(())
    }

    /// Makes it at least `size` bytes, for a write: past the room held,
    /// reserved even past the limit.
    fn grow(&mut self, size: usize) {
        if let Some(held) = self.held.as_mut().filter(|held| held.bytes() < size) {
            held.resize_anyway(size);
        }
        self.bytes.resize(size, 0);
    }

    /// Holds room for `size` bytes.
    fn reserve(&mut self, size: usize) -> Result<()> {
        match self.held.as_mut().filter(|held| held.bytes() < size) {
            Some(held) => held.resize(size),
            None => Ok(()),
        }
    }
}

/// \[DESIGN\] A file system in memory. It may be shared between threads.
#[derive(Default)]
pub struct MemoryFileSystem {
    entries: Mutex<BTreeMap<PathBuf, Entry>>,
    /// The files locked (`FileSystem::lock`).
    locked: Arc<Mutex<BTreeSet<PathBuf>>>,
    /// The budget its files' bytes are reserved of, and what they are, as
    /// its reservations say.
    budget: Option<(Arc<MemoryBudget>, String)>,
}

/// A lock of a `MemoryFileSystem`'s, given up when dropped.
struct MemoryLock {
    locked: Arc<Mutex<BTreeSet<PathBuf>>>,
    path: PathBuf,
}

impl Drop for MemoryLock {
    fn drop(&mut self) {
        lock_builtin(&self.locked).remove(&self.path);
    }
}

fn failed(what: &str, path: &Path, why: &str) -> SqlError {
    SqlError::new(Code::Io, format!("[DESIGN] could not {what} {} in memory: {why}", path.display()))
}

impl MemoryFileSystem {
    /// An empty file system: only its root directory exists.
    pub fn new() -> MemoryFileSystem {
        MemoryFileSystem::default()
    }

    /// An empty file system whose files' bytes are reserved of `budget`,
    /// each file's reservation held by `what` and its path.
    pub fn within(budget: Arc<MemoryBudget>, what: &str) -> MemoryFileSystem {
        MemoryFileSystem { budget: Some((budget, what.to_owned())), ..MemoryFileSystem::default() }
    }

    /// The content of a new file at `path`: no bytes, and none reserved.
    fn content(&self, path: &Path) -> Result<Arc<RwLock<Content>>> {
        let held = match &self.budget {
            Some((budget, what)) => Some(budget.reserve(0, &format!("{what} {}", path.display()))?),
            None => None,
        };
        Ok(Arc::new(RwLock::new(Content { bytes: Vec::new(), held })))
    }
}

/// Whether `path` is the root, which always exists.
fn is_root(path: &Path) -> bool {
    path.parent().is_none() || path.as_os_str().is_empty()
}

/// Whether `entries` hold directory `path`.
fn is_directory(entries: &BTreeMap<PathBuf, Entry>, path: &Path) -> bool {
    is_root(path) || matches!(entries.get(path), Some(Entry::Directory))
}

impl FileSystem for MemoryFileSystem {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn DiskFile>> {
        let mut entries = lock_builtin(&self.entries);
        let content = match (entries.get(path).cloned(), mode) {
            (None, OpenMode::CreateNew) => {
                if !path.parent().is_some_and(|dir| is_directory(&entries, dir)) {
                    return Err(failed("create", path, "its directory does not exist"));
                }
                let content = self.content(path)?;
                entries.insert(path.to_path_buf(), Entry::File(Arc::clone(&content)));
                content
            }
            (Some(Entry::File(content)), OpenMode::ReadWrite | OpenMode::ReadOnly) => content,
            (Some(_), OpenMode::CreateNew) => return Err(failed("create", path, "it exists")),
            (Some(Entry::Directory), OpenMode::ReadWrite | OpenMode::ReadOnly) => return Err(failed("open", path, "it is a directory")),
            (None, OpenMode::ReadWrite | OpenMode::ReadOnly) => return Err(failed("open", path, "it does not exist")),
        };
        Ok(Box::new(MemoryFile { content, path: path.to_path_buf(), writable: mode != OpenMode::ReadOnly }))
    }

    fn remove(&self, path: &Path) -> Result<()> {
        let mut entries = lock_builtin(&self.entries);
        match entries.get(path) {
            Some(Entry::File(_)) => {
                entries.remove(path);
                Ok(())
            }
            Some(Entry::Directory) => Err(failed("remove", path, "it is a directory")),
            None => Err(failed("remove", path, "it does not exist")),
        }
    }

    fn exists(&self, path: &Path) -> Result<bool> {
        let entries = lock_builtin(&self.entries);
        Ok(is_root(path) || entries.contains_key(path))
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        let mut entries = lock_builtin(&self.entries);
        for dir in path.ancestors().filter(|d| !is_root(d)) {
            match entries.get(dir) {
                Some(Entry::File(_)) => return Err(failed("create the directory", path, "a file is in the way")),
                Some(Entry::Directory) => {}
                None => {
                    entries.insert(dir.to_path_buf(), Entry::Directory);
                }
            }
        }
        Ok(())
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        let entries = lock_builtin(&self.entries);
        if !is_directory(&entries, path) {
            return Err(failed("read the directory", path, "it is not a directory"));
        }
        // A BTreeMap keeps its keys sorted.
        Ok(entries.keys().filter(|p| p.parent() == Some(path)).cloned().collect())
    }

    fn sync_dir(&self, path: &Path) -> Result<()> {
        let entries = lock_builtin(&self.entries);
        if is_directory(&entries, path) { Ok(()) } else { Err(failed("sync the directory", path, "it is not a directory")) }
    }
    fn lock(&self, path: &Path) -> Result<FileLock> {
        if !self.exists(path)? {
            drop(self.open(path, OpenMode::CreateNew)?);
        }
        if !lock_builtin(&self.locked).insert(path.to_path_buf()) {
            return Err(locked(path));
        }
        Ok(FileLock::new(MemoryLock { locked: Arc::clone(&self.locked), path: path.to_path_buf() }))
    }
}

/// A file of `MemoryFileSystem`.
struct MemoryFile {
    content: Arc<RwLock<Content>>,
    path: PathBuf,
    writable: bool,
}

/// The index of `offset` in memory, if it has one.
fn index(offset: u64, path: &Path) -> Result<usize> {
    usize::try_from(offset).map_err(|_| failed("reach", path, "the offset is beyond memory"))
}

impl DiskFile for MemoryFile {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize> {
        // As for a file on disk, a range past 2^64 bytes is refused first.
        u64::try_from(buf.len())
            .ok()
            .and_then(|n| offset.checked_add(n))
            .ok_or_else(|| failed("read", &self.path, "the range is beyond 2^64 bytes"))?;
        let content = self.content.read().unwrap_or_else(PoisonError::into_inner);
        let from = index(offset, &self.path)?;
        let available = content.bytes.get(from..).unwrap_or_default();
        let read = available.len().min(buf.len());
        if let (Some(into), Some(bytes)) = (buf.get_mut(..read), available.get(..read)) {
            into.copy_from_slice(bytes);
        }
        Ok(read)
    }

    fn write_all_at(&self, data: &[u8], offset: u64) -> Result<()> {
        if !self.writable {
            return Err(failed("write", &self.path, "it is open for reading only"));
        }
        let from = index(offset, &self.path)?;
        let end = from.checked_add(data.len()).ok_or_else(|| failed("write", &self.path, "the end is beyond memory"))?;
        let mut content = self.content.write().unwrap_or_else(PoisonError::into_inner);
        if content.bytes.len() < end {
            content.grow(end);
        }
        if let Some(into) = content.bytes.get_mut(from..end) {
            into.copy_from_slice(data);
        }
        Ok(())
    }

    fn size(&self) -> Result<u64> {
        let content = self.content.read().unwrap_or_else(PoisonError::into_inner);
        u64::try_from(content.bytes.len()).map_err(|_| failed("find the size of", &self.path, "it is beyond 2^64 bytes"))
    }

    fn set_size(&self, size: u64) -> Result<()> {
        if !self.writable {
            return Err(failed("set the size of", &self.path, "it is open for reading only"));
        }
        let size = index(size, &self.path)?;
        self.content.write().unwrap_or_else(PoisonError::into_inner).set_size(size)
    }

    /// Memory is as durable as the process, whatever is asked.
    fn sync(&self, _durability: Durability) -> Result<()> {
        Ok(())
    }

    fn reserve(&self, size: u64) -> Result<()> {
        let size = index(size, &self.path)?;
        self.content.write().unwrap_or_else(PoisonError::into_inner).reserve(size)
    }
}

// Unit tests: what the memory file system refuses beyond the contract tests
// it shares with the operating system's (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D30, `MemoryFileSystem`: a file is not
    /// created in a directory that does not exist, a directory is neither
    /// opened as a file nor removed as one, each with SQLCODE -915; the
    /// root always exists; a file read-only refuses a new size.
    #[test]
    fn directories_are_not_files() {
        let fs = MemoryFileSystem::new();
        let code = |r: Result<()>| r.err().map(|e| e.code);
        assert_eq!(code(fs.open(Path::new("/missing/f"), OpenMode::CreateNew).map(drop)), Some(Code::Io));
        fs.create_dir_all(Path::new("/d")).unwrap();
        assert_eq!(code(fs.open(Path::new("/d"), OpenMode::ReadOnly).map(drop)), Some(Code::Io));
        assert_eq!(code(fs.remove(Path::new("/d"))), Some(Code::Io));
        assert!(fs.exists(Path::new("/")).unwrap());
        fs.sync_dir(Path::new("/")).unwrap();
        drop(fs.open(Path::new("/d/f"), OpenMode::CreateNew).unwrap());
        assert_eq!(code(fs.open(Path::new("/d/f"), OpenMode::ReadOnly).unwrap().set_size(4)), Some(Code::Io));
    }

    /// Verifies: [DESIGN] RULES.md D56: within a budget, a file's bytes are
    /// reserved, held by what and its path: room held ahead, or a size set,
    /// is refused past the limit with SQLCODE -918, the file left as it
    /// was; a write within the room takes no more, one past it is counted
    /// even past the limit; a smaller size, and the file removed and
    /// closed, give them back.
    #[test]
    fn files_are_reserved_of_a_budget() {
        let budget = MemoryBudget::new(100);
        let fs = MemoryFileSystem::within(Arc::clone(&budget), "a spill file");
        let path = Path::new("/f");
        let file = fs.open(path, OpenMode::CreateNew).unwrap();
        file.reserve(60).unwrap();
        file.write_all_at(&[1_u8; 40], 0).unwrap();
        assert_eq!(budget.reservations(), [("a spill file /f".to_owned(), 60)], "within the room");
        let refused = |r: Result<()>| r.err().map(|e| e.code);
        assert_eq!(refused(file.reserve(101)), Some(Code::Resources));
        assert_eq!(refused(file.set_size(101)), Some(Code::Resources));
        assert_eq!((file.size().unwrap(), budget.reserved()), (40_u64, 60));
        file.write_all_at(&[2_u8; 70], 40).unwrap();
        assert_eq!(budget.reserved(), 110, "written past the limit");
        file.set_size(10).unwrap();
        assert_eq!(budget.reserved(), 10);
        fs.remove(path).unwrap();
        assert_eq!(budget.reserved(), 10, "held while open");
        drop(file);
        assert_eq!(budget.reserved(), 0);
    }

    /// A file system in memory noting the directories it syncs.
    #[derive(Default)]
    struct Noting {
        inner: MemoryFileSystem,
        synced: Mutex<Vec<PathBuf>>,
    }

    impl FileSystem for Noting {
        fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn DiskFile>> {
            self.inner.open(path, mode)
        }
        fn remove(&self, path: &Path) -> Result<()> {
            self.inner.remove(path)
        }
        fn exists(&self, path: &Path) -> Result<bool> {
            self.inner.exists(path)
        }
        fn create_dir_all(&self, path: &Path) -> Result<()> {
            self.inner.create_dir_all(path)
        }
        fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
            self.inner.read_dir(path)
        }
        fn sync_dir(&self, path: &Path) -> Result<()> {
            lock_builtin(&self.synced).push(path.to_path_buf());
            self.inner.sync_dir(path)
        }
        fn lock(&self, path: &Path) -> Result<FileLock> {
            self.inner.lock(path)
        }
    }

    /// Verifies: [DESIGN] RULES.md D56, `FileSystem::create_dir_durably`:
    /// each directory made is synced in its parent, and none that was there
    /// before; made again, nothing is synced.
    #[test]
    fn directories_made_are_synced_in_their_parents() {
        let fs = Noting::default();
        fs.create_dir_all(Path::new("/a")).unwrap();
        fs.create_dir_durably(Path::new("/a/b/c")).unwrap();
        let synced = std::mem::take(&mut *lock_builtin(&fs.synced));
        assert_eq!(synced, [PathBuf::from("/a/b"), PathBuf::from("/a")]);
        fs.create_dir_durably(Path::new("/a/b/c")).unwrap();
        assert_eq!(*lock_builtin(&fs.synced), Vec::<PathBuf>::new(), "made before");
    }
}

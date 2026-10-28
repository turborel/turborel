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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use super::io::{DiskFile, Durability, FileLock, FileSystem, OpenMode, locked};
use crate::error::{Code, Result, SqlError};
use crate::sync::lock_builtin;

/// What a path names.
#[derive(Clone)]
enum Entry {
    Directory,
    File(Arc<RwLock<Vec<u8>>>),
}

/// \[DESIGN\] A file system in memory. It may be shared between threads.
#[derive(Default)]
pub struct MemoryFileSystem {
    entries: Mutex<BTreeMap<PathBuf, Entry>>,
    /// The files locked (`FileSystem::lock`).
    locked: Arc<Mutex<BTreeSet<PathBuf>>>,
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
                let content = Arc::new(RwLock::new(Vec::new()));
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
    content: Arc<RwLock<Vec<u8>>>,
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
        let available = content.get(from..).unwrap_or_default();
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
        if content.len() < end {
            content.resize(end, 0);
        }
        if let Some(into) = content.get_mut(from..end) {
            into.copy_from_slice(data);
        }
        Ok(())
    }

    fn size(&self) -> Result<u64> {
        let content = self.content.read().unwrap_or_else(PoisonError::into_inner);
        u64::try_from(content.len()).map_err(|_| failed("find the size of", &self.path, "it is beyond 2^64 bytes"))
    }

    fn set_size(&self, size: u64) -> Result<()> {
        if !self.writable {
            return Err(failed("set the size of", &self.path, "it is open for reading only"));
        }
        let size = index(size, &self.path)?;
        self.content.write().unwrap_or_else(PoisonError::into_inner).resize(size, 0);
        Ok(())
    }

    /// Memory is as durable as the process, whatever is asked.
    fn sync(&self, _durability: Durability) -> Result<()> {
        Ok(())
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
}

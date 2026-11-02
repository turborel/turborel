//! \[TOOL\] A file system in memory that loses what was not synced when
//! it crashes (RULES.md D57, R-TEST-5): what the simulator's replicas
//! keep their databases in. Live, it is a `MemoryFileSystem` the process
//! sees whole; durable is what the syncs captured: a file's bytes as of
//! its last `sync`, its existence from its own sync or the sync of its
//! directory (empty, if its bytes were never synced), its removal from
//! the sync of its directory, and a directory from its creation. A crash
//! makes the live system the durable one again.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use turborel::disk::io::{DiskFile, Durability, FileLock, FileSystem, OpenMode};
use turborel::disk::memory::MemoryFileSystem;
use turborel::error::Result;

/// What the syncs captured.
#[derive(Default)]
struct Durable {
    /// Each file, by path, as of its last sync.
    files: BTreeMap<PathBuf, Vec<u8>>,
    /// The directories made.
    dirs: BTreeSet<PathBuf>,
}

/// \[TOOL\] A file system in memory that loses unsynced writes at a
/// crash: see the module's documentation.
#[derive(Default)]
pub struct VolatileFileSystem {
    live: Mutex<Arc<MemoryFileSystem>>,
    durable: Arc<Mutex<Durable>>,
}

/// A file of a `VolatileFileSystem`: its syncs capture its bytes.
struct VolatileFile {
    inner: Box<dyn DiskFile>,
    path: PathBuf,
    durable: Arc<Mutex<Durable>>,
}

impl VolatileFileSystem {
    /// An empty file system.
    pub fn new() -> VolatileFileSystem {
        VolatileFileSystem::default()
    }

    /// The live system.
    fn live(&self) -> Arc<MemoryFileSystem> {
        Arc::clone(&self.live.lock().unwrap())
    }

    /// What the syncs captured, locked.
    fn durable(&self) -> MutexGuard<'_, Durable> {
        self.durable.lock().unwrap()
    }

    /// A crash: what was not synced is lost, the live system made again
    /// from what the syncs captured.
    ///
    /// # Errors
    ///
    /// A file system error making it again.
    pub fn crash(&self) -> Result<()> {
        let made = MemoryFileSystem::new();
        let durable = self.durable();
        for dir in &durable.dirs {
            made.create_dir_all(dir)?;
        }
        for (path, bytes) in &durable.files {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                made.create_dir_all(parent)?;
            }
            let file = made.open(path, OpenMode::CreateNew)?;
            file.write_all_at(bytes, 0)?;
        }
        drop(durable);
        *self.live.lock().unwrap_or_else(PoisonError::into_inner) = Arc::new(made);
        Ok(())
    }

    /// How many files the syncs captured.
    pub fn durable_files(&self) -> usize {
        self.durable().files.len()
    }
}

impl FileSystem for VolatileFileSystem {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn DiskFile>> {
        let inner = self.live().open(path, mode)?;
        Ok(Box::new(VolatileFile { inner, path: path.to_path_buf(), durable: Arc::clone(&self.durable) }))
    }

    fn remove(&self, path: &Path) -> Result<()> {
        self.live().remove(path)
    }

    fn exists(&self, path: &Path) -> Result<bool> {
        self.live().exists(path)
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        self.live().create_dir_all(path)?;
        self.durable().dirs.insert(path.to_path_buf());
        Ok(())
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        self.live().read_dir(path)
    }

    /// The directory's entries captured: files removed are gone, files
    /// made exist, empty unless their bytes were synced.
    fn sync_dir(&self, path: &Path) -> Result<()> {
        let live = self.live();
        live.sync_dir(path)?;
        let children: BTreeSet<PathBuf> = live.read_dir(path)?.into_iter().collect();
        let mut durable = self.durable();
        durable.files.retain(|kept, _| kept.parent() != Some(path) || children.contains(kept));
        for child in children {
            let is_file = live.open(&child, OpenMode::ReadOnly).is_ok();
            if is_file && !durable.files.contains_key(&child) {
                durable.files.insert(child, Vec::new());
            }
        }
        Ok(())
    }

    fn lock(&self, path: &Path) -> Result<FileLock> {
        self.live().lock(path)
    }
}

impl DiskFile for VolatileFile {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize> {
        self.inner.read_at(buf, offset)
    }

    fn write_all_at(&self, data: &[u8], offset: u64) -> Result<()> {
        self.inner.write_all_at(data, offset)
    }

    fn size(&self) -> Result<u64> {
        self.inner.size()
    }

    fn set_size(&self, size: u64) -> Result<()> {
        self.inner.set_size(size)
    }

    /// The file's bytes captured, whatever `durability` asks: a sync of
    /// `None` is no sync.
    fn sync(&self, durability: Durability) -> Result<()> {
        self.inner.sync(durability)?;
        if durability == Durability::None {
            return Ok(());
        }
        let size = usize::try_from(self.inner.size()?).unwrap_or(usize::MAX);
        let mut bytes = vec![0; size];
        self.inner.read_exact_at(&mut bytes, 0)?;
        self.durable.lock().unwrap_or_else(PoisonError::into_inner).files.insert(self.path.clone(), bytes);
        Ok(())
    }

    fn reserve(&self, size: u64) -> Result<()> {
        self.inner.reserve(size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [TOOL] R-TEST-5, RULES.md D57: a crash keeps a file's
    /// bytes as of its last sync and loses those written after; a file
    /// made and never synced is gone unless its directory was synced, when
    /// it is empty; a file removed is back unless its directory was synced;
    /// a directory made stays; a sync of durability `None` captures nothing.
    #[test]
    fn a_crash_keeps_what_was_synced() {
        let fs = VolatileFileSystem::new();
        fs.create_dir_all(Path::new("/d")).unwrap();
        let kept = fs.open(Path::new("/d/kept"), OpenMode::CreateNew).unwrap();
        kept.write_all_at(b"one", 0).unwrap();
        kept.sync(Durability::Full).unwrap();
        kept.write_all_at(b"two", 3).unwrap();
        let unsynced = fs.open(Path::new("/d/unsynced"), OpenMode::CreateNew).unwrap();
        unsynced.write_all_at(b"gone", 0).unwrap();
        unsynced.sync(Durability::None).unwrap();
        let removed = fs.open(Path::new("/d/removed"), OpenMode::CreateNew).unwrap();
        removed.sync(Durability::Data).unwrap();
        drop(removed);
        fs.remove(Path::new("/d/removed")).unwrap();
        fs.create_dir_all(Path::new("/d/e")).unwrap();
        let entry_only = fs.open(Path::new("/d/e/entry"), OpenMode::CreateNew).unwrap();
        entry_only.write_all_at(b"lost bytes", 0).unwrap();
        fs.sync_dir(Path::new("/d/e")).unwrap();
        assert_eq!(fs.durable_files(), 3_usize);
        fs.crash().unwrap();
        let mut buf = vec![0; 6];
        let read = fs.open(Path::new("/d/kept"), OpenMode::ReadOnly).unwrap().read_at(&mut buf, 0).unwrap();
        assert_eq!(&buf[..read], b"one");
        assert!(!fs.exists(Path::new("/d/unsynced")).unwrap(), "never synced, in an unsynced directory");
        assert!(fs.exists(Path::new("/d/removed")).unwrap(), "removed in an unsynced directory");
        assert!(fs.exists(Path::new("/d/e")).unwrap());
        assert_eq!(fs.open(Path::new("/d/e/entry"), OpenMode::ReadOnly).unwrap().size().unwrap(), 0_u64, "its entry synced, its bytes not");
        fs.sync_dir(Path::new("/d")).unwrap();
        fs.remove(Path::new("/d/removed")).unwrap();
        fs.sync_dir(Path::new("/d")).unwrap();
        fs.crash().unwrap();
        assert!(!fs.exists(Path::new("/d/removed")).unwrap(), "removed, the directory synced");
    }
}

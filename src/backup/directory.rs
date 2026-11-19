//! \[EXTENSION\] The built-in backup target (RULES.md D166): objects as
//! files under a directory, an object's `/`-separated parts its
//! directories and file. An upload is written to a file of its own beside
//! where the object goes, named with `UPLOAD_MARK`, synced, and renamed into
//! place as it finishes, its directory then synced, so an object is there
//! whole or not at all, whatever crash comes. Uploads a crash left behind
//! are never listed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::after_scheme;
use super::target::{BackupTarget, DIR_SCHEME, SEPARATOR, TargetProvider, check_name, exists, no_object, no_upload};
use crate::disk::io::{DiskFile, Durability, FileSystem, OpenMode, OsFileSystem};
use crate::error::{Code, Result, SqlError};
use crate::sync::lock;

/// \[EXTENSION\] What an upload's file is named with, after its object's
/// name: never in an object's name of a backup's or the archive's.
const UPLOAD_MARK: &str = ".upload-";

/// An upload under way: its object's name, its file and where it is, and
/// how much was written.
struct Upload {
    name: String,
    path: PathBuf,
    file: Box<dyn DiskFile>,
    written: u64,
}

/// \[EXTENSION\] A target of files under a directory: see the module's
/// documentation.
pub struct DirectoryTarget {
    location: String,
    root: PathBuf,
    uploads: Mutex<HashMap<u64, Upload>>,
    next: AtomicU64,
}

impl DirectoryTarget {
    /// The target under `root`, made if missing, named `location`.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the directory cannot be made.
    pub fn new(location: &str, root: &Path) -> Result<DirectoryTarget> {
        OsFileSystem.create_dir_all(root)?;
        Ok(DirectoryTarget {
            location: location.to_owned(),
            root: root.to_path_buf(),
            uploads: Mutex::new(HashMap::new()),
            next: AtomicU64::new(0),
        })
    }

    /// Where object `name` is.
    fn path(&self, name: &str) -> PathBuf {
        name.split(SEPARATOR).fold(self.root.clone(), |path, part| path.join(part))
    }

    /// The names of the files under `dir`, as objects, `within` before
    /// them, into `into`.
    fn walk(dir: &Path, within: &str, into: &mut Vec<String>) -> Result<()> {
        for path in OsFileSystem.read_dir(dir)? {
            let Some(part) = path.file_name().and_then(|n| n.to_str()) else { continue };
            let name = if within.is_empty() { part.to_owned() } else { format!("{within}{SEPARATOR}{part}") };
            if path.is_dir() {
                Self::walk(&path, &name, into)?;
            } else if !part.contains(UPLOAD_MARK) {
                into.push(name);
            }
        }
        Ok(())
    }
}

/// The failure to rename `from` to `to`.
fn renaming(from: &Path, to: &Path, e: &std::io::Error) -> SqlError {
    SqlError::new(Code::Io, format!("[EXTENSION] RULES.md D166: {} could not be renamed to {}: {e}", from.display(), to.display()))
}

impl BackupTarget for DirectoryTarget {
    fn location(&self) -> String {
        self.location.clone()
    }

    fn begin(&self, name: &str) -> Result<u64> {
        check_name(name)?;
        let upload = self.next.fetch_add(1, Ordering::Relaxed);
        let target = self.path(name);
        if let Some(parent) = target.parent() {
            OsFileSystem.create_dir_all(parent)?;
        }
        let path = PathBuf::from(format!("{}{UPLOAD_MARK}{}-{upload}", target.display(), std::process::id()));
        let file = OsFileSystem.open(&path, OpenMode::CreateNew)?;
        lock(&self.uploads)?.insert(upload, Upload { name: name.to_owned(), path, file, written: 0 });
        Ok(upload)
    }

    fn write(&self, upload: u64, bytes: &[u8]) -> Result<()> {
        let mut uploads = lock(&self.uploads)?;
        let held = uploads.get_mut(&upload).ok_or_else(|| no_upload(upload))?;
        held.file.write_all_at(bytes, held.written)?;
        held.written = held.written.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        Ok(())
    }

    fn finish(&self, upload: u64) -> Result<()> {
        let held = lock(&self.uploads)?.remove(&upload).ok_or_else(|| no_upload(upload))?;
        let target = self.path(&held.name);
        let refused = |e: SqlError| {
            OsFileSystem.discard(&held.path);
            Err(e)
        };
        if let Err(e) = held.file.sync(Durability::Full) {
            return refused(e);
        }
        if OsFileSystem.exists(&target)? {
            return refused(exists(&held.name));
        }
        if let Err(e) = std::fs::rename(&held.path, &target) {
            return refused(renaming(&held.path, &target, &e));
        }
        target.parent().map_or(Ok(()), |dir| OsFileSystem.sync_dir(dir))
    }

    fn abandon(&self, upload: u64) -> Result<()> {
        let held = lock(&self.uploads)?.remove(&upload);
        held.map_or(Ok(()), |h| OsFileSystem.remove(&h.path))
    }

    fn read(&self, name: &str, offset: u64, max: u32) -> Result<Vec<u8>> {
        check_name(name)?;
        let path = self.path(name);
        if !OsFileSystem.exists(&path)? {
            return Err(no_object(name));
        }
        let file = OsFileSystem.open(&path, OpenMode::ReadOnly)?;
        let size = file.size()?;
        let left = size.saturating_sub(offset).min(u64::from(max));
        let mut out = vec![0; usize::try_from(left).unwrap_or(0)];
        let mut filled = 0;
        while filled < out.len() {
            let at = offset.saturating_add(u64::try_from(filled).unwrap_or(u64::MAX));
            let read = file.read_at(out.get_mut(filled..).unwrap_or_default(), at)?;
            if read == 0 {
                break;
            }
            filled = filled.saturating_add(read);
        }
        out.truncate(filled);
        Ok(out)
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let mut names = Vec::new();
        Self::walk(&self.root, "", &mut names)?;
        names.retain(|n| n.starts_with(prefix));
        names.sort();
        Ok(names)
    }

    fn remove(&self, name: &str) -> Result<()> {
        check_name(name)?;
        let path = self.path(name);
        if OsFileSystem.exists(&path)? { OsFileSystem.remove(&path) } else { Ok(()) }
    }
}

/// \[EXTENSION\] The provider of directory targets, of the scheme `dir`
/// and of locations with none: the location's path, after its scheme.
pub struct DirectoryProvider;

impl TargetProvider for DirectoryProvider {
    fn scheme(&self) -> &str {
        DIR_SCHEME
    }

    fn open(&self, location: &str) -> Result<Arc<dyn BackupTarget>> {
        Ok(Arc::new(DirectoryTarget::new(location, Path::new(after_scheme(location)))?))
    }
}

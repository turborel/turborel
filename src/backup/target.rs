//! \[EXTENSION\] Where backups and the archived log go (RULES.md D166): a
//! target, a flat store of named objects, and the providers that open a
//! target from a location. The trait is one request and one reply a
//! method, of strings, integers and chunks of bytes, so a plug-in in
//! another process can serve it as well as one in this one. Writes are an
//! upload begun, its chunks sent, and the upload finished or abandoned,
//! and reads are ranges.
//!
//! Built in: `DirectoryTarget`, objects as files under a directory, and
//! `MemoryTarget`, objects in memory, for embedding programs and tests.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{Code, Result, SqlError};
use crate::sync::lock;

/// \[EXTENSION\] A store of named objects that backups and the archive are
/// written to (RULES.md D166): an extension point (R-SAFE-3).
///
/// Contract (checked for every built-in target by
/// `test/integration/backup_targets.rs`):
/// * An object exists once `finish` of its upload returns, durably, and
///   never before: `read` and `list` see none of an upload not finished.
/// * An object is never replaced: `finish` of an upload whose name exists
///   fails (SQLCODE -910), the object left as it was.
/// * `read(name, offset, max)` returns up to `max` bytes from `offset`,
///   fewer only at the object's end, none at or past it; an object not
///   there is refused (SQLCODE -910).
/// * `list(prefix)` names every object whose name begins with `prefix`,
///   sorted, and no upload.
/// * `remove` of an object not there succeeds.
/// * Names are of `/`-separated parts, each of letters, digits, `.`, `_`
///   and `-`, never `.` or `..` alone (`check_name`).
/// * Methods may be called from several threads at once, and never call
///   back into the database (RULES.md R-ERR-4).
pub trait BackupTarget: Send + Sync {
    /// Its location, as SQL and settings name it.
    fn location(&self) -> String;
    /// Begins an upload of object `name`: its handle.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the name is malformed; -915 if it cannot begin.
    fn begin(&self, name: &str) -> Result<u64>;
    /// Appends `bytes` to upload `upload`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such upload; -915 if they cannot be
    /// written.
    fn write(&self, upload: u64, bytes: &[u8]) -> Result<()>;
    /// Makes upload `upload` its object, durably.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such upload, or its object exists; -915
    /// if it cannot be made durable, the object then not there.
    fn finish(&self, upload: u64) -> Result<()>;
    /// Abandons upload `upload`, leaving no object.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if what it wrote cannot be removed.
    fn abandon(&self, upload: u64) -> Result<()>;
    /// Up to `max` bytes of object `name` from `offset`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no such object; -915 if it cannot be read.
    fn read(&self, name: &str, offset: u64, max: u32) -> Result<Vec<u8>>;
    /// The names of the objects beginning with `prefix`, sorted.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if they cannot be listed.
    fn list(&self, prefix: &str) -> Result<Vec<String>>;
    /// Removes object `name`, if it is there.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it cannot be removed.
    fn remove(&self, name: &str) -> Result<()>;
}

pub use BackupTarget as ArchiveTarget;

/// \[EXTENSION\] What opens targets of one scheme from their locations
/// (RULES.md D166), registered with a database
/// (`Database::register_target_provider`).
pub trait TargetProvider: Send + Sync {
    /// The scheme its locations begin with, before a `:`.
    fn scheme(&self) -> &str;
    /// The target at `location`, the whole location string, its scheme
    /// and `:` included.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the location is malformed; -915 if the target
    /// cannot be reached.
    fn open(&self, location: &str) -> Result<std::sync::Arc<dyn BackupTarget>>;
}

/// \[EXTENSION\] The scheme of the built-in directory targets, which a
/// location of no scheme also names.
pub const DIR_SCHEME: &str = "dir";

/// \[EXTENSION\] The scheme of the built-in memory targets: each location
/// a store of its own for the database's life.
pub const MEMORY_SCHEME: &str = "memory";

/// The separator of an object name's parts.
pub const SEPARATOR: char = '/';

/// Refuses `name` unless it is an object name: see `BackupTarget`.
///
/// # Errors
///
/// SQLCODE -910 if it is not.
pub fn check_name(name: &str) -> Result<()> {
    let part_ok = |p: &str| !p.is_empty() && p != "." && p != ".." && p.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if name.split(SEPARATOR).all(part_ok) {
        return Ok(());
    }
    Err(SqlError::new(Code::Api, format!("[EXTENSION] RULES.md D166: {name:?} is not an object name")))
}

/// The refusal of an upload `upload` not begun or already ended.
pub(crate) fn no_upload(upload: u64) -> SqlError {
    SqlError::new(Code::Api, format!("[EXTENSION] RULES.md D166: there is no upload {upload}"))
}

/// The refusal of a missing object `name`.
pub(crate) fn no_object(name: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[EXTENSION] RULES.md D166: the target holds no object {name}"))
}

/// The refusal to replace object `name`.
pub(crate) fn exists(name: &str) -> SqlError {
    SqlError::new(
        Code::Api,
        format!("[EXTENSION] RULES.md D166: the target already holds an object {name}, and objects are never replaced"),
    )
}

/// The bytes of `object` from `offset`, up to `max`.
fn range(object: &[u8], offset: u64, max: u32) -> Vec<u8> {
    let from = usize::try_from(offset).unwrap_or(usize::MAX).min(object.len());
    let count = usize::try_from(max).unwrap_or(usize::MAX).min(object.len().saturating_sub(from));
    object.get(from..from.saturating_add(count)).unwrap_or_default().to_vec()
}

/// \[EXTENSION\] A target in memory (RULES.md D166), whose objects last as
/// long as it does.
#[derive(Debug, Default)]
pub struct MemoryTarget {
    location: String,
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
    uploads: Mutex<HashMap<u64, (String, Vec<u8>)>>,
    next: AtomicU64,
}

impl MemoryTarget {
    /// An empty target named `location`.
    pub fn new(location: &str) -> MemoryTarget {
        MemoryTarget { location: location.to_owned(), ..MemoryTarget::default() }
    }

    /// \[EXTENSION\] Replaces object `name`'s bytes, for a test damaging a
    /// backup, as no backup ever replaces an object.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn put(&self, name: &str, bytes: Vec<u8>) -> Result<()> {
        lock(&self.objects)?.insert(name.to_owned(), bytes);
        Ok(())
    }
}

impl BackupTarget for MemoryTarget {
    fn location(&self) -> String {
        self.location.clone()
    }

    fn begin(&self, name: &str) -> Result<u64> {
        check_name(name)?;
        let upload = self.next.fetch_add(1, Ordering::Relaxed);
        lock(&self.uploads)?.insert(upload, (name.to_owned(), Vec::new()));
        Ok(upload)
    }

    fn write(&self, upload: u64, bytes: &[u8]) -> Result<()> {
        let mut uploads = lock(&self.uploads)?;
        let (_, held) = uploads.get_mut(&upload).ok_or_else(|| no_upload(upload))?;
        held.extend_from_slice(bytes);
        Ok(())
    }

    fn finish(&self, upload: u64) -> Result<()> {
        let (name, bytes) = lock(&self.uploads)?.remove(&upload).ok_or_else(|| no_upload(upload))?;
        let mut objects = lock(&self.objects)?;
        if objects.contains_key(&name) {
            return Err(exists(&name));
        }
        objects.insert(name, bytes);
        Ok(())
    }

    fn abandon(&self, upload: u64) -> Result<()> {
        lock(&self.uploads)?.remove(&upload);
        Ok(())
    }

    fn read(&self, name: &str, offset: u64, max: u32) -> Result<Vec<u8>> {
        let objects = lock(&self.objects)?;
        Ok(range(objects.get(name).ok_or_else(|| no_object(name))?, offset, max))
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(lock(&self.objects)?.keys().filter(|k| k.starts_with(prefix)).cloned().collect())
    }

    fn remove(&self, name: &str) -> Result<()> {
        lock(&self.objects)?.remove(name);
        Ok(())
    }
}

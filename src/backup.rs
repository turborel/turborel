//! \[EXTENSION\] Backups, the archived log and point-in-time recovery
//! (RULES.md D160 to D166): what is written to a target and read back from
//! it, apart from the database that takes or restores them
//! (`db/backup.rs`).
//!
//! A target is a flat store of objects (`target.rs`). A backup is the
//! objects under `backups/<id>/`: its files' chunks, its checkpoint's
//! image, the log's records it is recovered with, and its manifest, written
//! last (`manifest.rs`). The archive is the objects under `archive/`: the
//! log's records, a segment of them an object, by timeline, and each
//! timeline's history. Every object is a block file of format version 1,
//! as a stream (`disk/block/stream.rs`, `object.rs`), so it names its
//! checksum, its key and its encodings, and each block carries its digest.
//!
//! ```text
//! backups/<id>/manifest          the manifest, one block
//! backups/<id>/image             the checkpoint's image, one block
//! backups/<id>/log               the records, a block each
//! backups/<id>/files/<within>    a file's chunks, a block each
//! archive/<timeline>/<first>-<last>   archived records, a block each
//! archive/<timeline>.history     the timeline's ancestry, one block
//! ```

pub mod decide;
mod directory;
pub mod history;
pub mod manifest;
pub(crate) mod object;
pub mod seed;
pub mod seeded;
pub mod target;
pub mod timeline;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

pub use directory::{DirectoryProvider, DirectoryTarget};
pub use target::{ArchiveTarget, BackupTarget, DIR_SCHEME, MEMORY_SCHEME, MemoryTarget, TargetProvider};

use crate::error::{Code, Result, SqlError, SqlState};
use crate::sync::lock;

/// \[EXTENSION\] The prefix of the backups' objects in a target.
pub const BACKUPS: &str = "backups/";
/// \[EXTENSION\] The prefix of the archive's objects in a target.
pub const ARCHIVE: &str = "archive/";
/// \[EXTENSION\] A backup's manifest, within its prefix.
pub const MANIFEST: &str = "manifest";
/// \[EXTENSION\] A backup's checkpoint image, within its prefix.
pub const IMAGE: &str = "image";
/// \[EXTENSION\] A backup's log records, within its prefix.
pub const LOG: &str = "log";
/// \[EXTENSION\] A backup's files, within its prefix.
pub const FILES: &str = "files/";
/// \[EXTENSION\] The suffix of a timeline's history object.
pub const HISTORY_SUFFIX: &str = ".history";

/// \[EXTENSION\] The bytes of a file's chunk (RULES.md D162): the unit an
/// incremental backup copies or leaves to its parent. 64 KiB, a few pages
/// of the data file, so a manifest of a large file stays a small fraction
/// of it (16 bytes of digest a chunk).
pub const CHUNK_BYTES: usize = 64 << 10;

/// \[EXTENSION\] The bytes asked of a target at a time, and gathered before
/// a write to it: 1 MiB, few calls to a target in another process.
pub const PIECE_BYTES: u32 = 1 << 20;

/// \[EXTENSION\] The bytes of the position before a record's bytes, or of
/// the number before a chunk's, in a block of a backup's object: a `u64`.
pub(crate) const POSITION_BYTES: usize = 8;

/// \[EXTENSION\] The code of a full backup in a manifest of format 1.
const FULL_CODE: u8 = 1;
/// \[EXTENSION\] The code of an incremental backup in a manifest of format 1.
const INCREMENTAL_CODE: u8 = 2;

/// \[EXTENSION\] What a backup is (RULES.md D161, D162).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupKind {
    /// Every chunk of every file.
    Full,
    /// The chunks changed since its parent.
    Incremental,
}

impl BackupKind {
    /// Its name, as INFORMATION_SCHEMA.BACKUPS and BACKUP DATABASE show it.
    pub fn name(self) -> &'static str {
        match self {
            BackupKind::Full => "FULL",
            BackupKind::Incremental => "INCREMENTAL",
        }
    }

    /// Its code in a manifest.
    pub(crate) fn code(self) -> u8 {
        match self {
            BackupKind::Full => FULL_CODE,
            BackupKind::Incremental => INCREMENTAL_CODE,
        }
    }

    /// The kind of `code`, if it is one.
    pub(crate) fn of(code: u8) -> Option<BackupKind> {
        [BackupKind::Full, BackupKind::Incremental].into_iter().find(|k| k.code() == code)
    }
}

/// The prefix of backup `id`'s objects.
pub fn backup_prefix(id: &str) -> String {
    format!("{BACKUPS}{id}/")
}

/// \[EXTENSION\] The refusal of a backup or archived object found damaged
/// (RULES.md D161): SQLCODE -916, SQLSTATE TBB01, naming the object.
pub fn damaged(object: &str, why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[EXTENSION] RULES.md D161: {object} is damaged: {why}")).with_state(SqlState::BACKUP_DAMAGED)
}

/// \[EXTENSION\] The providers of targets a database opens locations with
/// (RULES.md D166), the built-in ones first: `dir`, which a location of
/// no scheme names too, and `memory`.
pub struct Targets {
    providers: RwLock<BTreeMap<String, Arc<dyn TargetProvider>>>,
}

impl Default for Targets {
    fn default() -> Self {
        let mut providers: BTreeMap<String, Arc<dyn TargetProvider>> = BTreeMap::new();
        providers.insert(DIR_SCHEME.to_owned(), Arc::new(DirectoryProvider));
        providers.insert(MEMORY_SCHEME.to_owned(), Arc::new(MemoryProvider::default()));
        Targets { providers: RwLock::new(providers) }
    }
}

impl Targets {
    /// Registers `provider` for its scheme, replacing any before.
    pub fn register(&self, provider: Arc<dyn TargetProvider>) {
        let mut providers = self.providers.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        providers.insert(provider.scheme().to_ascii_lowercase(), provider);
    }

    /// Whether a provider is registered for `scheme`.
    pub fn serves(&self, scheme: &str) -> bool {
        self.providers.read().unwrap_or_else(std::sync::PoisonError::into_inner).contains_key(&scheme.to_ascii_lowercase())
    }

    /// The target at `location`: of the provider its scheme names, the
    /// letters before its first `:`, or of `dir` if it has none. A scheme
    /// of one letter is a drive's, as `C:\backups`, so a directory.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the location is empty or its scheme has no
    /// provider; the provider's.
    pub fn open(&self, location: &str) -> Result<Arc<dyn BackupTarget>> {
        if location.trim().is_empty() {
            return Err(SqlError::new(Code::Api, "[EXTENSION] RULES.md D166: a backup target's location is empty"));
        }
        let scheme = scheme_of(location).unwrap_or(DIR_SCHEME).to_ascii_lowercase();
        let provider = self.providers.read().unwrap_or_else(std::sync::PoisonError::into_inner).get(&scheme).cloned();
        let provider = provider.ok_or_else(|| {
            SqlError::new(Code::Api, format!("[EXTENSION] RULES.md D166: no backup target provider is registered for the scheme {scheme}"))
        })?;
        provider.open(location)
    }
}

/// The scheme `location` names, if it names one: letters, digits, `+`,
/// `-` and `.` before a `:`, at least two of them.
pub fn scheme_of(location: &str) -> Option<&str> {
    let (scheme, _) = location.split_once(':')?;
    let named = scheme.len() > 1 && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
    named.then_some(scheme)
}

/// What follows a location's scheme and `:`, or the whole of it if it has
/// none.
pub fn after_scheme(location: &str) -> &str {
    match scheme_of(location) {
        Some(scheme) => location.get(scheme.len().saturating_add(1)..).unwrap_or_default(),
        None => location,
    }
}

/// \[EXTENSION\] The built-in provider of memory targets: one store each
/// location, kept as long as the provider is, so a database opening a
/// location twice finds what it wrote.
#[derive(Default)]
pub struct MemoryProvider {
    stores: Mutex<BTreeMap<String, Arc<MemoryTarget>>>,
}

impl TargetProvider for MemoryProvider {
    fn scheme(&self) -> &str {
        MEMORY_SCHEME
    }

    fn open(&self, location: &str) -> Result<Arc<dyn BackupTarget>> {
        let mut stores = lock(&self.stores)?;
        let store = stores.entry(location.to_owned()).or_insert_with(|| Arc::new(MemoryTarget::new(location)));
        Ok(Arc::<MemoryTarget>::clone(store))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] RULES.md D166, locations: a scheme is two
    /// characters or more before a `:`, so a drive letter is a directory's
    /// path; what follows the scheme is what its provider reads.
    #[test]
    fn locations_name_their_schemes() {
        assert_eq!(scheme_of("memory:a"), Some("memory"));
        assert_eq!(scheme_of("C:\\backups"), None);
        assert_eq!(scheme_of("/var/backups"), None);
        assert_eq!(after_scheme("dir:/x"), "/x");
        assert_eq!(after_scheme("/x"), "/x");
        let targets = Targets::default();
        assert_eq!(targets.open("nothing:x").err().map(|e| e.code), Some(Code::Api));
        assert_eq!(targets.open(" ").err().map(|e| e.code), Some(Code::Api));
        let one = targets.open("memory:one").unwrap();
        let upload = one.begin("a").unwrap();
        one.finish(upload).unwrap();
        assert_eq!(targets.open("memory:one").unwrap().list("").unwrap(), vec!["a".to_owned()]);
    }
}

//! \[EXTENSION\] A database's backup history, kept durably (RULES.md
//! D166): a block file of format version 1, `history` in the plug-in
//! directory `turborel-backup`, kept as written, each entry
//! (`backup/history.rs`) a block appended and synced. A file the file
//! manager knows as durable, it goes with every export and backup. It is
//! read once, as it is first asked for, and again if the database is
//! opened from another directory since.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{Backups, Database, formats_of};
use crate::backup::history::Entry;
use crate::db::State;
use crate::disk::block::{BlockFile, BlockId, BlockOptions, HEADER_LEN};
use crate::disk::io::{Durability, FileSystem};
use crate::error::Result;
use crate::sync::lock;

/// \[EXTENSION\] The plug-in directory the history is kept in.
pub(crate) const HISTORY_DIR: &str = "turborel-backup";
/// \[EXTENSION\] The history's file within it.
const HISTORY_FILE: &str = "history";

/// The history file opened, its entries read, and where it is.
pub(super) struct HistoryFile {
    file: BlockFile,
    entries: Vec<Entry>,
    fs: Arc<dyn FileSystem>,
    path: PathBuf,
}

impl HistoryFile {
    /// Whether it is the file at `path` of `fs`.
    fn is_at(&self, fs: &Arc<dyn FileSystem>, path: &Path) -> bool {
        crate::backup::decide::same_history((Arc::ptr_eq(&self.fs, fs), self.path == path))
    }
}

/// The history file at `path` of `fs`, opened or made, its entries read,
/// in the formats of `st`'s files.
fn open_history(st: &State, fs: &Arc<dyn FileSystem>, path: &Path) -> Result<HistoryFile> {
    let (writing, reading) = formats_of(st)?;
    let options = BlockOptions::default();
    if !fs.exists(path)? {
        let file = BlockFile::create(&**fs, path, writing.hash, writing.pipeline, options)?;
        return Ok(HistoryFile { file, entries: Vec::new(), fs: Arc::clone(fs), path: path.to_path_buf() });
    }
    let (file, _) = BlockFile::open(&**fs, path, &reading.hashes, &reading.encodings, options)?;
    let mut entries = Vec::new();
    let mut cursor = file.cursor(BlockId(HEADER_LEN));
    let mut payload = Vec::new();
    while cursor.next(&mut payload)?.is_some() {
        entries.push(Entry::decode(&payload)?);
    }
    Ok(HistoryFile { file, entries, fs: Arc::clone(fs), path: path.to_path_buf() })
}

/// Runs `work` on the history of `st`, as `kept` holds it, opened first if it is not, or is
/// of another directory; or gives `absent` if it is not and `make` is
/// false and there is no file.
fn with_history<T>(
    (st, kept): (&State, &Backups),
    (make, absent): (bool, T),
    work: impl FnOnce(&mut HistoryFile) -> Result<T>,
) -> Result<T> {
    let (fs, dir) = st.plugins.directory(HISTORY_DIR)?;
    let path = dir.join(HISTORY_FILE);
    let mut held = lock(&kept.history)?;
    if !held.as_ref().is_some_and(|h| h.is_at(&fs, &path)) {
        if crate::backup::decide::history_absent((make, fs.exists(&path)?)) {
            return Ok(absent);
        }
        *held = Some(open_history(st, &fs, &path)?);
    }
    held.as_mut().map_or_else(|| Err(super::super::api("[EXTENSION] the backup history went as it was read")), work)
}

/// \[EXTENSION\] The entries of the history of `st`, as `kept` holds it,
/// oldest first, none if it has none yet: what INFORMATION_SCHEMA shows
/// (RULES.md D166).
///
/// # Errors
///
/// SQLCODE -916 if the file is damaged; -915 if it cannot be read.
pub(crate) fn history_entries(st: &State, kept: &Backups) -> Result<Vec<Entry>> {
    with_history((st, kept), (false, Vec::new()), |h| Ok(h.entries.clone()))
}

impl Database {
    /// \[EXTENSION\] Appends `entry` to the history, durably (RULES.md
    /// D166).
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it cannot be written or synced.
    pub(crate) fn record_history(&self, entry: Entry) -> Result<()> {
        with_history((&*self.snapshot()?, self.backups()), (true, ()), |h| {
            h.file.append(&entry.encode())?;
            h.file.sync(Durability::Full)?;
            h.entries.push(entry);
            Ok(())
        })
    }

    /// \[EXTENSION\] The entries of the history, oldest first (RULES.md
    /// D166).
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the file is damaged; -915 if it cannot be read.
    pub fn backup_history(&self) -> Result<Vec<Entry>> {
        history_entries(&*self.snapshot()?, self.backups())
    }
}

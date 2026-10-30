//! \[DESIGN\] The built-in catalog store (`db/stores.rs`, RULES.md
//! D25): a block file (`disk/block.rs`) of catalog records in the
//! database's directory, each appended and synced; the last whole one says
//! what state the database's files are in. A record cut short by a crash is cut off when
//! the file is opened, so the one before it stands; a record damaged, valid
//! ones after it, refuses the file (SQLCODE -916, `disk/block/tail.rs`).
//!
//! The file is `catalog.0` or `catalog.1`. Once one holds CATALOG.COMPACT_AFTER
//! records, or an append to it failed (a block file then takes no more),
//! the next is written alone to the other, made anew, which is
//! synced with its directory before the first is removed: the catalog
//! never grows past that, and has no rename to rely on. Each record has a
//! sequence number; opening takes the file whose last record's is higher,
//! so a crash during the switch leaves either the old file or the new one
//! standing, and removes the other.
//!
//! ```text
//! record:     sequence u64, then the record given (`db/image.rs`)
//! ```

use std::mem;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::formats::FileFormats;
use super::stores::{CatalogStorage, CatalogStore, StorePlace};
use crate::bytes::{Put, Reader};
use crate::disk::block::{BlockFile, BlockOptions};
use crate::disk::encoding::Pipeline;
use crate::disk::io::{Durability, FileSystem};
use crate::error::{Code, Result, SqlError};
use crate::hash::HashFunction;
use crate::sync::lock_builtin;

/// \[EXTENSION\] The built-in kind of catalog store: two files of the
/// database's directory, by turns.
#[derive(Debug, Clone, Copy, Default)]
pub struct CatalogFiles;

impl CatalogStorage for CatalogFiles {
    fn name(&self) -> &'static str {
        "files"
    }

    fn open(&self, place: &StorePlace<'_>) -> Result<Option<Box<dyn CatalogStore>>> {
        let there = place.fs.exists(&place.dir.join(name(0)))? || place.fs.exists(&place.dir.join(name(1)))?;
        if !there {
            return Ok(None);
        }
        Ok(Some(Box::new(CatalogFile::open(Arc::clone(place.fs), place.dir.to_path_buf(), place.formats)?)))
    }

    fn create(&self, place: &StorePlace<'_>) -> Result<Box<dyn CatalogStore>> {
        let formats = (Arc::clone(&place.formats.hash), place.formats.pipeline.clone());
        Ok(Box::new(CatalogFile::create(Arc::clone(place.fs), place.dir.to_path_buf(), formats)?))
    }
}

/// A record and its sequence number, as the file holds it.
fn framed(sequence: u64, record: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(record.len().saturating_add(8));
    out.put_u64(sequence);
    out.extend_from_slice(record);
    out
}

/// A record with its sequence number.
type Numbered = (u64, Vec<u8>);

/// The sequence number and the record of what `framed` made.
fn unframed(bytes: &[u8]) -> Result<Numbered> {
    let mut r = Reader::new(bytes, "catalog record");
    let sequence = r.u64()?;
    Ok((sequence, r.rest().to_vec()))
}

/// The name of catalog file `which` (0 or 1): a catalog alternates between
/// the two.
fn name(which: usize) -> &'static str {
    if which == 0 { "catalog.0" } else { "catalog.1" }
}
/// \[DESIGN\] The default of CATALOG.COMPACT_AFTER: the records a catalog
/// file holds before the next goes to a new one.
pub const COMPACT_AFTER: usize = 64;

/// The file a catalog writes to now.
struct Active {
    blocks: BlockFile,
    /// Which of the two files it is (`name`).
    which: usize,
    records: usize,
    /// Set once an append failed: a block file then takes no more, so the
    /// next record goes to the other file.
    broken: bool,
}

/// \[DESIGN\] A database's catalog file.
struct CatalogFile {
    fs: Arc<dyn FileSystem>,
    dir: PathBuf,
    /// What its next file is checksummed with, and encoded through: as the
    /// one before it was.
    hash: Arc<dyn HashFunction>,
    pipeline: Pipeline,
    /// The file it writes to; taken out while a record is written, so no
    /// lock is held over the file system's calls (RULES.md R-ERR-4).
    active: Mutex<Option<Active>>,
    /// Its last record, with its sequence number, kept apart from the file
    /// so it is read while one is written.
    last: Mutex<Option<Numbered>>,
}

/// The last record of `blocks`, with its sequence number, and how many
/// records it holds.
fn last_record(blocks: &BlockFile) -> Result<(Option<Numbered>, usize)> {
    let mut cursor = blocks.cursor(blocks.first());
    let (mut payload, mut last, mut records) = (Vec::new(), None, 0_usize);
    while cursor.next(&mut payload)?.is_some() {
        records = records.saturating_add(1);
        last = Some(mem::take(&mut payload));
    }
    Ok((last.as_deref().map(unframed).transpose()?, records))
}

impl CatalogFile {
    /// Creates a catalog in `dir`, checksummed with `hash` and encoded
    /// through `pipeline`.
    ///
    /// # Errors
    ///
    /// The block file's: SQLCODE -915 if it exists or cannot be written.
    fn create(fs: Arc<dyn FileSystem>, dir: PathBuf, (hash, pipeline): (Arc<dyn HashFunction>, Pipeline)) -> Result<CatalogFile> {
        let blocks = BlockFile::create(&*fs, &dir.join(name(0)), Arc::clone(&hash), pipeline.clone(), BlockOptions::default())?;
        let active = Active { blocks, which: 0, records: 0, broken: false };
        Ok(CatalogFile { fs, dir, hash, pipeline, active: Mutex::new(Some(active)), last: Mutex::new(None) })
    }

    /// Opens the catalog in `dir`: of its two files, the one whose last
    /// record is the later, the other removed; records a crash cut short
    /// are cut off. Each is read with the hash function and encodings it
    /// names, of `formats`'.
    ///
    /// # Errors
    ///
    /// The block file's: SQLCODE -916 if one is not a block file or holds
    /// a malformed record, -915 if one cannot be read or removed.
    fn open(fs: Arc<dyn FileSystem>, dir: PathBuf, formats: &FileFormats) -> Result<CatalogFile> {
        let mut found = Vec::new();
        for which in 0..2 {
            let path = dir.join(name(which));
            if fs.exists(&path)? {
                let (blocks, _) = BlockFile::open(&*fs, &path, &formats.hashes, &formats.encodings, BlockOptions::default())?;
                let (last, records) = last_record(&blocks)?;
                found.push((Active { blocks, which, records, broken: false }, last));
            }
        }
        let newest = found.iter().enumerate().max_by_key(|(_, (_, last))| last.as_ref().map(|(sequence, _)| *sequence)).map(|(at, _)| at);
        let active = newest.and_then(|at| (at < found.len()).then(|| found.swap_remove(at)));
        let (active, last) = active.ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] {} holds no catalog", dir.display())))?;
        for (other, _) in found {
            drop(other.blocks);
            fs.remove(&dir.join(name(other.which)))?;
        }
        let (hash, pipeline) = (Arc::clone(active.blocks.hash()), active.blocks.pipeline().clone());
        Ok(CatalogFile { fs, dir, hash, pipeline, active: Mutex::new(Some(active)), last: Mutex::new(last) })
    }
}

impl CatalogStore for CatalogFile {
    fn last(&self) -> Option<Vec<u8>> {
        lock_builtin(&self.last).as_ref().map(|(_, record)| record.clone())
    }

    /// Records `record`, durably, with the next sequence number; in a new
    /// file once this one holds `compact_after` records. Its file is taken
    /// out while it is written, so no lock is held over the file system,
    /// a plug-in (RULES.md R-ERR-4); records take turns already, as
    /// checkpoints do (`db/checkpoint.rs`), so one asked for meanwhile is
    /// refused, never waited for.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if another record is being written; the file system's
    /// and the block file's.
    fn record(&self, record: &[u8], compact_after: usize) -> Result<()> {
        let taken = lock_builtin(&self.active).take();
        let mut active = taken
            .ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a catalog record is being written: records take turns (RULES.md R-ERR-4)"))?;
        let sequence = lock_builtin(&self.last).as_ref().map_or(0, |(s, _)| s.saturating_add(1));
        let written = self.write(&mut active, &framed(sequence, record), compact_after);
        *lock_builtin(&self.active) = Some(active);
        written?;
        *lock_builtin(&self.last) = Some((sequence, record.to_vec()));
        Ok(())
    }
}

impl CatalogFile {
    /// Writes `bytes`, a record framed, to `active`, durably; or, once it
    /// holds `compact_after` records or an append to it failed, alone to
    /// the other file, made anew, which `active` then is.
    fn write(&self, active: &mut Active, bytes: &[u8], compact_after: usize) -> Result<()> {
        if active.records < compact_after && !active.broken {
            let appended = active.blocks.append(bytes).and_then(|_| active.blocks.sync(Durability::Full));
            if let Err(e) = appended {
                active.broken = true;
                return Err(e);
            }
            active.records = active.records.saturating_add(1);
        } else {
            let which = 1_usize.saturating_sub(active.which);
            let (path, old) = (self.dir.join(name(which)), self.dir.join(name(active.which)));
            if self.fs.exists(&path)? {
                self.fs.remove(&path)?;
            }
            let blocks = BlockFile::create(&*self.fs, &path, Arc::clone(&self.hash), self.pipeline.clone(), BlockOptions::default())?;
            blocks.append(bytes)?;
            blocks.sync(Durability::Full)?;
            self.fs.sync_dir(&self.dir)?;
            self.fs.remove(&old)?;
            self.fs.sync_dir(&self.dir)?;
            *active = Active { blocks, which, records: 1, broken: false };
        }
        Ok(())
    }
}

// Unit tests: records read back as written, the last whole one stands, and
// the file is compacted into the other (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;

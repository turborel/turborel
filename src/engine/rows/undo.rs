//! \[DESIGN\] The undo store of the row store (RULES.md D31): the older
//! committed versions of rows, appended to a `Log`, so they go to block
//! files in segments (`FileLog`), or to memory (`MemoryLog`), as the log
//! plugged in. Every table of a row store shares it.
//!
//! A version is appended when a transaction first changes a row, and is
//! needed while a reader may go back to it: until the transaction rolls
//! back, or commits at a timestamp every open snapshot is past. Each
//! position appended is pinned until then; the log is truncated before the
//! oldest pinned one, so whole segments go once nothing in them is needed.
//! A long transaction keeps what was appended after its first change.
//!
//! A store over a log on disk gathers versions in batches (`batched`), each
//! written as one record once it holds enough, so an update writes a few
//! large records rather than one small one per row; the log's block
//! manager leaves batching to the components above it (`disk/block.rs`).
//! A version's position is then its batch's number and its index in the
//! batch, and the store keeps where each batch it wrote is in the log.
//! Versions of a batch not yet written are read from memory: the batch
//! being gathered, and those being written, which no lock is held over.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::log::{Log, Lsn};
use crate::sync::lock_builtin;

/// Bits of a batched position that hold a version's index in its batch.
const INDEX_BITS: u32 = 16;
/// The index bits of a batched position.
const INDEX_MASK: u64 = (1 << INDEX_BITS) - 1;
/// The most versions a batch holds.
const MOST_VERSIONS: usize = 1 << INDEX_BITS;

/// \[DESIGN\] The undo store: a log of older versions, with the positions
/// still needed.
pub struct UndoStore {
    log: Arc<dyn Log>,
    /// Bytes of versions gathered before they are written as one record;
    /// 0 writes each alone, its position the log's.
    batch: usize,
    /// The positions still needed, each with how many hold it.
    pinned: Mutex<BTreeMap<Lsn, usize>>,
    /// Whether a position was released since the last truncation: only then
    /// can there be more to remove, and every transaction's end asks.
    released: AtomicBool,
    batches: Mutex<Batches>,
}

/// The batches of a batched store.
#[derive(Default)]
struct Batches {
    /// The number of the batch being gathered, its versions, and their bytes.
    number: u64,
    gathering: Vec<Vec<u8>>,
    bytes: usize,
    /// Batches being written, by number, read from memory until they are.
    writing: BTreeMap<u64, Arc<Vec<Vec<u8>>>>,
    /// Batches written, by number: where the log holds each.
    written: BTreeMap<u64, Lsn>,
    /// The batch last read from the log, as readers going back through a
    /// row's versions, or along a table, read one batch many times.
    last: Option<(u64, Arc<Vec<Vec<u8>>>)>,
}

impl UndoStore {
    /// An undo store keeping its versions in `log`, each alone, which
    /// should be empty, or hold nothing still needed.
    pub fn new(log: Arc<dyn Log>) -> UndoStore {
        UndoStore::batched(log, 0)
    }

    /// An undo store keeping its versions in `log`, as `new`, gathered in
    /// batches of `bytes`: for a log whose each record costs a write.
    pub fn batched(log: Arc<dyn Log>, bytes: usize) -> UndoStore {
        let (pinned, batches) = (Mutex::new(BTreeMap::new()), Mutex::new(Batches::default()));
        UndoStore { log, batch: bytes, pinned, released: AtomicBool::new(false), batches }
    }

    /// Appends `version`, pinned; its position. The append and the pin are
    /// one step, so a truncation never falls between them.
    ///
    /// # Errors
    ///
    /// The log's, if the version, or the batch it fills, cannot be written.
    pub(super) fn append(&self, version: &[u8]) -> Result<Lsn> {
        if self.batch == 0 {
            let mut pinned = lock_builtin(&self.pinned);
            let at = self.log.append(version)?;
            pin(&mut pinned, at);
            return Ok(at);
        }
        let (at, full) = {
            let mut batches = lock_builtin(&self.batches);
            let at = Lsn(batches.number.checked_shl(INDEX_BITS).unwrap_or(u64::MAX) | index(batches.gathering.len()));
            batches.gathering.push(version.to_vec());
            batches.bytes = batches.bytes.saturating_add(version.len());
            pin(&mut lock_builtin(&self.pinned), at);
            let full = batches.bytes >= self.batch || batches.gathering.len() >= MOST_VERSIONS;
            (at, full.then(|| take_batch(&mut batches)))
        };
        if let Some((number, batch)) = full
            && let Err(e) = self.write(number, &batch)
        {
            // The batch stays in memory, readable; this version is not
            // the caller's, as it is told the append failed.
            self.release(at);
            return Err(e);
        }
        Ok(at)
    }

    /// Writes batch `number`, then records where it is: no lock held over
    /// the write, so appends and reads go on.
    fn write(&self, number: u64, batch: &[Vec<u8>]) -> Result<()> {
        let mut record = Vec::new();
        record.put_u32(u32::try_from(batch.len()).unwrap_or(u32::MAX));
        for version in batch {
            record.put_bytes(version);
        }
        let lsn = self.log.append(&record)?;
        let mut batches = lock_builtin(&self.batches);
        batches.writing.remove(&number);
        batches.written.insert(number, lsn);
        Ok(())
    }

    /// The version at `at`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the log no longer holds it: it was released too soon,
    /// or lost; the log's errors.
    pub(super) fn read(&self, at: Lsn) -> Result<Vec<u8>> {
        let gone = || SqlError::new(Code::Corrupt, format!("[DESIGN] an older row version, at {}, is not in the undo store", at.0));
        if self.batch == 0 {
            return self.log.read_at(at)?.ok_or_else(gone);
        }
        let (number, at_index) = (at.0 >> INDEX_BITS, usize::try_from(at.0 & INDEX_MASK).unwrap_or(usize::MAX));
        let lsn = {
            let batches = lock_builtin(&self.batches);
            let in_memory = if number == batches.number {
                batches.gathering.get(at_index).cloned()
            } else {
                let cached = batches.last.as_ref().filter(|(n, _)| *n == number).map(|(_, b)| b);
                cached.or_else(|| batches.writing.get(&number)).and_then(|b| b.get(at_index).cloned())
            };
            if let Some(version) = in_memory {
                return Ok(version);
            }
            *batches.written.get(&number).ok_or_else(gone)?
        };
        let batch = Arc::new(decoded(&self.log.read_at(lsn)?.ok_or_else(gone)?)?);
        let version = batch.get(at_index).cloned().ok_or_else(gone)?;
        lock_builtin(&self.batches).last = Some((number, batch));
        Ok(version)
    }

    /// Releases position `at`: no reader will go back to it.
    pub(super) fn release(&self, at: Lsn) {
        let mut pinned = lock_builtin(&self.pinned);
        if let Some(holders) = pinned.get_mut(&at) {
            *holders = holders.saturating_sub(1);
            if *holders == 0 {
                pinned.remove(&at);
                self.released.store(true, Ordering::Release);
            }
        }
    }

    /// Removes what is before the oldest position still needed, as far as
    /// the log removes records.
    ///
    /// # Errors
    ///
    /// The log's, if it cannot remove them; they stay for a later call.
    pub(super) fn truncate(&self) -> Result<()> {
        if !self.released.swap(false, Ordering::AcqRel) {
            return Ok(());
        }
        let oldest = lock_builtin(&self.pinned).first_key_value().map(|(&at, _)| at);
        let before = if self.batch == 0 { oldest.unwrap_or_else(|| self.log.end()) } else { self.batches_before(oldest) };
        self.log.truncate_before(before).inspect_err(|_| self.released.store(true, Ordering::Release))
    }

    /// The log position before which no batch still needed is, once those
    /// before `oldest`'s batch, the oldest position pinned, are forgotten.
    /// Batches may be written out of their order, so it is the least
    /// position of those still needed, and no later than the log's end,
    /// past which a batch being written will be.
    fn batches_before(&self, oldest: Option<Lsn>) -> Lsn {
        let mut batches = lock_builtin(&self.batches);
        let needed = oldest.map_or(batches.number, |at| at.0 >> INDEX_BITS);
        batches.written = batches.written.split_off(&needed);
        batches.writing = batches.writing.split_off(&needed);
        if batches.last.as_ref().is_some_and(|(n, _)| *n < needed) {
            batches.last = None;
        }
        batches.written.values().copied().min().unwrap_or_else(|| self.log.end()).min(self.log.end())
    }

    /// How many positions are pinned: for tests and statistics.
    pub fn pinned(&self) -> usize {
        lock_builtin(&self.pinned).len()
    }
}

/// Pins position `at` once more.
fn pin(pinned: &mut BTreeMap<Lsn, usize>, at: Lsn) {
    let holders = pinned.entry(at).or_default();
    *holders = holders.saturating_add(1);
}

/// The index `n` as the low bits of a position.
fn index(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(INDEX_MASK) & INDEX_MASK
}

/// The batch being gathered, taken to be written: its number, and its
/// versions, readable meanwhile; the next begun.
fn take_batch(batches: &mut Batches) -> (u64, Arc<Vec<Vec<u8>>>) {
    let number = batches.number;
    let batch = Arc::new(std::mem::take(&mut batches.gathering));
    batches.bytes = 0;
    batches.number = number.saturating_add(1);
    batches.writing.insert(number, Arc::clone(&batch));
    (number, batch)
}

/// The versions of a batch's record.
///
/// # Errors
///
/// SQLCODE -916 if it is not one.
fn decoded(record: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut r = Reader::new(record, "undo batch");
    let count = r.u32()?;
    let mut versions = Vec::new();
    for _ in 0..count {
        versions.push(r.bytes()?.to_vec());
    }
    r.finish()?;
    Ok(versions)
}

// Unit tests: batches, read and truncated (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

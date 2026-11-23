//! \[DESIGN\] The undo store of the row store (RULES.md D31): the older
//! committed versions of rows, appended to a `Log`, so they go to block
//! files in segments (`FileLog`), to memory (`MemoryLog`), or to pages of
//! a temporary table's file (`db/temporary/undo.rs`), as the log plugged
//! in. Every table of a row store shares it.
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
//! A store over a log of pages (`fitted`) writes a batch before the next
//! version would carry its record past a page, so a record is one page.
//! A version's position is then its batch's number and its index in the
//! batch, and the store keeps where each batch it wrote is in the log.
//! Versions of a batch not yet written are read from memory: the batches
//! being gathered, and those being written, which no lock is held over.
//! Each stripe of threads gathers a batch of its own, and a version is
//! pinned by its batch's count of it, so an append takes no lock the whole
//! store shares (`undo/gathered.rs`, RULES.md D873).
//!
//! A version read is shared, not copied (`undo/held.rs`): each gathered is
//! kept alone, and each batch written is kept as its record and where each
//! version lies in it, so a read costs no allocation per version.
//!
//! Batches in memory are found through shards (`undo/resident.rs`, RULES.md
//! D745), each under a lock of its own taken for reading, so readers of
//! many sessions going back through hot rows never wait on one lock. A
//! batch written stays in memory until the store is truncated past it,
//! within a share of the memory budget (`undo/kept.rs`, D746): the versions
//! an open snapshot may still read are served from memory, and the log is
//! read only for a batch the bound made go. The last `CACHED` batches read
//! back from the log are kept as well, as readers going back to one read
//! it many times.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::log::{Log, Lsn};
use crate::memory::MemoryBudget;
use crate::sync::lock_builtin;
use gathered::{Gathering, Pinned, Stripes};
pub(super) use held::{Held, Seen};
use kept::Kept;
use resident::{Batch, Resident, Stored, Versions};

/// Bits of a batched position that hold a version's index in its batch.
const INDEX_BITS: u32 = 16;
/// The index bits of a batched position.
const INDEX_MASK: u64 = (1 << INDEX_BITS) - 1;
/// The most versions a batch holds.
const MOST_VERSIONS: usize = 1 << INDEX_BITS;
/// Bytes of a batch's record before its versions, a `u32` count, and
/// before each version, its length, a `u32`.
const LENGTH_BYTES: usize = 4;
/// \[DESIGN\] The batches read back from the log kept in memory: a few of
/// `UNDO_BATCH`'s 32 KiB each (`db/files.rs`).
const CACHED: usize = 8;

/// \[DESIGN\] The undo store: a log of older versions, with the positions
/// still needed.
pub struct UndoStore {
    log: Arc<dyn Log>,
    /// Bytes of versions gathered before they are written as one record;
    /// 0 writes each alone, its position the log's.
    batch: usize,
    /// Whether a batch is written before a version would carry its record
    /// past `batch` bytes, rather than once its versions reach them.
    fit: bool,
    /// Of a store of no batches, the positions still needed, each with
    /// how many hold it.
    pinned: Mutex<BTreeMap<Lsn, usize>>,
    /// Of a store of batches, the pins of each batch (`undo/gathered.rs`).
    pins: Pinned,
    /// Whether a position of a store of no batches, or the last pinned
    /// version of a batch, was released since the last truncation:
    /// only then can there be more to remove, and every transaction's end
    /// asks.
    released: AtomicBool,
    /// The batches being gathered, one a stripe of threads.
    stripes: Stripes,
    /// The next batch's number, and where the batches written are: taken
    /// as a batch begins and ends, by a truncation, and by readers only
    /// for a version memory no longer holds.
    batches: Mutex<Batches>,
    /// The batches in memory, which readers find without `batches`.
    resident: Resident,
    /// The last batches read back from the log, at most `CACHED`, the
    /// oldest first.
    read: Mutex<Vec<Stored>>,
}

/// Where the batches of a batched store are.
struct Batches {
    /// The number the next batch begun takes.
    number: u64,
    /// Batches being written, by number, each with the log's end as it was
    /// taken, which its record will be at or past, so a truncation keeps
    /// what is from there on.
    writing: BTreeMap<u64, Lsn>,
    /// Batches written, by number: where the log holds each.
    written: BTreeMap<u64, Lsn>,
    /// Where the batches written are, the least first, for truncation.
    places: BTreeSet<Lsn>,
    /// The oldest batch not yet forgotten by a truncation.
    first: u64,
    /// Which batches written stay in memory.
    kept: Kept,
}

impl UndoStore {
    /// An undo store keeping its versions in `log`, each alone, which
    /// should be empty, or hold nothing still needed.
    pub fn new(log: Arc<dyn Log>) -> UndoStore {
        UndoStore::batched(log, 0)
    }

    /// An undo store keeping its versions in `log`, as `new`, gathered in
    /// batches of `bytes`: for a log whose each record costs a write. No
    /// batch written stays in memory but by `in_budget`.
    pub fn batched(log: Arc<dyn Log>, bytes: usize) -> UndoStore {
        let batches = Batches {
            number: 0,
            writing: BTreeMap::new(),
            written: BTreeMap::new(),
            places: BTreeSet::new(),
            first: 0,
            kept: Kept::none(),
        };
        let (pinned, released, read) = (Mutex::new(BTreeMap::new()), AtomicBool::new(false), Mutex::new(Vec::new()));
        let (pins, stripes) = (Pinned::default(), Stripes::new());
        UndoStore {
            log,
            batch: bytes,
            fit: false,
            pinned,
            pins,
            released,
            stripes,
            batches: Mutex::new(batches),
            resident: Resident::new(),
            read,
        }
    }

    /// An undo store keeping its versions in `log`, as `batched`, each
    /// batch written before the next version would carry its record past
    /// `bytes`, which should be more than 0: for a log whose each record is
    /// a page of `bytes` of content. A version too long for one is a batch
    /// alone.
    pub fn fitted(log: Arc<dyn Log>, bytes: usize) -> UndoStore {
        UndoStore { fit: true, ..UndoStore::batched(log, bytes.max(1)) }
    }

    /// This store, its batches written kept in memory within a share of
    /// `budget` (`undo/kept.rs`, RULES.md D746), so readers of open
    /// snapshots read the log only for a batch the share made go.
    ///
    /// # Errors
    ///
    /// The budget's, if it refuses even an empty reservation.
    pub fn in_budget(self, budget: &Arc<MemoryBudget>) -> Result<UndoStore> {
        lock_builtin(&self.batches).kept = Kept::in_budget(budget)?;
        Ok(self)
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
        let missing = || SqlError::new(Code::Api, "[DESIGN] a stripe of the undo store's batches is missing");
        let (at, full) = {
            let mut stripe = self.stripes.mine().ok_or_else(missing)?;
            let early = if self.overflows(&stripe, version) { self.take_batch(&mut stripe) } else { None };
            let number = stripe.number.unwrap_or_else(|| self.begin_batch(&mut stripe));
            let at = Lsn(number.checked_shl(INDEX_BITS).unwrap_or(u64::MAX) | index(stripe.count));
            self.resident.push(number, Arc::from(version));
            // Counted in its batch, it is pinned (`undo/gathered.rs`).
            stripe.count_one(version.len());
            let reached = !self.fit && stripe.bytes >= self.batch;
            let full = reached || stripe.count >= MOST_VERSIONS;
            (at, early.or_else(|| if full { self.take_batch(&mut stripe) } else { None }))
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

    /// Whether `version`, gathered into `stripe`'s batch, would carry the
    /// record of a fitted store's batch past its bytes, the batch holding
    /// any: its count, then each version's length and bytes (`write`).
    fn overflows(&self, stripe: &Gathering, version: &[u8]) -> bool {
        let lengths = stripe.count.saturating_add(2).saturating_mul(LENGTH_BYTES);
        let record = lengths.saturating_add(stripe.bytes).saturating_add(version.len());
        self.fit && stripe.count > 0 && record > self.batch
    }

    /// Begins `stripe`'s next batch: its number, the store's next, its
    /// pins kept from now.
    fn begin_batch(&self, stripe: &mut Gathering) -> u64 {
        let mut batches = lock_builtin(&self.batches);
        let number = batches.number;
        batches.number = number.saturating_add(1);
        self.pins.begin(number, stripe);
        number
    }

    /// The batch `stripe` gathers, if it has one, taken to be written, its
    /// record to be at or past the log's end: its number, and its
    /// versions, readable meanwhile. The stripe's next version begins
    /// another.
    fn take_batch(&self, stripe: &mut Gathering) -> Option<(u64, Versions)> {
        let taken = std::mem::take(stripe);
        let number = taken.number?;
        let versions = self.resident.take(number);
        lock_builtin(&self.batches).writing.insert(number, self.log.end());
        if self.pins.close(number, taken.count) {
            self.released.store(true, Ordering::Release);
        }
        Some((number, versions))
    }

    /// Writes batch `number`, then records where it is: no lock held over
    /// the write, so appends and reads go on. Its record stays in memory if
    /// the batches kept have room (`undo/kept.rs`); what memory lets go is
    /// dropped once no lock is held.
    fn write(&self, number: u64, batch: &[Arc<[u8]>]) -> Result<()> {
        let record = record_of(number, batch);
        let lsn = self.log.append(&record.record)?;
        let gone: Vec<Batch> = {
            let mut batches = lock_builtin(&self.batches);
            batches.writing.remove(&number);
            // One a truncation forgot meanwhile is needed by no reader.
            if number < batches.first {
                return Ok(());
            }
            batches.written.insert(number, lsn);
            batches.places.insert(lsn);
            let (evicted, kept) = batches.kept.keep(number, record.bytes());
            let mut gone: Vec<Batch> = evicted.into_iter().filter_map(|n| self.resident.remove(n)).collect();
            gone.extend(self.resident.written(number, kept.then_some(record)));
            gone
        };
        drop(gone);
        Ok(())
    }

    /// The version at `at`: from memory, through its batch's shard, unless
    /// the batch was let go; then from the log.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the log no longer holds it: it was released too soon,
    /// or lost; the log's errors.
    pub(super) fn read(&self, at: Lsn) -> Result<Held> {
        let gone = || SqlError::new(Code::Corrupt, format!("[DESIGN] an older row version, at {}, is not in the undo store", at.0));
        if self.batch == 0 {
            return self.log.read_at(at)?.map(Held::from).ok_or_else(gone);
        }
        let (number, at_index) = (at.0 >> INDEX_BITS, usize::try_from(at.0 & INDEX_MASK).unwrap_or(usize::MAX));
        if let Some(version) = self.resident.held(number, at_index) {
            return Ok(version);
        }
        if let Some(version) = lock_builtin(&self.read).iter().rev().find(|r| r.number == number).and_then(|r| r.held(at_index)) {
            return Ok(version);
        }
        // Memory lets a batch go only once where it is in the log is
        // recorded (`write`), so it is found here.
        let lsn = *lock_builtin(&self.batches).written.get(&number).ok_or_else(gone)?;
        let record = self.log.read_at(lsn)?.ok_or_else(gone)?;
        let read = Stored { number, spans: spans(&record)?, record: Arc::new(record) };
        let version = read.held(at_index).ok_or_else(gone)?;
        keep_read(&mut lock_builtin(&self.read), read);
        Ok(version)
    }

    /// Releases position `at`: no reader will go back to it.
    pub(super) fn release(&self, at: Lsn) {
        if self.batch > 0 {
            let (number, at_index) = (at.0 >> INDEX_BITS, usize::try_from(at.0 & INDEX_MASK).unwrap_or(usize::MAX));
            if self.pins.release(number, at_index) {
                self.released.store(true, Ordering::Release);
            }
            return;
        }
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
        let before = if self.batch == 0 { self.alone_before() } else { self.batches_before() };
        self.log.truncate_before(before).inspect_err(|_| self.released.store(true, Ordering::Release))
    }

    /// The log position before which no version still needed is, in a
    /// store of no batches: the oldest pinned, or the log's end if none
    /// is. Both are read under the pins' lock, which an append holds over
    /// its write and its pin, so a version appended meanwhile is at or past
    /// what is returned; the end read after the lock is let go could be
    /// past a version appended and pinned in between.
    fn alone_before(&self) -> Lsn {
        let pinned = lock_builtin(&self.pinned);
        pinned.first_key_value().map_or_else(|| self.log.end(), |(&at, _)| at)
    }

    /// The log position before which no batch still needed is, once those
    /// before the oldest batch holding a pinned version are forgotten, in
    /// memory too. Batches may be written out of their order, so it is the
    /// least position of those still needed: of each written, where it is;
    /// of each being written, the log's end as it was taken, as its record
    /// may be in the log already, not yet recorded as written; and no
    /// later than the log's end. Every stripe is held meanwhile, so no
    /// version is appended, and so pinned, and no batch taken to be
    /// written, whose record is then at or past the end read here. Read
    /// once the stripes are let go, the end could be past such a record,
    /// and the truncation remove a batch a reader still has pinned, as
    /// `alone_before` says of a version alone. A batch being gathered
    /// whose every version is released is given up, so a stripe no thread
    /// appends through holds nothing back.
    fn batches_before(&self) -> Lsn {
        let (least, gone) = {
            let mut stripes = self.stripes.all();
            let mut batches = lock_builtin(&self.batches);
            let (gathering, mut gone) = self.gathering_needed(&mut stripes);
            let needed = gathering.into_iter().chain(self.pins.oldest_written()).min().unwrap_or(batches.number);
            let later = batches.written.split_off(&needed);
            for lsn in std::mem::replace(&mut batches.written, later).into_values() {
                batches.places.remove(&lsn);
            }
            batches.writing = batches.writing.split_off(&needed);
            batches.kept.forget_before(needed);
            gone.extend((batches.first..needed).filter_map(|n| self.resident.remove(n)));
            batches.first = batches.first.max(needed);
            lock_builtin(&self.read).retain(|r| r.number >= needed);
            let floors = batches.writing.values().min().copied();
            let end = self.log.end();
            (batches.places.first().copied().into_iter().chain(floors).min().map_or(end, |least| least.min(end)), gone)
        };
        drop(gone);
        least
    }

    /// Of the batches `stripes` gather, each held, the oldest holding a
    /// pinned version, if any does; one holding none is given up, gone
    /// from memory, its stripe's next version beginning another: what was
    /// given up, to be dropped once no lock is held.
    fn gathering_needed(&self, stripes: &mut [std::sync::MutexGuard<'_, Gathering>]) -> (Option<u64>, Vec<Batch>) {
        let (mut oldest, mut gone) = (None, Vec::new());
        for stripe in stripes {
            let Some(number) = stripe.number else { continue };
            if self.pins.live(number, stripe.count) > 0 {
                oldest = Some(oldest.map_or(number, |least: u64| least.min(number)));
            } else {
                self.pins.forget(number);
                gone.extend(self.resident.remove(number));
                **stripe = Gathering::default();
            }
        }
        (oldest, gone)
    }

    /// How many positions are pinned: for tests and statistics.
    pub fn pinned(&self) -> usize {
        if self.batch == 0 {
            return lock_builtin(&self.pinned).len();
        }
        let stripes = self.stripes.all();
        let gathering = stripes.iter().filter_map(|stripe| Some(self.pins.live(stripe.number?, stripe.count)));
        gathering.fold(self.pins.written(), usize::saturating_add)
    }

    /// The bytes of the batches written kept in memory: for tests and
    /// statistics.
    pub fn kept_bytes(&self) -> usize {
        lock_builtin(&self.batches).kept.bytes()
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

/// The record of batch `number`, `batch`: its count, then each version's
/// length and bytes; and where each version lies in it.
fn record_of(number: u64, batch: &[Arc<[u8]>]) -> Stored {
    let lengths = batch.len().saturating_add(1).saturating_mul(LENGTH_BYTES);
    let mut record = Vec::with_capacity(batch.iter().fold(lengths, |n, v| n.saturating_add(v.len())));
    record.put_u32(u32::try_from(batch.len()).unwrap_or(u32::MAX));
    let mut spans = Vec::with_capacity(batch.len());
    for version in batch {
        record.put_bytes(version);
        let (end, len) = (record.len(), version.len());
        spans.push((end.saturating_sub(len), end));
    }
    Stored { number, record: Arc::new(record), spans }
}

/// Keeps `read`, a batch read back from the log, in `cached`, the oldest
/// kept forgotten if `CACHED` are; unless another reader kept it meanwhile.
fn keep_read(cached: &mut Vec<Stored>, read: Stored) {
    if cached.iter().any(|r| r.number == read.number) {
        return;
    }
    if cached.len() >= CACHED {
        cached.remove(0);
    }
    cached.push(read);
}

/// Where each version of a batch's record lies in it.
///
/// # Errors
///
/// SQLCODE -916 if it is not one.
fn spans(record: &[u8]) -> Result<Vec<(usize, usize)>> {
    let mut r = Reader::new(record, "undo batch");
    let count = r.u32()?;
    let mut spans = Vec::with_capacity(usize::try_from(count).unwrap_or(0).min(MOST_VERSIONS));
    for _ in 0..count {
        let version = r.bytes()?;
        let end = r.position();
        spans.push((end.saturating_sub(version.len()), end));
    }
    r.finish()?;
    Ok(spans)
}

mod gathered;
mod held;
mod kept;
mod resident;

// A log for the tests of the store's races.
#[cfg(test)]
pub(super) mod hooked;
// Unit tests: batches, read and truncated (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

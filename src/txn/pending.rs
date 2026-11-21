//! \[DESIGN\] A transaction's log records held until it commits (RULES.md
//! D25, D470): each row change is encoded as it is made, as before, but
//! kept in the transaction's own buffer, with no lock another session
//! takes and no write; at its commit the records go to the log together,
//! one write (`Log::append_all`), before it takes its commit timestamp,
//! and its commit record after. So a transaction writes the log twice,
//! where it wrote it for every change.
//!
//! The data file moves only at a checkpoint, so no page holding a change
//! is durable before a checkpoint is; and a checkpoint, while its gate is
//! shut, first writes every open transaction's held records
//! (`Transactions::write_held`), so the image never holds a change whose
//! record comes after it in the log. Conflicting changes are ordered by
//! their commits (RULES.md D29), which order their records too: a later
//! writer's records are written after the commit that let it write.
//!
//! A transaction holding more than `HELD_BYTES` writes them as a run of
//! its own, under the gate, and holds on from there. A record's place in
//! the log is known only once it is written, so a place undone to is
//! noted by the record's number among the transaction's (`LogPos::Held`):
//! undoing to a place whose records are all still held drops them, and
//! nothing is logged; undoing to one written logs the records from it
//! cancelled, as before (`Change::StatementUndone`), for which a write
//! notes the places of the records the transaction marked (`wanted`).

use std::collections::HashMap;
use std::mem;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{Transactions, TxId, TxState, Txn};
use crate::error::{Code, Result, SqlError};
use crate::log::{Log, Lsn};
use crate::sync::lock_builtin;

/// \[DESIGN\] The bytes of records a transaction holds before it writes
/// them as a run of their own: 1 MiB, so a large transaction holds
/// little, and the run is one write of a size a device takes at once.
pub(crate) const HELD_BYTES: usize = 1024 * 1024;

/// \[DESIGN\] The shards a database's map of the transactions holding
/// records is split in, by transaction id, as `txn/stop.rs` splits its own.
const HELD_SHARDS: u64 = 32;

/// The place of a record of a transaction: its number among the
/// transaction's records, while it may still be held, or its position in
/// the log, once known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogPos {
    /// The record numbered so, from 0, in the order the transaction made
    /// them.
    Held(u64),
    /// The record at this position in the log.
    At(Lsn),
}

/// What is to be logged of the records from a place undone to.
pub(crate) enum Undone {
    /// Nothing: they were all held, and are dropped.
    Nothing,
    /// That those from this position are cancelled (`Change::StatementUndone`).
    From(Lsn),
    /// The position of the first is not known: the transaction can then
    /// only roll back.
    Unknown,
}

/// A transaction's records not yet written, and what it knows of those
/// written.
#[derive(Default)]
struct Held {
    /// The records held, oldest first.
    records: Vec<Vec<u8>>,
    /// Their bytes.
    bytes: usize,
    /// The number of the first held: those before it are written.
    base: u64,
    /// The numbers of records a place undone to may name: a write notes
    /// their positions in `found`.
    wanted: Vec<u64>,
    /// Of `wanted` records written, the number and position.
    found: Vec<(u64, Lsn)>,
    /// Whether a record of it may have reached the log: its end is then
    /// logged, as an abort if it rolls back.
    reached: bool,
    /// Whether its first record written is noted as its first
    /// (`Transactions::first_logged`): written before its end, by a
    /// checkpoint or a run past `HELD_BYTES`.
    pinned: bool,
    /// Why a write of its records failed, if one did: it can then only
    /// roll back, what it changed being partly logged.
    lost: Option<SqlError>,
}

impl Held {
    /// Drops the records held from the one numbered `n`, if held.
    fn truncate_from(&mut self, n: u64) {
        let keep = usize::try_from(n.saturating_sub(self.base)).unwrap_or(usize::MAX);
        self.records.truncate(keep);
        self.bytes = self.records.iter().map(Vec::len).sum();
    }
}

/// \[DESIGN\] A transaction's held records and the log they go to: the
/// one the transaction writes, found by a checkpoint through `Pending`.
pub(crate) struct TxLog {
    log: Arc<dyn Log>,
    held: Mutex<Held>,
}

impl TxLog {
    /// Writes the records held, then `last`, if given, in one run of
    /// blocks: `last`'s position, or the last record's, if any was
    /// written. The positions of the records wanted are noted, and a
    /// first run of a transaction that goes on is noted as its first.
    ///
    /// # Errors
    ///
    /// The error of an earlier write that failed; the log's. A failure
    /// leaves the transaction able only to roll back.
    fn write(&self, shared: &Transactions, tx: TxId, last: Option<Vec<u8>>) -> Result<Option<Lsn>> {
        let ends = last.is_some();
        let (mut records, from) = {
            let mut held = lock_builtin(&self.held);
            if let Some(lost) = &held.lost {
                return Err(lost.clone());
            }
            let from = held.base;
            held.base = from.saturating_add(u64::try_from(held.records.len()).unwrap_or(u64::MAX));
            held.bytes = 0;
            (mem::take(&mut held.records), from)
        };
        let count = records.len();
        records.extend(last);
        if records.is_empty() {
            return Ok(None);
        }
        // Appended with the buffer unlocked: the log is a plug-in (RULES.md
        // R-ERR-4). Its session writes under the gate, and a checkpoint
        // with the gate shut, so the two never write it at once.
        let written = self.log.append_all(&records);
        let mut held = lock_builtin(&self.held);
        held.reached = true;
        let lsns = written.inspect_err(|e| {
            held.lost.get_or_insert_with(|| e.clone());
        })?;
        for (n, at) in (from..).zip(lsns.iter().take(count)) {
            if held.wanted.contains(&n) {
                held.found.push((n, *at));
            }
        }
        let first = (!ends && !held.pinned).then(|| lsns.first().copied()).flatten();
        held.pinned |= first.is_some();
        drop(held);
        if let Some(first) = first {
            shared.first_logged(tx, first);
        }
        shared.logged.fetch_add(u64::try_from(count).unwrap_or(u64::MAX), Ordering::Relaxed);
        Ok(lsns.last().copied())
    }
}

/// \[DESIGN\] The transactions holding records, for a checkpoint to write
/// them: in `HELD_SHARDS` shards, by transaction id.
pub(crate) struct Pending {
    shards: Vec<Mutex<HashMap<TxId, Arc<TxLog>>>>,
}

impl Default for Pending {
    fn default() -> Self {
        Pending { shards: (0..HELD_SHARDS).map(|_| Mutex::new(HashMap::new())).collect() }
    }
}

impl Pending {
    /// The shard of `tx`, if there is one.
    fn shard(&self, tx: TxId) -> Option<&Mutex<HashMap<TxId, Arc<TxLog>>>> {
        usize::try_from(tx % HELD_SHARDS).ok().and_then(|at| self.shards.get(at))
    }
}

impl Transactions {
    /// \[DESIGN\] Writes the records every open transaction holds: a
    /// checkpoint's, with its gate shut, before it takes the log's end
    /// (`db/checkpoint.rs`), so the records of every change its image holds
    /// come before that end.
    ///
    /// # Errors
    ///
    /// The log's: the checkpoint then fails, and the transaction whose
    /// records could not be written can only roll back.
    pub(crate) fn write_held(&self) -> Result<()> {
        for shard in &self.pending.shards {
            let open: Vec<(TxId, Arc<TxLog>)> = lock_builtin(shard).iter().map(|(&tx, held)| (tx, Arc::clone(held))).collect();
            for (tx, held) in open {
                held.write(self, tx, None)?;
            }
        }
        Ok(())
    }
}

impl Txn {
    /// Its held records' buffer, made and noted among the database's as it
    /// first holds one.
    fn tx_log(&self, log: &Arc<dyn Log>) -> &Arc<TxLog> {
        self.pending.get_or_init(|| {
            let made = Arc::new(TxLog { log: Arc::clone(log), held: Mutex::new(Held::default()) });
            if let Some(shard) = self.shared.pending.shard(self.id) {
                lock_builtin(shard).insert(self.id, Arc::clone(&made));
            }
            made
        })
    }

    /// Holds `record`, a change it made, to be written to `log` at its
    /// commit: the place of the first record of each open statement,
    /// mark and savepoint with none yet. Past `HELD_BYTES`, or once it
    /// wrote a table whose engine makes its own writes durable, which it
    /// may do before the commit, what it holds is written at once.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if its state's lock is poisoned; the log's, if what it
    /// holds was to be written and could not be.
    pub(super) fn hold(&self, log: &Arc<dyn Log>, record: Vec<u8>) -> Result<()> {
        let tx_log = self.tx_log(log);
        let full = {
            let mut st = self.state()?;
            let mut held = lock_builtin(&tx_log.held);
            let n = held.base.saturating_add(u64::try_from(held.records.len()).unwrap_or(u64::MAX));
            held.bytes = held.bytes.saturating_add(record.len());
            held.records.push(record);
            st.logged = true;
            if st.logged_at(LogPos::Held(n)) {
                held.wanted.push(n);
            }
            held.bytes > HELD_BYTES || st.forced
        };
        if full {
            let _gate = self.shared.gate.enter();
            tx_log.write(&self.shared, self.id, None)?;
        }
        Ok(())
    }

    /// Holds `record`, no place noted: a record of what was undone.
    ///
    /// # Errors
    ///
    /// As `hold`.
    pub(super) fn hold_unmarked(&self, log: &Arc<dyn Log>, record: Vec<u8>) -> Result<()> {
        let tx_log = self.tx_log(log);
        let full = {
            let mut held = lock_builtin(&tx_log.held);
            held.bytes = held.bytes.saturating_add(record.len());
            held.records.push(record);
            held.bytes > HELD_BYTES
        };
        if full {
            let _gate = self.shared.gate.enter();
            tx_log.write(&self.shared, self.id, None)?;
        }
        Ok(())
    }

    /// Writes what it holds now, under the gate: before a record of its own
    /// appended apart from them, a definition's (`State::log_definition`).
    ///
    /// # Errors
    ///
    /// As `write_with`.
    pub(crate) fn write_held_now(&self) -> Result<()> {
        let Some(tx_log) = self.pending.get() else { return Ok(()) };
        let _gate = self.shared.gate.enter();
        tx_log.write(&self.shared, self.id, None).map(|_| ())
    }

    /// Writes what it still holds and then `last`, its commit record, in one
    /// run: `last`'s position. What it held is written before, as its commit
    /// begins (`Txn::write_held_first`), unless that failed or the database
    /// took no change.
    ///
    /// # Errors
    ///
    /// The log's, or that of an earlier write of its records that failed.
    pub(super) fn write_with(&self, log: &Arc<dyn Log>, last: Vec<u8>) -> Result<Lsn> {
        let written = self.tx_log(log).write(&self.shared, self.id, Some(last))?;
        written.ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] the log gave no position for the commit record"))
    }

    /// Whether a record of it may have reached the log, so its end is
    /// logged.
    pub(super) fn reached_log(&self) -> bool {
        self.pending.get().is_some_and(|tx_log| lock_builtin(&tx_log.held).reached)
    }

    /// Whether its first record was noted as its first, so it is to be
    /// forgotten as it ends.
    pub(super) fn pinned_log(&self) -> bool {
        self.pending.get().is_some_and(|tx_log| lock_builtin(&tx_log.held).pinned)
    }

    /// Drops what it holds from `from`, a place undone to: what is then to
    /// be logged of the records from it.
    pub(super) fn drop_held_from(&self, from: LogPos) -> Undone {
        let Some(tx_log) = self.pending.get() else {
            return match from {
                LogPos::At(at) => Undone::From(at),
                LogPos::Held(_) => Undone::Nothing,
            };
        };
        let mut held = lock_builtin(&tx_log.held);
        let base = held.base;
        match from {
            LogPos::At(at) => {
                held.truncate_from(base);
                Undone::From(at)
            }
            LogPos::Held(n) => {
                // Held still, it and what follows go unwritten; written, so
                // did every record after it.
                let still = n >= base;
                held.truncate_from(if still { n } else { base });
                let found = held.found.iter().find(|(k, _)| *k == n).map(|&(_, at)| at);
                if still { Undone::Nothing } else { found.map_or(Undone::Unknown, Undone::From) }
            }
        }
    }

    /// Notes in `st` the positions of the written records its places name,
    /// as a statement begins, and wants only the records the places left
    /// name: so what a write notes stays as few as the places open.
    pub(super) fn settle_places(&self, st: &mut TxState) {
        let Some(tx_log) = self.pending.get() else { return };
        let mut held = lock_builtin(&tx_log.held);
        let found = mem::take(&mut held.found);
        held.wanted.clear();
        for place in st.places() {
            if let Some(LogPos::Held(n)) = *place {
                if let Some(&(_, at)) = found.iter().find(|(k, _)| *k == n) {
                    *place = Some(LogPos::At(at));
                } else {
                    held.wanted.push(n);
                }
            }
        }
    }

    /// Forgets its held records' buffer among the database's, as it ends.
    pub(super) fn forget_held(&self) {
        if self.pending.get().is_some()
            && let Some(shard) = self.shared.pending.shard(self.id)
        {
            lock_builtin(shard).remove(&self.id);
        }
    }
}

/// The first record an open transaction wrote before its end: its
/// position, the row changes logged before it, and when.
pub(super) struct First {
    at: Lsn,
    logged: u64,
    when: Instant,
}

impl Transactions {
    /// The position of the first record of the oldest open transaction
    /// that wrote one, if any.
    pub(crate) fn oldest_logged(&self) -> Option<Lsn> {
        lock_builtin(&self.firsts).values().map(|first| first.at).min()
    }

    /// Notes that `tx` wrote its first record at `at`.
    pub(crate) fn first_logged(&self, tx: TxId, at: Lsn) {
        let logged = self.logged.load(Ordering::Relaxed);
        lock_builtin(&self.firsts).entry(tx).or_insert(First { at, logged, when: Instant::now() });
    }

    /// Of the oldest open transaction that wrote a record, from whose
    /// first the log is kept: the row changes logged since, its own among
    /// them, and how long ago it wrote it.
    pub(crate) fn pinned(&self) -> Option<(u64, Duration)> {
        let firsts = lock_builtin(&self.firsts);
        let oldest = firsts.values().min_by_key(|first| first.at)?;
        Some((self.logged.load(Ordering::Relaxed).saturating_sub(oldest.logged), oldest.when.elapsed()))
    }
}

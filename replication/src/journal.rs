//! \[EXTENSION\] A replica's journal, an extension point (RULES.md D57,
//! R-SAFE-3): where a driver keeps what its consensus has it persist, the
//! records of its log and the protocol's own state, so a replica made
//! again after a crash has not forgotten what it did ([VRR §4.3]: "If nodes
//! record their state on disk before sending messages, a node will be able
//! to rejoin the system as soon as it has reinitialized its state by
//! reading from disk").
//!
//! Built in: [`LogJournal`], a journal kept in a core `Log`, so a journal
//! on files is the core's file log, checksummed, encoded through the
//! database's encodings, and cut at a torn end as the database's own log
//! is (RULES.md D25).

use std::collections::BTreeMap;
use std::sync::Arc;

use turborel::bytes::{Put, Reader};
use turborel::error::Result;
use turborel::log::Log;

use crate::{Configuration, Lsn, Mark, Member, Recovered, ReplicaId};

/// \[EXTENSION\] A replica's journal: the outputs `Persist`, `Truncate` and
/// `PersistState` of a [`Consensus`](crate::Consensus) carried out.
///
/// Contract:
/// * Writes take effect in the order given; a crash keeps a prefix of
///   them, and every write before a `sync` or `persist_state` that returned.
/// * `persist_state` makes the state, and every write before it, durable
///   at once, before it returns.
/// * `sync` returns the positions of the records made durable since it
///   last returned, each once, in the order they were persisted: those
///   `persist_state` and `flush` made durable too; a journal on a slow
///   disk may make its writes durable after `sync` returns.
/// * `flush` makes every write durable, before it returns, and returns
///   as `sync` does.
/// * Opened again, it holds what a crash kept (`Recovered`): the records
///   as the writes left them, and the last state.
pub trait Journal: Send {
    /// Whether what it makes durable survives a crash.
    fn durable(&self) -> bool;
    /// Writes `record` at `at`, the position after its last.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it cannot be written.
    fn persist(&mut self, at: Lsn, record: &[u8]) -> Result<()>;
    /// Removes the records after `after`.
    ///
    /// # Errors
    ///
    /// As `persist`.
    fn truncate(&mut self, after: Lsn) -> Result<()>;
    /// \[EXTENSION\] Lets the records through `through` go (RULES.md D57,
    /// log retention): opened again, the journal's records begin after it
    /// (`Recovered::first`), whether or not it held every one through it;
    /// `through` before where they began, the journal begins again there,
    /// holding none: a state transfer installed begins the log before
    /// where it was let go, with the exporter's records persisted after.
    ///
    /// # Errors
    ///
    /// As `persist`.
    fn retain(&mut self, through: Lsn) -> Result<()>;
    /// \[EXTENSION\] Marks the record at `at`, persisted before or next,
    /// as the reconfiguration `mark` [VRR §7]: opened again, the journal's
    /// `Recovered::marks` has it, as long as the record is held; a mark
    /// at the position after the last is the record's persisted next, at
    /// once, and void otherwise, as a record persisted there later, after
    /// a crash lost the one marked, is another's.
    ///
    /// # Errors
    ///
    /// As `persist`.
    fn mark(&mut self, at: Lsn, mark: &Mark) -> Result<()>;
    /// Writes the protocol's state, and makes it and every write before it
    /// durable.
    ///
    /// # Errors
    ///
    /// As `persist`, or if it cannot be made durable.
    fn persist_state(&mut self, state: &[u8]) -> Result<()>;
    /// The records made durable since it last returned, by position, the
    /// writes given made durable as the journal has it.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if they cannot be made durable.
    fn sync(&mut self) -> Result<Vec<Lsn>>;
    /// \[EXTENSION\] Makes every write durable, before it returns; the
    /// records made durable since `sync` or this last returned, by
    /// position (RULES.md D57: a state transfer's records are durable
    /// before the database takes the transfer).
    ///
    /// # Errors
    ///
    /// As `sync`.
    fn flush(&mut self) -> Result<Vec<Lsn>>;
}

/// What the entries of a [`LogJournal`] are called in errors.
const JOURNAL: &str = "replication journal entry";

/// \[EXTENSION\] The tags of a [`LogJournal`]'s entries; never reused.
const RECORD: u8 = 1;
const TRUNCATE: u8 = 2;
const STATE: u8 = 3;
const RETAIN: u8 = 4;
/// \[EXTENSION\] A record marked a reconfiguration (`Journal::mark`).
const MARK: u8 = 5;

/// A configuration as entries and frames carry it: its members, each its
/// number and address.
pub(crate) fn put_configuration(out: &mut Vec<u8>, configuration: &Configuration) {
    out.put_u32(u32::try_from(configuration.len()).unwrap_or(u32::MAX));
    for member in configuration {
        out.put_u32(member.id.0);
        out.put_str(&member.address);
    }
}

/// A configuration read, as `put_configuration` wrote it.
///
/// # Errors
///
/// The reader's, if it ends first.
pub(crate) fn read_configuration(r: &mut Reader<'_>) -> Result<Configuration> {
    let count = r.u32()?;
    let mut configuration = Vec::new();
    for _ in 0..count {
        configuration.push(Member { id: ReplicaId(r.u32()?), address: r.str()? });
    }
    Ok(configuration)
}

/// A mark as entries and frames carry it: the epoch it begins, then its
/// configuration.
pub(crate) fn put_mark(out: &mut Vec<u8>, mark: &Mark) {
    out.put_u64(mark.epoch);
    put_configuration(out, &mark.configuration);
}

/// A mark read, as `put_mark` wrote it.
///
/// # Errors
///
/// The reader's, if it ends first.
pub(crate) fn read_mark(r: &mut Reader<'_>) -> Result<Mark> {
    let epoch = r.u64()?;
    Ok(Mark { epoch, configuration: read_configuration(r)? })
}

/// \[EXTENSION\] A journal kept in a core `Log`: each write one record of
/// it, a tag then its fields (`turborel::bytes`): a record, its position,
/// whether the mark written just before is its, then its bytes; a
/// truncation, the last position kept; a state, its
/// bytes; a retention, the last position let go. Opening it reads them
/// all, in order, as the writes left the journal. A retention, once
/// durable, lets the log's records before the first record kept go
/// (`Log::truncate_before`): a file log drops its segments before it.
pub struct LogJournal {
    log: Arc<dyn Log>,
    /// The log's position of the last entry written and not yet synced.
    unsynced: Option<Lsn>,
    /// The records written and not yet synced, by position.
    waiting: Vec<Lsn>,
    /// The records made durable and not yet returned by `sync`.
    owed: Vec<Lsn>,
    /// Where in the log each record held was written, by its position.
    written: BTreeMap<u64, Lsn>,
    /// The position the last write marked, if it was a mark: the record
    /// written next there is marked by it.
    marking: Option<u64>,
}

impl LogJournal {
    /// The journal kept in `log`, and what it holds.
    ///
    /// # Errors
    ///
    /// The log's errors reading; SQLCODE -916 if an entry is malformed, or
    /// writes a record past the position after the last.
    pub fn open(log: Arc<dyn Log>) -> Result<(LogJournal, Recovered)> {
        let mut recovered = Recovered::default();
        let mut written = BTreeMap::new();
        for read in log.read_from(Lsn(0)) {
            let (at, entry) = read?;
            apply(&mut recovered, &entry, (at, &mut written))?;
        }
        // A mark written for a record a crash lost is void.
        let last = recovered.first.0.saturating_add(u64::try_from(recovered.records.len()).unwrap_or(u64::MAX));
        recovered.marks.retain(|&marked, _| marked.0 <= last);
        Ok((LogJournal { log, unsynced: None, waiting: Vec::new(), owed: Vec::new(), written, marking: None }, recovered))
    }

    /// Writes `entry`.
    fn write(&mut self, entry: &[u8]) -> Result<()> {
        self.unsynced = Some(self.log.append(entry)?);
        Ok(())
    }

    /// Syncs what was written, if anything was.
    fn sync_writes(&mut self) -> Result<()> {
        if let Some(through) = self.unsynced {
            self.log.sync(through)?;
            self.unsynced = None;
        }
        self.owed.append(&mut self.waiting);
        Ok(())
    }
}

impl Journal for LogJournal {
    fn durable(&self) -> bool {
        self.log.capabilities().durable
    }

    fn persist(&mut self, at: Lsn, record: &[u8]) -> Result<()> {
        let mut entry = vec![RECORD];
        entry.put_u64(at.0);
        entry.put_u8(u8::from(self.marking.take() == Some(at.0)));
        entry.extend_from_slice(record);
        self.write(&entry)?;
        if let Some(wrote) = self.unsynced {
            self.written.split_off(&at.0);
            self.written.insert(at.0, wrote);
        }
        self.waiting.push(at);
        Ok(())
    }

    fn truncate(&mut self, after: Lsn) -> Result<()> {
        let mut entry = vec![TRUNCATE];
        entry.put_u64(after.0);
        self.written.split_off(&after.0.saturating_add(1));
        self.marking = None;
        self.write(&entry)
    }

    /// The retention written and made durable first, so the log is never
    /// cut before a journal that does not say so; then the log before the
    /// first record kept let go.
    fn retain(&mut self, through: Lsn) -> Result<()> {
        let mut entry = vec![RETAIN];
        entry.put_u64(through.0);
        self.marking = None;
        self.write(&entry)?;
        self.sync_writes()?;
        self.written = self.written.split_off(&through.0.saturating_add(1));
        if let Some(&kept) = self.written.values().next() {
            self.log.truncate_before(kept)?;
        }
        Ok(())
    }

    fn mark(&mut self, at: Lsn, mark: &Mark) -> Result<()> {
        let mut entry = vec![MARK];
        entry.put_u64(at.0);
        put_mark(&mut entry, mark);
        self.write(&entry)?;
        self.marking = Some(at.0);
        Ok(())
    }

    fn persist_state(&mut self, state: &[u8]) -> Result<()> {
        let mut entry = vec![STATE];
        entry.extend_from_slice(state);
        self.marking = None;
        self.write(&entry)?;
        self.sync_writes()
    }

    /// Every write made durable: a file log syncs as `sync` is asked.
    fn sync(&mut self) -> Result<Vec<Lsn>> {
        self.sync_writes()?;
        Ok(std::mem::take(&mut self.owed))
    }

    fn flush(&mut self) -> Result<Vec<Lsn>> {
        self.sync()
    }
}

/// A RETAIN entry applied: the records through `through` let go, the
/// journal beginning after it, before or after where it did.
fn retain(recovered: &mut Recovered, written: &mut BTreeMap<u64, Lsn>, through: u64) {
    if through > recovered.first.0 {
        let gone = usize::try_from(through.saturating_sub(recovered.first.0)).unwrap_or(usize::MAX).min(recovered.records.len());
        recovered.records.drain(..gone);
        recovered.first = Lsn(through);
        recovered.marks = recovered.marks.split_off(&Lsn(through.saturating_add(1)));
        *written = written.split_off(&through.saturating_add(1));
    } else if through < recovered.first.0 {
        recovered.records.clear();
        recovered.marks.clear();
        recovered.first = Lsn(through);
        written.clear();
    }
}

/// `recovered` as the entry `entry`, written at `at` of the log, leaves
/// it; `written` where each record kept was written.
fn apply(recovered: &mut Recovered, entry: &[u8], (at, written): (Lsn, &mut BTreeMap<u64, Lsn>)) -> Result<()> {
    let mut r = Reader::new(entry, JOURNAL);
    match r.u8()? {
        RECORD => {
            let position = r.u64()?;
            let marked = flag(&mut r)?;
            // The log's first entry, the entries before let go with the
            // records they wrote: the journal begins before it.
            if recovered.records.is_empty() && position > recovered.first.0.saturating_add(1) {
                recovered.first = Lsn(position.saturating_sub(1));
            }
            let kept = position.checked_sub(recovered.first.0).and_then(|n| n.checked_sub(1)).and_then(|n| usize::try_from(n).ok());
            let kept = kept.filter(|&n| n <= recovered.records.len());
            let kept = kept.ok_or_else(|| r.corrupt("it writes a record past the position after the last, or one let go"))?;
            recovered.records.truncate(kept);
            recovered.records.push(r.rest().to_vec());
            // A mark at this position is the record's if the record says
            // so, written just after it; another's record there, written
            // after a crash lost the one marked, is unmarked; the records
            // after, let go, take their marks.
            recovered.marks.retain(|&at_mark, _| at_mark.0 < position || (at_mark.0 == position && marked));
            written.split_off(&position);
            written.insert(position, at);
        }
        TRUNCATE => {
            let after = r.u64()?;
            r.finish()?;
            recovered.records.truncate(usize::try_from(after.saturating_sub(recovered.first.0)).unwrap_or(usize::MAX));
            recovered.marks.retain(|&marked, _| marked.0 <= after);
            written.split_off(&after.saturating_add(1));
        }
        MARK => {
            let marked = Lsn(r.u64()?);
            let mark = read_mark(&mut r)?;
            r.finish()?;
            // A mark for the record written next is kept until that is
            // read, which says whether it is its.
            let last = recovered.first.0.saturating_add(u64::try_from(recovered.records.len()).unwrap_or(u64::MAX));
            if marked > recovered.first && marked.0 <= last.saturating_add(1) {
                recovered.marks.insert(marked, mark);
            }
        }
        RETAIN => {
            let through = r.u64()?;
            r.finish()?;
            retain(recovered, written, through);
        }
        STATE => recovered.state = Some(r.rest().to_vec()),
        _ => return Err(r.corrupt("its tag is of no entry known")),
    }
    Ok(())
}

/// A flag read: 0 or 1.
///
/// # Errors
///
/// SQLCODE -916 if it is another byte.
fn flag(r: &mut Reader<'_>) -> Result<bool> {
    match r.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(r.corrupt("its flag is neither 0 nor 1")),
    }
}

//! A replica's journal, as a driver keeps it: the records a consensus has
//! it persist, and its state, some written and waiting to be made durable.
//! A crash keeps what is durable, and of what is waiting, as many writes as
//! the test says reached the disk.

use std::collections::{BTreeMap, VecDeque};

use turborel::error::{Code, Result, SqlError};
use turborel_replication::{Lsn, Mark, Recovered};

/// A write given to the journal and not yet durable.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Write {
    /// A record at a position.
    Record(u64, Vec<u8>),
    /// The records after a position removed.
    Truncate(u64),
    /// The records through a position let go.
    Retain(u64),
    /// The record at a position marked a reconfiguration.
    Mark(u64, Mark),
}

/// A replica's journal.
#[derive(Debug, Clone, Default)]
pub struct Journal {
    /// What survives a crash.
    durable: Recovered,
    /// The position before the first record as written: those through it
    /// let go.
    first: u64,
    /// The records as written, durable or not, after `first`: what the
    /// driver reads to deliver them.
    records: Vec<Vec<u8>>,
    /// The marks, as written, durable or not.
    marks: BTreeMap<Lsn, Mark>,
    /// The position marked by the last write, if that was a mark: a mark
    /// at the position after the last record is the next record's, if
    /// that is written next, and void otherwise.
    marking: Option<u64>,
    /// As `marking`, of the writes durable.
    durable_marking: Option<u64>,
    /// Writes not yet durable, in order.
    waiting: VecDeque<Write>,
    /// The positions of records made durable and not yet answered with
    /// `Persisted`, in order.
    owed: VecDeque<Lsn>,
}

impl Journal {
    /// The journal a replica recovers from: a mark durable for a record
    /// not yet, written next, is void.
    pub fn recovered(&self) -> Recovered {
        let mut recovered = self.durable.clone();
        let last = recovered.first.0 + u64::try_from(recovered.records.len()).unwrap();
        recovered.marks.retain(|at, _| at.0 <= last);
        recovered
    }

    /// Whether it holds a state: a replica made from it is not recovering.
    pub fn holds_state(&self) -> bool {
        self.durable.state.is_some()
    }

    /// The records as written.
    pub fn records(&self) -> &[Vec<u8>] {
        &self.records
    }

    /// The record at `at`, as written.
    pub fn record(&self, at: Lsn) -> Option<&Vec<u8>> {
        let index = usize::try_from(at.0.checked_sub(self.first)?.checked_sub(1)?).ok()?;
        self.records.get(index)
    }

    /// The position before the first record as written.
    pub fn first(&self) -> Lsn {
        Lsn(self.first)
    }

    /// How many writes wait to be made durable.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// `Persist { at, record }`: the record written at `at`, which is the
    /// position after the last.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `at` is another position: the consensus's log and
    /// its journal disagree.
    pub fn persist(&mut self, at: Lsn, record: Vec<u8>) -> Result<()> {
        let next = self.first + u64::try_from(self.records.len()).unwrap() + 1;
        if at.0 != next {
            return Err(SqlError::new(
                Code::Api,
                format!("[TOOL] a Persist at {}, not at {next}, the position after the journal's last", at.0),
            ));
        }
        self.records.push(record.clone());
        if self.marking != Some(at.0) {
            self.marks.remove(&at);
        }
        self.marking = None;
        self.waiting.push_back(Write::Record(at.0, record));
        Ok(())
    }

    /// `Truncate { after }`.
    pub fn truncate(&mut self, after: Lsn) {
        self.records.truncate(usize::try_from(after.0.saturating_sub(self.first)).unwrap());
        self.marks.retain(|at, _| *at <= after);
        self.marking = None;
        self.waiting.push_back(Write::Truncate(after.0));
    }

    /// `Mark { at, mark }`: the record at `at`, written or written next,
    /// marked, with its write waiting; a mark for a record written next
    /// is void unless that record is the next write.
    pub fn mark(&mut self, at: Lsn, mark: &Mark) {
        self.waiting.push_back(Write::Mark(at.0, mark.clone()));
        let next = self.first + u64::try_from(self.records.len()).unwrap() + 1;
        if at.0 <= next {
            self.marks.insert(at, mark.clone());
        }
        self.marking = Some(at.0);
    }

    /// `Retain { through }`: before the first position, the journal begins
    /// again there, holding none.
    pub fn retain(&mut self, through: Lsn) {
        retain(&mut self.first, &mut self.records, through.0);
        if through.0 < self.first {
            self.marks.clear();
        }
        self.marking = None;
        self.waiting.push_back(Write::Retain(through.0));
    }

    /// `PersistState`: the state written, and it and every write before it
    /// durable at once. The `Persisted` the records are answered with are
    /// still owed.
    pub fn persist_state(&mut self, state: Vec<u8>) {
        while self.make_durable() {}
        self.durable.state = Some(state);
    }

    /// Makes the oldest write waiting durable; false if none waits.
    pub fn make_durable(&mut self) -> bool {
        let Some(write) = self.waiting.pop_front() else { return false };
        let marking = self.durable_marking.take();
        match write {
            Write::Record(at, record) => {
                self.durable.records.truncate(usize::try_from(at - self.durable.first.0 - 1).unwrap());
                self.durable.records.push(record);
                if marking != Some(at) {
                    self.durable.marks.remove(&Lsn(at));
                }
                self.owed.push_back(Lsn(at));
            }
            Write::Truncate(after) => {
                self.durable.records.truncate(usize::try_from(after.saturating_sub(self.durable.first.0)).unwrap());
                self.durable.marks.retain(|at, _| at.0 <= after);
            }
            Write::Retain(through) => {
                let mut first = self.durable.first.0;
                retain(&mut first, &mut self.durable.records, through);
                self.durable.first = Lsn(first);
                self.durable.marks = self.durable.marks.split_off(&Lsn(through.saturating_add(1)));
                if through < first {
                    self.durable.marks.clear();
                }
            }
            Write::Mark(at, mark) => {
                self.durable.marks.insert(Lsn(at), mark);
                self.durable_marking = Some(at);
            }
        }
        true
    }

    /// The `Persisted` owed, in order, taken.
    pub fn take_owed(&mut self) -> Vec<Lsn> {
        self.owed.drain(..).collect()
    }

    /// `flush`: every write made durable at once, as a sync of the disk
    /// would, and the `Persisted` owed taken.
    pub fn flush(&mut self) -> Vec<Lsn> {
        while self.make_durable() {}
        self.take_owed()
    }

    /// A crash: of the writes waiting, the first `reached` reached the disk;
    /// the rest are lost, as are the `Persisted` owed. The journal is then
    /// what is durable.
    pub fn crash(&mut self, reached: usize) {
        for _ in 0..reached {
            if !self.make_durable() {
                break;
            }
        }
        self.waiting.clear();
        self.owed.clear();
        self.first = self.durable.first.0;
        self.records.clone_from(&self.durable.records);
        // A mark durable for a record the crash lost is void: a record
        // written at its position after is another's.
        let last = self.durable.first.0 + u64::try_from(self.durable.records.len()).unwrap();
        self.durable.marks.retain(|at, _| at.0 <= last);
        self.marks.clone_from(&self.durable.marks);
        self.marking = None;
        self.durable_marking = None;
    }
}

/// Lets the records of `records`, after `first`, through `through` go.
fn retain(first: &mut u64, records: &mut Vec<Vec<u8>>, through: u64) {
    if through < *first {
        records.clear();
        *first = through;
        return;
    }
    if through == *first {
        return;
    }
    let gone = usize::try_from(through - *first).unwrap().min(records.len());
    records.drain(..gone);
    *first = through;
}

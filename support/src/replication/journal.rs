//! A replica's journal, as a driver keeps it: the records a consensus has
//! it persist, and its state, some written and waiting to be made durable.
//! A crash keeps what is durable, and of what is waiting, as many writes as
//! the test says reached the disk.

use std::collections::VecDeque;

use turborel_replication::{Lsn, Recovered};

/// A write given to the journal and not yet durable.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Write {
    /// A record at a position.
    Record(u64, Vec<u8>),
    /// The records after a position removed.
    Truncate(u64),
}

/// A replica's journal.
#[derive(Debug, Clone, Default)]
pub struct Journal {
    /// What survives a crash.
    durable: Recovered,
    /// The records as written, durable or not: what the driver reads to
    /// deliver them.
    records: Vec<Vec<u8>>,
    /// Writes not yet durable, in order.
    waiting: VecDeque<Write>,
    /// The positions of records made durable and not yet answered with
    /// `Persisted`, in order.
    owed: VecDeque<Lsn>,
}

impl Journal {
    /// The journal a replica recovers from.
    pub fn recovered(&self) -> Recovered {
        self.durable.clone()
    }

    /// The records as written.
    pub fn records(&self) -> &[Vec<u8>] {
        &self.records
    }

    /// The record at `at`, as written.
    pub fn record(&self, at: Lsn) -> Option<&Vec<u8>> {
        let index = usize::try_from(at.0.checked_sub(1)?).ok()?;
        self.records.get(index)
    }

    /// How many writes wait to be made durable.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// `Persist { at, record }`: the record written at `at`, which is the
    /// position after the last.
    pub fn persist(&mut self, at: Lsn, record: Vec<u8>) {
        assert_eq!(at.0, u64::try_from(self.records.len()).unwrap() + 1, "a Persist not at the position after the journal's last");
        self.records.push(record.clone());
        self.waiting.push_back(Write::Record(at.0, record));
    }

    /// `Truncate { after }`.
    pub fn truncate(&mut self, after: Lsn) {
        self.records.truncate(usize::try_from(after.0).unwrap());
        self.waiting.push_back(Write::Truncate(after.0));
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
        match write {
            Write::Record(at, record) => {
                self.durable.records.truncate(usize::try_from(at - 1).unwrap());
                self.durable.records.push(record);
                self.owed.push_back(Lsn(at));
            }
            Write::Truncate(after) => self.durable.records.truncate(usize::try_from(after).unwrap()),
        }
        true
    }

    /// The `Persisted` owed, in order, taken.
    pub fn take_owed(&mut self) -> Vec<Lsn> {
        self.owed.drain(..).collect()
    }

    /// A crash: of the writes waiting, the first `reached` reached the disk;
    /// the rest are lost, as are the `Persisted` owed. The journal is then
    /// what is durable.
    pub fn crash(&mut self, reached: usize) {
        for _ in 0..reached {
            self.make_durable();
        }
        self.waiting.clear();
        self.owed.clear();
        self.records.clone_from(&self.durable.records);
    }
}

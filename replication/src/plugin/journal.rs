//! \[EXTENSION\] A journal served by a host plug-in (kind 33, RULES.md
//! D272): each write one call, and a record taken as durable only once
//! SYNC or FLUSH names it. The bridge keeps the records persisted and not
//! yet named, in order, and a program naming one it was not given, or out
//! of the order they were persisted, breaks the protocol and the replica
//! stops: no record is acknowledged that the journal was not given, and a
//! program that ends mid-write acknowledges none.

use std::collections::VecDeque;

use turborel::bytes::Put;
use turborel::error::Result;
use turborel::plugin::errors::violation;
use turborel::plugin::payload::replication::{
    JOURNAL_FLUSH, JOURNAL_MARK, JOURNAL_OPEN, JOURNAL_PERSIST, JOURNAL_RETAIN, JOURNAL_STATE, JOURNAL_SYNC, JOURNAL_TRUNCATE, held,
    positions, put_mark,
};

use super::convert::{configuration_to, recovered_from};
use super::{Pipe, Stopping};
use crate::{Journal, Lsn, Mark, Recovered};

/// \[EXTENSION\] A host plug-in's journal.
pub struct PluginJournal {
    pipe: Stopping,
    durable: bool,
    /// The records persisted and not yet named durable, in order.
    waiting: VecDeque<u64>,
}

impl PluginJournal {
    /// The journal over `pipe`, opened, and what it holds.
    ///
    /// # Errors
    ///
    /// The pipe's.
    pub fn open(pipe: Box<dyn Pipe>) -> Result<(PluginJournal, Recovered)> {
        let pipe = Stopping::new(pipe, "journal");
        let got = pipe.ask(JOURNAL_OPEN, Vec::new(), held)?;
        let durable = got.durable;
        Ok((PluginJournal { pipe, durable, waiting: VecDeque::new() }, recovered_from(got)))
    }

    /// A write: operation `op` of `payload`, answered by nothing.
    fn write(&self, op: u8, payload: Vec<u8>) -> Result<()> {
        self.pipe.ask(op, payload, |_| Ok(()))
    }

    /// The positions `op` names durable, each checked against those
    /// persisted and waiting, in order.
    fn named(&mut self, op: u8) -> Result<Vec<Lsn>> {
        let named = self.pipe.ask(op, Vec::new(), positions)?;
        for &at in &named {
            if self.waiting.front() != Some(&at) {
                let why = format!("its journal named record {at} durable, which was not the next persisted, {:?}", self.waiting.front());
                return Err(self.pipe.fail(&violation(&why)));
            }
            self.waiting.pop_front();
        }
        Ok(named.into_iter().map(Lsn).collect())
    }
}

impl Journal for PluginJournal {
    fn durable(&self) -> bool {
        self.durable
    }

    fn persist(&mut self, at: Lsn, record: &[u8]) -> Result<()> {
        let mut payload = Vec::new();
        payload.put_u64(at.0);
        payload.put_bytes(record);
        self.write(JOURNAL_PERSIST, payload)?;
        self.waiting.push_back(at.0);
        Ok(())
    }

    fn truncate(&mut self, after: Lsn) -> Result<()> {
        let mut payload = Vec::new();
        payload.put_u64(after.0);
        self.write(JOURNAL_TRUNCATE, payload)?;
        // Records removed are never made durable, so never named.
        self.waiting.retain(|&at| at <= after.0);
        Ok(())
    }

    fn retain(&mut self, through: Lsn) -> Result<()> {
        let mut payload = Vec::new();
        payload.put_u64(through.0);
        self.write(JOURNAL_RETAIN, payload)
    }

    fn mark(&mut self, at: Lsn, mark: &Mark) -> Result<()> {
        let mut payload = Vec::new();
        payload.put_u64(at.0);
        put_mark(&mut payload, mark.epoch, &configuration_to(&mark.configuration));
        self.write(JOURNAL_MARK, payload)
    }

    fn persist_state(&mut self, state: &[u8]) -> Result<()> {
        let mut payload = Vec::new();
        payload.put_bytes(state);
        self.write(JOURNAL_STATE, payload)
    }

    fn sync(&mut self) -> Result<Vec<Lsn>> {
        self.named(JOURNAL_SYNC)
    }

    fn flush(&mut self) -> Result<Vec<Lsn>> {
        self.named(JOURNAL_FLUSH)
    }
}

//! \[EXTENSION\] A host plug-in's spill (RULES.md D250): where a sort, a
//! hash join or a grouping puts what memory does not hold.
//!
//! An area keeps one process of the program for as long as it lives, so
//! its tables and runs are that program's to hold however it likes. A
//! run's records are pushed a batch at a time, as many as the area's
//! batch of bytes, one call a batch, and read back a page of them a call.
//! A table's inserts are sent a batch at a time too, sent before any read
//! of it, and a read of a key is one call.

use std::sync::{Arc, Mutex};

use crate::bytes::Put;
use crate::error::Result;
use crate::memory::MemoryBudget;
use crate::plugin::bridge::{answer, byte, leaked, number};
use crate::plugin::hosted::{HostLease, HostLink};
use crate::plugin::manifest::HostDeclared;
use crate::plugin::payload::hosted::{
    SPILL_DROP, SPILL_GET, SPILL_INSERT, SPILL_PUSH, SPILL_READ, SPILL_REMOVE, SPILL_RUN, SPILL_TABLE, parts, put_parts,
};
use crate::spill::TempFiles;
use crate::spill::{Area, Records, Spill, SpillArea, SpillRun, SpillTable};
use crate::sync::lock_builtin;

/// Records a page of a run's read holds.
const PAGE: u32 = 1024;

/// A host plug-in's spill.
#[derive(Debug)]
pub struct PluginSpill {
    link: HostLink,
    name: &'static str,
}

impl PluginSpill {
    /// The spill `d` declares.
    pub fn new(d: &HostDeclared, link: HostLink) -> PluginSpill {
        PluginSpill { link, name: leaked(&d.name) }
    }
}

impl Spill for PluginSpill {
    fn name(&self) -> &'static str {
        self.name
    }
    fn area(&self, _memory: &Arc<MemoryBudget>, _temp: &Arc<TempFiles>, area: Area) -> Result<Box<dyn SpillArea>> {
        Ok(Box::new(PluginArea { lease: Arc::new(self.link.lease()?), batch: area.batch.max(1) }))
    }
}

/// An area of a host spill: the process it keeps, and its batch of bytes.
struct PluginArea {
    lease: Arc<HostLease>,
    batch: usize,
}

impl PluginArea {
    /// A new table or run, by `op`: its id.
    fn made(&self, op: u8) -> Result<Held> {
        let id = answer(&self.lease.call(op, Vec::new())?, number)?;
        Ok(Held { lease: Arc::clone(&self.lease), id, batch: self.batch, pending: Mutex::new((Vec::new(), 0)) })
    }
}

impl SpillArea for PluginArea {
    fn table(&self) -> Result<Box<dyn SpillTable>> {
        Ok(Box::new(PluginTable(self.made(SPILL_TABLE)?)))
    }
    fn run(&self) -> Result<Box<dyn SpillRun>> {
        Ok(Box::new(PluginRun(self.made(SPILL_RUN)?)))
    }
}

/// A table or run the program holds: its id, and what waits to be sent.
struct Held {
    lease: Arc<HostLease>,
    id: u64,
    batch: usize,
    pending: Mutex<(Vec<Vec<u8>>, usize)>,
}

impl Held {
    /// A payload beginning with its id.
    fn payload(&self) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.put_u64(self.id);
        payload
    }

    /// Adds `parts` to what waits, sent by `op` once a batch waits.
    fn add(&self, op: u8, parts: &[&[u8]]) -> Result<()> {
        let full = {
            let mut pending = lock_builtin(&self.pending);
            for part in parts {
                pending.1 = pending.1.saturating_add(part.len());
                pending.0.push(part.to_vec());
            }
            pending.1 >= self.batch
        };
        if full { self.send(op) } else { Ok(()) }
    }

    /// Sends what waits by `op`, if anything does, no lock held as it is
    /// sent (R-ERR-4).
    fn send(&self, op: u8) -> Result<()> {
        let (waiting, _) = std::mem::take(&mut *lock_builtin(&self.pending));
        if waiting.is_empty() {
            return Ok(());
        }
        let mut payload = self.payload();
        let parts: Vec<&[u8]> = waiting.iter().map(Vec::as_slice).collect();
        put_parts(&mut payload, &parts);
        self.lease.call(op, payload).map(drop)
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        // Dropped or not, the program's process goes with the area; one
        // that cannot answer has stopped the node.
        drop(self.lease.call(SPILL_DROP, self.payload()));
    }
}

/// A run of a host spill.
struct PluginRun(Held);

impl SpillRun for PluginRun {
    fn push(&mut self, record: &[u8]) -> Result<()> {
        self.0.add(SPILL_PUSH, &[record])
    }
    fn records(&mut self) -> Result<Records> {
        self.0.send(SPILL_PUSH)?;
        let mut all = Vec::new();
        let mut from = 0_u64;
        loop {
            let mut payload = self.0.payload();
            payload.put_u64(from);
            payload.put_u32(PAGE);
            let (page, more) = answer(&self.0.lease.call(SPILL_READ, payload)?, |r| Ok((parts(r)?, r.u8()? == 1)))?;
            from = from.saturating_add(u64::try_from(page.len()).unwrap_or(u64::MAX));
            let ended = !more || page.is_empty();
            all.extend(page);
            if ended {
                return Ok(Box::new(all.into_iter().map(Ok)));
            }
        }
    }
}

/// A table of a host spill.
struct PluginTable(Held);

impl PluginTable {
    /// A payload of its id and `parts`, what waits sent first.
    fn asked(&self, parts: &[&[u8]]) -> Vec<u8> {
        let mut payload = self.0.payload();
        for part in parts {
            payload.put_bytes(part);
        }
        payload
    }
}

impl SpillTable for PluginTable {
    fn insert(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        self.0.add(SPILL_INSERT, &[key, value])
    }
    fn get(&self, key: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.0.send(SPILL_INSERT)?;
        answer(&self.0.lease.call(SPILL_GET, self.asked(&[key]))?, parts)
    }
    fn remove(&mut self, key: &[u8], value: &[u8]) -> Result<bool> {
        self.0.send(SPILL_INSERT)?;
        Ok(answer(&self.0.lease.call(SPILL_REMOVE, self.asked(&[key, value]))?, byte)? == 1)
    }
}

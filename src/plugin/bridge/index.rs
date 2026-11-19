//! \[EXTENSION\] A plug-in's index method (RULES.md D226, D227), an
//! `IndexMethod` whose indexes keep their entries in the program's care.
//!
//! Writes: inserts and removes, of transactions' statements and of the
//! coordinator's undo alike, are kept here in order and given the program
//! a batch at a time (WRITE): when BATCH of them wait, before any read,
//! and at a checkpoint. A WRITE that fails damages the index (SQLSTATE
//! 38T10): its every call fails from then, until the database is opened
//! again and rebuilds it.
//!
//! Reads: a search is SEARCH, a page of BATCH entries a call; ROWS gives
//! a key's rows; NEAREST the nearest entries by the query's metric, an
//! approximate search (`NearestSupport::Approximate`). Each entry's key is
//! checked against the probe, so an entry the program gives wrongly is
//! never read, and the reader rechecks each row against the version it
//! sees, as of every index (RULES.md D34).
//!
//! Recovery: WRITE marks an index not clean in the program's care, and
//! CHECKPOINT, as the database makes its indexes durable, marks it clean;
//! an index not clean when the database opens is rebuilt from its table
//! (`IndexMethod::open` gives none).
//!
//! The manifest's SUPPORTS names what it answers: EQUALITY, `=` and keys'
//! rows; RANGE, every comparison but `<>`, prefixes and ranges, its
//! entries in order of key; NEAREST, approximate nearest searches.

mod search;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};

use crate::error::Result;
use crate::index::{Index, IndexMethod, NearestQuery, NearestSupport};
use crate::plugin::errors::broken;
use crate::plugin::manifest::{EQUALITY, NEAREST, RANGE};
use crate::plugin::payload::index::{Write, put_writes};
use crate::plugin::payload::statistics::{self, IndexAnswer};
use crate::plugin::payload::{INDEX_CHECKPOINT, INDEX_CREATE, INDEX_OPEN, INDEX_STATISTICS, INDEX_WRITE, put_strings};
use crate::sync::lock_builtin;
use crate::value::{CompOp, DataType};

use super::{Link, answer, leaked};

pub use search::PluginIndex;

/// The entries a call carries when the manifest says no BATCH.
pub const DEFAULT_BATCH: usize = 1024;

/// A plug-in's index method.
#[derive(Debug)]
pub struct PluginIndexMethod {
    link: Link,
    name: &'static str,
    supports: Vec<String>,
    batch: usize,
    indexes: Mutex<Vec<Weak<Shared>>>,
}

/// The writes waiting, and whether a thread is giving them.
#[derive(Debug, Default)]
struct Pending {
    writes: Vec<Write>,
    giving: bool,
}

/// What an index and its searches share.
#[derive(Debug)]
pub(super) struct Shared {
    link: Link,
    locator: Vec<u8>,
    batch: usize,
    ordered: bool,
    pending: Mutex<Pending>,
    given: Condvar,
    damaged: AtomicBool,
    /// What the program knows of its entries (RULES.md D278).
    known: super::known::Known<IndexAnswer>,
}

impl PluginIndexMethod {
    /// The method `link` names, answering what `supports` names.
    pub fn new(link: Link, supports: Vec<String>, batch: Option<u32>) -> PluginIndexMethod {
        let name = leaked(&link.object);
        let batch = batch.and_then(|b| usize::try_from(b).ok()).unwrap_or(DEFAULT_BATCH).max(1);
        PluginIndexMethod { link, name, supports, batch, indexes: Mutex::default() }
    }

    /// Whether it SUPPORTS `what`.
    fn has(&self, what: &str) -> bool {
        self.supports.iter().any(|s| s == what)
    }

    /// The index of `locator`, kept for checkpoints.
    fn index(&self, locator: Vec<u8>) -> Box<dyn Index> {
        let shared = Arc::new(Shared {
            link: self.link.clone(),
            locator,
            batch: self.batch,
            ordered: self.has(RANGE),
            pending: Mutex::default(),
            given: Condvar::new(),
            damaged: AtomicBool::new(false),
            known: super::known::Known::default(),
        });
        let mut all = lock_builtin(&self.indexes);
        all.retain(|i| i.strong_count() > 0);
        all.push(Arc::downgrade(&shared));
        Box::new(PluginIndex(shared))
    }
}

impl IndexMethod for PluginIndexMethod {
    fn name(&self) -> &'static str {
        self.name
    }
    fn supports(&self, op: CompOp) -> bool {
        match op {
            CompOp::Eq => self.has(EQUALITY) || self.has(RANGE),
            CompOp::Lt | CompOp::Gt | CompOp::Le | CompOp::Ge => self.has(RANGE),
            CompOp::Ne => false,
        }
    }
    fn supports_prefix(&self, columns: usize, equal: usize, then: Option<CompOp>) -> bool {
        equal <= columns && then.is_none_or(|op| self.supports(op)) && (self.has(RANGE) || (self.has(EQUALITY) && equal == columns))
    }
    fn supports_range(&self, columns: usize, equal: usize) -> bool {
        self.has(RANGE) && equal < columns
    }
    fn supports_nearest(&self, query: &NearestQuery, _columns: &[DataType]) -> NearestSupport {
        if self.has(NEAREST) && query.metric.is_some() { NearestSupport::Approximate } else { NearestSupport::No }
    }
    fn serves_metric(&self, _index: &crate::catalog::IndexDesc, _call: &str) -> bool {
        self.has(NEAREST)
    }
    fn ordered(&self) -> bool {
        self.has(RANGE)
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        let mut payload = Vec::new();
        put_strings(&mut payload, &columns.iter().map(ToString::to_string).collect::<Vec<_>>());
        let locator = answer(&self.link.plain(INDEX_CREATE, payload)?, |r| r.bytes().map(<[u8]>::to_vec))?;
        Ok(self.index(locator))
    }
    fn open(&self, _columns: &[DataType], locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        let mut payload = Vec::new();
        crate::bytes::Put::put_bytes(&mut payload, locator);
        let (found, clean) = answer(&self.link.plain(INDEX_OPEN, payload)?, |r| Ok((r.u8()? != 0, r.u8()? != 0)))?;
        Ok((found && clean).then(|| self.index(locator.to_vec())))
    }
    fn checkpoint(&self) -> Result<()> {
        let all: Vec<Arc<Shared>> = lock_builtin(&self.indexes).iter().filter_map(Weak::upgrade).collect();
        for shared in all {
            shared.give()?;
            let mut payload = Vec::new();
            crate::bytes::Put::put_bytes(&mut payload, &shared.locator);
            shared.link.plain(INDEX_CHECKPOINT, payload)?;
        }
        Ok(())
    }
    fn check_columns(&self, columns: &[DataType], what: &str) -> Result<()> {
        if self.has(EQUALITY) || self.has(RANGE) {
            return columns.iter().try_for_each(|t| crate::binder::key_operand(t, what));
        }
        Ok(())
    }
}

impl Shared {
    /// That it is not damaged.
    fn whole(&self) -> Result<()> {
        if self.damaged.load(Ordering::Acquire) { Err(broken("index", "a write of it failed")) } else { Ok(()) }
    }

    /// `w` kept, and the writes waiting given if BATCH of them wait.
    fn write(&self, w: Write) -> Result<()> {
        self.whole()?;
        self.known.written(1);
        let full = {
            let mut pending = lock_builtin(&self.pending);
            pending.writes.push(w);
            pending.writes.len() >= self.batch && !pending.giving
        };
        if full { self.give() } else { Ok(()) }
    }

    /// Every write waiting given the program, in order, one thread giving
    /// at a time and no lock held across a call (R-ERR-4).
    fn give(&self) -> Result<()> {
        let mut pending = lock_builtin(&self.pending);
        loop {
            while pending.giving {
                pending = self.given.wait(pending).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            self.whole()?;
            if pending.writes.is_empty() {
                return Ok(());
            }
            let writes = std::mem::take(&mut pending.writes);
            pending.giving = true;
            drop(pending);
            let done = self.send(&writes);
            pending = lock_builtin(&self.pending);
            pending.giving = false;
            self.given.notify_all();
            if let Err(e) = done {
                self.damaged.store(true, Ordering::Release);
                return Err(broken("index", &e.msg));
            }
        }
    }

    /// What the program knows of the index's entries, and after how many
    /// written it is stale; none if it knows nothing (STATISTICS): the
    /// writes waiting given first, as before any read.
    ///
    /// # Errors
    ///
    /// The calls'.
    fn statistics(&self) -> Result<Option<(IndexAnswer, u64)>> {
        self.give()?;
        let mut payload = Vec::new();
        crate::bytes::Put::put_bytes(&mut payload, &self.locator);
        let got = answer(&self.link.plain(INDEX_STATISTICS, payload)?, statistics::index)?;
        Ok(got.map(|i| (i, i.refresh)))
    }

    /// What `pick` takes of what the program knows of the index, if it
    /// knows it.
    fn known(&self, pick: impl FnOnce(&IndexAnswer) -> f64) -> Option<f64> {
        self.known.get(|| self.statistics()).and_then(|i| statistics::known(pick(&i)))
    }

    /// One WRITE of `writes`.
    fn send(&self, writes: &[Write]) -> Result<()> {
        let mut payload = Vec::new();
        put_writes(&mut payload, &self.locator, writes)?;
        self.link.plain(INDEX_WRITE, payload).map(drop)
    }
}

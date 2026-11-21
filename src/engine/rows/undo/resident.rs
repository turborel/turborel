//! \[DESIGN\] The batches of the undo store in memory, by number, in shards
//! (RULES.md D31, D745): the batch being gathered, those being written, and
//! those written and kept (`kept.rs`). A reader of an older version takes
//! one shard's lock for reading, and only for the moment of counting a
//! share of the version, so readers of many sessions never wait on each
//! other, nor on appends but to the batch being gathered. Consecutive
//! batches are in different shards, so readers going back to recent
//! versions, the ones read most, spread over them.

use std::collections::BTreeMap;
use std::mem;
use std::sync::{Arc, RwLock};

use super::Held;
use crate::sync::{read_builtin, write_builtin};

/// \[DESIGN\] The shards: a power of two, the batch number's low bits
/// choosing one; more than the threads a machine runs gains nothing.
const SHARDS: usize = 64;
/// The bits of a batch number that choose its shard.
const SHARD_MASK: u64 = 63;

/// The versions of a batch, shared with its readers.
pub(super) type Versions = Arc<Vec<Arc<[u8]>>>;

/// A batch's record, read from the log or as it was written to it, and
/// where each of its versions lies in the record.
pub(super) struct Stored {
    pub(super) number: u64,
    pub(super) record: Arc<Vec<u8>>,
    pub(super) spans: Vec<(usize, usize)>,
}

impl Stored {
    /// Its version at `index`, shared.
    pub(super) fn held(&self, index: usize) -> Option<Held> {
        self.spans.get(index).map(|&span| Held::in_record(&self.record, span))
    }

    /// The bytes it holds: its record and its spans.
    pub(super) fn bytes(&self) -> usize {
        self.record.len().saturating_add(self.spans.len().saturating_mul(size_of::<(usize, usize)>()))
    }
}

/// A batch in memory.
pub(super) enum Batch {
    /// Being gathered: each version alone.
    Gathering(Vec<Arc<[u8]>>),
    /// Being written: its versions, as they were taken.
    Writing(Versions),
    /// Written, and kept: its record.
    Kept(Stored),
}

impl Batch {
    /// Its version at `index`, shared.
    fn held(&self, index: usize) -> Option<Held> {
        match self {
            Batch::Gathering(versions) => versions.get(index).map(Held::version),
            Batch::Writing(versions) => versions.get(index).map(Held::version),
            Batch::Kept(record) => record.held(index),
        }
    }
}

/// One shard: its batches by number.
type Shard = RwLock<BTreeMap<u64, Batch>>;

/// The batches in memory.
pub(super) struct Resident {
    shards: Vec<Shard>,
}

impl Resident {
    /// None yet.
    pub(super) fn new() -> Resident {
        Resident { shards: (0..SHARDS).map(|_| RwLock::new(BTreeMap::new())).collect() }
    }

    /// The shard of batch `number`.
    fn shard(&self, number: u64) -> Option<&Shard> {
        self.shards.get(usize::try_from(number & SHARD_MASK).unwrap_or(0))
    }

    /// The version at `index` of batch `number`, if memory holds it.
    pub(super) fn held(&self, number: u64, index: usize) -> Option<Held> {
        read_builtin(self.shard(number)?).get(&number)?.held(index)
    }

    /// Adds `version` to batch `number`, being gathered.
    pub(super) fn push(&self, number: u64, version: Arc<[u8]>) {
        if let Some(shard) = self.shard(number)
            && let Batch::Gathering(versions) = write_builtin(shard).entry(number).or_insert_with(|| Batch::Gathering(Vec::new()))
        {
            versions.push(version);
        }
    }

    /// Batch `number`, gathered, taken to be written: its versions, which
    /// stay readable while it is.
    pub(super) fn take(&self, number: u64) -> Versions {
        let Some(shard) = self.shard(number) else { return Versions::default() };
        let mut shard = write_builtin(shard);
        let batch = shard.entry(number).or_insert_with(|| Batch::Gathering(Vec::new()));
        let versions = match batch {
            Batch::Gathering(versions) => Arc::new(mem::take(versions)),
            Batch::Writing(versions) => Arc::clone(versions),
            Batch::Kept(_) => return Versions::default(),
        };
        *batch = Batch::Writing(Arc::clone(&versions));
        versions
    }

    /// Batch `number`, written: kept as `record`, or with none, gone from
    /// memory. What it was, to be dropped once no lock is held.
    pub(super) fn written(&self, number: u64, record: Option<Stored>) -> Option<Batch> {
        let mut shard = write_builtin(self.shard(number)?);
        match record {
            Some(record) => shard.insert(number, Batch::Kept(record)),
            None => shard.remove(&number),
        }
    }

    /// Batch `number` gone from memory: what it was, to be dropped once no
    /// lock is held.
    pub(super) fn remove(&self, number: u64) -> Option<Batch> {
        write_builtin(self.shard(number)?).remove(&number)
    }
}

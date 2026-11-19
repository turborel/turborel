//! \[EXTENSION\] Point-in-time recovery as crash recovery over a seeded log
//! (RULES.md D164). A restored database is opened with a log that serves
//! first the records of its backup and of the archive, each with the
//! position it had, up to the recovery target, then a new file log whose
//! first segment is numbered past them. Recovery reads it as it reads any
//! log, so a restore replays nothing in a way of its own. Transactions
//! committed before the target are present, the others undone, as after a
//! crash there.
//!
//! The seed is let go once a checkpoint truncates the log past it, so the
//! restored database's log is then its file log alone.

use std::sync::{Arc, Mutex};

use super::object::{ObjectIn, Reading};
use super::target::BackupTarget;
use crate::bytes::Reader;
use crate::error::Result;
use crate::log::Lsn;
use crate::log::record::marks::{commit_wall, point_name};
use crate::sync::lock_builtin;

/// \[EXTENSION\] Where recovery from a backup stops (RULES.md D164).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryTarget {
    /// After the last record there is.
    End,
    /// After the record at this position, or the last before it.
    Position(Lsn),
    /// Before the first commit after this instant, microseconds from
    /// 1970-01-01 00:00 UTC.
    Time(i64),
    /// After the restore point of this name.
    Point(String),
}

/// \[EXTENSION\] One object of records on the path to the target: its
/// target, its name, and the last position of its on the path, for a
/// timeline's ancestor (RULES.md D165).
#[derive(Clone)]
pub struct SeedPart {
    /// Where it is.
    pub target: Arc<dyn BackupTarget>,
    /// Its object: a backup's log, or an archived segment.
    pub object: String,
    /// The last position of it on the path, if not every one.
    pub upto: Option<Lsn>,
}

/// \[EXTENSION\] How far a seed's records went, as recovery read them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reached {
    /// The last record's position served.
    pub last: Lsn,
    /// Whether the target was met: a record past it, at it, or for `End`
    /// and `Time` the records' end.
    pub met: bool,
    /// The records served.
    pub records: u64,
}

/// \[EXTENSION\] The records a restored database is recovered with: see
/// the module's documentation.
pub struct Seed {
    parts: Vec<SeedPart>,
    reading: Reading,
    stop: RecoveryTarget,
    reached: Mutex<Reached>,
}

impl Seed {
    /// The seed of `parts`, in order, read with `reading`, stopped at
    /// `stop`.
    pub fn new(parts: Vec<SeedPart>, reading: Reading, stop: RecoveryTarget) -> Seed {
        Seed { parts, reading, stop, reached: Mutex::new(Reached::default()) }
    }

    /// How far its records went the last time they were read.
    pub fn reached(&self) -> Reached {
        *lock_builtin(&self.reached)
    }
}

/// A record of a backup's log or an archived segment: its position, then
/// its bytes.
fn record(payload: &[u8], object: &str) -> Result<(Lsn, Vec<u8>)> {
    let mut r = Reader::new(payload, "an archived record");
    let at = r.u64().map_err(|e| super::damaged(object, &e.msg))?;
    Ok((Lsn(at), r.rest().to_vec()))
}

/// What a record does to the reading of a seed.
enum Step {
    /// Served.
    Serve,
    /// Served, and the target met after it.
    ServeLast,
    /// Not served, as the target is met before it.
    Stop,
}

/// What record `bytes` at `at` is to target `stop`.
fn step(stop: &RecoveryTarget, at: Lsn, bytes: &[u8]) -> Step {
    match stop {
        RecoveryTarget::End => Step::Serve,
        RecoveryTarget::Position(p) => match at.cmp(p) {
            std::cmp::Ordering::Less => Step::Serve,
            std::cmp::Ordering::Equal => Step::ServeLast,
            std::cmp::Ordering::Greater => Step::Stop,
        },
        RecoveryTarget::Time(t) => {
            if commit_wall(bytes).is_some_and(|wall| wall > *t) {
                Step::Stop
            } else {
                Step::Serve
            }
        }
        RecoveryTarget::Point(name) => {
            if point_name(bytes).as_deref() == Some(name.as_str()) {
                Step::ServeLast
            } else {
                Step::Serve
            }
        }
    }
}

/// The records of a seed from `from` on.
pub(super) struct SeedRecords {
    seed: Arc<Seed>,
    from: Lsn,
    part: usize,
    current: Option<ObjectIn>,
    done: bool,
    reached: Reached,
}

impl SeedRecords {
    /// The records of `seed` from `from` on.
    pub(super) fn new(seed: Arc<Seed>, from: Lsn) -> SeedRecords {
        SeedRecords { seed, from, part: 0, current: None, done: false, reached: Reached::default() }
    }

    /// Ends the reading, the target met or not.
    fn end(&mut self, met: bool) {
        self.done = true;
        self.reached.met = met;
        *lock_builtin(&self.seed.reached) = self.reached;
    }

    /// The next record of the parts in order, after the last served, on
    /// the path: none at their end.
    fn next_record(&mut self) -> Result<Option<(Lsn, Vec<u8>)>> {
        loop {
            let Some(part) = self.seed.parts.get(self.part).cloned() else { return Ok(None) };
            if self.current.is_none() {
                self.current = Some(ObjectIn::open(&part.target, &part.object, &self.seed.reading)?);
            }
            let block = self.current.as_mut().map_or(Ok(None), ObjectIn::next)?;
            let Some(payload) = block else {
                self.current = None;
                self.part = self.part.saturating_add(1);
                continue;
            };
            let (at, bytes) = record(&payload, &part.object)?;
            if part.upto.is_some_and(|upto| at > upto) {
                self.current = None;
                self.part = self.part.saturating_add(1);
                continue;
            }
            if crate::backup::decide::serves(self.reached.records, at, self.reached.last) {
                return Ok(Some((at, bytes)));
            }
        }
    }
}

impl Iterator for SeedRecords {
    type Item = Result<(Lsn, Vec<u8>)>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.done {
            let (at, bytes) = match self.next_record() {
                Ok(Some(found)) => found,
                Ok(None) => {
                    let met = matches!(self.seed.stop, RecoveryTarget::End | RecoveryTarget::Time(_));
                    self.end(met);
                    return None;
                }
                Err(e) => {
                    self.end(false);
                    return Some(Err(e));
                }
            };
            let last = match step(&self.seed.stop, at, &bytes) {
                Step::Stop => {
                    self.end(true);
                    return None;
                }
                Step::Serve => false,
                Step::ServeLast => true,
            };
            self.reached.last = at;
            self.reached.records = self.reached.records.saturating_add(1);
            if last {
                self.end(true);
            }
            if at >= self.from {
                return Some(Ok((at, bytes)));
            }
        }
        None
    }
}

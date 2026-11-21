//! \[DESIGN\] Reading a file log's records in order, segment after segment
//! (`log/file.rs`): each record read into one buffer, which the iterator
//! copies out for each record it yields, and `FileLog::read_at` takes, as it
//! wants one record alone.

use std::sync::Arc;

use super::{FileLog, lsn, split};
use crate::disk::block::BlockId;
use crate::error::Result;
use crate::log::Lsn;
use crate::sync::lock_builtin;

/// Reads a file log's records in order, segment after segment.
pub(super) struct Reader<'a> {
    log: &'a FileLog,
    at: Lsn,
    payload: Vec<u8>,
}

impl<'a> Reader<'a> {
    /// A reader of `log`'s records at or after `at`.
    pub(super) fn new(log: &'a FileLog, at: Lsn) -> Reader<'a> {
        Reader { log, at, payload: Vec::new() }
    }

    /// Reads the next record into its buffer: where it is, if the log
    /// holds one more.
    pub(super) fn step(&mut self) -> Option<Result<Lsn>> {
        loop {
            let (number, id) = split(self.at);
            // The first segment kept at or after the position's.
            let segment = lock_builtin(&self.log.segments).range(number..).next().map(|(_, s)| Arc::clone(s))?;
            let id = if segment.number == number { id.max(segment.blocks.first()) } else { segment.blocks.first() };
            if id >= segment.blocks.end() {
                if self.log.segment(segment.number.saturating_add(1)).is_none() {
                    self.at = lsn(segment.number, id);
                    return None;
                }
                self.at = lsn(segment.number.saturating_add(1), BlockId(0));
                continue;
            }
            return Some(match segment.blocks.read_into(id, &mut self.payload) {
                Ok(next) => {
                    self.at = lsn(segment.number, next);
                    Ok(lsn(segment.number, id))
                }
                Err(e) => {
                    // Stop after an error rather than repeat it.
                    self.at = Lsn(u64::MAX);
                    Err(e)
                }
            });
        }
    }

    /// The record last read, taken from its buffer.
    pub(super) fn taken(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.payload)
    }
}

impl Iterator for Reader<'_> {
    type Item = Result<(Lsn, Vec<u8>)>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.step()?.map(|at| (at, self.payload.clone())))
    }
}

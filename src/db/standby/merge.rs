//! \[DESIGN\] The records a following database replays at once, in
//! position order (RULES.md D57): those of one call to `Standby::replay`
//! that replay as they come, and those held of each transaction they end,
//! each already in position order, merged as they are read, so a
//! transaction's records spilled (`txn/followed.rs`) are read back a
//! segment at a time, never all at once.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::error::{Result, SqlError};
use crate::log::Lsn;
use crate::log::record::Record;

/// Records in position order, or the error reading them.
pub(super) type Stream<'a> = Box<dyn Iterator<Item = Result<(Lsn, Record)>> + 'a>;

/// \[DESIGN\] Streams in position order, merged: the record of least
/// position each time; an error of any stream once met, and then nothing.
pub(super) struct Merged<'a> {
    streams: Vec<Stream<'a>>,
    /// The record each stream gave next, not yet taken.
    heads: Vec<Option<Record>>,
    /// The positions of the heads, the least first, with their streams.
    order: BinaryHeap<Reverse<(Lsn, usize)>>,
    failed: Option<SqlError>,
}

impl<'a> Merged<'a> {
    /// `streams`, merged.
    pub(super) fn new(streams: Vec<Stream<'a>>) -> Merged<'a> {
        let heads = streams.iter().map(|_| None).collect();
        let mut merged = Merged { streams, heads, order: BinaryHeap::new(), failed: None };
        for at in 0..merged.streams.len() {
            merged.advance(at);
        }
        merged
    }

    /// Takes the next record of stream `at`, if it has one.
    fn advance(&mut self, at: usize) {
        let Some(stream) = self.streams.get_mut(at) else { return };
        match stream.next() {
            Some(Ok((lsn, record))) => {
                if let Some(head) = self.heads.get_mut(at) {
                    *head = Some(record);
                    self.order.push(Reverse((lsn, at)));
                }
            }
            Some(Err(e)) => {
                self.failed.get_or_insert(e);
            }
            None => {}
        }
    }
}

impl Iterator for Merged<'_> {
    type Item = Result<(Lsn, Record)>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(e) = self.failed.take() {
            self.order.clear();
            return Some(Err(e));
        }
        let Reverse((lsn, at)) = self.order.pop()?;
        let record = self.heads.get_mut(at).and_then(Option::take)?;
        self.advance(at);
        Some(Ok((lsn, record)))
    }
}

// Unit tests: the order merged, an error met (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;
    use crate::log::record::Change;

    /// Records at `positions`, of transaction `tx`, as a stream.
    fn stream(tx: u64, positions: &[u64]) -> Stream<'static> {
        let records: Vec<_> = positions.iter().map(|&p| Ok((Lsn(p), Record { tx, change: Change::Abort }))).collect();
        Box::new(records.into_iter())
    }

    /// Verifies: [DESIGN] RULES.md D57: streams each in position order are
    /// merged into one, the least position first, each record once, of
    /// the stream it came from; an empty stream gives nothing.
    #[test]
    fn streams_merge_in_position_order() {
        let merged =
            Merged::new(vec![stream(1_u64, &[2_u64, 5_u64, 9_u64]), stream(2_u64, &[]), stream(3_u64, &[1_u64, 3_u64, 4_u64, 10_u64])]);
        let taken: Vec<(u64, u64)> = merged.map(|r| r.map(|(at, record)| (at.0, record.tx)).unwrap()).collect();
        let expected: Vec<(u64, u64)> = vec![(1, 3), (2, 1), (3, 3), (4, 3), (5, 1), (9, 1), (10, 3)];
        assert_eq!(taken, expected);
    }

    /// Verifies: [DESIGN] RULES.md D57: a stream's error is given as it is
    /// met, and nothing after it.
    #[test]
    fn an_error_ends_the_merge() {
        let failing: Stream<'static> = Box::new(
            vec![Ok((Lsn(2_u64), Record { tx: 1_u64, change: Change::Abort })), Err(SqlError::new(Code::Corrupt, "broken"))].into_iter(),
        );
        let mut merged = Merged::new(vec![failing, stream(2_u64, &[1_u64, 3_u64])]);
        assert_eq!(merged.next().map(|r| r.map(|(at, _)| at.0).unwrap()), Some(1_u64));
        assert_eq!(merged.next().map(|r| r.map(|(at, _)| at.0).unwrap()), Some(2_u64));
        assert_eq!(merged.next().map(|r| r.map(|_| ()).unwrap_err().code), Some(Code::Corrupt));
        assert!(merged.next().is_none());
    }
}

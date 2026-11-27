//! \[DESIGN\] The bytes of a log whose positions number its records, not
//! its bytes (RULES.md D1231): what such a log answers
//! `Log::bytes_between` from. Each record kept has a mark, the bytes
//! appended before it, set as it is appended under the log's own lock;
//! the distance between two positions is the difference of their marks,
//! found at once, whatever the records between.

use std::collections::VecDeque;

/// \[DESIGN\] The bytes appended before each record a log keeps, by the
/// record's place among them: the first kept at 0.
#[derive(Debug, Default, Clone)]
pub struct ByteMarks {
    /// The bytes of the records let go from the front.
    gone: u64,
    /// The bytes appended through each record kept, in order.
    through: VecDeque<u64>,
}

impl ByteMarks {
    /// The marks of `records`, kept in order.
    pub fn of<'r>(records: impl IntoIterator<Item = &'r [u8]>) -> ByteMarks {
        let mut marks = ByteMarks::default();
        records.into_iter().for_each(|record| marks.push(record.len()));
        marks
    }

    /// A record of `len` bytes appended after the last kept.
    pub fn push(&mut self, len: usize) {
        let total = self.before(self.through.len());
        self.through.push_back(total.saturating_add(u64::try_from(len).unwrap_or(u64::MAX)));
    }

    /// Every record from place `kept` on taken back: the next appended
    /// follows the one before it.
    pub fn truncate(&mut self, kept: usize) {
        self.through.truncate(kept);
    }

    /// The first `gone` records let go: the one after them is at place 0.
    pub fn drain_front(&mut self, gone: usize) {
        let gone = gone.min(self.through.len());
        self.gone = self.before(gone);
        self.through.drain(..gone);
    }

    /// The bytes appended before the record at place `at`: every record's
    /// if `at` is past the last.
    pub fn before(&self, at: usize) -> u64 {
        let at = at.min(self.through.len());
        at.checked_sub(1).and_then(|last| self.through.get(last)).copied().unwrap_or(self.gone)
    }

    /// The bytes of the records from place `from` to place `to`, none if
    /// `to` comes first.
    pub fn between(&self, from: usize, to: usize) -> u64 {
        self.before(to).saturating_sub(self.before(from))
    }

    /// How many records are kept.
    pub fn len(&self) -> usize {
        self.through.len()
    }

    /// Whether none is kept.
    pub fn is_empty(&self) -> bool {
        self.through.is_empty()
    }
}

/// The place among records kept from position `first` on of position `at`:
/// 0 for one let go before them.
pub fn place(first: u64, at: u64) -> usize {
    usize::try_from(at.saturating_sub(first)).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_tell_the_bytes_between_places_through_appends_truncations_and_drains() {
        let mut marks = ByteMarks::of([&b"abc"[..], b"de", b"f"]);
        assert_eq!((marks.between(0, 3), marks.between(1, 2), marks.between(2, 1)), (6, 2, 0));
        assert_eq!(marks.between(1, 99), 3, "a place past the last is the end");
        marks.truncate(1);
        marks.push(10);
        assert_eq!((marks.len(), marks.between(0, 2)), (2, 13), "the record taken back no longer counts");
        marks.drain_front(1);
        assert_eq!((marks.before(0), marks.between(0, 1)), (3, 10), "a drained record's bytes stay before the first");
        marks.drain_front(5);
        assert!(marks.is_empty());
        marks.push(4);
        assert_eq!((marks.before(0), marks.before(1)), (13, 17), "appends after all were let go count on");
    }
}

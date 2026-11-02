//! \[EXTENSION\] A replica's log as a window (RULES.md D57): the records
//! after `first`, those through it let go by log retention, each held by
//! the replica's database durably, in its files, so the log need not hold
//! it; the window's last position is the op-number. Split from vr.rs
//! (RULES.md R-SIZE-2).

/// \[EXTENSION\] The records of a log after a position.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Window {
    /// The position before the first record held: every position through
    /// it was let go.
    first: u64,
    /// The records, from position `first + 1`.
    records: Vec<Vec<u8>>,
}

impl Window {
    /// The records `records` after `first`.
    pub(crate) fn new(first: u64, records: Vec<Vec<u8>>) -> Window {
        Window { first, records }
    }

    /// The position before the first record held.
    pub(crate) fn first(&self) -> u64 {
        self.first
    }

    /// The op-number: the last position held, `first` if none is.
    pub(crate) fn op(&self) -> u64 {
        self.first.saturating_add(u64::try_from(self.records.len()).unwrap_or(u64::MAX))
    }

    /// The record at position `at`, if the window holds it.
    pub(crate) fn record(&self, at: u64) -> Option<&Vec<u8>> {
        let index = at.checked_sub(self.first)?.checked_sub(1)?;
        self.records.get(usize::try_from(index).ok()?)
    }

    /// The records after `after`, through the op-number: all of them if
    /// `after` is before the window.
    pub(crate) fn after(&self, after: u64) -> Vec<Vec<u8>> {
        let skip = usize::try_from(after.saturating_sub(self.first)).unwrap_or(usize::MAX);
        self.records.get(skip..).unwrap_or_default().to_vec()
    }

    /// Adds `record` at the position after the op-number.
    pub(crate) fn push(&mut self, record: Vec<u8>) {
        self.records.push(record);
    }

    /// Keeps the records through `op`, letting the later ones go.
    pub(crate) fn truncate_to(&mut self, op: u64) {
        let kept = usize::try_from(op.saturating_sub(self.first)).unwrap_or(usize::MAX);
        self.records.truncate(kept);
    }

    /// Lets the records through `through` go, if any is held: the window
    /// then begins after `through`, even past the records it held, which
    /// are all let go.
    pub(crate) fn retain_after(&mut self, through: u64) {
        if through <= self.first {
            return;
        }
        let gone = usize::try_from(through.saturating_sub(self.first)).unwrap_or(usize::MAX).min(self.records.len());
        self.records.drain(..gone);
        self.first = through;
    }

    /// How many records of `theirs`, after `after`, agree with those held
    /// at the same positions: the length of the common prefix of the two
    /// from `after` on, as far as both hold records.
    pub(crate) fn agreeing(&self, after: u64, theirs: &[Vec<u8>]) -> usize {
        theirs
            .iter()
            .enumerate()
            .take_while(|(i, record)| {
                self.record(after.saturating_add(u64::try_from(*i).unwrap_or(u64::MAX)).saturating_add(1)) == Some(record)
            })
            .count()
    }
}

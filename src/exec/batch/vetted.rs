//! \[DESIGN\] (RULES.md D1266) What a scan judged of each batch
//! (`storage::Vetted`), taken from the scan's own stream to the first
//! filter over it and to no other operator. The rows the scan kept of a
//! batch travel as the rows a filter before kept do, beside the batch. The
//! conjuncts it judged exactly are said to a `Said` that only that scan's
//! stream and that filter hold: the filter passes over those conjuncts
//! (`judge/vetted.rs`).
//!
//! Between a scan and its filter a stream may count rows or judge runtime
//! filters, each a batch in and the same batch out. What is said is of
//! the batch the scan gave last, so it is heard only while the filter
//! judges each batch before the scan gives the next, the batch it is
//! handed being as many as the scan has given: were a batch ever held
//! back, given twice or made of two, the counts part, and from then on
//! nothing is heard and every conjunct is judged.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::stream::Batches;
use crate::logical::Cond;
use crate::storage::{Conjunction, Sel, Split, TableStorage, Vetted, VettedBatches};
use crate::txn::TxId;

/// \[DESIGN\] (RULES.md D1266) What a scan says of the batch it gave last,
/// for the first filter over it to hear. A stream is read by one thread
/// at a time, so the counts are read and written in turn, in whatever
/// order the memory allows.
#[derive(Debug, Default)]
pub(in crate::exec) struct Said {
    /// How many batches the scan has given.
    given: AtomicU64,
    /// How many the filter has judged.
    judged: AtomicU64,
    /// The told conjuncts judged exactly of the batch given last
    /// (`Vetted::exact`).
    exact: AtomicU64,
    /// The told conjunct judged of that batch's first rows alone, by its
    /// bit, no bit if none was, and the place of the first row it did
    /// not judge (`Vetted::partly`).
    partly: AtomicU64,
    from: AtomicU32,
    /// Whether the filter was ever handed a batch that was not the one
    /// the scan gave last.
    parted: AtomicBool,
}

/// \[DESIGN\] (RULES.md D1266) The filter a scan is read under: its
/// condition, whose conjuncts the scan is told, and what the scan says to
/// it of each batch.
pub(super) type Filtering<'a> = (&'a Cond, &'a Arc<Said>);

/// \[DESIGN\] (RULES.md D1266) What a filter hears of the batch it is
/// handed: the told conjuncts its scan judged exactly of every row kept,
/// and the one it judged of the rows before a place alone, by its bit, no
/// bit if none was (`storage::Vetted`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::exec) struct Heard {
    pub(super) exact: u64,
    pub(super) partly: (u64, u32),
}

impl Said {
    /// The scan gives a batch of which it judged what `vetted` says: what
    /// it said of the batch before is said no more.
    pub(super) fn say(&self, vetted: &Vetted) {
        let (partly, from) = vetted.partly.unwrap_or((0, 0));
        self.given.fetch_add(1, Ordering::Relaxed);
        self.exact.store(vetted.exact, Ordering::Relaxed);
        self.partly.store(partly, Ordering::Relaxed);
        self.from.store(from, Ordering::Relaxed);
    }

    /// What the scan judged of the batch the filter is handed now:
    /// nothing if it is not the batch the scan gave last, nor ever after
    /// one that was not.
    pub(super) fn heard(&self) -> Heard {
        let judged = self.judged.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        if judged != self.given.load(Ordering::Relaxed) {
            self.parted.store(true, Ordering::Relaxed);
        }
        if self.parted.load(Ordering::Relaxed) {
            return Heard::default();
        }
        let partly = (self.partly.load(Ordering::Relaxed), self.from.load(Ordering::Relaxed));
        Heard { exact: self.exact.load(Ordering::Relaxed), partly }
    }
}

impl Heard {
    /// Whether the filter's chain of conjuncts passes over the one of
    /// bit `told`: the scan judged it exactly of every row kept, or of
    /// the first of them, the rest then judged apart
    /// (`Judged::rest_kept`).
    pub(super) fn passes(self, told: u64) -> bool {
        told & (self.exact | self.partly.0) != 0
    }
}

/// `read`'s batches, each with the rows its scan kept of it as the rows a
/// filter before kept, what the scan judged exactly of it said to `said`.
fn said(read: VettedBatches, said: &Arc<Said>) -> Batches {
    let said = Arc::clone(said);
    Box::new(read.map(move |vetted| {
        let (batch, vetted) = vetted?;
        let vetted = vetted.unwrap_or_default();
        said.say(&vetted);
        Ok((batch, vetted.rows.map(Sel::of)))
    }))
}

/// The batches of a scan of `columns` of `storage` told `told`, those of
/// `late` late: for a filter whose conjuncts it was told, which hears
/// `to`, each with what the scan judged of it
/// (`TableStorage::scan_vetted`); for any other reader, as
/// `TableStorage::scan_batches` gives them, every row of each kept.
pub(super) fn scanned(
    storage: Arc<dyn TableStorage>,
    tx: TxId,
    (columns, told, late): (&[usize], Option<&Conjunction>, &[usize]),
    to: Option<&Arc<Said>>,
) -> Batches {
    match to {
        Some(to) => said(storage.scan_vetted(tx, columns, told, late), to),
        None => Box::new(storage.scan_batches(tx, columns, told, late).map(|b| b.map(|b| (b, None)))),
    }
}

/// As `scanned`, of morsel `n` of `split` (`Split::morsel_vetted`,
/// `Split::morsel_batches`): none if its engine reads no morsel by batch.
pub(in crate::exec) fn morsel(split: &dyn Split, n: usize, late: &[usize], to: Option<&Arc<Said>>) -> Option<Batches> {
    match to {
        Some(to) => Some(said(split.morsel_vetted(n, late)?, to)),
        None => Some(Box::new(split.morsel_batches(n, late)?.map(|b| b.map(|b| (b, None))))),
    }
}

// Unit tests: what is heard of which batch (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{Batch, Vetted};

    /// A scan's batches of `rows` rows each, of each what `each` says.
    fn read(each: Vec<Option<Vetted>>, rows: usize) -> VettedBatches {
        Box::new(each.into_iter().map(move |vetted| Ok((Batch { columns: Vec::new(), rows }, vetted))))
    }

    /// Verifies: [DESIGN] RULES.md D1266 what a scan judged of a batch,
    /// exactly and of its first rows alone, is heard of that batch
    /// alone. A batch it said nothing of,
    /// as one of the tree's rows is, is heard as nothing though the batch
    /// before it had conjuncts judged exactly, and the rows the scan kept
    /// of a batch come beside it as the rows kept.
    #[test]
    fn what_is_said_of_a_batch_is_heard_of_it_alone() {
        let to = Arc::new(Said::default());
        let first = Vetted { rows: Some(vec![1, 3]), exact: 0b101, partly: Some((0b010, 3)) };
        let each = vec![Some(first), None, Some(Vetted { rows: None, exact: 0b010, partly: None })];
        let mut batches = said(read(each, 4), &to);
        let (_, kept) = batches.next().unwrap().unwrap();
        assert_eq!(kept.map(|r| r.as_slice().to_vec()), Some(vec![1, 3]), "the rows the scan kept");
        assert_eq!(to.heard(), Heard { exact: 0b101, partly: (0b010, 3) });
        assert!(batches.next().unwrap().unwrap().1.is_none(), "every row of a batch nothing is said of");
        assert_eq!(to.heard(), Heard::default(), "nothing is said of the tree's rows, and the last batch's word is gone");
        assert!(batches.next().unwrap().unwrap().1.is_none());
        assert_eq!(to.heard(), Heard { exact: 0b010, partly: (0, 0) });
        assert!(batches.next().is_none());
    }

    /// Verifies: [DESIGN] RULES.md D1266 the filter hears nothing of a
    /// batch that is not the one the scan gave last, and nothing ever
    /// after. The two conditions are that the filter has judged as many
    /// batches as the scan gave (false of a batch held back, which is
    /// then judged under the next one's word, and of one judged twice),
    /// and that no batch before parted the counts (false after either,
    /// though the counts meet again).
    #[test]
    fn nothing_is_heard_once_a_batch_is_not_the_scans_last() {
        let vetted = |exact| Some(Vetted { rows: None, exact, partly: Some((8, 1)) });
        let nothing = Heard::default();
        let held_back = Arc::new(Said::default());
        let mut batches = said(read(vec![vetted(1), vetted(2), vetted(4)], 1), &held_back);
        assert!(batches.next().is_some() && batches.next().is_some(), "two given before one is judged");
        assert_eq!(held_back.heard(), nothing, "the first judged under the second's word");
        assert_eq!(held_back.heard(), nothing, "the counts meet again, and still nothing is heard");
        assert!(batches.next().is_some());
        assert_eq!(held_back.heard(), nothing, "nor of any batch after");
        let twice = Arc::new(Said::default());
        let mut again = said(read(vec![vetted(1), vetted(2)], 1), &twice);
        assert!(again.next().is_some());
        assert_eq!(twice.heard(), Heard { exact: 1, partly: (8, 1) });
        assert_eq!(twice.heard(), nothing, "a batch judged again is not the scan's last");
        assert!(again.next().is_some());
        assert_eq!(twice.heard(), nothing, "nor is any after it");
    }
}

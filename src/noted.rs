//! \[EXTENSION\] What EXPLAIN ANALYZE notes of an operator beside its rows
//! (RULES.md D620, D621): what it spilled, the batches it gave, and what
//! it read of indexes. As a counted operator is opened or pulled, its
//! notes are the thread's (`within`), so the work done below the pull is
//! noted of the innermost operator being pulled, an input's own pulls
//! noting theirs. What notes the work, a spill area, a tree's scan or an
//! index scan's reading, takes the thread's notes once as it is made
//! (`noting`) and adds to them as it works: made with none, as everything
//! is of a statement not analyzed, it notes nothing, and the thread's
//! notes are read once each made, never a row at a time.
//!
//! A part run in parallel takes the notes of the thread that starts it to
//! each of its workers (`exec/parallel/run.rs`).

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// \[EXTENSION\] What one operator did beside giving rows, added to by
/// each thread working for it.
#[derive(Debug, Default)]
pub struct Notes {
    batches: AtomicU64,
    spilled: AtomicU64,
    partitions: AtomicU64,
    passes: AtomicU64,
    entries: AtomicU64,
    leaves: AtomicU64,
    fetched: AtomicU64,
    rechecked: AtomicU64,
    side: AtomicU64,
}

/// \[EXTENSION\] What an operator's notes count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Noted {
    /// The batches it gave.
    Batches,
    /// The bytes of the records it wrote to its spill areas.
    Spilled,
    /// The tables and runs it made in its spill areas.
    Partitions,
    /// The times it turned from writing to its spill areas to reading
    /// back what it wrote.
    Passes,
    /// The entries indexes gave it.
    Entries,
    /// The leaf pages of trees it read.
    Leaves,
    /// The rows it read from a table for an index's entries.
    Fetched,
    /// The rows whose condition it judged again of the row read, an
    /// index's entry not deciding it.
    Rechecked,
    /// The groups its first morsel made by the columns of the grouping
    /// over it, of the first grouping of a pair that keeps its plain set
    /// functions by those columns (RULES.md D1324).
    Side,
}

/// \[EXTENSION\] What an operator's notes held once it ran: each count of
/// `Noted`, for a tool to read (`Operator::notes`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NotesRead {
    /// The batches it gave.
    pub batches: u64,
    /// The bytes it spilled.
    pub spilled: u64,
    /// The tables and runs it spilled to.
    pub partitions: u64,
    /// The times it read back what it spilled.
    pub passes: u64,
    /// The index entries it read.
    pub entries: u64,
    /// The leaf pages it read.
    pub leaves: u64,
    /// The rows it fetched for index entries.
    pub fetched: u64,
    /// The rows it rechecked.
    pub rechecked: u64,
    /// The groups of its first morsel's side, of a pair's first grouping
    /// that keeps one.
    pub side: u64,
}

impl Notes {
    /// Adds `n` to what it counts of `what`.
    pub fn add(&self, what: Noted, n: u64) {
        let count = match what {
            Noted::Batches => &self.batches,
            Noted::Spilled => &self.spilled,
            Noted::Partitions => &self.partitions,
            Noted::Passes => &self.passes,
            Noted::Entries => &self.entries,
            Noted::Leaves => &self.leaves,
            Noted::Fetched => &self.fetched,
            Noted::Rechecked => &self.rechecked,
            Noted::Side => &self.side,
        };
        count.fetch_add(n, Ordering::Relaxed);
    }

    /// What it holds now.
    pub fn read(&self) -> NotesRead {
        let at = |c: &AtomicU64| c.load(Ordering::Relaxed);
        NotesRead {
            batches: at(&self.batches),
            spilled: at(&self.spilled),
            partitions: at(&self.partitions),
            passes: at(&self.passes),
            entries: at(&self.entries),
            leaves: at(&self.leaves),
            fetched: at(&self.fetched),
            rechecked: at(&self.rechecked),
            side: at(&self.side),
        }
    }
}

thread_local! {
    /// The notes of the operator this thread pulls, if it is counted.
    static NOTING: RefCell<Option<Arc<Notes>>> = const { RefCell::new(None) };
}

/// The notes of the operator this thread pulls, if it is counted: taken
/// once by what is made to work for it.
pub(crate) fn noting() -> Option<Arc<Notes>> {
    NOTING.try_with(|n| n.borrow().clone()).ok().flatten()
}

/// `f`, run with `notes` the thread's, those before put back after, as
/// they are if `f` panics.
pub(crate) fn within<R>(notes: Option<Arc<Notes>>, f: impl FnOnce() -> R) -> R {
    let before = NOTING.try_with(|n| n.replace(notes)).ok().flatten();
    let _back = Back(before);
    f()
}

/// The notes to put back as the thread's once dropped.
struct Back(Option<Arc<Notes>>);

impl Drop for Back {
    fn drop(&mut self) {
        let before = self.0.take();
        let _ = NOTING.try_with(|n| n.replace(before));
    }
}

/// Adds `n` to `notes`' count of `what`, if there are notes.
pub(crate) fn add(notes: Option<&Notes>, what: Noted, n: usize) {
    if let Some(notes) = notes {
        notes.add(what, u64::try_from(n).unwrap_or(u64::MAX));
    }
}

// Unit tests: whose notes a thread's work adds to (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] RULES.md D620: a thread notes of the notes it
    /// runs within, the innermost while it runs, those before put back
    /// after, and nothing of none.
    #[test]
    fn work_notes_of_the_innermost_operator_pulled() {
        let (outer, inner) = (Arc::new(Notes::default()), Arc::new(Notes::default()));
        assert!(noting().is_none());
        within(Some(Arc::clone(&outer)), || {
            add(noting().as_deref(), Noted::Spilled, 10);
            within(Some(Arc::clone(&inner)), || add(noting().as_deref(), Noted::Entries, 3));
            add(noting().as_deref(), Noted::Passes, 1);
        });
        add(noting().as_deref(), Noted::Spilled, 99);
        assert_eq!(outer.read(), NotesRead { spilled: 10, passes: 1, ..NotesRead::default() });
        assert_eq!(inner.read(), NotesRead { entries: 3, ..NotesRead::default() });
        assert!(noting().is_none(), "none after");
    }
}

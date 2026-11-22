//! \[DESIGN\] The bindings a session made on a thread, counted so that a
//! statement bound at each execution is told from one whose binding is
//! kept (RULES.md D793). A binding is one statement, query or change bound
//! against the catalog (`Session::bound`); a count on the thread costs it
//! no shared write.

use std::cell::Cell;

std::thread_local! {
    static MADE: Cell<u64> = const { Cell::new(0) };
}

/// One binding more made on this thread.
pub(crate) fn counted() {
    MADE.with(|made| made.set(made.get().wrapping_add(1)));
}

/// The bindings this thread made since it began.
pub fn bindings() -> u64 {
    MADE.with(Cell::get)
}

// Unit tests: the count, which no other API shows (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D793: each binding counts once, on the
    /// thread that made it, and another thread's count starts at none.
    #[test]
    fn each_binding_counts_once_on_its_thread() {
        let before = bindings();
        counted();
        counted();
        assert_eq!(bindings().wrapping_sub(before), 2);
        let other = std::thread::spawn(bindings).join().unwrap();
        assert_eq!(other, 0);
    }
}

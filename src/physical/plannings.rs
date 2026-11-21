//! \[DESIGN\] The plannings made on a thread, counted so that a statement
//! planned at each execution is told from one whose plan is kept (RULES.md
//! D613). A planning is one physical plan chosen for a statement or a
//! subquery; a count on the thread costs it no shared write.

use std::cell::Cell;

std::thread_local! {
    static MADE: Cell<u64> = const { Cell::new(0) };
}

/// One planning more made on this thread.
pub(super) fn counted() {
    MADE.with(|made| made.set(made.get().wrapping_add(1)));
}

/// The plannings this thread made since it began.
pub fn plannings() -> u64 {
    MADE.with(Cell::get)
}

// Unit tests: the count, which no other API shows (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_planning_counts_once_on_its_thread() {
        let before = plannings();
        counted();
        counted();
        assert_eq!(plannings().wrapping_sub(before), 2);
        let other = std::thread::spawn(plannings).join().unwrap();
        assert_eq!(other, 0);
    }
}

//! Unit tests: what a program knows, asked once and kept until stale
//! (RULES.md R-TEST-1).

use std::cell::Cell;

use super::*;

/// Verifies: [EXTENSION] RULES.md D278: an answer is asked once and kept;
/// asked again once the writes reach the count it gave, and at once when
/// refreshed; a call that fails is kept as none until then; an answer
/// that says no count is never stale.
#[test]
fn an_answer_is_kept_until_stale() {
    let asked = Cell::new(0_u32);
    let ask = |refresh: u64| {
        asked.set(asked.get() + 1);
        Ok(Some((asked.get(), refresh)))
    };
    let known: Known<u32> = Known::default();
    assert!(known.due(), "none asked yet");
    assert_eq!(known.get(|| ask(3)).as_deref(), Some(&1));
    assert_eq!(known.get(|| ask(3)).as_deref(), Some(&1), "kept");
    known.written(2);
    assert_eq!(known.get(|| ask(3)).as_deref(), Some(&1), "not yet stale");
    known.written(1);
    assert_eq!(known.get(|| ask(0)).as_deref(), Some(&2), "asked again");
    known.written(1000);
    assert_eq!(known.get(|| ask(0)).as_deref(), Some(&2), "0 is never stale");
    assert_eq!(known.refresh(|| ask(0)).as_deref(), Some(&3), "refreshed");
    assert_eq!(known.refresh(|| Err(crate::plugin::errors::violation("down"))), None);
    assert_eq!(known.get(|| ask(0)), None, "a failure kept as none");
    assert_eq!(asked.get(), 3);
}

//! \[DESIGN\] Whether a connection's user is still a user, as it was last
//! asked (RULES.md D815). Asking reads the database's snapshot, which
//! every worker shares; the answer holds until the database publishes
//! another snapshot, as dropping a user does, so it is asked again only
//! once the count of publications moved.

use std::sync::atomic::{AtomicU64, Ordering};

/// The answer last given, and the count of publications it was asked at:
/// that count plus one, shifted left once, with the answer in the low bit;
/// 0 while it was never asked.
#[derive(Default)]
pub(super) struct UserSeen(AtomicU64);

impl UserSeen {
    /// Whether the user is a user, the database having published `count`
    /// snapshots: the answer kept if it was asked at that count, else
    /// `ask`ed and kept.
    pub(super) fn is(&self, count: u64, ask: impl FnOnce() -> bool) -> bool {
        let at = count.saturating_add(1);
        let seen = self.0.load(Ordering::Relaxed);
        if seen >> 1_u32 == at {
            return seen & 1 == 1;
        }
        let is = ask();
        self.0.store((at << 1_u32) | u64::from(is), Ordering::Relaxed);
        is
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// Verifies: [DESIGN] RULES.md D815: whether a connection's user is a
    /// user is asked once at each count of publications, the answer kept
    /// until the count moves, then asked again: a user dropped meanwhile
    /// is told. MC/DC of keeping the answer: asked at this count or not.
    #[test]
    fn a_user_is_asked_again_only_once_a_snapshot_is_published() {
        let (seen, asked) = (UserSeen::default(), Cell::new(0_u32));
        let counted = &asked;
        let ask = |answer: bool| {
            move || {
                counted.set(counted.get() + 1);
                answer
            }
        };
        assert!(seen.is(0, ask(true)), "asked first");
        assert!(seen.is(0, ask(false)), "kept at the same count");
        assert_eq!(asked.get(), 1);
        assert!(!seen.is(1, ask(false)), "asked again once one was published");
        assert!(!seen.is(1, ask(true)), "kept again");
        assert_eq!(asked.get(), 2);
    }
}

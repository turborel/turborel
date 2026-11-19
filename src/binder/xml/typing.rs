//! Whether the definitions a statement makes are typed statically
//! [XQuery §5.2.3] (RULES.md D106): as XQUERY.STATIC_TYPING was for the
//! session that ran the statement, which the state the definitions are
//! made in holds no value of, as it holds none of a session's settings.
//! The statement's value is logged with it, so a replay makes the
//! definitions alike; the definition keeps it (`Horizon::static_typing`)
//! and is typed so whenever it is bound again.
//!
//! \[DESIGN\] As `parser/stored.rs` does for SQL.FUNCTION_CALLS (RULES.md
//! D74), the value is held by the thread making the definitions, while a
//! `MakingTyped` is held.

use std::cell::Cell;

std::thread_local! {
    /// Whether the definitions this thread makes are typed statically, if
    /// it is making a statement's.
    static MAKING: Cell<Option<bool>> = const { Cell::new(None) };
}

/// While held, the definitions this thread makes are typed statically as
/// it says; dropped, as before it was held.
#[must_use]
pub(crate) struct MakingTyped(Option<bool>);

impl Drop for MakingTyped {
    fn drop(&mut self) {
        MAKING.set(self.0);
    }
}

/// The definitions this thread makes, typed statically if `typed`, until
/// what this returns is dropped.
pub(crate) fn making_typed(typed: bool) -> MakingTyped {
    MakingTyped(MAKING.replace(Some(typed)))
}

/// Whether the definitions this thread makes are typed statically, as
/// logged with the statement that makes them.
pub(crate) fn making_typed_now() -> bool {
    MAKING.get() == Some(true)
}

/// Whether the definitions this thread makes are typed statically, if it
/// is making a statement's.
pub(super) fn making() -> Option<bool> {
    MAKING.get()
}

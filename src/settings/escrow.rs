//! \[DESIGN\] Whether a searched UPDATE that only adds to columns reserves
//! its increments rather than writing its rows (RULES.md D452,
//! `db/escrow.rs`).

use super::builtin::{SESSION, defined, text};
use super::{Definition, Key, Kind, ON, SWITCHES};

/// TRANSACTION.ESCROW, at its key's place after the others.
pub const TRANSACTION_ESCROW: Key = Key(93);

/// TRANSACTION.ESCROW's definition.
pub(super) fn escrow() -> Definition {
    defined(
        ("TRANSACTION.ESCROW", Kind::Choice(SWITCHES), text(ON)),
        SESSION,
        "Whether a READ COMMITTED searched UPDATE whose every SET item adds to or subtracts from an exact numeric column, no trigger, constraint, index or generated column depending on it, reserves its increments, applied as its transaction commits, rather than holding its rows until then: ON; or writes its rows as any UPDATE does, OFF, as to compare the two.",
    )
}

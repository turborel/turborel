//! \[EXTENSION\] How long a session a debugger paused waits for its word
//! before it goes on by itself (RULES.md D1011, `debugger.rs`).

use super::builtin::{DATABASE, at_least, defined, whole};
use super::{Definition, Key};

/// DEBUGGER.PAUSE_TIMEOUT_MS, at its key's place after the compiled
/// procedures'.
pub const DEBUGGER_PAUSE_TIMEOUT_MS: Key = Key(97);

/// \[DESIGN\] Five minutes: long enough to read a paused routine's frames
/// and think, short enough that the locks it holds are not held for long
/// by a debugger gone quiet.
const FIVE_MINUTES_MS: i128 = 300_000;

/// DEBUGGER.PAUSE_TIMEOUT_MS's definition.
pub(super) fn pause_timeout() -> Definition {
    defined(
        ("DEBUGGER.PAUSE_TIMEOUT_MS", at_least(0), whole(FIVE_MINUTES_MS)),
        DATABASE,
        "The milliseconds a session paused by a debugger waits for the debugger to tell it to go on, after which it goes on by itself to its next breakpoint and the debugger is told it timed out, so no lock it holds is held for ever; 0 to go on at once.",
    )
}

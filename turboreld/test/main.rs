//! Integration tests of `turboreld`: its command line, and the program run
//! as a process, its clients connecting over loopback TCP or a Unix socket.
//! One test binary (RULES.md D5), one module per area.
//! Every test traces to its requirements with a `/// Verifies:` line.
//!
//! A panic is how a test reports failure, so test code may unwrap, index and
//! panic (RULES.md R-ERR-1 applies to production code only).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_in_result,
    clippy::panic_in_result_fn,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    missing_docs,
    reason = "test code: a panic is a test failure (RULES.md R-TEST-4)"
)]
#![allow(
    clippy::default_numeric_fallback,
    clippy::shadow_unrelated,
    clippy::wildcard_enum_match_arm,
    clippy::cognitive_complexity,
    reason = "test code: literal fixtures, rebinding a session after drop, plug-in fixtures that \
              handle one variant, and flat lists of assertions (RULES.md R-TEST-4)"
)]

mod program;
mod users;

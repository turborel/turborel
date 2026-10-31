//! Integration tests of replication (RULES.md D57), through the crate's
//! public API and the fixtures of `turborel-test-support`. One test binary
//! (RULES.md D5), one module per area. Every test traces to its
//! requirements with a `/// Verifies:` line.
//!
//! A panic is how a test reports failure, so test code may unwrap, index and
//! panic (RULES.md R-ERR-1 applies to production code only).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    missing_docs,
    reason = "test code: a panic is a test failure (RULES.md R-TEST-4)"
)]

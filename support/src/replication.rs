//! Fixtures for replication's tests (RULES.md D57, R-TEST-10): a replica's
//! journal; a group of replicas whose frames and journal writes a test
//! carries out step by step, so each step of a protocol can be shown; and
//! a deterministic simulator, which runs a group, its databases modelled
//! or real turborel databases, through faults at random from a seed and
//! checks it throughout (`sim`).
//!
//! Each replica's consensus is driven as the crate's contract says a driver
//! drives it: its outputs carried out in order, each `Persist` answered by
//! one `Persisted` once its journal makes it durable.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::unwrap_in_result,
    clippy::missing_errors_doc,
    reason = "test code: a panic is a test failure, and an error a violation found (RULES.md R-TEST-4)"
)]

pub mod contract;
mod group;
mod journal;
pub mod sim;

pub use group::{Frame, Group, Make, Node};
pub use journal::Journal;

//! Fixtures for replication's tests (RULES.md D57, R-TEST-10): a replica's
//! journal, and a group of replicas whose frames and journal writes a test
//! carries out step by step, so each step of a protocol can be shown.
//!
//! Each replica's consensus is driven as the crate's contract says a driver
//! drives it: its outputs carried out in order, each `Persist` answered by
//! one `Persisted` once its journal makes it durable.
#![allow(clippy::indexing_slicing, clippy::arithmetic_side_effects, reason = "test code: a panic is a test failure (RULES.md R-TEST-4)")]

mod group;
mod journal;

pub use group::{Frame, Group, Make, Node};
pub use journal::Journal;

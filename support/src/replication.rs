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

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use turborel_replication::{Vr, VrOptions};

pub mod contract;
mod group;
mod journal;
pub mod sim;

pub use group::{Frame, Group, Make, Node};
pub use journal::Journal;

/// The nonces of the recoveries made here, each once.
static NONCES: AtomicU64 = AtomicU64::new(1);

/// \[TOOL\] What makes a VR replica, of the crate's default options: new
/// by fiat where the host says so, else from what its journal held,
/// recovering from the group if nothing (`Vr::restart`), its recovery's
/// nonce a count.
pub fn vr_make() -> Make {
    Arc::new(|members, me, recovered| {
        Box::new(match recovered {
            None => Vr::new(members, me, VrOptions::default()).unwrap(),
            Some(held) => Vr::restart(members, me, VrOptions::default(), held, NONCES.fetch_add(1, Ordering::Relaxed)).unwrap(),
        })
    })
}

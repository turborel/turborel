//! [ISO/IEC 9075-2:2023 §4.46, §11.49, §11.50, §15.19 to §15.21] Triggers
//! (Features T200, T211 to T218): defined and dropped (`define.rs`), the
//! privileges of an INSTEAD OF trigger (`instead.rs`), the state changes of
//! a statement (`changes.rs`), the triggers they activate executed
//! (`fire.rs`), each invocation of a triggered action (`run.rs`), and the
//! INSTEAD OF trigger that takes a change of a view in place of it
//! (`routed.rs`, `rerouted.rs`).

mod changes;
mod debugged;
mod define;
mod dependents;
mod fire;
mod instead;
mod mutated;
mod named;
mod rerouted;
mod routed;
mod run;

pub(super) use changes::Event;
pub(super) use fire::Triggered;
pub(super) use named::named;
pub(super) use rerouted::view_rows;
pub(super) use routed::{Instead, instead_for, updatable};
pub(super) use run::Triggering;
pub(crate) use run::trigger_texts;

//! [ISO/IEC 9075-2:2023 §4.46, §11.49, §11.50, §15.19 to §15.21] Triggers
//! (Features T200, T211 to T218): defined and dropped (`define.rs`), the
//! privileges of an INSTEAD OF trigger (`instead.rs`), the state changes of
//! a statement (`changes.rs`), the triggers they activate executed
//! (`fire.rs`), each invocation of a triggered action (`run.rs`).

mod changes;
mod define;
mod dependents;
mod fire;
mod instead;
mod named;
mod rerouted;
mod run;

pub(super) use fire::Triggered;
pub(super) use run::Triggering;

/// \[DESIGN\] How deep triggered actions nest, a trigger's statement
/// activating another's, or its own: past it, the statement fails, as
/// with any triggered action exception (SQLSTATE 09000), not the stack
/// of the thread that runs it (RULES.md R-CF-6).
pub(super) const MAX_TRIGGER_DEPTH: usize = 16;

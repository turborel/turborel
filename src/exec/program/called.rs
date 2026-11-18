//! [ISO/IEC 9075-2:2023 §16.1, §9.18] A CALL bound with the program it is
//! in, of a function's body: each argument's value assigned to its
//! parameter [GR4c], the procedure's program run over a frame of them, and
//! each output parameter's value assigned to its target after [GR10b].

use std::sync::Arc;

use super::Host;
use super::run::{Ended, run_program};
use crate::db::{Completion, assign_targets};
use crate::error::{Result, Subject};
use crate::logical::Called;
use crate::params::Params;
use crate::value::{Value, assign};

/// [§16.1 GR1, §9.18 GR4c, GR10b] The CALL `c` run in `host` from the frame
/// `frame`.
///
/// # Errors
///
/// An argument's, or its store assignment's; an exception of the
/// procedure no handler of it handles; an output parameter's assignment's.
pub(super) fn call<H: Host>(host: &mut H, c: &Called, frame: &mut Params) -> Result<Completion> {
    let row = host.rows(&c.inputs, frame, Some(1))?.into_iter().next().unwrap_or_default();
    let mut values = Vec::with_capacity(c.program.base.saturating_add(c.program.variables.len()));
    for (k, (v, (name, mode, t))) in row.into_iter().zip(&c.parameters).enumerate() {
        let about = || Subject::parameter(c.name.parts(), c.specific.parts(), (k.saturating_add(1), mode.name(), name.clone()));
        values.push(assign(v, t, "ISO/IEC 9075-2:2023 §9.18 GR4c i").map_err(|e| e.or_about(about()))?);
    }
    values.resize(c.program.base.saturating_add(c.program.variables.len()), Value::Null);
    let mut inner = Params::of_routine(values, Arc::clone(&c.program.scope));
    // [§9.18 GR7, GR15] at a savepoint level of its own, if of NEW
    // SAVEPOINT LEVEL.
    if c.new_level {
        host.open_level(false);
    }
    let ran = run_program(&c.program, &mut inner, host);
    if c.new_level {
        host.close_level();
    }
    let completion = match ran? {
        Ended::Completed(completion) => completion,
        Ended::Returned(_) => Completion::Success,
    };
    frame.set_routine(Arc::clone(&c.scope));
    let outputs: Vec<Value> = c.targets.iter().map(|(k, _)| inner.dynamic_values().get(*k).cloned().unwrap_or(Value::Null)).collect();
    let specs: Vec<_> = c.targets.iter().map(|(_, t)| t.clone()).collect();
    let rules = ("ISO/IEC 9075-2:2023 §9.18 GR10b i, §9.1 GR3b", "ISO/IEC 9075-2:2023 §9.18 GR10b i; X3.135-1986 §8.6 GR10");
    assign_targets(&outputs, &specs, frame, rules.0, rules.1)?;
    Ok(completion)
}

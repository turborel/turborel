//! \[EXTENSION\] A statement of a triggered action run alone, as a plain
//! action's statements are, looked at by the debugger attached to the
//! session (RULES.md D1014): as it begins, a frame of the trigger is pushed,
//! showing where it begins and the transition variables, and it pauses
//! there if a step or a breakpoint stops there. A control statement is a
//! program, whose own frame does that (`exec/program/debug.rs`).

use super::super::{Session, State};
use crate::catalog::TriggerDesc;
use crate::debugger::{Kind, Origin, Tracked, line_of, may_debug};
use crate::error::Result;
use crate::exec::{pause_most, transition_named};
use crate::params::Params;

impl Session {
    /// The statement of `t`'s action beginning at `at`, run with `params`
    /// in `st`, looked at by the debugger attached: its frame, kept while
    /// it runs, if one is attached.
    ///
    /// # Errors
    ///
    /// The cancellation of the statement while it is paused.
    pub(super) fn trigger_step(&self, st: &State, t: &TriggerDesc, at: usize, params: &Params) -> Result<Option<Tracked>> {
        let Some(link) = self.stop.debugged().link() else { return Ok(None) };
        let origin = Origin { kind: Kind::Trigger, name: t.name.clone(), shown: t.name.clone() };
        let visible = link.debugger().is_ok_and(|d| may_debug(&*st.catalog, &d, Some(&origin)));
        let Some(tracked) = Tracked::alone(link, origin, visible) else { return Ok(None) };
        if !visible {
            tracked.update(|f| f.at = Some(at));
            return Ok(Some(tracked));
        }
        let parameters = transition_named(params).unwrap_or_default();
        tracked.update(|f| (f.at, f.line, f.parameters) = (Some(at), Some(line_of(&t.text, at)), parameters));
        if let Some(reason) = tracked.stops(at) {
            let (index, shared) = tracked.link();
            shared.pause((index, reason), pause_most(st), &|| self.stop.check())?;
        }
        Ok(Some(tracked))
    }
}

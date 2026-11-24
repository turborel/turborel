//! \[EXTENSION\] A program run with a debugger attached to its host's
//! session (RULES.md D1005, D1007): as each statement but a compound
//! statement begins, its frame is shown as it is, and the statement pauses
//! if a step or a breakpoint stops there.
//!
//! With none attached, `Run::debugged` is the host's one look at its flag
//! and returns: no frame is made, and nothing else is done.

use std::time::Duration;

use super::Host;
use super::run::{Run, Stop};
use crate::catalog::Catalog;
use crate::debugger::{CursorShown, Frame, Kind, Named, Origin, Tracked, line_of, may_debug};
use crate::logical::Step;
use crate::settings::DEBUGGER_PAUSE_TIMEOUT_MS;

impl<H: Host> Run<'_, '_, H> {
    /// The statement `step` looked at by the debugger attached, if one is:
    /// its frame shown, and paused there if it stops there.
    ///
    /// # Errors
    ///
    /// The cancellation of the statement while it is paused.
    pub(super) fn debugged(&mut self, step: &Step) -> Result<(), Stop> {
        let Some((link, st)) = self.host.debugged() else { return Ok(()) };
        if matches!(step, Step::Block(_)) {
            return Ok(());
        }
        if self.tracked.is_none() {
            let visible = link.debugger().is_ok_and(|d| may_debug(&*st.catalog, &d, self.program.origin.as_ref()));
            self.tracked = Tracked::enter(link, self.program, visible);
        }
        let Some(tracked) = &self.tracked else { return Ok(()) };
        let at = tracked.place(step);
        if !tracked.visible() {
            tracked.update(|f| f.at = Some(at));
            return Ok(());
        }
        let (parameters, variables, cursors) = self.shown();
        let written = self.program.origin.as_ref().and_then(|o| text_of(&*st.catalog, o)).map(|text| line_of(text, at));
        tracked.update(|f| *f = Frame { at: Some(at), line: written, parameters, variables, cursors, ..f.clone() });
        let Some(reason) = tracked.stops(at) else { return Ok(()) };
        let most = Duration::from_millis(u64::try_from(st.settings.integer(DEBUGGER_PAUSE_TIMEOUT_MS)).unwrap_or_default());
        let (index, shared) = tracked.link();
        let host = &*self.host;
        shared.pause((index, reason), most, &|| host.interrupted()).map_err(Stop::Unhandled)
    }

    /// The frame's parameters, or a trigger's transition variables, its
    /// variables in scope and its cursors in scope, each with its value.
    fn shown(&self) -> (Vec<Named>, Vec<Named>, Vec<CursorShown>) {
        let values = self.frame.dynamic_values();
        let value = |slot: usize| values.get(slot).cloned().unwrap_or(crate::value::Value::Null);
        let parameters = if let Some(t) = self.frame.transitions() {
            let width = t.columns.len();
            let rows = [(t.old_row.as_deref(), 0), (t.new_row.as_deref(), width)];
            rows.iter()
                .filter_map(|(name, first)| name.map(|n| (n, *first)))
                .flat_map(|(row, first)| {
                    t.columns
                        .iter()
                        .enumerate()
                        .map(move |(k, c)| (format!("{row}.{}", c.name), c.data_type.to_string(), first.saturating_add(k)))
                })
                .map(|(name, data_type, slot)| Named { name, data_type, value: value(slot) })
                .collect()
        } else {
            let declared = self.program.scope.parameters.iter().enumerate();
            declared.map(|(k, (name, t, _))| Named { name: name.clone(), data_type: t.to_string(), value: value(k) }).collect()
        };
        let blocks = self.scopes.iter().map(|s| s.block);
        let variables = blocks
            .clone()
            .flat_map(|b| &b.variables)
            .map(|v| Named { name: v.name.clone(), data_type: v.data_type.to_string(), value: value(v.slot) })
            .collect();
        let cursors = blocks
            .flat_map(|b| &b.cursors)
            .filter_map(|n| self.program.cursors.get(*n))
            .map(|c| CursorShown { name: c.name.clone(), open: self.host.cursor_open(&self.key(c)), query: c.text.clone() })
            .collect();
        (parameters, variables, cursors)
    }
}

/// The text of the routine or trigger `origin` names, as its definition
/// holds it: what a frame's place is a character of.
fn text_of<'c>(catalog: &'c dyn Catalog, origin: &Origin) -> Option<&'c str> {
    match origin.kind {
        Kind::Trigger => catalog.trigger(&origin.name).map(|t| t.text.as_str()),
        Kind::Procedure | Kind::Function => catalog.routine(&origin.name).map(|r| r.text.as_str()),
    }
}

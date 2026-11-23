//! \[DESIGN\] A prepared CALL run straight (RULES.md D940).
//!
//! A prepared CALL keeps its binding between executions (D793). Once an
//! execution has run it as any CALL runs, the session keeps beside that
//! binding what the next ones need of the procedure. That is its body's
//! program as the session keeps it compiled (D406, D841), the owner its
//! context runs as, and a frame of the program's size. An execution that
//! finds the same binding kept then runs the compiled program over that
//! frame. Each argument that is a `?` or a constant is assigned to its
//! parameter's slot there, and no row of values is planned, no scope is
//! built and no frame is copied.
//!
//! The binding found kept was kept with what holds now (`prepared/plans.rs`,
//! `Key`), and the program was kept with what held as the session took it
//! beside that binding. The two were then the same, so the program is
//! the one the session would find for the CALL now. A binding bound again
//! is another binding, and finds nothing kept beside it.
//!
//! An execution whose argument raises a warning or an exception as it is
//! given its parameter's type runs as any CALL does, so the condition is
//! the one that path raises.

use std::sync::Arc;

use super::super::prepared::{Keeping, KeptPlan};
use super::super::savepoints::Level;
use super::super::{Session, StatementContext};
use super::call::{BoundCall, Given};
use super::kept::KeptProgram;
use super::program::atomic;
use crate::ast::SqlStatement;
use crate::catalog::owner_of;
use crate::error::Result;
use crate::params::Params;
use crate::value::Value;

/// How many prepared CALLs a session keeps what it runs them straight
/// with. One more forgets the one kept first.
const MOST_KEPT_CALLS: usize = 16;

/// The prepared CALLs a session runs straight, the one kept first first.
#[derive(Debug, Default)]
pub(in crate::db) struct KeptCalls(Vec<KeptCall>);

/// What a session keeps beside a prepared CALL's binding.
#[derive(Debug)]
struct KeptCall {
    /// The binding, held so that no other is ever at its address.
    bound: Arc<BoundCall>,
    /// What runs it straight. None if it runs as any CALL does.
    running: Option<Arc<Running>>,
    /// A frame of its program's size no execution is using.
    frame: Option<Params>,
}

/// What runs a prepared CALL straight.
#[derive(Debug)]
struct Running {
    /// Its body's program, as the session keeps it.
    program: KeptProgram,
    /// Whether its body is ATOMIC.
    atomic: bool,
    /// Whether its body runs at a savepoint level of its own.
    level: bool,
    /// The owner of its schema, whom its context runs as.
    owner: String,
}

impl Session {
    /// [ISO/IEC 9075-2:2023 §16.1 GR1, §9.18 GR3 to GR10] The CALL `bound`
    /// binds, found kept, run straight in `cx` with `params`. None if the
    /// session keeps nothing to run it straight with, or an argument is
    /// not given its parameter's type cleanly. Nothing has run then.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 past `MAX_NESTING` contexts; the body's.
    pub(super) fn call_direct(&self, cx: StatementContext<'_>, bound: &Arc<BoundCall>, params: &mut Params) -> Result<Option<()>> {
        let Some(given) = bound.given.as_deref() else { return Ok(None) };
        let Some((running, spare)) = self.kept_call(bound) else { return Ok(None) };
        let Some(mut frame) = framed(bound, given, (&running, spare), params.dynamic_values()) else { return Ok(None) };
        let desc = &bound.desc;
        let ran = {
            let level = running.level.then_some(Level::Procedure);
            let pushed = self.push(&desc.name, (running.owner.clone(), desc.schema.clone()), level);
            pushed.and_then(|_pushed| self.run_framed(cx, &running.program, &mut frame, running.atomic))
        };
        // [ISO/IEC 9075-2:2023 §20.7 GR7a] a `?`'s value its output's.
        let given_back = ran.and_then(|()| {
            let out = frame.dynamic_values();
            bound.dynamic.iter().try_for_each(|(k, index)| params.set_dynamic(*index, out.get(*k).cloned().unwrap_or(Value::Null)))
        });
        self.spare_frame(bound, frame);
        given_back.map(Some)
    }

    /// What runs `bound` straight and the frame kept for it, taken, if the
    /// session keeps them.
    fn kept_call(&self, bound: &Arc<BoundCall>) -> Option<(Arc<Running>, Option<Params>)> {
        let mut inner = self.inner.borrow_mut();
        let kept = inner.routines.1.0.iter_mut().find(|k| Arc::ptr_eq(&k.bound, bound))?;
        Some((Arc::clone(kept.running.as_ref()?), kept.frame.take()))
    }

    /// Keeps `frame` for the next execution of `bound`.
    fn spare_frame(&self, bound: &Arc<BoundCall>, frame: Params) {
        if let Some(kept) = self.inner.borrow_mut().routines.1.0.iter_mut().find(|k| Arc::ptr_eq(&k.bound, bound)) {
            kept.frame = Some(frame);
        }
    }

    /// Keeps what runs `bound` straight, a CALL that has just run in `cx`
    /// as any does, if its statement keeps that binding with what holds
    /// now. A binding that cannot run straight is kept as one, so it is
    /// not looked at again.
    pub(super) fn keep_call(&self, cx: StatementContext<'_>, bound: &Arc<BoundCall>, keeping: &Keeping<'_>) {
        if bound.given.is_none() || self.inner.borrow().routines.1.0.iter().any(|k| Arc::ptr_eq(&k.bound, bound)) {
            return;
        }
        let Ok(Some(KeptPlan::Call(kept))) = keeping.plan() else { return };
        if !Arc::ptr_eq(&kept, bound) {
            return;
        }
        let running = self.running(cx, bound).map(Arc::new);
        let calls = &mut self.inner.borrow_mut().routines.1.0;
        if calls.len() >= MOST_KEPT_CALLS {
            calls.remove(0);
        }
        calls.push(KeptCall { bound: kept, running, frame: None });
    }

    /// What runs `bound` straight in `cx`, if its procedure's body is a
    /// compound statement whose program the session keeps, with what
    /// holds now, and runs compiled.
    fn running(&self, cx: StatementContext<'_>, bound: &BoundCall) -> Option<Running> {
        let desc = &bound.desc;
        let routine = self.kept_routine(desc).ok()?;
        let body = routine.body().ok()?;
        if !matches!(body, SqlStatement::Control(_)) {
            return None;
        }
        let program = routine.current(cx.st, self)?;
        if !self.runs_compiled(cx.st, &program) || program.program.base != desc.parameters.len() {
            return None;
        }
        let owner = owner_of(&*cx.st.catalog, &desc.name.auth).to_owned();
        Some(Running { program, atomic: atomic(body), level: routine.new_savepoint_level(), owner })
    }
}

/// The frame of `running`'s program for an execution of `bound` with
/// `slots`, the values of its `?`s. Each parameter's slot holds its
/// argument's value as `given` gives it, each variable's the null value,
/// in `spare` if it is of the program's size. None if an argument is not
/// given its parameter's type cleanly.
fn framed(bound: &BoundCall, given: &[Given], (running, spare): (&Running, Option<Params>), slots: &[Value]) -> Option<Params> {
    let program = &running.program.program;
    let size = program.base.saturating_add(program.variables.len());
    let mut frame = match spare.filter(|f| f.dynamic_values().len() == size) {
        Some(mut frame) => {
            frame.set_routine(Arc::clone(&program.scope));
            frame
        }
        None => Params::of_routine(vec![Value::Null; size], Arc::clone(&program.scope)),
    };
    for (k, (g, p)) in given.iter().zip(&bound.desc.parameters).enumerate() {
        frame.set_slot(k, g.value(&p.data_type, slots)?).ok()?;
    }
    for k in program.base..size {
        frame.set_slot(k, Value::Null).ok()?;
    }
    Some(frame)
}

#[cfg(test)]
mod tests;

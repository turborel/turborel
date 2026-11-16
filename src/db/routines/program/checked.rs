//! [ISO/IEC 9075-4:2006 draft Clause 14] A program bound as the routine or
//! trigger whose body it is is defined, and as what it needs is revoked or
//! dropped: its leaves, and each statement within it in the scope where it
//! is, so each is checked and what each names and invokes is noted.

use std::sync::Arc;

use super::super::super::{Session, State};
use crate::ast::SqlStatement;
use crate::binder::{Context, Frame, bind_program};
use crate::catalog::WHOLE;
use crate::error::Result;
use crate::params::{Params, RoutineScope};
use crate::value::{DataType, Value};

/// The frame `stmt` is bound over with `params`: their values', and of a
/// function, its result type `returns`.
fn frame_of(params: &Params, returns: Option<DataType>) -> Frame {
    let scope = params.routine_scope().map_or_else(|| RoutineScope::unnamed(WHOLE.clone()), RoutineScope::clone);
    Frame { base: params.dynamic_values().len(), scope: Arc::new(scope), transitions: params.transitions_shared(), returns, inline: false }
}

impl State {
    /// The control statement `stmt` of a routine's body or a triggered
    /// action, bound in `cx` as its owner `owner` would with `params`, a
    /// function's of the result type `returns`.
    ///
    /// # Errors
    ///
    /// A leaf's or a statement's, bound.
    pub(in crate::db) fn bind_program_checked(
        &self,
        cx: Context<'_>,
        stmt: &SqlStatement,
        (owner, params): (&str, &Params),
        returns: Option<DataType>,
    ) -> Result<()> {
        let program = bind_program(cx, stmt, frame_of(params, returns))?;
        let mut values = params.dynamic_values().to_vec();
        values.resize(values.len().saturating_add(program.variables.len()), Value::Null);
        for c in program.contained() {
            let mut inner = Params::of_routine(values.clone(), Arc::clone(&c.scope));
            if let Some(t) = params.transitions_shared() {
                inner = inner.with_transitions(t);
            }
            self.bind_triggered(Context { params: &inner, ..cx }, &c.statement, (owner, &inner))?;
        }
        Ok(())
    }
}

impl Session {
    /// SIGNAL or RESIGNAL `stmt`, prepared: bound so each `?` takes its
    /// type.
    ///
    /// # Errors
    ///
    /// Its binding's.
    pub(in crate::db) fn bind_program_only(&self, st: &State, stmt: &SqlStatement, params: &Params) -> Result<()> {
        self.bound(st, params, |cx| bind_program(cx, stmt, frame_of(params, None))).map(drop)
    }
}

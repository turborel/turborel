//! [ISO/IEC 9075-2:2023 §20.7 GR7a, §20.10, §20.13 GR6, GR10] The output
//! dynamic parameters of a prepared CALL: each `?` that is the argument
//! of an output parameter, or of one both input and output, of the
//! parameter's type [§20.7 SR5a].
//!
//! The prepared statement is described with a column for each, in the
//! order of the procedure's parameters, which DESCRIBE OUTPUT describes
//! [§20.10]; its input parameters are the other `?`s and those of
//! parameters both input and output, which DESCRIBE INPUT describes and
//! an execution gives values for. An execution gives back the output
//! parameters' values, in the order of the columns: a server sends them
//! as a row, and EXECUTE puts them where its `<result using clause>` says
//! \[§20.13 GR10\].

use super::super::{Session, State};
use crate::ast::CallStatement;
use crate::binder::OutColumn;
use crate::error::Result;
use crate::params::{ParameterType, Params};
use crate::value::Value;

/// The output dynamic parameters of a statement prepared.
#[derive(Debug, Clone, Default)]
pub(super) struct Outputs {
    /// The column that describes each.
    pub(super) columns: Vec<OutColumn>,
    /// Each one's place among the statement's `?`s, from 0, in the order
    /// of the columns.
    pub(super) places: Vec<usize>,
    /// The places of those that are output parameters alone, ascending:
    /// an execution gives them no value.
    pub(super) only: Vec<usize>,
}

impl Outputs {
    /// `types`, each `?`'s, those of the input parameters alone.
    pub(super) fn inputs(&self, types: Vec<ParameterType>) -> Vec<ParameterType> {
        types.into_iter().enumerate().filter(|(i, _)| !self.only.contains(i)).map(|(_, t)| t).collect()
    }

    /// `values`, one for each input parameter, with the null value in the
    /// place of each output parameter alone: one for each `?`.
    pub(super) fn expanded(&self, values: Vec<Value>) -> Vec<Value> {
        if self.only.is_empty() {
            return values;
        }
        let mut given = values.into_iter();
        let all = given.len().saturating_add(self.only.len());
        (0..all).map(|i| if self.only.contains(&i) { Value::Null } else { given.next().unwrap_or(Value::Null) }).collect()
    }

    /// The output parameters' values, as an execution with `params` left
    /// them.
    pub(super) fn values(&self, params: &Params) -> Vec<Value> {
        let all = params.dynamic_values();
        self.places.iter().map(|i| all.get(*i).cloned().unwrap_or(Value::Null)).collect()
    }
}

impl Session {
    /// `call` bound as `bind_only` binds it, so each `?` takes its type:
    /// its output dynamic parameters. Of a procedure written in Rust its
    /// arguments alone are bound, all input.
    pub(super) fn call_outputs(&self, st: &State, call: &CallStatement, params: &Params) -> Result<Outputs> {
        if let Some(p) = Session::rust_procedure(st, call) {
            return self.bound(st, params, |cx| super::super::routines::rust_arguments(cx, &*p, call)).map(|_| Outputs::default());
        }
        let bound = self.bound(st, params, |cx| State::bind_call(cx, call))?;
        let mut outputs = Outputs::default();
        for (k, place) in &bound.dynamic {
            let Some(p) = bound.desc.parameters.get(*k) else { continue };
            outputs.columns.push(super::super::routines::out_column(p));
            outputs.places.push(*place);
            if p.mode == crate::catalog::ParameterMode::Out {
                outputs.only.push(*place);
            }
        }
        outputs.only.sort_unstable();
        Ok(outputs)
    }
}

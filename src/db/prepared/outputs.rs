//! [ISO/IEC 9075-2:2023 §20.7 GR7a, §20.10, §20.13 GR6, GR10] (RULES.md
//! D336) The output
//! dynamic parameters of a prepared CALL: each `?` that is the argument
//! of an output parameter, or of one both input and output, of the
//! parameter's type [§20.7 SR5a].
//!
//! The prepared statement is described with a column for each, in the
//! order of the procedure's parameters, which DESCRIBE OUTPUT describes
//! [§20.10]; its input parameters are the other `?`s and those of
//! parameters both input and output, which DESCRIBE INPUT describes and
//! an execution gives values for. An execution gives back the output
//! parameters' values, in the order of the columns. A server sends them
//! as a row, and EXECUTE puts them where its `<result using clause>` says
//! \[§20.13 GR10\].
//!
//! Each `?` of a CALL is described with the mode and the ordinal position
//! of the parameter whose argument holds it, and the procedure's specific
//! name [§20.10 GR7e] (RULES.md D977): an IN parameter's argument may hold
//! several, an output one's holds one alone.

use super::super::{Session, State};
use super::Prepared;
use crate::ast::CallStatement;
use crate::binder::OutColumn;
use crate::catalog::{ParameterMode, QualifiedName};
use crate::error::Result;
use crate::params::{DynamicParameter, ParameterType, Params};
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
    /// [§20.10 GR7e] Of each `?` of an argument of a CALL of an SQL-invoked
    /// procedure, by its number, the mode and the ordinal position, from
    /// 1, of that argument's parameter.
    pub(super) questions: Vec<(usize, ParameterMode, u32)>,
    /// [§20.10 GR7e vi] The specific name of the procedure such a CALL
    /// invokes.
    pub(super) specific: Option<QualifiedName>,
}

impl Outputs {
    /// Each `?`, of the types `types`, in order, as DESCRIBE describes it
    /// (RULES.md D977).
    pub(super) fn described(&self, types: Vec<ParameterType>) -> Vec<DynamicParameter> {
        let of = |i: usize| self.questions.iter().find(|(number, ..)| *number == i);
        let each = |(i, data_type): (usize, ParameterType)| match of(i) {
            Some((_, mode, ordinal)) => DynamicParameter { data_type, mode: *mode, ordinal: Some(*ordinal) },
            None => DynamicParameter::input(data_type),
        };
        types.into_iter().enumerate().map(each).collect()
    }

    /// `values`, one for each input parameter, with the null value in the
    /// place of each output parameter alone, one for each `?`.
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

impl Prepared {
    /// [ISO/IEC 9075-2:2023 §20.10 GR7e vi] Of a CALL of an SQL-invoked
    /// procedure, the procedure's specific name.
    pub fn procedure(&self) -> Option<&QualifiedName> {
        self.outputs.specific.as_ref()
    }

    /// The `?` its `at`-th column describes, of a CALL, from 0.
    pub(crate) fn output_parameter(&self, at: usize) -> Option<&DynamicParameter> {
        self.parameters.get(*self.outputs.places.get(at)?)
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
        let mut outputs = Outputs { specific: Some(bound.desc.specific.clone()), ..Outputs::default() };
        for (number, k) in &bound.questions {
            let Some(p) = bound.desc.parameters.get(*k) else { continue };
            let ordinal = u32::try_from(k.saturating_add(1)).unwrap_or(u32::MAX);
            outputs.questions.push((*number, p.mode, ordinal));
        }
        for (k, place) in &bound.dynamic {
            let Some(p) = bound.desc.parameters.get(*k) else { continue };
            outputs.columns.push(super::super::routines::out_column(p));
            outputs.places.push(*place);
            if p.mode == ParameterMode::Out {
                outputs.only.push(*place);
            }
        }
        outputs.only.sort_unstable();
        Ok(outputs)
    }
}

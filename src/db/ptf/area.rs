//! [ISO/IEC 9075-2:2023 §9.24 GR2] A PTF data area: its private
//! variables, its PTF descriptor areas and PTF cursors, each by its PTF
//! extended name, and its status variable; and the arguments an
//! invocation of a component procedure passes of it [§9.27 GR3a].
//!
//! \[DESIGN\] The PTF extended names are implementation-dependent (UV131):
//! `PTF_` and what each is, then the ordinal of its generic table
//! parameter among them, or of its descriptor parameter among the
//! function's parameters; each an `<identifier>`, as an extended name's
//! value is [§5.4].

use std::collections::HashMap;

use super::super::dynamic::Descriptor;
use crate::catalog::{GenericParameter, SqlRoutineDesc, TableSemantics};
use crate::storage::Row;
use crate::value::{DataType, Value};

/// [ISO/IEC 9075-2:2023 §9.24 GR2f] The status variable's initial value,
/// successful completion.
pub(crate) const SUCCESSFUL: &str = "00000";

/// The name of the initial result row type descriptor area.
const INITIAL_RESULT: &str = "PTF_INITIAL_RESULT";
/// The name of the intermediate result row type descriptor area.
const INTERMEDIATE_RESULT: &str = "PTF_INTERMEDIATE_RESULT";

/// A component procedure of a polymorphic table function [§11.60 SR10i].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Component {
    /// Its PTF describe component procedure.
    Describe,
    /// Its PTF start component procedure.
    Start,
    /// Its PTF fulfill component procedure.
    Fulfill,
    /// Its PTF finish component procedure.
    Finish,
}

/// [ISO/IEC 9075-2:2023 §9.24 GR2b vi, §9.25 GR20c i] A PTF cursor: the
/// rows of its cursor specification, each of the requested columns and,
/// of a table argument of PASS THROUGH, its pass-through input surrogate
/// column; of the types `types`; and where FETCH is among them, if it is
/// open.
#[derive(Debug, Clone, Default)]
pub(crate) struct Cursor {
    /// Its rows.
    pub(crate) rows: Vec<Row>,
    /// The types of its columns.
    pub(crate) types: Vec<DataType>,
    /// The number of the row FETCH gives next, if it is open.
    pub(crate) next: Option<usize>,
}

/// [ISO/IEC 9075-2:2023 §9.24 GR2] A PTF data area.
#[derive(Debug, Clone)]
pub(crate) struct DataArea {
    /// Its PTF descriptor areas, by their PTF extended names.
    pub(crate) descriptors: HashMap<String, Descriptor>,
    /// Its PTF cursors, by theirs.
    pub(crate) cursors: HashMap<String, Cursor>,
    /// The values of its private variables [GR2a], in order.
    pub(crate) private: Vec<Value>,
    /// Its status variable's value, CHARACTER(5) [GR2f].
    pub(crate) status: Value,
    /// The component procedure running on it, if one is.
    pub(crate) running: Option<Component>,
    /// [ISO/IEC 9075-2:2023 §20.28 GR11] The rows its component procedures
    /// piped, each the values of the columns the intermediate result row
    /// type descriptor area described as it was piped.
    pub(crate) piped: Vec<Vec<Value>>,
    /// The name of the one catalog, which its character strings' character
    /// sets are of.
    pub(crate) catalog: String,
}

impl DataArea {
    /// An area of no descriptor areas or cursors, of the private variables
    /// `private` and the catalog `catalog`, its status successful.
    pub(crate) fn new(private: Vec<Value>, catalog: String) -> DataArea {
        DataArea {
            descriptors: HashMap::new(),
            cursors: HashMap::new(),
            private,
            status: Value::Char(SUCCESSFUL.into()),
            running: None,
            piped: Vec::new(),
            catalog,
        }
    }

    /// Whether its status variable is other than successful completion.
    pub(crate) fn failed(&self) -> Option<String> {
        if let Value::Char(s) = &self.status {
            return (s != SUCCESSFUL).then(|| s.to_string());
        }
        if self.status.is_null() {
            return Some(" ".repeat(SUCCESSFUL.len()));
        }
        Some(self.status.to_literal())
    }
}

/// The PTF extended names of a PTF data area, and what an invocation of a
/// component procedure passes of it: the function's parameters, of each
/// generic table parameter its semantics, in order.
#[derive(Debug, Clone)]
pub(crate) struct Layout {
    /// Of each parameter, its generic table parameter's ordinal among
    /// them and its semantics, or whether a descriptor parameter; none of
    /// a parameter of a `<data type>`.
    pub(crate) parameters: Vec<Option<GenericParameter>>,
    /// Whether the initial result row type descriptor area's name is passed
    /// to the describe component procedure [§9.27 GR3a v 1].
    pub(crate) describes_result: bool,
}

impl Layout {
    /// The layout of the function `desc`.
    pub(crate) fn of(desc: &SqlRoutineDesc) -> Layout {
        let describes_result = desc.ptf.as_ref().is_some_and(|p| p.columns.is_none() && !p.only_pass_through);
        Layout { parameters: desc.parameters.iter().map(|p| p.generic).collect(), describes_result }
    }

    /// The ordinals, from 1, of its generic table parameters, with each's
    /// parameter's position.
    pub(crate) fn tables(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.parameters.iter().enumerate().filter(|(_, g)| matches!(g, Some(GenericParameter::Table(_)))).zip(1..).map(|((p, _), t)| (t, p))
    }

    /// [ISO/IEC 9075-2:2023 §9.27 GR3a] The arguments an invocation of
    /// `component` passes, of an area of private variables `private` and
    /// of the function's arguments `arguments`, a descriptor argument's
    /// whether it is null: the private variables, then of each parameter
    /// what it passes, then the name of the result row type descriptor area
    /// it passes, then its status variable.
    pub(crate) fn arguments(&self, component: Component, area: &DataArea, arguments: &[Value]) -> Vec<Value> {
        let mut out = area.private.clone();
        let mut t = 0_usize;
        for (p, generic) in self.parameters.iter().enumerate() {
            let argument = arguments.get(p).cloned().unwrap_or(Value::Null);
            match generic {
                None => out.push(argument),
                Some(GenericParameter::Descriptor) => {
                    out.push(if argument.is_null() { Value::Null } else { name(&descriptor(p)) });
                }
                Some(GenericParameter::Table(g)) => {
                    t = t.saturating_add(1);
                    match component {
                        Component::Describe => out.push(name(&full(t))),
                        Component::Fulfill => out.push(name(&cursor_row(t))),
                        Component::Start | Component::Finish => {}
                    }
                    if let TableSemantics::Set { .. } = g.semantics {
                        out.extend([name(&partitioning(t)), name(&ordering(t))]);
                    }
                    match component {
                        Component::Describe => out.push(name(&requested(t))),
                        Component::Fulfill => out.push(name(&cursor(t))),
                        Component::Start | Component::Finish => {}
                    }
                }
            }
        }
        match component {
            Component::Describe if self.describes_result => out.push(name(INITIAL_RESULT)),
            Component::Describe => {}
            Component::Start | Component::Fulfill | Component::Finish => out.push(name(INTERMEDIATE_RESULT)),
        }
        out.push(area.status.clone());
        out
    }
}

/// The value of an argument that names `n`.
fn name(n: &str) -> Value {
    Value::Char(n.into())
}

/// The name of the full row type descriptor area of generic table `t`.
pub(crate) fn full(t: usize) -> String {
    format!("PTF_FULL_ROW_{t}")
}

/// The name of the partitioning descriptor area of generic table `t`.
pub(crate) fn partitioning(t: usize) -> String {
    format!("PTF_PARTITIONING_{t}")
}

/// The name of the ordering descriptor area of generic table `t`.
pub(crate) fn ordering(t: usize) -> String {
    format!("PTF_ORDERING_{t}")
}

/// The name of the requested row type descriptor area of generic table
/// `t`.
pub(crate) fn requested(t: usize) -> String {
    format!("PTF_REQUESTED_ROW_{t}")
}

/// The name of the cursor row type descriptor area of generic table `t`.
pub(crate) fn cursor_row(t: usize) -> String {
    format!("PTF_CURSOR_ROW_{t}")
}

/// The name of the PTF cursor of generic table `t`.
pub(crate) fn cursor(t: usize) -> String {
    format!("PTF_CURSOR_{t}")
}

/// The name of the descriptor area of the descriptor argument of
/// parameter `p`, from 0.
pub(crate) fn descriptor(p: usize) -> String {
    format!("PTF_DESCRIPTOR_{}", p.saturating_add(1))
}

/// The name of the initial result row type descriptor area.
pub(crate) fn initial_result() -> &'static str {
    INITIAL_RESULT
}

/// The name of the intermediate result row type descriptor area.
pub(crate) fn intermediate_result() -> &'static str {
    INTERMEDIATE_RESULT
}

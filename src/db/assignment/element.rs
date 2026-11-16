//! [ISO/IEC 9075-2:2023 §6.4 SR8] A `<target array element specification>`
//! (Feature S097): a FETCH, SELECT INTO or CALL target `A[i]` that assigns
//! one element of the array an SQL parameter or a new row column holds.
//! The value is one a site of the element type may take [§14.5 SR9b iii
//! 1, §14.7 SR3b ii 1]. The array with that element the value is stored
//! in its target [§14.5 GR5b i 1, §14.7 GR4b ii 1, §10.4 GR10b ii 1],
//! which stores each element in the element type [§9.2 GR3b xxii], the
//! value too.

use super::declared_type;
use crate::ast::{ElementPosition, Literal, ParameterSpecification, TargetKind};
use crate::error::{Code, Result, SqlError};
use crate::params::Params;
use crate::value::types::array::with_element;
use crate::value::{DataType, Value};

/// The rules of an element of a null array assigned, case A of each.
const NULL_ARRAY: &str = "§14.5 GR5b i 1 A, §14.7 GR4b ii 1 A, §10.4 GR10b ii 1 A";
/// The rules of an element past the maximum cardinality assigned, case B V
/// 3 of each.
const NO_ELEMENT: &str = "§14.5 GR5b i 1 B V 3, §14.7 GR4b ii 1 B V 3, §10.4 GR10b ii 1 B V 3";

/// [ISO/IEC 9075-2:2023 §6.4 SR8] The declared type of target `t`, whose
/// array, if it has an element, is of declared type `declared`: the
/// element type of that array \[SR8b\]; else `declared` itself.
///
/// # Errors
///
/// SQLCODE -103 if the array is not of an array type, nor of a distinct
/// type whose source is one \[SR8a\]; or the position is not exact
/// numeric of scale 0 \[SR8c\].
pub(super) fn target_type(params: &Params, t: &ParameterSpecification, declared: DataType) -> Result<DataType> {
    let Some(position) = &t.element else { return Ok(declared) };
    let DataType::Array(a) = declared.represented() else {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §6.4 SR8a] an element of {} is assigned, which is of {declared}, not an array type nor a distinct \
             type whose source is one",
            t.name
        )));
    };
    let exact = match &**position {
        ElementPosition::Literal(Literal::Exact(d)) => d.scale == 0,
        ElementPosition::Literal(_) => false,
        ElementPosition::Parameter(p) => matches!(params.routine_type(p), Some(DataType::Exact { scale: 0, .. })),
    };
    if !exact {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §6.4 SR8c] the position of the element of {} a target assigns is exact numeric of scale 0",
            t.name
        )));
    }
    Ok(a.element.clone())
}

/// [ISO/IEC 9075-2:2023 §14.5 GR5b i 1, §14.7 GR4b ii 1, §10.4 GR10b ii
/// 1] The value target `spec` takes as `fetched` is assigned to it: of an
/// element, the array it holds with that element `fetched`; else
/// `fetched`.
///
/// # Errors
///
/// SQLSTATE 2200E if the array is null \[A\]; 2202E if the position is
/// null, below 1 or past the array's maximum cardinality \[B V 3\].
pub(super) fn assigned(params: &Params, spec: &ParameterSpecification, fetched: Value) -> Result<Value> {
    let Some(position) = &spec.element else { return Ok(fetched) };
    let held = match spec.kind {
        TargetKind::Routine => params.routine_value(&spec.name),
        TargetKind::Transition => params.transition_value(&spec.name),
        TargetKind::Parameter => None,
    };
    let declared = declared_type(params, &spec.name)?;
    let (Some(array), DataType::Array(a)) = (held, declared.represented()) else {
        return Err(design(&format!("{} holds no array whose element a target may assign", spec.name)));
    };
    let i = match &**position {
        ElementPosition::Literal(Literal::Exact(d)) if d.scale == 0 => Some(d.units),
        ElementPosition::Parameter(p) => match params.routine_value(p) {
            Some(Value::Exact(d)) if d.scale == 0 => Some(d.units),
            Some(Value::Null) => None,
            Some(_) | None => return Err(design(&format!("SQL parameter {p} holds no position in an array"))),
        },
        ElementPosition::Literal(_) => return Err(design("a position in an array is not an integer literal")),
    };
    with_element(&array, i, (a.max, fetched), (NULL_ARRAY, NO_ELEMENT))
}

/// A Syntax Rule violation, SQLCODE -103.
fn rule(msg: String) -> SqlError {
    SqlError::new(Code::SyntaxRule, msg)
}

/// A target the checks of its statement should have refused.
fn design(what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {what}"))
}

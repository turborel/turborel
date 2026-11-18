//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.6 to §15.8] The routines
//! of ST_PositionExp, ST_LRMeasure and ST_StartValue.
//!
//! [§15.8.1] ST_StartValue's type definition names its observer of
//! ST_PrivateMeasure ST_LRMeasure(), where §15.8.4, the subclause that
//! defines it, names it ST_Measure(): the subclause's name is taken.

use super::super::args::{Args, P, element_out, int, lr_out, or_null, text};
use super::super::routine::{Routine, called, null_call};
use super::super::{UNIT_LENGTH, from_gml_arg, from_text_arg, from_text_or_gml_arg};
use super::{DISTANCE, ELEMENT, LRM, MEASURE, POSITION, START};
use crate::error::Result;
use crate::spatial::lr::{LrKind, LrMeasure, LrValue, PositionExp, StartValue};
use crate::value::Value;

/// SELF, a position expression or the null value.
fn this(a: &Args<'_>) -> Result<Option<PositionExp>> {
    a.position(0)
}

/// An ST_PositionExp value or the null value.
fn out(p: Option<PositionExp>) -> Result<Value> {
    lr_out(p.map(LrValue::PositionExp))
}

/// An ST_LRMeasure value or the null value.
fn measure_out(m: Option<LrMeasure>) -> Result<Value> {
    lr_out(m.map(LrValue::LrMeasure))
}

/// An ST_StartValue value or the null value.
fn start_out(s: Option<StartValue>) -> Result<Value> {
    lr_out(s.map(LrValue::StartValue))
}

/// The value of a null-call constructor's arguments, none null.
fn made<T>(v: Option<T>) -> Result<T> {
    v.ok_or_else(|| crate::error::SqlError::new(crate::error::Code::Corrupt, "[DESIGN] a null-call routine was given the null value"))
}

/// [§15.6.8] ST_Equals(ST_PositionExp).
fn equals(a: &Args<'_>) -> Result<Value> {
    let found = PositionExp::equals(this(a)?.as_ref(), a.position(1)?.as_ref(), &a.catalog)?;
    Ok(or_null(found, int))
}

/// The routines.
pub static ROUTINES: &[Routine] = &[
    // [§15.6.2] ST_PositionExp's constructors.
    null_call("ST_POSITIONEXP", &[P::Clob], POSITION, |a| from_text_or_gml_arg(a, LrKind::PositionExp)),
    null_call("ST_POSITIONEXP", &[P::Int, P::Int, DISTANCE], POSITION, |a| {
        out(Some(PositionExp::of_ids(made(a.int(0)?)?, made(a.int(1)?)?, made(a.distance(2)?)?)))
    }),
    null_call("ST_POSITIONEXP", &[P::Int, LRM, DISTANCE], POSITION, |a| {
        out(Some(PositionExp::of_id_and_lrm(made(a.int(0)?)?, made(a.lrm(1)?)?, made(a.distance(2)?)?)))
    }),
    null_call("ST_POSITIONEXP", &[ELEMENT, P::Int, DISTANCE], POSITION, |a| {
        out(Some(PositionExp::of_element_and_id(made(a.element(0)?)?, made(a.int(1)?)?, made(a.distance(2)?)?)))
    }),
    null_call("ST_POSITIONEXP", &[ELEMENT, LRM, DISTANCE], POSITION, |a| {
        out(Some(PositionExp::of_element_and_lrm(made(a.element(0)?)?, made(a.lrm(1)?)?, made(a.distance(2)?)?)))
    }),
    // [§15.6.3 to §15.6.7] Its observers and mutators.
    null_call("ST_LINEARELEMENTID", &[POSITION], P::Int, |a| Ok(or_null(this(a)?.map(|p| p.linear_element_id()), int))),
    called("ST_LINEARELEMENTID", &[POSITION, P::Int], POSITION, |a| {
        out(PositionExp::with_linear_element_id(this(a)?.as_ref(), a.int(1)?)?)
    }),
    null_call("ST_LINEARELEMENT", &[POSITION], ELEMENT, |a| element_out(this(a)?.as_ref().and_then(PositionExp::linear_element).cloned())),
    called("ST_LINEARELEMENT", &[POSITION, ELEMENT], POSITION, |a| out(PositionExp::with_linear_element(this(a)?.as_ref(), a.element(1)?))),
    null_call("ST_LRMID", &[POSITION], P::Int, |a| Ok(or_null(this(a)?.map(|p| p.lrm_id()), int))),
    called("ST_LRMID", &[POSITION, P::Int], POSITION, |a| out(PositionExp::with_lrm_id(this(a)?.as_ref(), a.int(1)?)?)),
    null_call("ST_LRM", &[POSITION], LRM, |a| lr_out(this(a)?.as_ref().and_then(PositionExp::lrm).cloned().map(LrValue::Lrm))),
    called("ST_LRM", &[POSITION, LRM], POSITION, |a| out(PositionExp::with_lrm(this(a)?.as_ref(), a.lrm(1)?))),
    null_call("ST_DISTANCEEXP", &[POSITION], DISTANCE, |a| lr_out(this(a)?.map(|p| LrValue::DistanceExp(p.distance_exp().clone())))),
    called("ST_DISTANCEEXP", &[POSITION, DISTANCE], POSITION, |a| out(PositionExp::with_distance_exp(this(a)?.as_ref(), a.distance(1)?)?)),
    // [§15.6.8] Comparing.
    called("ST_EQUALS", &[POSITION, POSITION], P::Int, equals).reading(),
    // [§15.6.9, §15.6.10] The functions of the text and GML.
    null_call("ST_POSEXPFROMTEXT", &[P::Clob], POSITION, |a| from_text_arg(a, LrKind::PositionExp)),
    null_call("ST_POSEXPFROMGML", &[P::Clob], POSITION, |a| from_gml_arg(a, LrKind::PositionExp)),
    // [§15.7] ST_LRMeasure.
    null_call("ST_LRMEASURE", &[P::Double], MEASURE, |a| measure_out(Some(LrMeasure::new(made(a.double(0)?)?)))),
    called("ST_LRMEASURE", &[P::Double, P::Text(UNIT_LENGTH)], MEASURE, |a| {
        measure_out(Some(LrMeasure::new_with_units(a.double(0)?, a.text(1)?, a.catalog.units)?))
    }),
    null_call("ST_MEASURE", &[MEASURE], P::Double, |a| Ok(or_null(a.measure(0)?.map(|m| m.measure()), Value::Approx))),
    called("ST_MEASURE", &[MEASURE, P::Double], MEASURE, |a| measure_out(LrMeasure::with_measure(a.measure(0)?.as_ref(), a.double(1)?)?)),
    null_call("ST_UNITOFMEASURE", &[MEASURE], P::Text(UNIT_LENGTH), |a| {
        Ok(or_null(a.measure(0)?.as_ref().and_then(LrMeasure::unit_of_measure), text))
    }),
    called("ST_UNITOFMEASURE", &[MEASURE, P::Text(UNIT_LENGTH)], MEASURE, |a| {
        measure_out(LrMeasure::with_unit_of_measure(a.measure(0)?.as_ref(), a.text(1)?, a.catalog.units)?)
    }),
    // [§15.8] ST_StartValue.
    null_call("ST_STARTVALUE", &[P::Int, MEASURE], START, |a| start_out(Some(StartValue::new(made(a.int(0)?)?, made(a.measure(1)?)?)))),
    null_call("ST_LRM", &[START], P::Int, |a| Ok(or_null(a.start_value(0)?.map(|s| s.lrm()), int))),
    called("ST_LRM", &[START, P::Int], START, |a| start_out(StartValue::with_lrm(a.start_value(0)?.as_ref(), a.int(1)?)?)),
    null_call("ST_MEASURE", &[START], MEASURE, |a| measure_out(a.start_value(0)?.map(|s| s.measure().clone()))),
    called("ST_MEASURE", &[START, MEASURE], START, |a| start_out(StartValue::with_measure(a.start_value(0)?.as_ref(), a.measure(1)?)?)),
];

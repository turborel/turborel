//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.9] The routines of
//! ST_DistanceExp: its sixteen constructors, its observers and mutators,
//! and the functions of its text and GML.

use super::super::args::{Args, P, lr_out, or_null, text};
use super::super::routine::{Routine, called, null_call};
use super::super::{FEATURE_ID_LENGTH, NAME_LENGTH, from_gml_arg, from_text_arg, from_text_or_gml_arg};
use super::{DISTANCE, LATERAL, MEASURE, VECTOR, VERTICAL};
use crate::error::Result;
use crate::spatial::lr::{DistanceExp, LrKind, LrValue, ReferentNames};
use crate::value::Value;

const ID: P = P::Text(FEATURE_ID_LENGTH);
const NAME: P = P::Text(NAME_LENGTH);

/// SELF, a distance expression or the null value.
fn this(a: &Args<'_>) -> Result<Option<DistanceExp>> {
    a.distance(0)
}

/// An ST_DistanceExp value or the null value.
fn out(d: Option<DistanceExp>) -> Result<Value> {
    lr_out(d.map(LrValue::DistanceExp))
}

/// The referents named by the `count` arguments after the first: none,
/// the "from" referent, or both.
fn names(a: &Args<'_>, count: usize) -> Result<ReferentNames> {
    let mut n = ReferentNames::default();
    if count >= 2 {
        n.from_feature = a.text(1)?;
        n.from_name = a.text(2)?;
    }
    if count >= 4 {
        n.towards_feature = a.text(3)?;
        n.towards_name = a.text(4)?;
    }
    Ok(n)
}

/// [§15.9.2 Description 30] A distance expression of `count` referent
/// arguments and the lateral and vertical offsets at `lateral` and
/// `vertical`.
fn offsets(a: &Args<'_>, count: usize, (lateral, vertical): (Option<usize>, Option<usize>)) -> Result<Value> {
    let lat = lateral.map(|at| a.lateral(at)).transpose()?.flatten();
    let ver = vertical.map(|at| a.vertical(at)).transpose()?.flatten();
    out(Some(DistanceExp::new(a.measure(0)?, names(a, count)?, lat, ver)?))
}

/// [§15.9.2 Description 32] A distance expression of `count` referent
/// arguments and the vector offset at `at`.
fn vectors(a: &Args<'_>, count: usize, at: usize) -> Result<Value> {
    out(Some(DistanceExp::new_vector(a.measure(0)?, names(a, count)?, a.vector(at)?)?))
}

/// An optional text observed.
fn opt(s: Option<&str>) -> Value {
    or_null(s, text)
}

/// The routines.
pub static ROUTINES: &[Routine] = &[
    // [§15.9.2] The constructors.
    null_call("ST_DISTANCEEXP", &[P::Clob], DISTANCE, |a| from_text_or_gml_arg(a, LrKind::DistanceExp)),
    null_call("ST_DISTANCEEXP", &[MEASURE], DISTANCE, |a| offsets(a, 0, (None, None))),
    null_call("ST_DISTANCEEXP", &[MEASURE, LATERAL], DISTANCE, |a| offsets(a, 0, (Some(1), None))),
    null_call("ST_DISTANCEEXP", &[MEASURE, VERTICAL], DISTANCE, |a| offsets(a, 0, (None, Some(1)))),
    null_call("ST_DISTANCEEXP", &[MEASURE, LATERAL, VERTICAL], DISTANCE, |a| offsets(a, 0, (Some(1), Some(2)))),
    null_call("ST_DISTANCEEXP", &[MEASURE, VECTOR], DISTANCE, |a| vectors(a, 0, 1)),
    null_call("ST_DISTANCEEXP", &[MEASURE, ID, NAME], DISTANCE, |a| offsets(a, 2, (None, None))),
    null_call("ST_DISTANCEEXP", &[MEASURE, ID, NAME, LATERAL], DISTANCE, |a| offsets(a, 2, (Some(3), None))),
    null_call("ST_DISTANCEEXP", &[MEASURE, ID, NAME, VERTICAL], DISTANCE, |a| offsets(a, 2, (None, Some(3)))),
    null_call("ST_DISTANCEEXP", &[MEASURE, ID, NAME, LATERAL, VERTICAL], DISTANCE, |a| offsets(a, 2, (Some(3), Some(4)))),
    null_call("ST_DISTANCEEXP", &[MEASURE, ID, NAME, VECTOR], DISTANCE, |a| vectors(a, 2, 3)),
    null_call("ST_DISTANCEEXP", &[MEASURE, ID, NAME, ID, NAME], DISTANCE, |a| offsets(a, 4, (None, None))),
    null_call("ST_DISTANCEEXP", &[MEASURE, ID, NAME, ID, NAME, LATERAL], DISTANCE, |a| offsets(a, 4, (Some(5), None))),
    null_call("ST_DISTANCEEXP", &[MEASURE, ID, NAME, ID, NAME, VERTICAL], DISTANCE, |a| offsets(a, 4, (None, Some(5)))),
    called("ST_DISTANCEEXP", &[MEASURE, ID, NAME, ID, NAME, LATERAL, VERTICAL], DISTANCE, |a| offsets(a, 4, (Some(5), Some(6)))),
    called("ST_DISTANCEEXP", &[MEASURE, ID, NAME, ID, NAME, VECTOR], DISTANCE, |a| vectors(a, 4, 5)),
    // [§15.9.3 to §15.9.10] The observers and mutators.
    null_call("ST_DISTANCEALONG", &[DISTANCE], MEASURE, |a| lr_out(this(a)?.map(|d| LrValue::LrMeasure(d.distance_along().clone())))),
    called("ST_DISTANCEALONG", &[DISTANCE, MEASURE], DISTANCE, |a| {
        out(DistanceExp::with_distance_along(this(a)?.as_ref(), a.measure(1)?)?)
    }),
    null_call("ST_FROMREFFEAID", &[DISTANCE], ID, |a| Ok(opt(this(a)?.as_ref().and_then(DistanceExp::from_ref_fea_id)))),
    called("ST_FROMREFFEAID", &[DISTANCE, ID], DISTANCE, |a| out(DistanceExp::with_from_ref_fea_id(this(a)?.as_ref(), a.text(1)?))),
    null_call("ST_FROMREFNAME", &[DISTANCE], NAME, |a| Ok(opt(this(a)?.as_ref().and_then(DistanceExp::from_ref_name)))),
    called("ST_FROMREFNAME", &[DISTANCE, NAME], DISTANCE, |a| out(DistanceExp::with_from_ref_name(this(a)?.as_ref(), a.text(1)?))),
    null_call("ST_TOWARDSREFFEAID", &[DISTANCE], ID, |a| Ok(opt(this(a)?.as_ref().and_then(DistanceExp::towards_ref_fea_id)))),
    called("ST_TOWARDSREFFEAID", &[DISTANCE, ID], DISTANCE, |a| out(DistanceExp::with_towards_ref_fea_id(this(a)?.as_ref(), a.text(1)?))),
    null_call("ST_TOWARDSREFNAME", &[DISTANCE], NAME, |a| Ok(opt(this(a)?.as_ref().and_then(DistanceExp::towards_ref_name)))),
    called("ST_TOWARDSREFNAME", &[DISTANCE, NAME], DISTANCE, |a| out(DistanceExp::with_towards_ref_name(this(a)?.as_ref(), a.text(1)?)?)),
    null_call("ST_LATOFFSETEXP", &[DISTANCE], LATERAL, |a| {
        lr_out(this(a)?.as_ref().and_then(DistanceExp::lat_offset_exp).cloned().map(LrValue::LatOffsetExp))
    }),
    called("ST_LATOFFSETEXP", &[DISTANCE, LATERAL], DISTANCE, |a| out(DistanceExp::with_lat_offset_exp(this(a)?.as_ref(), a.lateral(1)?)?)),
    null_call("ST_VEROFFSETEXP", &[DISTANCE], VERTICAL, |a| {
        lr_out(this(a)?.as_ref().and_then(DistanceExp::ver_offset_exp).cloned().map(LrValue::VerOffsetExp))
    }),
    called("ST_VEROFFSETEXP", &[DISTANCE, VERTICAL], DISTANCE, |a| {
        out(DistanceExp::with_ver_offset_exp(this(a)?.as_ref(), a.vertical(1)?)?)
    }),
    null_call("ST_VECTOROFFSETEXP", &[DISTANCE], VECTOR, |a| {
        lr_out(this(a)?.as_ref().and_then(DistanceExp::vector_offset_exp).cloned().map(LrValue::VectorOffsetExp))
    }),
    called("ST_VECTOROFFSETEXP", &[DISTANCE, VECTOR], DISTANCE, |a| {
        out(DistanceExp::with_vector_offset_exp(this(a)?.as_ref(), a.vector(1)?)?)
    }),
    // [§15.9.11, §15.9.12] The functions of the text and GML.
    null_call("ST_DISEXPFROMTEXT", &[P::Clob], DISTANCE, |a| from_text_arg(a, LrKind::DistanceExp)),
    null_call("ST_DISEXPFROMGML", &[P::Clob], DISTANCE, |a| from_gml_arg(a, LrKind::DistanceExp)),
];

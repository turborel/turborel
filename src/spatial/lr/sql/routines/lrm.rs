//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1] The routines of ST_LRM.

use super::super::args::{Args, P, int, lr_out, or_null, text};
use super::super::routine::{Routine, called, null_call};
use super::super::{LRM_NAME_LENGTH, TYPE_LENGTH, UNIT_LENGTH, from_gml_arg, from_text_arg, from_text_or_gml_arg};
use super::{DIRECTION_LENGTH, LRM};
use crate::spatial::lr::{LrKind, LrValue, Lrm, LrmParts, OffsetParts};
use crate::value::Value;
use std::sync::Arc;

const SELF: &[P] = &[LRM];
const SELF_INT: &[P] = &[LRM, P::Int];
const SELF_NAME: &[P] = &[LRM, P::Text(LRM_NAME_LENGTH)];
const SELF_TYPE: &[P] = &[LRM, P::Text(TYPE_LENGTH)];
const SELF_UNIT: &[P] = &[LRM, P::Text(UNIT_LENGTH)];
const SELF_DIR: &[P] = &[LRM, P::Text(DIRECTION_LENGTH)];
const SELF_ARRAY: &[P] = &[LRM, P::TextArray];
const FIVE: &[P] = &[P::Int, P::Text(LRM_NAME_LENGTH), P::Text(TYPE_LENGTH), P::Text(UNIT_LENGTH), P::TextArray];
const EIGHT: &[P] = &[
    P::Int,
    P::Text(LRM_NAME_LENGTH),
    P::Text(TYPE_LENGTH),
    P::Text(UNIT_LENGTH),
    P::TextArray,
    P::Text(UNIT_LENGTH),
    P::Text(DIRECTION_LENGTH),
    P::Text(DIRECTION_LENGTH),
];

/// SELF, an ST_LRM value or the null value.
fn this(a: &Args<'_>) -> crate::error::Result<Option<Lrm>> {
    a.lrm(0)
}

/// An ST_LRM value or the null value.
fn out(l: Option<Lrm>) -> crate::error::Result<Value> {
    lr_out(l.map(LrValue::Lrm))
}

/// The parts of the five-argument constructor.
fn parts(a: &Args<'_>) -> crate::error::Result<LrmParts> {
    Ok(LrmParts { id: a.int(0)?, name: a.text(1)?, lrm_type: a.text(2)?, units: a.text(3)?, constraints: a.texts(4)? })
}

/// An optional text observed.
fn opt_text(s: Option<&str>) -> Value {
    or_null(s, text)
}

/// The routines.
pub static ROUTINES: &[Routine] = &[
    // [§15.1.2] The constructors.
    null_call("ST_LRM", &[P::Clob], LRM, |a| from_text_or_gml_arg(a, LrKind::Lrm)),
    called("ST_LRM", FIVE, LRM, |a| out(Some(Lrm::new(parts(a)?, a.catalog.units)?))),
    called("ST_LRM", EIGHT, LRM, |a| {
        let offsets = OffsetParts { units: a.text(5)?, lateral: a.text(6)?, vertical: a.text(7)? };
        out(Some(Lrm::new_with_offsets(parts(a)?, offsets, a.catalog.units)?))
    }),
    // [§15.1.3 to §15.1.10] The observers and mutators.
    null_call("ST_LRMID", SELF, P::Int, |a| Ok(or_null(this(a)?.map(|l| l.lrm_id()), int))),
    called("ST_LRMID", SELF_INT, LRM, |a| out(Lrm::with_lrm_id(this(a)?.as_ref(), a.int(1)?)?)),
    null_call("ST_LRMNAME", SELF, P::Text(LRM_NAME_LENGTH), |a| Ok(or_null(this(a)?.as_ref().map(Lrm::lrm_name), text))),
    called("ST_LRMNAME", SELF_NAME, LRM, |a| out(Lrm::with_lrm_name(this(a)?.as_ref(), a.text(1)?)?)),
    null_call("ST_LRMTYPE", SELF, P::Text(TYPE_LENGTH), |a| Ok(or_null(this(a)?.as_ref().map(Lrm::lrm_type), text))),
    called("ST_LRMTYPE", SELF_TYPE, LRM, |a| out(Lrm::with_lrm_type(this(a)?.as_ref(), a.text(1)?)?)),
    null_call("ST_UNITOFMEASURE", SELF, P::Text(UNIT_LENGTH), |a| Ok(or_null(this(a)?.as_ref().map(Lrm::unit_of_measure), text))),
    called("ST_UNITOFMEASURE", SELF_UNIT, LRM, |a| out(Lrm::with_unit_of_measure(this(a)?.as_ref(), a.text(1)?, a.catalog.units)?)),
    null_call("ST_CONSTRAINTS", SELF, P::TextArray, |a| {
        let l = this(a)?;
        let items = l.as_ref().and_then(Lrm::constraints).map(|cs| cs.iter().map(|c| text(c)).collect::<Vec<_>>());
        Ok(or_null(items, |v| Value::Array(Arc::from(v))))
    }),
    called("ST_CONSTRAINTS", SELF_ARRAY, LRM, |a| out(Lrm::with_constraints(this(a)?.as_ref(), a.texts(1)?))),
    null_call("ST_OFFSETMEASUNIT", SELF, P::Text(UNIT_LENGTH), |a| Ok(opt_text(this(a)?.as_ref().and_then(Lrm::offset_meas_unit)))),
    called("ST_OFFSETMEASUNIT", SELF_UNIT, LRM, |a| out(Lrm::with_offset_meas_unit(this(a)?.as_ref(), a.text(1)?, a.catalog.units)?)),
    null_call("ST_POSLATOFFSETDIR", SELF, P::Text(DIRECTION_LENGTH), |a| Ok(opt_text(this(a)?.as_ref().and_then(Lrm::pos_lat_offset_dir)))),
    called("ST_POSLATOFFSETDIR", SELF_DIR, LRM, |a| out(Lrm::with_pos_lat_offset_dir(this(a)?.as_ref(), a.text(1)?)?)),
    null_call("ST_POSVEROFFSETDIR", SELF, P::Text(DIRECTION_LENGTH), |a| Ok(opt_text(this(a)?.as_ref().and_then(Lrm::pos_ver_offset_dir)))),
    called("ST_POSVEROFFSETDIR", SELF_DIR, LRM, |a| out(Lrm::with_pos_ver_offset_dir(this(a)?.as_ref(), a.text(1)?)?)),
    // [§15.1.11, §15.1.12] The functions of the text and GML.
    null_call("ST_LRMFROMTEXT", &[P::Clob], LRM, |a| from_text_arg(a, LrKind::Lrm)),
    null_call("ST_LRMFROMGML", &[P::Clob], LRM, |a| from_gml_arg(a, LrKind::Lrm)),
];

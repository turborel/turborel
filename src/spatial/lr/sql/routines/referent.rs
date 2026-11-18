//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.10 to §15.12] The
//! routines of ST_Referent, ST_LatOffsetExp and ST_VerOffsetExp.
//!
//! [§15.13] ST_VectorOffsetExp's constructor and ST_Vectors take and
//! give an ARRAY of §17.2's ST_Vector values (`geometry/support.rs`); an
//! offset vector has no spatial reference system, so one given loses its
//! SRID and one returned is of SRID 0.

use super::super::args::{Args, P, geom_out, lr_out, or_null, text, vectors_out};
use super::super::routine::{Routine, called, null_call};
use super::super::{DESCRIPTION_LENGTH, NAME_LENGTH, TYPE_LENGTH};
use super::{LATERAL, MEASURE, POSITION, REFERENT, VECTOR, VERTICAL};
use crate::error::Result;
use crate::geometry::model::Kind as GeomKind;
use crate::spatial::lr::{LatOffsetExp, LrValue, Referent, VectorOffsetExp, VerOffsetExp};
use crate::value::Value;

const NAME: P = P::Text(NAME_LENGTH);
const KIND: P = P::Text(TYPE_LENGTH);
const POINT: P = P::Geo(GeomKind::Point);
const GEOMETRY: P = P::Geo(GeomKind::Geometry);
const DESCRIPTION: P = P::Text(DESCRIPTION_LENGTH);

/// SELF, a referent or the null value.
fn this(a: &Args<'_>) -> Result<Option<Referent>> {
    a.referent(0)
}

/// An ST_Referent value or the null value.
fn out(r: Option<Referent>) -> Result<Value> {
    lr_out(r.map(LrValue::Referent))
}

/// An ST_LatOffsetExp value or the null value.
fn lateral_out(l: Option<LatOffsetExp>) -> Result<Value> {
    lr_out(l.map(LrValue::LatOffsetExp))
}

/// An ST_VerOffsetExp value or the null value.
fn vertical_out(v: Option<VerOffsetExp>) -> Result<Value> {
    lr_out(v.map(LrValue::VerOffsetExp))
}

/// A referent of a name, a type and the position and location at
/// `position` and `location`, if given.
fn referent(a: &Args<'_>, (position, location): (Option<usize>, Option<usize>)) -> Result<Value> {
    let point = position.map(|at| a.geom(at, GeomKind::Point)).transpose()?.flatten();
    let place = location.map(|at| a.position(at)).transpose()?.flatten();
    out(Some(Referent::new(a.text(0)?, a.text(1)?, point, place, &a.catalog)?))
}

/// The ST_LRMeasure at `at`, a null-call routine's argument.
fn measure(a: &Args<'_>, at: usize) -> Result<crate::spatial::lr::LrMeasure> {
    a.measure(at)?
        .ok_or_else(|| crate::error::SqlError::new(crate::error::Code::Corrupt, "[DESIGN] a null-call routine was given the null value"))
}

/// The routines.
pub static ROUTINES: &[Routine] = &[
    // [§15.10.2] ST_Referent's constructors.
    null_call("ST_REFERENT", &[NAME, KIND], REFERENT, |a| referent(a, (None, None))),
    null_call("ST_REFERENT", &[NAME, KIND, POINT], REFERENT, |a| referent(a, (Some(2), None))),
    null_call("ST_REFERENT", &[NAME, KIND, POSITION], REFERENT, |a| referent(a, (None, Some(2)))),
    called("ST_REFERENT", &[NAME, KIND, POINT, POSITION], REFERENT, |a| referent(a, (Some(2), Some(3)))).reading(),
    // [§15.10.3 to §15.10.7] Its observers and mutators.
    null_call("ST_REFERENTNAME", &[REFERENT], NAME, |a| Ok(or_null(this(a)?.as_ref().map(Referent::referent_name), text))),
    called("ST_REFERENTNAME", &[REFERENT, NAME], REFERENT, |a| out(Referent::with_referent_name(this(a)?.as_ref(), a.text(1)?)?)),
    null_call("ST_REFERENTTYPE", &[REFERENT], KIND, |a| Ok(or_null(this(a)?.as_ref().map(Referent::referent_type), text))),
    called("ST_REFERENTTYPE", &[REFERENT, KIND], REFERENT, |a| out(Referent::with_referent_type(this(a)?.as_ref(), a.text(1)?)?)),
    null_call("ST_POSITION", &[REFERENT], POINT, |a| Ok(geom_out(this(a)?.as_ref().and_then(Referent::position)))),
    called("ST_POSITION", &[REFERENT, POINT], REFERENT, |a| {
        out(Referent::with_position(this(a)?.as_ref(), a.geom(1, GeomKind::Point)?, &a.catalog)?)
    })
    .reading(),
    null_call("ST_LOCATION", &[REFERENT], POSITION, |a| {
        lr_out(this(a)?.as_ref().and_then(Referent::location).cloned().map(LrValue::PositionExp))
    }),
    called("ST_LOCATION", &[REFERENT, POSITION], REFERENT, |a| {
        out(Referent::with_location(this(a)?.as_ref(), a.position(1)?, &a.catalog)?)
    })
    .reading(),
    called("ST_CHANGEPOSANDLOC", &[REFERENT, POINT, POSITION], REFERENT, |a| {
        out(Referent::change_pos_and_loc(this(a)?.as_ref(), a.geom(1, GeomKind::Point)?, a.position(2)?, &a.catalog)?)
    })
    .reading(),
    // [§15.11] ST_LatOffsetExp.
    null_call("ST_LATOFFSETEXP", &[MEASURE], LATERAL, |a| lateral_out(Some(LatOffsetExp::new(measure(a, 0)?)))),
    null_call("ST_LATOFFSETEXP", &[MEASURE, GEOMETRY], LATERAL, |a| {
        let g = a.geom(1, GeomKind::Geometry)?.ok_or_else(|| crate::error::SqlError::new(crate::error::Code::Corrupt, "[DESIGN] null"))?;
        lateral_out(Some(LatOffsetExp::with_geometry(measure(a, 0)?, g)))
    }),
    null_call("ST_LATOFFSETEXP", &[MEASURE, DESCRIPTION], LATERAL, |a| {
        lateral_out(Some(LatOffsetExp::with_description(measure(a, 0)?, a.text(1)?.unwrap_or_default())))
    }),
    null_call("ST_OFFSETLATDIST", &[LATERAL], MEASURE, |a| {
        lr_out(a.lateral(0)?.as_ref().and_then(LatOffsetExp::offset_lat_dist).cloned().map(LrValue::LrMeasure))
    }),
    called("ST_OFFSETLATDIST", &[LATERAL, MEASURE], LATERAL, |a| {
        lateral_out(LatOffsetExp::with_offset_lat_dist(a.lateral(0)?.as_ref(), a.measure(1)?))
    }),
    null_call("ST_FEATUREGEOMETRY", &[LATERAL], GEOMETRY, |a| {
        Ok(geom_out(a.lateral(0)?.as_ref().and_then(LatOffsetExp::feature_geometry)))
    }),
    called("ST_FEATUREGEOMETRY", &[LATERAL, GEOMETRY], LATERAL, |a| {
        lateral_out(LatOffsetExp::with_feature_geometry(a.lateral(0)?.as_ref(), a.geom(1, GeomKind::Geometry)?)?)
    }),
    null_call("ST_OFFSETREFDESC", &[LATERAL], DESCRIPTION, |a| {
        Ok(or_null(a.lateral(0)?.as_ref().and_then(LatOffsetExp::offset_ref_desc), text))
    }),
    called("ST_OFFSETREFDESC", &[LATERAL, DESCRIPTION], LATERAL, |a| {
        lateral_out(LatOffsetExp::with_offset_ref_desc(a.lateral(0)?.as_ref(), a.text(1)?)?)
    }),
    // [§15.12] ST_VerOffsetExp.
    null_call("ST_VEROFFSETEXP", &[MEASURE], VERTICAL, |a| vertical_out(Some(VerOffsetExp::new(measure(a, 0)?)))),
    null_call("ST_VEROFFSETEXP", &[MEASURE, GEOMETRY], VERTICAL, |a| {
        let g = a.geom(1, GeomKind::Geometry)?.ok_or_else(|| crate::error::SqlError::new(crate::error::Code::Corrupt, "[DESIGN] null"))?;
        vertical_out(Some(VerOffsetExp::with_geometry(measure(a, 0)?, g)))
    }),
    null_call("ST_VEROFFSETEXP", &[MEASURE, DESCRIPTION], VERTICAL, |a| {
        vertical_out(Some(VerOffsetExp::with_description(measure(a, 0)?, a.text(1)?.unwrap_or_default())))
    }),
    null_call("ST_OFFSETVERDIST", &[VERTICAL], MEASURE, |a| {
        lr_out(a.vertical(0)?.as_ref().and_then(VerOffsetExp::offset_ver_dist).cloned().map(LrValue::LrMeasure))
    }),
    called("ST_OFFSETVERDIST", &[VERTICAL, MEASURE], VERTICAL, |a| {
        vertical_out(VerOffsetExp::with_offset_ver_dist(a.vertical(0)?.as_ref(), a.measure(1)?))
    }),
    null_call("ST_FEATUREGEOMETRY", &[VERTICAL], GEOMETRY, |a| {
        Ok(geom_out(a.vertical(0)?.as_ref().and_then(VerOffsetExp::feature_geometry)))
    }),
    called("ST_FEATUREGEOMETRY", &[VERTICAL, GEOMETRY], VERTICAL, |a| {
        vertical_out(VerOffsetExp::with_feature_geometry(a.vertical(0)?.as_ref(), a.geom(1, GeomKind::Geometry)?)?)
    }),
    null_call("ST_OFFSETREFDESC", &[VERTICAL], DESCRIPTION, |a| {
        Ok(or_null(a.vertical(0)?.as_ref().and_then(VerOffsetExp::offset_ref_desc), text))
    }),
    called("ST_OFFSETREFDESC", &[VERTICAL, DESCRIPTION], VERTICAL, |a| {
        vertical_out(VerOffsetExp::with_offset_ref_desc(a.vertical(0)?.as_ref(), a.text(1)?)?)
    }),
    // [§15.13] ST_VectorOffsetExp.
    null_call("ST_VECTOROFFSETEXP", &[P::Vectors], VECTOR, |a| {
        lr_out(Some(LrValue::VectorOffsetExp(VectorOffsetExp::new(a.vectors(0)?.unwrap_or_default())?)))
    }),
    null_call("ST_VECTORS", &[VECTOR], P::Vectors, |a| Ok(a.vector(0)?.map_or(Value::Null, |v| vectors_out(v.vectors())))),
    called("ST_VECTORS", &[VECTOR, P::Vectors], VECTOR, |a| {
        lr_out(VectorOffsetExp::with_vectors(a.vector(0)?.as_ref(), a.vectors(1)?)?.map(LrValue::VectorOffsetExp))
    }),
];

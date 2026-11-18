//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.2] The routines of
//! ST_Vector: the constructors, the observers and mutators of x, y, z and
//! the SRID, ST_Coordinates, ST_Is3D, ST_IsEmpty, ST_Equals, the text,
//! binary and GML representations, and ST_VectorFromText,
//! ST_VectorFromWKB and ST_VectorFromGML. ST_OrderingEquals [§17.2.20] is
//! the function of that name the geometry types share (`ordering.rs`).
//!
//! A constructor or ST_GMLToSQL of a CHARACTER LARGE OBJECT reads GML if
//! it begins with `<`, else well-known text [§17.2.2, the parameter
//! awktorgml].

use super::Support::Vector as V;
use super::ops::{Op, Systems, Ty, arg, clob, double, int, integer, octets, spatial, text};
use super::values::Vector;
use super::vector_formats::{read_binary, read_gml, read_text, write_binary, write_gml, write_text};
use crate::error::{Result, SqlState};
use crate::value::Value;
use crate::value::lob::Lob;

/// ST_MaxVectorAsText, implementation-defined: the GML of a vector whose
/// srsName is a definition fits it too [§17.2.1 DR2].
pub const MAX_VECTOR_AS_TEXT: u32 = 8192;
/// ST_MaxVectorAsBinary: a header and three doubles.
pub const MAX_VECTOR_AS_BINARY: u32 = 29;
/// ST_MaxVectorAsGML, implementation-defined.
pub const MAX_VECTOR_AS_GML: u32 = 8192;

const VECTOR: Ty = Ty::Of(V);
const TEXT: Ty = Ty::Clob(MAX_VECTOR_AS_TEXT);
const BINARY: Ty = Ty::Blob(MAX_VECTOR_AS_BINARY);
const GML: Ty = Ty::Clob(MAX_VECTOR_AS_GML);
const D: Ty = Ty::Double;
const I: Ty = Ty::Integer;

/// The routines.
pub(super) static OPS: &[Op] = &[
    Op::constructor(V, &[], |_, _| Ok(Vector::EMPTY.value())),
    Op::constructor(V, &[TEXT], |a, s| text_or_gml(a, 0, s)).in_catalog(),
    Op::constructor(V, &[TEXT, I], |a, s| text_or_gml(a, integer(a, 1)?, s)).in_catalog(),
    Op::constructor(V, &[BINARY], |a, _| Ok(read_binary(&octets(a, 0)?, 0)?.value())),
    Op::constructor(V, &[BINARY, I], |a, _| Ok(read_binary(&octets(a, 0)?, integer(a, 1)?)?.value())),
    Op::constructor(V, &[D, D], |a, _| xyz(a, None, 0)),
    Op::constructor(V, &[D, D, I], |a, _| xyz(a, None, integer(a, 2)?)),
    Op::constructor(V, &[D, D, D], |a, _| xyz(a, Some(double(a, 2)?), 0)),
    Op::constructor(V, &[D, D, D, I], with_z).called(),
    Op::method("ST_X", &[VECTOR], D, |a, _| Ok(Vector::of(arg(a, 0)?)?.x.map_or(Value::Null, Value::Approx))),
    Op::method("ST_X", &[VECTOR, D], VECTOR, |a, _| set(a, |v, x| v.x = Some(x))).called(),
    Op::method("ST_Y", &[VECTOR], D, |a, _| Ok(Vector::of(arg(a, 0)?)?.y.map_or(Value::Null, Value::Approx))),
    Op::method("ST_Y", &[VECTOR, D], VECTOR, |a, _| set(a, |v, y| v.y = Some(y))).called(),
    Op::method("ST_Z", &[VECTOR], D, |a, _| Ok(Vector::of(arg(a, 0)?)?.z.map_or(Value::Null, Value::Approx))),
    Op::method("ST_Z", &[VECTOR, D], VECTOR, set_z).called(),
    Op::method("ST_COORDINATES", &[VECTOR], Ty::DoubleArray(3), coordinates),
    Op::method("ST_IS3D", &[VECTOR], I, |a, _| Ok(int(i64::from(Vector::of(arg(a, 0)?)?.z.is_some())))),
    Op::method("ST_SRID", &[VECTOR], I, |a, _| Ok(int(i64::from(Vector::of(arg(a, 0)?)?.srid)))),
    Op::method("ST_SRID", &[VECTOR, I], VECTOR, |a, _| Ok(Vector { srid: integer(a, 1)?, ..Vector::of(arg(a, 0)?)? }.value())),
    Op::method("ST_ISEMPTY", &[VECTOR], I, |a, _| Ok(int(i64::from(Vector::of(arg(a, 0)?)?.is_empty())))),
    Op::method("ST_EQUALS", &[VECTOR, VECTOR], I, |a, _| Ok(int(i64::from(Vector::of(arg(a, 0)?)?.equals(&Vector::of(arg(a, 1)?)?))))),
    Op::method("ST_WKTTOSQL", &[VECTOR, TEXT], VECTOR, |a, _| Ok(read_text(&text(a, 1)?, Vector::of(arg(a, 0)?)?.srid)?.value())),
    Op::method("ST_ASTEXT", &[VECTOR], TEXT, |a, _| Ok(clob(&write_text(&Vector::of(arg(a, 0)?)?)?))),
    Op::method("ST_WKBTOSQL", &[VECTOR, BINARY], VECTOR, |a, _| Ok(read_binary(&octets(a, 1)?, Vector::of(arg(a, 0)?)?.srid)?.value())),
    Op::method("ST_ASBINARY", &[VECTOR], BINARY, |a, _| Ok(Value::Lob(Lob::of_octets(&write_binary(&Vector::of(arg(a, 0)?)?)?)))),
    Op::method("ST_GMLTOSQL", &[VECTOR, GML], VECTOR, |a, s| Ok(read_gml(&text(a, 1)?, None, s)?.value())).in_catalog(),
    Op::method("ST_ASGML", &[VECTOR], GML, |a, s| Ok(clob(&write_gml(&Vector::of(arg(a, 0)?)?, s)?))).in_catalog(),
    Op::function("ST_VECTORFROMTEXT", &[TEXT], VECTOR, |a, _| Ok(read_text(&text(a, 0)?, 0)?.value())),
    Op::function("ST_VECTORFROMTEXT", &[TEXT, I], VECTOR, |a, _| Ok(read_text(&text(a, 0)?, integer(a, 1)?)?.value())),
    Op::function("ST_VECTORFROMWKB", &[BINARY], VECTOR, |a, _| Ok(read_binary(&octets(a, 0)?, 0)?.value())),
    Op::function("ST_VECTORFROMWKB", &[BINARY, I], VECTOR, |a, _| Ok(read_binary(&octets(a, 0)?, integer(a, 1)?)?.value())),
    Op::function("ST_VECTORFROMGML", &[GML], VECTOR, |a, s| Ok(read_gml(&text(a, 0)?, Some(0_i32), s)?.value())),
    Op::function("ST_VECTORFROMGML", &[GML, I], VECTOR, |a, s| Ok(read_gml(&text(a, 0)?, Some(integer(a, 1)?), s)?.value())),
];

/// [§17.2.2] A vector of the WKT or GML of argument 0, of SRID `srid`.
fn text_or_gml(args: &[Value], srid: i32, systems: Systems<'_>) -> Result<Value> {
    let given = text(args, 0)?;
    let read = if given.trim_start().starts_with('<') { read_gml(&given, Some(srid), systems)? } else { read_text(&given, srid)? };
    Ok(read.value())
}

/// [§17.2.2] A vector of x and y, the first two arguments, and `z`.
fn xyz(args: &[Value], z: Option<f64>, srid: i32) -> Result<Value> {
    Ok(Vector { x: Some(double(args, 0)?), y: Some(double(args, 1)?), z, srid }.value())
}

/// 2FF03 null argument.
fn null_argument(clause: &str) -> crate::error::SqlError {
    spatial(SqlState::SPATIAL_NULL_ARGUMENT, clause, "null argument")
}

/// [§17.2.2] ST_Vector(x, y, z, srid), called on null input: x and y
/// both null or neither, z null exactly when they are; the null value of
/// a null SRID.
fn with_z(args: &[Value], _: Systems<'_>) -> Result<Value> {
    let given = |at: usize| arg(args, at).map(|v| !v.is_null());
    let (x, y, z) = (given(0)?, given(1)?, given(2)?);
    if x != y {
        return Err(null_argument("§17.2.2"));
    }
    if !x && z {
        return Err(spatial(SqlState::SPATIAL_NOT_AN_EMPTY_SET, "§17.2.2", "not an empty set"));
    }
    if x && !z {
        return Err(null_argument("§17.2.2"));
    }
    if !given(3)? {
        return Ok(Value::Null);
    }
    let srid = integer(args, 3)?;
    if !x {
        return Ok(Vector { srid, ..Vector::EMPTY }.value());
    }
    xyz(args, Some(double(args, 2)?), srid)
}

/// [§17.2.3, §17.2.4] ST_X(DOUBLE PRECISION) and ST_Y(DOUBLE PRECISION):
/// a null coordinate is a null argument; a null SELF gives null.
fn set(args: &[Value], put: fn(&mut Vector, f64)) -> Result<Value> {
    if arg(args, 1)?.is_null() {
        return Err(null_argument("§17.2.3 Description 4a"));
    }
    if arg(args, 0)?.is_null() {
        return Ok(Value::Null);
    }
    let mut v = Vector::of(arg(args, 0)?)?;
    put(&mut v, double(args, 1)?);
    Ok(v.value())
}

/// [§17.2.5 Description 4] ST_Z(DOUBLE PRECISION), raising what the
/// Description says; the Definition's SQL body signals the two
/// conditions the other way round.
fn set_z(args: &[Value], _: Systems<'_>) -> Result<Value> {
    let self_null = arg(args, 0)?.is_null();
    let z = if arg(args, 1)?.is_null() { None } else { Some(double(args, 1)?) };
    let v = if self_null { None } else { Some(Vector::of(arg(args, 0)?)?) };
    let empty = v.is_some_and(|v| v.is_empty());
    if empty && z.is_some() {
        return Err(null_argument("§17.2.5 Description 4a"));
    }
    if v.is_some_and(|v| !v.is_empty()) && z.is_none() {
        return Err(spatial(SqlState::SPATIAL_NOT_AN_EMPTY_SET, "§17.2.5 Description 4b", "not an empty set"));
    }
    Ok(v.map_or(Value::Null, |v| Vector { z, ..v }.value()))
}

/// [§17.2.6] ST_Coordinates(): the null value of the empty set.
fn coordinates(args: &[Value], _: Systems<'_>) -> Result<Value> {
    let v = Vector::of(arg(args, 0)?)?;
    if v.is_empty() {
        return Ok(Value::Null);
    }
    let all = [v.x, v.y, v.z].into_iter().flatten().map(Value::Approx).collect::<Vec<_>>();
    Ok(Value::Array(all.into()))
}

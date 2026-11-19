//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.4, §17.5] The routines
//! of ST_NURBSPoint and ST_Knot: the constructor function, whose value
//! is the empty set, the constructor of every attribute, the observer and
//! mutator of each attribute and ST_IsEmpty. ST_TINElement is
//! `geometry/tin_element.rs`'s.
//!
//! An observer of the empty set gives the null value. ST_Knot's mutators
//! refuse a null argument with 2FF03 and give the null value of a null
//! SELF; the mutators of ST_NURBSPoint say neither, so a null argument
//! gives the null value.

use super::Support::{Knot as K, NurbsPoint as N};
use super::ops::{Op, Ty, arg, double, int, integer, spatial};
use super::values::{Knot, NurbsPoint};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Geom, Kind};
use crate::value::Value;

const KNOT: Ty = Ty::Of(K);
const NURBS: Ty = Ty::Of(N);
const POINT: Ty = Ty::Geom(Kind::Point);

/// The routines.
pub(super) static OPS: &[Op] = &[
    Op::constructor(N, &[], |_, _| Ok(NurbsPoint::default().value())),
    Op::constructor(N, &[POINT, Ty::Double], |a, _| {
        Ok(NurbsPoint { point: Some(Geom::from_value(arg(a, 0)?)?.encode()), weight: Some(double(a, 1)?) }.value())
    }),
    Op::method("ST_WEIGHTEDPOINT", &[NURBS], POINT, |a, _| {
        Ok(NurbsPoint::of(arg(a, 0)?)?.point.map_or(Value::Null, crate::geometry::kind::wrap))
    }),
    Op::method("ST_WEIGHTEDPOINT", &[NURBS, POINT], NURBS, |a, _| {
        Ok(NurbsPoint { point: Some(Geom::from_value(arg(a, 1)?)?.encode()), ..NurbsPoint::of(arg(a, 0)?)? }.value())
    }),
    Op::method("ST_WEIGHT", &[NURBS], Ty::Double, |a, _| Ok(NurbsPoint::of(arg(a, 0)?)?.weight.map_or(Value::Null, Value::Approx))),
    Op::method("ST_WEIGHT", &[NURBS, Ty::Double], NURBS, |a, _| {
        Ok(NurbsPoint { weight: Some(double(a, 1)?), ..NurbsPoint::of(arg(a, 0)?)? }.value())
    }),
    Op::method("ST_ISEMPTY", &[NURBS], Ty::Integer, |a, _| Ok(int(i64::from(NurbsPoint::of(arg(a, 0)?)? == NurbsPoint::default())))),
    Op::constructor(K, &[], |_, _| Ok(Knot::default().value())),
    Op::constructor(K, &[Ty::Double, Ty::Integer], |a, _| {
        Ok(Knot { value: Some(double(a, 0)?), multiplicity: Some(integer(a, 1)?) }.value())
    }),
    Op::method("ST_VALUE", &[KNOT], Ty::Double, |a, _| Ok(Knot::of(arg(a, 0)?)?.value.map_or(Value::Null, Value::Approx))),
    Op::method("ST_VALUE", &[KNOT, Ty::Double], KNOT, |a, _| {
        set_knot(a, "§17.5.3 Description 4a", |k, a| {
            k.value = Some(double(a, 1)?);
            Ok(())
        })
    })
    .called(),
    Op::method("ST_MULTIPLICITY", &[KNOT], Ty::Integer, |a, _| {
        Ok(Knot::of(arg(a, 0)?)?.multiplicity.map_or(Value::Null, |n| int(i64::from(n))))
    }),
    Op::method("ST_MULTIPLICITY", &[KNOT, Ty::Integer], KNOT, |a, _| {
        set_knot(a, "§17.5.4 Description 4a", |k, a| {
            k.multiplicity = Some(integer(a, 1)?);
            Ok(())
        })
    })
    .called(),
    Op::method("ST_ISEMPTY", &[KNOT], Ty::Integer, |a, _| Ok(int(i64::from(Knot::of(arg(a, 0)?)? == Knot::default())))),
];

/// 2FF03 null argument, citing `clause`.
fn null_argument(clause: &str) -> crate::error::SqlError {
    spatial(SqlState::SPATIAL_NULL_ARGUMENT, clause, "null argument")
}

/// What a mutator changes: the value, of the arguments.
type Change<T> = fn(&mut T, &[Value]) -> Result<()>;

/// A mutator of ST_Knot, called on null input.
fn set_knot(args: &[Value], clause: &str, change: Change<Knot>) -> Result<Value> {
    if arg(args, 1)?.is_null() {
        return Err(null_argument(clause));
    }
    if arg(args, 0)?.is_null() {
        return Ok(Value::Null);
    }
    let mut k = Knot::of(arg(args, 0)?)?;
    change(&mut k, args)?;
    Ok(k.value())
}

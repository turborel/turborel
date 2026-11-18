//! The canonical `<spatial reference system>` text of a coordinate system,
//! as ST_AsWKTSRS returns it (ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §14.1.3 Description 2): key words upper case, `<left bracket>` and
//! `<right bracket>` delimiters, no spaces outside names, and each number
//! the shortest decimal that reads back to the same value, so reading the
//! text gives back an equal model. \[EXTENSION\] items are written only
//! when the model has them.

use std::fmt::Write as _;

use super::super::model::{
    Authority, Axis, CoordinateSystem, Datum, Direction, Geocentric, Geographic, Helmert, PrimeMeridian, Projected, Unit,
};
use super::super::number::shortest;
use super::{AUTHORITY, AXIS, DATUM, DIRECTIONS, GEOCCS, GEOGCS, PARAMETER, PRIMEM, PROJCS, PROJECTION, SPHEROID, TOWGS84, UNIT};

/// The canonical text of `system`.
pub fn write(system: &CoordinateSystem) -> String {
    let mut out = String::new();
    match system {
        CoordinateSystem::Projected(cs) => projected(&mut out, cs),
        CoordinateSystem::Geographic(cs) => geographic(&mut out, cs),
        CoordinateSystem::Geocentric(cs) => geocentric(&mut out, cs),
    }
    out
}

fn open(out: &mut String, keyword: &str, name: &str) {
    // Writing to a String cannot fail.
    let _ = write!(out, "{keyword}[\"{name}\"");
}

fn number(out: &mut String, value: f64) {
    out.push(',');
    out.push_str(&shortest(value));
}

fn close(out: &mut String, authority: Option<&Authority>) {
    if let Some(authority) = authority {
        let _ = write!(out, ",{AUTHORITY}[\"{}\",\"{}\"]", authority.name, authority.code);
    }
    out.push(']');
}

fn projected(out: &mut String, cs: &Projected) {
    open(out, PROJCS, &cs.name);
    out.push(',');
    geographic(out, &cs.geographic);
    out.push(',');
    open(out, PROJECTION, &cs.projection.name);
    close(out, cs.projection.authority.as_ref());
    for parameter in &cs.parameters {
        out.push(',');
        open(out, PARAMETER, &parameter.name);
        number(out, parameter.value);
        out.push(']');
    }
    out.push(',');
    unit(out, &cs.linear_unit);
    axes(out, &cs.axes);
    close(out, cs.authority.as_ref());
}

fn geographic(out: &mut String, cs: &Geographic) {
    open(out, GEOGCS, &cs.name);
    out.push(',');
    datum(out, &cs.datum);
    out.push(',');
    prime_meridian(out, &cs.prime_meridian);
    out.push(',');
    unit(out, &cs.angular_unit);
    if let Some(linear) = &cs.linear_unit {
        out.push(',');
        unit(out, linear);
    }
    axes(out, &cs.axes);
    close(out, cs.authority.as_ref());
}

fn geocentric(out: &mut String, cs: &Geocentric) {
    open(out, GEOCCS, &cs.name);
    out.push(',');
    datum(out, &cs.datum);
    out.push(',');
    prime_meridian(out, &cs.prime_meridian);
    out.push(',');
    unit(out, &cs.linear_unit);
    axes(out, &cs.axes);
    close(out, cs.authority.as_ref());
}

fn datum(out: &mut String, datum: &Datum) {
    open(out, DATUM, &datum.name);
    out.push(',');
    open(out, SPHEROID, &datum.spheroid.name);
    number(out, datum.spheroid.semi_major);
    number(out, datum.spheroid.inverse_flattening);
    close(out, datum.spheroid.authority.as_ref());
    if let Some(shift) = &datum.to_wgs84 {
        helmert(out, shift);
    }
    close(out, datum.authority.as_ref());
}

fn helmert(out: &mut String, shift: &Helmert) {
    out.push(',');
    out.push_str(TOWGS84);
    out.push('[');
    out.push_str(&shortest(shift.translation[0]));
    for value in shift.translation.iter().skip(1).chain(&shift.rotation).chain([&shift.scale]) {
        number(out, *value);
    }
    out.push(']');
}

fn prime_meridian(out: &mut String, meridian: &PrimeMeridian) {
    open(out, PRIMEM, &meridian.name);
    number(out, meridian.longitude);
    close(out, meridian.authority.as_ref());
}

fn unit(out: &mut String, unit: &Unit) {
    open(out, UNIT, &unit.name);
    number(out, unit.factor);
    close(out, unit.authority.as_ref());
}

fn axes(out: &mut String, axes: &[Axis]) {
    for axis in axes {
        let _ = write!(out, ",{AXIS}[\"{}\",{}]", axis.name, direction(axis.direction));
    }
}

fn direction(direction: Direction) -> &'static str {
    DIRECTIONS.iter().find(|(_, known)| *known == direction).map_or("OTHER", |(key, _)| key)
}

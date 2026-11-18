//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §9.2.3 to §9.2.6, §8.6.4,
//! §8.6.6] The shells of a B-rep solid, observed and replaced, and the
//! maximum side length and clip of a TIN. A replaced shell is checked as
//! §9.2.3 Description 4 and §9.2.4 Description 4 say: each a shell, no
//! two meeting in more than points, and every interior shell within the
//! exterior one and outside the others.

use super::relations::linear;
use super::{integer, number, spatial};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Body, Collection, Dims, Geom, Kind, Surface};
use crate::geometry::{MAX_GEOMETRY_ARRAY_ELEMENTS, data_type_of};
use crate::spatial::solids::measure::volume;
use crate::spatial::solids::ops::{SetOp, set_op};
use crate::spatial::solids::shape::{patches_simple, surface_is_closed};
use crate::spatial::solids::tin::{clip as clip_tin, with_max_side};
use crate::value::{DataType, Decimal, Value};

/// ST_Surface ARRAY[ST_MaxGeometryArrayElements].
pub fn array() -> DataType {
    DataType::array(data_type_of(Kind::Surface), MAX_GEOMETRY_ARRAY_ELEMENTS)
}

/// The shells of a solid, exterior first.
fn shells(g: &Geom) -> &[Surface] {
    if let Body::Solid(all) = &g.body { all } else { &[] }
}

/// §9.2.3 ST_ExteriorShell(): null of the empty set.
pub fn exterior(g: &Geom) -> Value {
    shells(g).first().map_or(Value::Null, |s| g.part(Body::Surface(s.clone())).to_value())
}

/// §9.2.4 ST_InteriorShells(): null of the empty set.
pub fn interiors(g: &Geom) -> Value {
    if g.is_empty() {
        return Value::Null;
    }
    Value::Array(shells(g).iter().skip(1).map(|s| g.part(Body::Surface(s.clone())).to_value()).collect::<Vec<_>>().into())
}

/// §9.2.5 ST_NumIntShells(): null of the empty set.
pub fn count(g: &Geom) -> Value {
    if g.is_empty() {
        return Value::Null;
    }
    let n = shells(g).len().saturating_sub(1);
    Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0))
}

/// §9.2.6 ST_InteriorShellN(INTEGER): null of the empty set; null and
/// the warning 01F01 at a position out of range.
///
/// # Errors
///
/// As the position's reading.
pub fn nth(g: &Geom, rest: &[Value]) -> Result<(Value, Option<SqlState>)> {
    if g.is_empty() {
        return Ok((Value::Null, None));
    }
    let position = integer(rest.first().unwrap_or(&Value::Null))?;
    let at = usize::try_from(position).ok().filter(|n| *n >= 1);
    match at.and_then(|n| shells(g).get(n)) {
        Some(s) => Ok((g.part(Body::Surface(s.clone())).to_value(), None)),
        None => Ok((Value::Null, Some(SqlState::SPATIAL_INVALID_POSITION))),
    }
}

fn invalid(clause: &str, detail: &str) -> crate::error::SqlError {
    spatial(SqlState::SPATIAL_INVALID_ARGUMENT, clause, detail)
}

/// The shell `v` holds, of `g`'s reference system, with x, y and z.
///
/// # Errors
///
/// SQLSTATE 2FF10 for another system; 2FF73 without z; 2FF97 with m;
/// 2FF02 if it is no surface or no shell.
fn shell_of(g: &Geom, v: &Value) -> Result<Surface> {
    let s = linear(&Geom::from_value(v)?)?;
    if s.srid != g.srid {
        return Err(spatial(SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS, "§9.2.3 Description 4c", "mixed spatial reference systems"));
    }
    if !s.dims.has_z() {
        return Err(spatial(SqlState::SPATIAL_GEOMETRY_IS_NOT_3D, "§9.2", "a shell is 3D"));
    }
    if s.dims.has_m() {
        return Err(spatial(SqlState::SPATIAL_M_COORDINATES_NOT_ALLOWED, "§9.2", "a shell has no m coordinates"));
    }
    let Body::Surface(surface) = &s.body else { return Err(invalid("§9.2.3 Description 4d", "a shell is a surface")) };
    if s.is_empty() || !surface_is_closed(&s) || !patches_simple(&[surface], s.dims)? {
        return Err(invalid("§9.2.3 Description 4d", "the surface is not a shell"));
    }
    Ok(surface.clone())
}

/// §9.2.3 ST_ExteriorShell(ST_Surface) and §9.2.4
/// ST_InteriorShells(ST_Surface ARRAY): SELF with the shell, or the
/// array of shells, `v`, as `exterior` says.
///
/// # Errors
///
/// As `shell_of`; SQLSTATE 2FF03 for an array holding the null value;
/// 2FF98 for interior shells of a solid with no exterior one; 2FF02 for
/// shells that meet, or an interior shell outside the exterior one or
/// inside another.
pub fn replaced(g: &Geom, v: &Value, exterior: bool) -> Result<Value> {
    let mut all: Vec<Surface> = shells(g).to_vec();
    if exterior {
        let shell = shell_of(g, v)?;
        if all.is_empty() {
            all.push(shell);
        } else if let Some(slot) = all.first_mut() {
            *slot = shell;
        }
    } else {
        let Value::Array(items) = v else { return Err(invalid("§9.2.4", "the shells are an array")) };
        if items.iter().any(Value::is_null) {
            return Err(spatial(SqlState::SPATIAL_NULL_ARGUMENT, "§18.1.1", "the array holds the null value"));
        }
        let Some(outer) = all.first().cloned() else {
            return Err(spatial(SqlState::SPATIAL_NULL_EXTERIOR_SHELL, "§9.2.4 Description 4a", "the solid has no exterior shell"));
        };
        all = std::iter::once(Ok(outer)).chain(items.iter().map(|i| shell_of(g, i))).collect::<Result<_>>()?;
    }
    let solid = Geom::new(g.srid, Dims::Xyz, Body::Solid(all))?;
    check_shells(&solid)?;
    Ok(solid.to_value())
}

/// Whether `g` holds something of a dimension above 0.
fn beyond_points(g: &Geom) -> bool {
    match &g.body {
        Body::Point(_) | Body::Collection(Collection::MultiPoint(_)) => false,
        Body::Collection(Collection::Geometry(parts)) => parts.iter().any(|p| beyond_points(&g.part(p.clone()))),
        Body::Curve(_)
        | Body::Surface(_)
        | Body::Solid(_)
        | Body::Collection(
            Collection::MultiCurve(_) | Collection::MultiLineString(_) | Collection::MultiSurface(_) | Collection::MultiPolygon(_),
        ) => !g.is_empty(),
    }
}

/// §9.2.4 Description 4f to 4i of the shells of `solid`.
///
/// # Errors
///
/// SQLSTATE 2FF02 if two shells meet in more than points, an interior
/// shell is not within the exterior one, or two interior shells' solids
/// overlap.
fn check_shells(solid: &Geom) -> Result<()> {
    let all = shells(solid);
    let surface = |s: &Surface| solid.part(Body::Surface(s.clone()));
    let filled = |s: &Surface| solid.part(Body::Solid(vec![s.clone()]));
    for (at, one) in all.iter().enumerate() {
        for two in all.iter().skip(at.saturating_add(1)) {
            if beyond_points(&set_op(SetOp::Intersection, &surface(one), &surface(two))?) {
                return Err(invalid("§9.2.4 Description 4f", "two shells meet in more than points"));
            }
            if at > 0 && volume(&set_op(SetOp::Intersection, &filled(one), &filled(two))?)? > 0.0_f64 {
                return Err(invalid("§9.2.4 Description 4h", "two interior shells overlap"));
            }
        }
    }
    let Some(outer) = all.first() else { return Ok(()) };
    for inner in all.iter().skip(1) {
        if !set_op(SetOp::Difference, &filled(inner), &filled(outer))?.is_empty() {
            return Err(invalid("§9.2.4 Description 4g", "an interior shell is not within the exterior shell"));
        }
    }
    Ok(())
}

/// §8.6.4 ST_MaxSideLength() and ST_MaxSideLength(DOUBLE PRECISION,
/// INTEGER).
///
/// # Errors
///
/// As the TIN engine.
pub fn max_side_length(g: &Geom, rest: &[Value]) -> Result<Value> {
    let [length, again] = rest else {
        let Body::Surface(Surface::Tin(tin)) = &g.body else { return Ok(Value::Null) };
        let held = tin.max_side_length.filter(|_| !g.is_empty());
        return Ok(held.map_or(Value::Null, Value::Approx));
    };
    Ok(with_max_side(g, number(length)?, integer(again)? == 1)?.to_value())
}

/// §8.6.6 ST_Clip(ST_Polygon).
///
/// # Errors
///
/// SQLSTATE 2FF10 for a polygon of another system; 2FF02 for no polygon
/// or an empty one; as the TIN engine.
pub fn clip(g: &Geom, rest: &[Value]) -> Result<Value> {
    let window = Geom::from_value(rest.first().unwrap_or(&Value::Null))?;
    if window.srid != g.srid {
        return Err(spatial(SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS, "§8.6.6", "mixed spatial reference systems"));
    }
    let Body::Surface(Surface::Polygon(rings)) = &window.body else {
        return Err(invalid("§8.6.6 Description 1a", "the clip is a polygon"));
    };
    if rings.is_empty() {
        return Err(invalid("§8.6.6 Description 1a", "the clip polygon is empty"));
    }
    let stride = window.dims.stride();
    let plan: Vec<Vec<[f64; 2]>> = rings
        .iter()
        .map(|r| {
            r.chunks(stride)
                .filter_map(|p| match p {
                    [x, y, ..] => Some([*x, *y]),
                    _ => None,
                })
                .collect()
        })
        .collect();
    Ok(clip_tin(g, &plan)?.to_value())
}

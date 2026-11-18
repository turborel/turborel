//! The routines' calls: each decodes its values, takes their engine
//! forms and gives the clause's result.

use super::super::engine::{engine_of, geom_of};
use super::super::model::{Body, Collection, Dims, Geom, Kind, cast};
use super::super::observers::{Exact, exact};
use super::super::{SpatialOperation, invalid};
use super::units::Systems;
use super::{Routine, results, units};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::spatial::planar::{self, Geometry, Operation, Point};
use crate::value::{Decimal, Value};

pub(super) fn name(routine: Routine) -> &'static str {
    match routine {
        Routine::Relate => "ST_RELATE",
        Routine::Equals => "ST_EQUALS",
        Routine::Touches => "ST_TOUCHES",
        Routine::Crosses => "ST_CROSSES",
        Routine::Within => "ST_WITHIN",
        Routine::Contains => "ST_CONTAINS",
        Routine::Overlaps => "ST_OVERLAPS",
        Routine::Intersection => "ST_INTERSECTION",
        Routine::Union => "ST_UNION",
        Routine::Difference => "ST_DIFFERENCE",
        Routine::SymDifference => "ST_SYMDIFFERENCE",
        Routine::Buffer => "ST_BUFFER",
        Routine::ConvexHull => "ST_CONVEXHULL",
        Routine::Boundary => "ST_BOUNDARY",
        Routine::IsValid => "ST_ISVALID",
        Routine::IsSimple => "ST_ISSIMPLE",
        Routine::IsRing => "ST_ISRING",
        Routine::Area => "ST_AREA",
        Routine::Length => "ST_LENGTH",
        Routine::Perimeter => "ST_PERIMETER",
        Routine::Centroid => "ST_CENTROID",
        Routine::PointOnSurface => "ST_POINTONSURFACE",
        Routine::BdPolyFromText => "ST_BDPOLYFROMTEXT",
        Routine::BdPolyFromWkb => "ST_BDPOLYFROMWKB",
        Routine::BdMPolyFromText => "ST_BDMPOLYFROMTEXT",
        Routine::BdMPolyFromWkb => "ST_BDMPOLYFROMWKB",
    }
}

/// What an index may find rows by: a relationship true only of operands
/// that meet, or equality, true also of two empty sets.
pub(super) fn spatial_operation(routine: Routine) -> Option<SpatialOperation> {
    match routine {
        Routine::Touches | Routine::Crosses | Routine::Within | Routine::Contains | Routine::Overlaps => Some(SpatialOperation::Meets),
        Routine::Equals => Some(SpatialOperation::Equals),
        Routine::Relate
        | Routine::Intersection
        | Routine::Union
        | Routine::Difference
        | Routine::SymDifference
        | Routine::Buffer
        | Routine::ConvexHull
        | Routine::Boundary
        | Routine::IsValid
        | Routine::IsSimple
        | Routine::IsRing
        | Routine::Area
        | Routine::Length
        | Routine::Perimeter
        | Routine::Centroid
        | Routine::PointOnSurface
        | Routine::BdPolyFromText
        | Routine::BdPolyFromWkb
        | Routine::BdMPolyFromText
        | Routine::BdMPolyFromWkb => None,
    }
}

fn integer(value: bool) -> Value {
    Value::Exact(Decimal::new(i128::from(value), 0))
}

fn truth(value: Option<bool>) -> Value {
    value.map_or(Value::Null, integer)
}

fn number(value: Option<f64>) -> Value {
    value.map_or(Value::Null, Value::Approx)
}

fn text(value: &Value) -> Result<&str> {
    let Value::Char(text) = value else { return Err(invalid("expected a character string")) };
    Ok(text)
}

fn double(value: &Value) -> Result<f64> {
    value.as_f64().ok_or_else(|| invalid("expected a number"))
}

/// Two values of one spatial reference system.
fn pair(first: &Value, second: &Value) -> Result<(Geom, Geom)> {
    let (one, two) = (Geom::from_value(first)?, Geom::from_value(second)?);
    if one.srid != two.srid {
        return Err(invalid("spatial operands must have the same SRID"));
    }
    Ok((one, two))
}

pub(super) fn call(routine: Routine, args: &[Value], systems: Systems<'_>) -> Result<Value> {
    match (routine, args) {
        (Routine::Relate, [first, second, matrix]) => relate(first, second, text(matrix)?),
        (
            Routine::Equals | Routine::Touches | Routine::Crosses | Routine::Within | Routine::Contains | Routine::Overlaps,
            [first, second],
        ) => relation(routine, first, second),
        (Routine::Intersection, [first, second]) => operation(Operation::Intersection, first, second),
        (Routine::Union, [first, second]) => operation(Operation::Union, first, second),
        (Routine::Difference, [first, second]) => operation(Operation::Difference, first, second),
        (Routine::SymDifference, [first, second]) => operation(Operation::SymDifference, first, second),
        (Routine::BdPolyFromText | Routine::BdPolyFromWkb, _) => rings(routine, args, Kind::Polygon),
        (Routine::BdMPolyFromText | Routine::BdMPolyFromWkb, _) => rings(routine, args, Kind::MultiPolygon),
        (other, [subject, rest @ ..]) => one(other, &Geom::from_value(subject)?, rest, systems),
        (other, []) => Err(super::signature(name(other))),
    }
}

/// ST_Relate: the null value of an empty operand, else whether the
/// DE-9IM matches the matrix, whose characters are checked [§5.1.45
/// Description 2].
fn relate(first: &Value, second: &Value, matrix: &str) -> Result<Value> {
    let (one, two) = pair(first, second)?;
    if one.is_empty() || two.is_empty() {
        return Ok(Value::Null);
    }
    let pattern = planar::Pattern::parse(matrix).map_err(|e| e.with_state(SqlState::SPATIAL_INVALID_INTERSECTION_MATRIX))?;
    Ok(truth(planar::relate(&engine_of(&one)?, &engine_of(&two)?)?.map(|m| m.matches(&pattern))))
}

fn relation(routine: Routine, first: &Value, second: &Value) -> Result<Value> {
    let (one, two) = pair(first, second)?;
    let (a, b) = (engine_of(&one)?, engine_of(&two)?);
    let decided = match routine {
        Routine::Equals => planar::equals(&a, &b),
        Routine::Touches => planar::touches(&a, &b),
        Routine::Crosses => planar::crosses(&a, &b),
        Routine::Within => planar::within(&a, &b),
        Routine::Contains => planar::contains(&a, &b),
        Routine::Overlaps => planar::overlaps(&a, &b),
        Routine::Relate
        | Routine::Intersection
        | Routine::Union
        | Routine::Difference
        | Routine::SymDifference
        | Routine::Buffer
        | Routine::ConvexHull
        | Routine::Boundary
        | Routine::IsValid
        | Routine::IsSimple
        | Routine::IsRing
        | Routine::Area
        | Routine::Length
        | Routine::Perimeter
        | Routine::Centroid
        | Routine::PointOnSurface
        | Routine::BdPolyFromText
        | Routine::BdPolyFromWkb
        | Routine::BdMPolyFromText
        | Routine::BdMPolyFromWkb => return Err(super::signature(name(routine))),
    }?;
    Ok(truth(decided))
}

/// A set operation, in SELF's system, typed by Tables 8 to 11.
fn operation(operation: Operation, first: &Value, second: &Value) -> Result<Value> {
    let (one, two) = pair(first, second)?;
    if let Some(result) = results::of_empty(operation, &one, &two) {
        return Ok(result.to_value());
    }
    let result = planar::overlay(&engine_of(&one)?, &engine_of(&two)?, operation)?;
    Ok(results::typed(operation, &one, &two, geom_of(&result, one.srid)?)?.to_value())
}

/// The point of SRID `srid`, a centroid or a point on a surface.
fn point(at: Option<Point>, srid: i32) -> Result<Value> {
    let Some([x, y]) = at else { return Ok(Value::Null) };
    Ok(Geom::new(srid, Dims::Xy, Body::Point(vec![x, y]))?.to_value())
}

/// Whether the value's type is one the routine is declared for; a
/// function call of another is an invalid argument.
fn declared_for(routine: Routine, g: &Geom) -> Result<()> {
    let kind = g.kind();
    if routine.subjects().iter().any(|wanted| kind.is_subtype_of(*wanted)) {
        return Ok(());
    }
    Err(SqlError::new(Code::Data, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1] {} takes no {}", name(routine), kind.sql_name()))
        .with_state(SqlState::SPATIAL_INVALID_ARGUMENT))
}

/// A measure of the value, in `unit` if one is given.
fn measure(value: Option<f64>, (g, systems): (&Geom, Systems<'_>), unit: Option<&Value>, area: bool) -> Result<Value> {
    let (Some(value), Some(unit)) = (value, unit) else { return Ok(number(value)) };
    let (name, of) = (text(unit)?, (g.srid, systems));
    Ok(Value::Approx(if area { units::area_in(value, of, name)? } else { units::length_in(value, of, name)? }))
}

fn one(routine: Routine, g: &Geom, rest: &[Value], systems: Systems<'_>) -> Result<Value> {
    declared_for(routine, g)?;
    let empty = g.is_empty();
    match routine {
        Routine::IsValid => return Ok(integer(empty || planar::is_valid(&engine_of(g)?)?)),
        Routine::IsSimple => return Ok(integer(empty || planar::is_simple(&engine_of(g)?)?)),
        Routine::IsRing => return Ok(integer(!empty && ring(g)?)),
        Routine::Relate
        | Routine::Equals
        | Routine::Touches
        | Routine::Crosses
        | Routine::Within
        | Routine::Contains
        | Routine::Overlaps
        | Routine::Intersection
        | Routine::Union
        | Routine::Difference
        | Routine::SymDifference
        | Routine::Buffer
        | Routine::ConvexHull
        | Routine::Boundary
        | Routine::Area
        | Routine::Length
        | Routine::Perimeter
        | Routine::Centroid
        | Routine::PointOnSurface
        | Routine::BdPolyFromText
        | Routine::BdPolyFromWkb
        | Routine::BdMPolyFromText
        | Routine::BdMPolyFromWkb => (),
    }
    if empty {
        return Ok(Value::Null);
    }
    observe(routine, (g, systems), &engine_of(g)?, rest)
}

/// Of a nonempty value, what the routine observes or builds.
fn observe(routine: Routine, (g, systems): (&Geom, Systems<'_>), planar: &Geometry, rest: &[Value]) -> Result<Value> {
    let srid = g.srid;
    let built = |result: &Geometry| geom_of(result, srid).map(|value| value.to_value());
    match routine {
        Routine::Buffer => {
            let (distance, unit) = (double(rest.first().ok_or_else(|| invalid("a buffer needs a distance"))?)?, rest.get(1));
            let distance = match unit {
                Some(name) => units::length_of(distance, (srid, systems), text(name)?)?,
                None => distance,
            };
            built(&planar::buffer(planar, distance)?)
        }
        Routine::ConvexHull => built(&planar::convex_hull(planar)),
        Routine::Boundary => built(&planar::boundary(planar)?),
        Routine::Area => measure(exact(Exact::Area, g, || planar::area(planar))?, (g, systems), rest.first(), true),
        Routine::Length => measure(exact(Exact::Length, g, || planar::length(planar))?, (g, systems), rest.first(), false),
        Routine::Perimeter => measure(exact(Exact::Perimeter, g, || planar::perimeter(planar))?, (g, systems), rest.first(), false),
        Routine::Centroid => point(planar::centroid(planar), srid),
        Routine::PointOnSurface => point(planar::point_on_surface(planar), srid),
        Routine::Relate
        | Routine::Equals
        | Routine::Touches
        | Routine::Crosses
        | Routine::Within
        | Routine::Contains
        | Routine::Overlaps
        | Routine::Intersection
        | Routine::Union
        | Routine::Difference
        | Routine::SymDifference
        | Routine::IsValid
        | Routine::IsSimple
        | Routine::IsRing
        | Routine::BdPolyFromText
        | Routine::BdPolyFromWkb
        | Routine::BdMPolyFromText
        | Routine::BdMPolyFromWkb => Err(super::signature(name(routine))),
    }
}

/// ST_IsRing of a nonempty curve: simple and closed [§7.1.8], of x and y.
fn ring(g: &Geom) -> Result<bool> {
    let line = engine_of(g)?;
    let Geometry::Line(points) = &line else { return Ok(false) };
    let closed = points.first().zip(points.last()).is_some_and(|(first, last)| planar::same(*first, *last));
    Ok(closed && planar::is_simple(&line)?)
}

/// ST_BdPolyFromText and its peers: the multilinestring the text or
/// binary gives, its rings made into a polygon or multipolygon [§8.3.9,
/// §8.3.10, §10.6.7, §10.6.8].
fn rings(routine: Routine, args: &[Value], target: Kind) -> Result<Value> {
    let super::Shape::Rings(constructor, _) = routine.shape() else { return Err(super::signature(name(routine))) };
    let read = crate::functions::ScalarFunction::call(&constructor, args)?;
    let lines = Geom::from_value(&read)?;
    let Body::Collection(Collection::MultiLineString(_)) = &lines.body else {
        let state = if matches!(routine, Routine::BdPolyFromWkb | Routine::BdMPolyFromWkb) {
            SqlState::SPATIAL_INVALID_WKB
        } else {
            SqlState::SPATIAL_INVALID_WKT
        };
        return Err(SqlError::new(
            Code::Data,
            format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.3.9] {} takes a multilinestring", name(routine)),
        )
        .with_state(state));
    };
    let xy = lines.xy();
    let Body::Collection(Collection::MultiLineString(flat)) = &xy.body else { return Err(super::signature(name(routine))) };
    let rings: Vec<Vec<Point>> = flat.iter().map(|coords| coords.as_chunks::<2>().0.to_vec()).collect();
    let built = if target == Kind::Polygon { planar::polygon_from_rings(&rings) } else { planar::multipolygon_from_rings(&rings) };
    let built = built.map_err(|e| if e.code == Code::Data { e.with_state(SqlState::SPATIAL_INVALID_ARGUMENT) } else { e })?;
    Ok(cast(&geom_of(&built, lines.srid)?, target)?.to_value())
}

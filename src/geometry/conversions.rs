//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.55] The cast methods of
//! ST_Geometry, one for each instantiable type: ST_ToPoint to
//! ST_ToMultiPolygon. Each returns SELF if it is of the type, a value
//! made of SELF by its own rule b, or the one element of a collection
//! (or a compound curve) cast by the method again. Any other value that
//! is not empty raises 2FF16, and the empty set gives the type's empty
//! value of SELF's reference system. Each is a method of ST_Geometry
//! (`methods.rs`) and a function of SELF.

use super::collections::collected;
use super::model::{Body, Collection, Curve, Geom, Kind, Surface, empty_body};
use super::observers::{CurveMethod, RingMethod, Routine};
use super::representations::spatial;
use super::{data_type, data_type_of, elements, kind, of_data_type};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::ScalarFunction;
use crate::value::{DataType, Value};

/// A cast method: its name and the type it casts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToKind {
    /// Its name, upper case.
    pub name: &'static str,
    /// The type of its result.
    pub target: Kind,
}

const fn to(name: &'static str, target: Kind) -> ToKind {
    ToKind { name, target }
}

/// [§5.1.1, §5.1.55] The cast methods: the 22 ST_Geometry's type
/// definition declares, ST_ToTriangle, which §5.1.55 defines (Description
/// 37) and §23.3 requires, and ST_ToNURBSCurve and ST_ToSpiralCurve, the
/// spellings §5.1.55 defines ST_ToNURBS and ST_ToSpiral by.
pub const CONVERSIONS: [ToKind; 25] = [
    to("ST_TOPOINT", Kind::Point),
    to("ST_TOLINESTRING", Kind::LineString),
    to("ST_TOCIRCULAR", Kind::CircularString),
    to("ST_TOCIRCLE", Kind::Circle),
    to("ST_TOGEODESIC", Kind::GeodesicString),
    to("ST_TOELLIPTICAL", Kind::EllipticalCurve),
    to("ST_TONURBS", Kind::NurbsCurve),
    to("ST_TONURBSCURVE", Kind::NurbsCurve),
    to("ST_TOCLOTHOID", Kind::Clothoid),
    to("ST_TOSPIRAL", Kind::SpiralCurve),
    to("ST_TOSPIRALCURVE", Kind::SpiralCurve),
    to("ST_TOCOMPOUND", Kind::CompoundCurve),
    to("ST_TOCURVEPOLY", Kind::CurvePolygon),
    to("ST_TOPOLYGON", Kind::Polygon),
    to("ST_TOTRIANGLE", Kind::Triangle),
    to("ST_TOPOLYHDRLSURF", Kind::PolyhdrlSurface),
    to("ST_TOTIN", Kind::Tin),
    to("ST_TOCOMPSURFACE", Kind::CompoundSurface),
    to("ST_TOBREPSOLID", Kind::BRepSolid),
    to("ST_TOGEOMCOLL", Kind::GeomCollection),
    to("ST_TOMULTIPOINT", Kind::MultiPoint),
    to("ST_TOMULTICURVE", Kind::MultiCurve),
    to("ST_TOMULTILINE", Kind::MultiLineString),
    to("ST_TOMULTISURFACE", Kind::MultiSurface),
    to("ST_TOMULTIPOLYGON", Kind::MultiPolygon),
];

fn signature(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.55] invalid arguments to {name}"))
}

/// [§5.1.55] `g` cast to `target` by its method.
///
/// # Errors
///
/// SQLSTATE 2FF16 if no rule takes `g` and it is not empty; 2FF08 if a
/// collection's elements are of no type the multi type holds; as
/// ST_CurveToLine and ST_CurvePolyToPoly.
pub fn converted(g: &Geom, target: Kind) -> Result<Geom> {
    if g.kind().is_subtype_of(target) {
        return Ok(g.clone());
    }
    if g.is_empty() {
        let body = empty_body(target).ok_or_else(|| signature("a cast method"))?;
        return Ok(g.part(body));
    }
    if let Some(made) = made(g, target)? {
        return Ok(made);
    }
    match only_element(g, target) {
        Some(one) => converted(&g.part(one), target),
        None => Err(spatial(SqlState::SPATIAL_NOT_AN_EMPTY_SET, "§5.1.55", "not an empty set")),
    }
}

/// The type the one element of a collection is of for the method of
/// `target` to cast it, and whether a compound curve of one element of
/// it is cast too; none of a method without that rule.
fn element_rule(target: Kind) -> Option<(Kind, bool)> {
    Some(match target {
        Kind::LineString | Kind::CompoundCurve => (Kind::Curve, false),
        Kind::CircularString
        | Kind::Circle
        | Kind::GeodesicString
        | Kind::EllipticalCurve
        | Kind::NurbsCurve
        | Kind::Clothoid
        | Kind::SpiralCurve => (target, true),
        Kind::CurvePolygon | Kind::Polygon => (Kind::CurvePolygon, false),
        Kind::CompoundSurface => (Kind::Surface, false),
        Kind::Point | Kind::Triangle | Kind::PolyhdrlSurface | Kind::Tin | Kind::BRepSolid => (target, false),
        Kind::Geometry
        | Kind::Curve
        | Kind::Surface
        | Kind::Solid
        | Kind::GeomCollection
        | Kind::MultiPoint
        | Kind::MultiCurve
        | Kind::MultiLineString
        | Kind::MultiSurface
        | Kind::MultiPolygon => return None,
    })
}

/// The one element of a collection, or of a compound curve where the
/// rule takes one, if it is of the type the rule names.
fn only_element(g: &Geom, target: Kind) -> Option<Body> {
    let (of, compound) = element_rule(target)?;
    let all = if let Body::Curve(Curve::Compound(parts)) = &g.body {
        parts.iter().cloned().map(Body::Curve).filter(|_| compound).collect()
    } else {
        elements(g)?
    };
    let [one] = <[Body; 1]>::try_from(all).ok()?;
    one.kind().is_subtype_of(of).then_some(one)
}

/// [§5.1.55 rules b] The value a method makes of SELF of another type:
/// a curve's line string, a curve polygon's polygon, a compound or
/// collection holding SELF, or a multi type of a collection's elements.
fn made(g: &Geom, target: Kind) -> Result<Option<Geom>> {
    let wrapped = |body: Body| Geom::new(g.srid, g.dims, body).map(Some);
    let source = g.kind();
    match target {
        Kind::LineString => source.is_subtype_of(Kind::Curve).then(|| line(g)).transpose(),
        Kind::CompoundCurve => match &g.body {
            Body::Curve(c) => wrapped(Body::Curve(Curve::Compound(vec![c.clone()]))),
            Body::Point(_) | Body::Surface(_) | Body::Solid(_) | Body::Collection(_) => Ok(None),
        },
        Kind::Polygon => (source == Kind::CurvePolygon).then(|| polygon(g)).transpose(),
        Kind::CompoundSurface => match &g.body {
            Body::Surface(s) => wrapped(Body::Surface(Surface::Compound(vec![s.clone()]))),
            Body::Point(_) | Body::Curve(_) | Body::Solid(_) | Body::Collection(_) => Ok(None),
        },
        Kind::GeomCollection => match &g.body {
            Body::Point(_) | Body::Curve(_) | Body::Surface(_) => wrapped(Body::Collection(Collection::Geometry(vec![g.body.clone()]))),
            Body::Solid(_) | Body::Collection(_) => Ok(None),
        },
        Kind::MultiPoint | Kind::MultiCurve | Kind::MultiLineString | Kind::MultiSurface | Kind::MultiPolygon => {
            let Some(bodies) = multi_elements(g, source, target)? else { return Ok(None) };
            wrapped(Body::Collection(collected(target, bodies)?))
        }
        Kind::Geometry
        | Kind::Point
        | Kind::Curve
        | Kind::CircularString
        | Kind::Circle
        | Kind::GeodesicString
        | Kind::EllipticalCurve
        | Kind::NurbsCurve
        | Kind::Clothoid
        | Kind::SpiralCurve
        | Kind::Surface
        | Kind::CurvePolygon
        | Kind::Triangle
        | Kind::PolyhdrlSurface
        | Kind::Tin
        | Kind::Solid
        | Kind::BRepSolid => Ok(None),
    }
}

/// [§5.1.55 Description 56 to 68] The elements of the multi type
/// `target` made of `g`: a collection's, or SELF, of the multi type's
/// element type, as a line string or a polygon where the multi type
/// holds those.
fn multi_elements(g: &Geom, source: Kind, target: Kind) -> Result<Option<Vec<Body>>> {
    if let Some(all) = elements(g) {
        return Ok(Some(all));
    }
    let one = match target {
        Kind::MultiPoint => source.is_subtype_of(Kind::Point).then(|| g.body.clone()),
        Kind::MultiCurve => source.is_subtype_of(Kind::Curve).then(|| g.body.clone()),
        Kind::MultiLineString => {
            if source.is_subtype_of(Kind::Curve) {
                Some(line(g)?.body)
            } else {
                None
            }
        }
        Kind::MultiSurface => source.is_subtype_of(Kind::Surface).then(|| g.body.clone()),
        Kind::MultiPolygon => {
            if source.is_subtype_of(Kind::CurvePolygon) {
                Some(converted(g, Kind::Polygon)?.body)
            } else {
                None
            }
        }
        Kind::Geometry
        | Kind::Point
        | Kind::Curve
        | Kind::LineString
        | Kind::CircularString
        | Kind::Circle
        | Kind::GeodesicString
        | Kind::EllipticalCurve
        | Kind::NurbsCurve
        | Kind::Clothoid
        | Kind::SpiralCurve
        | Kind::CompoundCurve
        | Kind::Surface
        | Kind::CurvePolygon
        | Kind::Polygon
        | Kind::Triangle
        | Kind::PolyhdrlSurface
        | Kind::Tin
        | Kind::CompoundSurface
        | Kind::Solid
        | Kind::BRepSolid
        | Kind::GeomCollection => None,
    };
    Ok(one.map(|b| vec![b]))
}

/// The value one of the observers returns of `g`.
fn observed(f: &dyn ScalarFunction, g: &Geom) -> Result<Geom> {
    Geom::from_value(&f.call(&[g.to_value()])?)
}

/// [§7.1.10] SELF.ST_CurveToLine().
fn line(g: &Geom) -> Result<Geom> {
    observed(&Routine(CurveMethod::CurveToLine), g)
}

/// [§8.2.7] SELF.ST_CurvePolyToPoly().
fn polygon(g: &Geom) -> Result<Geom> {
    observed(&Routine(RingMethod::CurvePolyToPoly), g)
}

impl ScalarFunction for ToKind {
    fn name(&self) -> &'static str {
        self.name
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [first] = args else { return Err(signature(self.name)) };
        of_data_type(first).map(|_| data_type_of(self.target)).ok_or_else(|| signature(self.name))
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        (position == 0).then(|| known.first().cloned().flatten().filter(|t| of_data_type(t).is_some()).unwrap_or_else(data_type))
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let [first] = args else { return Err(signature(self.name)) };
        if first.is_null() {
            return Ok(Value::Null);
        }
        kind::payload(first).ok_or_else(|| signature(self.name))?;
        Ok(converted(&Geom::from_value(first)?, self.target)?.to_value())
    }

    fn deterministic(&self) -> bool {
        true
    }
}

//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5 to §10; ISO/IEC 9075-2:2023
//! §9.18, §9.19] The methods of the geometry types, a table of them for
//! each type: the methods its descriptor declares, original or
//! overriding, each with the routine that runs it. A method invocation
//! `v.m(args)` binds to the subject routine its declared type's type
//! precedence list finds first [§9.18 SR10b], and runs, of that method
//! and those that override it, the one the value's most specific type
//! finds first [§9.18 GR3b]; `(v AS T).m(args)` runs the one T finds
//! [GR3b ii 1 D I] (RULES.md D-SPATIAL-FAMILY).
//!
//! A worker adding a method adds its row here; one overriding a method
//! adds a row of its own type whose routine is its own.

use std::sync::Arc;

use super::collections::CollectionFunction;
use super::functions::GeometryFunction;
use super::model::Kind;
use super::observers::{
    CompoundMethod, CurveMethod, MeasureMethod, ParameterMethod, PatchMethod, PointListMethod, RingMethod, Routine, SurfaceMethod,
};
use super::representations::AsGml;
use super::tins::TinRoutine;
use crate::error::{Result, SqlError, SqlState};
use crate::functions::{ScalarFunction, ScalarRef};
use crate::value::{DataType, Value};

/// A method of a geometry type.
#[derive(Clone, Copy)]
pub struct Method {
    /// The type whose descriptor declares it.
    pub of: Kind,
    /// Its name, as a `<method name>` folds to upper case.
    pub name: &'static str,
    /// The number of its parameters after SELF.
    pub arity: usize,
    /// Whether it overrides the method of its name and parameters of a
    /// proper supertype [ISO/IEC 9075-2:2023 §11.51 SR8].
    pub overriding: bool,
    /// The routine that runs it, of SELF and the other arguments.
    pub routine: fn() -> Arc<dyn ScalarFunction>,
}

impl std::fmt::Debug for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}/{}", self.of.sql_name(), self.name, self.arity)
    }
}

/// The method `name` of `arity` of `of` that `routine` runs.
const fn original(of: Kind, name: &'static str, arity: usize, routine: fn() -> Arc<dyn ScalarFunction>) -> Method {
    Method { of, name, arity, overriding: false, routine }
}

/// As `original`, overriding.
const fn overriding(of: Kind, name: &'static str, arity: usize, routine: fn() -> Arc<dyn ScalarFunction>) -> Method {
    Method { of, name, arity, overriding: true, routine }
}

macro_rules! routine {
    ($f:expr) => {{
        fn made() -> Arc<dyn ScalarFunction> {
            Arc::new($f)
        }
        made
    }};
}

/// The table: of each type, the methods it declares.
pub static METHODS: [Method; 107] = [
    // [§5.1.1] ST_Geometry.
    original(Kind::Geometry, "ST_SRID", 0, routine!(GeometryFunction::Srid)),
    original(Kind::Geometry, "ST_SRID", 1, routine!(GeometryFunction::SetSrid)),
    original(Kind::Geometry, "ST_ISEMPTY", 0, routine!(GeometryFunction::IsEmpty)),
    original(Kind::Geometry, "ST_GEOMETRYTYPE", 0, routine!(GeometryFunction::TypeName)),
    original(Kind::Geometry, "ST_ASTEXT", 0, routine!(GeometryFunction::AsText)),
    original(Kind::Geometry, "ST_ASBINARY", 0, routine!(GeometryFunction::AsBinary)),
    original(Kind::Geometry, "ST_INTERSECTS", 1, routine!(GeometryFunction::Intersects)),
    original(Kind::Geometry, "ST_DISJOINT", 1, routine!(GeometryFunction::Disjoint)),
    original(Kind::Geometry, "ST_DISTANCE", 1, routine!(GeometryFunction::Distance)),
    original(Kind::Geometry, "ST_MINX", 0, routine!(GeometryFunction::MinX)),
    original(Kind::Geometry, "ST_MAXX", 0, routine!(GeometryFunction::MaxX)),
    original(Kind::Geometry, "ST_MINY", 0, routine!(GeometryFunction::MinY)),
    original(Kind::Geometry, "ST_MAXY", 0, routine!(GeometryFunction::MaxY)),
    // [§6.1.1] ST_Point.
    original(Kind::Point, "ST_X", 0, routine!(GeometryFunction::X)),
    original(Kind::Point, "ST_Y", 0, routine!(GeometryFunction::Y)),
    // [§10.1.1] ST_GeomCollection.
    original(Kind::GeomCollection, "ST_NUMGEOMETRIES", 0, routine!(CollectionFunction::NumGeometries)),
    original(Kind::GeomCollection, "ST_GEOMETRYN", 1, routine!(CollectionFunction::GeometryN)),
    original(Kind::GeomCollection, "ST_GEOMETRIES", 0, routine!(CollectionFunction::Geometries(Kind::GeomCollection))),
    original(Kind::GeomCollection, "ST_GEOMETRIES", 1, routine!(CollectionFunction::Geometries(Kind::GeomCollection))),
    // [§10.2.1] ST_MultiPoint.
    overriding(Kind::MultiPoint, "ST_GEOMETRIES", 0, routine!(CollectionFunction::Geometries(Kind::MultiPoint))),
    overriding(Kind::MultiPoint, "ST_GEOMETRIES", 1, routine!(CollectionFunction::Geometries(Kind::MultiPoint))),
    // [§10.3.1] ST_MultiCurve.
    overriding(Kind::MultiCurve, "ST_GEOMETRIES", 0, routine!(CollectionFunction::Geometries(Kind::MultiCurve))),
    overriding(Kind::MultiCurve, "ST_GEOMETRIES", 1, routine!(CollectionFunction::Geometries(Kind::MultiCurve))),
    // [§10.4.1] ST_MultiLineString.
    overriding(Kind::MultiLineString, "ST_GEOMETRIES", 0, routine!(CollectionFunction::Geometries(Kind::MultiLineString))),
    overriding(Kind::MultiLineString, "ST_GEOMETRIES", 1, routine!(CollectionFunction::Geometries(Kind::MultiLineString))),
    // [§10.5.1] ST_MultiSurface.
    overriding(Kind::MultiSurface, "ST_GEOMETRIES", 0, routine!(CollectionFunction::Geometries(Kind::MultiSurface))),
    overriding(Kind::MultiSurface, "ST_GEOMETRIES", 1, routine!(CollectionFunction::Geometries(Kind::MultiSurface))),
    // [§10.6.1] ST_MultiPolygon.
    overriding(Kind::MultiPolygon, "ST_GEOMETRIES", 0, routine!(CollectionFunction::Geometries(Kind::MultiPolygon))),
    overriding(Kind::MultiPolygon, "ST_GEOMETRIES", 1, routine!(CollectionFunction::Geometries(Kind::MultiPolygon))),
    // [§5.1.13, §5.1.15, §5.1.57, §5.1.61] ST_Geometry, of the formats and measures.
    original(Kind::Geometry, "ST_ASGML", 0, routine!(AsGml)),
    original(Kind::Geometry, "ST_LOCATEALONG", 1, routine!(Routine(MeasureMethod::LocateAlong))),
    original(Kind::Geometry, "ST_LOCATEBETWEEN", 2, routine!(Routine(MeasureMethod::LocateBetween))),
    // [§7.1.1] ST_Curve.
    original(Kind::Curve, "ST_LENGTH", 0, routine!(Routine(CurveMethod::Length))),
    original(Kind::Curve, "ST_STARTPOINT", 0, routine!(Routine(CurveMethod::StartPoint))),
    original(Kind::Curve, "ST_ENDPOINT", 0, routine!(Routine(CurveMethod::EndPoint))),
    original(Kind::Curve, "ST_ISCLOSED", 0, routine!(Routine(CurveMethod::IsClosed))),
    original(Kind::Curve, "ST_ISRING", 0, routine!(Routine(CurveMethod::IsRing))),
    original(Kind::Curve, "ST_CURVETOLINE", 0, routine!(Routine(CurveMethod::CurveToLine))),
    original(Kind::Curve, "ST_DISTANCETOPOINT", 1, routine!(Routine(CurveMethod::DistanceToPoint))),
    original(Kind::Curve, "ST_POINTATDISTANCE", 1, routine!(Routine(CurveMethod::PointAtDistance))),
    original(Kind::Curve, "ST_PERPPOINTS", 1, routine!(Routine(CurveMethod::PerpPoints))),
    // [§7.2.1, §7.5.1] ST_LineString and ST_GeodesicString.
    original(Kind::LineString, "ST_POINTS", 0, routine!(Routine(PointListMethod::Points))),
    original(Kind::LineString, "ST_NUMPOINTS", 0, routine!(Routine(PointListMethod::NumPoints))),
    original(Kind::LineString, "ST_POINTN", 1, routine!(Routine(PointListMethod::PointN))),
    original(Kind::GeodesicString, "ST_POINTS", 0, routine!(Routine(PointListMethod::Points))),
    original(Kind::GeodesicString, "ST_NUMPOINTS", 0, routine!(Routine(PointListMethod::NumPoints))),
    original(Kind::GeodesicString, "ST_POINTN", 1, routine!(Routine(PointListMethod::PointN))),
    // [§7.3.1] ST_CircularString.
    original(Kind::CircularString, "ST_POINTS", 0, routine!(Routine(PointListMethod::Points))),
    original(Kind::CircularString, "ST_NUMPOINTS", 0, routine!(Routine(PointListMethod::NumPoints))),
    original(Kind::CircularString, "ST_POINTN", 1, routine!(Routine(PointListMethod::PointN))),
    original(Kind::CircularString, "ST_NUMSEGMENTS", 0, routine!(Routine(PointListMethod::NumSegments))),
    original(Kind::CircularString, "ST_SEGMENTN", 1, routine!(Routine(PointListMethod::SegmentN))),
    original(Kind::CircularString, "ST_MIDPOINTREP", 0, routine!(Routine(PointListMethod::MidPointRep))),
    original(Kind::CircularString, "ST_BULGE", 0, routine!(Routine(PointListMethod::Bulge))),
    original(Kind::CircularString, "ST_CENTER", 0, routine!(Routine(PointListMethod::Center))),
    original(Kind::CircularString, "ST_RADIUS", 0, routine!(Routine(PointListMethod::Radius))),
    // [§7.4.1] ST_Circle.
    original(Kind::Circle, "ST_POINTS", 0, routine!(Routine(PointListMethod::Points))),
    original(Kind::Circle, "ST_POINTN", 1, routine!(Routine(PointListMethod::PointN))),
    original(Kind::Circle, "ST_CENTER", 0, routine!(Routine(PointListMethod::Center))),
    original(Kind::Circle, "ST_RADIUS", 0, routine!(Routine(PointListMethod::Radius))),
    // [§7.6.1 to §7.9.1] The parametric curves.
    original(Kind::EllipticalCurve, "ST_STARTM", 0, routine!(Routine(ParameterMethod::StartM))),
    original(Kind::EllipticalCurve, "ST_ENDM", 0, routine!(Routine(ParameterMethod::EndM))),
    original(Kind::NurbsCurve, "ST_STARTM", 0, routine!(Routine(ParameterMethod::StartM))),
    original(Kind::NurbsCurve, "ST_ENDM", 0, routine!(Routine(ParameterMethod::EndM))),
    original(Kind::Clothoid, "ST_STARTM", 0, routine!(Routine(ParameterMethod::StartM))),
    original(Kind::Clothoid, "ST_ENDM", 0, routine!(Routine(ParameterMethod::EndM))),
    original(Kind::SpiralCurve, "ST_STARTM", 0, routine!(Routine(ParameterMethod::StartM))),
    original(Kind::SpiralCurve, "ST_ENDM", 0, routine!(Routine(ParameterMethod::EndM))),
    original(Kind::NurbsCurve, "ST_DEGREE", 0, routine!(Routine(ParameterMethod::Degree))),
    original(Kind::Clothoid, "ST_SCALEFACTOR", 0, routine!(Routine(ParameterMethod::ScaleFactor))),
    original(Kind::Clothoid, "ST_STARTDISTANCE", 0, routine!(Routine(ParameterMethod::StartDistance))),
    original(Kind::Clothoid, "ST_ENDDISTANCE", 0, routine!(Routine(ParameterMethod::EndDistance))),
    original(Kind::SpiralCurve, "ST_STARTCURVATURE", 0, routine!(Routine(ParameterMethod::StartCurvature))),
    original(Kind::SpiralCurve, "ST_ENDCURVATURE", 0, routine!(Routine(ParameterMethod::EndCurvature))),
    original(Kind::SpiralCurve, "ST_SPIRALTYPE", 0, routine!(Routine(ParameterMethod::SpiralType))),
    // [§7.10.1] ST_CompoundCurve.
    original(Kind::CompoundCurve, "ST_CURVES", 0, routine!(Routine(CompoundMethod::Curves))),
    original(Kind::CompoundCurve, "ST_NUMCURVES", 0, routine!(Routine(CompoundMethod::NumCurves))),
    original(Kind::CompoundCurve, "ST_CURVEN", 1, routine!(Routine(CompoundMethod::CurveN))),
    // [§8.1.1, §10.5.1] ST_Surface and ST_MultiSurface.
    original(Kind::Surface, "ST_AREA", 0, routine!(Routine(SurfaceMethod::Area))),
    original(Kind::Surface, "ST_PERIMETER", 0, routine!(Routine(SurfaceMethod::Perimeter))),
    original(Kind::Surface, "ST_CENTROID", 0, routine!(Routine(SurfaceMethod::Centroid))),
    original(Kind::Surface, "ST_POINTONSURFACE", 0, routine!(Routine(SurfaceMethod::PointOnSurface))),
    original(Kind::MultiSurface, "ST_AREA", 0, routine!(Routine(SurfaceMethod::Area))),
    original(Kind::MultiSurface, "ST_PERIMETER", 0, routine!(Routine(SurfaceMethod::Perimeter))),
    original(Kind::MultiSurface, "ST_CENTROID", 0, routine!(Routine(SurfaceMethod::Centroid))),
    original(Kind::MultiSurface, "ST_POINTONSURFACE", 0, routine!(Routine(SurfaceMethod::PointOnSurface))),
    // [§8.2.1, §8.3.1, §8.4.1] ST_CurvePolygon, ST_Polygon and ST_Triangle.
    original(Kind::CurvePolygon, "ST_EXTERIORRING", 0, routine!(Routine(RingMethod::ExteriorRing))),
    original(Kind::CurvePolygon, "ST_INTERIORRINGS", 0, routine!(Routine(RingMethod::InteriorRings))),
    original(Kind::CurvePolygon, "ST_NUMINTERIORRING", 0, routine!(Routine(RingMethod::NumInteriorRing))),
    original(Kind::CurvePolygon, "ST_INTERIORRINGN", 1, routine!(Routine(RingMethod::InteriorRingN))),
    original(Kind::CurvePolygon, "ST_CURVEPOLYTOPOLY", 0, routine!(Routine(RingMethod::CurvePolyToPoly))),
    overriding(Kind::Polygon, "ST_EXTERIORRING", 0, routine!(Routine(RingMethod::ExteriorRing))),
    overriding(Kind::Polygon, "ST_INTERIORRINGS", 0, routine!(Routine(RingMethod::InteriorRings))),
    overriding(Kind::Polygon, "ST_INTERIORRINGN", 1, routine!(Routine(RingMethod::InteriorRingN))),
    overriding(Kind::Triangle, "ST_EXTERIORRING", 0, routine!(Routine(RingMethod::ExteriorRing))),
    overriding(Kind::Triangle, "ST_INTERIORRINGS", 0, routine!(Routine(RingMethod::InteriorRings))),
    overriding(Kind::Triangle, "ST_INTERIORRINGN", 1, routine!(Routine(RingMethod::InteriorRingN))),
    original(Kind::Triangle, "ST_POINTS", 0, routine!(Routine(PointListMethod::Points))),
    // [§8.5.1, §8.6.1, §8.7.1] ST_PolyhdrlSurface, ST_TIN and ST_CompoundSurface.
    original(Kind::PolyhdrlSurface, "ST_PATCHES", 0, routine!(Routine(PatchMethod::Patches))),
    original(Kind::PolyhdrlSurface, "ST_NUMPATCHES", 0, routine!(Routine(PatchMethod::NumPatches))),
    original(Kind::PolyhdrlSurface, "ST_PATCHN", 1, routine!(Routine(PatchMethod::PatchN))),
    overriding(Kind::Tin, "ST_PATCHES", 0, routine!(Routine(PatchMethod::Patches))),
    original(Kind::Tin, "ST_MAXSIDELENGTH", 0, routine!(Routine(PatchMethod::MaxSideLength))),
    original(Kind::Tin, "ST_TINELEMENTS", 0, routine!(TinRoutine::Elements)),
    original(Kind::CompoundSurface, "ST_SURFACES", 0, routine!(Routine(PatchMethod::Surfaces))),
    original(Kind::CompoundSurface, "ST_NUMSURFACES", 0, routine!(Routine(PatchMethod::NumSurfaces))),
    original(Kind::CompoundSurface, "ST_SURFACEN", 1, routine!(Routine(PatchMethod::SurfaceN))),
];

/// The methods `of`'s descriptor declares.
pub fn declared(of: Kind) -> impl Iterator<Item = &'static Method> {
    METHODS.iter().filter(move |m| m.of == of)
}

/// The method `name` of `arity` that `kind` declares, if it declares one.
fn declared_by(kind: Kind, name: &str, arity: usize) -> Option<&'static Method> {
    declared(kind).find(|m| m.name == name && m.arity == arity)
}

/// [ISO/IEC 9075-2:2023 §9.18 SR10b, §9.6] The subject routine of `name`
/// of `arity` invoked of a value of declared type `kind`: the method that
/// the first type of its type precedence list declaring one declares,
/// `kind` then each supertype in turn.
pub fn subject_method(kind: Kind, name: &str, arity: usize) -> Option<&'static Method> {
    std::iter::successors(Some(kind), |k| k.supertype()).find_map(|k| declared_by(k, name, arity))
}

/// [ISO/IEC 9075-2:2023 §9.18 GR3b iv C] The methods that may be run for
/// the subject routine `subject`: it and those of its name and arity that
/// a subtype of its type declares, as overriding it.
pub fn overriding_set(subject: &Method) -> Vec<&'static Method> {
    METHODS.iter().filter(|m| m.name == subject.name && m.arity == subject.arity && m.of.is_subtype_of(subject.of)).collect()
}

/// [ISO/IEC 9075-2:2023 §9.18 GR3b iv E] Of `set`, the method that the
/// first type of `kind`'s type precedence list declaring one declares.
pub fn run_method<'m>(set: &[&'m Method], kind: Kind) -> Option<&'m Method> {
    std::iter::successors(Some(kind), |k| k.supertype()).find_map(|k| set.iter().copied().find(|m| m.of == k))
}

/// [ISO/IEC 9075-2:2023 §9.18 GR3b] A method invocation that runs, of
/// those of its overriding set, the routine of the method the subject's
/// type selects: the most specific type of SELF's value, or the type a
/// `<generalized expression>` names, `fixed`.
#[derive(Debug)]
pub struct Dispatched {
    /// The method's name.
    pub name: &'static str,
    /// The type `(v AS T)` names, if the invocation is generalized.
    pub fixed: Option<Kind>,
    /// The overriding set, each method with its routine.
    pub set: Vec<(&'static Method, ScalarRef)>,
    /// The routine of the subject routine, which a null SELF takes.
    pub subject: ScalarRef,
    /// Its declared type, the subject routine's [§9.18 SR10].
    pub result: DataType,
}

impl Dispatched {
    /// The routine that runs of SELF `first`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if SELF's type is outside the overriding set's.
    fn routine(&self, first: Option<&Value>) -> Result<&ScalarRef> {
        let most = first.and_then(super::kind::payload).and_then(super::most_specific);
        let Some(kind) = self.fixed.or(most) else { return Ok(&self.subject) };
        let methods: Vec<&Method> = self.set.iter().map(|(m, _)| *m).collect();
        let chosen = run_method(&methods, kind).and_then(|m| self.set.iter().find(|(n, _)| std::ptr::eq(*n, m)));
        chosen.map(|(_, r)| r).ok_or_else(|| {
            SqlError::new(crate::error::Code::Corrupt, format!("a value of {} has no method {}", kind.sql_name(), self.name))
        })
    }
}

impl ScalarFunction for Dispatched {
    fn name(&self) -> &str {
        self.name
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.result.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        self.routine(args.first())?.0.call(args)
    }
    fn warns(&self) -> bool {
        self.set.iter().any(|(_, r)| r.0.warns())
    }
    fn call_warned(&self, args: &[Value], temp: Option<&dyn crate::value::lob::TempLobs>) -> Result<(Value, Option<SqlState>)> {
        self.routine(args.first())?.0.call_warned(args, temp)
    }
    fn deterministic(&self) -> bool {
        self.set.iter().all(|(_, r)| r.0.deterministic())
    }
}

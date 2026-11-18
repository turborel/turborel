//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1, §7.1, §8.1, §8.4, §8.6,
//! §9.1, §9.2, §10.3, §10.5] The routines in space: the 3D relationships
//! and set operations of every geometry, the 3D measures of curves,
//! surfaces and solids, the slope of a triangle, the maximum side length
//! and clip of a TIN, and the shells of a B-rep solid. Each is called as
//! a method (`geometry/methods.rs`) or as a function of SELF first; the
//! 3D engine (`spatial::solids`) does the work.

mod construct;
mod locate;
mod measures;
mod relations;
mod shells;
mod table;
mod units;

pub use construct::BRepSolid;
pub use table::{TinFromTable, TinTable};

use super::model::{Geom, Kind};
use super::{data_type, data_type_of};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::ScalarFunction;
use crate::spatial::solids::ops::SetOp;
use crate::value::{CharUnits, DOUBLE_PRECISION, DataType, Spelled, Value};

const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
const DOUBLE: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };
/// ST_MaxUnitNameLength.
const UNIT_NAME: DataType = DataType::Varying { length: 128, units: CharUnits::Characters };

/// A routine in space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolidFunction {
    /// [§5.1.31] ST_3DIntersection(ST_Geometry).
    Intersection,
    /// [§5.1.34] ST_3DUnion(ST_Geometry).
    Union,
    /// [§5.1.36] ST_3DDifference(ST_Geometry).
    Difference,
    /// [§5.1.38] ST_3DSymDifference(ST_Geometry).
    SymDifference,
    /// [§5.1.42] ST_3DDistance(ST_Geometry [, CHARACTER VARYING]).
    Distance,
    /// [§5.1.44] ST_3DEquals(ST_Geometry).
    Equals,
    /// [§5.1.47] ST_3DDisjoint(ST_Geometry).
    Disjoint,
    /// [§5.1.49] ST_3DIntersects(ST_Geometry).
    Intersects,
    /// [§5.1.9] ST_3DIsSimple().
    IsSimple,
    /// [§5.1.18] ST_3DBoundary().
    Boundary,
    /// [§5.1.14] ST_3DLocateAlong(DOUBLE PRECISION).
    LocateAlong,
    /// [§5.1.16] ST_3DLocateBetween(DOUBLE PRECISION, DOUBLE PRECISION).
    LocateBetween,
    /// [§7.1.3, §10.3.6] ST_3DLength([CHARACTER VARYING]).
    Length,
    /// [§7.1.7, §8.1.11, §10.3.4] ST_3DIsClosed().
    IsClosed,
    /// [§7.1.9] ST_3DIsRing().
    IsRing,
    /// [§7.1.14] ST_3DDistanceToPt(ST_Point [, CHARACTER VARYING]).
    DistanceToPt,
    /// [§7.1.16] ST_3DPtAtDistance(DOUBLE PRECISION [, CHARACTER VARYING]).
    PtAtDistance,
    /// [§8.1.3, §10.5.4] ST_3DArea([CHARACTER VARYING]).
    Area,
    /// [§8.1.5, §10.5.6] ST_3DPerimeter([CHARACTER VARYING]).
    Perimeter,
    /// [§8.1.7, §9.1.4, §10.5.8] ST_3DCentroid().
    Centroid,
    /// [§8.1.9, §10.5.10] ST_3DPointOnSurf().
    PointOnSurf,
    /// [§8.1.12] ST_IsShell().
    IsShell,
    /// [§8.4.4] ST_3DSlope().
    Slope,
    /// [§9.1.2] ST_3DSurfaceArea([CHARACTER VARYING]).
    SurfaceArea,
    /// [§9.1.3] ST_3DVolume([CHARACTER VARYING]).
    Volume,
    /// [§9.1.5] ST_3DPointOnSolid().
    PointOnSolid,
    /// [§9.2.3] ST_ExteriorShell() and ST_ExteriorShell(ST_Surface).
    ExteriorShell,
    /// [§9.2.4] ST_InteriorShells() and ST_InteriorShells(ST_Surface ARRAY).
    InteriorShells,
    /// [§9.2.5] ST_NumIntShells().
    NumIntShells,
    /// [§9.2.6] ST_InteriorShellN(INTEGER).
    InteriorShellN,
    /// [§8.6.4] ST_MaxSideLength() and ST_MaxSideLength(DOUBLE PRECISION, INTEGER).
    MaxSideLength,
    /// [§8.6.6] ST_Clip(ST_Polygon).
    Clip,
}

/// Every routine in space.
pub const SOLID_FUNCTIONS: [SolidFunction; 32] = [
    SolidFunction::Intersection,
    SolidFunction::Union,
    SolidFunction::Difference,
    SolidFunction::SymDifference,
    SolidFunction::Distance,
    SolidFunction::Equals,
    SolidFunction::Disjoint,
    SolidFunction::Intersects,
    SolidFunction::IsSimple,
    SolidFunction::Boundary,
    SolidFunction::LocateAlong,
    SolidFunction::LocateBetween,
    SolidFunction::Length,
    SolidFunction::IsClosed,
    SolidFunction::IsRing,
    SolidFunction::DistanceToPt,
    SolidFunction::PtAtDistance,
    SolidFunction::Area,
    SolidFunction::Perimeter,
    SolidFunction::Centroid,
    SolidFunction::PointOnSurf,
    SolidFunction::IsShell,
    SolidFunction::Slope,
    SolidFunction::SurfaceArea,
    SolidFunction::Volume,
    SolidFunction::PointOnSolid,
    SolidFunction::ExteriorShell,
    SolidFunction::InteriorShells,
    SolidFunction::NumIntShells,
    SolidFunction::InteriorShellN,
    SolidFunction::MaxSideLength,
    SolidFunction::Clip,
];

/// What a routine is: its name, the types whose values it takes as SELF,
/// and the counts of its arguments, SELF among them.
struct Spec {
    name: &'static str,
    takes: &'static [Kind],
    counts: &'static [usize],
}

const ANY: &[Kind] = &[Kind::Geometry];
const CURVES: &[Kind] = &[Kind::Curve, Kind::MultiCurve];
const SURFACES: &[Kind] = &[Kind::Surface, Kind::MultiSurface];
const SOLIDS: &[Kind] = &[Kind::Solid];
const ONE: &[usize] = &[1];
const TWO: &[usize] = &[2];
const ONE_OR_TWO: &[usize] = &[1, 2];
const TWO_OR_THREE: &[usize] = &[2, 3];

impl SolidFunction {
    fn spec(self) -> Spec {
        let s = |name, takes, counts| Spec { name, takes, counts };
        match self {
            Self::Intersection => s("ST_3DINTERSECTION", ANY, TWO),
            Self::Union => s("ST_3DUNION", ANY, TWO),
            Self::Difference => s("ST_3DDIFFERENCE", ANY, TWO),
            Self::SymDifference => s("ST_3DSYMDIFFERENCE", ANY, TWO),
            Self::Distance => s("ST_3DDISTANCE", ANY, TWO_OR_THREE),
            Self::Equals => s("ST_3DEQUALS", ANY, TWO),
            Self::Disjoint => s("ST_3DDISJOINT", ANY, TWO),
            Self::Intersects => s("ST_3DINTERSECTS", ANY, TWO),
            Self::IsSimple => s("ST_3DISSIMPLE", ANY, ONE),
            Self::Boundary => s("ST_3DBOUNDARY", ANY, ONE),
            Self::LocateAlong => s("ST_3DLOCATEALONG", ANY, TWO),
            Self::LocateBetween => s("ST_3DLOCATEBETWEEN", ANY, &[3]),
            Self::Length => s("ST_3DLENGTH", CURVES, ONE_OR_TWO),
            Self::IsClosed => s("ST_3DISCLOSED", &[Kind::Curve, Kind::MultiCurve, Kind::Surface], ONE),
            Self::IsRing => s("ST_3DISRING", &[Kind::Curve], ONE),
            Self::DistanceToPt => s("ST_3DDISTANCETOPT", &[Kind::Curve], TWO_OR_THREE),
            Self::PtAtDistance => s("ST_3DPTATDISTANCE", &[Kind::Curve], TWO_OR_THREE),
            Self::Area => s("ST_3DAREA", SURFACES, ONE_OR_TWO),
            Self::Perimeter => s("ST_3DPERIMETER", SURFACES, ONE_OR_TWO),
            Self::Centroid => s("ST_3DCENTROID", &[Kind::Surface, Kind::MultiSurface, Kind::Solid], ONE),
            Self::PointOnSurf => s("ST_3DPOINTONSURF", SURFACES, ONE),
            Self::IsShell => s("ST_ISSHELL", &[Kind::Surface], ONE),
            Self::Slope => s("ST_3DSLOPE", &[Kind::Triangle], ONE),
            Self::SurfaceArea => s("ST_3DSURFACEAREA", SOLIDS, ONE_OR_TWO),
            Self::Volume => s("ST_3DVOLUME", SOLIDS, ONE_OR_TWO),
            Self::PointOnSolid => s("ST_3DPOINTONSOLID", SOLIDS, ONE),
            Self::ExteriorShell => s("ST_EXTERIORSHELL", &[Kind::BRepSolid], ONE_OR_TWO),
            Self::InteriorShells => s("ST_INTERIORSHELLS", &[Kind::BRepSolid], ONE_OR_TWO),
            Self::NumIntShells => s("ST_NUMINTSHELLS", &[Kind::BRepSolid], ONE),
            Self::InteriorShellN => s("ST_INTERIORSHELLN", &[Kind::BRepSolid], TWO),
            Self::MaxSideLength => s("ST_MAXSIDELENGTH", &[Kind::Tin], &[1, 3]),
            Self::Clip => s("ST_CLIP", &[Kind::Tin], TWO),
        }
    }

    /// Whether SELF's declared type `kind` may hold a value it takes.
    fn declared(self, kind: Kind) -> bool {
        self.spec().takes.iter().any(|t| kind.is_subtype_of(*t) || t.is_subtype_of(kind))
    }

    /// Whether it takes, as SELF, a value of most specific type `kind`.
    fn takes(self, kind: Kind) -> bool {
        self.spec().takes.iter().any(|t| kind.is_subtype_of(*t))
    }

    /// The declared type of what a call of `args` returns.
    fn result(self, args: &[DataType]) -> DataType {
        let observer = args.len() == 1;
        let first = || args.first().cloned().unwrap_or_else(data_type);
        match self {
            Self::Intersection
            | Self::Union
            | Self::Difference
            | Self::SymDifference
            | Self::Boundary
            | Self::LocateAlong
            | Self::LocateBetween => data_type(),
            Self::Distance
            | Self::Length
            | Self::DistanceToPt
            | Self::Area
            | Self::Perimeter
            | Self::Slope
            | Self::SurfaceArea
            | Self::Volume => DOUBLE,
            Self::Equals
            | Self::Disjoint
            | Self::Intersects
            | Self::IsSimple
            | Self::IsClosed
            | Self::IsRing
            | Self::IsShell
            | Self::NumIntShells => INTEGER,
            Self::PtAtDistance | Self::Centroid | Self::PointOnSurf | Self::PointOnSolid => data_type_of(Kind::Point),
            Self::ExteriorShell => observed(observer, data_type_of(Kind::Surface), first),
            Self::InteriorShells => observed(observer, shells::array(), first),
            Self::MaxSideLength => observed(observer, DOUBLE, first),
            Self::Clip => first(),
            Self::InteriorShellN => data_type_of(Kind::Surface),
        }
    }

    /// The declared type of the argument at `position` after SELF, of a
    /// call of `count` arguments.
    fn parameter(self, position: usize, count: usize) -> Option<DataType> {
        let unit = |at| (position == at).then_some(UNIT_NAME);
        match self {
            Self::Intersection
            | Self::Union
            | Self::Difference
            | Self::SymDifference
            | Self::Equals
            | Self::Disjoint
            | Self::Intersects => Some(data_type()),
            Self::Distance => unit(2).or(Some(data_type())),
            Self::IsSimple
            | Self::Boundary
            | Self::IsClosed
            | Self::IsRing
            | Self::Centroid
            | Self::PointOnSurf
            | Self::IsShell
            | Self::Slope
            | Self::PointOnSolid
            | Self::NumIntShells => None,
            Self::LocateAlong | Self::LocateBetween => Some(DOUBLE),
            Self::Length | Self::Area | Self::Perimeter | Self::SurfaceArea | Self::Volume => unit(1),
            Self::DistanceToPt => unit(2).or(Some(data_type_of(Kind::Point))),
            Self::PtAtDistance => unit(2).or(Some(DOUBLE)),
            Self::ExteriorShell => Some(data_type_of(Kind::Surface)),
            Self::InteriorShells => Some(shells::array()),
            Self::InteriorShellN => Some(INTEGER),
            Self::MaxSideLength => Some(if position == 1 && count == 3 { DOUBLE } else { INTEGER }),
            Self::Clip => Some(data_type_of(Kind::Polygon)),
        }
    }

    /// The arguments whose null value is an exception rather than a null
    /// result [§8.6.4 Description 4a, §9.2.3 Description 4a, §9.2.4
    /// Description 4a].
    fn checks_null(self, position: usize, count: usize) -> bool {
        let mutator = matches!(self, Self::ExteriorShell | Self::InteriorShells) && count == 2;
        position == 1 && (mutator || self == Self::MaxSideLength && count == 3)
    }

    /// The value of a call of SELF `g` and the other arguments `rest`,
    /// and the warning raised, if one.
    fn of(self, g: &Geom, rest: &[Value]) -> Result<(Value, Option<SqlState>)> {
        let value = match self {
            Self::Intersection => relations::set(SetOp::Intersection, g, rest),
            Self::Union => relations::set(SetOp::Union, g, rest),
            Self::Difference => relations::set(SetOp::Difference, g, rest),
            Self::SymDifference => relations::set(SetOp::SymDifference, g, rest),
            Self::Distance => relations::distance(g, rest),
            Self::Equals => relations::relation(0, g, rest),
            Self::Disjoint => relations::relation(1, g, rest),
            Self::Intersects => relations::relation(2, g, rest),
            Self::IsSimple => relations::is_simple(g),
            Self::Boundary => relations::boundary(g),
            Self::LocateAlong | Self::LocateBetween => relations::locate(g, rest),
            Self::Length => measures::length(g, rest),
            Self::IsClosed => measures::is_closed(g),
            Self::IsRing => measures::is_ring(g),
            Self::DistanceToPt => measures::distance_to_pt(g, rest),
            Self::PtAtDistance => measures::pt_at_distance(g, rest),
            Self::Area => measures::area_or_perimeter(g, rest, false),
            Self::Perimeter => measures::area_or_perimeter(g, rest, true),
            Self::Centroid => measures::centroid(g),
            Self::PointOnSurf => measures::point_on_surf(g),
            Self::IsShell => measures::is_shell(g),
            Self::Slope => measures::slope(g),
            Self::SurfaceArea => measures::solid_measure(g, rest, false),
            Self::Volume => measures::solid_measure(g, rest, true),
            Self::PointOnSolid => measures::point_on_solid(g),
            Self::ExteriorShell => rest.first().map_or(Ok(shells::exterior(g)), |v| shells::replaced(g, v, true)),
            Self::InteriorShells => rest.first().map_or(Ok(shells::interiors(g)), |v| shells::replaced(g, v, false)),
            Self::NumIntShells => Ok(shells::count(g)),
            Self::InteriorShellN => return shells::nth(g, rest),
            Self::MaxSideLength => shells::max_side_length(g, rest),
            Self::Clip => shells::clip(g, rest),
        };
        value.map(|v| (v, None))
    }
}

/// What an observer returns, `seen`, or else the type of SELF.
fn observed(observer: bool, seen: DataType, first: impl FnOnce() -> DataType) -> DataType {
    if observer { seen } else { first() }
}

/// An SQL/MM Spatial exception of `state` of the rule `clause`.
pub(crate) fn spatial(state: SqlState, clause: &str, detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) {clause}] {detail}")).with_state(state)
}

fn signature(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1] invalid arguments to {name}"))
}

/// A DOUBLE PRECISION argument.
fn number(v: &Value) -> Result<f64> {
    if let Value::Approx(f) = v {
        return Ok(*f);
    }
    if let Value::Exact(d) = v {
        return Ok(d.to_f64());
    }
    Err(signature("a routine in space"))
}

/// A CHARACTER VARYING argument.
fn text(v: &Value) -> Result<&str> {
    if let Value::Char(s) = v { Ok(s) } else { Err(signature("a routine in space")) }
}

/// An INTEGER argument.
fn integer(v: &Value) -> Result<i64> {
    let whole = if let Value::Exact(d) = v { (d.scale == 0).then_some(d.units) } else { None };
    whole.and_then(|n| i64::try_from(n).ok()).ok_or_else(|| signature("a routine in space"))
}

impl ScalarFunction for SolidFunction {
    fn name(&self) -> &'static str {
        self.spec().name
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let spec = self.spec();
        let first = args.first().filter(|t| data_type().kind().comparable(t)).ok_or_else(|| signature(spec.name))?;
        let declared = super::of_data_type(first).ok_or_else(|| signature(spec.name))?;
        if !spec.counts.contains(&args.len()) || !self.declared(declared) {
            return Err(signature(spec.name));
        }
        Ok(self.result(args))
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        if position == 0 {
            return known.first().cloned().flatten().filter(|t| super::of_data_type(t).is_some()).or_else(|| Some(data_type()));
        }
        self.parameter(position, known.len())
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        self.call_warned(args, None).map(|(v, _)| v)
    }

    fn warns(&self) -> bool {
        *self == Self::InteriorShellN
    }

    fn call_warned(&self, args: &[Value], _temp: Option<&dyn crate::value::lob::TempLobs>) -> Result<(Value, Option<SqlState>)> {
        let spec = self.spec();
        if !spec.counts.contains(&args.len()) {
            return Err(signature(spec.name));
        }
        if args.iter().enumerate().any(|(at, v)| v.is_null() && self.checks_null(at, args.len())) {
            return Err(spatial(
                SqlState::SPATIAL_NULL_ARGUMENT,
                "§8.6.4 Description 4a",
                &format!("an argument of {} is null", spec.name),
            ));
        }
        let [first, rest @ ..] = args else { return Err(signature(spec.name)) };
        if args.iter().any(Value::is_null) {
            return Ok((Value::Null, None));
        }
        let g = Geom::from_value(first)?;
        if !self.takes(g.kind()) {
            let detail = format!("{} of an {}", spec.name, g.kind().sql_name());
            return Err(spatial(SqlState::SPATIAL_INVALID_ARGUMENT, "§5.1", &detail));
        }
        self.of(&g, rest)
    }

    fn deterministic(&self) -> bool {
        true
    }
}

/// The table functions, built in: [§8.6.5] ST_TINTable().
pub fn table_functions() -> Vec<std::sync::Arc<dyn crate::functions::builtin::TableFunction>> {
    vec![std::sync::Arc::new(TinTable)]
}

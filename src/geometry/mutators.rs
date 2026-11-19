//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §6 to §8] The mutators of the
//! geometry types: the type-preserving methods that return SELF with one
//! attribute replaced. A point's coordinates (`points.rs`); the points of
//! a linestring, a circular string, a circle, a geodesic string and a
//! triangle, and the curves of a compound curve (`lists.rs`); the rings
//! of a curve polygon, a polygon and a triangle, and the patches of a
//! polyhedral surface and a TIN (`rings.rs`); and the attributes of the
//! curves given by parameters (`parameters.rs`). Each checks what its
//! clause checks, and the value it makes is checked as every value is
//! (`model/check.rs`).
//!
//! A value of the model holds every attribute or none, so a mutator of
//! one attribute of an empty elliptical curve, NURBS curve, clothoid or
//! spiral, whose other attributes are null, raises 2FF02: such a value is
//! made whole by its constructor. A curve has start and end measures
//! together, as its coordinate dimension says whether it is measured, so
//! ST_StartM or ST_EndM of an unmeasured curve gives a measured one whose
//! other measure is the same, and either of the null value removes both.
//! A NURBS curve's control points and knots agree in every value, so a
//! mutator that would part them raises 2FF02.

mod constructors;
mod either;
mod lists;
mod parameters;
mod points;
mod properties;
mod rings;

use super::kind::{data_type, of_data_type};
use super::model::{Geom, Kind};
use super::observers::{DOUBLE, signature};
use super::representations::spatial;
use crate::error::{Result, SqlError, SqlState};
use crate::functions::{CallContext, ScalarFunction};
use crate::value::{CharUnits, DataType, Value};

pub use constructors::{CONSTRUCTED, Constructor};
pub use either::{Either, WithoutSelf};
use parameters::Attribute;
use points::Axis;
pub use properties::Property;
pub(crate) use properties::envelope;

/// ST_MaxUnitNameLength [§5.1.30 DR 1].
const UNIT_NAME_LENGTH: u32 = 128;
/// The length of a spiral type's name [§7.9.7].
const SPIRAL_TYPE_LENGTH: u32 = 64;
/// The arguments of a unit form: SELF, the length and the unit.
const UNIT_FORM: usize = 3;

/// A mutator of a geometry type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutator {
    /// [§6.1.3] ST_X(DOUBLE PRECISION).
    X,
    /// [§6.1.4] ST_Y(DOUBLE PRECISION).
    Y,
    /// [§6.1.5] ST_Z(DOUBLE PRECISION).
    Z,
    /// [§6.1.6] ST_M(DOUBLE PRECISION).
    M,
    /// [§7.2.3, §7.3.3, §7.4.3, §7.5.3, §8.4.3] ST_Points(ST_Point ARRAY).
    Points,
    /// [§7.10.3] ST_Curves(ST_Curve ARRAY).
    Curves,
    /// [§8.2.3, §8.3.3, §8.4.5] ST_ExteriorRing(ST_Curve).
    ExteriorRing,
    /// [§8.2.4, §8.3.4, §8.4.6] ST_InteriorRings(ST_Curve ARRAY).
    InteriorRings,
    /// [§8.5.3, §8.6.7] ST_Patches(ST_Polygon ARRAY).
    Patches,
    /// [§7.6.3, §7.8.3, §7.9.3] ST_RefLocation(ST_AffinePlacement).
    RefLocation,
    /// [§7.6.4] ST_UAxisLength(DOUBLE PRECISION [, unit]).
    UAxisLength,
    /// [§7.6.5] ST_VAxisLength(DOUBLE PRECISION [, unit]).
    VAxisLength,
    /// [§7.6.6] ST_StartAngle(ST_Angle).
    StartAngle,
    /// [§7.6.7] ST_EndAngle(ST_Angle).
    EndAngle,
    /// [§7.6.8, §7.7.6, §7.8.7, §7.9.8] ST_StartM(DOUBLE PRECISION).
    StartM,
    /// [§7.6.9, §7.7.7, §7.8.8, §7.9.9] ST_EndM(DOUBLE PRECISION).
    EndM,
    /// [§7.7.4] ST_ControlPoints(ST_NURBSPoint ARRAY).
    ControlPoints,
    /// [§7.7.5] ST_Knots(ST_Knot ARRAY).
    Knots,
    /// [§7.8.4] ST_ScaleFactor(DOUBLE PRECISION).
    ScaleFactor,
    /// [§7.8.5] ST_StartDistance(DOUBLE PRECISION [, unit]).
    StartDistance,
    /// [§7.8.6] ST_EndDistance(DOUBLE PRECISION [, unit]).
    EndDistance,
    /// [§7.9.4] ST_Length(DOUBLE PRECISION [, unit]).
    Length,
    /// [§7.9.5] ST_StartCurvature(DOUBLE PRECISION).
    StartCurvature,
    /// [§7.9.6] ST_EndCurvature(DOUBLE PRECISION).
    EndCurvature,
    /// [§7.9.7] ST_SpiralType(CHARACTER VARYING).
    SpiralType,
}

/// What a mutator takes after SELF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Takes {
    /// A DOUBLE PRECISION number.
    Number,
    /// A length, with a unit or not.
    Length,
    /// A geometry, as given.
    Geometry,
    /// An ARRAY, as given.
    Array,
    /// A value of a type of §16 or §17.
    Support(super::support::Support),
    /// A spiral type's name.
    Name,
}

impl Mutator {
    /// Its name, upper case.
    pub const fn name(self) -> &'static str {
        match self {
            Self::X => "ST_X",
            Self::Y => "ST_Y",
            Self::Z => "ST_Z",
            Self::M => "ST_M",
            Self::Points => "ST_POINTS",
            Self::Curves => "ST_CURVES",
            Self::ExteriorRing => "ST_EXTERIORRING",
            Self::InteriorRings => "ST_INTERIORRINGS",
            Self::Patches => "ST_PATCHES",
            Self::RefLocation => "ST_REFLOCATION",
            Self::UAxisLength => "ST_UAXISLENGTH",
            Self::VAxisLength => "ST_VAXISLENGTH",
            Self::StartAngle => "ST_STARTANGLE",
            Self::EndAngle => "ST_ENDANGLE",
            Self::StartM => "ST_STARTM",
            Self::EndM => "ST_ENDM",
            Self::ControlPoints => "ST_CONTROLPOINTS",
            Self::Knots => "ST_KNOTS",
            Self::ScaleFactor => "ST_SCALEFACTOR",
            Self::StartDistance => "ST_STARTDISTANCE",
            Self::EndDistance => "ST_ENDDISTANCE",
            Self::Length => "ST_LENGTH",
            Self::StartCurvature => "ST_STARTCURVATURE",
            Self::EndCurvature => "ST_ENDCURVATURE",
            Self::SpiralType => "ST_SPIRALTYPE",
        }
    }

    fn takes(self) -> Takes {
        use super::support::Support as S;
        match self {
            Self::X
            | Self::Y
            | Self::Z
            | Self::M
            | Self::StartM
            | Self::EndM
            | Self::ScaleFactor
            | Self::StartCurvature
            | Self::EndCurvature => Takes::Number,
            Self::UAxisLength | Self::VAxisLength | Self::StartDistance | Self::EndDistance | Self::Length => Takes::Length,
            Self::ExteriorRing => Takes::Geometry,
            Self::Points | Self::Curves | Self::InteriorRings | Self::Patches | Self::ControlPoints | Self::Knots => Takes::Array,
            Self::RefLocation => Takes::Support(S::AffinePlacement),
            Self::StartAngle | Self::EndAngle => Takes::Support(S::Angle),
            Self::SpiralType => Takes::Name,
        }
    }

    /// SELF with the attribute replaced by `arg`, a length's in the unit
    /// `unit` if given, in the reference systems with it.
    fn apply(self, g: &Geom, arg: &Value, unit: Option<(&str, Systems<'_>)>) -> Result<Geom> {
        let attribute = |a: Attribute| parameters::replace(a, self.name(), g, arg, unit);
        match self {
            Self::X => points::coordinate(Axis::X, g, arg),
            Self::Y => points::coordinate(Axis::Y, g, arg),
            Self::Z => points::coordinate(Axis::Z, g, arg),
            Self::M => points::coordinate(Axis::M, g, arg),
            Self::Points | Self::Curves => lists::replace(self, g, arg),
            Self::ExteriorRing => rings::exterior(g, arg),
            Self::InteriorRings => rings::interior(g, arg),
            Self::Patches => rings::patches(g, arg),
            Self::RefLocation => attribute(Attribute::RefLocation),
            Self::UAxisLength => attribute(Attribute::UAxisLength),
            Self::VAxisLength => attribute(Attribute::VAxisLength),
            Self::StartAngle => attribute(Attribute::StartAngle),
            Self::EndAngle => attribute(Attribute::EndAngle),
            Self::StartM => attribute(Attribute::StartM),
            Self::EndM => attribute(Attribute::EndM),
            Self::ControlPoints => attribute(Attribute::ControlPoints),
            Self::Knots => attribute(Attribute::Knots),
            Self::ScaleFactor => attribute(Attribute::ScaleFactor),
            Self::StartDistance => attribute(Attribute::StartDistance),
            Self::EndDistance => attribute(Attribute::EndDistance),
            Self::Length => attribute(Attribute::Length),
            Self::StartCurvature => attribute(Attribute::StartCurvature),
            Self::EndCurvature => attribute(Attribute::EndCurvature),
            Self::SpiralType => attribute(Attribute::SpiralType),
        }
    }

    /// A call of `args`, a unit form's in the reference systems `systems`.
    fn run(self, args: &[Value], systems: Systems<'_>) -> Result<Value> {
        let [first, arg, rest @ ..] = args else { return Err(signature(self.name())) };
        let unit = match rest {
            [] => None,
            [Value::Char(unit)] => Some((unit.as_str(), systems)),
            [Value::Null] => return Err(null_argument(self.name())),
            [_, ..] => return Err(signature(self.name())),
        };
        let nullable = matches!(self, Self::Z | Self::M | Self::StartM | Self::EndM);
        if arg.is_null() && !nullable {
            return Err(null_argument(self.name()));
        }
        if first.is_null() {
            return Ok(Value::Null);
        }
        Ok(self.apply(&Geom::from_value(first)?, arg, unit)?.to_value())
    }
}

/// The reference systems a unit form reads.
pub(crate) type Systems<'a> = Option<&'a crate::catalog::SpatialCatalog>;

/// 2FF03 null argument of the routine `name`.
pub(crate) fn null_argument(name: &str) -> SqlError {
    spatial(SqlState::SPATIAL_NULL_ARGUMENT, "§6 to §10", &format!("null argument to {name}"))
}

/// 2FF02 invalid argument, citing `clause`.
pub(crate) fn invalid(clause: &str, msg: &str) -> SqlError {
    spatial(SqlState::SPATIAL_INVALID_ARGUMENT, clause, msg)
}

/// 2FF10 mixed spatial reference systems, citing `clause`.
pub(crate) fn mixed_systems(clause: &str) -> SqlError {
    spatial(SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS, clause, "mixed spatial reference systems")
}

/// CHARACTER VARYING of `length` characters.
const fn varying(length: u32) -> DataType {
    DataType::Varying { length, units: CharUnits::Characters }
}

impl ScalarFunction for Mutator {
    fn name(&self) -> &str {
        Mutator::name(*self)
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let unit = self.takes() == Takes::Length && args.len() == UNIT_FORM;
        let Some(first) = args.first().filter(|t| of_data_type(t).is_some()) else { return Err(signature(self.name())) };
        if args.len() != 2 && !unit {
            return Err(signature(self.name()));
        }
        Ok(first.clone())
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        let given = known.get(position).cloned().flatten();
        match position {
            0 => Some(given.filter(|t| of_data_type(t).is_some()).unwrap_or_else(data_type)),
            1 => match self.takes() {
                Takes::Number | Takes::Length => Some(DOUBLE),
                Takes::Geometry => Some(given.filter(|t| of_data_type(t).is_some()).unwrap_or_else(|| super::data_type_of(Kind::Curve))),
                Takes::Array => None,
                Takes::Support(s) => Some(super::support::data_type(s)),
                Takes::Name => Some(varying(SPIRAL_TYPE_LENGTH)),
            },
            2 => Some(varying(UNIT_NAME_LENGTH)),
            _ => None,
        }
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        self.run(args, None)
    }

    fn uses_context(&self, arity: usize) -> bool {
        self.takes() == Takes::Length && arity == UNIT_FORM
    }

    fn call_in(&self, args: &[Value], cx: &dyn CallContext) -> Result<Value> {
        self.run(args, cx.spatial())
    }

    fn deterministic(&self) -> bool {
        true
    }
}

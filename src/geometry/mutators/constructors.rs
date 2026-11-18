//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §6.1.2, §7.2.2 to §7.10.2,
//! §8.2.2 to §8.5.2, §10.1.2 to §10.6.2] The constructors of the
//! geometry types, the functions named after them that `NEW` calls: of a
//! representation, read as the type's FromText, FromWKB or FromGML
//! function reads it (`solids::represented`); and of the values each
//! type's definition takes, each built as its definition builds it, most
//! as the empty value given an SRID and then its mutators
//! (`assembled.rs`), the points, arcs and curves given by parameters
//! from their parameters (`parametric.rs`). A constructor's forms are
//! chosen by their arguments' types: an INTEGER is an SRID, a degree or
//! a multiplicity, where another form takes a DOUBLE PRECISION, so
//! `ST_Point(1, 2, 3)` is a point of SRID 3 and `ST_Point(1, 2, 3E0)` one
//! of z 3. ST_Point of x and y, and an SRID, is ST_Point of D85
//! (`GeometryFunction::Point`).

mod arcs;
mod assembled;
mod parametric;

use super::Systems;
use crate::error::Result;
use crate::functions::{AppendedQuery, CallContext, ScalarFunction};
use crate::geometry::functions::GeometryFunction;
use crate::geometry::kind::{data_type_of, of_data_type, payload};
use crate::geometry::model::Kind;
use crate::geometry::observers::{DOUBLE, INTEGER, signature};
use crate::geometry::solids::represented::{appended, is_representation, own, read, representation_type};
use crate::geometry::support::{self, Support};
use crate::params::ParameterType;
use crate::value::{CharUnits, DataType, Value};

/// The most arguments of a representation's form: it and an SRID.
const REPRESENTATION_FORM: usize = 2;
/// ST_MaxUnitNameLength [§5.1.30 DR 1].
const UNIT_NAME_LENGTH: u32 = 128;
/// The length of a spiral type's name [§7.9.2].
const SPIRAL_TYPE_LENGTH: u32 = 64;
/// The forms of ST_Point that are D85's.
const POINT_FORMS: usize = 3;

/// The types a constructor is a function of.
pub const CONSTRUCTED: [Kind; 20] = [
    Kind::Point,
    Kind::LineString,
    Kind::CircularString,
    Kind::Circle,
    Kind::GeodesicString,
    Kind::EllipticalCurve,
    Kind::NurbsCurve,
    Kind::Clothoid,
    Kind::SpiralCurve,
    Kind::CompoundCurve,
    Kind::CurvePolygon,
    Kind::Polygon,
    Kind::Triangle,
    Kind::PolyhdrlSurface,
    Kind::GeomCollection,
    Kind::MultiPoint,
    Kind::MultiCurve,
    Kind::MultiLineString,
    Kind::MultiSurface,
    Kind::MultiPolygon,
];

/// A parameter of a constructor's form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Param {
    /// DOUBLE PRECISION.
    Double,
    /// INTEGER.
    Integer,
    /// A unit's name.
    Unit,
    /// A spiral type's name.
    Name,
    /// A value of a geometry type.
    One(Kind),
    /// An ARRAY of a geometry type's values.
    Many(Kind),
    /// An ARRAY of DOUBLE PRECISION.
    Doubles,
    /// A value of a type of §16 or §17.
    Record(Support),
    /// An ARRAY of a type of §16 or §17.
    Records(Support),
}

use Param::{Double as D, Integer as I, Many, One, Record, Records, Unit as U};

/// The suffixes of a curve given by parameters: measures, a unit, an
/// SRID [§7.6.2, §7.8.2, §7.9.2].
const SUFFIXES: [&[Param]; 8] = [&[], &[I], &[U], &[D, D], &[U, I], &[D, D, I], &[D, D, U], &[D, D, U, I]];
/// The suffixes of a NURBS curve: measures, an SRID [§7.7.2].
const NURBS_SUFFIXES: [&[Param]; 4] = [&[], &[I], &[D, D], &[D, D, I]];

/// The forms of the constructor of `kind`, each its parameters, without
/// the representations'.
pub(super) fn forms(kind: Kind) -> Vec<Vec<Param>> {
    let with_srid = |base: &[Param]| vec![base.to_vec(), [base, &[I]].concat()];
    let suffixed = |base: &[Param], suffixes: &[&[Param]]| suffixes.iter().map(|s| [base, s].concat()).collect();
    let points = Many(Kind::Point);
    match kind {
        Kind::Point => vec![vec![D, D], vec![D, D, I], vec![D, D, D], vec![D, D, D, I], vec![D, D, D, D], vec![D, D, D, D, I]],
        Kind::LineString | Kind::GeodesicString | Kind::MultiPoint => with_srid(&[points]),
        Kind::CircularString => [with_srid(&[points]), with_srid(&[points, Param::Doubles, Records(Support::Vector)])]
            .concat()
            .into_iter()
            .chain(with_srid(&[One(Kind::Point), D, Record(Support::Angle), Record(Support::Angle)]))
            .collect(),
        Kind::Circle => [with_srid(&[points]), with_srid(&[One(Kind::Point), D, Record(Support::Vector)])].concat(),
        Kind::EllipticalCurve => {
            suffixed(&[Record(Support::AffinePlacement), D, D, Record(Support::Angle), Record(Support::Angle)], &SUFFIXES)
        }
        Kind::Clothoid => suffixed(&[Record(Support::AffinePlacement), D, D, D], &SUFFIXES),
        Kind::SpiralCurve => suffixed(&[Record(Support::AffinePlacement), D, D, D, Param::Name], &SUFFIXES),
        Kind::NurbsCurve => suffixed(&[I, Records(Support::NurbsPoint), Records(Support::Knot)], &NURBS_SUFFIXES),
        Kind::CompoundCurve => [with_srid(&[One(Kind::Curve)]), with_srid(&[Many(Kind::Curve)])].concat(),
        Kind::CurvePolygon => [with_srid(&[One(Kind::Curve)]), with_srid(&[One(Kind::Curve), Many(Kind::Curve)])].concat(),
        Kind::Polygon => [with_srid(&[One(Kind::LineString)]), with_srid(&[One(Kind::LineString), Many(Kind::LineString)])].concat(),
        Kind::Triangle => [with_srid(&[One(Kind::LineString)]), with_srid(&[points])].concat(),
        Kind::PolyhdrlSurface | Kind::MultiPolygon => with_srid(&[Many(Kind::Polygon)]),
        Kind::GeomCollection => [with_srid(&[One(Kind::Geometry)]), with_srid(&[Many(Kind::Geometry)])].concat(),
        Kind::MultiCurve => with_srid(&[Many(Kind::Curve)]),
        Kind::MultiLineString => with_srid(&[Many(Kind::LineString)]),
        Kind::MultiSurface => with_srid(&[Many(Kind::Surface)]),
        Kind::Geometry | Kind::Curve | Kind::Surface | Kind::Tin | Kind::CompoundSurface | Kind::Solid | Kind::BRepSolid => Vec::new(),
    }
}

/// Whether a value of `given` may be one of `wanted`: one is of the
/// other, as the mutators check the value itself.
fn related(given: Kind, wanted: Kind) -> bool {
    given.is_subtype_of(wanted) || wanted.is_subtype_of(given)
}

/// The element type of an ARRAY type.
fn element(t: &DataType) -> Option<&DataType> {
    if let DataType::Array(a) = t { Some(&a.element) } else { None }
}

impl Param {
    /// Whether an argument of type `t`, unknown if none, fits.
    fn fits_type(self, t: Option<&DataType>) -> bool {
        let Some(t) = t else { return true };
        let geometry = |k: Kind, of: &DataType| of_data_type(of).is_some_and(|g| related(g, k));
        match self {
            Self::Double => t.is_numeric(),
            Self::Integer => matches!(t, DataType::Exact { scale: 0, .. }),
            Self::Unit | Self::Name => t.is_character(),
            Self::One(k) => geometry(k, t),
            Self::Many(k) => element(t).is_some_and(|e| geometry(k, e)),
            Self::Doubles => element(t).is_some_and(DataType::is_numeric),
            Self::Record(s) => support::of_data_type(t) == Some(s),
            Self::Records(s) => element(t).is_some_and(|e| support::of_data_type(e) == Some(s)),
        }
    }

    /// Whether the argument `v` fits, the null value fitting each.
    fn fits_value(self, v: &Value) -> bool {
        if v.is_null() {
            return true;
        }
        match self {
            Self::Double => matches!(v, Value::Approx(_)),
            Self::Integer => matches!(v, Value::Exact(_)),
            Self::Unit | Self::Name => matches!(v, Value::Char(_)),
            Self::Many(_) | Self::Doubles | Self::Records(_) => matches!(v, Value::Array(_)),
            Self::One(_) => payload(v).is_some(),
            Self::Record(s) => support::of_value(v).is_some_and(|(of, _)| of == s),
        }
    }

    /// The type an argument is given as, none to keep its own.
    fn data_type(self, given: Option<&DataType>) -> Option<DataType> {
        match self {
            Self::Double => Some(DOUBLE),
            Self::Integer => Some(INTEGER),
            Self::Unit => Some(DataType::Varying { length: UNIT_NAME_LENGTH, units: CharUnits::Characters }),
            Self::Name => Some(DataType::Varying { length: SPIRAL_TYPE_LENGTH, units: CharUnits::Characters }),
            Self::One(k) => Some(given.filter(|t| of_data_type(t).is_some()).cloned().unwrap_or_else(|| data_type_of(k))),
            Self::Record(s) => Some(support::data_type(s)),
            Self::Many(_) | Self::Doubles | Self::Records(_) => None,
        }
    }
}

/// A constructor of a geometry type, the function of its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Constructor(pub Kind);

impl Constructor {
    /// The form whose parameters the types `known` fit.
    fn form_of_types(self, known: &[Option<&DataType>]) -> Option<Vec<Param>> {
        forms(self.0).into_iter().find(|f| f.len() == known.len() && f.iter().zip(known).all(|(p, t)| p.fits_type(*t)))
    }

    /// A call of `args`, a unit's lengths in the reference systems
    /// `systems`.
    fn run(self, args: &[Value], systems: Systems<'_>) -> Result<Value> {
        if let Some(read) = read(self.0, args)? {
            return Ok(read);
        }
        let form = forms(self.0)
            .into_iter()
            .find(|f| f.len() == args.len() && f.iter().zip(args).all(|(p, v)| p.fits_value(v)))
            .ok_or_else(|| signature(self.0.type_name()))?;
        if self.is_d85(&form) {
            return GeometryFunction::Point.call(args);
        }
        if parametric::takes(self.0) {
            return parametric::build(self.0, &form, args, systems);
        }
        if self.0 == Kind::Point {
            return parametric::point(&form, args);
        }
        if args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        assembled::build(self.0, &form, args)
    }

    /// Whether the form `f` is one of D85's ST_Point.
    fn is_d85(self, f: &[Param]) -> bool {
        self.0 == Kind::Point && f.len() < POINT_FORMS.saturating_add(1) && f.get(2).is_none_or(|p| *p == I)
    }
}

impl ScalarFunction for Constructor {
    fn name(&self) -> &str {
        self.0.type_name()
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if args.first().is_some_and(is_representation) {
            return if own(args) <= REPRESENTATION_FORM { Ok(data_type_of(self.0)) } else { Err(signature(self.name())) };
        }
        let known: Vec<Option<&DataType>> = args.iter().map(Some).collect();
        let form = self.form_of_types(&known).ok_or_else(|| signature(self.name()))?;
        if self.is_d85(&form) {
            return GeometryFunction::Point.return_type(args);
        }
        Ok(data_type_of(self.0))
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        if let Some(t) = representation_type(known) {
            return match position {
                0 => Some(t),
                1 => Some(INTEGER),
                _ => None,
            };
        }
        let refs: Vec<Option<&DataType>> = known.iter().map(Option::as_ref).collect();
        let form = self.form_of_types(&refs)?;
        if self.is_d85(&form) {
            return GeometryFunction::Point.argument_type(position, known);
        }
        form.get(position)?.data_type(known.get(position).and_then(Option::as_ref))
    }

    fn parameter_type(&self, position: usize, known: &[Option<DataType>]) -> Option<ParameterType> {
        let refs: Vec<Option<&DataType>> = known.iter().map(Option::as_ref).collect();
        if self.0 == Kind::Point && self.form_of_types(&refs).is_none_or(|f| self.is_d85(&f)) {
            return GeometryFunction::Point.parameter_type(position, known);
        }
        self.argument_type(position, known).map(ParameterType::Of)
    }

    fn catalog_query(&self, _: &[Option<&Value>], types: &[DataType], _: &dyn crate::catalog::Catalog) -> Option<AppendedQuery> {
        appended(types)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        self.run(args, None)
    }

    fn uses_context(&self, arity: usize) -> bool {
        forms(self.0).iter().any(|f| f.len() == arity && f.contains(&U))
    }

    fn call_in(&self, args: &[Value], cx: &dyn CallContext) -> Result<Value> {
        self.run(args, cx.spatial())
    }

    fn deterministic(&self) -> bool {
        true
    }
}

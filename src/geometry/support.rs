//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §14, §16, §17, §18] The types
//! of SQL/MM Spatial beside the geometry types, built in, each a kind of
//! type through the SqlType contract (RULES.md D53,
//! D-SPATIAL-SUPPORT): ST_SpatialRefSys, ST_Angle, ST_Direction,
//! ST_Vector, ST_AffinePlacement, ST_NURBSPoint and ST_Knot; their
//! routines (`ops.rs` and a module of each); and the support routines of
//! geometry arrays (`arrays.rs`). ST_TINElement, of §17.1 too, is
//! `geometry/tin_element.rs`'s.
//!
//! Each type is ordered as its clause declares. ST_SpatialRefSys and
//! ST_Vector are EQUALS ONLY BY RELATIVE with ST_OrderingEquals
//! [§14.1.7, §17.2.20], each a family of one type, so `=` and `<>`
//! compare them as the geometry types are compared and nothing sorts or
//! groups them. ST_Angle and ST_Direction are ORDER FULL BY RELATIVE with
//! ST_OrderingCompare [§16.1.19, §16.2.21], which compares radians, so
//! their values compare, sort, group and key by radians. The other three
//! declare no ordering, so no comparison takes them [ISO/IEC
//! 9075-2:2023 §4.2.4].
//!
//! A constructor is a function of the type's name, which NEW invokes
//! (`binder/structured/types.rs`). A method is a row of the type's
//! methods (`methods`), bound by `binder/structured/support_methods.rs`.

mod angles;
mod arrays;
mod directions;
mod elements;
mod ops;
mod placement;
mod systems;
mod transforms;
mod types;
mod values;
mod vector_formats;
mod vectors;

use std::sync::Arc;

pub use arrays::{ArrayProcedure, procedures};
pub use ops::{Form, Op, Routine, Ty};
pub use transforms::{TRANSFORM_GROUPS, TransformGroup};
pub use types::SupportType;
pub(crate) use values::ordering_equals;

use crate::functions::ScalarFunction;
use crate::value::types::{ExtensionType, ExtensionValue, Kind as TypeKind, TypeParams};
use crate::value::{DataType, SqlType, Value};

/// One of the types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// [§14.1] ST_SpatialRefSys.
    SpatialRefSys,
    /// [§16.1] ST_Angle.
    Angle,
    /// [§16.2] ST_Direction.
    Direction,
    /// [§17.2] ST_Vector.
    Vector,
    /// [§17.3] ST_AffinePlacement.
    AffinePlacement,
    /// [§17.4] ST_NURBSPoint.
    NurbsPoint,
    /// [§17.5] ST_Knot.
    Knot,
}

/// The types, in the order of their clauses.
pub const SUPPORTS: [Support; 7] = [
    Support::SpatialRefSys,
    Support::Angle,
    Support::Direction,
    Support::Vector,
    Support::AffinePlacement,
    Support::NurbsPoint,
    Support::Knot,
];

/// How a type's values are ordered [ISO/IEC 9075-2:2023 §4.13].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ordering {
    /// EQUALS ONLY BY RELATIVE WITH ST_OrderingEquals.
    EqualsOnly,
    /// ORDER FULL BY RELATIVE WITH ST_OrderingCompare.
    Full,
    /// None declared.
    None,
}

impl Support {
    /// Its name, as a type is written.
    pub const fn name(self) -> &'static str {
        match self {
            Support::SpatialRefSys => "ST_SPATIALREFSYS",
            Support::Angle => "ST_ANGLE",
            Support::Direction => "ST_DIRECTION",
            Support::Vector => "ST_VECTOR",
            Support::AffinePlacement => "ST_AFFINEPLACEMENT",
            Support::NurbsPoint => "ST_NURBSPOINT",
            Support::Knot => "ST_KNOT",
        }
    }

    /// Its name as the standard writes it.
    pub const fn sql_name(self) -> &'static str {
        match self {
            Support::SpatialRefSys => "ST_SpatialRefSys",
            Support::Angle => "ST_Angle",
            Support::Direction => "ST_Direction",
            Support::Vector => "ST_Vector",
            Support::AffinePlacement => "ST_AffinePlacement",
            Support::NurbsPoint => "ST_NURBSPoint",
            Support::Knot => "ST_Knot",
        }
    }

    /// The subclause that defines it.
    pub const fn clause(self) -> &'static str {
        match self {
            Support::SpatialRefSys => "§14.1",
            Support::Angle => "§16.1",
            Support::Direction => "§16.2",
            Support::Vector => "§17.2",
            Support::AffinePlacement => "§17.3",
            Support::NurbsPoint => "§17.4",
            Support::Knot => "§17.5",
        }
    }

    /// Its ordering, as its clause declares it.
    pub const fn ordering(self) -> Ordering {
        match self {
            Support::SpatialRefSys | Support::Vector => Ordering::EqualsOnly,
            Support::Angle | Support::Direction => Ordering::Full,
            Support::AffinePlacement | Support::NurbsPoint | Support::Knot => Ordering::None,
        }
    }

    /// The subclause of its ordering definition, of a type that declares
    /// one; else its own clause.
    pub const fn ordering_clause(self) -> &'static str {
        match self {
            Support::SpatialRefSys => "§14.1.7",
            Support::Angle => "§16.1.19",
            Support::Direction => "§16.2.21",
            Support::Vector => "§17.2.20",
            Support::AffinePlacement | Support::NurbsPoint | Support::Knot => self.clause(),
        }
    }

    /// Its place in `SUPPORTS`.
    const fn at(self) -> usize {
        match self {
            Support::SpatialRefSys => 0,
            Support::Angle => 1,
            Support::Direction => 2,
            Support::Vector => 3,
            Support::AffinePlacement => 4,
            Support::NurbsPoint => 5,
            Support::Knot => 6,
        }
    }

    /// The type named `name`, upper case, if one is.
    pub fn named(name: &str) -> Option<Support> {
        SUPPORTS.into_iter().find(|s| s.name() == name)
    }
}

/// The types as kinds of type.
static REFS: [&dyn SqlType; 7] =
    [&types::TYPES[0], &types::TYPES[1], &types::TYPES[2], &types::TYPES[3], &types::TYPES[4], &types::TYPES[5], &types::TYPES[6]];

/// The types as a type holds them.
static KIND_OF: [TypeKind; 7] = [&REFS[0], &REFS[1], &REFS[2], &REFS[3], &REFS[4], &REFS[5], &REFS[6]];

/// The kinds of type, in the order of their clauses.
pub fn kinds() -> &'static [TypeKind] {
    &KIND_OF
}

/// The kind of type of `s`.
pub fn kind_of(s: Support) -> TypeKind {
    KIND_OF.get(s.at()).unwrap_or(&KIND_OF[0])
}

/// The data type of `s`.
pub fn data_type(s: Support) -> DataType {
    DataType::Extension(ExtensionType { kind: kind_of(s), params: TypeParams::default() })
}

/// The type of the kind of type `t`, if it is one of these.
pub fn support_kind(t: &dyn SqlType) -> Option<Support> {
    SUPPORTS.into_iter().find(|s| REFS.get(s.at()).is_some_and(|r| std::ptr::addr_eq(*r, t)))
}

/// The type of the data type `t`, if it is one of these.
pub fn of_data_type(t: &DataType) -> Option<Support> {
    let DataType::Extension(e) = t else { return None };
    support_kind(*e.kind)
}

/// The type of the value `v` and its bytes, if it is a value of these.
pub fn of_value(v: &Value) -> Option<(Support, &[u8])> {
    let Value::Extension(e) = v else { return None };
    support_kind(e.kind).map(|s| (s, e.bytes.as_ref()))
}

/// A value of `s` of bytes `bytes`, as `values.rs` writes them.
pub(crate) fn wrap(s: Support, bytes: Vec<u8>) -> Value {
    let kind: &'static dyn SqlType = REFS.get(s.at()).copied().unwrap_or(REFS[0]);
    Value::Extension(ExtensionValue { kind, bytes: bytes.into() })
}

/// [§17.2] The coordinates of the ST_Vector value `v`, none of the empty
/// vector or the null value.
///
/// # Errors
///
/// SQLCODE -301 if `v` is of another type.
pub fn vector_coordinates(v: &Value) -> crate::error::Result<(Option<f64>, Option<f64>, Option<f64>)> {
    if v.is_null() {
        return Ok((None, None, None));
    }
    let vector = values::Vector::of(v)?;
    Ok((vector.x, vector.y, vector.z))
}

/// [§17.2] The ST_Vector value of coordinates `x`, `y` and `z`, of SRID 0.
pub fn vector_value(x: Option<f64>, y: Option<f64>, z: Option<f64>) -> Value {
    values::Vector { x, y, z, srid: 0 }.value()
}

/// [Annex A.1] The meta-variables of the types' sizes: each name, value
/// and description, as ST_SIZINGS shows them [§20.5].
pub fn sizings() -> Vec<(&'static str, Option<i64>, &'static str)> {
    let n = |v: u32| Some(i64::from(v));
    vec![
        ("ST_MaxAngleAsGML", n(angles::MAX_ANGLE_AS_GML), "the characters of an angle's GML"),
        ("ST_MaxAngleAsText", n(angles::MAX_ANGLE_AS_TEXT), "the characters of an angle's well-known text"),
        ("ST_MaxAngleString", n(angles::MAX_ANGLE_STRING), "the characters of ST_String of an angle"),
        ("ST_MaxDirectionAsGML", n(directions::MAX_DIRECTION_AS_GML), "the characters of a direction's GML"),
        ("ST_MaxDirectionAsText", n(directions::MAX_DIRECTION_AS_TEXT), "the characters of a direction's well-known text"),
        ("ST_MaxDirectionString", n(directions::MAX_DIRECTION_STRING), "the characters of a bearing or azimuth string"),
        ("ST_MaxSRSAsText", n(systems::MAX_SRS_AS_TEXT), "the characters of a system's well-known text"),
        ("ST_MaxVectorArrayElements", n(super::MAX_GEOMETRY_ARRAY_ELEMENTS), "the elements of an array of vectors"),
        ("ST_MaxVectorAsBinary", n(vectors::MAX_VECTOR_AS_BINARY), "the octets of a vector's well-known binary"),
        ("ST_MaxVectorAsGML", n(vectors::MAX_VECTOR_AS_GML), "the characters of a vector's GML"),
        ("ST_MaxVectorAsText", n(vectors::MAX_VECTOR_AS_TEXT), "the characters of a vector's well-known text"),
    ]
}

/// The routines registered as functions: the constructors and the
/// functions of §14, §16, §17 and §18.
pub(crate) fn functions() -> Vec<Arc<dyn ScalarFunction>> {
    ops::all().filter(|op| op.form != Form::Method).map(|op| -> Arc<dyn ScalarFunction> { Arc::new(Routine(op)) }).collect()
}

/// The methods `name`, upper case, of `arity` parameters after SELF that
/// type `s` declares.
pub fn methods(s: Support, name: &str, arity: usize) -> Vec<&'static Op> {
    ops::all()
        .filter(|op| op.form == Form::Method && op.name == name && op.params.len() == arity.saturating_add(1))
        .filter(|op| op.params.first() == Some(&Ty::Of(s)))
        .collect()
}

#[cfg(test)]
mod tests;

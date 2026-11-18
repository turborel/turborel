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
pub static METHODS: [Method; 29] = [
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

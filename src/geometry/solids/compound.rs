//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §8.7.2, §8.7.3] The
//! constructors of a compound surface, ST_CompoundSurface() and of a
//! representation, a surface or an array of them, with an SRID or 0, and
//! the mutator ST_Surfaces(ST_Surface ARRAY), which the constructors of
//! surfaces call on the empty value. Each two surfaces next in the array
//! meet in curves, as §8.7.3's definition checks them pair by pair.

use super::represented::{appended, is_representation, own, read, representation_type};
use super::shells::array;
use super::{integer, signature, spatial};
use crate::error::{Result, SqlState};
use crate::functions::{AppendedQuery, ScalarFunction};
use crate::geometry::engine::{engine_of, geom_of};
use crate::geometry::model::{Body, Dims, Geom, Kind, Surface};
use crate::geometry::{data_type_of, elements, of_data_type};
use crate::spatial::planar::{Operation, overlay};
use crate::spatial::solids::ops::{SetOp, set_op};
use crate::value::{DataType, Spelled, Value};

const NAME: &str = "ST_COMPOUNDSURFACE";
const MUTATOR: &str = "ST_SURFACES";
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
/// The most arguments of a constructor, its representation or surfaces
/// and the SRID.
const MOST: usize = 2;

/// ST_CompoundSurface, called as a function or by NEW.
#[derive(Debug, Clone, Copy)]
pub struct CompoundSurface;

/// [§8.7.3] ST_Surfaces(ST_Surface ARRAY), a method of a compound
/// surface.
#[derive(Debug, Clone, Copy)]
pub struct Surfaces;

impl ScalarFunction for CompoundSurface {
    fn name(&self) -> &'static str {
        NAME
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let surfaces = |t: &DataType| matches!(t, DataType::Array(_)) || of_data_type(t).is_some_and(|k| k.is_subtype_of(Kind::Surface));
        let first = args.first().is_none_or(|t| is_representation(t) || surfaces(t));
        if !first || own(args) > MOST {
            return Err(signature(NAME));
        }
        Ok(data_type_of(Kind::CompoundSurface))
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        match position {
            0 => representation_type(known).or_else(|| {
                let given = known.first().cloned().flatten();
                given.filter(|t| matches!(t, DataType::Array(_)) || of_data_type(t).is_some()).or_else(|| Some(data_type_of(Kind::Surface)))
            }),
            1 => Some(INTEGER),
            _ => None,
        }
    }

    fn catalog_query(&self, _: &[Option<&Value>], types: &[DataType], _: &dyn crate::catalog::Catalog) -> Option<AppendedQuery> {
        appended(types)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        if args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        if let Some(read) = read(Kind::CompoundSurface, args)? {
            return Ok(read);
        }
        let (srid, surfaces) = match args {
            [] => return Ok(Geom::new(0, Dims::Xy, Body::Surface(Surface::Compound(Vec::new())))?.to_value()),
            [given] => (0, given),
            [given, srid] => (integer(srid)?, given),
            _ => return Err(signature(NAME)),
        };
        let srid = i32::try_from(srid).map_err(|_| signature(NAME))?;
        let one = || Value::Array(vec![surfaces.clone()].into());
        let array = if let Value::Array(_) = surfaces { surfaces.clone() } else { one() };
        let empty = Geom::new(srid, Dims::Xy, Body::Surface(Surface::Compound(Vec::new())))?;
        with_surfaces(&empty, &array)
    }

    fn deterministic(&self) -> bool {
        true
    }
}

impl ScalarFunction for Surfaces {
    fn name(&self) -> &'static str {
        MUTATOR
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [first, _] = args else { return Err(signature(MUTATOR)) };
        of_data_type(first).filter(|k| k.is_subtype_of(Kind::CompoundSurface)).ok_or_else(|| signature(MUTATOR))?;
        Ok(first.clone())
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        match position {
            0 => known.first().cloned().flatten().or_else(|| Some(data_type_of(Kind::CompoundSurface))),
            1 => Some(array()),
            _ => None,
        }
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let [first, surfaces] = args else { return Err(signature(MUTATOR)) };
        // [§8.7.3 Description 4a] the array is checked though SELF is null.
        check_nulls(surfaces)?;
        if first.is_null() {
            return Ok(Value::Null);
        }
        with_surfaces(&Geom::from_value(first)?, surfaces)
    }

    fn deterministic(&self) -> bool {
        true
    }
}

/// [§18.1.1] ST_CheckNulls: that `array` is not null and holds no null.
///
/// # Errors
///
/// SQLSTATE 2FF03 if it is null or holds one.
fn check_nulls(array: &Value) -> Result<()> {
    let held = if let Value::Array(items) = array { items.iter().any(Value::is_null) } else { false };
    if array.is_null() || held {
        return Err(spatial(SqlState::SPATIAL_NULL_ARGUMENT, "§18.1.1", "the array is null or holds the null value"));
    }
    Ok(())
}

/// [§8.7.3 Description 4] `g` with the surfaces of `array`.
///
/// # Errors
///
/// SQLSTATE 2FF03 for a null array or element; 2FF10 for a surface of
/// another reference system; 2FF85 for two next surfaces that do not
/// meet in curves; 2FF25 for mixed coordinate dimensions; 2FF08 for an
/// element that is no surface; 2FF06 for an empty one.
pub fn with_surfaces(g: &Geom, array: &Value) -> Result<Value> {
    check_nulls(array)?;
    let Value::Array(items) = array else { return Err(signature(MUTATOR)) };
    let parts = items.iter().map(Geom::from_value).collect::<Result<Vec<_>>>()?;
    if parts.iter().any(|p| p.srid != g.srid) {
        return Err(spatial(
            SqlState::SPATIAL_MIXED_SPATIAL_REFERENCE_SYSTEMS,
            "§8.7.3 Description 4b ii",
            "mixed spatial reference systems",
        ));
    }
    for pair in parts.windows(2) {
        let [one, two] = pair else { continue };
        if !meet_in_curves(one, two)? {
            return Err(spatial(SqlState::SPATIAL_NON_CONTIGUOUS_SURFACES, "§8.7.3 Description 4b iii", "non-contiguous surfaces"));
        }
    }
    let dims = parts.first().map_or(Dims::Xy, |p| p.dims);
    if parts.iter().any(|p| p.dims != dims) {
        return Err(spatial(SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS, "§18.1.3", "mixed coordinate dimensions"));
    }
    let surfaces = parts.into_iter().map(|p| if let Body::Surface(s) = p.body { Some(s) } else { None }).collect::<Option<Vec<_>>>();
    let surfaces = surfaces
        .ok_or_else(|| spatial(SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE, "§8.7.1", "an element of a compound surface is a surface"))?;
    Ok(Geom::new(g.srid, dims, Body::Surface(Surface::Compound(surfaces)))?.to_value())
}

/// Whether the intersection of `one` and `two` is of dimension 1: in
/// space if both have z, in the coordinate plane else.
fn meet_in_curves(one: &Geom, two: &Geom) -> Result<bool> {
    let shared = if one.dims.has_z() && two.dims.has_z() {
        set_op(SetOp::Intersection, &super::relations::linear(one)?, &super::relations::linear(two)?)?
    } else {
        geom_of(&overlay(&engine_of(one)?, &engine_of(two)?, Operation::Intersection)?, one.srid)?
    };
    Ok(dimension(&shared) == Some(1))
}

/// [§5.1.2] The dimension of `g`, none of the empty set: a collection's
/// is its greatest element's.
fn dimension(g: &Geom) -> Option<i16> {
    if g.is_empty() {
        return None;
    }
    match elements(g) {
        Some(all) => all.into_iter().filter(|b| !b.is_empty()).filter_map(|b| b.kind().dimension()).max(),
        None => g.kind().dimension(),
    }
}

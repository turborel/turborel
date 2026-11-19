//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §9.2.1 Description 9, §9.2.2]
//! The constructors of a B-rep solid: ST_BRepSolid(), the empty solid;
//! of a representation [Description 1 to 8] (`represented.rs`); and of
//! shells, ST_BRepSolid(ST_Surface [, ST_Surface ARRAY] [, INTEGER])
//! [Description 9 to 16]. A solid of shells is made empty, of the SRID
//! given or 0, and given its exterior shell and then its interior shells
//! by the mutators, which check them.

use super::represented::{appended, is_representation, own, read, representation_type};
use super::shells::{array, replaced};
use super::{integer, signature};
use crate::error::Result;
use crate::functions::{AppendedQuery, ScalarFunction};
use crate::geometry::model::{Body, Dims, Geom, Kind};
use crate::geometry::{data_type_of, of_data_type};
use crate::value::{DataType, Spelled, Value};

/// ST_BRepSolid of shells, called as a function.
#[derive(Debug, Clone, Copy)]
pub struct BRepSolid;

const NAME: &str = "ST_BREPSOLID";
const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

impl ScalarFunction for BRepSolid {
    fn name(&self) -> &'static str {
        NAME
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let surface = |t: &DataType| of_data_type(t).is_some_and(|k| k.is_subtype_of(Kind::Surface) || Kind::Surface.is_subtype_of(k));
        let represented = args.first().is_some_and(is_representation);
        let most = if represented { 2 } else { 3 };
        if !args.first().is_none_or(|t| represented || surface(t)) || own(args) > most {
            return Err(signature(NAME));
        }
        Ok(data_type_of(Kind::BRepSolid))
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        let is_array = |t: &Option<DataType>| matches!(t, Some(DataType::Array(_)));
        if let Some(t) = representation_type(known) {
            return [Some(t), Some(INTEGER)].into_iter().nth(position).flatten();
        }
        if position == 0 {
            let declared = known.first().cloned().flatten().filter(|t| of_data_type(t).is_some());
            return declared.or_else(|| Some(data_type_of(Kind::Surface)));
        }
        let shells_second = known.len() == 3 || known.get(1).is_some_and(is_array);
        match (position, shells_second) {
            (1, true) => Some(array()),
            (1 | 2, _) => Some(INTEGER),
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
        if let Some(read) = read(Kind::BRepSolid, args)? {
            return Ok(read);
        }
        let [surface, rest @ ..] = args else { return Ok(Geom::new(0, Dims::Xyz, Body::Solid(Vec::new()))?.to_value()) };
        let (shells, srid) = match rest {
            [] => (None, 0),
            [Value::Array(_)] => (rest.first(), 0),
            [one] => (None, integer(one)?),
            [shells, srid] => (Some(shells), integer(srid)?),
            _ => return Err(signature(NAME)),
        };
        let srid = i32::try_from(srid).map_err(|_| signature(NAME))?;
        let empty = Geom::new(srid, Dims::Xyz, Body::Solid(Vec::new()))?;
        let solid = Geom::from_value(&replaced(&empty, surface, true)?)?;
        match shells {
            Some(array) => replaced(&solid, array, false),
            None => Ok(solid.to_value()),
        }
    }

    fn deterministic(&self) -> bool {
        true
    }
}

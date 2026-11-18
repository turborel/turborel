//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.65] ST_OrderingEquals, the
//! ordering of ST_Geometry: EQUALS ONLY BY RELATIVE [§5.2], so a
//! comparison `a = b` of geometry values is `ST_OrderingEquals(a, b) = 0`
//! (`binder/structured/orderings.rs`), and no value of the family is
//! grouped, sorted or a key (RULES.md D-SPATIAL-FAMILY).
//!
//! It is 0 if the two are spatially equal [§5.1.65 GR], by `EQUALITY`,
//! which the planar engine supplies (RULES.md D-SPATIAL-FAMILY); until it
//! does, two values are equal if their stored forms are, or both are
//! empty of one SRID, and else unequal.

use super::kind::{data_type, payload};
use super::model::Geom;
use crate::error::{Code, Result, SqlError};
use crate::functions::ScalarFunction;
use crate::value::{DataType, Decimal, Spelled, Value};

/// The name of ST_OrderingEquals.
pub const ORDERING_EQUALS: &str = "ST_ORDERINGEQUALS";

/// \[DESIGN\] Whether two values are spatially equal [§5.1.65 GR2a, 2b]:
/// of x and y, or of x, y and z if both have z. The planar engine puts its
/// ST_Equals and ST_3DEquals here.
pub const EQUALITY: fn(&Geom, &Geom) -> Result<bool> = |a, b| Ok(stored_equality(a, b));

/// Equal stored forms, or both empty of one SRID.
fn stored_equality(a: &Geom, b: &Geom) -> bool {
    a.srid == b.srid && (a == b || (a.is_empty() && b.is_empty()))
}

/// ST_ORDERINGEQUALS(a, b): INTEGER 0 if equal, else 1.
#[derive(Debug, Clone, Copy)]
pub struct OrderingEquals;

impl ScalarFunction for OrderingEquals {
    fn name(&self) -> &'static str {
        ORDERING_EQUALS
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if args.len() != 2 || !args.iter().all(|t| super::of_data_type(t).is_some()) {
            return Err(SqlError::new(
                Code::SyntaxRule,
                "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.65] ST_OrderingEquals takes two ST_Geometry values",
            ));
        }
        Ok(DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer })
    }

    fn argument_type(&self, position: usize, _: &[Option<DataType>]) -> Option<DataType> {
        (position < 2).then(data_type)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let [a, b] = args else {
            return Err(SqlError::new(
                Code::SyntaxRule,
                "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.65] ST_OrderingEquals takes two values",
            ));
        };
        if a.is_null() || b.is_null() {
            return Ok(Value::Null);
        }
        let same = payload(a).zip(payload(b)).is_some_and(|(x, y)| x == y);
        let equal = same || EQUALITY(&Geom::from_value(a)?, &Geom::from_value(b)?)?;
        Ok(Value::Exact(Decimal::new(i128::from(!equal), 0)))
    }
}

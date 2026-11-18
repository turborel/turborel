//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.65] ST_OrderingEquals, the
//! ordering of ST_Geometry: EQUALS ONLY BY RELATIVE [§5.2], so a
//! comparison `a = b` of geometry values is `ST_OrderingEquals(a, b) = 0`
//! (`binder/structured/orderings.rs`), and no value of the family is
//! grouped, sorted or a key (RULES.md D120).
//!
//! It is 0 if the two are spatially equal [§5.1.65 GR], by `EQUALITY`,
//! which the planar engine supplies (RULES.md D120). Values
//! of one SRID are equal when their point sets of x and y are, by the
//! exact relationship of ST_Equals [§5.1.43].

use super::kind::{data_type, payload};
use super::model::Geom;
use crate::error::{Code, Result, SqlError};
use crate::functions::ScalarFunction;
use crate::value::{DataType, Decimal, Spelled, Value};

/// The name of ST_OrderingEquals.
pub const ORDERING_EQUALS: &str = "ST_ORDERINGEQUALS";

/// \[DESIGN\] Whether two values are spatially equal [§5.1.65 GR2a],
/// by the planar engine's exact ST_Equals of x and y (`engine.rs`).
pub const EQUALITY: fn(&Geom, &Geom) -> Result<bool> = super::engine::equal;

/// ST_ORDERINGEQUALS(a, b): INTEGER 0 if equal, else 1.
#[derive(Debug, Clone, Copy)]
pub struct OrderingEquals;

impl ScalarFunction for OrderingEquals {
    fn name(&self) -> &'static str {
        ORDERING_EQUALS
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let geometries = args.iter().all(|t| super::of_data_type(t).is_some());
        // [§14.1.7, §17.2.20] of two ST_SpatialRefSys or two ST_Vector values.
        let support = |t: &DataType| super::support::of_data_type(t).filter(|s| s.ordering() == super::support::Ordering::EqualsOnly);
        let supported = if let [a, b] = args { support(a).is_some() && support(a) == support(b) } else { false };
        if args.len() != 2 || !(geometries || supported) {
            return Err(SqlError::new(
                Code::SyntaxRule,
                "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.65, §14.1.7, §17.2.20] ST_OrderingEquals takes two ST_Geometry values, two ST_SpatialRefSys values or two ST_Vector values",
            ));
        }
        Ok(DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer })
    }

    fn argument_type(&self, position: usize, args: &[Option<DataType>]) -> Option<DataType> {
        // [§14.1.7, §17.2.20] two ST_SpatialRefSys or two ST_Vector values
        // keep their type.
        let kept = args.get(position).cloned().flatten().filter(|t| super::support::of_data_type(t).is_some());
        (position < 2).then(|| kept.unwrap_or_else(data_type))
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
        if let Some(equal) = super::support::ordering_equals(a, b) {
            return Ok(Value::Exact(Decimal::new(i128::from(!equal?), 0)));
        }
        let same = payload(a).zip(payload(b)).is_some_and(|(x, y)| x == y);
        let equal = same || EQUALITY(&Geom::from_value(a)?, &Geom::from_value(b)?)?;
        Ok(Value::Exact(Decimal::new(i128::from(!equal), 0)))
    }
}

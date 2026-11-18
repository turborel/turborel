//! \[EXTENSION\] System change numbers [GS §8, §19]: what GRAPH_TABLE's
//! `AS OF SCN n` reads its element tables at.
//!
//! \[DESIGN\] A system-versioned table's rows are stamped with the
//! transaction timestamp of the transaction changing them, an instant in
//! microseconds of UTC (RULES.md D82), and FOR SYSTEM_TIME AS OF reads
//! them at an instant. The logical timestamps commits are ordered by
//! (RULES.md D29) are kept with no row, so no read can be made at one.
//! The SCN of a change is therefore its transaction timestamp's count of
//! microseconds from 0001-01-01 00:00:00 UTC, the number a system-time
//! period is kept in, and `AS OF SCN n` is `AS OF TIMESTAMP
//! SCN_TO_TIMESTAMP(n)`. `TIMESTAMP_TO_SCN(CURRENT_TIMESTAMP)` is the
//! current SCN, and `TIMESTAMP_TO_SCN` of a row's system-time start the
//! SCN of the change that wrote it.

use std::sync::Arc;

use super::ScalarFunction;
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::params::ParameterType;
use crate::value::datetime::{LAST_INSTANT, Moment, Now};
use crate::value::types::datetime::current;
use crate::value::{BIGINT_PRECISION, DataType, Decimal, Spelled, Value};

/// \[EXTENSION\] SCN_TO_TIMESTAMP(n): the instant of SCN n.
pub const SCN_TO_TIMESTAMP: &str = "SCN_TO_TIMESTAMP";
/// \[EXTENSION\] TIMESTAMP_TO_SCN(ts): the SCN of instant ts.
pub const TIMESTAMP_TO_SCN: &str = "TIMESTAMP_TO_SCN";

/// The type of an SCN: BIGINT.
fn scn_type() -> DataType {
    DataType::Exact { precision: BIGINT_PRECISION, scale: 0, spelled: Spelled::Bigint }
}

/// The two functions.
pub(super) fn functions() -> Vec<Arc<dyn ScalarFunction>> {
    vec![Arc::new(ScnToTimestamp), Arc::new(TimestampToScn)]
}

/// SCN_TO_TIMESTAMP.
struct ScnToTimestamp;

impl ScalarFunction for ScnToTimestamp {
    fn name(&self) -> &str {
        SCN_TO_TIMESTAMP
    }

    /// An exact number of scale 0; TIMESTAMP(6) WITH TIME ZONE.
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if let [DataType::Exact { scale: 0, .. }] = args {
            return Ok(DataType::Timestamp { precision: 6, zoned: true });
        }
        fail(Code::SyntaxRule, format!("[EXTENSION] {SCN_TO_TIMESTAMP} takes one exact number of scale 0, an SCN"))
    }

    /// A `?` SCN BIGINT.
    fn parameter_type(&self, _position: usize, _args: &[Option<DataType>]) -> Option<ParameterType> {
        Some(ParameterType::Of(scn_type()))
    }

    /// The instant `n` microseconds from 0001-01-01 00:00:00 UTC, at
    /// +00:00; SQLSTATE 22008 if it is no instant of a timestamp, so no
    /// change was or can be made at it.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(Value::Exact(d)) = args.first() else { return Ok(Value::Null) };
        let utc = i64::try_from(d.units).ok().filter(|u| (0..=LAST_INSTANT).contains(u));
        let Some(utc) = utc.filter(|_| d.scale == 0) else {
            return fail_with(
                Code::Data,
                SqlState::DATETIME_FIELD_OVERFLOW,
                format!("[EXTENSION] SCN {d} is out of range: an SCN is from 0 to {LAST_INSTANT}"),
            );
        };
        Ok(current(Moment::Timestamp, 6, Now { utc, zone: 0 }))
    }
}

/// TIMESTAMP_TO_SCN.
struct TimestampToScn;

impl ScalarFunction for TimestampToScn {
    fn name(&self) -> &str {
        TIMESTAMP_TO_SCN
    }

    /// A timestamp; BIGINT.
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if let [DataType::Timestamp { .. }] = args {
            return Ok(scn_type());
        }
        fail(Code::SyntaxRule, format!("[EXTENSION] {TIMESTAMP_TO_SCN} takes one timestamp"))
    }

    /// Its microseconds from 0001-01-01 00:00:00 UTC.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(Value::Datetime(d)) = args.first() else { return Ok(Value::Null) };
        Ok(Value::Exact(Decimal::new(i128::from(d.utc), 0)))
    }
}

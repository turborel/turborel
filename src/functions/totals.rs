//! SUM and AVG, the SQL-86 set functions of numbers [X3.135-1986 §5.8]:
//! their result types. Their accumulators are in `sum.rs`.

use super::sum::{AvgAcc, SumAcc};
use super::{Accumulator, AggregateFunction};
use crate::error::{Code, Result, fail};
use crate::lexer::kw;
use crate::value::{DOUBLE_PRECISION, DataType, MAX_DECFLOAT_PRECISION, MAX_EXACT_PRECISION, Spelled};

/// [X3.135-1986 §5.8 SR9c] implementor-defined: AVG of an exact numeric with scale S has
/// scale S + `AVG_EXTRA_SCALE`, truncated toward zero.
pub const AVG_EXTRA_SCALE: u32 = 2;

fn not_character(name: &str, arg: Option<DataType>) -> Result<DataType> {
    match arg.filter(|t| t.is_numeric() || matches!(t, DataType::Interval(_))) {
        Some(t) => Ok(t),
        None => fail(
            Code::SyntaxRule,
            format!("[X3.135-1986 §5.8 SR9a; ISO/IEC 9075:1992 §6.5 SR9a] the argument of {name} shall be a number or an interval"),
        ),
    }
}

/// SUM. [X3.135-1986 §5.8 SR9b] exact with scale S stays exact with scale S. [ISO/IEC 9075:1992 §6.5 GR2b iv]
/// the sum, not each sum on the way to it, shall be within range (`sum.rs`). [X3.135-1986 §5.8 GR4c] null when S is
/// empty.
pub(super) struct Sum;

impl AggregateFunction for Sum {
    fn name(&self) -> &'static str {
        kw::SUM
    }
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        Ok(match not_character(kw::SUM, arg)? {
            DataType::Exact { scale, .. } => DataType::Exact { precision: MAX_EXACT_PRECISION, scale, spelled: Spelled::Numeric },
            // [ISO/IEC 9075:1992 §6.5 SR9e] of the interval's precision.
            DataType::Interval(q) => DataType::Interval(q),
            // [ISO/IEC 9075-2:2023 §10.9 SR7g v] DECFLOAT of 34 digits (IV170).
            DataType::DecFloat { .. } => DataType::DecFloat { precision: MAX_DECFLOAT_PRECISION },
            // [X3.135-1986 §5.8 SR9d]; `not_character` has refused the types not
            // numbers or intervals.
            DataType::Approximate { .. }
            | DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Binary { .. }
            | DataType::VarBinary { .. }
            | DataType::Lob { .. }
            | DataType::Boolean
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Array(_)
            | DataType::Multiset(_)
            | DataType::Structured(_)
            | DataType::Reference(_)
            | DataType::Row(_)
            | DataType::Distinct(_)
            | DataType::Json
            | DataType::Extension(_) => DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float },
        })
    }
    fn accumulator(&self, result: DataType) -> Box<dyn Accumulator> {
        Box::new(SumAcc::new(result))
    }
}

/// AVG. [ISO/IEC 9075:1992 §6.5 GR2b ii] the average, which alone shall be within range (`sum.rs`): SQL-92 drops
/// the range of the sum that [X3.135-1986 §5.8 GR4f] sets (RULES.md D32).
pub(super) struct Avg;

impl AggregateFunction for Avg {
    fn name(&self) -> &'static str {
        kw::AVG
    }
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        Ok(match not_character(kw::AVG, arg)? {
            DataType::Exact { scale, .. } => DataType::Exact {
                precision: MAX_EXACT_PRECISION,
                scale: scale.saturating_add(AVG_EXTRA_SCALE).min(MAX_EXACT_PRECISION),
                spelled: Spelled::Numeric,
            },
            // [ISO/IEC 9075:1992 §6.5 SR9e] of the interval's precision.
            DataType::Interval(q) => DataType::Interval(q),
            // [ISO/IEC 9075-2:2023 §10.9 SR7g v] DECFLOAT of 34 digits (IV170).
            DataType::DecFloat { .. } => DataType::DecFloat { precision: MAX_DECFLOAT_PRECISION },
            // `not_character` has refused the types not numbers or intervals.
            DataType::Approximate { .. }
            | DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Binary { .. }
            | DataType::VarBinary { .. }
            | DataType::Lob { .. }
            | DataType::Boolean
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Array(_)
            | DataType::Multiset(_)
            | DataType::Structured(_)
            | DataType::Reference(_)
            | DataType::Row(_)
            | DataType::Distinct(_)
            | DataType::Json
            | DataType::Extension(_) => DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float },
        })
    }
    fn accumulator(&self, result: DataType) -> Box<dyn Accumulator> {
        Box::new(AvgAcc::new(result))
    }
}

//! \[DESIGN\] (RULES.md D356) The built-in set functions a grouping keeps
//! in columns of states, one slot a group, in place of an accumulator a
//! group (`exec/table/states.rs`): COUNT, SUM, AVG, MAX and MIN. Each
//! state is finished, written and turned into an accumulator by the
//! accumulator's own code, given the state the columns hold, so a group
//! kept in columns gives the result, the error and the written bytes one
//! kept as an accumulator gives.

use super::Accumulator;
use super::floats::Wide;
use crate::error::Result;
use crate::value::{DataType, Decimal, Value};

/// \[DESIGN\] (RULES.md D356) What built-in set function a function is,
/// as `AggregateFunction::columnar` tells it. Only the core makes one, so
/// no plug-in's function is taken for a built-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Columnar(pub(crate) Kind);

/// The built-in set functions kept in columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

/// A sum kept in a column: of no value yet, of exact numbers as their
/// units at one scale, or of approximate ones (`Wide`).
#[derive(Debug, Clone, Copy)]
pub(crate) enum Summed {
    Empty,
    Exact(Decimal),
    Approx(Wide),
}

/// No approximate number added yet.
pub(crate) const NO_FLOATS: Wide = Wide::EMPTY;

/// The approximate sum `sum` with `f` added, as SUM adds it.
pub(crate) fn plus_float(sum: Wide, f: f64) -> Wide {
    sum.with(f)
}

/// COUNT's accumulator, having counted `n`.
pub(crate) fn count_acc(n: i128) -> Box<dyn Accumulator> {
    Box::new(super::count::CountAcc(n))
}

/// COUNT's result of `n`, as its accumulator finishes it.
///
/// # Errors
///
/// As `Accumulator::finish`, which never fails of a count.
pub(crate) fn count_finished(n: i128) -> Result<Value> {
    super::count::CountAcc(n).finish()
}

/// SUM's accumulator of result type `result`, its total `total`.
pub(crate) fn sum_acc(total: Summed, result: DataType) -> Box<dyn Accumulator> {
    Box::new(super::sum::SumAcc::of(total, result))
}

/// SUM's result of total `total`, of type `result`, as its accumulator
/// finishes it.
///
/// # Errors
///
/// SQLSTATE 22003 if it is out of range [ISO/IEC 9075:1992 §6.5 GR2b iv].
pub(crate) fn sum_finished(total: Summed, result: &DataType) -> Result<Value> {
    super::sum::SumAcc::of(total, result.clone()).finish()
}

/// AVG's accumulator of result type `result`, its total `total` of
/// `count` values.
pub(crate) fn avg_acc(total: Summed, count: i128, result: DataType) -> Box<dyn Accumulator> {
    Box::new(super::sum::AvgAcc::of(total, count, result))
}

/// AVG's result of total `total` of `count` values, of type `result`, as
/// its accumulator finishes it.
///
/// # Errors
///
/// As `Accumulator::finish` of AVG.
pub(crate) fn avg_finished(total: Summed, count: i128, result: &DataType) -> Result<Value> {
    super::sum::AvgAcc::of(total, count, result.clone()).finish()
}

/// MAX's accumulator, if `max`, or MIN's, of result type `result`, the best
/// value so far `best`, null of none.
pub(crate) fn extreme_acc(max: bool, best: Value, result: &DataType) -> Box<dyn Accumulator> {
    super::extremes::ExtremeAcc::of(max, best, result)
}

/// \[DESIGN\] (RULES.md D1186) COUNT's state of `n`, written to `out` as
/// its accumulator writes it, none made on the heap: whether it has one.
pub(crate) fn count_written(n: i128, out: &mut Vec<u8>) -> bool {
    super::count::CountAcc(n).write_state(out)
}

/// \[DESIGN\] (RULES.md D1186) SUM's state of total `total`, of type
/// `result`, written to `out` as its accumulator writes it: whether it
/// has one.
pub(crate) fn sum_written(total: Summed, result: &DataType, out: &mut Vec<u8>) -> bool {
    super::sum::SumAcc::of(total, result.clone()).write_state(out)
}

/// \[DESIGN\] (RULES.md D1186) AVG's state of total `total` of `count`
/// values, of type `result`, written to `out` as its accumulator writes
/// it: whether it has one.
pub(crate) fn avg_written(total: Summed, count: i128, result: &DataType, out: &mut Vec<u8>) -> bool {
    super::sum::AvgAcc::of(total, count, result.clone()).write_state(out)
}

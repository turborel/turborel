//! [ISO/IEC 9075-2:2023 §8.20] The `<period predicate>`s, over periods
//! and a point in time the binder has cast to TIMESTAMP WITH TIME ZONE
//! [GR4a ii, GR4a iv, GR4b iii] (`binder/periods.rs`). A period written
//! PERIOD ( start , end ) makes the predicate Unknown if a bound of it is
//! null \[GR2a\] and fails if its start is not before its end \[GR2b\];
//! a `<period reference>`'s bounds are its columns', which its period
//! constraint keeps so, though an outer join may make them null, and
//! then a comparison is not True, so the predicate is False [GR4].

use std::cmp::Ordering;

use super::ScalarFunction;
use crate::ast::PeriodOperator;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::comparison::compare;
use crate::value::{DataType, Value};

/// [ISO/IEC 9075-2:2023 §8.20] A `<period predicate>` of operator `op`:
/// its arguments the start and end of its first period, then of its
/// second, or after CONTAINS a point in time alone; `written`, of each
/// period, whether PERIOD ( start , end ) gives it.
pub struct PeriodTest {
    /// The predicate.
    pub op: PeriodOperator,
    /// Of the first period and the second, whether each is written.
    pub written: [bool; 2],
}

/// The name of the predicate of `op`, as a plan shows it.
fn name(op: PeriodOperator) -> &'static str {
    match op {
        PeriodOperator::Overlaps => "PERIOD OVERLAPS",
        PeriodOperator::Equals => "PERIOD EQUALS",
        PeriodOperator::Contains => "PERIOD CONTAINS",
        PeriodOperator::Precedes => "PERIOD PRECEDES",
        PeriodOperator::Succeeds => "PERIOD SUCCEEDS",
        PeriodOperator::ImmediatelyPrecedes => "PERIOD IMMEDIATELY PRECEDES",
        PeriodOperator::ImmediatelySucceeds => "PERIOD IMMEDIATELY SUCCEEDS",
    }
}

/// Whether `a` is ordered against `b` as one of `orders`; not if either is
/// null [GR4].
fn is(a: &Value, b: &Value, orders: &[Ordering]) -> bool {
    compare(a, b).is_some_and(|o| orders.contains(&o))
}

/// [ISO/IEC 9075-2:2023 §8.20 GR2a] Whether `pair`, a period, is written
/// and has a null bound.
fn written_null(pair: &[Value], written: bool) -> bool {
    written && pair.iter().any(Value::is_null)
}

/// [ISO/IEC 9075-2:2023 §8.20 GR2b] `pair`'s start and end, a period that
/// is `written` or not; none if `pair` is not two values.
///
/// # Errors
///
/// SQLSTATE 22020 if it is written and its start is not before its end.
fn bounds(pair: &[Value], written: bool) -> Result<Option<(&Value, &Value)>> {
    let [start, end] = pair else { return Ok(None) };
    if written && is(start, end, &[Ordering::Equal, Ordering::Greater]) {
        return Err(SqlError::new(
            Code::Data,
            format!("[ISO/IEC 9075-2:2023 §8.20 GR2b] the period from {start} to {end} does not start before it ends"),
        )
        .with_state(SqlState::INVALID_PERIOD_VALUE));
    }
    Ok(Some((start, end)))
}

impl PeriodTest {
    /// [ISO/IEC 9075-2:2023 §8.20 GR4b iv to ix] The predicate of the
    /// periods from `s1` to `e1` and from `s2` to `e2`.
    fn of_periods(&self, (s1, e1): (&Value, &Value), (s2, e2): (&Value, &Value)) -> bool {
        use Ordering::{Equal, Greater, Less};
        match self.op {
            PeriodOperator::Overlaps => is(s1, e2, &[Less]) && is(e1, s2, &[Greater]),
            PeriodOperator::Equals => is(s1, s2, &[Equal]) && is(e1, e2, &[Equal]),
            // [GR4a v 3] a period of the other's instants.
            PeriodOperator::Contains => is(s1, s2, &[Less, Equal]) && is(e1, e2, &[Greater, Equal]),
            PeriodOperator::Precedes => is(e1, s2, &[Less, Equal]),
            PeriodOperator::Succeeds => is(s1, e2, &[Greater, Equal]),
            PeriodOperator::ImmediatelyPrecedes => is(e1, s2, &[Equal]),
            PeriodOperator::ImmediatelySucceeds => is(s1, e2, &[Equal]),
        }
    }
}

impl ScalarFunction for PeriodTest {
    fn name(&self) -> &str {
        name(self.op)
    }
    /// \[GR2b\] Only a written period's bounds can raise: one a
    /// `<period reference>` names is judged by comparisons alone.
    fn raise_free(&self) -> bool {
        self.written == [false, false]
    }
    fn period_operator(&self) -> Option<PeriodOperator> {
        self.raise_free().then_some(self.op)
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let (first, second) = args.split_at(args.len().min(2));
        let [w1, w2] = self.written;
        // [GR2a] Unknown of a written period's null bound, before [GR2b]
        // judges any.
        if written_null(first, w1) || written_null(second, w2) {
            return Ok(Value::Null);
        }
        let Some((s1, e1)) = bounds(first, w1)? else { return Ok(Value::Null) };
        let Some(p2) = bounds(second, w2)? else {
            // [GR4a v 1, 2] CONTAINS of a point in time: Unknown of a null
            // one, else whether the period holds it.
            let Some(point) = second.first().filter(|v| !v.is_null()) else { return Ok(Value::Null) };
            return Ok(Value::Bool(is(s1, point, &[Ordering::Less, Ordering::Equal]) && is(e1, point, &[Ordering::Greater])));
        };
        Ok(Value::Bool(self.of_periods((s1, e1), p2)))
    }
}

#[cfg(test)]
mod tests;

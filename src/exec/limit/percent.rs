//! [ISO/IEC 9075-2:2023 §7.17 GR7b ii] The count of a `<fetch first
//! percentage>`: CEILING ( FFP * LOCT / 100.0E0 ), LOCT the number of rows
//! before OFFSET passes over any [NOTE 328], in SQL's arithmetic, so of an
//! approximate number, or of a DECFLOAT if the percentage is one.

use super::super::subquery::Subqueries;
use super::super::{broken, eval};
use crate::error::{Code, Result, SqlState, fail_with};
use crate::logical::Expr;
use crate::value::{ArithOp, Decimal, Value, arith};

/// [GR7b ii] `100.0E0`, by which the percentage of the rows is divided.
const HUNDRED: f64 = 100.0_f64;
/// The base of an exponent of ten.
const TEN: i128 = 10;
/// [GR7c] The rule a count below 1 breaks.
const RULE: &str = "[ISO/IEC 9075-2:2023 §7.17 GR7c] the fetch first row count";

/// The rows a percentage `e` of `rows` rows keeps, evaluated with
/// `subqueries`: at most `rows`, as no more are kept of more.
///
/// # Errors
///
/// SQLSTATE 2201W if the percentage is null, or the count below 1
/// \[GR7c\]; its evaluation's, and its arithmetic's.
pub(super) fn count(e: &Expr, rows: usize, subqueries: &dyn Subqueries) -> Result<usize> {
    let percentage = eval(e, &Vec::new(), subqueries)?;
    if percentage.is_null() {
        return fail_with(Code::Data, SqlState::INVALID_ROW_COUNT_IN_FETCH_FIRST, format!("{RULE} is null, not a number of rows"));
    }
    let total = Value::Exact(Decimal::new(i128::try_from(rows).unwrap_or(i128::MAX), 0));
    let quotient = arith(ArithOp::Div, &arith(ArithOp::Mul, &percentage, &total)?, &Value::Approx(HUNDRED))?;
    match ceiling(&quotient, rows)? {
        Some(n) => Ok(n),
        None => fail_with(
            Code::Data,
            SqlState::INVALID_ROW_COUNT_IN_FETCH_FIRST,
            format!("{RULE}, {percentage} percent of {rows} rows, is less than 1"),
        ),
    }
}

/// CEILING ( `q` ), at most `rows`; none if it is below 1.
fn ceiling(q: &Value, rows: usize) -> Result<Option<usize>> {
    match q {
        Value::Approx(x) => Ok(approximate(*x, rows)),
        Value::DecFloat(d) => Ok(exact(d.coefficient(), i64::from(d.exponent()), rows)),
        Value::Exact(d) => Ok(exact(d.units, i64::from(d.scale).saturating_neg(), rows)),
        Value::Null
        | Value::Char(_)
        | Value::Binary(_)
        | Value::Lob(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Extension(_) => Err(broken("a percentage of the rows is not a number")),
    }
}

/// CEILING ( `x` ), at most `rows`; none if below 1, as is every `x` not
/// above 0.
fn approximate(x: f64, rows: usize) -> Option<usize> {
    if x.is_nan() || x <= 0.0_f64 {
        return None;
    }
    let all = Decimal::new(i128::try_from(rows).unwrap_or(i128::MAX), 0).to_f64();
    if x >= all {
        return Some(rows);
    }
    // Above 0 and below `rows`, its ceiling is a whole number of rows.
    format!("{:.0}", x.ceil()).parse::<usize>().ok().map(|n| n.min(rows))
}

/// CEILING ( `c` × 10^`e` ), at most `rows`; none if below 1.
fn exact(c: i128, e: i64, rows: usize) -> Option<usize> {
    if c <= 0 {
        return None;
    }
    let most = i128::try_from(rows).unwrap_or(i128::MAX);
    let whole = match u32::try_from(e) {
        // A whole number: past `rows`, all of them.
        Ok(up) => TEN.checked_pow(up).and_then(|p| c.checked_mul(p)).unwrap_or(most),
        Err(_) => match u32::try_from(e.saturating_neg()).ok().and_then(|down| TEN.checked_pow(down)) {
            Some(p) => c.div_euclid(p).saturating_add(i128::from(c.rem_euclid(p) > 0)),
            // Below 1 and above 0.
            None => 1,
        },
    };
    usize::try_from(whole.min(most)).ok()
}

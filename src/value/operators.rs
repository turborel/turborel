//! The operators shared by the AST, planner and executor: `<comp op>`
//! [X3.135-1986 §5.11] and the dyadic arithmetic operators [§5.9], and
//! arithmetic itself.

use std::cmp::Ordering;

use super::{Value, interval, types};
use crate::error::{Code, Result, SqlState, fail, fail_with};

/// [X3.135-1986 §5.11] `<comp op>` ::= = | <> | < | > | <= | >=
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompOp {
    /// =
    Eq,
    /// <>
    Ne,
    /// <
    Lt,
    /// >
    Gt,
    /// <=
    Le,
    /// >=
    Ge,
}

impl CompOp {
    /// [X3.135-1986 §5.11 GR3] the truth of "x `<op>` y" given how x compares to y.
    pub fn holds(self, ord: Ordering) -> bool {
        match self {
            CompOp::Eq => ord == Ordering::Equal,
            CompOp::Ne => ord != Ordering::Equal,
            CompOp::Lt => ord == Ordering::Less,
            CompOp::Gt => ord == Ordering::Greater,
            CompOp::Le => ord != Ordering::Greater,
            CompOp::Ge => ord != Ordering::Less,
        }
    }
    /// The operator to use when the operands are swapped: "5 < X" is "X > 5".
    #[must_use]
    pub fn flip(self) -> CompOp {
        match self {
            CompOp::Lt => CompOp::Gt,
            CompOp::Gt => CompOp::Lt,
            CompOp::Le => CompOp::Ge,
            CompOp::Ge => CompOp::Le,
            CompOp::Eq | CompOp::Ne => self,
        }
    }
    /// The operator as written in SQL.
    pub fn symbol(self) -> &'static str {
        match self {
            CompOp::Eq => "=",
            CompOp::Ne => "<>",
            CompOp::Lt => "<",
            CompOp::Gt => ">",
            CompOp::Le => "<=",
            CompOp::Ge => ">=",
        }
    }
}

/// [X3.135-1986 §5.9] dyadic arithmetic operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithOp {
    /// +
    Add,
    /// -
    Sub,
    /// *
    Mul,
    /// /
    Div,
}

impl ArithOp {
    /// The operator as written in SQL.
    pub fn symbol(self) -> &'static str {
        match self {
            ArithOp::Add => "+",
            ArithOp::Sub => "-",
            ArithOp::Mul => "*",
            ArithOp::Div => "/",
        }
    }
}

/// [X3.135-1986 §5.9] apply a dyadic operator.
///
/// # Errors
///
/// SQLCODE -301 for a zero divisor [X3.135-1986 §5.9 GR5], a result out of
/// range [X3.135-1986 §5.9 GR6], or a character string operand; of
/// intervals and datetimes, as `interval::add`, `scale` and `shift` say.
pub fn arith(op: ArithOp, a: &Value, b: &Value) -> Result<Value> {
    match (a, b) {
        // [X3.135-1986 §5.9 GR1] if any <primary> is null, the result is null.
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        // [X3.135-1986 §5.9 SR4] exact op exact is exact.
        (Value::Exact(x), Value::Exact(y)) => Ok(Value::Exact(match op {
            ArithOp::Add => x.checked_add(*y)?,
            ArithOp::Sub => x.checked_sub(*y)?,
            ArithOp::Mul => x.checked_mul(*y)?,
            ArithOp::Div => x.checked_div(*y)?,
        })),
        // [ISO/IEC 9075-2:2023 §6.30 SR1a] DECFLOAT if either is.
        (Value::DecFloat(_), Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_))
        | (Value::Exact(_) | Value::Approx(_), Value::DecFloat(_)) => types::decfloat_arith(op, a, b),
        // [ISO/IEC 9075:1992 §6.14, §6.15] of intervals and datetimes.
        (Value::Interval(x), Value::Interval(y)) => interval::add(x, y, op == ArithOp::Sub),
        (Value::Interval(x), Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_)) => interval::scale(x, op, b),
        (Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_), Value::Interval(y)) => interval::scale(y, op, a),
        (Value::Datetime(d), Value::Interval(i)) => interval::shift(d, i, op == ArithOp::Sub),
        (Value::Interval(i), Value::Datetime(d)) => interval::shift(d, i, false),
        // [X3.135-1986 §5.9 SR5] if either operand is approximate, the result is approximate.
        _ => match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => approximate(op, x, y),
            // The binder refuses this; a plug-in function's value may not.
            _ => fail(Code::Data, "[X3.135-1986 §5.9 SR3] character string values cannot be used with arithmetic operators"),
        },
    }
}

/// [X3.135-1986 §5.9 SR5] an operator applied to approximate operands.
fn approximate(op: ArithOp, x: f64, y: f64) -> Result<Value> {
    if op == ArithOp::Div && y == 0.0_f64 {
        return fail_with(Code::Data, SqlState::DIVISION_BY_ZERO, "[X3.135-1986 §5.9 GR5] a divisor shall not be 0");
    }
    let r = match op {
        ArithOp::Add => x + y,
        ArithOp::Sub => x - y,
        ArithOp::Mul => x * y,
        ArithOp::Div => x / y,
    };
    if r.is_finite() {
        Ok(Value::Approx(r))
    } else {
        fail_with(
            Code::Data,
            SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
            "[X3.135-1986 §5.9 SR5] the approximate result is outside the range of this implementation's approximate numbers",
        )
    }
}

//! Data types and values: [X3.135-1986 §4.2, §5.5, §5.9] (arithmetic), [X3.135-1986 §5.11] (comparison).

use std::cmp::Ordering;
use std::fmt;

use crate::error::{Code, Result, SqlState, fail, fail_with};

mod cast;
pub(crate) mod codec;
mod comparison;
mod decimal;
mod like;

pub use cast::cast;
pub use comparison::{Bucket, bucket, compare, compare_strings};
pub use decimal::{Decimal, pow10};
pub use like::like;

// ---------------------------------------------------------------------------
// Implementor-defined choices. The standard leaves each of these to us.
// ---------------------------------------------------------------------------

/// Largest exact numeric precision (decimal digits) this implementation supports.
pub const MAX_EXACT_PRECISION: u32 = 18;
/// [X3.135-1986 §5.5 SR8] INTEGER has implementor-defined precision and scale 0.
pub const INTEGER_PRECISION: u32 = 10;
/// [X3.135-1986 §5.5 SR9] SMALLINT precision is not larger than INTEGER's.
pub const SMALLINT_PRECISION: u32 = 5;
/// [X3.135-1986 §5.5 SR3] precision of NUMERIC / DECIMAL when omitted.
pub const DEFAULT_EXACT_PRECISION: u32 = 18;
/// [X3.135-1986 §5.5 SR11] REAL precision, in binary digits (IEEE single).
pub const REAL_PRECISION: u32 = 24;
/// [X3.135-1986 §5.5 SR12] DOUBLE PRECISION, in binary digits (IEEE double).
pub const DOUBLE_PRECISION: u32 = 53;
/// [X3.135-1986 §8.3 GR3b vii] "The order of the null value relative to nonnull values is
/// implementor-defined, but shall be either greater than or less than all
/// nonnull values." We choose greater.
pub const NULLS_SORT_HIGH: bool = true;

/// [X3.135-1986 §5.6 SR2] "The specific `<exact numeric type>` of indicator
/// parameters is implementor-defined": exact numeric of this precision, scale 0.
pub const INDICATOR_PRECISION: u32 = 9;

// ---------------------------------------------------------------------------
// Limits the standard does not set. A type beyond one is valid SQL-86 that this
// implementation does not process: SQLCODE -900 (RULES.md D18).
// ---------------------------------------------------------------------------

/// \[DESIGN\] Longest `CHARACTER(<length>)`. [X3.135-1986 §5.5 SR2] only requires a
/// length greater than 0; values are stored padded to their length
/// [X3.135-1986 §8.7 GR7b], so this bounds one value to 32 KiB.
pub const MAX_CHARACTER_LENGTH: u32 = 32_767;

// ---------------------------------------------------------------------------
// [X3.135-1986 §5.5] <data type>
// ---------------------------------------------------------------------------

/// A `<data type>` [X3.135-1986 §5.5].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DataType {
    /// CHARACTER(length)
    Character {
        /// The length in characters.
        length: u32,
    },
    /// NUMERIC, DECIMAL, INTEGER, SMALLINT.
    Exact {
        /// Decimal digits in all.
        precision: u32,
        /// Decimal digits after the point.
        scale: u32,
    },
    /// FLOAT, REAL, DOUBLE PRECISION.
    Approximate {
        /// Binary digits of the mantissa.
        precision: u32,
    },
}

impl DataType {
    /// Whether this is a character string type.
    pub fn is_character(&self) -> bool {
        matches!(self, DataType::Character { .. })
    }
    /// [X3.135-1986 §4.2] "A character string and a number are not comparable values."
    pub fn comparable(&self, other: &DataType) -> bool {
        self.is_character() == other.is_character()
    }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataType::Character { length } => write!(f, "CHARACTER({length})"),
            DataType::Exact { precision, scale } => write!(f, "NUMERIC({precision},{scale})"),
            DataType::Approximate { precision } => write!(f, "FLOAT({precision})"),
        }
    }
}

// ---------------------------------------------------------------------------
// Operators shared by the AST, planner and executor
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Values [X3.135-1986 §4.2] null, character string, or number
// ---------------------------------------------------------------------------

/// A value [X3.135-1986 §4.2].
#[derive(Debug, Clone)]
pub enum Value {
    /// The null value [X3.135-1986 §4.2].
    Null,
    /// A character string.
    Char(String),
    /// An exact number.
    Exact(Decimal),
    /// An approximate number.
    Approx(f64),
}

impl Value {
    /// Whether this is the null value.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// A number as its nearest `f64`; `None` for null or a string.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Exact(d) => Some(d.to_f64()),
            Value::Approx(f) => Some(*f),
            Value::Null | Value::Char(_) => None,
        }
    }

    /// [X3.135-1986 §5.9 GR4] monadic minus reverses the sign.
    #[must_use]
    pub fn negate(&self) -> Value {
        match self {
            Value::Exact(d) => Value::Exact(d.negated()),
            Value::Approx(f) => Value::Approx(-f),
            Value::Null | Value::Char(_) => self.clone(),
        }
    }

    /// How this value is written as a `<literal>` ([X3.135-1986 §4.2] "the logical
    /// representation of a value is a `<literal>`").
    pub fn to_literal(&self) -> String {
        match self {
            Value::Null => "NULL".into(),
            Value::Char(s) => format!("'{}'", s.replace('\'', "''")),
            Value::Exact(_) | Value::Approx(_) => self.to_string(),
        }
    }

    /// A hashable form under which two values are equal exactly when they are
    /// "identical" per [X3.135-1986 §5.11] (trailing spaces ignored, 1.50 = 1.5, NULL = NULL
    /// as needed for DISTINCT by [X3.135-1986 §5.11 GR7]).
    pub fn norm(&self) -> NormKey {
        match self {
            Value::Null => NormKey::Null,
            Value::Char(s) => NormKey::Char(s.trim_end_matches(' ').to_string()),
            Value::Exact(d) => {
                let (u, s) = d.normalized();
                NormKey::Exact(u, s)
            }
            Value::Approx(f) => NormKey::Approx(if *f == 0.0 { 0f64.to_bits() } else { f.to_bits() }),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => write!(f, "NULL"),
            Value::Char(s) => write!(f, "{s}"),
            Value::Exact(d) => write!(f, "{d}"),
            // Written as an <approximate numeric literal>: <mantissa>E<exponent>.
            Value::Approx(x) => write!(f, "{x:E}"),
        }
    }
}

/// The form of a value that `Value::norm` gives: equal exactly when the
/// values are identical.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NormKey {
    /// The null value.
    Null,
    /// A string without trailing spaces.
    Char(String),
    /// Units and scale with no trailing fractional zeros.
    Exact(i128, u32),
    /// The bits of an `f64`, with -0 as +0.
    Approx(u64),
}

/// [X3.135-1986 §5.9] apply a dyadic operator.
///
/// # Errors
///
/// SQLCODE -301 for a zero divisor [X3.135-1986 §5.9 GR5], a result out of
/// range [X3.135-1986 §5.9 GR6], or a character string operand.
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

/// [ISO/IEC 9075:1992 §9.2 GR2, GR3] Store assignment of `v` to a column,
/// parameter or target of type `target`, as the rule `rule` (a citation,
/// e.g. "ISO/IEC 9075:1992 §13.8 GR4b iii") applies it: null stays null; a
/// character string is padded with spaces to the length, or loses
/// trailing spaces beyond it \[GR3b, GR3c\]; a number is approximated in
/// the type \[GR3k\], an exact one's extra fraction digits truncated and an
/// approximate one rounded, which is implementation-defined. Retrieval
/// assignment [§9.1 GR3] is the same but for strings, which its callers
/// truncate first.
///
/// # Errors
///
/// SQLSTATE 22001 if a string's characters beyond the length are not all
/// spaces; 22003 if a number has no approximation in the type; 22005 if
/// the value is of a type that cannot be assigned to it \[SR2\], as a
/// plug-in function might give.
pub fn assign(v: Value, target: &DataType, rule: &str) -> Result<Value> {
    match (v, target) {
        (Value::Null, _) => Ok(Value::Null),
        (Value::Char(s), DataType::Character { length }) => {
            let (n, l) = (s.chars().count(), usize::try_from(*length).unwrap_or(usize::MAX));
            if n <= l {
                return Ok(Value::Char(format!("{s}{}", " ".repeat(l.saturating_sub(n)))));
            }
            if s.chars().skip(l).all(|c| c == ' ') {
                return Ok(Value::Char(s.chars().take(l).collect()));
            }
            fail_with(
                Code::Data,
                SqlState::STRING_DATA_RIGHT_TRUNCATION,
                format!(
                    "[{rule}; ISO/IEC 9075:1992 §9.2 GR3b ii] a value of {n} characters, not spaces beyond {length}, does not fit {target}"
                ),
            )
        }
        (Value::Exact(d), DataType::Exact { precision, scale }) => {
            let units = if d.scale > *scale {
                d.scale.checked_sub(*scale).and_then(pow10).and_then(|p| d.units.checked_div(p))
            } else {
                scale.checked_sub(d.scale).and_then(pow10).and_then(|p| d.units.checked_mul(p))
            };
            // The divisor 10^n is at least 1, and the quotient smaller than `units`.
            match units.map(|u| Decimal::new(u, *scale)).filter(|out| out.fits(*precision, *scale)) {
                Some(out) => Ok(Value::Exact(out)),
                None => fail_with(
                    Code::Data,
                    SqlState::NUMERIC_VALUE_OUT_OF_RANGE,
                    format!("[{rule}; ISO/IEC 9075:1992 §9.2 GR3k] {d} does not fit {target} without losing leading digits"),
                ),
            }
        }
        (Value::Approx(x), DataType::Exact { precision, scale }) => cast::approximate_to_exact(x, (*precision, *scale), rule),
        (v, t) => match (v.as_f64(), t) {
            (Some(x), DataType::Approximate { precision }) => {
                Ok(Value::Approx(if *precision <= REAL_PRECISION { round_to_single(x) } else { x }))
            }
            (None, _) | (Some(_), DataType::Character { .. } | DataType::Exact { .. }) => fail_with(
                Code::Data,
                SqlState::ERROR_IN_ASSIGNMENT,
                format!("[{rule}; ISO/IEC 9075:1992 §9.2 SR2] the value {v} cannot be assigned to {t}"),
            ),
        },
    }
}

/// `x` rounded to the nearest IEEE single-precision value, the precision of
/// REAL ([X3.135-1986 §8.7 GR7d] "the approximate value of v").
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "RULES.md R-DEV-3: rounding to single precision is the intent, and std has no f64-to-f32 conversion function"
)]
fn round_to_single(x: f64) -> f64 {
    f64::from(x as f32)
}

/// [X3.135-1986 §8.3 GR3b] The ORDER BY ordering of two values (ascending),
/// nulls placed by `NULLS_SORT_HIGH` [X3.135-1986 §8.3 GR3b vii].
pub fn sort_cmp(a: &Value, b: &Value) -> Ordering {
    match (a.is_null(), b.is_null()) {
        (true, true) => Ordering::Equal,
        (true, false) => {
            if NULLS_SORT_HIGH {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        }
        (false, true) => {
            if NULLS_SORT_HIGH {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }
        (false, false) => compare(a, b).unwrap_or(Ordering::Equal),
    }
}

// Unit tests: private decision logic of `Decimal`, one test per decision, with
// MC/DC cases written out (RULES.md R-TEST-1, R-COV-3).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [X3.135-1986 §5.9 GR5] an approximate zero divisor is a data exception.
    ///
    /// Decision `op == Div && y == 0.0`. MC/DC: (T T) error; (F T) adding zero is
    /// fine; (T F) dividing by one is fine.
    #[test]
    fn approximate_division_by_zero() {
        let one = Value::Approx(1.0);
        let zero = Value::Approx(0.0);
        assert_eq!(arith(ArithOp::Div, &one, &zero).map_err(|e| e.code).err(), Some(Code::Data)); // T T
        assert!(arith(ArithOp::Add, &one, &zero).is_ok()); // F T
        assert!(arith(ArithOp::Div, &one, &one).is_ok()); // T F
    }

    /// Verifies: [X3.135-1986 §5.9 SR3, §8.7 SR6] [ISO/IEC 9075:1992 §9.2 SR2] a value of the wrong type,
    /// as a plug-in function might return, is a data exception in arithmetic
    /// and in assignment, not a panic.
    #[test]
    fn mistyped_values_are_reported() {
        let text = Value::Char("A".into());
        let one = Value::Exact(Decimal::new(1, 0));
        assert_eq!(arith(ArithOp::Add, &text, &one).unwrap_err().code, Code::Data);
        let approximate = DataType::Approximate { precision: REAL_PRECISION };
        assert_eq!(assign(text.clone(), &approximate, "test").unwrap_err().code, Code::Data);
        let exact = DataType::Exact { precision: 5, scale: 0 };
        assert_eq!(assign(text, &exact, "test").unwrap_err().code, Code::Data);
        let tiny_scale = DataType::Exact { precision: 5, scale: 0 };
        assert_eq!(assign(Value::Exact(Decimal::new(1, u32::MAX)), &tiny_scale, "test").unwrap_err().code, Code::Data);
    }
}

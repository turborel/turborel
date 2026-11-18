//! The statistical set functions of Feature T621 [ISO/IEC 9075-2:2023
//! §10.9]: of one `<value expression>`, STDDEV_POP, STDDEV_SAMP, VAR_POP
//! and VAR_SAMP; and the `<binary set function>`s COVAR_POP, COVAR_SAMP,
//! CORR, REGR_SLOPE, REGR_INTERCEPT, REGR_COUNT, REGR_R2, REGR_AVGX,
//! REGR_AVGY, REGR_SXX, REGR_SYY and REGR_SXY, of a `<dependent variable
//! expression>` Y and an `<independent variable expression>` X. Their
//! names are no key words of SQL-92: the parser reads each before "(" as
//! the set function (`parser/numeric.rs`), and each is registered.
//!
//! A binary set function's argument is the pair (Y, X), made by `Pair`,
//! which is null if either is: so the engine eliminates the pair, and
//! warns of it, as it does a null argument \[GR8a\].
//!
//! \[DESIGN\] Of numbers not DECFLOAT the result is DOUBLE PRECISION
//! (IV173, IV176), found by Welford's updates of the means and of the sums
//! of squared deviations and their products, which lose no digits to the
//! cancellation of SXS − SX × SX / N, and merged by Chan's (`moments.rs`).
//! Of a DECFLOAT operand the result is DECFLOAT(34) (IV172, IV175), found
//! by the General Rules' formulas in DECFLOAT arithmetic (`decimal.rs`).

use std::sync::Arc;

use super::{Accumulator, AggregateFunction, ScalarFunction};
use crate::error::{Code, Result, fail};
use crate::value::types::Field;
use crate::value::{DOUBLE_PRECISION, DataType, MAX_DECFLOAT_PRECISION, MAX_EXACT_PRECISION, Spelled, Value};

mod decimal;
mod moments;

/// [ISO/IEC 9075-2:2023 §10.9] STDDEV_POP, no key word of SQL-92.
pub const STDDEV_POP: &str = "STDDEV_POP";
/// [§10.9] STDDEV_SAMP.
pub const STDDEV_SAMP: &str = "STDDEV_SAMP";
/// [§10.9] VAR_POP.
pub const VAR_POP: &str = "VAR_POP";
/// [§10.9] VAR_SAMP.
pub const VAR_SAMP: &str = "VAR_SAMP";
/// [§10.9] COVAR_POP.
pub const COVAR_POP: &str = "COVAR_POP";
/// [§10.9] COVAR_SAMP.
pub const COVAR_SAMP: &str = "COVAR_SAMP";
/// [§10.9] CORR.
pub const CORR: &str = "CORR";
/// [§10.9] REGR_SLOPE.
pub const REGR_SLOPE: &str = "REGR_SLOPE";
/// [§10.9] REGR_INTERCEPT.
pub const REGR_INTERCEPT: &str = "REGR_INTERCEPT";
/// [§10.9] REGR_COUNT.
pub const REGR_COUNT: &str = "REGR_COUNT";
/// [§10.9] REGR_R2.
pub const REGR_R2: &str = "REGR_R2";
/// [§10.9] REGR_AVGX.
pub const REGR_AVGX: &str = "REGR_AVGX";
/// [§10.9] REGR_AVGY.
pub const REGR_AVGY: &str = "REGR_AVGY";
/// [§10.9] REGR_SXX.
pub const REGR_SXX: &str = "REGR_SXX";
/// [§10.9] REGR_SYY.
pub const REGR_SYY: &str = "REGR_SYY";
/// [§10.9] REGR_SXY.
pub const REGR_SXY: &str = "REGR_SXY";

/// The name the pair of a binary set function's argument shows in its
/// call's label; no call can name it.
const PAIR: &str = "PAIR";
/// The names of the pair's fields.
const DEPENDENT: &str = "Y";
const INDEPENDENT: &str = "X";

/// The result of each but REGR_COUNT of numbers not DECFLOAT (IV173,
/// IV176): FLOAT(53), as SUM's approximate result.
const DOUBLE: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::Float };
/// The result of a DECFLOAT operand (IV172, IV175): of 34 digits, not less
/// than any operand's.
const DECFLOAT: DataType = DataType::DecFloat { precision: MAX_DECFLOAT_PRECISION };
/// [§10.9 SR9c i] REGR_COUNT's result (IV174), COUNT's.
const COUNTED: DataType = DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric };

/// Which of the set functions it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Statistic {
    /// VAR_POP.
    VarPop,
    /// VAR_SAMP.
    VarSamp,
    /// STDDEV_POP.
    StddevPop,
    /// STDDEV_SAMP.
    StddevSamp,
    /// COVAR_POP.
    CovarPop,
    /// COVAR_SAMP.
    CovarSamp,
    /// CORR.
    Corr,
    /// REGR_SLOPE.
    Slope,
    /// REGR_INTERCEPT.
    Intercept,
    /// REGR_COUNT.
    Count,
    /// REGR_R2.
    R2,
    /// REGR_AVGX.
    AvgX,
    /// REGR_AVGY.
    AvgY,
    /// REGR_SXX.
    Sxx,
    /// REGR_SYY.
    Syy,
    /// REGR_SXY.
    Sxy,
}

impl Statistic {
    /// Each: the four of one argument, then the binary set functions in
    /// the order of `<binary set function type>`.
    pub const EVERY: [Statistic; 16] = [
        Statistic::StddevPop,
        Statistic::StddevSamp,
        Statistic::VarSamp,
        Statistic::VarPop,
        Statistic::CovarPop,
        Statistic::CovarSamp,
        Statistic::Corr,
        Statistic::Slope,
        Statistic::Intercept,
        Statistic::Count,
        Statistic::R2,
        Statistic::AvgX,
        Statistic::AvgY,
        Statistic::Sxx,
        Statistic::Syy,
        Statistic::Sxy,
    ];

    /// The set function named `name`, if one is.
    pub fn named(name: &str) -> Option<Statistic> {
        Statistic::EVERY.into_iter().find(|s| s.name() == name)
    }

    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Statistic::VarPop => VAR_POP,
            Statistic::VarSamp => VAR_SAMP,
            Statistic::StddevPop => STDDEV_POP,
            Statistic::StddevSamp => STDDEV_SAMP,
            Statistic::CovarPop => COVAR_POP,
            Statistic::CovarSamp => COVAR_SAMP,
            Statistic::Corr => CORR,
            Statistic::Slope => REGR_SLOPE,
            Statistic::Intercept => REGR_INTERCEPT,
            Statistic::Count => REGR_COUNT,
            Statistic::R2 => REGR_R2,
            Statistic::AvgX => REGR_AVGX,
            Statistic::AvgY => REGR_AVGY,
            Statistic::Sxx => REGR_SXX,
            Statistic::Syy => REGR_SYY,
            Statistic::Sxy => REGR_SXY,
        }
    }

    /// Whether it is a `<binary set function>`, of Y and X.
    pub fn binary(self) -> bool {
        !matches!(self, Statistic::VarPop | Statistic::VarSamp | Statistic::StddevPop | Statistic::StddevSamp)
    }

    /// Whether its result is null, not an exception, where the
    /// approximate mathematical result is out of range [§10.9 GR8c x 3,
    /// xi 3, xii 2, xiii 2]: CORR, REGR_R2, REGR_SLOPE, REGR_INTERCEPT.
    fn nulled_out_of_range(self) -> bool {
        matches!(self, Statistic::Corr | Statistic::R2 | Statistic::Slope | Statistic::Intercept)
    }
}

/// The set functions, to be registered.
pub(super) fn functions() -> Vec<Arc<dyn AggregateFunction>> {
    Statistic::EVERY.into_iter().map(|s| -> Arc<dyn AggregateFunction> { Arc::new(s) }).collect()
}

impl AggregateFunction for Statistic {
    fn name(&self) -> &str {
        Statistic::name(*self)
    }

    /// [§10.9 SR7h, SR9c] Of VAR_POP, VAR_SAMP, STDDEV_POP and STDDEV_SAMP
    /// the argument a number; the result DECFLOAT(34) of a DECFLOAT, else
    /// DOUBLE PRECISION. Of a binary set function, REGR_COUNT's an exact
    /// number of scale 0; the others' DECFLOAT(34) if Y or X is DECFLOAT,
    /// else DOUBLE PRECISION.
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        let operands = match (self.binary(), arg) {
            (true, Some(DataType::Row(row))) => row.fields.iter().map(|f| f.data_type.clone()).collect::<Vec<_>>(),
            (false, Some(t)) => {
                if !t.is_numeric() {
                    return fail(
                        Code::SyntaxRule,
                        format!("[ISO/IEC 9075-2:2023 §10.9 SR7h] the argument of {} shall be a number, not {t}", self.name()),
                    );
                }
                vec![t]
            }
            (true | false, _) => return fail(Code::Api, format!("[DESIGN] {} is called with its argument", self.name())),
        };
        if *self == Statistic::Count {
            return Ok(COUNTED);
        }
        Ok(if operands.iter().any(|t| matches!(t, DataType::DecFloat { .. })) { DECFLOAT } else { DOUBLE })
    }

    fn accumulator(&self, result: DataType) -> Box<dyn Accumulator> {
        if matches!(result, DataType::DecFloat { .. }) {
            Box::new(decimal::Sums::new(*self))
        } else {
            Box::new(moments::Moments::new(*self))
        }
    }
}

/// The pair (Y, X) a binary set function is called with, the binder's
/// own; none if `name` is not its name.
pub fn paired(name: &str) -> Option<Arc<dyn ScalarFunction>> {
    (name == PAIR).then(|| -> Arc<dyn ScalarFunction> { Arc::new(Pair) })
}

/// The name of the pair, which the parser gives as the argument of a
/// binary set function.
pub const PAIRED: &str = PAIR;

/// [§10.9 GR8a] The pair (Y, X) of a row: null if either is, so it is
/// eliminated; else the row of the two.
struct Pair;

impl ScalarFunction for Pair {
    fn name(&self) -> &str {
        PAIR
    }

    /// [§10.9] `<dependent variable expression>` and `<independent
    /// variable expression>` are each a `<numeric value expression>`.
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let [y, x] = args else { return fail(Code::Api, "[DESIGN] a binary set function takes two operands") };
        for (what, t) in [("dependent", y), ("independent", x)] {
            if !t.is_numeric() {
                return fail(
                    Code::SyntaxRule,
                    format!("[ISO/IEC 9075-2:2023 §10.9] the <{what} variable expression> is a <numeric value expression>, not {t}"),
                );
            }
        }
        let field = |name: &str, t: &DataType| Field { name: name.to_owned(), data_type: t.clone(), unnamed: true };
        Ok(DataType::row(vec![field(DEPENDENT, y), field(INDEPENDENT, x)]))
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        if args.iter().any(Value::is_null) { Ok(Value::Null) } else { Ok(Value::Row(args.into())) }
    }
}

/// The operands a set function `kind` is given in `v`: X alone, or of a
/// binary set function the pair (Y, X).
fn operands(kind: Statistic, v: &Value) -> Result<(&Value, &Value)> {
    if !kind.binary() {
        return Ok((v, v));
    }
    if let Value::Row(fields) = v
        && let [y, x] = fields.as_ref()
    {
        return Ok((y, x));
    }
    fail(Code::Api, "[DESIGN] a binary set function is given a pair")
}

// Unit tests: each General Rule against values worked by hand, nulls,
// one row and none, states merged (RULES.md R-TEST-2).
#[cfg(test)]
mod tests;

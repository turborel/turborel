//! Abstract syntax of `<set function specification>` [X3.135-1986 §5.8;
//! ISO/IEC 9075-2:2023 §10.9] and its argument.

use super::{ValueExpression, ValueOrder};

/// [X3.135-1986 §5.8] `<set function specification>` ::=
///     COUNT(*) | `<distinct set function>` | `<all set function>`
#[derive(Debug, Clone)]
pub struct SetFunctionSpecification {
    /// COUNT, AVG, MAX, MIN or SUM; \[ISO/IEC 9075-2:2023 §10.9\] EVERY,
    /// ANY or SOME
    pub name: &'static str,
    /// What the function is applied to.
    pub arg: SetArg,
    /// [ISO/IEC 9075-2:2023 §10.9] The `<sort specification list>` of an
    /// ARRAY_AGG, if written, or of a LISTAGG's `<within group
    /// specification>`; of every other function, none.
    pub order_by: Vec<ValueOrder>,
    /// [ISO/IEC 9075-2:2023 §10.9] A `<listagg set function>`'s separator
    /// and overflow behavior; of every other function, none.
    pub listagg: Option<Box<Listagg>>,
    /// Where it begins in the statement: a character offset, for the
    /// positions of errors (`SqlError::or_at`).
    pub at: usize,
}

/// The argument of a `<set function specification>`.
#[derive(Debug, Clone)]
pub enum SetArg {
    /// COUNT(*)
    CountStar,
    /// `<general set function>` with DISTINCT [ISO/IEC 9075:1992 §6.5]:
    /// SQL-86's `<distinct set function>` of a `<column specification>`
    /// among them.
    Distinct(Box<ValueExpression>),
    /// `<general set function>` with ALL, or neither [ISO/IEC 9075:1992
    /// §6.5 SR1]: SQL-86's `<all set function>` among them.
    All(Box<ValueExpression>),
    /// [ISO/IEC 9075-2:2023 §6.9] The `<column reference>`s of a `<grouping
    /// operation>`, each a `ValueExpression::Column`.
    Grouping(Vec<ValueExpression>),
}

/// [ISO/IEC 9075-2:2023 §10.9] What a `<listagg set function>` writes
/// besides its argument and `<within group specification>`.
#[derive(Debug, Clone)]
pub struct Listagg {
    /// The `<listagg separator>`, a `<character string literal>`'s value.
    pub separator: String,
    /// The `<overflow behavior>`, ERROR if none is written \[SR11f\].
    pub overflow: Overflow,
}

/// [ISO/IEC 9075-2:2023 §10.9] `<overflow behavior>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overflow {
    /// ON OVERFLOW ERROR, written or implicit.
    Error,
    /// ON OVERFLOW TRUNCATE.
    Truncate {
        /// The `<listagg truncation filler>`, `...` if none is written
        /// \[SR11g\].
        filler: String,
        /// WITH COUNT rather than WITHOUT COUNT.
        count: bool,
    },
}

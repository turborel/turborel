//! Abstract syntax of periods [ISO/IEC 9075-2:2023 §4.16, §11.3, §11.27,
//! §11.28]: a `<table period definition>` in a table's elements or added
//! by ALTER TABLE, and DROP PERIOD; and the `<period predicate>`s that
//! compare periods [§8.20].

/// [ISO/IEC 9075-2:2023 §11.3] The name of the system-time period.
pub const SYSTEM_TIME: &str = "SYSTEM_TIME";

/// [ISO/IEC 9075-2:2023 §11.3] `<system or application time period
/// specification>`: the name of a period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeriodName {
    /// `<system time period specification>` ::= PERIOD FOR SYSTEM_TIME.
    SystemTime,
    /// `<application time period specification>` ::= PERIOD FOR
    /// `<application time period name>`.
    Application(String),
}

impl PeriodName {
    /// The name as the period descriptor holds it: SYSTEM_TIME, or the
    /// application-time period's [§4.16.1].
    pub fn name(&self) -> &str {
        match self {
            PeriodName::SystemTime => SYSTEM_TIME,
            PeriodName::Application(n) => n,
        }
    }
}

/// [ISO/IEC 9075-2:2023 §11.3] `<table period definition>` ::= `<system or
/// application time period specification>` ( `<period begin column name>`
/// , `<period end column name>` ).
#[derive(Debug, Clone)]
pub struct PeriodDefinition {
    /// The period's name.
    pub name: PeriodName,
    /// The `<period begin column name>`, and where it is written.
    pub start: (String, usize),
    /// The `<period end column name>`, and where it is written.
    pub end: (String, usize),
    /// Where PERIOD begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §8.20] Which `<period predicate>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodOperator {
    /// `<period overlaps predicate>`.
    Overlaps,
    /// `<period equals predicate>`.
    Equals,
    /// `<period contains predicate>`.
    Contains,
    /// `<period precedes predicate>`.
    Precedes,
    /// `<period succeeds predicate>`.
    Succeeds,
    /// `<period immediately precedes predicate>`.
    ImmediatelyPrecedes,
    /// `<period immediately succeeds predicate>`.
    ImmediatelySucceeds,
}

/// [ISO/IEC 9075-2:2023 §8.20] An operand of a `<period predicate>`: a
/// `<period predicand>` PERIOD ( start , end ); or a value, which the
/// binder finds a `<period reference>`, a `<basic identifier chain>`
/// naming a period, or, after CONTAINS, a point in time.
#[derive(Debug, Clone)]
pub enum PeriodOperand {
    /// PERIOD ( `<period start value>` , `<period end value>` ).
    Written(Box<super::ValueExpression>, Box<super::ValueExpression>),
    /// A `<period reference>`, or a `<datetime value expression>`.
    Value(Box<super::ValueExpression>),
}

/// [ISO/IEC 9075-2:2023 §14.9, §14.14] FOR PORTION OF `<application time
/// period name>` FROM `<point in time 1>` TO `<point in time 2>`.
#[derive(Debug, Clone)]
pub struct Portion {
    /// The period's name.
    pub period: String,
    /// `<point in time 1>`, FROMVAL \[SR7a ii\].
    pub from: super::ValueExpression,
    /// `<point in time 2>`, TOVAL \[SR7a iii\].
    pub to: super::ValueExpression,
    /// Where it begins in the statement.
    pub at: usize,
}

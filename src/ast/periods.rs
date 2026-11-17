//! Abstract syntax of periods [ISO/IEC 9075-2:2023 §4.16, §11.3, §11.27,
//! §11.28]: a `<table period definition>` in a table's elements or added
//! by ALTER TABLE, and DROP PERIOD.

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
            PeriodName::SystemTime => "SYSTEM_TIME",
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

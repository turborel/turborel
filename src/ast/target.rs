//! Abstract syntax of a `<parameter specification>` [X3.135-1986 §5.6]
//! and of the `<target specification>`s it stands for [ISO/IEC
//! 9075-2:2023 §6.4]: a host parameter, an SQL parameter, a new
//! transition variable's column, or an element of the array either of the
//! last two holds (Feature S097).

use super::Literal;

/// [X3.135-1986 §5.6] `<parameter specification>` ::= `<parameter name>` \[`<indicator parameter>`\]
/// `<indicator parameter>` ::= \[INDICATOR\] `<parameter name>`
///
/// Also the only implemented form of `<target specification>`, and of
/// `<target array element specification>` [ISO/IEC 9075-2:2023 §6.4]
/// ::= `<target array reference>` `[` `<simple value specification>` `]`,
/// its `element` given.
#[derive(Debug, Clone)]
pub struct ParameterSpecification {
    /// The `<parameter name>`.
    pub name: String,
    /// The `<indicator parameter>`'s name, if written.
    pub indicator: Option<String>,
    /// What it names: a parameter, a column of a new transition variable,
    /// or an SQL parameter.
    pub kind: TargetKind,
    /// Of a `<target array element specification>`, the position of the
    /// element of the array `name` holds that it assigns.
    pub element: Option<Box<ElementPosition>>,
    /// Where it begins in the statement: a character offset, for the
    /// positions of errors (`SqlError::or_at`).
    pub at: usize,
}

impl ParameterSpecification {
    /// The target `name` of kind `kind`, written at `at`, of no indicator
    /// and no element.
    pub fn of(name: String, kind: TargetKind, at: usize) -> ParameterSpecification {
        ParameterSpecification { name, indicator: None, kind, element: None, at }
    }
}

/// [ISO/IEC 9075-2:2023 §6.4] What a `<target specification>` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// A `<host parameter specification>`: `name` is a parameter of the
    /// procedure the statement is of [X3.135-1986 §5.6].
    Parameter,
    /// [§6.4 SR7, §6.6 SR13] A new transition variable column reference of
    /// a BEFORE trigger: `name` is the column of the new row it assigns.
    Transition,
    /// [§6.8 SR1, §6.6 SR8] An `<SQL parameter reference>` in the body of
    /// an SQL-invoked procedure: `name` is the SQL parameter it assigns.
    Routine,
}

/// [ISO/IEC 9075-2:2023 §6.4] The `<simple value specification>` of a
/// `<target array element specification>`: a `<literal>`, or an `<SQL
/// parameter reference>` of the procedure whose body it is in. Its other
/// forms, a `<host parameter name>` and an `<embedded variable name>`, are
/// never where an array may be a target, an SQL parameter's or a new row
/// column's.
#[derive(Debug, Clone)]
pub enum ElementPosition {
    /// A `<literal>`.
    Literal(Literal),
    /// The SQL parameter of this name.
    Parameter(String),
}

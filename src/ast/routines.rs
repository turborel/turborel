//! Abstract syntax of SQL-invoked routines [ISO/IEC 9075-2:2023 §11.60,
//! §11.62, §10.6, §16.1, §16.2] (Feature T321): CREATE FUNCTION and
//! CREATE PROCEDURE of an SQL routine body, the `<specific routine
//! designator>` DROP and GRANT name one by, CALL and RETURN.

use super::{SqlStatement, TableName, TypeName, ValueExpression};

/// [ISO/IEC 9075-2:2023 §11.60] Whether an `<SQL-invoked routine>` is a
/// `<schema procedure>` or a `<schema function>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutineKind {
    /// CREATE FUNCTION: an SQL-invoked function, invoked by a `<routine
    /// invocation>` that is a value expression [§4.35.1].
    Function,
    /// CREATE PROCEDURE: an SQL-invoked procedure, invoked by CALL.
    Procedure,
}

/// [ISO/IEC 9075-2:2023 §10.6] `<routine type>` ::= ROUTINE | FUNCTION |
/// PROCEDURE: which routines a `<specific routine designator>` may name
/// \[SR4\].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutineType {
    /// ROUTINE: a function or a procedure.
    Routine,
    /// FUNCTION: a regular function.
    Function,
    /// PROCEDURE: a procedure.
    Procedure,
}

/// [ISO/IEC 9075-2:2023 §10.6] `<specific routine designator>` ::=
/// SPECIFIC `<routine type>` `<specific name>` | `<routine type>`
/// `<member name>`, its `<data type list>` if written.
#[derive(Debug, Clone)]
pub struct RoutineDesignator {
    /// Which routines it may name.
    pub routine_type: RoutineType,
    /// SPECIFIC was written: `name` is a `<specific name>`.
    pub specific: bool,
    /// The `<specific name>` or the `<schema qualified routine name>`.
    pub name: TableName,
    /// The `<data type list>`, if written.
    pub parameters: Option<Vec<TypeName>>,
}

/// [ISO/IEC 9075-2:2023 §11.60] `<parameter mode>` ::= IN | OUT | INOUT
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterMode {
    /// IN: an input parameter \[SR16e i\], as one of no mode is \[SR16d\].
    In,
    /// OUT: an output parameter \[SR16e ii\].
    Out,
    /// INOUT: both \[SR16e iii\].
    InOut,
}

/// [ISO/IEC 9075-2:2023 §11.60] `<SQL parameter declaration>` ::=
/// \[`<parameter mode>`\] `<SQL parameter name>` `<parameter type>`
#[derive(Debug, Clone)]
pub struct ParameterDeclaration {
    /// The `<parameter mode>`, if written.
    pub mode: Option<ParameterMode>,
    /// The `<SQL parameter name>`, which an SQL routine names every
    /// parameter by \[SR14\].
    pub name: String,
    /// The `<parameter type>`.
    pub data_type: TypeName,
    /// Where it is written.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §11.60] `<SQL-data access indication>`, of which
/// an SQL routine has one \[SR9g\], NO SQL refused \[SR9m ii\].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataAccess {
    /// CONTAINS SQL: possibly contains SQL, neither reading nor modifying
    /// SQL-data.
    Contains,
    /// READS SQL DATA: possibly reads SQL-data.
    Reads,
    /// MODIFIES SQL DATA: possibly modifies SQL-data.
    Modifies,
}

/// [ISO/IEC 9075-2:2023 §11.60] `<SQL-invoked routine>` of an SQL routine:
/// CREATE {PROCEDURE | FUNCTION} `<schema qualified routine name>` (
/// \[`<SQL parameter declaration>` \[{, ...}...\]\] ) \[RETURNS `<data
/// type>`\] `<routine characteristics>` `<SQL procedure statement>`
#[derive(Debug)]
pub struct RoutineDefinition {
    /// FUNCTION or PROCEDURE.
    pub kind: RoutineKind,
    /// The `<schema qualified routine name>`.
    pub name: TableName,
    /// The `<SQL parameter declaration>`s, in order.
    pub parameters: Vec<ParameterDeclaration>,
    /// Of a function, the `<returns data type>`.
    pub returns: Option<TypeName>,
    /// The `<specific name>` of SPECIFIC, if written.
    pub specific: Option<TableName>,
    /// DETERMINISTIC was written; NOT DETERMINISTIC is the default
    /// \[SR9e\].
    pub deterministic: bool,
    /// The `<SQL-data access indication>`.
    pub access: DataAccess,
    /// RETURNS NULL ON NULL INPUT was written: a null-call function;
    /// CALLED ON NULL INPUT is the default \[SR9f ii\].
    pub null_call: bool,
    /// The `<SQL procedure statement>` of its `<SQL routine body>`.
    pub body: Box<SqlStatement>,
    /// \[DESIGN\] The `<SQL-invoked routine>` as written, alone even in a
    /// `<schema definition>`: what the routine descriptor keeps, to read
    /// its body again.
    pub text: String,
    /// The `<SQL routine body>` as written, which the Information Schema
    /// shows.
    pub body_text: String,
}

/// [ISO/IEC 9075-2:2023 §10.4] `<SQL argument>` of a CALL: a value
/// expression, a target specification of an OUT or INOUT parameter, as
/// the binder finds by its parameter's mode, or a `<contextually typed
/// value specification>`.
#[derive(Debug, Clone)]
pub enum SqlArgument {
    /// A `<value expression>`, or a `<target specification>` written as
    /// one.
    Value(ValueExpression),
    /// DEFAULT, where it is written.
    Default(usize),
    /// NULL, where it is written.
    Null(usize),
}

/// [ISO/IEC 9075-2:2023 §16.1] `<call statement>` ::= CALL `<routine
/// invocation>`
#[derive(Debug, Clone)]
pub struct CallStatement {
    /// The `<routine name>`.
    pub name: TableName,
    /// The `<SQL argument>`s, in order.
    pub arguments: Vec<SqlArgument>,
}

/// [ISO/IEC 9075-2:2023 §16.2] `<return statement>` ::= RETURN `<return
/// value>`, `<return value>` ::= `<value expression>` | NULL
#[derive(Debug, Clone)]
pub struct ReturnStatement {
    /// The `<value expression>`; none for NULL, a null of the function's
    /// result type \[SR2\].
    pub value: Option<ValueExpression>,
    /// Where RETURN is written.
    pub at: usize,
}

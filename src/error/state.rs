//! \[EXTENSION\] SQLSTATE values beside SQLCODE values: a class of two
//! characters and a subclass of three [ISO/IEC 9075:1992 §22.1, Table 23].
//! X3.135-1986 has only SQLCODE; SQLSTATE is its successor's preferred
//! status parameter. Split from error.rs (RULES.md R-SIZE-2).
//!
//! Statements are invoked directly (README, conformance), so a statement
//! that breaks a Format, Syntax Rule or Access Rule raises the class of
//! direct invocation, 2A [ISO/IEC 9075:1992 §20.1 GR5]. Conditions this
//! implementation defines are in class TB, as a class beginning with T is
//! implementation-defined [§22.1]: the subclass is the SQLCODE's digits.

use std::fmt;

use super::Code;

/// \[EXTENSION\] An SQLSTATE value [ISO/IEC 9075:1992 §22.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqlState(&'static str);

impl SqlState {
    /// Successful completion.
    pub const SUCCESSFUL_COMPLETION: SqlState = SqlState("00000");
    /// No data.
    pub const NO_DATA: SqlState = SqlState("02000");
    /// Warning: cursor operation conflict [ISO/IEC 9075:1992 §11.8 GR8].
    pub const CURSOR_OPERATION_CONFLICT: SqlState = SqlState("01001");
    /// Warning: privilege not revoked [ISO/IEC 9075:1992 §11.37 GR8, GR9].
    pub const PRIVILEGE_NOT_REVOKED: SqlState = SqlState("01006");
    /// Warning: privilege not granted [ISO/IEC 9075:1992 §11.36 GR14, GR15].
    pub const PRIVILEGE_NOT_GRANTED: SqlState = SqlState("01007");
    /// Warning: search condition too long for information schema [ISO/IEC
    /// 9075:1992 §11.9 GR2, §11.34 GR4].
    pub const SEARCH_CONDITION_TOO_LONG: SqlState = SqlState("01009");
    /// Warning: query expression too long for information schema [ISO/IEC
    /// 9075:1992 §11.19 GR13].
    pub const QUERY_EXPRESSION_TOO_LONG: SqlState = SqlState("0100A");
    /// Warning: string data, right truncation [ISO/IEC 9075:1992 §9.1
    /// GR3b, GR3e; §6.10 GR5c ii, GR5d, GR6c ii, GR6d, GR7b, GR8b].
    pub const STRING_TRUNCATED: SqlState = SqlState("01004");
    /// Warning: implicit zero-bit padding [ISO/IEC 9075:1992 §9.1 GR3h,
    /// §6.10 GR5d, GR6d, GR7c].
    pub const ZERO_BIT_PADDING: SqlState = SqlState("01008");
    /// Warning: null value eliminated in set function [ISO/IEC 9075:1992
    /// §6.5 GR1b].
    pub const NULL_VALUE_ELIMINATED: SqlState = SqlState("01003");
    /// Dependent privilege descriptors still exist [ISO/IEC 9075:1992
    /// §11.37 GR2b].
    pub const DEPENDENT_PRIVILEGES: SqlState = SqlState("2B000");
    /// Feature not supported.
    pub const FEATURE_NOT_SUPPORTED: SqlState = SqlState("0A000");
    /// Cardinality violation.
    pub const CARDINALITY_VIOLATION: SqlState = SqlState("21000");
    /// Connection exception [ISO/IEC 9075:1992 §22.1, Table 23].
    pub const CONNECTION_EXCEPTION: SqlState = SqlState("08000");
    /// Connection exception: SQL-server rejected establishment of
    /// SQL-connection.
    pub const CONNECTION_REJECTED: SqlState = SqlState("08004");
    /// Connection exception: SQL-client unable to establish SQL-connection.
    pub const UNABLE_TO_CONNECT: SqlState = SqlState("08001");
    /// Connection exception: connection name in use.
    pub const CONNECTION_NAME_IN_USE: SqlState = SqlState("08002");
    /// Connection exception: connection does not exist.
    pub const CONNECTION_DOES_NOT_EXIST: SqlState = SqlState("08003");
    /// Feature not supported: multiple server transactions.
    pub const MULTIPLE_SERVER_TRANSACTIONS: SqlState = SqlState("0A001");
    /// Invalid connection name.
    pub const INVALID_CONNECTION_NAME: SqlState = SqlState("2E000");
    /// Warning: disconnect error.
    pub const DISCONNECT_ERROR: SqlState = SqlState("01002");
    /// Connection exception: connection failure.
    pub const CONNECTION_FAILURE: SqlState = SqlState("08006");
    /// Invalid authorization specification.
    pub const INVALID_AUTHORIZATION: SqlState = SqlState("28000");
    /// Dynamic SQL error [ISO/IEC 9075:1992 §22.1, Table 23].
    pub const DYNAMIC_SQL_ERROR: SqlState = SqlState("07000");
    /// Dynamic SQL error: using clause does not match dynamic parameter
    /// specifications.
    pub const USING_CLAUSE_MISMATCH: SqlState = SqlState("07001");
    /// Dynamic SQL error: cursor specification cannot be executed.
    pub const CURSOR_SPECIFICATION_NOT_EXECUTED: SqlState = SqlState("07003");
    /// Dynamic SQL error: prepared statement not a cursor specification.
    pub const NOT_A_CURSOR_SPECIFICATION: SqlState = SqlState("07005");
    /// Invalid SQL statement name.
    pub const INVALID_STATEMENT_NAME: SqlState = SqlState("26000");
    /// Syntax error or access rule violation in dynamic SQL statement.
    pub const DYNAMIC_SYNTAX_ERROR: SqlState = SqlState("37000");
    /// Data exception.
    pub const DATA_EXCEPTION: SqlState = SqlState("22000");
    /// Data exception: string data, right truncation.
    pub const STRING_DATA_RIGHT_TRUNCATION: SqlState = SqlState("22001");
    /// Data exception: string data, length mismatch.
    pub const STRING_DATA_LENGTH_MISMATCH: SqlState = SqlState("22026");
    /// Data exception: null value, no indicator parameter.
    pub const NULL_VALUE_NO_INDICATOR: SqlState = SqlState("22002");
    /// Data exception: numeric value out of range.
    pub const NUMERIC_VALUE_OUT_OF_RANGE: SqlState = SqlState("22003");
    /// Data exception: error in assignment.
    pub const ERROR_IN_ASSIGNMENT: SqlState = SqlState("22005");
    /// Data exception: division by zero.
    pub const DIVISION_BY_ZERO: SqlState = SqlState("22012");
    /// Data exception: invalid escape character.
    pub const INVALID_ESCAPE_CHARACTER: SqlState = SqlState("22019");
    /// Data exception: invalid character value for cast [ISO/IEC 9075:1992
    /// §6.10 GR3b i].
    pub const INVALID_CHARACTER_VALUE_FOR_CAST: SqlState = SqlState("22018");
    /// Data exception: substring error [ISO/IEC 9075:1992 §6.7 GR1d].
    pub const SUBSTRING_ERROR: SqlState = SqlState("22011");
    /// Data exception: trim error [ISO/IEC 9075:1992 §6.7 GR5d].
    pub const TRIM_ERROR: SqlState = SqlState("22027");
    /// Data exception: indicator overflow.
    pub const INDICATOR_OVERFLOW: SqlState = SqlState("22022");
    /// Data exception: invalid escape sequence.
    pub const INVALID_ESCAPE_SEQUENCE: SqlState = SqlState("22025");
    /// Data exception: invalid datetime format.
    pub const INVALID_DATETIME_FORMAT: SqlState = SqlState("22007");
    /// Data exception: datetime field overflow.
    pub const DATETIME_FIELD_OVERFLOW: SqlState = SqlState("22008");
    /// Data exception: invalid time zone displacement value.
    pub const INVALID_TIME_ZONE_DISPLACEMENT: SqlState = SqlState("22009");
    /// Data exception: interval field overflow.
    pub const INTERVAL_FIELD_OVERFLOW: SqlState = SqlState("22015");
    /// Integrity constraint violation.
    pub const INTEGRITY_CONSTRAINT_VIOLATION: SqlState = SqlState("23000");
    /// Triggered data change violation.
    pub const TRIGGERED_DATA_CHANGE_VIOLATION: SqlState = SqlState("27000");
    /// Invalid cursor state.
    pub const INVALID_CURSOR_STATE: SqlState = SqlState("24000");
    /// Invalid transaction state.
    pub const INVALID_TRANSACTION_STATE: SqlState = SqlState("25000");
    /// Invalid condition number.
    pub const INVALID_CONDITION_NUMBER: SqlState = SqlState("35000");
    /// Invalid catalog name.
    pub const INVALID_CATALOG_NAME: SqlState = SqlState("3D000");
    /// Invalid schema name.
    pub const INVALID_SCHEMA_NAME: SqlState = SqlState("3F000");
    /// Syntax error or access rule violation in direct SQL statement.
    pub const SYNTAX_ERROR_IN_DIRECT_STATEMENT: SqlState = SqlState("2A000");
    /// Connection exception: transaction resolution unknown [ISO/IEC
    /// 9075:1992 §4.29]: whether the transaction committed cannot be
    /// verified. \[DESIGN\] Of a commit in doubt (RULES.md D25).
    pub const TRANSACTION_RESOLUTION_UNKNOWN: SqlState = SqlState("08007");
    /// Transaction rollback.
    pub const TRANSACTION_ROLLBACK: SqlState = SqlState("40000");
    /// Transaction rollback: serialization failure.
    pub const SERIALIZATION_FAILURE: SqlState = SqlState("40001");
    /// Transaction rollback: integrity constraint violation [ISO/IEC
    /// 9075:1992 §14.3 GR5a].
    pub const TRANSACTION_ROLLBACK_INTEGRITY: SqlState = SqlState("40002");
    /// With check option violation.
    pub const WITH_CHECK_OPTION_VIOLATION: SqlState = SqlState("44000");
    /// \[DESIGN\] A statement refused by a conflict with another
    /// transaction, undone while its transaction goes on, so not a
    /// transaction rollback.
    pub const STATEMENT_CONFLICT: SqlState = SqlState("TB911");
    /// \[DESIGN\] Use refused by the transaction policy.
    pub const POLICY: SqlState = SqlState("TB912");
    /// \[DESIGN\] A table damaged by a change that could not be undone.
    pub const DAMAGED: SqlState = SqlState("TB914");
    /// \[DESIGN\] A file could not be read, written or synced.
    pub const IO: SqlState = SqlState("TB915");
    /// \[DESIGN\] Stored data failed its checksum or format.
    pub const CORRUPT: SqlState = SqlState("TB916");
    /// \[DESIGN\] The host (Rust) API was misused.
    pub const API: SqlState = SqlState("TB910");
    /// \[DESIGN\] The memory budget is spent (SQLCODE -918).
    pub const RESOURCES: SqlState = SqlState("TB918");
    /// \[DESIGN\] The statement was cancelled (SQLCODE -920).
    pub const CANCELLED: SqlState = SqlState("TB920");

    /// The five characters.
    pub fn as_str(self) -> &'static str {
        self.0
    }

    /// The class: the first two characters.
    pub fn class(self) -> &'static str {
        self.0.get(..2).unwrap_or_default()
    }

    /// Whether it is of a class of syntax error or access rule violation
    /// [ISO/IEC 9075:1992 §18.1 GR3g]: 42, 2A (in a direct SQL statement)
    /// or 37 (in a dynamic SQL statement) [§22.1].
    pub fn is_syntax_or_access(self) -> bool {
        /// [§22.1] The class of syntax error or access rule violation.
        const SYNTAX_OR_ACCESS: &str = "42";
        [SYNTAX_OR_ACCESS, SqlState::SYNTAX_ERROR_IN_DIRECT_STATEMENT.class(), SqlState::DYNAMIC_SYNTAX_ERROR.class()]
            .contains(&self.class())
    }

    /// The state an error of `code` has, unless raised with a closer one:
    /// the class of its condition, without a subclass.
    pub fn of(code: Code) -> SqlState {
        match code {
            Code::Lexical | Code::Syntax | Code::SyntaxRule | Code::Privilege => SqlState::SYNTAX_ERROR_IN_DIRECT_STATEMENT,
            Code::Constraint => SqlState::INTEGRITY_CONSTRAINT_VIOLATION,
            Code::Data => SqlState::DATA_EXCEPTION,
            Code::Cursor => SqlState::INVALID_CURSOR_STATE,
            Code::Cardinality => SqlState::CARDINALITY_VIOLATION,
            Code::Conflict => SqlState::STATEMENT_CONFLICT,
            Code::Policy => SqlState::POLICY,
            Code::TransactionState => SqlState::INVALID_TRANSACTION_STATE,
            Code::Resources => SqlState::RESOURCES,
            Code::Connection => SqlState::CONNECTION_EXCEPTION,
            Code::Cancelled => SqlState::CANCELLED,
            Code::Dynamic => SqlState::DYNAMIC_SQL_ERROR,
            Code::InDoubt => SqlState::TRANSACTION_RESOLUTION_UNKNOWN,
            Code::CommitFailed => SqlState::TRANSACTION_ROLLBACK,
            Code::Damaged => SqlState::DAMAGED,
            Code::Io => SqlState::IO,
            Code::Corrupt => SqlState::CORRUPT,
            Code::Unsupported => SqlState::FEATURE_NOT_SUPPORTED,
            Code::Api => SqlState::API,
        }
    }
}

impl fmt::Display for SqlState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

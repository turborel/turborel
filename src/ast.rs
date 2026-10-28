//! Abstract syntax: the parsed form of `<schema>` [X3.135-1986 §6], of the
//! `<SQL statement>`s and `<declare cursor>` [X3.135-1986 §7.3, §8], and of the
//! common elements they are built from [X3.135-1986 §5]. Each type is named
//! after the production it represents.
//! Comments give the BNF exactly as implemented; alternatives in the
//! standard that are not implemented are listed under "Not implemented".

use crate::value::datetime::{Field, Moment, Written};
use crate::value::interval::Interval;
use crate::value::{Bits, Decimal};

/// [X3.135-1986 §5.4] `<table name>` ::= \[`<authorization identifier>`.\] `<table identifier>`
///
/// Also used for [X3.135-1986 §5.7] `<qualifier>` ::= `<table name>` | `<correlation name>`,
/// where a single identifier may be either one (resolved during binding).
#[derive(Debug, Clone)]
pub struct TableName {
    /// The `<authorization identifier>`, if written.
    pub auth: Option<String>,
    /// The `<table identifier>`, or a `<correlation name>` when this is a `<qualifier>`.
    pub ident: String,
    /// Where it begins in the statement: a character offset, for the
    /// positions of errors (`SqlError::or_at`).
    pub at: usize,
}

mod connection;
mod diagnostics;
mod expression;
mod query;
mod schema;

pub use connection::{ConnectionStatement, Disconnected};
pub use diagnostics::{ConditionItem, GetDiagnostics, StatementItem};
pub use expression::{
    CaseExpression, ColumnSpecification, Predicate, RowValue, SearchCondition, SetArg, SetFunctionSpecification, Sign, Subquery,
    TruthValue, ValueExpression,
};
pub use query::{
    Correspondence, CursorSpecification, DerivedTable, JoinKind, JoinSpecification, JoinedTable, NamedTable, QueryExpression,
    QuerySpecification, RowElement, SelectItem, SelectList, SelectSublist, SetOperation, SetOperator, SortColumn, SortSpecification,
    TableExpression, TableReference, Updatability, ValuesRow,
};

pub use schema::{
    Action, AlterDomain, AlterDomainAction, AssertionDefinition, Check, CheckLevels, ColumnConstraint, ColumnConstraintKind,
    ColumnDefinition, ConstraintAttributes, DefaultOption, DomainDefinition, MatchType, OptionValue, PrivilegeDefinition, References,
    ReferentialAction, RevokeStatement, Schema, SchemaStatement, TableConstraint, TableConstraintKind, TableDefinition, TableElement,
    TableOptionValue, TableStorage, Temporary, TemporaryKind, TypeName, ViewDefinition,
};

// ---------------------------------------------------------------------------
// [X3.135-1986 §8.7] <insert statement>
// ---------------------------------------------------------------------------

/// [X3.135-1986 §8.7; ISO/IEC 9075:1992 §13.8] `<insert statement>` ::=
///     INSERT INTO `<table name>` {\[(`<insert column list>`)\] `<query
///     expression>` | DEFAULT VALUES}
#[derive(Debug)]
pub struct InsertStatement {
    /// The `<table name>`.
    pub table: TableName,
    /// The `<insert column list>`, if written.
    pub columns: Option<Vec<ColumnName>>,
    /// The rows inserted.
    pub source: InsertSource,
}

/// A `<column name>` of a column list, and where it is written.
#[derive(Debug, Clone)]
pub struct ColumnName {
    /// The name.
    pub name: String,
    /// Where it begins in the statement: a character offset, for the
    /// positions of errors (`SqlError::or_at`).
    pub at: usize,
}

/// [X3.135-1986 §8.7] What an `<insert statement>` inserts.
#[derive(Debug)]
pub enum InsertSource {
    /// [ISO/IEC 9075:1992 §13.8] A `<query expression>`: its rows. SQL-86's
    /// VALUES (`<insert value list>`) is a `<table value constructor>` of
    /// one row.
    Query(Box<QueryExpression>),
    /// [ISO/IEC 9075:1992 §13.8 SR2] DEFAULT VALUES: one row, each column's
    /// default.
    DefaultValues,
}

// ---------------------------------------------------------------------------
// [X3.135-1986 §5.6] <value specification>, [X3.135-1986 §5.2] <literal>
// ---------------------------------------------------------------------------

/// [X3.135-1986 §5.6] `<value specification>` ::= `<parameter specification>` | `<literal>` | USER
/// | CURRENT_USER | SESSION_USER | SYSTEM_USER [ISO/IEC 9075:1992 §6.2]
///
/// Not implemented: `<variable specification>` (embedded SQL).
#[derive(Debug, Clone)]
pub enum ValueSpecification {
    /// A `<parameter specification>`.
    Parameter(ParameterSpecification),
    /// A `<literal>`.
    Literal(Literal),
    /// USER, or another `<authorization identifier>`.
    User(UserFunction),
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §6.2] A `<dynamic parameter
    /// specification>`, `?`, of a prepared statement \[SR2\]: the `index`-th
    /// of its statement, from 0, written at `at`.
    Dynamic {
        /// Its place among the statement's, in the order written.
        index: usize,
        /// Where it begins in the statement.
        at: usize,
    },
}

/// [ISO/IEC 9075:1992 §6.2] The `<authorization identifier>` a `<general
/// value specification>` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserFunction {
    /// CURRENT_USER, or USER, which is it \[SR3\]: the current one \[GR9\].
    Current,
    /// SESSION_USER: the SQL-session's \[GR10\].
    Session,
    /// SYSTEM_USER: the operating system user's \[GR11\].
    System,
}

/// [X3.135-1986 §5.6] `<parameter specification>` ::= `<parameter name>` \[`<indicator parameter>`\]
/// `<indicator parameter>` ::= \[INDICATOR\] `<parameter name>`
///
/// Also the only implemented form of `<target specification>`.
#[derive(Debug, Clone)]
pub struct ParameterSpecification {
    /// The `<parameter name>`.
    pub name: String,
    /// The `<indicator parameter>`'s name, if written.
    pub indicator: Option<String>,
    /// Where it begins in the statement: a character offset, for the
    /// positions of errors (`SqlError::or_at`).
    pub at: usize,
}

/// [X3.135-1986 §5.2] `<literal>` ::= `<character string literal>` | `<numeric literal>`
#[derive(Debug, Clone)]
pub enum Literal {
    /// A `<character string literal>`'s value.
    Character(String),
    /// An `<exact numeric literal>`.
    Exact(Decimal),
    /// An `<approximate numeric literal>`.
    Approximate(f64),
    /// A `<bit string literal>`'s or `<hex string literal>`'s value
    /// [ISO/IEC 9075:1992 §5.3].
    Bit(Bits),
    /// A `<datetime literal>`'s fields, as written [ISO/IEC 9075:1992
    /// §5.3]: of a displacement the SQL-session gives, if it writes none
    /// \[SR18\].
    Datetime(Written),
    /// An `<interval literal>`'s value, its sign applied [ISO/IEC
    /// 9075:1992 §5.3 GR5].
    Interval(Interval),
}

/// [ISO/IEC 9075:1992 §6.8] A `<datetime value function>`: CURRENT_DATE,
/// CURRENT_TIME or CURRENT_TIMESTAMP, of a precision if written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatetimeFunction {
    /// Which: of a date, a time or a timestamp.
    pub moment: Moment,
    /// The `<time precision>` or `<timestamp precision>`, if written.
    pub precision: Option<u32>,
}

/// [ISO/IEC 9075:1992 §6.6] An `<extract field>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractField {
    /// A `<datetime field>`.
    Datetime(Field),
    /// TIMEZONE_HOUR.
    TimezoneHour,
    /// TIMEZONE_MINUTE.
    TimezoneMinute,
}

// ---------------------------------------------------------------------------
// [X3.135-1986 §7.3] <SQL statement>, and [X3.135-1986 §8.3] <declare cursor>
// ---------------------------------------------------------------------------

/// [X3.135-1986 §7.3] `<SQL statement>` ::= `<close statement>` | `<commit statement>`
///   | `<delete statement: positioned>` | `<delete statement: searched>`
///   | `<fetch statement>` | `<insert statement>` | `<open statement>`
///   | `<rollback statement>` | `<select statement>`
///   | `<update statement: positioned>` | `<update statement: searched>`
#[derive(Debug)]
pub enum SqlStatement {
    /// [X3.135-1986 §8.1] `<close statement>` ::= CLOSE `<cursor name>`
    Close(String),
    /// [X3.135-1986 §8.2] `<commit statement>` ::= COMMIT WORK
    Commit,
    /// [X3.135-1986 §8.4] `<delete statement: positioned>` ::=
    ///     DELETE FROM `<table name>` WHERE CURRENT OF `<cursor name>`
    DeletePositioned {
        /// The `<table name>`.
        table: TableName,
        /// The `<cursor name>`, and where it is written.
        cursor: (String, usize),
    },
    /// [X3.135-1986 §8.5] `<delete statement: searched>` ::=
    ///     DELETE FROM `<table name>` \[WHERE `<search condition>`\]
    DeleteSearched {
        /// The `<table name>`.
        table: TableName,
        /// The `<search condition>`, if written.
        where_clause: Option<SearchCondition>,
    },
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §11.10] `<alter table statement>` ::=
    ///     ALTER TABLE `<table name>` `<alter table action>`
    AlterTable {
        /// The `<table name>`.
        table: TableName,
        /// The `<alter table action>`.
        action: AlterAction,
        /// The `<alter table action>` as written, which the log keeps.
        text: String,
    },
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §11.2, §11.18, §11.20] `<drop
    /// schema statement>`, `<drop table statement>` or `<drop view
    /// statement>`: DROP {SCHEMA `<schema name>` | TABLE `<table name>` |
    /// VIEW `<table name>`} `<drop behavior>`
    Drop {
        /// What it names.
        object: DropObject,
        /// The `<drop behavior>`.
        behavior: DropBehavior,
    },
    /// \[EXTENSION\] A table, view or privileges defined or revoked outside a
    /// `<schema definition>` (`SchemaStatement`).
    Schema {
        /// The statement.
        statement: SchemaStatement,
        /// The statement as written, which the log keeps.
        text: String,
    },
    /// [X3.135-1986 §8.6; ISO/IEC 9075:1992 §13.3] `<fetch statement>` ::=
    /// FETCH \[\[`<fetch orientation>`\] FROM\] `<cursor name>` INTO `<fetch
    /// target list>`
    Fetch {
        /// The `<cursor name>`.
        cursor: String,
        /// The `<fetch orientation>`: NEXT if none is written \[SR1\].
        orientation: FetchOrientation,
        /// The `<fetch target list>`.
        targets: Vec<ParameterSpecification>,
    },
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §18.1] `<get diagnostics statement>`
    GetDiagnostics(GetDiagnostics),
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §13.11, §20.1] `<temporary table
    /// declaration>`: a declared local temporary table, of kind
    /// `TemporaryKind::Declared`.
    DeclareTable(TableDefinition),
    /// [X3.135-1986 §8.7]
    Insert(InsertStatement),
    /// [X3.135-1986 §8.8] `<open statement>` ::= OPEN `<cursor name>`
    Open(String),
    /// [X3.135-1986 §8.9] `<rollback statement>` ::= ROLLBACK WORK
    Rollback,
    /// [X3.135-1986 §8.10] `<select statement>` ::= SELECT \[ALL | DISTINCT\] `<select list>`
    ///     INTO `<select target list>` `<table expression>`
    Select {
        /// The query, without INTO.
        query: Box<QuerySpecification>,
        /// The `<select target list>`.
        targets: Vec<ParameterSpecification>,
    },
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §14.1] `<set transaction
    /// statement>` ::= SET TRANSACTION `<transaction mode>` \[{,
    /// `<transaction mode>`}...\]
    SetTransaction(TransactionModes),
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §16.2] `<set schema statement>` ::=
    /// SET SCHEMA `<value specification>`
    SetSchema(ValueSpecification),
    /// [ISO/IEC 9075:1992 §16.5] `<set local time zone statement>`: SET TIME
    /// ZONE, of an `<interval value expression>`, or LOCAL, none.
    SetTimeZone(Option<ValueExpression>),
    /// [ISO/IEC 9075:1992 §16.4] `<set session authorization identifier
    /// statement>` ::= SET SESSION AUTHORIZATION `<value specification>`
    SetSessionAuthorization(ValueSpecification),
    /// [ISO/IEC 9075:1992 §15] A statement of connections, the SQL-agent's:
    /// a client runs it, never a session.
    Connection(ConnectionStatement),
    /// \[EXTENSION\] SET `<setting name>` {= | TO} {`<value
    /// specification>` | DEFAULT}: the session's own value of a setting
    /// (RULES.md D36), where `<setting name>` ::= `<identifier>` \[{.
    /// `<identifier>`}...\].
    SetSetting {
        /// The setting's name, its parts joined by periods.
        name: String,
        /// Its value, or none for DEFAULT: the database's again.
        value: Option<ValueSpecification>,
    },
    /// \[EXTENSION\] `<set database setting statement>` ::= SET DATABASE
    /// `<setting name>` {`<equals operator>` | TO} {`<value specification>`
    /// | DEFAULT}: the database's value of a setting (RULES.md D36), set by
    /// its owner (D43). ISO/IEC 9075:1992 has no such statement: its SET
    /// statements of a session [§16.1–§16.5] set no database's value.
    SetDatabase {
        /// The setting's name, its parts joined by periods.
        name: String,
        /// Its value, or none for DEFAULT: its default again.
        value: Option<ValueSpecification>,
    },
    /// \[EXTENSION\] CREATE INDEX, DROP INDEX or ALTER INDEX (RULES.md D34,
    /// D41).
    Index(IndexStatement),
    /// \[EXTENSION\] A statement of users (RULES.md D43).
    User(UserStatement),
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §14.2] `<set constraints mode
    /// statement>` ::= SET CONSTRAINTS `<constraint name list>` {DEFERRED |
    /// IMMEDIATE}
    SetConstraints {
        /// The `<constraint name>`s, or none for ALL.
        names: Option<Vec<TableName>>,
        /// DEFERRED, rather than IMMEDIATE.
        deferred: bool,
    },
    /// [X3.135-1986 §8.11] `<update statement: positioned>` ::= UPDATE `<table name>`
    ///     SET `<set clause: positioned>` \[{,`<set clause: positioned>`}...\] WHERE CURRENT OF `<cursor name>`
    UpdatePositioned {
        /// The `<table name>`.
        table: TableName,
        /// The `<set clause>`s, in order.
        set: Vec<SetClause>,
        /// The `<cursor name>`, and where it is written.
        cursor: (String, usize),
    },
    /// [X3.135-1986 §8.12] `<update statement: searched>` ::= UPDATE `<table name>`
    ///     SET `<set clause: searched>` \[{,`<set clause: searched>`}...\] \[WHERE `<search condition>`\]
    UpdateSearched {
        /// The `<table name>`.
        table: TableName,
        /// The `<set clause>`s, in order.
        set: Vec<SetClause>,
        /// The `<search condition>`, if written.
        where_clause: Option<SearchCondition>,
    },
}

/// [ISO/IEC 9075:1992 §11.10] `<alter table action>` ::= `<add column
/// definition>` | `<alter column definition>` | `<drop column definition>`
/// | `<add table constraint definition>` | `<drop table constraint
/// definition>`
#[derive(Debug)]
pub enum AlterAction {
    /// [ISO/IEC 9075:1992 §11.11] `<add column definition>` ::= ADD \[COLUMN\]
    /// `<column definition>`
    AddColumn(ColumnDefinition),
    /// [ISO/IEC 9075:1992 §11.15] `<drop column definition>` ::= DROP
    /// \[COLUMN\] `<column name>` `<drop behavior>`
    DropColumn {
        /// The `<column name>`.
        column: String,
        /// Where it begins in the statement, for the positions of errors.
        at: usize,
        /// The `<drop behavior>`.
        behavior: DropBehavior,
    },
    /// [ISO/IEC 9075:1992 §11.12] `<alter column definition>` ::= ALTER
    /// \[COLUMN\] `<column name>` {SET `<default clause>` | DROP DEFAULT}
    /// [§11.13, §11.14]
    AlterColumn {
        /// The `<column name>`.
        column: String,
        /// Where it begins in the statement, for the positions of errors.
        at: usize,
        /// The `<default option>` set, or none for DROP DEFAULT.
        default: Option<DefaultOption>,
    },
    /// [ISO/IEC 9075:1992 §11.16] `<add table constraint definition>` ::=
    /// ADD `<table constraint definition>`
    AddConstraint(TableConstraint),
    /// [ISO/IEC 9075:1992 §11.17] `<drop table constraint definition>` ::=
    /// DROP CONSTRAINT `<constraint name>` `<drop behavior>`
    DropConstraint {
        /// The `<constraint name>`.
        name: TableName,
        /// The `<drop behavior>`.
        behavior: DropBehavior,
    },
    /// \[EXTENSION\] `<alter table storage>` ::= SET `<table options>` |
    /// USING `<engine name>` \[WITH `<table options>`\] (RULES.md D41)
    Storage(TableStorage),
}

/// \[EXTENSION\] A statement of users (RULES.md D43), run outside a
/// transaction by the database's owner. ISO/IEC 9075:1992 has none: which
/// identifiers are authorization identifiers is implementation-defined
/// [§5.4; Annex B, item 41].
#[derive(Debug)]
pub enum UserStatement {
    /// `<user definition>` ::= CREATE USER `<authorization identifier>`
    /// \[WITH\] PASSWORD `<character string literal>`
    Create {
        /// The `<authorization identifier>`, and where it begins.
        name: (String, usize),
        /// The password, the literal's value.
        password: String,
    },
    /// `<alter user statement>` ::= ALTER USER `<authorization identifier>`
    /// \[WITH\] PASSWORD `<character string literal>`
    Alter {
        /// The `<authorization identifier>`, and where it begins.
        name: (String, usize),
        /// The new password.
        password: String,
    },
    /// `<drop user statement>` ::= DROP USER `<authorization identifier>`
    Drop {
        /// The `<authorization identifier>`, and where it begins.
        name: (String, usize),
    },
}

/// \[EXTENSION\] A statement of indexes (RULES.md D34, D41), run outside a
/// transaction, as the index is built while transactions write its table.
#[derive(Debug)]
pub enum IndexStatement {
    /// `<create index statement>` ::= CREATE INDEX `<index name>` ON
    /// `<table name>` ( `<column name list>` ) \[USING `<method name>`\]
    /// \[WITH `<table options>`\]
    Create {
        /// The `<index name>`, an `<identifier>`, and where it begins.
        name: (String, usize),
        /// The `<table name>`.
        table: TableName,
        /// The `<column name>`s, in order, and where each begins.
        columns: Vec<(String, usize)>,
        /// The method, as an engine is named, and its options.
        storage: TableStorage,
    },
    /// `<drop index statement>` ::= DROP INDEX `<index name>`
    Drop {
        /// The `<index name>`, and where it begins.
        name: (String, usize),
    },
    /// `<alter index statement>` ::= ALTER INDEX `<index name>` SET
    /// `<table options>`
    Alter {
        /// The `<index name>`, and where it begins.
        name: (String, usize),
        /// The `<table option>`s, in order.
        options: Vec<TableOptionValue>,
    },
}

/// What a DROP statement names [ISO/IEC 9075:1992 §11.2, §11.18, §11.20,
/// §11.27].
#[derive(Debug, Clone)]
pub enum DropObject {
    /// A base table.
    Table(TableName),
    /// A view.
    View(TableName),
    /// A domain.
    Domain(TableName),
    /// An assertion [§11.35], which DROP names with no `<drop behavior>`.
    Assertion(TableName),
    /// A schema: `<schema name>` ::= \[`<catalog name>` .\] `<unqualified
    /// schema name>` [§5.4], its name an `<authorization identifier>` as
    /// every schema's is.
    Schema {
        /// The `<catalog name>`, if written.
        catalog: Option<String>,
        /// The `<unqualified schema name>`.
        name: String,
        /// Where it begins in the statement.
        at: usize,
    },
}

/// [ISO/IEC 9075:1992 §11.2] `<drop behavior>` ::= CASCADE | RESTRICT
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropBehavior {
    /// CASCADE: what depends on the object is dropped with it.
    Cascade,
    /// RESTRICT: the object is not dropped while anything depends on it.
    Restrict,
}

/// [X3.135-1986 §8.11, §8.12] `<set clause>` ::= `<object column>` = `<update source>`
#[derive(Debug)]
pub struct SetClause {
    /// The `<object column>`.
    pub column: String,
    /// The `<update source>`.
    pub value: UpdateSource,
    /// Where it begins in the statement: a character offset, for the
    /// positions of errors (`SqlError::or_at`).
    pub at: usize,
}

/// [ISO/IEC 9075:1992 §13.9] `<update source>` ::= `<value expression>` |
/// `<null specification>` | DEFAULT
#[derive(Debug)]
pub enum UpdateSource {
    /// A `<value expression>`.
    Value(ValueExpression),
    /// NULL.
    Null,
    /// DEFAULT: the column's default.
    Default,
}

/// [X3.135-1986 §8.3; ISO/IEC 9075:1992 §13.1] `<declare cursor>` ::=
/// DECLARE `<cursor name>` \[INSENSITIVE\] \[SCROLL\] CURSOR FOR `<cursor
/// specification>`
///
/// The `<cursor specification>` is kept as text: its `<parameter name>`s are
/// those of the procedure that OPENs it [X3.135-1986 §8.3 SR2], so it is
/// parsed and bound at OPEN [X3.135-1986 §8.8 GR3a].
#[derive(Debug, Clone)]
pub struct DeclareCursor {
    /// The `<cursor name>`.
    pub name: String,
    /// INSENSITIVE was written.
    pub insensitive: bool,
    /// SCROLL was written.
    pub scroll: bool,
    /// The text of the `<cursor specification>`.
    pub specification: String,
}

/// [ISO/IEC 9075:1992 §13.3] `<fetch orientation>` ::= NEXT | PRIOR | FIRST
/// | LAST | {ABSOLUTE | RELATIVE} `<simple value specification>`
#[derive(Debug, Clone)]
pub enum FetchOrientation {
    /// NEXT.
    Next,
    /// PRIOR.
    Prior,
    /// FIRST.
    First,
    /// LAST.
    Last,
    /// ABSOLUTE, with the row's number.
    Absolute(ValueSpecification),
    /// RELATIVE, with how far.
    Relative(ValueSpecification),
}

/// [ISO/IEC 9075:1992 §14.1] The `<transaction mode>`s of a `<set
/// transaction statement>`, each written at most once \[SR1\].
#[derive(Debug, Default)]
pub struct TransactionModes {
    /// `<isolation level>`, if written.
    pub isolation: Option<IsolationLevel>,
    /// `<transaction access mode>`, if written.
    pub access: Option<AccessMode>,
    /// `<diagnostics size>`: its `<number of conditions>`, a `<simple value
    /// specification>`, if written.
    pub diagnostics: Option<ValueSpecification>,
}

/// [ISO/IEC 9075:1992 §14.1] `<level of isolation>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationLevel {
    /// READ UNCOMMITTED.
    ReadUncommitted,
    /// READ COMMITTED.
    ReadCommitted,
    /// REPEATABLE READ.
    RepeatableRead,
    /// SERIALIZABLE.
    Serializable,
}

/// [ISO/IEC 9075:1992 §14.1] `<transaction access mode>` ::= READ ONLY |
/// READ WRITE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    /// READ ONLY.
    ReadOnly,
    /// READ WRITE.
    ReadWrite,
}

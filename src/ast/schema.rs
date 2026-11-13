//! Abstract syntax of the schema definition language [X3.135-1986 §6;
//! ISO/IEC 9075:1992 §11].

use super::{
    AlterSequence, DatetimeFunction, DropBehavior, Literal, QueryExpression, SearchCondition, SequenceDefinition, TableName,
    TypeDefinition, UserFunction,
};
use crate::value::DataType;
use crate::value::collation::Pad;

/// [X3.135-1986 §6.1; ISO/IEC 9075:1992 §11.1] `<schema definition>` ::=
/// CREATE SCHEMA `<schema name clause>` \[`<schema element>`...\]
/// `<schema name clause>` ::= `<schema name>` | AUTHORIZATION `<schema
/// authorization identifier>` | `<schema name>` AUTHORIZATION `<schema
/// authorization identifier>`
/// `<schema element>` ::= `<domain definition>` | `<table definition>` | `<view definition>`
///   | `<grant statement>` | `<assertion definition>` | `<collation definition>`
///   | `<sequence generator definition>`; a character set
///   or translation definition is read, then refused.
#[derive(Debug)]
pub struct Schema {
    /// The `<catalog name>` of its `<schema name>`, if written.
    pub catalog: Option<String>,
    /// Its `<unqualified schema name>`, the `<schema authorization
    /// identifier>` if none is written \[SR1\].
    pub name: String,
    /// The `<schema authorization identifier>`, if written: else the
    /// session's \[SR2b\].
    pub auth: Option<String>,
    /// The `<collation definition>`s, in order [ISO/IEC 9075-2:2023 §11.43].
    pub collations: Vec<CollationDefinition>,
    /// The `<sequence generator definition>`s, in order [ISO/IEC
    /// 9075-2:2023 §11.72].
    pub sequences: Vec<SequenceDefinition>,
    /// The `<user-defined type definition>`s, in order [ISO/IEC 9075-2:2023
    /// §11.51].
    pub types: Vec<TypeDefinition>,
    /// The `<domain definition>`s, in order [ISO/IEC 9075:1992 §11.21].
    pub domains: Vec<DomainDefinition>,
    /// The `<assertion definition>`s, in order [§11.34].
    pub assertions: Vec<AssertionDefinition>,
    /// The `<table definition>`s, in order.
    pub tables: Vec<TableDefinition>,
    /// The `<view definition>`s, in order.
    pub views: Vec<ViewDefinition>,
    /// The `<privilege definition>`s, in order.
    pub privileges: Vec<PrivilegeDefinition>,
}

/// \[EXTENSION\] [ISO/IEC 9075:1992 §20.1] An `<SQL schema statement>` of
/// those implemented outside a `<schema definition>`, run in a transaction
/// (RULES.md D33): a `<table definition>` [§11.3], a `<view definition>`
/// [§11.19], a `<grant statement>` [§11.36], a `<revoke statement>`
/// [§11.37], a `<domain definition>` [§11.21], an `<alter domain
/// statement>` [§11.22] or an `<assertion definition>` [§11.34].
#[derive(Debug)]
pub enum SchemaStatement {
    /// A `<table definition>`.
    Table(TableDefinition),
    /// A `<view definition>`.
    View(ViewDefinition),
    /// A `<grant statement>`.
    Grant(PrivilegeDefinition),
    /// A `<revoke statement>`.
    Revoke(RevokeStatement),
    /// A `<domain definition>` [ISO/IEC 9075:1992 §11.21].
    Domain(DomainDefinition),
    /// An `<alter domain statement>` [§11.22].
    AlterDomain(AlterDomain),
    /// An `<assertion definition>` [§11.34].
    Assertion(AssertionDefinition),
    /// A `<collation definition>` [ISO/IEC 9075-2:2023 §11.43].
    Collation(CollationDefinition),
    /// A `<sequence generator definition>` [ISO/IEC 9075-2:2023 §11.72].
    Sequence(SequenceDefinition),
    /// An `<alter sequence generator statement>` [ISO/IEC 9075-2:2023
    /// §11.73].
    AlterSequence(AlterSequence),
    /// A `<user-defined type definition>` [ISO/IEC 9075-2:2023 §11.51].
    Type(TypeDefinition),
}

/// [ISO/IEC 9075-2:2023 §11.43] `<collation definition>` ::= CREATE COLLATION
/// `<collation name>` FOR `<character set specification>` FROM `<existing
/// collation name>` \[`<pad characteristic>`\]
#[derive(Debug)]
pub struct CollationDefinition {
    /// The `<collation name>`.
    pub name: TableName,
    /// The `<existing collation name>`.
    pub from: TableName,
    /// The `<pad characteristic>`, if written.
    pub pad: Option<Pad>,
}

/// [ISO/IEC 9075:1992 §11.36] What an `<object name>` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivilegeObject {
    /// A table or view.
    Table,
    /// A domain.
    Domain,
    /// A collation [ISO/IEC 9075-2:2023 §11.43].
    Collation,
    /// A sequence generator [ISO/IEC 9075-2:2023 §11.72].
    Sequence,
    /// A user-defined type [ISO/IEC 9075-2:2023 §12.3].
    Type,
    /// [ISO/IEC 9075-2:2023 §12.3] SPECIFIC FUNCTION or SPECIFIC ROUTINE
    /// `<specific name>`.
    SpecificRoutine,
    /// [ISO/IEC 9075-2:2023 §12.3] FUNCTION or ROUTINE `<member name>`,
    /// its `<data type list>` if written.
    Routine,
}

/// [ISO/IEC 9075:1992 §11.34] `<assertion definition>` ::= CREATE ASSERTION
/// `<constraint name>` CHECK ( `<search condition>` ) \[`<constraint
/// attributes>`\]
#[derive(Debug)]
pub struct AssertionDefinition {
    /// The `<constraint name>`.
    pub name: TableName,
    /// The `<assertion check>`.
    pub check: Check,
    /// Its `<constraint attributes>`.
    pub attributes: ConstraintAttributes,
}

/// [ISO/IEC 9075:1992 §11.21] `<domain definition>` ::= CREATE DOMAIN
/// `<domain name>` \[AS\] `<data type>` \[`<default clause>`\] \[`<domain
/// constraint>`...\]; \[EXTENSION\] or of a type beyond the standard
/// (RULES.md D53).
#[derive(Debug)]
pub struct DomainDefinition {
    /// The `<domain name>`.
    pub name: TableName,
    /// The `<data type>`, or \[EXTENSION\] a type beyond the standard: a
    /// name alone is one, never a domain.
    pub data_type: TypeName,
    /// The `<default option>` of its `<default clause>`, if written.
    pub default: Option<DefaultOption>,
    /// The `<domain constraint>`s: each a `<check constraint definition>`,
    /// VALUE in it a column named VALUE.
    pub constraints: Vec<TableConstraint>,
    /// The `<collation name>` of its `<collate clause>`, if written.
    pub collate: Option<TableName>,
}

/// [ISO/IEC 9075:1992 §11.22] `<alter domain statement>` ::= ALTER DOMAIN
/// `<domain name>` `<alter domain action>`
#[derive(Debug)]
pub struct AlterDomain {
    /// The `<domain name>`.
    pub name: TableName,
    /// The `<alter domain action>`.
    pub action: AlterDomainAction,
}

/// [ISO/IEC 9075:1992 §11.22–§11.26] `<alter domain action>`.
#[derive(Debug)]
pub enum AlterDomainAction {
    /// SET `<default clause>` [§11.23].
    SetDefault(DefaultOption),
    /// DROP DEFAULT [§11.24].
    DropDefault,
    /// ADD `<domain constraint>` [§11.25].
    AddConstraint(TableConstraint),
    /// DROP CONSTRAINT `<constraint name>` [§11.26].
    DropConstraint(TableName),
}

/// [ISO/IEC 9075:1992 §11.4, §6.10] A `<data type>`, or a `<domain name>`:
/// a column's type, or a CAST's target. [ISO/IEC 9075-2:2023 §6.1] A
/// `<data type>` may name a user-defined type, as an array's element type
/// or a row's field type too, so a type written is a tree of the types it
/// names, resolved as a statement is processed.
#[derive(Debug, Clone)]
pub enum TypeName {
    /// A `<data type>` that names no type.
    Data(DataType),
    /// A `<domain name>`: the domain's data type; or [ISO/IEC 9075-2:2023
    /// §6.1] a `<path-resolved user-defined type name>`, a distinct type
    /// (RULES.md D69); or, unqualified and neither's, a type beyond the
    /// standard (RULES.md D53).
    Domain(TableName),
    /// [ISO/IEC 9075-2:2023 §6.1] `<data type>` ARRAY \[`[` `<maximum
    /// cardinality>` `]`\], its element type naming a type.
    Array(Box<TypeName>, u32),
    /// [ISO/IEC 9075-2:2023 §6.1] `<row type>`, a field's type naming a
    /// type: each field's name and type.
    Row(Vec<(String, TypeName)>),
    /// \[EXTENSION\] A type beyond the standard with parameters, as its
    /// name and `(a [, b])` write it (RULES.md D53): no domain takes
    /// parameters.
    Extension {
        /// The type's name.
        name: TableName,
        /// Its parameters, as written.
        params: Vec<u32>,
    },
}

/// [ISO/IEC 9075:1992 §11.37] `<revoke statement>` ::= REVOKE \[GRANT
/// OPTION FOR\] `<privileges>` ON `<object name>` FROM `<grantee>` \[{,
/// `<grantee>`}...\] `<drop behavior>`
#[derive(Debug)]
pub struct RevokeStatement {
    /// GRANT OPTION FOR was written: the grant option goes, the privilege stays.
    pub grant_option_for: bool,
    /// The `<action>`s, or none for ALL PRIVILEGES.
    pub actions: Option<Vec<Action>>,
    /// The `<table name>`, `<domain name>` or `<collation name>`.
    pub table: TableName,
    /// What it names [§11.36].
    pub object: PrivilegeObject,
    /// [ISO/IEC 9075-2:2023 §12.3] Of a routine named by FUNCTION or
    /// ROUTINE, its `<data type list>`, if written.
    pub parameters: Option<Vec<TypeName>>,
    /// The `<grantee>`s: an `<authorization identifier>`, or none for PUBLIC.
    pub grantees: Vec<Option<String>>,
    /// The `<drop behavior>`.
    pub behavior: DropBehavior,
}

/// [X3.135-1986 §6.5; ISO/IEC 9075:1992 §11.19] `<view definition>` ::=
/// CREATE VIEW `<table name>` \[(`<view column list>`)\] AS `<query
/// expression>` \[WITH \[`<levels clause>`\] CHECK OPTION\]
#[derive(Debug)]
pub struct ViewDefinition {
    /// The `<table name>`.
    pub name: TableName,
    /// The `<view column list>`, if written.
    pub columns: Option<Vec<String>>,
    /// The `<query expression>`.
    pub query: QueryExpression,
    /// The `<query expression>` as written, which the catalog keeps.
    pub text: String,
    /// WITH CHECK OPTION, if written, of its levels, CASCADED if none
    /// were written [ISO/IEC 9075:1992 §11.19 SR13].
    pub check_option: Option<CheckLevels>,
}

/// [ISO/IEC 9075:1992 §11.19] `<levels clause>` ::= CASCADED | LOCAL: which
/// views a view's WITH CHECK OPTION checks a row against \[GR11\].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckLevels {
    /// The row appears in the view: its condition and those of the views
    /// under it hold \[GR11a\].
    Cascaded,
    /// The view's own condition holds of a row that appears in the table
    /// its query reads \[GR11b\].
    Local,
}

/// [X3.135-1986 §6.6] `<privilege definition>` ::= GRANT `<privileges>` ON
/// `<table name>` TO `<grantee>` \[{, `<grantee>`}...\] \[WITH GRANT OPTION\]
#[derive(Debug)]
pub struct PrivilegeDefinition {
    /// The `<action>`s, or none for ALL PRIVILEGES.
    pub actions: Option<Vec<Action>>,
    /// The `<table name>`, `<domain name>` or `<collation name>`.
    pub table: TableName,
    /// What it names [ISO/IEC 9075:1992 §11.36].
    pub object: PrivilegeObject,
    /// [ISO/IEC 9075-2:2023 §12.3] Of a routine named by FUNCTION or
    /// ROUTINE, its `<data type list>`, if written.
    pub parameters: Option<Vec<TypeName>>,
    /// The `<grantee>`s: an `<authorization identifier>`, or none for PUBLIC.
    pub grantees: Vec<Option<String>>,
    /// WITH GRANT OPTION was written.
    pub grant_option: bool,
}

/// [X3.135-1986 §6.6] `<action>` ::= SELECT | INSERT | DELETE | UPDATE
/// \[(`<grant column list>`)\]; [ISO/IEC 9075:1992 §10.3] INSERT and
/// REFERENCES of a `<privilege column list>`, and USAGE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// SELECT.
    Select,
    /// INSERT, on the columns of the `<privilege column list>`, or on
    /// every column if none is written [ISO/IEC 9075:1992 §10.3 GR4].
    Insert(Option<Vec<String>>),
    /// DELETE.
    Delete,
    /// UPDATE, on the columns of the `<grant column list>`, or on every
    /// column if none is written [X3.135-1986 §6.6 SR2].
    Update(Option<Vec<String>>),
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §10.3] REFERENCES, on the columns
    /// listed, or on every column if none is.
    References(Option<Vec<String>>),
    /// [ISO/IEC 9075:1992 §10.3] USAGE, of a domain.
    Usage,
    /// [ISO/IEC 9075-2:2023 §12.3] EXECUTE, of a routine.
    Execute,
    /// [ISO/IEC 9075-2:2023 §12.3] UNDER, of a structured type.
    Under,
}

/// [X3.135-1986 §6.2; ISO/IEC 9075:1992 §11.3] `<table definition>` ::=
///     CREATE \[{GLOBAL | LOCAL} TEMPORARY\] TABLE `<table name>`
///     (`<table element>` \[{,`<table element>`}...\]) \[ON COMMIT {DELETE |
///     PRESERVE} ROWS\]
///
/// [ISO/IEC 9075-2:2023 §11.3] Its `<table contents source>`, a `<table
/// element list>` or an `<as subquery clause>`.
#[derive(Debug)]
pub struct TableDefinition {
    /// The `<table name>`.
    pub name: TableName,
    /// Of a temporary table, its kind and ON COMMIT, DELETE ROWS if none
    /// is written \[SR4\].
    pub temporary: Option<Temporary>,
    /// The `<table element>`s, in order: none of an `<as subquery
    /// clause>`.
    pub elements: Vec<TableElement>,
    /// Its `<as subquery clause>`, if written.
    pub subquery: Option<Box<super::AsSubquery>>,
    /// \[EXTENSION\] Its engine and options, if written.
    pub storage: TableStorage,
}

/// \[EXTENSION\] `<table storage>` ::= \[USING `<engine name>`\] \[WITH
/// `<table options>`\], after a `<table definition>`'s elements (RULES.md
/// D41); `<table options>` ::= ( `<table option>` \[{, `<table
/// option>`}...\] )
#[derive(Debug, Default)]
pub struct TableStorage {
    /// The `<engine name>`, an `<identifier>`, and where it begins.
    pub engine: Option<(String, usize)>,
    /// The `<table option>`s, in order.
    pub options: Vec<TableOptionValue>,
}

/// \[EXTENSION\] `<table option>` ::= `<option name>` = {`<literal>` |
/// `<identifier>` | DEFAULT}, its name `<identifier>`s joined by periods.
#[derive(Debug)]
pub struct TableOptionValue {
    /// The `<option name>`, its parts joined by periods.
    pub name: String,
    /// Where it begins in the statement, for the positions of errors.
    pub at: usize,
    /// Its value; none for DEFAULT.
    pub value: Option<OptionValue>,
}

/// \[EXTENSION\] The value of a `<table option>`.
#[derive(Debug)]
pub enum OptionValue {
    /// A `<literal>`.
    Literal(Literal),
    /// An `<identifier>`: a word, or a name.
    Word(String),
}

/// [X3.135-1986 §6.2] `<table element>` ::= `<column definition>` |
/// `<table constraint definition>` [ISO/IEC 9075:1992 §11.3] | `<like
/// clause>` [ISO/IEC 9075-2:2023 §11.3]
#[derive(Debug)]
pub enum TableElement {
    /// A `<column definition>`.
    Column(ColumnDefinition),
    /// A `<table constraint definition>`.
    Constraint(TableConstraint),
    /// A `<like clause>`.
    Like(super::LikeClause),
}

/// [X3.135-1986 §6.3] `<column definition>` ::= `<column name>` {`<data type>` | `<domain name>`}
/// \[`<default clause>`\] \[`<column constraint definition>`...\] [ISO/IEC
/// 9075:1992 §11.4]
///
/// \[`<collate clause>`\] [ISO/IEC 9075-2:2023 §10.7, Feature F692]; an
/// `<identity column specification>` or a `<generation clause>` in place
/// of the `<default clause>`, and no type with a `<generation clause>`
/// [ISO/IEC 9075-2:2023 §11.4].
#[derive(Debug)]
pub struct ColumnDefinition {
    /// The `<column name>`.
    pub name: String,
    /// Where it begins in the statement: a character offset, for the
    /// positions of errors (`SqlError::or_at`).
    pub at: usize,
    /// Its `<data type>` or `<domain name>`, which only a column with a
    /// `<generation clause>` may not write [ISO/IEC 9075-2:2023 §11.4
    /// SR11].
    pub data_type: Option<TypeName>,
    /// The `<default option>` of its `<default clause>`, if written.
    pub default: Option<DefaultOption>,
    /// [ISO/IEC 9075-2:2023 §11.4] Its `<identity column specification>`
    /// or `<generation clause>`, if written.
    pub generated: Option<Box<super::ColumnGeneration>>,
    /// The `<column constraint definition>`s, in order.
    pub constraints: Vec<ColumnConstraint>,
    /// The `<collation name>` of its `<collate clause>`, if written.
    pub collate: Option<TableName>,
}

/// [ISO/IEC 9075:1992 §11.5] `<default option>` ::= `<literal>` | USER |
/// CURRENT_USER | SESSION_USER | SYSTEM_USER | `<datetime value function>`
/// | NULL
#[derive(Debug, Clone)]
pub enum DefaultOption {
    /// A `<literal>`.
    Literal(Literal),
    /// USER or another `<authorization identifier>`.
    User(UserFunction),
    /// [ISO/IEC 9075:1992 §6.8] A `<datetime value function>`.
    Datetime(DatetimeFunction),
    /// NULL.
    Null,
    /// [ISO/IEC 9075-2:2023 §6.5] An `<empty specification>` of an array,
    /// ARRAY `[` `]`.
    Empty,
}

/// [ISO/IEC 9075:1992 §11.4] `<column constraint definition>` ::=
/// \[`<constraint name definition>`\] `<column constraint>`
/// \[`<constraint attributes>`\]
///
/// `<column constraint>` ::= NOT NULL | `<unique specification>` |
/// `<references specification>` | `<check constraint definition>`
#[derive(Debug)]
pub struct ColumnConstraint {
    /// The `<constraint name>`, if written [§10.6].
    pub name: Option<TableName>,
    /// Where it begins in the statement.
    pub at: usize,
    /// What it constrains the column to.
    pub kind: ColumnConstraintKind,
    /// Its `<constraint attributes>`.
    pub attributes: ConstraintAttributes,
}

/// [ISO/IEC 9075:1992 §10.6] What `<constraint attributes>` say, written or
/// implicit \[SR3, SR4\]: by default, not deferrable and immediate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConstraintAttributes {
    /// DEFERRABLE \[GR1\].
    pub deferrable: bool,
    /// INITIALLY DEFERRED: its initial constraint mode deferred \[GR2\].
    pub initially_deferred: bool,
}

/// [ISO/IEC 9075:1992 §11.4] `<column constraint>`.
#[derive(Debug)]
pub enum ColumnConstraintKind {
    /// NOT NULL.
    NotNull,
    /// UNIQUE, or PRIMARY KEY if `primary` [§11.7].
    Unique {
        /// PRIMARY KEY was written.
        primary: bool,
    },
    /// A `<references specification>` [§11.8].
    References(References),
    /// A `<check constraint definition>` [§11.9].
    Check(Check),
}

/// [ISO/IEC 9075:1992 §11.6] `<table constraint definition>` ::=
/// \[`<constraint name definition>`\] `<table constraint>`
/// \[`<constraint attributes>`\]
#[derive(Debug)]
pub struct TableConstraint {
    /// The `<constraint name>`, if written [§10.6].
    pub name: Option<TableName>,
    /// Where it begins in the statement.
    pub at: usize,
    /// The `<table constraint>`.
    pub kind: TableConstraintKind,
    /// Its `<constraint attributes>`.
    pub attributes: ConstraintAttributes,
}

/// [ISO/IEC 9075-2:2023 §8.11 SR3, §11.7 SR5] The `<unique null treatment>`
/// implicit where none is written, implementation-defined (ID106): NULLS
/// DISTINCT, as every key and UNIQUE predicate was before one could be
/// written. A `<column constraint>` writes none [§11.4].
pub const IMPLICIT_NULLS_DISTINCT: bool = true;

/// [ISO/IEC 9075:1992 §11.6] `<table constraint>` ::= `<unique constraint
/// definition>` | `<referential constraint definition>` | `<check
/// constraint definition>`
#[derive(Debug)]
pub enum TableConstraintKind {
    /// [§11.7] `<unique specification>` \[`<unique null treatment>`\]
    /// (`<unique column list>`): UNIQUE, or PRIMARY KEY if `primary`.
    Unique {
        /// PRIMARY KEY was written.
        primary: bool,
        /// The `<unique column list>`.
        columns: Vec<String>,
        /// [ISO/IEC 9075-2:2023 §11.7 SR5] NULLS DISTINCT, written or
        /// implicit, rather than NULLS NOT DISTINCT.
        nulls_distinct: bool,
    },
    /// [§11.8] FOREIGN KEY (`<referencing columns>`) `<references
    /// specification>`
    ForeignKey {
        /// The `<referencing columns>`.
        columns: Vec<String>,
        /// The `<references specification>`.
        references: References,
    },
    /// [§11.9] A `<check constraint definition>`.
    Check(Check),
}

/// [ISO/IEC 9075:1992 §11.8] `<references specification>` ::= REFERENCES
/// `<referenced table and columns>` \[MATCH `<match type>`\]
/// \[`<referential triggered action>`\]
#[derive(Debug, Clone)]
pub struct References {
    /// The referenced `<table name>`.
    pub table: TableName,
    /// The `<reference column list>`, if written; else the referenced
    /// table's PRIMARY KEY \[SR2b\].
    pub columns: Option<Vec<String>>,
    /// The `<match type>`, or none.
    pub match_type: MatchType,
    /// The `<update rule>`, NO ACTION if none is written \[SR8\].
    pub on_update: ReferentialAction,
    /// The `<delete rule>`, NO ACTION if none is written \[SR9\].
    pub on_delete: ReferentialAction,
}

/// [ISO/IEC 9075:1992 §11.8, §8.10] `<match type>` ::= FULL | PARTIAL, or
/// none written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchType {
    /// None written: a row with a null matches [§8.10 GR2a].
    Simple,
    /// PARTIAL: the values not null match [§8.10 GR3].
    Partial,
    /// FULL: all null, or none null and matching [§8.10 GR4].
    Full,
}

/// [ISO/IEC 9075:1992 §11.8] `<referential action>` ::= CASCADE | SET NULL
/// | SET DEFAULT | NO ACTION
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferentialAction {
    /// CASCADE.
    Cascade,
    /// SET NULL.
    SetNull,
    /// SET DEFAULT: each column's default [§11.5].
    SetDefault,
    /// NO ACTION.
    NoAction,
}

/// [ISO/IEC 9075:1992 §11.9] `<check constraint definition>` ::= CHECK
/// (`<search condition>`)
#[derive(Debug)]
pub struct Check {
    /// The `<search condition>`.
    pub condition: SearchCondition,
    /// The `<search condition>` as written, which the catalog keeps.
    pub text: String,
}

/// [ISO/IEC 9075:1992 §4.9] A temporary table's kind, and what a commit
/// does to its rows [§11.3 SR4, §14.3 GR3].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Temporary {
    /// Its kind.
    pub kind: TemporaryKind,
    /// ON COMMIT DELETE ROWS, rather than PRESERVE ROWS.
    pub delete_rows: bool,
}

/// [ISO/IEC 9075:1992 §4.9] The kinds of temporary table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemporaryKind {
    /// GLOBAL TEMPORARY: a global temporary table.
    Global,
    /// LOCAL TEMPORARY: a created local temporary table.
    CreatedLocal,
    /// DECLARE LOCAL TEMPORARY TABLE: a declared local temporary table
    /// [§13.11].
    Declared,
}

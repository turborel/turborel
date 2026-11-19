//! Abstract syntax of the `<insert statement>` [X3.135-1986 §8.7].

use super::{LexicallyScopedOptions, Overriding, QueryExpression, TableName};

/// [X3.135-1986 §8.7; ISO/IEC 9075:1992 §13.8] `<insert statement>` ::=
///     INSERT INTO `<table name>` {\[(`<insert column list>`)\] `<query
///     expression>` | DEFAULT VALUES}
#[derive(Debug, Clone)]
pub struct InsertStatement {
    /// The `<table name>`.
    pub table: TableName,
    /// The `<insert column list>`, if written.
    pub columns: Option<Vec<ColumnName>>,
    /// [ISO/IEC 9075-2:2023 §14.11] The `<override clause>`, if written.
    pub overriding: Option<Overriding>,
    /// The rows inserted.
    pub source: InsertSource,
    /// [ISO/IEC 9075-14:2006 draft §14.4] WITH's `<XML lexically scoped
    /// options>`, if written.
    pub xml: Option<LexicallyScopedOptions>,
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
#[derive(Debug, Clone)]
pub enum InsertSource {
    /// [ISO/IEC 9075:1992 §13.8] A `<query expression>`: its rows. SQL-86's
    /// VALUES (`<insert value list>`) is a `<table value constructor>` of
    /// one row.
    Query(Box<QueryExpression>),
    /// [ISO/IEC 9075:1992 §13.8 SR2] DEFAULT VALUES: one row, each column's
    /// default.
    DefaultValues,
}

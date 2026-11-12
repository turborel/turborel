//! [ISO/IEC 9075-2:2023 §11.3] What a `<table definition>` takes its
//! columns from beside its `<column definition>`s: a `<like clause>`, of
//! another table's columns (Features T171, T173), or an `<as subquery
//! clause>`, of a query's (Feature T172).

use super::{ColumnName, QueryExpression, TableName};

/// [ISO/IEC 9075-2:2023 §11.3] `<like clause>` ::= LIKE `<table name>`
/// \[`<like options>`\], each `<like option>` written at most once
/// \[SR9b iv 1\].
#[derive(Debug)]
pub struct LikeClause {
    /// The `<table name>` of the table whose columns it copies.
    pub table: TableName,
    /// Where LIKE begins in the statement.
    pub at: usize,
    /// INCLUDING IDENTITY, or EXCLUDING IDENTITY, if written: whether
    /// INCLUDING.
    pub identity: Option<bool>,
    /// INCLUDING DEFAULTS, or EXCLUDING DEFAULTS, if written.
    pub defaults: Option<bool>,
    /// INCLUDING GENERATED, or EXCLUDING GENERATED, if written.
    pub generated: Option<bool>,
}

impl LikeClause {
    /// Whether it writes `<like options>`, which a viewed table's does not
    /// \[SR9b ii\].
    pub fn has_options(&self) -> bool {
        self.identity.is_some() || self.defaults.is_some() || self.generated.is_some()
    }
}

/// [ISO/IEC 9075-2:2023 §11.3] `<as subquery clause>` ::= \[ ( `<column
/// name list>` ) \] AS `<table subquery>` `<with or without data>`
#[derive(Debug)]
pub struct AsSubquery {
    /// The `<column name list>`, if written.
    pub columns: Option<Vec<ColumnName>>,
    /// The `<query expression>` of the `<table subquery>`.
    pub query: QueryExpression,
    /// WITH DATA, rather than WITH NO DATA: its rows inserted \[GR10\].
    pub with_data: bool,
    /// Where AS begins in the statement.
    pub at: usize,
}

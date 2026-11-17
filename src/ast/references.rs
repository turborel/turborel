//! Abstract syntax of a `<references specification>` [ISO/IEC 9075:1992
//! §11.8; ISO/IEC 9075-2:2023 §11.8]: the table and columns referenced,
//! its period, its match type and its referential actions.

use super::TableName;

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
    /// [ISO/IEC 9075-2:2023 §11.8] The `<application time period name>`
    /// of a `<referenced period specification>`, if written.
    pub period: Option<String>,
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
    /// [ISO/IEC 9075-2:2023 §15.18 GR9a iv, GR10a iv] RESTRICT: refused
    /// while a row matched the row before the statement (Feature T191).
    Restrict,
}

impl ReferentialAction {
    /// Whether it writes the rows that match a row deleted or updated:
    /// CASCADE, SET NULL and SET DEFAULT.
    pub fn writes(self) -> bool {
        matches!(self, ReferentialAction::Cascade | ReferentialAction::SetNull | ReferentialAction::SetDefault)
    }
}

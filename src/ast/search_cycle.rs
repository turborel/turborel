//! Abstract syntax of the `<search or cycle clause>` of a `<with list
//! element>` [ISO/IEC 9075-2:2023 §7.18]: its `<search clause>`, its
//! `<cycle clause>`, or both.

use super::ValueExpression;

/// [ISO/IEC 9075-2:2023 §7.18] `<search clause>` ::= SEARCH `<recursive
/// search order>` SET `<sequence column>`.
#[derive(Debug, Clone)]
pub struct SearchClause {
    /// The `<recursive search order>`'s DEPTH FIRST or BREADTH FIRST.
    pub order: SearchOrder,
    /// The `<column name list>` of BY, in order.
    pub columns: Vec<String>,
    /// The `<sequence column>`.
    pub sequence: String,
    /// Where SEARCH is written.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §7.18] The order a `<recursive search order>`
/// names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOrder {
    /// DEPTH FIRST: the sequence column is the path of rows of the
    /// columns BY names, from the anchor's row \[SR5c i\].
    Depth,
    /// BREADTH FIRST: the sequence column is a row of the level, from 0,
    /// and the columns BY names \[SR5c ii\].
    Breadth,
}

/// [ISO/IEC 9075-2:2023 §7.18] `<cycle clause>` ::= CYCLE `<cycle column
/// list>` SET `<cycle mark column>` \[`<cycle mark option>`\] USING
/// `<path column>`.
#[derive(Debug, Clone)]
pub struct CycleClause {
    /// The `<cycle column list>`, in order.
    pub columns: Vec<String>,
    /// The `<cycle mark column>`.
    pub mark: String,
    /// The `<cycle mark option>` ::= TO `<cycle mark value>` DEFAULT
    /// `<non-cycle mark value>`, if written; else TO TRUE DEFAULT FALSE is
    /// implicit \[SR1\].
    pub marks: Option<(ValueExpression, ValueExpression)>,
    /// The `<path column>`.
    pub path: String,
    /// Where CYCLE is written.
    pub at: usize,
}

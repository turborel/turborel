//! \[EXTENSION\] GRAPH_TABLE, a `<table primary>` [ISO/IEC 9075-2:2023
//! §7.6] that matches a graph pattern of GPML [P1 §4, §5] in a property
//! graph, and the expressions of graph elements and paths its conditions
//! and columns may have [P1 §4.7].
//!
//! GRAPH_TABLE and every word of its Format are no key words of SQL-92,
//! so each is read as that word only where the Format has it, as
//! JSON_TABLE's are; a graph function's or predicate's name before "(" is
//! that function's only inside a GRAPH_TABLE, so a routine of the name is
//! still called elsewhere. Once read, the table's Syntax Rules are checked
//! (`graph/syntax.rs`); `graph/normalize.rs` gives the pattern's normal
//! form the binder and the path search compiler start from.

mod element;
mod expression;
mod normalize;
mod pattern;
mod syntax;

#[cfg(test)]
mod tests;

pub use normalize::normalize;
use syntax::check;
pub use syntax::{Declared, Degree, Exposed, Exposure, exposure};

use super::Parser;
use crate::ast::{
    ColumnSpecification, GraphColumn, GraphLimits, GraphReference, GraphRows, GraphTable, GraphVariable, SystemTimeKind, SystemTimeRead,
    TableName, TableReference, ValueExpression,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

// [P1 §4, §5; ISO/IEC 9075-2:2023 §7.6] The words of GRAPH_TABLE's
// Format that are no key words of SQL-92.
/// GRAPH_TABLE, the table's name.
const GRAPH_TABLE: &str = "GRAPH_TABLE";
/// VERTEX, of ONE ROW PER VERTEX and DIFFERENT VERTEX.
const VERTEX: &str = "VERTEX";
/// STEP, of ONE ROW PER STEP.
const STEP: &str = "STEP";
/// LINE_GRAPH, of a `<graph reference>`.
const LINE_GRAPH: &str = "LINE_GRAPH";
/// SCN, of AS OF SCN, which names a system change number.
const SCN: &str = "SCN";
/// \[EXTENSION\] [GS §20] The words of a GRAPH_TABLE's OPTIONS.
const OPTIONS: &str = "OPTIONS";
/// See `OPTIONS`.
const MAX_PATH_LENGTH: &str = "MAX_PATH_LENGTH";
/// See `OPTIONS`.
const MAX_PATHS_PER_PARTITION: &str = "MAX_PATHS_PER_PARTITION";
/// See `OPTIONS`.
const MAX_SEARCH_STATES: &str = "MAX_SEARCH_STATES";
/// See `OPTIONS`.
const LIMIT: &str = "LIMIT";
/// See `OPTIONS`.
const ERROR: &str = "ERROR";
/// See `OPTIONS`.
const TRUNCATE: &str = "TRUNCATE";
/// See `OPTIONS`.
const DETERMINISTIC_PATHS: &str = "DETERMINISTIC_PATHS";

impl Parser {
    /// Whether a GRAPH_TABLE begins here: GRAPH_TABLE before "(".
    pub(in crate::parser) fn at_graph_table(&self) -> bool {
        self.at_json(GRAPH_TABLE)
    }

    /// GRAPH_TABLE ( `<graph name>` MATCH `<graph pattern>` \[`<rows
    /// clause>`\] COLUMNS ( `<graph column>` \[, ...\] ) ) \[FOR
    /// SYSTEM_TIME ...\] \[\[AS\] `<correlation name>`\] \[AS OF TIMESTAMP
    /// `<value expression>`\], its Syntax Rules checked.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it does not conform to the Format; -103 if it
    /// breaks a Syntax Rule, its GRF condition named; -900 of AS OF SCN.
    pub(in crate::parser) fn graph_table(&mut self) -> Result<TableReference> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        self.graph = self.graph.saturating_add(1);
        let read = self.graph_table_body(at);
        self.graph = self.graph.saturating_sub(1);
        let mut table = read?;
        table.system_time = self.system_time_read()?;
        table.correlation = if self.is_kw(kw::AS) && self.next_is_kw(kw::OF) { None } else { self.correlation()? };
        if let Some(as_of) = self.as_of_timestamp()? {
            if table.system_time.is_some() {
                return Err(self.syntax("[EXTENSION] a GRAPH_TABLE is read at one point in time"));
            }
            table.system_time = Some(as_of);
        }
        check(&table)?;
        Ok(TableReference::Graph(Box::new(table)))
    }

    /// What GRAPH_TABLE's parentheses hold, and the right one.
    fn graph_table_body(&mut self, at: usize) -> Result<GraphTable> {
        let graph = self.graph_reference()?;
        self.expect_kw(kw::MATCH)?;
        let pattern = self.graph_pattern()?;
        let rows = self.graph_rows()?;
        self.expect_word(super::json::COLUMNS)?;
        let columns = self.graph_columns()?;
        let options = self.graph_limits()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(GraphTable { graph, pattern, rows, columns, correlation: None, system_time: None, options, at })
    }

    /// \[EXTENSION\] [GS §20] \[OPTIONS ( `<graph option>` \[, ...\] )\],
    /// each option at most once: MAX_PATH_LENGTH `n`,
    /// MAX_PATHS_PER_PARTITION `n`, MAX_SEARCH_STATES `n`, ON LIMIT
    /// {ERROR | TRUNCATE}, DETERMINISTIC_PATHS.
    fn graph_limits(&mut self) -> Result<GraphLimits> {
        let mut limits = GraphLimits::default();
        if !self.accept_word(OPTIONS) {
            return Ok(limits);
        }
        self.expect_delim(delim::LEFT_PAREN)?;
        loop {
            let at = self.here();
            let before = limits;
            if self.accept_word(MAX_PATH_LENGTH) && limits.max_path_length.is_none() {
                limits.max_path_length = Some(self.unsigned_integer()?);
            } else if self.accept_word(MAX_PATHS_PER_PARTITION) && limits.max_paths_per_partition.is_none() {
                limits.max_paths_per_partition = Some(self.unsigned_integer()?);
            } else if self.accept_word(MAX_SEARCH_STATES) && limits.max_search_states.is_none() {
                limits.max_search_states = Some(self.unsigned_integer()?);
            } else if self.accept_kw(kw::ON) && limits.truncate.is_none() {
                self.expect_word(LIMIT)?;
                let truncate = !self.accept_word(ERROR);
                if truncate {
                    self.expect_word(TRUNCATE)?;
                }
                limits.truncate = Some(truncate);
            } else if self.accept_word(DETERMINISTIC_PATHS) && !limits.deterministic {
                limits.deterministic = true;
            }
            if limits == before {
                return Err(SqlError::at(Code::Syntax, at, "[EXTENSION] expected a GRAPH_TABLE option not yet written (GS §20)"));
            }
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(limits)
    }

    /// `<graph reference>` ::= `<graph name>` | \[EXTENSION\] LINE_GRAPH (
    /// `<graph name>` , `<label>` ).
    fn graph_reference(&mut self) -> Result<GraphReference> {
        if !self.at_json(LINE_GRAPH) {
            return self.table_name().map(GraphReference::Named);
        }
        let at = self.here();
        self.advance();
        self.advance();
        let graph = self.table_name()?;
        self.expect_delim(delim::COMMA)?;
        let label = self.identifier()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(GraphReference::LineGraph { graph, label, at })
    }

    /// \[ONE ROW PER MATCH | ONE ROW PER VERTEX ( `<variable>` ) | ONE ROW
    /// PER STEP ( `<variable>` , `<variable>` , `<variable>` )\]; none
    /// written is ONE ROW PER MATCH.
    fn graph_rows(&mut self) -> Result<GraphRows> {
        if !self.accept_word(super::pattern::ONE) {
            return Ok(GraphRows::Match);
        }
        self.expect_word(super::window::ROW)?;
        self.expect_word(super::pattern::PER)?;
        if self.accept_kw(kw::MATCH) {
            return Ok(GraphRows::Match);
        }
        let vertex = self.accept_word(VERTEX);
        if !vertex {
            self.expect_word(STEP)?;
        }
        self.expect_delim(delim::LEFT_PAREN)?;
        let first = self.graph_variable()?;
        let rows = if vertex {
            GraphRows::Vertex(first)
        } else {
            self.expect_delim(delim::COMMA)?;
            let edge = self.graph_variable()?;
            self.expect_delim(delim::COMMA)?;
            GraphRows::Step(first, edge, self.graph_variable()?)
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(rows)
    }

    /// ( `<graph column>` \[, ...\] ).
    fn graph_columns(&mut self) -> Result<Vec<GraphColumn>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut columns = vec![self.graph_column()?];
        while self.accept_delim(delim::COMMA) {
            columns.push(self.graph_column()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(columns)
    }

    /// `<graph column>` ::= `<variable>`.* | `<value expression>` \[\[AS\]
    /// `<column name>`\].
    fn graph_column(&mut self) -> Result<GraphColumn> {
        let all = matches!(self.peek(), Tok::Identifier(_))
            && *self.peek_at(1) == Tok::Delimiter(delim::PERIOD)
            && *self.peek_at(2) == Tok::Delimiter(delim::ASTERISK);
        if all {
            let variable = self.graph_variable()?;
            self.advance();
            self.advance();
            return Ok(GraphColumn::All(variable));
        }
        let at = self.here();
        let value = self.value_expression()?;
        let named = self.accept_kw(kw::AS) || matches!(self.peek(), Tok::Identifier(_));
        let name = if named { Some(self.identifier()?) } else { None };
        Ok(GraphColumn::Value { value, name, at })
    }

    /// \[EXTENSION\] AS OF TIMESTAMP `<value expression>`, if written: the
    /// rows current then, as FOR SYSTEM_TIME AS OF reads them.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 of AS OF SCN: this implementation numbers no system
    /// changes a statement may name.
    fn as_of_timestamp(&mut self) -> Result<Option<Box<SystemTimeRead>>> {
        if !self.is_kw(kw::AS) || !self.next_is_kw(kw::OF) {
            return Ok(None);
        }
        let at = self.here();
        self.advance();
        self.advance();
        if self.is_word(SCN) {
            return Err(SqlError::at(
                Code::Unsupported,
                self.here(),
                "[EXTENSION] AS OF SCN is not supported: no system change number is kept",
            ));
        }
        self.expect_kw(kw::TIMESTAMP)?;
        let point = self.common_value_expression()?;
        Ok(Some(Box::new(SystemTimeRead { kind: SystemTimeKind::AsOf(point), at })))
    }

    /// A graph variable as written: an `<identifier>`.
    pub(in crate::parser) fn graph_variable(&mut self) -> Result<GraphVariable> {
        let at = self.here();
        Ok(GraphVariable { name: self.identifier()?, at, anonymous: false })
    }

    /// A property reference `<variable>` . `<property name>`, read as the
    /// `<column reference>` it is written as, which the binder resolves.
    pub(in crate::parser) fn property_reference(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        let qualifier = TableName { auth: None, ident: self.identifier()?, at };
        self.expect_delim(delim::PERIOD)?;
        let column = self.identifier()?;
        Ok(ValueExpression::Column(ColumnSpecification { qualifier: Some(qualifier), column, catalog: None, at }))
    }
}

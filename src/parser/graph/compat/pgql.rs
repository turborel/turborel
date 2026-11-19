//! \[EXTENSION\] [GS §10.3] A PGQL query, SELECT ... FROM MATCH ...,
//! read as a query of one GRAPH_TABLE:
//!
//! ```text
//! <pgql query> ::= [ <path macro>... ] SELECT [ DISTINCT | ALL ] <select list>
//!     FROM <pgql match> [ { , <pgql match> }... ]
//!     [ WHERE <search condition> ] [ <group by clause> ] [ <having clause> ]
//!     [ ORDER BY <sort specification list> ] [ LIMIT <row count> ] [ OFFSET <row count> ]
//! <pgql match> ::= MATCH <path pattern> [ { , <path pattern> }... ] [ ON <graph name> ]
//! ```
//!
//! The path patterns are GPML's, as PGQL 2 writes them, or hold PGQL's
//! reachabilities, of edges or of the path macros (`pgql/reach.rs`), and
//! together are the graph pattern; the WHERE clause is its postfilter [GS §6.8]. A
//! match names one graph, SYS_GRAPH of the default schema if it names
//! none. The select list, GROUP BY, HAVING and ORDER BY read the
//! variables through the GRAPH_TABLE's columns (`rewrite.rs`), so an
//! aggregate of a group variable is of each path, and any other of the
//! query's rows.

mod reach;

use super::{Frame, Rewrite};
use crate::ast::{
    GraphPattern, GraphReference, QueryExpression, QuerySpecification, RowCount, SearchCondition, SelectList, SortSpecification,
    TableExpression, TableName, TableReference,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};
use crate::parser::{Parser, exposure};

/// The correlation name of a PGQL query's GRAPH_TABLE.
const PGQL_TABLE: &str = "$PGQL";
/// LIMIT, of a row count.
const LIMIT: &str = "LIMIT";
/// OFFSET, of a row count.
const OFFSET: &str = "OFFSET";

/// What a PGQL query's frame reads after its pattern.
struct Tail {
    /// The postfilter, if written.
    where_clause: Option<SearchCondition>,
    /// DISTINCT was written.
    distinct: bool,
    /// The select list.
    select_list: SelectList,
    /// GROUP BY, HAVING and the window clause, in a table expression of
    /// no FROM.
    table_expression: TableExpression,
    /// ORDER BY's sort specifications.
    order_by: Vec<SortSpecification>,
    /// The row counts of LIMIT and OFFSET.
    rows: super::Rows,
}

impl Parser {
    /// The offset of FROM, if a PGQL query begins here: its path macros
    /// if any, SELECT, and FROM MATCH outside any parentheses after it.
    pub(in crate::parser) fn pgql_from(&self) -> Option<usize> {
        let select = self.macros_length();
        if *self.peek_at(select) != Tok::KeyWord(kw::SELECT) {
            return None;
        }
        let mut depth = 0_usize;
        let mut n = select.saturating_add(1);
        loop {
            let t = self.peek_at(n);
            if *t == Tok::End || (depth == 0 && *t == Tok::Delimiter(delim::RIGHT_PAREN)) {
                return None;
            }
            if depth == 0 && *t == Tok::KeyWord(kw::FROM) {
                return (*self.peek_at(n.saturating_add(1)) == Tok::KeyWord(kw::MATCH)).then_some(n);
            }
            if *t == Tok::Delimiter(delim::LEFT_PAREN) {
                depth = depth.saturating_add(1);
            } else if *t == Tok::Delimiter(delim::RIGHT_PAREN) {
                depth = depth.saturating_sub(1);
            }
            n = n.saturating_add(1);
        }
    }

    /// A PGQL query whose FROM is at offset `from`, as a query
    /// specification: no ORDER BY, LIMIT or OFFSET of its own.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it does not conform to the Format; -103 if its
    /// graph pattern breaks a Syntax Rule.
    pub(in crate::parser) fn pgql_specification(&mut self, from: usize) -> Result<QuerySpecification> {
        Ok(self.pgql(from, false)?.0)
    }

    /// A PGQL query whose FROM is at offset `from`, with its ORDER BY,
    /// LIMIT and OFFSET.
    ///
    /// # Errors
    ///
    /// As `pgql_specification`.
    pub(in crate::parser) fn pgql_query(&mut self, from: usize) -> Result<QueryExpression> {
        let (spec, ordering) = self.pgql(from, true)?;
        Ok(super::ordered_query(spec, ordering))
    }

    /// The PGQL query whose FROM is at offset `from`, and if `ordered`
    /// its ORDER BY, LIMIT and OFFSET.
    fn pgql(&mut self, from: usize, ordered: bool) -> Result<(QuerySpecification, super::Ordering)> {
        let (start, at) = (self.pos, self.here());
        let macros = self.path_macros()?;
        let select = self.pos;
        self.pos = start.saturating_add(from).saturating_add(1);
        let (graph, paths) = self.pgql_matches(&macros)?;
        let mut pattern = GraphPattern { mode: None, paths, keep: None, where_clause: None, segments: Vec::new(), at };
        let rewrite = Rewrite::of(exposure(&pattern)?, PGQL_TABLE);
        self.compat.push(Frame { rewrite: Some(rewrite), ..Frame::default() });
        let read = self.pgql_tail(select, start.saturating_add(from), ordered);
        let frame = self.compat.pop().unwrap_or_default();
        let tail = read?;
        pattern.where_clause = tail.where_clause;
        let columns = frame.rewrite.map(Rewrite::into_columns).unwrap_or_default().into_iter().next().unwrap_or_default();
        let table = super::graph_table(GraphReference::Named(graph), pattern, columns, PGQL_TABLE, at)?;
        let mut table_expression = tail.table_expression;
        table_expression.from = vec![TableReference::Graph(Box::new(table))];
        let spec = QuerySpecification { distinct: tail.distinct, select_list: tail.select_list, table_expression };
        Ok((spec, (tail.order_by, tail.rows)))
    }

    /// What a PGQL query reads in its frame, from just after its matches:
    /// the postfilter; then from `start` the select list up to FROM at
    /// `from`; then the rest.
    fn pgql_tail(&mut self, start: usize, from: usize, ordered: bool) -> Result<Tail> {
        let where_clause = if self.accept_kw(kw::WHERE) { Some(self.in_pattern(Parser::search_condition)?) } else { None };
        let rest = self.pos;
        self.pos = start;
        self.expect_kw(kw::SELECT)?;
        let distinct = self.accept_kw(kw::DISTINCT);
        if !distinct {
            self.accept_kw(kw::ALL);
        }
        let select_list = self.select_list()?;
        if self.pos != from {
            return Err(self.syntax("[GS §10.3] expected FROM MATCH"));
        }
        self.pos = rest;
        let group_by = if self.accept_kw(kw::GROUP) {
            self.expect_kw(kw::BY)?;
            Some(self.group_by()?)
        } else {
            None
        };
        let having = if self.accept_kw(kw::HAVING) { Some(self.search_condition()?) } else { None };
        let window = self.window_clause()?;
        let table_expression = TableExpression { from: Vec::new(), where_clause: None, group_by, having, window };
        let (order_by, rows) = if ordered { self.pgql_order()? } else { (Vec::new(), (None, None)) };
        Ok(Tail { where_clause, distinct, select_list, table_expression, order_by, rows })
    }

    /// ORDER BY, LIMIT and OFFSET, each if written.
    fn pgql_order(&mut self) -> Result<super::Ordering> {
        let mut order_by = Vec::new();
        if self.accept_kw(kw::ORDER) {
            self.expect_kw(kw::BY)?;
            order_by.push(self.sort_specification()?);
            while self.accept_delim(delim::COMMA) {
                order_by.push(self.sort_specification()?);
            }
        }
        Ok((order_by, (self.row_count_after(LIMIT)?, self.row_count_after(OFFSET)?)))
    }

    /// The row count after `word`, if `word` is next.
    pub(in crate::parser) fn row_count_after(&mut self, word: &str) -> Result<Option<RowCount>> {
        if !self.accept_word(word) {
            return Ok(None);
        }
        let at = self.here();
        Ok(Some(RowCount { value: self.number_of_rows()?, at }))
    }

    /// `read`, inside the pattern: where its variables are its own, and
    /// GPML's functions and predicates are read.
    pub(in crate::parser) fn in_pattern<T>(&mut self, read: fn(&mut Parser) -> Result<T>) -> Result<T> {
        self.graph = self.graph.saturating_add(1);
        let r = read(self);
        self.graph = self.graph.saturating_sub(1);
        r
    }

    /// The `<pgql match>`es: the graph they name, and their path patterns.
    fn pgql_matches(&mut self, macros: &[reach::Macro]) -> Result<(TableName, Vec<crate::ast::PathPattern>)> {
        let (mut graph, mut paths) = (None::<TableName>, Vec::new());
        loop {
            self.expect_kw(kw::MATCH)?;
            paths.extend(self.pgql_path(macros)?);
            while *self.peek() == Tok::Delimiter(delim::COMMA) && *self.peek_at(1) != Tok::KeyWord(kw::MATCH) {
                self.advance();
                paths.extend(self.pgql_path(macros)?);
            }
            if self.accept_kw(kw::ON) {
                let named = self.table_name()?;
                if graph.as_ref().is_some_and(|g| g.ident != named.ident || g.auth != named.auth) {
                    return Err(SqlError::at(Code::SyntaxRule, named.at, "[GS §10.3] the matches of a PGQL query name one graph"));
                }
                graph = Some(named);
            }
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        let at = self.here();
        Ok((graph.unwrap_or_else(|| super::default_graph(at)), paths))
    }
}

//! \[EXTENSION\] [GS §10.3] An openCypher MATCH ... RETURN query, read as
//! a query of one GRAPH_TABLE:
//!
//! ```text
//! <cypher query> ::= [ USE <graph name> ]
//!     MATCH <cypher pattern> [ { , <cypher pattern> }... ] [ WHERE <search condition> ]
//!     RETURN [ DISTINCT ] <return item> [ { , <return item> }... ]
//!     [ ORDER BY <sort specification list> ] [ SKIP <row count> ] [ LIMIT <row count> ]
//! <return item> ::= <value expression> [ AS <column name> ]
//! ```
//!
//! The patterns (`cypher/pattern.rs`) are the graph pattern, matched
//! DIFFERENT EDGES as Cypher matches no relationship twice, and the WHERE
//! clause is its postfilter [GS §6.8]. A query names one graph,
//! SYS_GRAPH of the default schema if it names none. The return items
//! and ORDER BY read the variables through the GRAPH_TABLE's columns
//! (`rewrite.rs`); a return item with an aggregate makes the others the
//! grouping keys, by the columns they read. Expressions are SQL's, a
//! string in single quotes.

mod pattern;

use super::{Frame, Rewrite};
use crate::ast::{
    ColumnSpecification, GraphPattern, GraphReference, GroupBy, GroupingColumn, GroupingElement, MatchMode, QueryExpression,
    QuerySpecification, SearchCondition, SelectItem, SelectList, SelectSublist, SortSpecification, TableExpression, TableReference,
    ValueExpression,
};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::parser::{Parser, exposure};

/// The correlation name of a Cypher query's GRAPH_TABLE.
const CYPHER_TABLE: &str = "$CYPHER";
/// USE, of the graph a query reads.
const USE: &str = "USE";
/// RETURN, of the items a query gives.
const RETURN: &str = "RETURN";
/// SKIP, of a row count.
const SKIP: &str = "SKIP";
/// LIMIT, of a row count.
const LIMIT: &str = "LIMIT";

/// What a Cypher query's frame reads after its pattern.
struct Returned {
    /// The postfilter, if written.
    where_clause: Option<SearchCondition>,
    /// DISTINCT was written.
    distinct: bool,
    /// The return items.
    items: Vec<SelectItem>,
    /// ORDER BY's sort specifications.
    order_by: Vec<SortSpecification>,
    /// The row counts of LIMIT and SKIP.
    rows: super::Rows,
}

impl Parser {
    /// Whether a Cypher query begins here: MATCH, or USE, a graph's name
    /// and MATCH. No other query begins so.
    pub(in crate::parser) fn at_cypher(&self) -> bool {
        let matched = |n: usize| *self.peek_at(n) == Tok::KeyWord(kw::MATCH);
        matched(0) || (self.is_word(USE) && matches!(self.peek_at(1), Tok::Identifier(_)) && (matched(2) || matched(4)))
    }

    /// `<cypher query>`.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it does not conform to the Format; -103 if its
    /// graph pattern breaks a Syntax Rule.
    pub(in crate::parser) fn cypher_query(&mut self) -> Result<QueryExpression> {
        let at = self.here();
        let graph = if self.accept_word(USE) { self.table_name()? } else { super::default_graph(at) };
        self.expect_kw(kw::MATCH)?;
        let mut paths = vec![self.in_pattern(Parser::cypher_pattern)?];
        while self.accept_delim(delim::COMMA) {
            paths.push(self.in_pattern(Parser::cypher_pattern)?);
        }
        let mut pattern =
            GraphPattern { mode: Some(MatchMode::DifferentEdges), paths, keep: None, where_clause: None, segments: Vec::new(), at };
        let rewrite = Rewrite { vars: exposure(&pattern)?, columns: Vec::new(), table: CYPHER_TABLE };
        self.compat.push(Frame { rewrite: Some(rewrite), ..Frame::default() });
        let read = self.cypher_rest();
        let frame = self.compat.pop().unwrap_or_default();
        let returned = read?;
        pattern.where_clause = returned.where_clause;
        let columns = frame.rewrite.map(|r| r.columns).unwrap_or_default();
        let table = super::graph_table(GraphReference::Named(graph), pattern, columns, CYPHER_TABLE, at)?;
        let group_by = grouping(&returned.items);
        let from = vec![TableReference::Graph(Box::new(table))];
        let table_expression = TableExpression { from, where_clause: None, group_by, having: None, window: Vec::new() };
        let select_list = SelectList::Values(returned.items.into_iter().map(SelectSublist::Derived).collect());
        let spec = QuerySpecification { distinct: returned.distinct, select_list, table_expression };
        Ok(super::ordered_query(spec, (returned.order_by, returned.rows)))
    }

    /// What follows a Cypher query's patterns, read in its frame: the
    /// postfilter, the return items and ORDER BY, and the row counts of
    /// LIMIT and SKIP.
    fn cypher_rest(&mut self) -> Result<Returned> {
        let where_clause = if self.accept_kw(kw::WHERE) { Some(self.in_pattern(Parser::search_condition)?) } else { None };
        self.expect_word(RETURN)?;
        let distinct = self.accept_kw(kw::DISTINCT);
        let mut items = vec![self.return_item()?];
        while self.accept_delim(delim::COMMA) {
            items.push(self.return_item()?);
        }
        let mut order_by = Vec::new();
        if self.accept_kw(kw::ORDER) {
            self.expect_kw(kw::BY)?;
            order_by.push(self.sort_specification()?);
            while self.accept_delim(delim::COMMA) {
                order_by.push(self.sort_specification()?);
            }
        }
        let offset = self.row_count_after(SKIP)?;
        let fetch = self.row_count_after(LIMIT)?;
        Ok(Returned { where_clause, distinct, items, order_by, rows: (fetch, offset) })
    }

    /// `<return item>`.
    fn return_item(&mut self) -> Result<SelectItem> {
        let at = self.here();
        let value = self.value_expression()?;
        let alias = if self.accept_kw(kw::AS) { Some(self.identifier()?) } else { None };
        Ok(SelectItem { value, alias, at })
    }
}

/// [GS §10.3] The grouping of return items `items`: none if none has an
/// aggregate, else by the match's columns the others read.
fn grouping(items: &[SelectItem]) -> Option<GroupBy> {
    let aggregated = |v: &ValueExpression| v.any(&|x| matches!(x, ValueExpression::SetFunction(_)));
    if !items.iter().any(|i| aggregated(&i.value)) {
        return None;
    }
    let mut columns: Vec<ColumnSpecification> = Vec::new();
    for item in items.iter().filter(|i| !aggregated(&i.value)) {
        read_columns(&item.value, &mut columns);
    }
    if columns.is_empty() {
        return None;
    }
    let keys = columns.into_iter().map(|column| GroupingColumn { column, collate: None }).collect();
    Some(GroupBy { distinct: false, elements: vec![GroupingElement::Ordinary(keys)] })
}

/// Adds to `out` the columns of the match `v` reads, each once.
fn read_columns(v: &ValueExpression, out: &mut Vec<ColumnSpecification>) {
    if let ValueExpression::Column(c) = v
        && !out.iter().any(|o| o.column == c.column)
    {
        out.push(c.clone());
    }
    v.for_each_child(&mut |child| read_columns(child, out));
}

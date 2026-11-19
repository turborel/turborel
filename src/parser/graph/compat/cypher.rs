//! \[EXTENSION\] [GS §10.3] An openCypher query of MATCH clauses and
//! RETURN, read as a query of the GRAPH_TABLEs of its clauses:
//!
//! ```text
//! <cypher query> ::= [ USE <graph name> ] <cypher match>...
//!     RETURN [ DISTINCT ] <return item> [ { , <return item> }... ]
//!     [ ORDER BY <sort specification list> ] [ SKIP <row count> ] [ LIMIT <row count> ]
//! <cypher match> ::= [ OPTIONAL ] MATCH <cypher pattern> [ { , <cypher pattern> }... ]
//!     [ WHERE <search condition> ]
//! <return item> ::= <value expression> [ AS <column name> ]
//! ```
//!
//! Each clause is a GRAPH_TABLE (`cypher/clauses.rs`), joined to those
//! before it on the variables they share. Its patterns
//! (`cypher/pattern.rs`) are matched DIFFERENT EDGES, as Cypher matches
//! no relationship twice in a clause. A query names one graph, SYS_GRAPH
//! of the default schema if it names none. The return items and ORDER BY
//! read the variables through the GRAPH_TABLEs' columns (`rewrite.rs`);
//! a return item with an aggregate makes the others the grouping keys,
//! by the columns they read. Expressions are SQL's, a string in single
//! quotes.

mod clauses;
mod pattern;

use super::{Frame, Rewrite};
use crate::ast::{
    ColumnSpecification, GroupBy, GroupingColumn, GroupingElement, QueryExpression, QuerySpecification, SelectItem, SelectList,
    SelectSublist, SortSpecification, TableExpression, ValueExpression,
};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;

/// The correlation name of a Cypher query's first GRAPH_TABLE; the
/// others' end in their number.
const CYPHER_TABLE: &str = "$CYPHER";
/// USE, of the graph a query reads.
const USE: &str = "USE";
/// OPTIONAL, of a MATCH clause whose matches may be none.
const OPTIONAL: &str = "OPTIONAL";
/// RETURN, of the items a query gives.
const RETURN: &str = "RETURN";
/// SKIP, of a row count.
const SKIP: &str = "SKIP";
/// LIMIT, of a row count.
const LIMIT: &str = "LIMIT";

/// What a Cypher query's frame reads after its clauses.
struct Returned {
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
    /// Whether a Cypher query begins here: MATCH or OPTIONAL MATCH,
    /// either after USE and a graph's name or not. No other query begins
    /// so.
    pub(in crate::parser) fn at_cypher(&self) -> bool {
        let matched = |n: usize| *self.peek_at(n) == Tok::KeyWord(kw::MATCH) || self.at_optional(n);
        matched(0) || (self.is_word(USE) && matches!(self.peek_at(1), Tok::Identifier(_)) && (matched(2) || matched(4)))
    }

    /// Whether OPTIONAL MATCH is at offset `n`.
    fn at_optional(&self, n: usize) -> bool {
        matches!(self.peek_at(n), Tok::Identifier(w) if w == OPTIONAL) && *self.peek_at(n.saturating_add(1)) == Tok::KeyWord(kw::MATCH)
    }

    /// `<cypher query>`.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it does not conform to the Format; -103 if a graph
    /// pattern breaks a Syntax Rule, or a later clause binds again a path
    /// or a relationship of variable length.
    pub(in crate::parser) fn cypher_query(&mut self) -> Result<QueryExpression> {
        let at = self.here();
        let graph = if self.accept_word(USE) { self.table_name()? } else { super::default_graph(at) };
        self.compat.push(Frame { rewrite: Some(Rewrite::default()), ..Frame::default() });
        let read = self.cypher_body();
        let frame = self.compat.pop().unwrap_or_default();
        let (clauses, returned) = read?;
        let columns = frame.rewrite.map(Rewrite::into_columns).unwrap_or_default();
        let from = vec![clauses::joined(&graph, clauses, columns, at)?];
        let group_by = grouping(&returned.items);
        let table_expression = TableExpression { from, where_clause: None, group_by, having: None, window: Vec::new() };
        let select_list = SelectList::Values(returned.items.into_iter().map(SelectSublist::Derived).collect());
        let spec = QuerySpecification { distinct: returned.distinct, select_list, table_expression };
        Ok(super::ordered_query(spec, (returned.order_by, returned.rows)))
    }

    /// A Cypher query's clauses and what follows them, read in its frame.
    fn cypher_body(&mut self) -> Result<(Vec<clauses::Clause>, Returned)> {
        let mut read = vec![self.cypher_match(0)?];
        while self.is_kw(kw::MATCH) || self.at_optional(0) {
            read.push(self.cypher_match(read.len())?);
        }
        Ok((read, self.cypher_rest()?))
    }

    /// What follows a Cypher query's clauses: the return items and ORDER
    /// BY, and the row counts of LIMIT and SKIP.
    fn cypher_rest(&mut self) -> Result<Returned> {
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
        Ok(Returned { distinct, items, order_by, rows: (fetch, offset) })
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
/// aggregate, else by the matches' columns the others read.
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

/// Adds to `out` the columns of the matches `v` reads, each once.
fn read_columns(v: &ValueExpression, out: &mut Vec<ColumnSpecification>) {
    let qualifier = |c: &ColumnSpecification| c.qualifier.as_ref().map(|q| q.ident.clone());
    if let ValueExpression::Column(c) = v
        && !out.iter().any(|o| o.column == c.column && qualifier(o) == qualifier(c))
    {
        out.push(c.clone());
    }
    v.for_each_child(&mut |child| read_columns(child, out));
}

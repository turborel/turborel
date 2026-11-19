//! \[EXTENSION\] [GS §10] The compatibility syntax, each front end
//! compiling to the one GPML tree GRAPH_TABLE reads [GS §14], bound by
//! GRAPH_TABLE's binder and run by its operator, never a second one
//! (RULES.md D146):
//!
//! * MATCH in a WHERE clause over the tables of the FROM clause, with
//!   SHORTEST_PATH, LAST_NODE, FOR PATH tables and WITHIN GROUP (GRAPH
//!   PATH) aggregates (`compat/server.rs`, `compat/measures.rs`)
//!   [GS §10.1], which the binder turns into a GRAPH_TABLE once it knows
//!   the tables' graph (`binder/query/graph_match.rs`);
//! * a GRAPH_TABLE's hints (`compat/hints.rs`) [GS §10.2, R16.8];
//! * a PGQL query, SELECT ... FROM MATCH ... (`compat/pgql.rs`), and an
//!   openCypher MATCH ... RETURN query (`compat/cypher.rs`) [GS §10.3],
//!   each read as a query of GRAPH_TABLEs, its expressions reading the
//!   pattern's variables through the match's columns
//!   (`compat/rewrite.rs`).
//!
//! Each query specification is read in a frame of its own, which gathers
//! what its MATCH, or its front end, needs from the rest of it.

mod cypher;
mod hints;
mod measures;
mod pgql;
mod rewrite;
mod server;

#[cfg(test)]
mod tests;

use super::super::Parser;
use crate::ast::{
    FetchFirst, FetchQuantity, GraphColumn, GraphMeasure, GraphPattern, GraphReference, GraphRows, GraphTable, Literal, MatchClause,
    OrderedQuery, PathTable, Predicate, QueryExpression, QuerySpecification, RowCount, SearchCondition, SortSpecification, TableName,
    ValueExpression, ValueSpecification,
};
use crate::error::{Code, Result, SqlError};
use crate::parser::Exposure;
use crate::value::Decimal;
pub(in crate::parser) use hints::merge_hints;
pub use server::LAST_NODE_PREFIX;

/// The row counts of LIMIT and OFFSET, each if written.
type Rows = (Option<RowCount>, Option<RowCount>);
/// ORDER BY's sort specifications, and the row counts of LIMIT and
/// OFFSET.
type Ordering = (Vec<SortSpecification>, Rows);

/// SYS_GRAPH, the graph a PGQL or Cypher query of no graph reads: of the
/// default schema [GS §4.2].
const SYS_GRAPH: &str = "SYS_GRAPH";

/// The correlation name of the GRAPH_TABLE a MATCH in a WHERE clause
/// becomes, which its WITHIN GROUP (GRAPH PATH) aggregates are columns of.
pub const MATCH_TABLE: &str = "$MATCH";

/// \[EXTENSION\] [GS §10] A query specification being read, as the
/// compatibility front ends see it.
#[derive(Debug, Default)]
pub(in crate::parser) struct Frame {
    /// [GS §10.1] Its FOR PATH tables.
    paths: Vec<PathTable>,
    /// [GS §10.1] Its WITHIN GROUP (GRAPH PATH) aggregates.
    measures: Vec<GraphMeasure>,
    /// [GS §10.3] Of a PGQL or Cypher query, how its expressions read the
    /// pattern's variables.
    rewrite: Option<Rewrite>,
}

/// [GS §10.3] How the expressions of a PGQL or Cypher query read the
/// variables of its patterns: each property, function of a variable and
/// aggregate of a group variable a column of a GRAPH_TABLE, read by
/// name. A Cypher query of several MATCH clauses has a GRAPH_TABLE each,
/// and a variable is read through the first that binds it.
#[derive(Debug, Default)]
struct Rewrite {
    /// The GRAPH_TABLEs, in the order of their patterns.
    tables: Vec<Bound>,
}

/// [GS §10.3] One GRAPH_TABLE of a PGQL or Cypher query.
#[derive(Debug)]
struct Bound {
    /// Its pattern's variables, of each its kind and degree.
    vars: Exposure,
    /// Its columns so far, each with the text it is of, so one written
    /// twice is one column.
    columns: Vec<(String, GraphColumn)>,
    /// Its correlation name.
    table: String,
}

impl Rewrite {
    /// The rewrite of one GRAPH_TABLE, named `table`, of a pattern of
    /// variables `vars`.
    fn of(vars: Exposure, table: &str) -> Rewrite {
        Rewrite { tables: vec![Bound { vars, columns: Vec::new(), table: table.to_owned() }] }
    }

    /// The columns of each GRAPH_TABLE, in order.
    fn into_columns(self) -> Vec<Vec<(String, GraphColumn)>> {
        self.tables.into_iter().map(|b| b.columns).collect()
    }
}

impl Parser {
    /// [X3.135-1986 §5.25] A `<query specification>`, `read` in a frame
    /// of its own, its FOR PATH tables and GRAPH PATH aggregates given to
    /// its MATCH; \[EXTENSION\] or a PGQL one [GS §10.3].
    ///
    /// # Errors
    ///
    /// As `read`; SQLCODE -103 if FOR PATH or GRAPH PATH is written with
    /// no MATCH a conjunct of the WHERE clause.
    pub(in crate::parser) fn framed_specification(
        &mut self,
        read: fn(&mut Parser) -> Result<QuerySpecification>,
    ) -> Result<QuerySpecification> {
        if let Some(from) = self.pgql_from() {
            return self.pgql_specification(from);
        }
        self.compat.push(Frame::default());
        let read = read(self);
        let frame = self.compat.pop().unwrap_or_default();
        let mut spec = read?;
        attach(frame, &mut spec)?;
        Ok(spec)
    }

    /// The frame of the query specification being read.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 outside one: `what` is of a query specification's MATCH.
    fn compat_frame(&mut self, what: &str, at: usize) -> Result<&mut Frame> {
        let msg = format!("[GS §10.1] {what} is of a query specification with a MATCH");
        self.compat.last_mut().ok_or_else(|| SqlError::at(Code::SyntaxRule, at, msg))
    }
}

/// [GS §10.1] Gives the FOR PATH tables and GRAPH PATH aggregates of
/// `frame` to the first MATCH of `spec` that is a conjunct of its WHERE
/// clause.
///
/// # Errors
///
/// SQLCODE -103 if it has some and `spec` has no such MATCH.
fn attach(frame: Frame, spec: &mut QuerySpecification) -> Result<()> {
    let Some(at) = frame.paths.first().map(|p| p.table.at).or_else(|| frame.measures.first().map(|m| m.at)) else { return Ok(()) };
    let clause = spec.table_expression.where_clause.as_mut().and_then(conjunct_match);
    let Some(clause) = clause else {
        let msg = "[GS §10.1] FOR PATH and WITHIN GROUP (GRAPH PATH) need a MATCH that is a conjunct of the WHERE clause";
        return Err(SqlError::at(Code::SyntaxRule, at, msg));
    };
    clause.path_tables = frame.paths;
    clause.measures = frame.measures;
    Ok(())
}

/// The first MATCH of `c` that is a conjunct of it.
fn conjunct_match(c: &mut SearchCondition) -> Option<&mut MatchClause> {
    match c {
        SearchCondition::And(a, b) => {
            if conjunct_match(a).is_some() {
                return conjunct_match(a);
            }
            conjunct_match(b)
        }
        SearchCondition::Predicate(p, _) => match &mut **p {
            Predicate::GraphMatch(m) => Some(m),
            Predicate::Comparison(..)
            | Predicate::Overlaps(..)
            | Predicate::Period(..)
            | Predicate::Between { .. }
            | Predicate::InList { .. }
            | Predicate::InSubquery { .. }
            | Predicate::Like { .. }
            | Predicate::LikeRegex { .. }
            | Predicate::JsonExists(_)
            | Predicate::IsJson(_)
            | Predicate::Null { .. }
            | Predicate::Distinct { .. }
            | Predicate::Quantified { .. }
            | Predicate::Exists(_)
            | Predicate::Unique { .. }
            | Predicate::Match { .. }
            | Predicate::Member { .. }
            | Predicate::Submultiset { .. }
            | Predicate::Set { .. }
            | Predicate::Graph(_)
            | Predicate::Xml { .. }
            | Predicate::XmlValid { .. }
            | Predicate::Type { .. } => None,
        },
        SearchCondition::Or(..) | SearchCondition::Not(_) | SearchCondition::Is(..) | SearchCondition::Predicand(..) => None,
    }
}

/// [GS §10.3] SYS_GRAPH of the default schema, written at `at`.
fn default_graph(at: usize) -> TableName {
    TableName { auth: None, ident: SYS_GRAPH.to_owned(), at }
}

/// [GS §10.3] The GRAPH_TABLE of a PGQL or Cypher query: `pattern` in
/// `graph`, one row per match of `columns`, named `table`, its Syntax
/// Rules checked.
///
/// # Errors
///
/// SQLCODE -103 if its pattern breaks a Syntax Rule.
fn graph_table(
    graph: GraphReference,
    pattern: GraphPattern,
    columns: Vec<(String, GraphColumn)>,
    table: &str,
    at: usize,
) -> Result<GraphTable> {
    let mut columns: Vec<GraphColumn> = columns.into_iter().map(|(_, c)| c).collect();
    if columns.is_empty() {
        // COLUMNS has one at least: of a query that reads no variable, a
        // constant.
        let one = ValueExpression::Value(ValueSpecification::Literal(Literal::Exact(Decimal::new(1, 0))));
        columns.push(GraphColumn::Value { value: one, name: Some("$1".to_owned()), at });
    }
    let t = GraphTable {
        graph,
        pattern,
        rows: GraphRows::Match,
        columns,
        correlation: Some(table.to_owned()),
        system_time: None,
        options: crate::ast::GraphLimits::default(),
        hints: crate::ast::GraphHints::default(),
        at,
    };
    super::syntax::check(&t)?;
    Ok(t)
}

/// `spec`, ordered and counted as `ordering` says.
fn ordered_query(spec: QuerySpecification, (order_by, (fetch, offset)): Ordering) -> QueryExpression {
    let body = QueryExpression::Spec(Box::new(spec));
    if order_by.is_empty() && fetch.is_none() && offset.is_none() {
        return body;
    }
    let fetch = fetch.map(|count| FetchFirst {
        quantity: Some(FetchQuantity::Rows(count.clone())),
        ties: false,
        at: count.at,
        search: crate::logical::Search::default(),
        accuracy_at: None,
        hints: crate::logical::Hints::default(),
    });
    QueryExpression::Ordered(Box::new(OrderedQuery { body, order_by, offset, fetch }))
}

/// \[EXTENSION\] [GS §10.1] Key column `index` of the element that
/// variable `var` binds, of type `t`, as a GRAPH_TABLE's column reads it:
/// JSON_VALUE of its ELEMENT_ID, whose `key` holds the key's values in
/// order [GS R5.1]; of a node or edge table's element, if `native`, its
/// ELEMENT_ID, which is its key.
///
/// # Errors
///
/// SQLCODE -102 if `t` is no type JSON_VALUE returns.
pub fn element_key(var: &str, (index, native): (usize, bool), t: &crate::value::DataType) -> Result<ValueExpression> {
    let id = format!("ELEMENT_ID({})", crate::catalog::identifier(var));
    let text = if native { id } else { format!("JSON_VALUE({id}, 'lax $.key[{index}]' RETURNING {t})") };
    let mut p = Parser::new(&text)?;
    p.graph = 1;
    p.value_expression()
}

/// \[EXTENSION\] [GS §10.1] The Syntax Rules of GRAPH_TABLE `table` made
/// of a MATCH, checked as the parser checks one written.
///
/// # Errors
///
/// SQLCODE -103 if one is broken, its GRF condition named.
pub fn checked_graph_table(table: &GraphTable) -> Result<()> {
    super::syntax::check(table)
}

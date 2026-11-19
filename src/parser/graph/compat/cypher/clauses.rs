//! \[EXTENSION\] [GS §10.3] The MATCH clauses of a Cypher query, each a
//! GRAPH_TABLE, joined in the order written:
//!
//! * a variable a clause shares with one before it is the same element:
//!   the clause is joined to the first that binds it where their
//!   ELEMENT_IDs are equal; a path variable, or a relationship's of
//!   variable length, is bound once;
//! * a clause of OPTIONAL MATCH is a left join, so a row of the clauses
//!   before it that it matches nowhere stays, its variables null; a
//!   first clause is joined to a table of one row so;
//! * a clause's WHERE is its pattern's postfilter if it reads only its
//!   own variables, else a condition of its join, reading the variables
//!   through the GRAPH_TABLEs' columns;
//! * a clause of no such condition is joined on TRUE, to each row before
//!   it.

use super::super::{Bound, Rewrite};
use super::CYPHER_TABLE;
use crate::ast::{
    GraphColumn, GraphFunction, GraphFunctionName, GraphPattern, GraphReference, GraphVariable, JoinKind, JoinSpecification, JoinedTable,
    Literal, MatchMode, Predicate, SearchCondition, TableName, TableReference, ValueExpression, ValueSpecification,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};
use crate::parser::{Declared, Degree, Exposed, Exposure, Parser, exposure};
use crate::value::CompOp;

/// The table of one row a first OPTIONAL MATCH is joined to.
const ONE_ROW: &str = "(VALUES (0)) AS \"$ONE\" (\"$ONE\")";

/// One `<cypher match>`, read.
pub(super) struct Clause {
    /// Its graph pattern, its postfilter if it has one.
    pattern: GraphPattern,
    /// OPTIONAL was written.
    optional: bool,
    /// The condition of its join to the clauses before it, if any.
    on: Option<SearchCondition>,
    /// Where it begins.
    at: usize,
}

impl Parser {
    /// `<cypher match>`, the clause numbered `index` from 0, read in its
    /// query's frame: the GRAPH_TABLE of its pattern added to the frame's
    /// rewrite.
    ///
    /// # Errors
    ///
    /// As `cypher_query`.
    pub(super) fn cypher_match(&mut self, index: usize) -> Result<Clause> {
        let at = self.here();
        let optional = self.accept_word(super::OPTIONAL);
        self.expect_kw(kw::MATCH)?;
        let mut paths = vec![self.in_pattern(Parser::cypher_pattern)?];
        while self.accept_delim(delim::COMMA) {
            paths.push(self.in_pattern(Parser::cypher_pattern)?);
        }
        let mut pattern =
            GraphPattern { mode: Some(MatchMode::DifferentEdges), paths, keep: None, where_clause: None, segments: Vec::new(), at };
        let vars = exposure(&pattern)?;
        let table = table_name(index);
        let rewrite = self.rewrite()?;
        rewrite.tables.push(Bound { vars: vars.clone(), columns: Vec::new(), table });
        let mut on = shared(rewrite, &vars, at)?;
        if self.accept_kw(kw::WHERE) {
            let (inner, outer) = self.clause_where(&vars)?;
            pattern.where_clause = inner;
            on = conjunction(on, outer);
        }
        Ok(Clause { pattern, optional, on, at })
    }

    /// The WHERE clause of a clause of variables `own`: its postfilter if
    /// it reads no variable of a clause before, else the condition of its
    /// join.
    fn clause_where(&mut self, own: &Exposure) -> Result<(Option<SearchCondition>, Option<SearchCondition>)> {
        let start = self.pos;
        let inner = self.in_pattern(Parser::search_condition)?;
        let rewrite = self.rewrite()?;
        let earlier: Vec<String> = rewrite.tables.iter().rev().skip(1).flat_map(|b| b.vars.keys().cloned()).collect();
        let reads_earlier = (start..self.pos).any(|n| {
            let named = matches!(self.toks.get(n).map(|t| &t.tok), Some(Tok::Identifier(v)) if earlier.contains(v) && !own.contains_key(v));
            let property = n > start && self.toks.get(n.saturating_sub(1)).is_some_and(|t| t.tok == Tok::Delimiter(delim::PERIOD));
            named && !property
        });
        if !reads_earlier {
            return Ok((Some(inner), None));
        }
        self.pos = start;
        Ok((None, Some(self.search_condition()?)))
    }
}

/// The condition joining the clause last added to `rewrite`, of
/// variables `vars`, to the clauses before it: its ELEMENT_ID equal to
/// theirs, of each variable it shares, if any.
///
/// # Errors
///
/// SQLCODE -103 if it shares a path variable, or a relationship's of
/// variable length.
fn shared(rewrite: &mut Rewrite, vars: &Exposure, at: usize) -> Result<Option<SearchCondition>> {
    let Some((own, earlier)) = rewrite.tables.split_last_mut() else { return Ok(None) };
    let mut on = None;
    for (name, exposed) in vars {
        let Some(first) = earlier.iter_mut().find(|b| b.vars.contains_key(name)) else { continue };
        let before = first.vars.get(name).copied();
        let once = |e: Exposed| matches!(e.kind, Declared::Path) || !matches!(e.degree, Degree::Singleton);
        if once(*exposed) || before.is_some_and(once) {
            let msg = format!("[GS §10.3] {name} is bound by an earlier MATCH, as only a node or a relationship of one hop is bound again");
            return Err(SqlError::at(Code::SyntaxRule, exposed.at, msg));
        }
        let equal = Predicate::Comparison(vec![identity(first, name, at)], CompOp::Eq, vec![identity(own, name, at)]);
        on = conjunction(on, Some(SearchCondition::Predicate(Box::new(equal), at)));
    }
    Ok(on)
}

/// The column of `bound` of the ELEMENT_ID of variable `name`, as reading
/// `name` alone makes it.
fn identity(bound: &mut Bound, name: &str, at: usize) -> ValueExpression {
    let operand = GraphVariable { name: name.to_owned(), at, anonymous: false };
    let value = ValueExpression::Graph(Box::new(GraphFunction { name: GraphFunctionName::ElementId, operand, property: None }), at);
    bound.column((format!("ELEMENT_ID({name})"), Some(name)), value, at)
}

/// The correlation name of the GRAPH_TABLE of clause `index`.
fn table_name(index: usize) -> String {
    if index == 0 { CYPHER_TABLE.to_owned() } else { format!("{CYPHER_TABLE}{}", index.saturating_add(1)) }
}

/// `a` AND `b`, of either alone if the other is none.
fn conjunction(a: Option<SearchCondition>, b: Option<SearchCondition>) -> Option<SearchCondition> {
    match (a, b) {
        (Some(a), Some(b)) => Some(SearchCondition::And(Box::new(a), Box::new(b))),
        (a, None) => a,
        (None, b) => b,
    }
}

/// [GS §10.3] The FROM of a Cypher query of `clauses` in `graph`: their
/// GRAPH_TABLEs, of `columns` each, joined in order.
///
/// # Errors
///
/// SQLCODE -103 if a pattern breaks a Syntax Rule.
pub(super) fn joined(
    graph: &TableName,
    clauses: Vec<Clause>,
    columns: Vec<Vec<(String, GraphColumn)>>,
    at: usize,
) -> Result<TableReference> {
    let mut from: Option<TableReference> = None;
    for (index, (clause, read)) in clauses.into_iter().zip(columns).enumerate() {
        let table = table_name(index);
        let made = super::super::graph_table(GraphReference::Named(graph.clone()), clause.pattern, read, &table, clause.at)?;
        let right = TableReference::Graph(Box::new(made));
        let left = match (from.take(), clause.optional) {
            (Some(left), true | false) => left,
            (None, true) => Parser::new(ONE_ROW)?.table_reference()?,
            (None, false) => {
                from = Some(right);
                continue;
            }
        };
        let kind = if clause.optional { JoinKind::Left } else { JoinKind::Inner };
        let truth =
            || SearchCondition::Predicand(Box::new(ValueExpression::Value(ValueSpecification::Literal(Literal::Boolean(Some(true))))), at);
        let on = Some(JoinSpecification::On(clause.on.unwrap_or_else(truth)));
        from = Some(TableReference::Joined(Box::new(JoinedTable {
            left,
            right,
            kind,
            natural: false,
            on,
            at: clause.at,
            partitioned: [None, None],
        })));
    }
    from.ok_or_else(|| SqlError::at(Code::Syntax, at, "[GS §10.3] a Cypher query has a MATCH clause"))
}

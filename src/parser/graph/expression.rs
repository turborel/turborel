//! \[EXTENSION\] [P1 §4.7] The predicates and functions of graph
//! elements and paths: SAME, ALL_DIFFERENT, PROPERTY_EXISTS, EXISTS IN
//! PATH and the functions of one variable, PREV and FIRST among them,
//! inside a GRAPH_TABLE only; and IS \[NOT\]
//! SOURCE OF, DESTINATION OF, LABELED and DIRECTED, and `x` : `<label
//! expression>`, after any row value, which the binder refuses outside a
//! GRAPH_TABLE.

use super::super::Parser;
use crate::ast::{
    ColumnSpecification, GraphFunction, GraphFunctionName, GraphPredicate, GraphVariable, PathEnd, Predicate, RowValue, SameKind,
    SearchCondition, ValueExpression,
};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

// [P1 §4.7] The words of the graph predicates that are no key words of
// SQL-92.
/// SAME, the predicate.
const SAME: &str = "SAME";
/// ALL_DIFFERENT, the predicate.
const ALL_DIFFERENT: &str = "ALL_DIFFERENT";
/// \[EXTENSION\] VERTEX_EQUAL, SAME of two nodes.
const VERTEX_EQUAL: &str = "VERTEX_EQUAL";
/// \[EXTENSION\] EDGE_EQUAL, SAME of two edges.
const EDGE_EQUAL: &str = "EDGE_EQUAL";
/// PROPERTY_EXISTS, the predicate.
const PROPERTY_EXISTS: &str = "PROPERTY_EXISTS";
/// SOURCE, of IS SOURCE OF.
const SOURCE: &str = "SOURCE";
/// DESTINATION, of IS DESTINATION OF.
const DESTINATION: &str = "DESTINATION";
/// LABELED, of IS LABELED.
const LABELED: &str = "LABELED";
/// DIRECTED, of IS DIRECTED.
const DIRECTED: &str = "DIRECTED";

/// The predicates written as a name before "(", each with its spelling of
/// SAME if one.
const CALLED: [(&str, Option<SameKind>); 5] = [
    (SAME, Some(SameKind::Same)),
    (VERTEX_EQUAL, Some(SameKind::VertexEqual)),
    (EDGE_EQUAL, Some(SameKind::EdgeEqual)),
    (ALL_DIFFERENT, None),
    (PROPERTY_EXISTS, None),
];

impl Parser {
    /// The name, before "(", of a graph predicate or function, if one is
    /// next inside a GRAPH_TABLE.
    fn graph_word(&self) -> Option<String> {
        let Tok::Identifier(word) = self.peek() else { return None };
        let call = self.graph > 0 && *self.peek_at(1) == Tok::Delimiter(delim::LEFT_PAREN);
        call.then(|| word.clone())
    }

    /// A graph predicate written as a call, SAME, ALL_DIFFERENT,
    /// VERTEX_EQUAL, EDGE_EQUAL or PROPERTY_EXISTS, if one is next inside a
    /// GRAPH_TABLE.
    pub(in crate::parser) fn graph_predicate_call(&mut self) -> Result<Option<Predicate>> {
        if self.graph > 0 && self.is_kw(kw::EXISTS) && self.next_is_kw(kw::IN) {
            return self.in_path().map(Some);
        }
        let Some(word) = self.graph_word() else { return Ok(None) };
        let Some((_, same)) = CALLED.iter().find(|(w, _)| *w == word) else { return Ok(None) };
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let element = self.graph_variable()?;
        self.expect_delim(delim::COMMA)?;
        let predicate = if word == PROPERTY_EXISTS {
            GraphPredicate::PropertyExists { element, property: self.identifier()? }
        } else {
            let mut elements = vec![element, self.graph_variable()?];
            let pair = matches!(same, Some(SameKind::VertexEqual | SameKind::EdgeEqual));
            while !pair && self.accept_delim(delim::COMMA) {
                elements.push(self.graph_variable()?);
            }
            match same {
                Some(kind) => GraphPredicate::Same { kind: *kind, elements },
                None => GraphPredicate::AllDifferent(elements),
            }
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(Some(Predicate::Graph(Box::new(predicate))))
    }

    /// A graph function or a graph predicate written as a call, as a
    /// value, if one is next inside a GRAPH_TABLE; PREV only of a
    /// variable, `PREV ( x )`, so a row pattern navigation of a subquery's
    /// MATCH_RECOGNIZE is still one.
    pub(in crate::parser) fn graph_primary(&mut self) -> Result<Option<ValueExpression>> {
        let at = self.here();
        if let Some(p) = self.graph_predicate_call()? {
            return Ok(Some(ValueExpression::Boolean(Box::new(SearchCondition::Predicate(Box::new(p), at)))));
        }
        let Some(name) = self.graph_word().and_then(|w| GraphFunctionName::named(&w)) else { return Ok(None) };
        if name == GraphFunctionName::Prev && !self.at_iteration_reference() {
            return Ok(None);
        }
        self.graph_function(name).map(Some)
    }

    /// \[EXTENSION\] FIRST ( `<variable>` ) inside a GRAPH_TABLE, else a
    /// row pattern navigation of FIRST or LAST.
    pub(in crate::parser) fn first_primary(&mut self) -> Result<ValueExpression> {
        let first = self.graph > 0 && self.is_kw(kw::FIRST) && self.at_iteration_reference();
        if first { self.graph_function(GraphFunctionName::First) } else { self.first_or_last_primary() }
    }

    /// Whether PREV or FIRST here is of an element variable: a name then
    /// ")", or PREV or FIRST again, which GRF-0041 refuses.
    fn at_iteration_reference(&self) -> bool {
        let named = matches!(self.peek_at(2), Tok::Identifier(_)) && *self.peek_at(3) == Tok::Delimiter(delim::RIGHT_PAREN);
        named || self.nested_iteration(2)
    }

    /// Whether PREV ( or FIRST ( is `n` tokens ahead.
    fn nested_iteration(&self, n: usize) -> bool {
        let word = matches!(self.peek_at(n), Tok::Identifier(w) if w == GraphFunctionName::Prev.word())
            || matches!(self.peek_at(n), Tok::KeyWord(kw::FIRST));
        word && *self.peek_at(n.saturating_add(1)) == Tok::Delimiter(delim::LEFT_PAREN)
    }

    /// The graph function `name`, its name next: `<name>` ( `<variable>`
    /// ), and of PREV and FIRST \[. `<property name>`\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 of PREV or FIRST of PREV or FIRST (GRF-0041).
    fn graph_function(&mut self, name: GraphFunctionName) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        if self.nested_iteration(0) {
            let msg = "[EXTENSION] PREV and FIRST are of an element variable, not nested (GRF-0041)";
            return Err(crate::error::SqlError::at(crate::error::Code::SyntaxRule, self.here(), msg));
        }
        let operand = self.graph_variable()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let iteration = matches!(name, GraphFunctionName::Prev | GraphFunctionName::First);
        let property = if iteration && self.accept_delim(delim::PERIOD) { Some(self.identifier()?) } else { None };
        Ok(ValueExpression::Graph(Box::new(GraphFunction { name, operand, property }), at))
    }

    /// \[EXTENSION\] EXISTS IN PATH `<path variable>` ( `<path pattern
    /// expression>` \[WHERE `<search condition>`\] ), EXISTS next.
    fn in_path(&mut self) -> Result<Predicate> {
        self.advance();
        self.advance();
        self.expect_word(super::super::modules::PATH)?;
        let path = self.graph_variable()?;
        if !self.is_delim(delim::LEFT_PAREN) {
            return Err(self.syntax("[EXTENSION] expected \"(\" and the pattern EXISTS IN PATH looks for"));
        }
        let pattern = Box::new(self.parenthesized_path(false)?);
        Ok(Predicate::Graph(Box::new(GraphPredicate::InPath { path, pattern })))
    }

    /// Whether a graph predicate's second part is next: IS \[NOT\] SOURCE
    /// OF, DESTINATION OF, LABELED or DIRECTED, or ":".
    pub(in crate::parser) fn at_graph_is_predicate(&self) -> bool {
        if self.is_delim(delim::COLON) {
            return true;
        }
        let n = if matches!(self.peek_at(1), Tok::KeyWord(kw::NOT)) { 2 } else { 1 };
        let word = |w: &str| matches!(self.peek_at(n), Tok::Identifier(x) if x == w);
        let of = matches!(self.peek_at(n.saturating_add(1)), Tok::KeyWord(kw::OF));
        self.is_kw(kw::IS) && ((of && (word(SOURCE) || word(DESTINATION))) || word(LABELED) || word(DIRECTED))
    }

    /// The graph predicate whose first part is `value`, at its second:
    /// IS \[NOT\] SOURCE OF `<edge>`, DESTINATION OF `<edge>`, LABELED
    /// `<label expression>` or DIRECTED, or : `<label expression>`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `value` is no graph variable.
    pub(in crate::parser) fn graph_is_predicate(&mut self, value: &RowValue) -> Result<Predicate> {
        let operand = self.graph_operand(value)?;
        if self.accept_delim(delim::COLON) {
            let label = self.label_expression()?;
            return Ok(Predicate::Graph(Box::new(GraphPredicate::Labeled { element: operand, label, not: false })));
        }
        self.expect_kw(kw::IS)?;
        let not = self.accept_kw(kw::NOT);
        let predicate = if self.accept_word(LABELED) {
            GraphPredicate::Labeled { element: operand, label: self.label_expression()?, not }
        } else if self.accept_word(DIRECTED) {
            GraphPredicate::Directed { edge: operand, not }
        } else {
            let end =
                if self.accept_word(SOURCE) { PathEnd::Source } else { self.expect_word(DESTINATION).map(|()| PathEnd::Destination)? };
            self.expect_kw(kw::OF)?;
            GraphPredicate::Endpoint { node: operand, edge: self.graph_variable()?, end, not }
        };
        Ok(Predicate::Graph(Box::new(predicate)))
    }

    /// The graph variable `value` names: one unqualified column
    /// reference.
    fn graph_operand(&self, value: &RowValue) -> Result<GraphVariable> {
        if let [ValueExpression::Column(ColumnSpecification { qualifier: None, column, catalog: None, at })] = value.as_slice() {
            return Ok(GraphVariable { name: column.clone(), at: *at, anonymous: false });
        }
        Err(self.rule("[EXTENSION] [P1 §4.7] a graph predicate is of a graph variable, written as its name"))
    }
}

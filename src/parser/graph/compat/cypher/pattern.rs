//! \[EXTENSION\] [GS §10.3] An openCypher pattern, read as a GPML path
//! pattern:
//!
//! ```text
//! <cypher pattern> ::= [ <path variable> = ] <cypher path>
//!     | [ <path variable> = ] { shortestPath | allShortestPaths } ( <cypher path> )
//! <cypher path> ::= <cypher node> [ { <cypher relationship> <cypher node> }... ]
//! <cypher node> ::= ( [ <variable> ] [ <cypher labels> ] [ <property map> ] )
//! <cypher labels> ::= : <label> [ { : <label> }... ] | : <label> [ { '|' [ : ] <label> }... ]
//! <cypher relationship> ::= [ < ] - [ '[' <cypher relationship detail> ']' ] - [ > ]
//! <cypher relationship detail> ::= [ <variable> ] [ : <type> [ { '|' [ : ] <type> }... ] ]
//!     [ * [ <unsigned integer> ] [ .. [ <unsigned integer> ] ] ] [ <property map> ]
//! ```
//!
//! `:A:B` is the label conjunction A & B, `:A|B` the disjunction. A
//! relationship pointing neither way or both ways, `-[ ]-` or `<-[ ]->`,
//! is of either direction, GPML's `-[ ]-`. With no detail, `-->`, `<--`,
//! `--` and `<-->`, a relationship has no variable, type or length; the
//! lexer reads "--" so only written with no space, between a node's ")"
//! and the next "(". `*` is `{1,}`, `*n` `{n}`, `*n..` `{n,}`, `*..m`
//! `{1,m}` and `*n..m` `{n,m}`. shortestPath is ANY SHORTEST and
//! allShortestPaths ALL SHORTEST.

use crate::ast::{
    EdgePattern, ElementPattern, LabelExpression, Orientation, PathExpression, PathFactor, PathPattern, PathPrefix, PathPrimary, PathTerm,
    Quantifier, Restrictor, Selector,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim};
use crate::parser::Parser;

/// shortestPath, as an identifier is read.
const SHORTEST_PATH: &str = "SHORTESTPATH";
/// allShortestPaths, as an identifier is read.
const ALL_SHORTEST_PATHS: &str = "ALLSHORTESTPATHS";
/// The range of a variable length relationship, `..`.
const RANGE: &str = "..";

impl Parser {
    /// `<cypher pattern>`.
    pub(super) fn cypher_pattern(&mut self) -> Result<PathPattern> {
        let at = self.here();
        let named = matches!(self.peek(), Tok::Identifier(_)) && *self.peek_at(1) == Tok::Delimiter(delim::EQUALS);
        let variable = if named { Some(self.graph_variable()?) } else { None };
        if named {
            self.advance();
        }
        let selector = [(SHORTEST_PATH, Selector::AnyShortest), (ALL_SHORTEST_PATHS, Selector::AllShortest)]
            .into_iter()
            .find(|(w, _)| self.is_word(w) && *self.peek_at(1) == Tok::Delimiter(delim::LEFT_PAREN))
            .map(|(_, s)| s);
        if selector.is_some() {
            self.advance();
            self.advance();
        }
        let factors = self.cypher_path()?;
        if selector.is_some() {
            self.expect_delim(delim::RIGHT_PAREN)?;
        }
        // [GS §10.3] Cypher matches no relationship twice: TRAIL, which
        // also bounds a variable length relationship [GS R7.3].
        let restrictor = if selector.is_some() { None } else { Some(Restrictor::Trail) };
        let prefix = Some(PathPrefix { selector, restrictor, at });
        Ok(PathPattern { variable, prefix, expression: PathExpression::Term(PathTerm { factors }), at })
    }

    /// `<cypher path>`: its factors.
    fn cypher_path(&mut self) -> Result<Vec<PathFactor>> {
        let mut factors = vec![self.cypher_node()?];
        while self.is_delim(delim::MINUS) || self.is_delim(delim::LESS) {
            factors.push(self.cypher_relationship()?);
            factors.push(self.cypher_node()?);
        }
        Ok(factors)
    }

    /// `<cypher node>`.
    fn cypher_node(&mut self) -> Result<PathFactor> {
        let at = self.here();
        self.expect_delim(delim::LEFT_PAREN)?;
        let (element, length) = self.cypher_filler(at)?;
        if length.is_some() {
            return Err(SqlError::at(Code::Syntax, at, "[GS §10.3] a node has no length"));
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(PathFactor { primary: PathPrimary::Node(element), quantifier: None })
    }

    /// A variable, labels, length and property map, each if written: the
    /// element pattern, and the quantifier of its length.
    fn cypher_filler(&mut self, at: usize) -> Result<(ElementPattern, Option<Quantifier>)> {
        let variable = if matches!(self.peek(), Tok::Identifier(_)) { Some(self.graph_variable()?) } else { None };
        let label = if self.is_delim(delim::COLON) { Some(self.cypher_labels()?) } else { None };
        let quantifier = self.cypher_length()?;
        let properties = if self.is_delim(delim::LEFT_BRACE) { self.property_specification()? } else { Vec::new() };
        Ok((ElementPattern { variable, label, where_clause: None, properties, at }, quantifier))
    }

    /// `<cypher labels>`.
    fn cypher_labels(&mut self) -> Result<LabelExpression> {
        self.expect_delim(delim::COLON)?;
        let mut labels = LabelExpression::Label(self.identifier()?, self.here());
        loop {
            let or = self.accept_delim(delim::VERTICAL_BAR);
            let and = !or && self.accept_delim(delim::COLON);
            if !or && !and {
                return Ok(labels);
            }
            if or {
                self.accept_delim(delim::COLON);
            }
            let next = Box::new(LabelExpression::Label(self.identifier()?, self.here()));
            labels = if or {
                LabelExpression::Disjunction(Box::new(labels), next)
            } else {
                LabelExpression::Conjunction(Box::new(labels), next)
            };
        }
    }

    /// `<cypher relationship>`: of a detail in brackets, or none, as the
    /// abbreviations `-->`, `<--`, `--` and `<-->` are.
    fn cypher_relationship(&mut self) -> Result<PathFactor> {
        let at = self.here();
        let left = self.accept_delim(delim::LESS);
        self.expect_delim(delim::MINUS)?;
        let detailed = self.accept_delim(delim::LEFT_BRACKET);
        let (filler, quantifier) = if detailed {
            let read = self.cypher_filler(at)?;
            self.expect_delim(delim::RIGHT_BRACKET)?;
            read
        } else {
            (ElementPattern { variable: None, label: None, where_clause: None, properties: Vec::new(), at }, None)
        };
        let right = self.accept_delim(delim::RIGHT_ARROW);
        if !right {
            self.expect_delim(delim::MINUS)?;
        }
        // [GS §10.3] Pointing both ways, as `<-->` does, a relationship
        // is of either direction.
        let orientation = match (left, right) {
            (true, false) => Orientation::Left,
            (false, true) => Orientation::Right,
            (false, false) | (true, true) => Orientation::Any,
        };
        Ok(PathFactor { primary: PathPrimary::Edge(EdgePattern { orientation, abbreviated: false, filler }), quantifier })
    }

    /// The length of a variable length relationship, `*` and what follows
    /// it, if written: its quantifier. The text after `*` is read as
    /// written, since `1..3` is no token of SQL.
    fn cypher_length(&mut self) -> Result<Option<Quantifier>> {
        if !self.is_delim(delim::ASTERISK) {
            return Ok(None);
        }
        let at = self.here();
        let from = self.toks.get(self.pos).map_or(at, |t| t.end);
        let rest: String = self.src.get(from..).unwrap_or_default().iter().collect();
        let written: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let end = from.saturating_add(written.chars().count());
        self.advance();
        while self.here() < end {
            self.advance();
        }
        let bad = || SqlError::at(Code::Syntax, at, format!("[GS §10.3] *{written} is no length of a relationship"));
        let bound = |s: &str| -> Result<Option<u32>> { if s.is_empty() { Ok(None) } else { s.parse().map(Some).map_err(|_| bad()) } };
        let (lower, upper) = match written.split_once(RANGE) {
            Some((low, high)) => (bound(low)?.unwrap_or(1), bound(high)?),
            None => match bound(&written)? {
                Some(n) => (n, Some(n)),
                None => (1, None),
            },
        };
        Ok(Some(Quantifier { lower, upper, sugar: None, at }))
    }
}

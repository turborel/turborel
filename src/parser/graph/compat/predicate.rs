//! \[EXTENSION\] [GS §10.1] MATCH in a WHERE clause, read as GPML
//! path patterns:
//!
//! ```text
//! <graph match predicate> ::= MATCH ( <graph search item> [ { AND <graph search item> }... ] )
//! <graph search item> ::= <simple match pattern>
//!     | SHORTEST_PATH ( <arbitrary length pattern> [ { AND <arbitrary length pattern> }... ] )
//!     | LAST_NODE ( <node alias> ) = LAST_NODE ( <node alias> )
//! <simple match pattern> ::= <match node> { <match edge> <match node> | <match group> }...
//! <match group> ::= ( <match edge> <node alias> ) <graph pattern quantifier>
//! <arbitrary length pattern> ::= <match node> <match group>
//!     | ( <node alias> <match edge> ) <graph pattern quantifier> <match node>
//! <match node> ::= <node alias> | LAST_NODE ( <node alias> )
//! <match edge> ::= - ( <edge alias> ) -> | <- ( <edge alias> ) -
//! ```
//!
//! Each alias is a variable of its name: a table of the FROM clause, or a
//! FOR PATH one, which inside a group is a group variable [P1 §4.4]. A
//! SHORTEST_PATH pattern is ANY SHORTEST [P1 §5.1], and its last node a
//! variable of its own, LAST_NODE ( x ), so another pattern starts there
//! and LAST_NODE ( x ) = LAST_NODE ( y ) is SAME of two of them. A group
//! may be written outside SHORTEST_PATH too, and an edge alias twice.

use crate::ast::{
    EdgePattern, ElementPattern, GraphVariable, MatchClause, Orientation, ParenthesizedPath, PathExpression, PathFactor, PathPattern,
    PathPrefix, PathPrimary, PathTerm, Predicate, SearchCondition, Selector,
};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;

/// SHORTEST_PATH, of an arbitrary length pattern.
const SHORTEST_PATH: &str = "SHORTEST_PATH";
/// LAST_NODE, of a SHORTEST_PATH pattern's last node.
const LAST_NODE: &str = "LAST_NODE";
/// The start of the name of a SHORTEST_PATH pattern's last node's
/// variable, which no FROM clause's table has.
pub const LAST_NODE_PREFIX: &str = "LAST_NODE(";

/// The name of the variable of the last node of the SHORTEST_PATH pattern
/// whose group node variable is `alias`.
pub(in crate::parser) fn last_node(alias: &str) -> String {
    format!("{LAST_NODE}({alias})")
}

/// A node pattern of variable `name`, at `at`.
pub(in crate::parser) fn node(name: String, at: usize) -> PathFactor {
    let variable = Some(GraphVariable { name, at, anonymous: false });
    let element = ElementPattern { variable, label: None, where_clause: None, properties: Vec::new(), at };
    PathFactor { primary: PathPrimary::Node(element), quantifier: None }
}

impl Parser {
    /// Whether MATCH ( begins here, in a search condition.
    pub(in crate::parser) fn at_graph_match(&self) -> bool {
        self.is_kw(kw::MATCH) && *self.peek_at(1) == Tok::Delimiter(delim::LEFT_PAREN)
    }

    /// `<graph match predicate>`.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it does not conform to the Format.
    pub(in crate::parser) fn graph_match(&mut self) -> Result<SearchCondition> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut clause = MatchClause { paths: Vec::new(), ends: Vec::new(), path_tables: Vec::new(), measures: Vec::new(), at };
        loop {
            self.graph_search_item(&mut clause)?;
            if !self.accept_kw(kw::AND) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(SearchCondition::Predicate(Box::new(Predicate::GraphMatch(Box::new(clause))), at))
    }

    /// A `<graph search item>`, added to `clause`.
    fn graph_search_item(&mut self, clause: &mut MatchClause) -> Result<()> {
        if self.is_word(SHORTEST_PATH) && *self.peek_at(1) == Tok::Delimiter(delim::LEFT_PAREN) {
            self.advance();
            self.advance();
            clause.paths.push(self.arbitrary_length_pattern()?);
            while self.accept_kw(kw::AND) {
                clause.paths.push(self.arbitrary_length_pattern()?);
            }
            return self.expect_delim(delim::RIGHT_PAREN);
        }
        let ends = self.at_last_node() && *self.peek_at(4) == Tok::Delimiter(delim::EQUALS);
        if ends {
            let first = self.match_node()?;
            self.expect_delim(delim::EQUALS)?;
            if !self.at_last_node() {
                return Err(self.syntax("[GS §10.1] expected LAST_NODE ( <node alias> )"));
            }
            clause.ends.push((first.0, self.match_node()?.0));
            return Ok(());
        }
        let at = self.here();
        let factors = self.simple_match_pattern()?;
        clause.paths.push(path(factors, None, at));
        Ok(())
    }

    /// Whether LAST_NODE ( begins here.
    fn at_last_node(&self) -> bool {
        self.is_word(LAST_NODE) && *self.peek_at(1) == Tok::Delimiter(delim::LEFT_PAREN)
    }

    /// `<match node>`: its variable's name, and where it is written.
    fn match_node(&mut self) -> Result<(String, usize)> {
        let at = self.here();
        if !self.at_last_node() {
            return Ok((self.identifier()?, at));
        }
        self.advance();
        self.advance();
        let alias = self.identifier()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok((last_node(&alias), at))
    }

    /// Whether a `<match edge>` begins here.
    fn at_match_edge(&self) -> bool {
        let minus = |p: &Self, n: usize| *p.peek_at(n) == Tok::Delimiter(delim::MINUS);
        let opens = |p: &Self, n: usize| *p.peek_at(n) == Tok::Delimiter(delim::LEFT_PAREN);
        (minus(self, 0) && opens(self, 1)) || (*self.peek() == Tok::Delimiter(delim::LESS) && minus(self, 1) && opens(self, 2))
    }

    /// `<match edge>`, an edge pattern in full.
    fn match_edge(&mut self) -> Result<PathFactor> {
        let at = self.here();
        let left = self.accept_delim(delim::LESS);
        self.expect_delim(delim::MINUS)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let name = self.identifier()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let orientation = if left {
            self.expect_delim(delim::MINUS)?;
            Orientation::Left
        } else {
            self.expect_delim(delim::RIGHT_ARROW)?;
            Orientation::Right
        };
        let variable = Some(GraphVariable { name, at, anonymous: false });
        let filler = ElementPattern { variable, label: None, where_clause: None, properties: Vec::new(), at };
        let edge = EdgePattern { orientation, abbreviated: false, filler };
        Ok(PathFactor { primary: PathPrimary::Edge(edge), quantifier: None })
    }

    /// `<simple match pattern>`: its factors.
    fn simple_match_pattern(&mut self) -> Result<Vec<PathFactor>> {
        let (name, at) = self.match_node()?;
        let mut factors = vec![node(name, at)];
        loop {
            if self.at_match_edge() {
                factors.push(self.match_edge()?);
                let (next, next_at) = self.match_node()?;
                factors.push(node(next, next_at));
            } else if self.is_delim(delim::LEFT_PAREN) {
                factors.push(self.match_group(false)?.0);
            } else {
                return Ok(factors);
            }
        }
    }

    /// `<match group>`, or with `backward` its edge after its node: the
    /// group, and the name of its node.
    fn match_group(&mut self, backward: bool) -> Result<(PathFactor, String)> {
        let at = self.here();
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut factors = Vec::new();
        if !backward {
            factors.push(self.match_edge()?);
        }
        let node_at = self.here();
        let name = self.identifier()?;
        factors.push(node(name.clone(), node_at));
        if backward {
            factors.push(self.match_edge()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        let Some(quantifier) = self.quantifier()? else {
            return Err(self.syntax("[GS §10.1] expected a quantifier after a group"));
        };
        let expression = PathExpression::Term(PathTerm { factors });
        let group = ParenthesizedPath { variable: None, prefix: None, expression, where_clause: None, bracketed: false, at };
        Ok((PathFactor { primary: PathPrimary::Parenthesized(Box::new(group)), quantifier: Some(quantifier) }, name))
    }

    /// `<arbitrary length pattern>`, as ANY SHORTEST with its last node a
    /// variable of its own.
    fn arbitrary_length_pattern(&mut self) -> Result<PathPattern> {
        let at = self.here();
        let selector = Some(PathPrefix { selector: Some(Selector::AnyShortest), restrictor: None, at });
        if self.is_delim(delim::LEFT_PAREN) {
            let (group, name) = self.match_group(true)?;
            let (anchor, anchor_at) = self.match_node()?;
            let factors = vec![node(last_node(&name), at), group, node(anchor, anchor_at)];
            return Ok(path(factors, selector, at));
        }
        let (anchor, anchor_at) = self.match_node()?;
        let (group, name) = self.match_group(false)?;
        let end = self.here();
        Ok(path(vec![node(anchor, anchor_at), group, node(last_node(&name), end)], selector, at))
    }
}

/// The path pattern of `factors`, of prefix `prefix`.
fn path(factors: Vec<PathFactor>, prefix: Option<PathPrefix>, at: usize) -> PathPattern {
    PathPattern { variable: None, prefix, expression: PathExpression::Term(PathTerm { factors }), at }
}

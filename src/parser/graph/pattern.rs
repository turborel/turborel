//! \[EXTENSION\] [P1 §4.2 to §4.5, §5.1, §7.1] A `<graph pattern>`: its
//! match mode, path patterns, their path variables, restrictors and
//! selectors, KEEP, and the unions, alternations and concatenations of
//! path factors they are made of. Element patterns and quantifiers are in
//! `element.rs`.
//!
//! Parenthesized patterns and path pattern expressions nest; each level
//! counts toward the statement's depth (`Parser::nested`, RULES.md
//! R-CF-6).

use super::super::Parser;
use super::super::modules::PATH;
use super::super::ptf::KEEP;
use super::super::window::GROUPS;
use crate::ast::{
    Cheapest, GraphPattern, GraphVariable, MatchMode, ParenthesizedPath, PathExpression, PathFactor, PathPattern, PathPrefix, PathPrimary,
    PathTerm, Restrictor, SegmentFilter, Selector, ValueExpression,
};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

// [P1 §5.1, §7.1, Fig. 7, Fig. 8] The words of the prefixes and match
// modes that are no key words of SQL-92.
/// PATHS, after a prefix.
const PATHS: &str = "PATHS";
/// WALK, the restrictor.
const WALK: &str = "WALK";
/// TRAIL, the restrictor.
const TRAIL: &str = "TRAIL";
/// ACYCLIC, the restrictor.
const ACYCLIC: &str = "ACYCLIC";
/// SIMPLE, the restrictor.
const SIMPLE: &str = "SIMPLE";
/// SHORTEST, of the selectors.
const SHORTEST: &str = "SHORTEST";
/// CHEAPEST, of the \[EXTENSION\] selectors.
const CHEAPEST: &str = "CHEAPEST";
/// COST, of a CHEAPEST selector.
const COST: &str = "COST";
/// REPEATABLE, of REPEATABLE ELEMENTS.
const REPEATABLE: &str = "REPEATABLE";
/// DIFFERENT, of DIFFERENT EDGES and DIFFERENT VERTICES.
const DIFFERENT: &str = "DIFFERENT";
/// ELEMENT, of REPEATABLE ELEMENT BINDINGS.
const ELEMENT: &str = "ELEMENT";
/// ELEMENTS, of REPEATABLE ELEMENTS.
const ELEMENTS: &str = "ELEMENTS";
/// EDGE, of DIFFERENT EDGE BINDINGS.
const EDGE: &str = "EDGE";
/// EDGES, of DIFFERENT EDGES.
const EDGES: &str = "EDGES";
/// VERTICES, of DIFFERENT VERTICES.
const VERTICES: &str = "VERTICES";
/// BINDINGS, of the match modes' longer spellings.
const BINDINGS: &str = "BINDINGS";
/// EACH, of FOR EACH SEGMENT.
const EACH: &str = "EACH";
/// SEGMENT, of FOR EACH SEGMENT.
const SEGMENT: &str = "SEGMENT";

/// The restrictors, each with its word.
const RESTRICTORS: [(&str, Restrictor); 4] =
    [(WALK, Restrictor::Walk), (TRAIL, Restrictor::Trail), (ACYCLIC, Restrictor::Acyclic), (SIMPLE, Restrictor::Simple)];

impl Parser {
    /// [P1 §4.3] `<graph pattern>` ::= \[`<match mode>`\] `<path pattern>`
    /// \[, ...\] \[KEEP `<path pattern prefix>`\] \[WHERE `<search
    /// condition>`\], KEEP also after the WHERE.
    pub(super) fn graph_pattern(&mut self) -> Result<GraphPattern> {
        let at = self.here();
        let mode = self.match_mode()?;
        let mut paths = vec![self.path_pattern()?];
        while self.accept_delim(delim::COMMA) {
            paths.push(self.path_pattern()?);
        }
        let mut keep = self.keep_clause()?;
        let where_clause = if self.accept_kw(kw::WHERE) { Some(self.search_condition()?) } else { None };
        if keep.is_none() {
            keep = self.keep_clause()?;
        }
        let mut segments = Vec::new();
        while self.is_kw(kw::FOR) && self.next_is_word(EACH) {
            segments.push(self.segment_filter()?);
        }
        Ok(GraphPattern { mode, paths, keep, where_clause, segments, at })
    }

    /// Whether the token after the one at hand is the word `word`.
    fn next_is_word(&self, word: &str) -> bool {
        matches!(self.peek_at(1), Tok::Identifier(w) if w == word)
    }

    /// \[EXTENSION\] `<segment filter>` ::= FOR EACH SEGMENT ( `<variable>`
    /// , `<variable>` ) IN `<path variable>` WHERE `<search condition>`.
    fn segment_filter(&mut self) -> Result<SegmentFilter> {
        self.advance();
        self.advance();
        self.expect_word(SEGMENT)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let first = self.graph_variable()?;
        self.expect_delim(delim::COMMA)?;
        let second = self.graph_variable()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        self.expect_kw(kw::IN)?;
        let path = self.graph_variable()?;
        self.expect_kw(kw::WHERE)?;
        Ok(SegmentFilter { first, second, path, condition: self.search_condition()? })
    }

    /// KEEP `<path pattern prefix>`, if written.
    fn keep_clause(&mut self) -> Result<Option<PathPrefix>> {
        if !self.accept_word(KEEP) {
            return Ok(None);
        }
        match self.path_prefix()? {
            Some(prefix) => Ok(Some(prefix)),
            None => Err(self.syntax("[EXTENSION] KEEP is followed by a selector or a restrictor")),
        }
    }

    /// [P1 §7.1] `<match mode>` ::= REPEATABLE {ELEMENTS | ELEMENT
    /// \[BINDINGS\]} | DIFFERENT {EDGES | EDGE \[BINDINGS\]} | \[EXTENSION\]
    /// DIFFERENT {VERTICES | VERTEX \[BINDINGS\]}, if written.
    fn match_mode(&mut self) -> Result<Option<MatchMode>> {
        let repeatable = self.is_word(REPEATABLE) && matches!(self.peek_at(1), Tok::Identifier(w) if w == ELEMENTS || w == ELEMENT);
        if repeatable {
            self.advance();
            let one = self.accept_word(ELEMENT);
            if !one {
                self.expect_word(ELEMENTS)?;
            }
            self.bindings(one);
            return Ok(Some(MatchMode::RepeatableElements));
        }
        let words = [EDGES, EDGE, VERTICES, super::VERTEX];
        if !(self.is_word(DIFFERENT) && matches!(self.peek_at(1), Tok::Identifier(w) if words.contains(&w.as_str()))) {
            return Ok(None);
        }
        self.advance();
        let mode = if self.is_word(EDGES) || self.is_word(EDGE) { MatchMode::DifferentEdges } else { MatchMode::DifferentVertices };
        let one = self.is_word(EDGE) || self.is_word(super::VERTEX);
        self.advance();
        self.bindings(one);
        Ok(Some(mode))
    }

    /// BINDINGS after the singular word of a match mode, if written.
    fn bindings(&mut self, singular: bool) {
        if singular {
            self.accept_word(BINDINGS);
        }
    }

    /// [P1 §4.2, §5.1] `<path pattern>` ::= \[`<path variable>` =\]
    /// \[`<path pattern prefix>`\] `<path pattern expression>`, the
    /// variable also after the prefix.
    pub(super) fn path_pattern(&mut self) -> Result<PathPattern> {
        let at = self.here();
        let (variable, prefix) = self.variable_and_prefix()?;
        let expression = self.path_pattern_expression()?;
        Ok(PathPattern { variable, prefix, expression, at })
    }

    /// A path or subpath variable and a prefix, each if written, in either
    /// order.
    fn variable_and_prefix(&mut self) -> Result<(Option<GraphVariable>, Option<PathPrefix>)> {
        let mut variable = self.path_variable()?;
        let prefix = self.path_prefix()?;
        if variable.is_none() && prefix.is_some() {
            variable = self.path_variable()?;
        }
        Ok((variable, prefix))
    }

    /// `<path variable>` =, if next.
    fn path_variable(&mut self) -> Result<Option<GraphVariable>> {
        if !matches!(self.peek(), Tok::Identifier(_)) || *self.peek_at(1) != Tok::Delimiter(delim::EQUALS) {
            return Ok(None);
        }
        let variable = self.graph_variable()?;
        self.advance();
        Ok(Some(variable))
    }

    /// Whether the word `word` is next and is no path variable's name, no
    /// "=" after it.
    fn at_prefix_word(&self, word: &str) -> bool {
        self.is_word(word) && *self.peek_at(1) != Tok::Delimiter(delim::EQUALS)
    }

    /// [P1 §5.1] `<path pattern prefix>` ::= `<selector>` \[`<restrictor>`\]
    /// \[PATH | PATHS\] | `<restrictor>` \[PATH | PATHS\], if written; of
    /// SHORTEST k, GROUP or GROUPS after them.
    pub(super) fn path_prefix(&mut self) -> Result<Option<PathPrefix>> {
        let at = self.here();
        let mut selector = self.selector()?;
        let restrictor = RESTRICTORS.iter().find(|(w, _)| self.at_prefix_word(w)).map(|(_, r)| *r);
        if restrictor.is_some() {
            self.advance();
        }
        if selector.is_none() && restrictor.is_none() {
            return Ok(None);
        }
        if self.at_prefix_word(PATH) || self.at_prefix_word(PATHS) {
            self.advance();
        }
        if let Some(Selector::ShortestK(k)) = selector
            && (self.accept_kw(kw::GROUP) || self.accept_word(GROUPS))
        {
            selector = Some(Selector::ShortestGroups(k));
        }
        Ok(Some(PathPrefix { selector, restrictor, at }))
    }

    /// [P1 Fig. 8] `<selector>`, if written: ANY \[k | SHORTEST\], ALL
    /// SHORTEST, SHORTEST k, or \[EXTENSION\] ANY CHEAPEST, ALL CHEAPEST
    /// or CHEAPEST k, each \[COST `<cost>`\].
    fn selector(&mut self) -> Result<Option<Selector>> {
        if self.accept_kw(kw::ANY) {
            if matches!(self.peek(), Tok::ExactNumeric(_)) {
                return Ok(Some(Selector::AnyK(self.paths_kept()?)));
            }
            if self.accept_word(SHORTEST) {
                return Ok(Some(Selector::AnyShortest));
            }
            if self.accept_word(CHEAPEST) {
                return self.cheapest(Cheapest::Any).map(Some);
            }
            return Ok(Some(Selector::Any));
        }
        if self.accept_kw(kw::ALL) {
            if self.accept_word(CHEAPEST) {
                return self.cheapest(Cheapest::All).map(Some);
            }
            self.expect_word(SHORTEST)?;
            return Ok(Some(Selector::AllShortest));
        }
        if self.at_prefix_word(SHORTEST) {
            self.advance();
            return Ok(Some(Selector::ShortestK(self.paths_kept()?)));
        }
        if self.at_prefix_word(CHEAPEST) {
            self.advance();
            let k = self.paths_kept()?;
            return self.cheapest(Cheapest::K(k)).map(Some);
        }
        Ok(None)
    }

    /// The k of a selector: an `<unsigned integer>` greater than 0.
    fn paths_kept(&mut self) -> Result<u32> {
        let at = self.here();
        let k = self.unsigned_integer()?;
        if k == 0 {
            return Err(crate::error::SqlError::at(
                crate::error::Code::SyntaxRule,
                at,
                "[EXTENSION] the number of paths a selector keeps is greater than 0",
            ));
        }
        Ok(k)
    }

    /// \[EXTENSION\] A CHEAPEST selector, `which`, and its \[COST
    /// `<cost>`\]: ( `<value expression>` ), or a property reference, so a
    /// path pattern after it is not read as a method's arguments.
    fn cheapest(&mut self, which: Cheapest) -> Result<Selector> {
        if !self.accept_word(COST) {
            return Ok(Selector::Cheapest { which, cost: None });
        }
        let cost: ValueExpression = if self.accept_delim(delim::LEFT_PAREN) {
            let value = self.value_expression()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            value
        } else {
            self.property_reference()?
        };
        Ok(Selector::Cheapest { which, cost: Some(Box::new(cost)) })
    }

    /// [P1 §4.5] `<path pattern expression>` ::= `<path term>` | `<path
    /// term>` | `<path term>` \[...\] | `<path term>` |+| `<path term>`
    /// \[...\]: unions and alternations are not mixed.
    pub(super) fn path_pattern_expression(&mut self) -> Result<PathExpression> {
        self.nested(|p| {
            let first = p.path_term()?;
            let alternation = p.at_alternation();
            if !alternation && !p.is_delim(delim::VERTICAL_BAR) {
                return Ok(PathExpression::Term(first));
            }
            let mut terms = vec![first];
            while p.path_operator(alternation)? {
                terms.push(p.path_term()?);
            }
            Ok(if alternation { PathExpression::Alternation(terms) } else { PathExpression::Union(terms) })
        })
    }

    /// Whether `|+|` is next.
    fn at_alternation(&self) -> bool {
        self.is_delim(delim::VERTICAL_BAR)
            && *self.peek_at(1) == Tok::Delimiter(delim::PLUS)
            && *self.peek_at(2) == Tok::Delimiter(delim::VERTICAL_BAR)
    }

    /// The operator of a union, `|`, or of an alternation, `|+|`, taken if
    /// next; the other refused.
    fn path_operator(&mut self, alternation: bool) -> Result<bool> {
        let other = if alternation { self.is_delim(delim::VERTICAL_BAR) && !self.at_alternation() } else { self.at_alternation() };
        if other {
            return Err(self.syntax("[EXTENSION] a path pattern union and a multiset alternation are not mixed unparenthesized"));
        }
        if !self.is_delim(delim::VERTICAL_BAR) {
            return Ok(false);
        }
        self.advance();
        if alternation {
            self.advance();
            self.advance();
        }
        Ok(true)
    }

    /// [P1 §4.2] `<path term>`: one path factor or more, concatenated.
    fn path_term(&mut self) -> Result<PathTerm> {
        if !self.at_path_primary() {
            return Err(self.syntax("[EXTENSION] expected a node pattern, an edge pattern or a parenthesized path pattern"));
        }
        let mut factors = Vec::new();
        while self.at_path_primary() {
            factors.push(self.path_factor()?);
        }
        Ok(PathTerm { factors })
    }

    /// Whether a `<path primary>` begins here: "(", "[", or what begins an
    /// edge pattern.
    pub(super) fn at_path_primary(&self) -> bool {
        match self.peek() {
            Tok::Delimiter(d) => [
                delim::LEFT_PAREN,
                delim::LEFT_BRACKET,
                delim::LESS,
                delim::EUCLIDEAN_DISTANCE,
                delim::MINUS,
                delim::RIGHT_ARROW,
                delim::TILDE,
            ]
            .contains(d),
            Tok::KeyWord(_)
            | Tok::Identifier(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::End => false,
        }
    }

    /// [P1 §4.4] `<path factor>` ::= `<path primary>` \[`<quantifier>`\]:
    /// of an edge pattern or a parenthesized path pattern, not a node
    /// pattern.
    pub(super) fn path_factor(&mut self) -> Result<PathFactor> {
        let primary = if self.is_delim(delim::LEFT_BRACKET) {
            PathPrimary::Parenthesized(Box::new(self.parenthesized_path(true)?))
        } else if self.is_delim(delim::LEFT_PAREN) && !self.at_node_filler() {
            PathPrimary::Parenthesized(Box::new(self.parenthesized_path(false)?))
        } else if self.is_delim(delim::LEFT_PAREN) {
            PathPrimary::Node(self.node_pattern()?)
        } else {
            PathPrimary::Edge(self.edge_pattern()?)
        };
        let quantifier = self.quantifier()?;
        if quantifier.is_some() && matches!(primary, PathPrimary::Node(_)) {
            return Err(self.rule(
                "[EXTENSION] [P1 §4.4] a quantifier applies to an edge pattern or a parenthesized path pattern, not a node pattern",
            ));
        }
        Ok(PathFactor { primary, quantifier })
    }

    /// [P1 §4.4] A parenthesized path pattern, in brackets if `bracketed`:
    /// \[`<subpath variable>` =\] \[`<path pattern prefix>`\] `<path pattern
    /// expression>` \[WHERE `<search condition>`\].
    pub(super) fn parenthesized_path(&mut self, bracketed: bool) -> Result<ParenthesizedPath> {
        let at = self.here();
        self.advance();
        self.nested(|p| {
            let (variable, prefix) = p.variable_and_prefix()?;
            let expression = p.path_pattern_expression()?;
            let where_clause = if p.accept_kw(kw::WHERE) { Some(p.search_condition()?) } else { None };
            p.expect_delim(if bracketed { delim::RIGHT_BRACKET } else { delim::RIGHT_PAREN })?;
            Ok(ParenthesizedPath { variable, prefix, expression, where_clause, bracketed, at })
        })
    }
}

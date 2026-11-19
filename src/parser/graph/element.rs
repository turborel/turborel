//! \[EXTENSION\] [P1 §4.1, §4.4, Fig. 5, Fig. 6] Element patterns: a
//! node pattern, an edge pattern in any of its seven orientations, full or
//! abbreviated, and what fills either, a variable, a label expression, a
//! WHERE and properties; and the quantifiers after a path primary.
//!
//! The arrows are read from the delimiters they are made of, `<`, `-`,
//! `->`, `~`, `>`, `[` and `]`, as the lexer gives them. Like GQL's
//! tokens, `-[`, `~[`, `<-[` and `<~[` always begin an edge pattern's full
//! form, so a bracketed path pattern after the abbreviations `-`, `~`,
//! `<-` and `<~` needs a node pattern between them; `->`, `<->` and `~>`
//! before `[` are abbreviations.

use super::super::Parser;
use crate::ast::{EdgePattern, ElementPattern, LabelExpression, Orientation, PropertySpec, Quantifier, QuantifierSugar};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

impl Parser {
    /// Whether "(" here begins a node pattern rather than a parenthesized
    /// path pattern: ")" follows it, or IS, ":", WHERE or "{", or a
    /// variable and then one of those.
    pub(super) fn at_node_filler(&self) -> bool {
        let filler = |n: usize| {
            matches!(self.peek_at(n), Tok::KeyWord(kw::IS | kw::WHERE))
                || matches!(self.peek_at(n), Tok::Delimiter(d) if [delim::RIGHT_PAREN, delim::COLON, delim::LEFT_BRACE].contains(d))
        };
        filler(1) || (matches!(self.peek_at(1), Tok::Identifier(_)) && filler(2))
    }

    /// [P1 §4.1] A node pattern: ( `<element pattern filler>` ).
    pub(super) fn node_pattern(&mut self) -> Result<ElementPattern> {
        let at = self.here();
        self.expect_delim(delim::LEFT_PAREN)?;
        let filler = self.element_filler(at)?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(filler)
    }

    /// [P1 §4.1] `<element pattern filler>` ::= \[`<variable>`\] \[{IS |
    /// :} `<label expression>`\] then \[WHERE `<search condition>`\] and
    /// \[EXTENSION\] \[{ `<property name>` : `<value expression>` \[, ...\]
    /// }\], either first.
    fn element_filler(&mut self, at: usize) -> Result<ElementPattern> {
        let variable = if matches!(self.peek(), Tok::Identifier(_)) { Some(self.graph_variable()?) } else { None };
        let label = if self.accept_kw(kw::IS) || self.accept_delim(delim::COLON) { Some(self.label_expression()?) } else { None };
        let mut properties = if self.is_delim(delim::LEFT_BRACE) { self.property_specification()? } else { Vec::new() };
        let where_clause = if self.accept_kw(kw::WHERE) { Some(self.search_condition()?) } else { None };
        if properties.is_empty() && self.is_delim(delim::LEFT_BRACE) {
            properties = self.property_specification()?;
        }
        Ok(ElementPattern { variable, label, where_clause, properties, at })
    }

    /// \[EXTENSION\] { `<property name>` : `<value expression>` \[, ...\] }.
    pub(super) fn property_specification(&mut self) -> Result<Vec<PropertySpec>> {
        self.expect_delim(delim::LEFT_BRACE)?;
        let mut properties = Vec::new();
        loop {
            let at = self.here();
            let name = self.identifier()?;
            self.expect_delim(delim::COLON)?;
            properties.push(PropertySpec { name, value: self.value_expression()?, at });
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_BRACE)?;
        Ok(properties)
    }

    /// [P1 §4.1] `<label expression>`: `<label term>` \[| ...\].
    pub(super) fn label_expression(&mut self) -> Result<LabelExpression> {
        self.nested(|p| {
            let mut left = p.label_term()?;
            while p.accept_delim(delim::VERTICAL_BAR) {
                p.deepen()?;
                let right = p.label_term()?;
                left = LabelExpression::Disjunction(Box::new(left), Box::new(right));
            }
            Ok(left)
        })
    }

    /// `<label term>`: `<label factor>` \[& ...\].
    fn label_term(&mut self) -> Result<LabelExpression> {
        let mut left = self.label_factor()?;
        while self.accept_delim(delim::AMPERSAND) {
            self.deepen()?;
            let right = self.label_factor()?;
            left = LabelExpression::Conjunction(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// `<label factor>`: ! `<label factor>`, a label, %, or ( `<label
    /// expression>` ).
    fn label_factor(&mut self) -> Result<LabelExpression> {
        let at = self.here();
        if self.accept_delim(delim::EXCLAMATION_MARK) {
            return self.nested(|p| p.label_factor().map(|l| LabelExpression::Negation(Box::new(l))));
        }
        if self.accept_delim(delim::PERCENT) {
            return Ok(LabelExpression::Wildcard(at));
        }
        if self.accept_delim(delim::LEFT_PAREN) {
            let inner = self.label_expression()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(inner);
        }
        match self.identifier() {
            Ok(label) => Ok(LabelExpression::Label(label, at)),
            Err(_) => Err(self.syntax("[EXTENSION] expected a label, %, ! or (")),
        }
    }

    /// [P1 §4.1, Fig. 5] An edge pattern: its full form, `<-[` ... `]-`
    /// and the others, or its abbreviation.
    pub(super) fn edge_pattern(&mut self) -> Result<EdgePattern> {
        let at = self.here();
        let opening = self.edge_opening()?;
        let arrow = matches!(opening, Opening::Arrow | Opening::LeftArrow);
        if arrow || !self.is_delim(delim::LEFT_BRACKET) {
            let orientation = match opening {
                Opening::LeftMinus => Orientation::Left,
                Opening::LeftTilde => Orientation::LeftOrUndirected,
                Opening::Tilde => self.tilde_end(Orientation::Undirected, Orientation::UndirectedOrRight),
                Opening::Minus => Orientation::Any,
                Opening::Arrow => Orientation::Right,
                Opening::LeftArrow => Orientation::LeftOrRight,
            };
            let filler = ElementPattern { variable: None, label: None, where_clause: None, properties: Vec::new(), at };
            return Ok(EdgePattern { orientation, abbreviated: true, filler });
        }
        self.advance();
        let filler = self.element_filler(at)?;
        self.expect_delim(delim::RIGHT_BRACKET)?;
        let orientation = self.edge_closing(opening)?;
        Ok(EdgePattern { orientation, abbreviated: false, filler })
    }

    /// What opens an edge pattern, taken: "<-", "<~", "<->", "~", "-" or
    /// "->".
    fn edge_opening(&mut self) -> Result<Opening> {
        let opening = if self.accept_delim(delim::LESS) {
            if self.accept_delim(delim::MINUS) {
                Opening::LeftMinus
            } else if self.accept_delim(delim::TILDE) {
                Opening::LeftTilde
            } else if self.accept_delim(delim::RIGHT_ARROW) {
                Opening::LeftArrow
            } else {
                return Err(self.syntax("[EXTENSION] expected \"-\", \"~\" or \"->\" after \"<\" of an edge pattern"));
            }
        } else if self.accept_delim(delim::EUCLIDEAN_DISTANCE) {
            // "<->" is one token, as the vector distance operator.
            Opening::LeftArrow
        } else if self.accept_delim(delim::TILDE) {
            Opening::Tilde
        } else if self.accept_delim(delim::MINUS) {
            Opening::Minus
        } else if self.accept_delim(delim::RIGHT_ARROW) {
            Opening::Arrow
        } else {
            return Err(self.syntax("[EXTENSION] expected an edge pattern"));
        };
        Ok(opening)
    }

    /// The orientation of a full edge pattern opened by `opening`, from
    /// what closes it after "]", taken.
    fn edge_closing(&mut self, opening: Opening) -> Result<Orientation> {
        let (minus, arrow) = match opening {
            Opening::LeftMinus => (Some(Orientation::Left), Some(Orientation::LeftOrRight)),
            Opening::Minus => (Some(Orientation::Any), Some(Orientation::Right)),
            Opening::LeftTilde => {
                self.expect_delim(delim::TILDE)?;
                return Ok(Orientation::LeftOrUndirected);
            }
            Opening::Tilde => {
                self.expect_delim(delim::TILDE)?;
                return Ok(self.tilde_end(Orientation::Undirected, Orientation::UndirectedOrRight));
            }
            Opening::Arrow | Opening::LeftArrow => (None, None),
        };
        if self.accept_delim(delim::MINUS)
            && let Some(o) = minus
        {
            return Ok(o);
        }
        if self.accept_delim(delim::RIGHT_ARROW)
            && let Some(o) = arrow
        {
            return Ok(o);
        }
        Err(self.syntax("[EXTENSION] expected \"-\" or \"->\" closing the edge pattern"))
    }

    /// After a "~" that ends an edge pattern's form: `pointed` if ">"
    /// follows, taken, else `plain`.
    fn tilde_end(&mut self, plain: Orientation, pointed: Orientation) -> Orientation {
        if self.accept_delim(delim::GREATER) { pointed } else { plain }
    }

    /// [P1 §4.4, Fig. 6] `<quantifier>`, if written: *, +, ?, { n }, {
    /// n , }, { , m } or { n , m }, the most at least the least and at
    /// least 1.
    pub(super) fn quantifier(&mut self) -> Result<Option<Quantifier>> {
        let at = self.here();
        let sugar = |lower, upper, s| Ok(Some(Quantifier { lower, upper, sugar: Some(s), at }));
        if self.accept_delim(delim::ASTERISK) {
            return sugar(0, None, QuantifierSugar::Star);
        }
        if self.accept_delim(delim::PLUS) {
            return sugar(1, None, QuantifierSugar::Plus);
        }
        if self.accept_delim(delim::QUESTION) {
            return sugar(0, Some(1), QuantifierSugar::Question);
        }
        if !self.accept_delim(delim::LEFT_BRACE) {
            return Ok(None);
        }
        let lower = if self.is_delim(delim::COMMA) { 0 } else { self.unsigned_integer()? };
        let upper = if !self.accept_delim(delim::COMMA) {
            Some(lower)
        } else if self.is_delim(delim::RIGHT_BRACE) {
            None
        } else {
            Some(self.unsigned_integer()?)
        };
        self.expect_delim(delim::RIGHT_BRACE)?;
        if upper.is_some_and(|u| u < lower || u == 0) {
            let msg = "[EXTENSION] [P1 §4.4] a quantifier's upper bound is at least its lower bound and at least 1";
            return Err(SqlError::at(Code::SyntaxRule, at, msg));
        }
        Ok(Some(Quantifier { lower, upper, sugar: None, at }))
    }
}

/// What an edge pattern begins with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opening {
    /// "<-".
    LeftMinus,
    /// "<~".
    LeftTilde,
    /// "<->", abbreviated only.
    LeftArrow,
    /// "~".
    Tilde,
    /// "-".
    Minus,
    /// "->", abbreviated only.
    Arrow,
}

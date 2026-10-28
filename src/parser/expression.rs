//! The expression grammar of the parser: `<search condition>` and its
//! predicates [X3.135-1986 §5.10-§5.18; ISO/IEC 9075:1992 §8], and `<value
//! expression>` [X3.135-1986 §5.9; ISO/IEC 9075:1992 §6.11] down to its
//! primaries (`parser/primary.rs`). Split from parser.rs (RULES.md
//! R-SIZE-2); the methods extend the same `Parser`.

use super::{Parser, furthest};
use crate::ast::{Predicate, RowValue, SearchCondition, Sign, Subquery, TruthValue, ValueExpression};
use crate::catalog::MatchType;
use crate::error::Result;
use crate::functions::CONCATENATION;
use crate::lexer::{Tok, delim, kw};
use crate::value::{ArithOp, CompOp};

impl Parser {
    // =====================================================================
    // [X3.135-1986 §5.18] <search condition>
    // =====================================================================

    /// [X3.135-1986 §5.18] `<search condition>` ::= `<boolean term>` | `<search condition>` OR `<boolean term>`
    pub(super) fn search_condition(&mut self) -> Result<SearchCondition> {
        self.nested(|p| {
            let mut left = p.boolean_term()?;
            while p.accept_kw(kw::OR) {
                p.deepen()?;
                let right = p.boolean_term()?;
                left = SearchCondition::Or(Box::new(left), Box::new(right));
            }
            Ok(left)
        })
    }

    /// [X3.135-1986 §5.18] `<boolean term>` ::= `<boolean factor>` | `<boolean term>` AND `<boolean factor>`
    fn boolean_term(&mut self) -> Result<SearchCondition> {
        self.nested(|p| {
            let mut left = p.boolean_factor()?;
            while p.accept_kw(kw::AND) {
                p.deepen()?;
                let right = p.boolean_factor()?;
                left = SearchCondition::And(Box::new(left), Box::new(right));
            }
            Ok(left)
        })
    }

    /// [X3.135-1986 §5.18] `<boolean factor>` ::= \[NOT\] `<boolean test>`
    fn boolean_factor(&mut self) -> Result<SearchCondition> {
        if self.accept_kw(kw::NOT) { Ok(SearchCondition::Not(Box::new(self.boolean_test()?))) } else { self.boolean_test() }
    }

    /// [ISO/IEC 9075:1992 §8.12] `<boolean test>` ::= `<boolean primary>`
    /// \[IS \[NOT\] `<truth value>`\]
    fn boolean_test(&mut self) -> Result<SearchCondition> {
        let primary = self.boolean_primary()?;
        if !self.accept_kw(kw::IS) {
            return Ok(primary);
        }
        let not = self.accept_kw(kw::NOT);
        // <truth value> ::= TRUE | FALSE | UNKNOWN
        let value = if self.accept_kw(kw::TRUE) {
            TruthValue::True
        } else if self.accept_kw(kw::FALSE) {
            TruthValue::False
        } else {
            self.expect_kw(kw::UNKNOWN)?;
            TruthValue::Unknown
        };
        Ok(SearchCondition::Is(Box::new(primary), value, not))
    }

    /// [X3.135-1986 §5.18] `<boolean primary>` ::= `<predicate>` | (`<search condition>`)
    ///
    /// "(A + 1) = 2" is a `<predicate>`; "(A = 1 OR B = 2)" is a parenthesized
    /// `<search condition>`. Try `<predicate>` first; on failure, backtrack.
    fn boolean_primary(&mut self) -> Result<SearchCondition> {
        let start = self.pos;
        let at = self.here();
        let first = match self.predicate() {
            Ok(p) => return Ok(SearchCondition::Predicate(Box::new(p), at)),
            Err(e) => e,
        };
        self.pos = start;
        if !self.accept_delim(delim::LEFT_PAREN) {
            return Err(first);
        }
        let second = self.search_condition().and_then(|c| {
            self.expect_delim(delim::RIGHT_PAREN)?;
            Ok(c)
        });
        second.map_err(|e| furthest(first, e))
    }

    /// [ISO/IEC 9075:1992 §8.1] `<predicate>` ::= `<comparison predicate>` |
    /// `<between predicate>` | `<in predicate>` | `<like predicate>` |
    /// `<null predicate>` | `<quantified comparison predicate>` | `<exists
    /// predicate>` | `<unique predicate>` | `<match predicate>`
    ///
    /// Not implemented: `<overlaps predicate>`.
    fn predicate(&mut self) -> Result<Predicate> {
        // [§8.8] <exists predicate> ::= EXISTS <table subquery>
        if self.accept_kw(kw::EXISTS) {
            return Ok(Predicate::Exists(Box::new(self.subquery()?)));
        }
        // [§8.9] <unique predicate> ::= UNIQUE <table subquery>
        if self.accept_kw(kw::UNIQUE) {
            return Ok(Predicate::Unique(Box::new(self.subquery()?)));
        }
        let value = self.row_value()?;
        if self.accept_kw(kw::IS) {
            // [§8.6] <null predicate> ::= <row value constructor> IS [NOT] NULL
            let not = self.accept_kw(kw::NOT);
            self.expect_kw(kw::NULL)?;
            return Ok(Predicate::Null { value, not });
        }
        if self.accept_kw(kw::MATCH) {
            // [§8.10] <match predicate> ::= <row value constructor> MATCH [UNIQUE] [PARTIAL | FULL] <table subquery>
            let unique = self.accept_kw(kw::UNIQUE);
            let match_type = if self.is_kw(kw::PARTIAL) || self.is_kw(kw::FULL) { self.match_type()? } else { MatchType::Simple };
            return Ok(Predicate::Match { value, unique, match_type, subquery: Box::new(self.subquery()?) });
        }
        let not = self.accept_kw(kw::NOT);
        if self.accept_kw(kw::BETWEEN) {
            // [§8.3] ... [NOT] BETWEEN <row value constructor> AND <row value constructor>
            let low = self.row_value()?;
            self.expect_kw(kw::AND)?;
            return Ok(Predicate::Between { value, low, high: self.row_value()?, not });
        }
        if self.accept_kw(kw::IN) {
            return self.in_predicate(value, not);
        }
        if self.accept_kw(kw::LIKE) {
            return self.like(value, not);
        }
        if not {
            return Err(self.syntax("expected BETWEEN, IN or LIKE after NOT"));
        }
        let op = self.comp_op()?;
        // [§8.7] <quantifier> ::= ALL | SOME | ANY
        if self.is_kw(kw::ANY) || self.is_kw(kw::SOME) || self.is_kw(kw::ALL) {
            let all = self.is_kw(kw::ALL);
            self.advance();
            return Ok(Predicate::Quantified { value, op, all, subquery: Box::new(self.subquery()?) });
        }
        // [§8.2] <comparison predicate> ::= <row value constructor> <comp op> <row value constructor>
        Ok(Predicate::Comparison(value, op, self.row_value()?))
    }

    /// [ISO/IEC 9075:1992 §7.1] `<row value constructor>` ::= `<row value
    /// constructor element>` | (`<row value constructor list>`) | `<row
    /// subquery>`. A list has two elements at least: one in parentheses is
    /// a `<value expression>`. A `<row subquery>` is read as a `<scalar
    /// subquery>`, a row of the subquery's degree (`binder/predicates.rs`).
    ///
    /// Not implemented: NULL and DEFAULT as elements outside a VALUES list.
    fn row_value(&mut self) -> Result<RowValue> {
        let start = self.pos;
        if let Some(list) = self.row_list()? {
            return Ok(list);
        }
        self.pos = start;
        Ok(vec![self.value_expression()?])
    }

    /// (`<row value constructor list>`), if a list of two elements or more
    /// begins here; else none, the position left anywhere.
    fn row_list(&mut self) -> Result<Option<RowValue>> {
        if !self.accept_delim(delim::LEFT_PAREN) || self.is_kw(kw::SELECT) {
            return Ok(None);
        }
        let Ok(first) = self.value_expression() else { return Ok(None) };
        if !self.accept_delim(delim::COMMA) {
            return Ok(None);
        }
        let mut values = vec![first, self.value_expression()?];
        while self.accept_delim(delim::COMMA) {
            values.push(self.value_expression()?);
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(Some(values))
    }

    /// [X3.135-1986 §5.11] `<comp op>`, taken.
    fn comp_op(&mut self) -> Result<CompOp> {
        let op = match self.peek() {
            Tok::Delimiter(delim::EQUALS) => CompOp::Eq,
            Tok::Delimiter(delim::NOT_EQUALS) => CompOp::Ne,
            Tok::Delimiter(delim::LESS) => CompOp::Lt,
            Tok::Delimiter(delim::GREATER) => CompOp::Gt,
            Tok::Delimiter(delim::LESS_OR_EQUAL) => CompOp::Le,
            Tok::Delimiter(delim::GREATER_OR_EQUAL) => CompOp::Ge,
            Tok::KeyWord(_)
            | Tok::Identifier(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BitString(_)
            | Tok::Delimiter(_)
            | Tok::End => return Err(self.syntax("expected a <comp op>")),
        };
        self.advance();
        Ok(op)
    }

    /// Whether a `<subquery>` begins here: "(", then a `<query
    /// expression>`, which begins with SELECT, VALUES or TABLE after any
    /// parentheses of its own. Under more than one parenthesis, it is a
    /// query only if what closes the first of them is followed by a set
    /// operator or another ")": "((SELECT ...) + 1)" is a `<value
    /// expression>`.
    pub(super) fn at_subquery(&self) -> bool {
        if !self.is_delim(delim::LEFT_PAREN) {
            return false;
        }
        let open =
            (1..self.toks.len().saturating_add(1)).find(|&n| !matches!(self.peek_at(n), Tok::Delimiter(delim::LEFT_PAREN))).unwrap_or(1);
        if !matches!(self.peek_at(open), Tok::KeyWord(kw::SELECT) | Tok::KeyWord(kw::VALUES) | Tok::KeyWord(kw::TABLE)) {
            return false;
        }
        if open == 1 {
            return true;
        }
        let mut depth = 1_usize;
        for n in open..self.toks.len().saturating_add(1) {
            match self.peek_at(n) {
                Tok::Delimiter(delim::LEFT_PAREN) => depth = depth.saturating_add(1),
                Tok::Delimiter(delim::RIGHT_PAREN) => depth = depth.saturating_sub(1),
                Tok::End => return false,
                Tok::KeyWord(_)
                | Tok::Identifier(_)
                | Tok::ExactNumeric(_)
                | Tok::ApproximateNumeric(_)
                | Tok::CharacterString(_)
                | Tok::BitString(_)
                | Tok::Delimiter(_) => {}
            }
            if depth == 0 {
                return matches!(
                    self.peek_at(n.saturating_add(1)),
                    Tok::KeyWord(kw::UNION) | Tok::KeyWord(kw::EXCEPT) | Tok::KeyWord(kw::INTERSECT) | Tok::Delimiter(delim::RIGHT_PAREN)
                );
            }
        }
        false
    }

    /// [X3.135-1986 §5.24; ISO/IEC 9075:1992 §7.11] `<subquery>` ::=
    /// (`<query expression>`). It counts one level toward the nesting
    /// limit, as a nested condition does.
    pub(super) fn subquery(&mut self) -> Result<Subquery> {
        let at = self.here();
        self.nested(|p| {
            p.expect_delim(delim::LEFT_PAREN)?;
            let query = p.query_expression()?;
            p.expect_delim(delim::RIGHT_PAREN)?;
            Ok(Subquery { query, at })
        })
    }

    /// [ISO/IEC 9075:1992 §8.4] the rest of an `<in predicate>`, after
    /// \[NOT\] IN: a `<table subquery>`, or (`<in value list>`), two or more
    /// `<value expression>`s.
    fn in_predicate(&mut self, value: RowValue, not: bool) -> Result<Predicate> {
        if self.at_subquery() {
            return Ok(Predicate::InSubquery { value, subquery: Box::new(self.subquery()?), not });
        }
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut list = vec![self.value_expression()?];
        while self.accept_delim(delim::COMMA) {
            list.push(self.value_expression()?);
        }
        if list.len() < 2 {
            return Err(self.syntax("[ISO/IEC 9075:1992 §8.4 F] an <in value list> has two or more <value expression>s"));
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(Predicate::InList { value, list, not })
    }

    /// [ISO/IEC 9075:1992 §8.5] the rest of a `<like predicate>`, after
    /// \[NOT\] LIKE: `<pattern>` \[ESCAPE `<escape character>`\], each, and
    /// the `<match value>`, a `<character value expression>`.
    fn like(&mut self, mut values: RowValue, not: bool) -> Result<Predicate> {
        let (Some(value), None) = (values.pop(), values.pop()) else {
            return Err(self.syntax("[ISO/IEC 9075:1992 §8.5 F] the <match value> of LIKE is one value, not a row"));
        };
        let pattern = Box::new(self.value_expression()?);
        let escape = if self.accept_kw(kw::ESCAPE) { Some(Box::new(self.value_expression()?)) } else { None };
        Ok(Predicate::Like { value: Box::new(value), pattern, escape, not })
    }

    // =====================================================================
    // [X3.135-1986 §5.9] <value expression>
    // =====================================================================

    /// [X3.135-1986 §5.9] `<value expression>` ::= `<term>` | `<value
    /// expression>` + `<term>` | `<value expression>` - `<term>`; and
    /// [ISO/IEC 9075:1992 §6.13] `<concatenation>` ::= `<character value
    /// expression>` || `<character factor>`
    pub(super) fn value_expression(&mut self) -> Result<ValueExpression> {
        self.nested(|p| {
            let mut left = p.term()?;
            loop {
                let at = p.here();
                if p.accept_delim(delim::CONCATENATION) {
                    p.deepen()?;
                    let right = p.term()?;
                    left = ValueExpression::Function { name: CONCATENATION, args: vec![left, right], at };
                    continue;
                }
                let op = if p.is_delim(delim::PLUS) {
                    ArithOp::Add
                } else if p.is_delim(delim::MINUS) {
                    ArithOp::Sub
                } else {
                    break;
                };
                p.advance();
                p.deepen()?;
                let right = p.term()?;
                left = ValueExpression::Dyadic(Box::new(left), op, Box::new(right));
            }
            Ok(left)
        })
    }

    /// [X3.135-1986 §5.9] `<term>` ::= `<factor>` | `<term>` * `<factor>` | `<term>` / `<factor>`
    fn term(&mut self) -> Result<ValueExpression> {
        self.nested(|p| {
            let mut left = p.factor()?;
            loop {
                let op = if p.is_delim(delim::ASTERISK) {
                    ArithOp::Mul
                } else if p.is_delim(delim::SOLIDUS) {
                    ArithOp::Div
                } else {
                    break;
                };
                p.advance();
                p.deepen()?;
                let right = p.factor()?;
                left = ValueExpression::Dyadic(Box::new(left), op, Box::new(right));
            }
            Ok(left)
        })
    }

    /// [X3.135-1986 §5.9] `<factor>` ::= \[+ | -\] `<primary>`
    fn factor(&mut self) -> Result<ValueExpression> {
        // A sign touching a number is part of that <literal>, not an operator.
        if (self.is_delim(delim::PLUS) || self.is_delim(delim::MINUS)) && !self.sign_attached() {
            let sign = if self.is_delim(delim::MINUS) { Sign::Minus } else { Sign::Plus };
            self.advance();
            // [X3.135-1986 §5.9 SR2] the first character of the token after a monadic
            // operator shall not be a plus or minus sign.
            if self.is_delim(delim::PLUS) || self.is_delim(delim::MINUS) {
                return Err(self.rule("[X3.135-1986 §5.9 SR2] the token after a monadic operator shall not begin with + or -"));
            }
            return Ok(ValueExpression::Monadic(sign, Box::new(self.primary()?)));
        }
        self.primary()
    }
}

//! The expression grammar of the parser: `<search condition>` and its
//! predicates [X3.135-1986 §5.10-§5.18; ISO/IEC 9075:1992 §8], and `<value
//! expression>` [X3.135-1986 §5.9; ISO/IEC 9075:1992 §6.11] down to its
//! primaries (`parser/primary.rs`).

use super::{Parser, furthest};
use crate::ast::{Predicate, RowValue, SearchCondition, Sign, Subquery, TruthValue, ValueExpression, WhenOperand};
use crate::catalog::MatchType;
use crate::error::Result;
use crate::functions::{CONCATENATION, FLAG, LIKE_REGEX};
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

    /// [X3.135-1986 §5.18] `<boolean primary>` ::= `<predicate>` | (`<search
    /// condition>`); or [ISO/IEC 9075-2:2023 §6.46] a `<boolean predicand>`.
    ///
    /// "(A + 1) = 2" is a `<predicate>`; "(A = 1 OR B = 2)" is a parenthesized
    /// `<search condition>`. Try `<predicate>` first; on failure, backtrack.
    fn boolean_primary(&mut self) -> Result<SearchCondition> {
        let start = self.pos;
        let first = match self.predicate() {
            Ok(c) => return Ok(c),
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
    /// predicate>` | `<unique predicate>` | `<match predicate>` | `<overlaps
    /// predicate>`; or [ISO/IEC 9075-2:2023 §6.46] a `<boolean predicand>`,
    /// one value that no predicate's operator follows.
    fn predicate(&mut self) -> Result<SearchCondition> {
        let at = self.here();
        let predicate = |p: Predicate| SearchCondition::Predicate(Box::new(p), at);
        // [§8.8] <exists predicate> ::= EXISTS <table subquery>
        if self.accept_kw(kw::EXISTS) {
            return Ok(predicate(Predicate::Exists(Box::new(self.subquery()?))));
        }
        // [ISO/IEC 9075-2:2023 §8.11] <unique predicate> ::= UNIQUE
        // [ <unique null treatment> ] <table subquery>
        if self.accept_kw(kw::UNIQUE) {
            let nulls_distinct = self.unique_null_treatment()?.unwrap_or(crate::ast::IMPLICIT_NULLS_DISTINCT);
            return Ok(predicate(Predicate::Unique { subquery: Box::new(self.subquery()?), nulls_distinct }));
        }
        let value = self.row_value()?;
        if self.at_null_test() {
            // [§8.6] <null predicate> ::= <row value constructor> IS [NOT] NULL
            self.advance();
            let not = self.accept_kw(kw::NOT);
            self.expect_kw(kw::NULL)?;
            return Ok(predicate(Predicate::Null { value, not }));
        }
        let row = match (self.at_predicate_operator(), <[ValueExpression; 1]>::try_from(value)) {
            (false, Ok([v])) => return Ok(SearchCondition::Predicand(Box::new(v), at)),
            (true, Ok(one)) => Vec::from(one),
            (false | true, Err(row)) => row,
        };
        self.predicate_rest(row).map(predicate)
    }

    /// [ISO/IEC 9075-2:2023 §6.12] A `<when operand>` of a simple CASE
    /// whose `<case operand>` is `operand`: a predicate's second part
    /// [Feature F262], read as the predicate with `operand` its first
    /// [SR2f ii]; else a `<row value predicand>` [SR2f i].
    pub(super) fn when_operand(&mut self, operand: &RowValue) -> Result<WhenOperand> {
        let at = self.here();
        let predicate = |p: Predicate| WhenOperand::Condition(SearchCondition::Predicate(Box::new(p), at));
        if self.at_null_test() {
            self.advance();
            let not = self.accept_kw(kw::NOT);
            self.expect_kw(kw::NULL)?;
            return Ok(predicate(Predicate::Null { value: operand.clone(), not }));
        }
        if self.at_predicate_operator() {
            return self.predicate_rest(operand.clone()).map(predicate);
        }
        Ok(WhenOperand::Value(self.row_value()?, at))
    }

    /// [ISO/IEC 9075-2:2023 §6.12] A `<case operand>`: a `<row value
    /// predicand>`, or an `<overlaps predicate part 1>`, which is one too.
    pub(super) fn case_operand(&mut self) -> Result<RowValue> {
        self.row_value()
    }

    /// Whether IS \[NOT\] NULL is next, IS \[NOT\] TRUE, FALSE or UNKNOWN
    /// being a `<boolean test>` of a predicand [ISO/IEC 9075-2:2023 §6.46].
    fn at_null_test(&self) -> bool {
        let null_at = |n: usize| matches!(self.peek_at(n), Tok::KeyWord(kw::NULL));
        self.is_kw(kw::IS) && (null_at(1) || (matches!(self.peek_at(1), Tok::KeyWord(kw::NOT)) && null_at(2)))
    }

    /// Whether what follows a row value makes it a predicate's: MATCH,
    /// NOT, BETWEEN, IN, LIKE, LIKE_REGEX, OVERLAPS or a `<comp op>`.
    fn at_predicate_operator(&self) -> bool {
        match self.peek() {
            // [ISO/IEC 9075-2:2023 §8.7] LIKE_REGEX, no key word of SQL-92.
            Tok::Identifier(name) => name == LIKE_REGEX,
            Tok::KeyWord(word) => [kw::MATCH, kw::NOT, kw::BETWEEN, kw::IN, kw::LIKE, kw::OVERLAPS].contains(word),
            Tok::Delimiter(d) => {
                [delim::EQUALS, delim::NOT_EQUALS, delim::LESS, delim::GREATER, delim::LESS_OR_EQUAL, delim::GREATER_OR_EQUAL].contains(d)
            }
            Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::End => false,
        }
    }

    /// The rest of a predicate after its first row value, `value`, but for
    /// a `<null predicate>`.
    fn predicate_rest(&mut self, value: RowValue) -> Result<Predicate> {
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
        if self.accept_word(LIKE_REGEX) {
            return self.like_regex(value, not);
        }
        if not {
            return Err(self.syntax("expected BETWEEN, IN, LIKE or LIKE_REGEX after NOT"));
        }
        // [§8.11] <overlaps predicate>
        if self.accept_kw(kw::OVERLAPS) {
            return Ok(Predicate::Overlaps(value, self.row_value()?));
        }
        let op = self.comp_op()?;
        // [§8.7] <quantifier> ::= ALL | SOME | ANY
        if self.at_quantifier() {
            let all = self.is_kw(kw::ALL);
            self.advance();
            return Ok(Predicate::Quantified { value, op, all, subquery: Box::new(self.subquery()?) });
        }
        // [§8.2] <comparison predicate> ::= <row value constructor> <comp op> <row value constructor>
        Ok(Predicate::Comparison(value, op, self.row_value()?))
    }

    /// Whether a `<quantifier>` is next: ALL, or SOME or ANY before a
    /// subquery, either being of SQL:2023 a set function too [ISO/IEC
    /// 9075-2:2023 §10.9].
    fn at_quantifier(&mut self) -> bool {
        if self.is_kw(kw::ALL) {
            return true;
        }
        if !self.is_kw(kw::ANY) && !self.is_kw(kw::SOME) {
            return false;
        }
        let start = self.pos;
        self.advance();
        let subquery = self.at_subquery();
        self.pos = start;
        subquery
    }

    /// [ISO/IEC 9075:1992 §7.1] `<row value constructor>` ::= `<row value
    /// constructor element>` | (`<row value constructor list>`) | `<row
    /// subquery>`. A list has two elements at least: one in parentheses is
    /// a `<value expression>`. A `<row subquery>` is read as a `<scalar
    /// subquery>`, a row of the subquery's degree (`binder/predicates.rs`).
    /// NULL and DEFAULT are elements only of an INSERT's VALUES [SR1].
    fn row_value(&mut self) -> Result<RowValue> {
        let start = self.pos;
        if let Some(list) = self.row_list()? {
            return Ok(list);
        }
        self.pos = start;
        Ok(vec![self.predicate_element()?])
    }

    /// A `<row value constructor element>` of a predicate's row: a `<value
    /// expression>`. NULL and DEFAULT are elements only of a query
    /// expression simply contained in an INSERT [ISO/IEC 9075:1992 §7.1
    /// SR1], and refused here (SQLCODE -103).
    fn predicate_element(&mut self) -> Result<ValueExpression> {
        if self.is_kw(kw::NULL) || self.is_kw(kw::DEFAULT) {
            return Err(self.rule("[ISO/IEC 9075:1992 §7.1 SR1] NULL and DEFAULT are row values of an INSERT's query expression alone"));
        }
        self.common_value_expression()
    }

    /// (`<row value constructor list>`), if a list of two elements or more
    /// begins here; else none, the position left anywhere.
    fn row_list(&mut self) -> Result<Option<RowValue>> {
        if !self.accept_delim(delim::LEFT_PAREN) || self.is_kw(kw::SELECT) {
            return Ok(None);
        }
        if self.is_kw(kw::NULL) || self.is_kw(kw::DEFAULT) {
            return self.predicate_element().map(|_| None);
        }
        let Ok(first) = self.common_value_expression() else { return Ok(None) };
        if !self.accept_delim(delim::COMMA) {
            return Ok(None);
        }
        let mut values = vec![first, self.predicate_element()?];
        while self.accept_delim(delim::COMMA) {
            values.push(self.predicate_element()?);
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
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
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
                | Tok::BinaryString(_)
                | Tok::Introducer(_)
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
        let mut list = vec![self.common_value_expression()?];
        while self.accept_delim(delim::COMMA) {
            list.push(self.common_value_expression()?);
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
        let pattern = Box::new(self.common_value_expression()?);
        let escape = if self.accept_kw(kw::ESCAPE) { Some(Box::new(self.common_value_expression()?)) } else { None };
        Ok(Predicate::Like { value: Box::new(value), pattern, escape, not })
    }

    /// [ISO/IEC 9075-2:2023 §8.7] the rest of a `<regex like predicate>`,
    /// after \[NOT\] LIKE_REGEX: `<XQuery pattern>` \[FLAG `<XQuery option
    /// flag>`\], and the value matched, of one value [SR1].
    fn like_regex(&mut self, mut values: RowValue, not: bool) -> Result<Predicate> {
        let (Some(value), None) = (values.pop(), values.pop()) else {
            return Err(self.rule("[ISO/IEC 9075-2:2023 §8.7 SR1] the value LIKE_REGEX matches is one value, not a row"));
        };
        let pattern = Box::new(self.common_value_expression()?);
        let flag = if self.accept_word(FLAG) { Some(Box::new(self.common_value_expression()?)) } else { None };
        Ok(Predicate::LikeRegex { value: Box::new(value), pattern, flag, not })
    }

    // =====================================================================
    // [X3.135-1986 §5.9] <value expression>
    // =====================================================================

    /// [ISO/IEC 9075-2:2023 §6.29] `<value expression>` ::= `<common value
    /// expression>` | `<boolean value expression>`: a `<search condition>`
    /// read as a value [§6.46], one alone that is no predicate being the
    /// value itself. Each one read is kept by where it begins, so a
    /// parenthesized one read again as the parser backtracks is not parsed
    /// again, which would take time exponential in its nesting.
    pub(super) fn value_expression(&mut self) -> Result<ValueExpression> {
        let start = self.pos;
        if let Some((read, end, dynamic)) = self.read_values.get(&start).cloned() {
            self.pos = end;
            self.dynamic = self.dynamic.max(dynamic);
            return read;
        }
        let read = self.search_condition().map(|c| match c {
            SearchCondition::Predicand(v, _) => *v,
            SearchCondition::Or(..)
            | SearchCondition::And(..)
            | SearchCondition::Not(_)
            | SearchCondition::Is(..)
            | SearchCondition::Predicate(..) => ValueExpression::Boolean(Box::new(c)),
        });
        self.read_values.insert(start, (read.clone(), self.pos, self.dynamic));
        read
    }

    /// [X3.135-1986 §5.9] `<value expression>` ::= `<term>` | `<value
    /// expression>` + `<term>` | `<value expression>` - `<term>`; and
    /// [ISO/IEC 9075:1992 §6.13] `<concatenation>` ::= `<character value
    /// expression>` || `<character factor>`. Of SQL:2023, a `<common value
    /// expression>` [ISO/IEC 9075-2:2023 §6.29], no truth value.
    pub(super) fn common_value_expression(&mut self) -> Result<ValueExpression> {
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
    pub(super) fn factor(&mut self) -> Result<ValueExpression> {
        // A sign touching a number is part of that <literal>, not an operator.
        if (self.is_delim(delim::PLUS) || self.is_delim(delim::MINUS)) && !self.sign_attached() {
            let sign = if self.is_delim(delim::MINUS) { Sign::Minus } else { Sign::Plus };
            self.advance();
            // [X3.135-1986 §5.9 SR2] the first character of the token after a monadic
            // operator shall not be a plus or minus sign.
            if self.is_delim(delim::PLUS) || self.is_delim(delim::MINUS) {
                return Err(self.rule("[X3.135-1986 §5.9 SR2] the token after a monadic operator shall not begin with + or -"));
            }
            let primary = self.qualified_primary()?;
            let factor = self.time_zone(primary)?;
            let factor = self.collated(factor)?;
            return Ok(ValueExpression::Monadic(sign, Box::new(factor)));
        }
        let primary = self.qualified_primary()?;
        let factor = self.time_zone(primary)?;
        // [ISO/IEC 9075:1992 §6.13] <character factor> ::= <character
        // primary> [<collate clause>]
        self.collated(factor)
    }

    /// `factor`, with the `<collate clause>` that may follow it.
    fn collated(&mut self, factor: ValueExpression) -> Result<ValueExpression> {
        Ok(match self.collate_clause()? {
            Some(collation) => ValueExpression::Collated(Box::new(factor), collation),
            None => factor,
        })
    }

    /// A `<primary>`, with the `<interval qualifier>` that may follow it
    /// [ISO/IEC 9075:1992 §6.15].
    fn qualified_primary(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        let primary = self.primary()?;
        self.qualified(primary, at)
    }
}

//! The rest of each production of the expression grammar after its first
//! operand: OR, AND and IS of a `<search condition>` [X3.135-1986 §5.18;
//! ISO/IEC 9075:1992 §8.12], a predicate's operator and operands
//! [ISO/IEC 9075:1992 §8.1], and the operators, sign, collation and
//! qualifiers of a `<value expression>` [X3.135-1986 §5.9; ISO/IEC
//! 9075:1992 §6.13, §6.15].
//!
//! \[DESIGN\] Each is read apart from the production that reads its first
//! operand, which is where a nesting of expressions recurs: so the frames
//! each level adds to the stack hold none of their locals, and a statement
//! nested to `MAX_NESTING_DEPTH` is read within half the default stack
//! (RULES.md R-CF-6).

use crate::ast::{Composite, PeriodOperand, Predicate, RowValue, SearchCondition, Sign, TruthValue, ValueExpression};
use crate::error::Result;
use crate::functions::CONCATENATION;
use crate::lexer::{delim, kw};
use crate::parser::Parser;
use crate::value::ArithOp;

impl Parser {
    /// [X3.135-1986 §5.18] The rest of a `<search condition>` whose first
    /// `<boolean term>` is `left`: OR and another, each.
    pub(super) fn disjunction(&mut self, mut left: SearchCondition) -> Result<SearchCondition> {
        while self.accept_kw(kw::OR) {
            self.deepen()?;
            let right = self.boolean_term()?;
            left = SearchCondition::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// [X3.135-1986 §5.18] The rest of a `<boolean term>` whose first
    /// `<boolean factor>` is `left`: AND and another, each.
    pub(super) fn conjunction(&mut self, mut left: SearchCondition) -> Result<SearchCondition> {
        while self.accept_kw(kw::AND) {
            self.deepen()?;
            let right = self.boolean_factor()?;
            left = SearchCondition::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// [ISO/IEC 9075:1992 §8.12] The rest of a `<boolean test>` of
    /// `primary`: IS \[NOT\] `<truth value>`, if written.
    pub(super) fn truth_test(&mut self, primary: SearchCondition) -> Result<SearchCondition> {
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

    /// [ISO/IEC 9075:1992 §8.8] `<exists predicate>` ::= EXISTS `<table
    /// subquery>`; or [ISO/IEC 9075-2:2023 §8.11] `<unique predicate>` ::=
    /// UNIQUE \[`<unique null treatment>`\] `<table subquery>`.
    pub(super) fn subquery_predicate(&mut self) -> Result<SearchCondition> {
        let at = self.here();
        let predicate = |p: Predicate| SearchCondition::Predicate(Box::new(p), at);
        if self.accept_kw(kw::EXISTS) {
            return Ok(predicate(Predicate::Exists(Box::new(self.subquery()?))));
        }
        self.expect_kw(kw::UNIQUE)?;
        let nulls_distinct = self.unique_null_treatment()?.unwrap_or(crate::ast::IMPLICIT_NULLS_DISTINCT);
        Ok(predicate(Predicate::Unique { subquery: Box::new(self.subquery()?), nulls_distinct }))
    }

    /// [ISO/IEC 9075:1992 §8.1] The rest of a predicate whose first row
    /// value, begun at `at`, is `value`; or [ISO/IEC 9075-2:2023 §6.46] a
    /// `<boolean predicand>`, one value that no predicate's operator
    /// follows.
    pub(super) fn predicate_of(&mut self, value: RowValue, at: usize) -> Result<SearchCondition> {
        let predicate = |p: Predicate| SearchCondition::Predicate(Box::new(p), at);
        if self.at_is_predicate() {
            return self.is_predicate(value).map(predicate);
        }
        let operators = (self.at_period_operator(), self.at_predicate_operator());
        let row = match (operators, <[ValueExpression; 1]>::try_from(value)) {
            // [ISO/IEC 9075-2:2023 §8.20] a <period predicate> of a <period
            // reference> or other value, which the binder judges; OVERLAPS
            // of two values is the <overlaps predicate> [§8.14].
            ((true, _), Ok([v])) => return self.period_predicate(PeriodOperand::Value(Box::new(v))).map(predicate),
            ((false, false), Ok([v])) => return Ok(SearchCondition::Predicand(Box::new(v), at)),
            ((false, true), Ok(one)) => Vec::from(one),
            // [ISO/IEC 9075-2:2023 §7.1] a row value constructor of no
            // predicate: a value of a row type.
            ((_, false), Err(row)) => {
                let constructed = ValueExpression::Composite(Box::new(Composite::Row(row)), at);
                return Ok(SearchCondition::Predicand(Box::new(constructed), at));
            }
            ((_, true), Err(row)) => row,
        };
        self.predicate_rest(row).map(predicate)
    }

    /// The rest of a (`<row value constructor list>`) whose first element
    /// is `first`, if a comma follows it: the other elements and ")".
    /// Else none, the position left anywhere.
    pub(super) fn row_list_rest(&mut self, first: ValueExpression) -> Result<Option<RowValue>> {
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

    /// [X3.135-1986 §5.9; ISO/IEC 9075:1992 §6.13] The rest of a `<value
    /// expression>` whose first `<term>` is `left`: + or - and another
    /// term, or || and a `<character factor>`, each.
    pub(super) fn additions(&mut self, mut left: ValueExpression) -> Result<ValueExpression> {
        loop {
            let at = self.here();
            if self.accept_delim(delim::CONCATENATION) {
                self.deepen()?;
                let right = self.term()?;
                left = ValueExpression::Function { name: CONCATENATION, args: vec![left, right], at };
                continue;
            }
            let op = if self.is_delim(delim::PLUS) {
                ArithOp::Add
            } else if self.is_delim(delim::MINUS) {
                ArithOp::Sub
            } else {
                return Ok(left);
            };
            self.advance();
            self.deepen()?;
            let right = self.term()?;
            left = ValueExpression::Dyadic(Box::new(left), op, Box::new(right));
        }
    }

    /// [X3.135-1986 §5.9] The rest of a `<term>` whose first `<factor>` is
    /// `left`: * or / and another factor, each.
    pub(super) fn products(&mut self, mut left: ValueExpression) -> Result<ValueExpression> {
        loop {
            let op = if self.is_delim(delim::ASTERISK) {
                ArithOp::Mul
            } else if self.is_delim(delim::SOLIDUS) {
                ArithOp::Div
            } else {
                return Ok(left);
            };
            self.advance();
            self.deepen()?;
            let right = self.factor()?;
            left = ValueExpression::Dyadic(Box::new(left), op, Box::new(right));
        }
    }

    /// [X3.135-1986 §5.9] A `<factor>` of a sign, + or -, apart from the
    /// number it is written before.
    pub(super) fn signed_factor(&mut self) -> Result<ValueExpression> {
        let sign = if self.is_delim(delim::MINUS) { Sign::Minus } else { Sign::Plus };
        self.advance();
        // [X3.135-1986 §5.9 SR2] the first character of the token after a monadic
        // operator shall not be a plus or minus sign.
        if self.is_delim(delim::PLUS) || self.is_delim(delim::MINUS) {
            return Err(self.rule("[X3.135-1986 §5.9 SR2] the token after a monadic operator shall not begin with + or -"));
        }
        let primary = self.qualified_primary()?;
        let factor = self.factor_rest(primary)?;
        Ok(ValueExpression::Monadic(sign, Box::new(factor)))
    }

    /// The rest of a `<factor>` of `primary`: its time zone, then the
    /// `<collate clause>` of a `<character factor>` [ISO/IEC 9075:1992
    /// §6.13].
    pub(super) fn factor_rest(&mut self, primary: ValueExpression) -> Result<ValueExpression> {
        let factor = self.time_zone(primary)?;
        self.collated(factor)
    }

    /// The `<filter clause>` [ISO/IEC 9075-2:2023 §10.9], OVER and
    /// `<interval qualifier>` [ISO/IEC 9075:1992 §6.15] that may follow
    /// `primary`, begun at `at`.
    pub(super) fn qualifiers(&mut self, primary: ValueExpression, at: usize) -> Result<ValueExpression> {
        let primary = self.filtered(primary)?;
        let primary = self.windowed(primary, at)?;
        let primary = self.postfixed(primary)?;
        self.qualified(primary, at)
    }
}

//! Multisets [ISO/IEC 9075-2:2023 §4.12.3]: the `<multiset value
//! constructor>`s [§6.52], `<multiset element reference>` [§6.26], the
//! `<multiset set function>` [§6.51], MULTISET UNION, EXCEPT and
//! INTERSECT [§6.50], and COLLECT, FUSION and INTERSECTION [§10.9]. The
//! predicates of multisets [§8.16 to §8.18] are read with the others
//! (`expression.rs`).
//!
//! \[DESIGN\] None of MULTISET, ELEMENT, MEMBER, SUBMULTISET, COLLECT,
//! FUSION and INTERSECTION is a key word of SQL-92, so each is one only
//! where what follows makes it so (RULES.md D80).

use super::Parser;
use crate::ast::{Composite, SetArg, SetFunctionSpecification, ValueExpression};
use crate::error::Result;
use crate::functions::{COLLECT, ELEMENT, FUSION, INTERSECTION};
use crate::lexer::{Tok, delim, kw};
use crate::value::multisets::BagOp;
use crate::value::types::MULTISET_NAME;

impl Parser {
    /// A `<value expression primary>` of multisets that begins with an
    /// `<identifier>` no key word of SQL-92 names, if one is next: MULTISET
    /// `[`, MULTISET (, ELEMENT (, or COLLECT, FUSION or INTERSECTION (.
    pub(super) fn multiset_primary(&mut self) -> Result<Option<ValueExpression>> {
        let Tok::Identifier(name) = self.peek() else { return Ok(None) };
        let at = self.here();
        let opens = |d: &str| matches!(self.peek_at(1), Tok::Delimiter(x) if *x == d);
        let composite = |c: Composite| Some(ValueExpression::Composite(Box::new(c), at));
        Ok(match name.as_str() {
            // [§6.52] MULTISET [ <multiset element list> ], none of an
            // <empty specification> [§6.5].
            MULTISET_NAME if opens(delim::LEFT_BRACKET) => composite(Composite::Multiset(self.array_elements()?)),
            // [§6.52] MULTISET <table subquery>.
            MULTISET_NAME if opens(delim::LEFT_PAREN) => {
                self.advance();
                composite(Composite::MultisetQuery(Box::new(self.subquery()?), false))
            }
            ELEMENT if opens(delim::LEFT_PAREN) => composite(Composite::MultisetElement(Box::new(self.multiset_operand_of()?))),
            COLLECT | FUSION | INTERSECTION if opens(delim::LEFT_PAREN) => Some(ValueExpression::SetFunction(self.multiset_aggregate()?)),
            _ => None,
        })
    }

    /// The `<value expression primary>` that begins with SET or TABLE, key
    /// words of SQL-92, before (: [§6.51] `<multiset set function>` ::= SET
    /// ( `<multiset value expression>` ), or [§6.52] `<table value
    /// constructor by query>` ::= TABLE `<table subquery>`; else none.
    pub(super) fn keyword_multiset_primary(&mut self) -> Result<Option<ValueExpression>> {
        if !matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN)) {
            return Ok(None);
        }
        let at = self.here();
        let c = if self.is_kw(kw::SET) {
            Composite::MultisetSet(Box::new(self.multiset_operand_of()?))
        } else if self.is_kw(kw::TABLE) {
            self.advance();
            Composite::MultisetQuery(Box::new(self.subquery()?), true)
        } else {
            return Ok(None);
        };
        Ok(Some(ValueExpression::Composite(Box::new(c), at)))
    }

    /// After a function's name: ( `<multiset value expression>` ).
    fn multiset_operand_of(&mut self) -> Result<ValueExpression> {
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let operand = self.nested(Parser::common_value_expression)?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(operand)
    }

    /// [ISO/IEC 9075-2:2023 §10.9] COLLECT, FUSION or INTERSECTION (
    /// \[DISTINCT | ALL\] `<value expression>` ), a `<general set
    /// function>`.
    fn multiset_aggregate(&mut self) -> Result<SetFunctionSpecification> {
        let at = self.here();
        let name = [COLLECT, FUSION].into_iter().find(|n| self.is_word(n)).unwrap_or(INTERSECTION);
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let arg = if self.accept_kw(kw::DISTINCT) {
            SetArg::Distinct(Box::new(self.value_expression()?))
        } else {
            // [§10.9 SR1] ALL, written or implicit.
            self.accept_kw(kw::ALL);
            SetArg::All(Box::new(self.value_expression()?))
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(SetFunctionSpecification { name, arg, order_by: Vec::new(), listagg: None, filter: None, semantics: None, at })
    }

    /// [ISO/IEC 9075-2:2023 §6.50] The rest of a `<multiset value
    /// expression>` whose first operand is `left`: each MULTISET INTERSECT
    /// and its operand first \[`<multiset term>`\], then each MULTISET UNION
    /// or EXCEPT and its `<multiset term>`, left to right.
    pub(super) fn multiset_operations(&mut self, left: ValueExpression) -> Result<ValueExpression> {
        let mut left = self.multiset_intersections(left)?;
        while let Some(op) = self.multiset_operator(false) {
            let (at, distinct) = self.multiset_quantifier()?;
            let first = self.multiset_operand()?;
            let right = self.multiset_intersections(first)?;
            left = operation((op, distinct), left, right, at);
        }
        Ok(left)
    }

    /// The rest of a `<multiset term>` whose first operand is `left`: each
    /// MULTISET INTERSECT and its operand.
    fn multiset_intersections(&mut self, mut left: ValueExpression) -> Result<ValueExpression> {
        while self.multiset_operator(true).is_some() {
            let (at, distinct) = self.multiset_quantifier()?;
            let right = self.multiset_operand()?;
            left = operation((BagOp::Intersect, distinct), left, right, at);
        }
        Ok(left)
    }

    /// An operand of a multiset operator: a `<multiset primary>`, read as a
    /// `<common value expression>` of no multiset operator, as the binder
    /// takes a value of a multiset type alone.
    fn multiset_operand(&mut self) -> Result<ValueExpression> {
        self.nested(Parser::sum)
    }

    /// MULTISET and then INTERSECT, if `intersect`, else UNION or EXCEPT,
    /// if next: its operator, read. It is read only after a whole operand,
    /// where a column named MULTISET never stands, as a select list's
    /// alias is followed by FROM (RULES.md D80).
    fn multiset_operator(&mut self, intersect: bool) -> Option<BagOp> {
        if !self.is_word(MULTISET_NAME) {
            return None;
        }
        let op = match self.peek_at(1) {
            Tok::KeyWord(kw::INTERSECT) if intersect => BagOp::Intersect,
            Tok::KeyWord(kw::UNION) if !intersect => BagOp::Union,
            Tok::KeyWord(kw::EXCEPT) if !intersect => BagOp::Except,
            Tok::Identifier(_)
            | Tok::KeyWord(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::Delimiter(_)
            | Tok::End => return None,
        };
        self.advance();
        self.advance();
        Some(op)
    }

    /// After a multiset operator, its `<set quantifier>`, ALL if none is
    /// written: where it was, and whether DISTINCT. A level deeper.
    fn multiset_quantifier(&mut self) -> Result<(usize, bool)> {
        let at = self.here();
        let distinct = self.accept_kw(kw::DISTINCT);
        if !distinct {
            self.accept_kw(kw::ALL);
        }
        self.deepen()?;
        Ok((at, distinct))
    }
}

/// The multiset operation `op`, DISTINCT if so, of `left` and `right`.
fn operation((op, distinct): (BagOp, bool), left: ValueExpression, right: ValueExpression, at: usize) -> ValueExpression {
    ValueExpression::Composite(Box::new(Composite::MultisetOperation(op, distinct, Box::new(left), Box::new(right))), at)
}

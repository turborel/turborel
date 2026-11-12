//! [ISO/IEC 9075-2:2023 §6.31, §6.33] The regular expression functions:
//! OCCURRENCES_REGEX, POSITION_REGEX, SUBSTRING_REGEX and TRANSLATE_REGEX,
//! each a call of its operands in the order `functions/regex.rs` takes
//! them, those not written their implicit values.

use super::Parser;
use crate::ast::{Literal, ValueExpression, ValueSpecification};
use crate::error::Result;
use crate::functions::{
    AFTER, CHARACTERS, FLAG, OCCURRENCE, OCCURRENCES_REGEX, OCTETS, POSITION_REGEX, START, SUBSTRING_REGEX, TRANSLATE_REGEX,
};
use crate::lexer::{Tok, delim, kw};
use crate::value::{CharUnits, Decimal};

/// A character literal of `s`.
fn characters(s: &str) -> ValueExpression {
    ValueExpression::Value(ValueSpecification::Literal(Literal::Character(s.to_owned())))
}

/// An exact literal of `n`.
fn exact(n: i128) -> ValueExpression {
    ValueExpression::Value(ValueSpecification::Literal(Literal::Exact(Decimal::new(n, 0))))
}

impl Parser {
    /// [ISO/IEC 9075-2:2023 §6.1, §6.31, §6.33] \[USING `<char length
    /// units>`\]: the units, if written.
    pub(super) fn char_length_units(&mut self) -> Result<Option<CharUnits>> {
        if !self.accept_kw(kw::USING) {
            return Ok(None);
        }
        if self.accept_word(CHARACTERS) {
            Ok(Some(CharUnits::Characters))
        } else if self.accept_word(OCTETS) {
            Ok(Some(CharUnits::Octets))
        } else {
            Err(self.syntax("[ISO/IEC 9075-2:2023 §6.1] expected CHARACTERS or OCTETS after USING"))
        }
    }
}

/// What follows the subject, written or implicit.
struct Rest {
    from: ValueExpression,
    units: ValueExpression,
    occurrence: ValueExpression,
    all: bool,
    group: ValueExpression,
}

impl Parser {
    /// `name` ( \[ START | AFTER \] `<XQuery pattern>` \[ FLAG `<XQuery
    /// option flag>` \] IN `<regex subject string>` \[ WITH `<XQuery
    /// replacement string>` \] \[ FROM `<start position>` \] \[ USING
    /// `<char length units>` \] \[ OCCURRENCE `<regex occurrence>` | ALL \]
    /// \[ GROUP `<regex capture group>` \] ), each part where `name` has
    /// it. START or AFTER is that word unless IN or FLAG follows it, so a
    /// column of that name may be the pattern.
    pub(super) fn regex_function(&mut self, name: &'static str) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let side = if name == POSITION_REGEX { self.side() } else { START };
        let pattern = self.common_value_expression()?;
        // [§6.31 SR3e, SR4h; §6.33 SR8e, SR12e]
        let flag = if self.accept_word(FLAG) { self.common_value_expression()? } else { characters("") };
        self.expect_kw(kw::IN)?;
        let subject = self.common_value_expression()?;
        // [§6.33 SR12d]
        let replacement =
            if name == TRANSLATE_REGEX && self.accept_kw(kw::WITH) { self.common_value_expression()? } else { characters("") };
        let rest = self.regex_rest(name)?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let Rest { from, units, occurrence, all, group } = rest;
        let args = if name == OCCURRENCES_REGEX {
            vec![units, pattern, flag, subject, from]
        } else if name == POSITION_REGEX {
            vec![units, characters(side), pattern, flag, subject, from, occurrence, group]
        } else if name == SUBSTRING_REGEX {
            vec![units, pattern, flag, subject, from, occurrence, group]
        } else {
            let every = characters(if all { kw::ALL } else { OCCURRENCE });
            vec![units, every, pattern, flag, subject, replacement, from, occurrence]
        };
        Ok(ValueExpression::Function { name, args, at })
    }

    /// [§6.31 SR4b] START or AFTER, START if neither is written.
    fn side(&mut self) -> &'static str {
        let word = [START, AFTER].into_iter().find(|w| matches!(self.peek(), Tok::Identifier(s) if s == *w));
        let pattern_next = matches!(self.peek_at(1), Tok::KeyWord(kw::IN)) || matches!(self.peek_at(1), Tok::Identifier(s) if s == FLAG);
        let Some(w) = word.filter(|_| !pattern_next) else { return START };
        self.advance();
        w
    }

    /// The start position, units, occurrence and group after the subject
    /// of `name`, each its implicit value if not written: 1, CHARACTERS,
    /// 1 or of TRANSLATE_REGEX ALL, and 0 [§6.31 SR3, SR4; §6.33 SR8,
    /// SR12].
    fn regex_rest(&mut self, name: &str) -> Result<Rest> {
        let from = if self.accept_kw(kw::FROM) { self.common_value_expression()? } else { exact(1) };
        let units = characters(match self.char_length_units()? {
            Some(CharUnits::Octets) => OCTETS,
            Some(CharUnits::Characters) | None => CHARACTERS,
        });
        let translate = name == TRANSLATE_REGEX;
        let (occurrence, all) = if name != OCCURRENCES_REGEX && self.accept_word(OCCURRENCE) {
            if translate && self.accept_kw(kw::ALL) { (exact(1), true) } else { (self.common_value_expression()?, false) }
        } else {
            (exact(1), translate)
        };
        let grouped = name == POSITION_REGEX || name == SUBSTRING_REGEX;
        let group = if grouped && self.accept_kw(kw::GROUP) { self.common_value_expression()? } else { exact(0) };
        Ok(Rest { from, units, occurrence, all, group })
    }
}

//! The expressions of user-defined types and references [ISO/IEC
//! 9075-2:2023 §6.18 to §6.24, §6.40]: method invocations, direct and
//! generalized, static method invocations, NEW, TREAT, DEREF,
//! SPECIFICTYPE, dereference operations and method references; the
//! transform groups of [§6.4]; and the `<type list>` of a `<type
//! predicate>` [§8.19]. Their words are no key words of SQL-92, so each
//! is read as an identifier.

use crate::ast::{StructuredExpression, TypeList, TypeName, ValueExpression};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;

/// NEW, of a `<new specification>` [ISO/IEC 9075-2:2023 §6.20].
const NEW: &str = "NEW";
/// TREAT, of a `<subtype treatment>` [§6.24].
const TREAT: &str = "TREAT";
/// DEREF, of a `<reference resolution>` [§6.23].
const DEREF: &str = "DEREF";
/// SPECIFICTYPE, of a `<specific type method>` [§6.40].
pub(in crate::parser) const SPECIFICTYPE: &str = "SPECIFICTYPE";
/// CURRENT_DEFAULT_TRANSFORM_GROUP [§6.4].
const CURRENT_DEFAULT_TRANSFORM_GROUP: &str = "CURRENT_DEFAULT_TRANSFORM_GROUP";
/// CURRENT_TRANSFORM_GROUP_FOR_TYPE [§6.4].
const CURRENT_TRANSFORM_GROUP_FOR_TYPE: &str = "CURRENT_TRANSFORM_GROUP_FOR_TYPE";

/// `s`, begun at `at`, as a value.
fn structured(s: StructuredExpression, at: usize) -> ValueExpression {
    ValueExpression::Structured(Box::new(s), at)
}

impl Parser {
    /// Whether the word `word` is next, then `next`.
    fn word_before(&self, word: &str, next: &str) -> bool {
        self.is_word(word) && matches!(self.peek_at(1), Tok::Delimiter(d) if *d == next)
    }

    /// A `<value expression primary>` of user-defined types that begins
    /// with an identifier, if one is next: NEW, TREAT ( or DEREF (, a
    /// static method invocation, or a transform group's name.
    pub(in crate::parser) fn structured_primary(&mut self) -> Result<Option<ValueExpression>> {
        let at = self.here();
        let s = if self.at_new() {
            self.advance();
            let udt = self.table_name()?;
            StructuredExpression::New { udt, args: self.sql_argument_list()? }
        } else if self.word_before(TREAT, delim::LEFT_PAREN) {
            self.advance();
            self.expect_delim(delim::LEFT_PAREN)?;
            let subject = self.nested(Parser::value_expression)?;
            self.expect_kw(kw::AS)?;
            let target = self.type_name()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            StructuredExpression::Treat { subject, target }
        } else if self.word_before(DEREF, delim::LEFT_PAREN) {
            self.advance();
            self.expect_delim(delim::LEFT_PAREN)?;
            let subject = self.nested(Parser::value_expression)?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            StructuredExpression::Deref(subject)
        } else if self.is_word(CURRENT_DEFAULT_TRANSFORM_GROUP) {
            self.advance();
            StructuredExpression::CurrentDefaultTransformGroup
        } else if self.is_word(CURRENT_TRANSFORM_GROUP_FOR_TYPE) {
            self.advance();
            StructuredExpression::CurrentTransformGroupForType(self.table_name()?)
        } else if self.at_static_method() {
            self.static_method()?
        } else {
            return Ok(None);
        };
        Ok(Some(structured(s, at)))
    }

    /// Whether a `<new specification>` begins here: NEW, a name, perhaps
    /// of a schema, then (, so NEW and a correlation name after it are
    /// still a column and its name.
    fn at_new(&self) -> bool {
        let name = |n: usize| matches!(self.peek_at(n), Tok::Identifier(_));
        let is = |n: usize, d: &str| matches!(self.peek_at(n), Tok::Delimiter(x) if *x == d);
        let opens = is(2, delim::LEFT_PAREN) || (is(2, delim::PERIOD) && name(3) && is(4, delim::LEFT_PAREN));
        self.is_word(NEW) && name(1) && opens
    }

    /// Whether a `<static method invocation>` begins here: a name,
    /// perhaps of a schema, then ::.
    fn at_static_method(&self) -> bool {
        let colons = |n: usize| matches!(self.peek_at(n), Tok::Delimiter(d) if *d == delim::DOUBLE_COLON);
        let qualified =
            matches!(self.peek_at(1), Tok::Delimiter(d) if *d == delim::PERIOD) && matches!(self.peek_at(2), Tok::Identifier(_));
        matches!(self.peek(), Tok::Identifier(_)) && (colons(1) || (qualified && colons(3)))
    }

    /// [ISO/IEC 9075-2:2023 §6.19] `<static method invocation>` ::=
    /// `<path-resolved user-defined type name>` :: `<method name>` \[`<SQL
    /// argument list>`\].
    fn static_method(&mut self) -> Result<StructuredExpression> {
        let udt = self.table_name()?;
        self.expect_delim(delim::DOUBLE_COLON)?;
        let name = self.identifier()?;
        let args = if self.is_delim(delim::LEFT_PAREN) { self.sql_argument_list()? } else { Vec::new() };
        Ok(StructuredExpression::Static { udt, name, args })
    }

    /// [ISO/IEC 9075-2:2023 §10.4] `<SQL argument list>` ::= ( \[`<SQL
    /// argument>` \[{, `<SQL argument>`}...\]\] ).
    pub(in crate::parser) fn sql_argument_list(&mut self) -> Result<Vec<ValueExpression>> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut args = Vec::new();
        if self.accept_delim(delim::RIGHT_PAREN) {
            return Ok(args);
        }
        loop {
            args.push(self.nested(Parser::common_value_expression)?);
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(args)
    }

    /// [ISO/IEC 9075-2:2023 §6.18] The rest of a `<generalized
    /// invocation>` ::= ( `<value expression primary>` AS `<data type>` ) .
    /// `<method name>` \[`<SQL argument list>`\], after `subject`, at AS,
    /// begun at `at`.
    pub(in crate::parser) fn generalized_invocation(&mut self, subject: ValueExpression, at: usize) -> Result<ValueExpression> {
        self.expect_kw(kw::AS)?;
        let as_type = Some(self.type_name()?);
        self.expect_delim(delim::RIGHT_PAREN)?;
        self.expect_delim(delim::PERIOD)?;
        let name = self.identifier()?;
        let args = if self.is_delim(delim::LEFT_PAREN) { self.sql_argument_list()? } else { Vec::new() };
        Ok(structured(StructuredExpression::Method { subject, as_type, name, args }, at))
    }

    /// What follows `primary` of a user-defined type or a reference, if
    /// something does: [§6.18] . `<method name>` `<SQL argument list>`,
    /// [§6.40] . SPECIFICTYPE \[( )\], or [§6.21] -> `<qualified identifier>`
    /// \[`<SQL argument list>`\], a dereference operation without the list
    /// and a method reference with it. Else `primary` back.
    pub(in crate::parser) fn structured_postfix(
        &mut self,
        primary: ValueExpression,
    ) -> Result<std::result::Result<ValueExpression, ValueExpression>> {
        let at = self.here();
        let after_period = |p: &Self, d: &str| matches!(p.peek_at(2), Tok::Delimiter(x) if *x == d);
        let period = self.is_delim(delim::PERIOD) && matches!(self.peek_at(1), Tok::Identifier(_));
        // [ISO/IEC 9075-2:2023 §6.36] a key word before ( names a method
        // too, as `size()` or `double()` of a JSON value.
        let worded = self.is_delim(delim::PERIOD) && matches!(self.peek_at(1), Tok::KeyWord(_));
        let s = if period && matches!(self.peek_at(1), Tok::Identifier(w) if w == SPECIFICTYPE) {
            self.advance();
            self.advance();
            if self.is_delim(delim::LEFT_PAREN) && matches!(self.peek_at(1), Tok::Delimiter(d) if *d == delim::RIGHT_PAREN) {
                self.advance();
                self.advance();
            }
            StructuredExpression::SpecificType(primary)
        } else if (period || worded) && after_period(self, delim::LEFT_PAREN) {
            self.advance();
            let name = self.method_name()?;
            StructuredExpression::Method { subject: primary, as_type: None, name, args: self.sql_argument_list()? }
        } else if self.accept_delim(delim::RIGHT_ARROW) {
            let name = self.identifier()?;
            if self.is_delim(delim::LEFT_PAREN) {
                StructuredExpression::MethodReference { subject: primary, name, args: self.sql_argument_list()? }
            } else {
                StructuredExpression::Dereference { subject: primary, attribute: name }
            }
        } else {
            return Ok(Err(primary));
        };
        Ok(Ok(structured(s, at)))
    }

    /// A `<method name>`: an `<identifier>`, or a key word, taken as the
    /// name it spells.
    fn method_name(&mut self) -> Result<String> {
        if let Tok::KeyWord(k) = self.peek() {
            let name = (*k).to_owned();
            self.advance();
            return Ok(name);
        }
        self.identifier()
    }

    /// [ISO/IEC 9075-2:2023 §8.19] ( `<type list>` ), `<type list>` ::=
    /// `<user-defined type specification>` \[{, `<user-defined type
    /// specification>`}...\], each \[ONLY\] `<path-resolved user-defined
    /// type name>`, after OF.
    pub(in crate::parser) fn type_list(&mut self) -> Result<TypeList> {
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut types = Vec::new();
        loop {
            let only = self.accept_kw(kw::ONLY);
            types.push((only, TypeName::Domain(self.table_name()?)));
            if !self.accept_delim(delim::COMMA) {
                break;
            }
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(TypeList(types))
    }
}

// Unit tests: each expression of user-defined types and references read
// (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;

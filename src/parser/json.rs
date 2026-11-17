//! [ISO/IEC 9075-2:2023 §4.48.4] The SQL/JSON query operators: JSON_EXISTS
//! [§8.23], a predicate, and JSON_VALUE [§6.28] and JSON_QUERY [§6.35],
//! primaries; and what they share, the `<JSON API common syntax>`
//! [§10.14] and its `<JSON input expression>`s [§10.12].
//!
//! The operators' names are no key words of SQL-92, so each is the
//! operator only before "(", as the regular expression functions' are;
//! and every word of their Formats that is none either, as PASSING, ERROR
//! and EMPTY, is read as that word only where the Format has it, so a
//! column of any of those names stays one.

use super::Parser;
use super::dynamic::WITHOUT;
use super::ptf::KEEP;
use crate::ast::{
    JsonArgument, JsonClauses, JsonCommon, JsonFormat, JsonInput, JsonOperator, JsonValueBehavior, Predicate, ValueExpression,
};
use crate::error::{Code, Result, SqlError};
use crate::functions::{FORMAT, JSON_EXISTS};
use crate::json::{ExistsBehavior, QueryBehavior, Quotes, Wrapper};
use crate::lexer::{Tok, delim, kw};
use crate::value::types::array::ARRAY_NAME;

// [ISO/IEC 9075-2:2023 §6.28, §6.35, §8.23, §10.12 to §10.14] The words of
// the operators' Formats that are no key words of SQL-92.
/// CONDITIONAL, of a `<JSON query wrapper behavior>`.
const CONDITIONAL: &str = "CONDITIONAL";
/// EMPTY, of ON EMPTY and of EMPTY ARRAY and EMPTY OBJECT.
const EMPTY: &str = "EMPTY";
/// ENCODING, of a `<JSON representation>`.
const ENCODING: &str = "ENCODING";
/// ERROR, of a behavior and of ON ERROR.
const ERROR: &str = "ERROR";
/// JSON, the `<JSON representation>`.
const JSON: &str = "JSON";
/// OBJECT, of EMPTY OBJECT.
const OBJECT: &str = "OBJECT";
/// OMIT, of a `<JSON query quotes behavior>`.
const OMIT: &str = "OMIT";
/// PASSING, of a `<JSON passing clause>`.
const PASSING: &str = "PASSING";
/// QUOTES, after a `<JSON query quotes behavior>`.
const QUOTES: &str = "QUOTES";
/// RETURNING, of a `<JSON returning clause>` or `<JSON output clause>`.
const RETURNING: &str = "RETURNING";
/// SCALAR, of ON SCALAR STRING.
const SCALAR: &str = "SCALAR";
/// STRING, of ON SCALAR STRING.
const STRING: &str = "STRING";
/// UNCONDITIONAL, of a `<JSON query wrapper behavior>`.
const UNCONDITIONAL: &str = "UNCONDITIONAL";
/// WRAPPER, after a `<JSON query wrapper behavior>`.
const WRAPPER: &str = "WRAPPER";
/// [§10.13] The encodings of a `<JSON representation>`.
const ENCODINGS: [&str; 3] = ["UTF8", "UTF16", "UTF32"];

impl Parser {
    /// Whether the SQL/JSON query operator `name` is next: its name before
    /// "(".
    pub(super) fn at_json(&self, name: &str) -> bool {
        self.is_word(name) && matches!(self.peek_at(1), Tok::Delimiter(delim::LEFT_PAREN))
    }

    /// [§8.23] `<JSON exists predicate>` ::= JSON_EXISTS ( `<JSON API
    /// common syntax>` \[`<JSON exists error behavior>` ON ERROR\] ).
    pub(super) fn json_exists(&mut self) -> Result<Predicate> {
        self.expect_word(JSON_EXISTS)?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let common = self.json_common()?;
        let on_error = self.exists_behavior();
        if on_error.is_some() {
            self.expect_kw(kw::ON)?;
            self.expect_word(ERROR)?;
        }
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(Predicate::JsonExists(Box::new(JsonOperator { common, clauses: JsonClauses::Exists { on_error } })))
    }

    /// [§8.23] `<JSON exists error behavior>` ::= TRUE | FALSE | UNKNOWN |
    /// ERROR, if one is next.
    fn exists_behavior(&mut self) -> Option<ExistsBehavior> {
        if self.accept_kw(kw::TRUE) {
            Some(ExistsBehavior::True)
        } else if self.accept_kw(kw::FALSE) {
            Some(ExistsBehavior::False)
        } else if self.accept_kw(kw::UNKNOWN) {
            Some(ExistsBehavior::Unknown)
        } else if self.accept_word(ERROR) {
            Some(ExistsBehavior::Error)
        } else {
            None
        }
    }

    /// [§6.28] `<JSON value function>` ::= JSON_VALUE ( `<JSON API common
    /// syntax>` \[RETURNING `<data type>`\] \[`<JSON value empty behavior>`
    /// ON EMPTY\] \[`<JSON value error behavior>` ON ERROR\] ), its name
    /// next.
    pub(super) fn json_value(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let common = self.json_common()?;
        let returning = if self.accept_word(RETURNING) { Some(self.type_name()?) } else { None };
        let (on_empty, on_error) = self.behaviors(Self::value_behavior)?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let clauses = JsonClauses::Value { returning, on_empty, on_error };
        Ok(ValueExpression::Json(Box::new(JsonOperator { common, clauses }), at))
    }

    /// [§6.28] A `<JSON value empty behavior>` or `<JSON value error
    /// behavior>`, ERROR | NULL | DEFAULT `<value expression>`, if one is
    /// next.
    fn value_behavior(&mut self) -> Result<Option<JsonValueBehavior>> {
        if self.accept_word(ERROR) {
            return Ok(Some(JsonValueBehavior::Error));
        }
        if self.accept_kw(kw::NULL) {
            return Ok(Some(JsonValueBehavior::Null));
        }
        if !self.accept_kw(kw::DEFAULT) {
            return Ok(None);
        }
        Ok(Some(JsonValueBehavior::Default(Box::new(self.value_expression()?))))
    }

    /// \[`<behavior>` ON EMPTY\] \[`<behavior>` ON ERROR\], each behavior
    /// as `read` reads one.
    fn behaviors<T>(&mut self, read: fn(&mut Parser) -> Result<Option<T>>) -> Result<(Option<T>, Option<T>)> {
        let Some(first) = read(self)? else { return Ok((None, None)) };
        self.expect_kw(kw::ON)?;
        if self.accept_word(ERROR) {
            return Ok((None, Some(first)));
        }
        if !self.accept_word(EMPTY) {
            return Err(self.syntax("expected EMPTY or ERROR"));
        }
        let Some(second) = read(self)? else { return Ok((Some(first), None)) };
        self.expect_kw(kw::ON)?;
        self.expect_word(ERROR)?;
        Ok((Some(first), Some(second)))
    }

    /// [§6.35] `<JSON query>` ::= JSON_QUERY ( `<JSON API common syntax>`
    /// \[`<JSON output clause>`\] \[`<JSON query wrapper behavior>`
    /// WRAPPER\] \[`<JSON query quotes behavior>` QUOTES \[ON SCALAR
    /// STRING\]\] \[`<JSON query empty behavior>` ON EMPTY\] \[`<JSON query
    /// error behavior>` ON ERROR\] ), its name next.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a quotes behavior follows WITH a wrapper \[SR8\].
    pub(super) fn json_query(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let common = self.json_common()?;
        // [§10.13] <JSON output clause> ::= RETURNING <data type> [FORMAT
        // <JSON representation>]
        let returning = if self.accept_word(RETURNING) {
            let to = self.type_name()?;
            Some((to, if self.accept_word(FORMAT) { Some(self.json_representation()?) } else { None }))
        } else {
            None
        };
        let wrapper = self.wrapper_behavior()?;
        let quotes_at = self.here();
        let quotes = self.quotes_behavior()?;
        if quotes.is_some() && matches!(wrapper, Some(Wrapper::Conditional | Wrapper::Unconditional)) {
            return Err(SqlError::at(
                Code::SyntaxRule,
                quotes_at,
                "[ISO/IEC 9075-2:2023 §6.35 SR8] JSON_QUERY has no <JSON query quotes behavior> WITH a wrapper",
            ));
        }
        let (on_empty, on_error) = self.behaviors(Self::query_behavior)?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let clauses = JsonClauses::Query { returning, wrapper, quotes, on_empty, on_error };
        Ok(ValueExpression::Json(Box::new(JsonOperator { common, clauses }), at))
    }

    /// [§6.35] `<JSON query wrapper behavior>` WRAPPER, if one is next:
    /// WITHOUT \[ARRAY\], or WITH \[CONDITIONAL | UNCONDITIONAL\] \[ARRAY\],
    /// UNCONDITIONAL if neither is written \[SR7\].
    fn wrapper_behavior(&mut self) -> Result<Option<Wrapper>> {
        let wrapper = if self.accept_word(WITHOUT) {
            Wrapper::Without
        } else if !self.accept_kw(kw::WITH) {
            return Ok(None);
        } else if self.accept_word(CONDITIONAL) {
            Wrapper::Conditional
        } else {
            self.accept_word(UNCONDITIONAL);
            Wrapper::Unconditional
        };
        self.accept_word(ARRAY_NAME);
        self.expect_word(WRAPPER)?;
        Ok(Some(wrapper))
    }

    /// [§6.35] `<JSON query quotes behavior>` QUOTES \[ON SCALAR STRING\],
    /// KEEP or OMIT, if one is next.
    fn quotes_behavior(&mut self) -> Result<Option<Quotes>> {
        let quotes = if self.accept_word(KEEP) {
            Quotes::Keep
        } else if self.accept_word(OMIT) {
            Quotes::Omit
        } else {
            return Ok(None);
        };
        self.expect_word(QUOTES)?;
        if self.accept_kw(kw::ON) {
            self.expect_word(SCALAR)?;
            self.expect_word(STRING)?;
        }
        Ok(Some(quotes))
    }

    /// [§6.35] A `<JSON query empty behavior>` or `<JSON query error
    /// behavior>`, ERROR | NULL | EMPTY ARRAY | EMPTY OBJECT, if one is
    /// next.
    fn query_behavior(&mut self) -> Result<Option<QueryBehavior>> {
        if self.accept_word(ERROR) {
            return Ok(Some(QueryBehavior::Error));
        }
        if self.accept_kw(kw::NULL) {
            return Ok(Some(QueryBehavior::Null));
        }
        if !self.accept_word(EMPTY) {
            return Ok(None);
        }
        if self.accept_word(ARRAY_NAME) {
            return Ok(Some(QueryBehavior::EmptyArray));
        }
        self.expect_word(OBJECT)?;
        Ok(Some(QueryBehavior::EmptyObject))
    }

    /// [§10.14] `<JSON API common syntax>` ::= `<JSON context item>` ,
    /// `<JSON path specification>` \[`<JSON passing clause>`\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if AS `<JSON table path name>` follows the path
    /// specification, as it does only in JSON_TABLE \[SR2a\].
    fn json_common(&mut self) -> Result<JsonCommon> {
        let context = self.json_input()?;
        self.expect_delim(delim::COMMA)?;
        let path_at = self.here();
        let Tok::CharacterString(path) = self.peek().clone() else {
            return Err(self.syntax("expected a <JSON path specification>, a <character string literal>"));
        };
        self.advance();
        if self.is_kw(kw::AS) {
            return Err(self.rule("[ISO/IEC 9075-2:2023 §10.14 SR2a] AS <JSON table path name> is written only in JSON_TABLE"));
        }
        let mut passing = Vec::new();
        if self.accept_word(PASSING) {
            passing.push(self.json_argument()?);
            while self.accept_delim(delim::COMMA) {
                passing.push(self.json_argument()?);
            }
        }
        Ok(JsonCommon { context, path, path_at, passing })
    }

    /// [§10.14] `<JSON argument>` ::= `<JSON input expression>` AS
    /// `<identifier>`.
    fn json_argument(&mut self) -> Result<JsonArgument> {
        let input = self.json_input()?;
        self.expect_kw(kw::AS)?;
        let at = self.here();
        let Tok::Identifier(name) = self.peek().clone() else { return Err(self.syntax("expected an <identifier>")) };
        self.advance();
        Ok(JsonArgument { input, name, at })
    }

    /// [§10.12] `<JSON input expression>` ::= `<value expression>`
    /// \[FORMAT `<JSON representation>`\].
    fn json_input(&mut self) -> Result<JsonInput> {
        let value = self.value_expression()?;
        let format = if self.accept_word(FORMAT) { Some(self.json_representation()?) } else { None };
        Ok(JsonInput { value, format })
    }

    /// [§10.13] `<JSON representation>` ::= JSON \[ENCODING { UTF8 | UTF16
    /// | UTF32 }\].
    fn json_representation(&mut self) -> Result<JsonFormat> {
        self.expect_word(JSON)?;
        if !self.accept_word(ENCODING) {
            return Ok(JsonFormat { encoding: None });
        }
        let encoding = ENCODINGS.into_iter().find(|e| self.accept_word(e));
        encoding.map(|e| JsonFormat { encoding: Some(e) }).ok_or_else(|| self.syntax("expected UTF8, UTF16 or UTF32"))
    }
}

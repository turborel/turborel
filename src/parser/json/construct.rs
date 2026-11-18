//! [ISO/IEC 9075-2:2023 §6.34, §10.11, §8.22] The SQL/JSON constructor
//! functions JSON_OBJECT and JSON_ARRAY, the `<JSON aggregate function>`s
//! JSON_OBJECTAGG and JSON_ARRAYAGG, and the `<JSON predicate>` IS JSON.
//! Their names are no key words of SQL-92, each the function only before
//! "(", and ABSENT, KEYS, SCALAR and the words the query operators share
//! are read as words only where the Formats have them.

use super::super::Parser;
use super::super::dynamic::WITHOUT;
use super::{ENCODING, JSON, OBJECT, RETURNING, SCALAR};
use crate::ast::{
    Composite, IsJson, JsonAggregate, JsonConstructor, JsonExpression, JsonInput, JsonMember, Predicate, RowValue, SetArg,
    SetFunctionSpecification, Subquery, ValueExpression,
};
use crate::error::Result;
use crate::functions::{FORMAT, JSON_ARRAYAGG, JSON_OBJECTAGG};
use crate::json::{JsonType, Keys, OnNull};
use crate::lexer::{Tok, delim, kw};
use crate::value::types::array::ARRAY_NAME;

// [§8.22 F] How many tokens a `<JSON predicate>`'s prefix has, before
// its IS or its JSON.
/// FORMAT JSON.
const FORMAT_JSON: usize = 2;
/// FORMAT JSON ENCODING encoding.
const FORMAT_JSON_ENCODING: usize = 4;
/// IS NOT.
const IS_NOT: usize = 2;

// [ISO/IEC 9075-2:2023 §6.34, §8.22] The words of the constructors' and
// IS JSON's Formats that are no key words of SQL-92.
/// ABSENT, of ABSENT ON NULL.
const ABSENT: &str = "ABSENT";
/// KEYS, of a `<JSON key uniqueness constraint>`.
const KEYS: &str = "KEYS";

impl Parser {
    /// [§6.34] `<JSON object constructor>` ::= JSON_OBJECT ( \[`<JSON name
    /// and value>` \[{ , `<JSON name and value>` }...\] \[`<JSON constructor
    /// null clause>`\] \[`<JSON key uniqueness constraint>`\]\] \[`<JSON
    /// output clause>`\] ), its name next.
    pub(super) fn json_object(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let (mut members, mut on_null, mut keys) = (Vec::new(), None, None);
        if !self.at_constructor_end() {
            members.push(self.json_member()?);
            while self.accept_delim(delim::COMMA) {
                members.push(self.json_member()?);
            }
            on_null = self.null_clause()?;
            keys = self.uniqueness()?;
        }
        let returning = self.json_output()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let made = JsonConstructor::Object { members, on_null, keys, returning };
        Ok(ValueExpression::Json(Box::new(JsonExpression::Constructor(made)), at))
    }

    /// Whether a constructor's ")" or `<JSON output clause>` is next, so it
    /// has no member or element: RETURNING not followed as a name or a
    /// value is, by VALUE, ":" or ",".
    fn at_constructor_end(&self) -> bool {
        if self.is_delim(delim::RIGHT_PAREN) {
            return true;
        }
        let operand = matches!(self.peek_at(1), Tok::KeyWord(kw::VALUE) | Tok::Delimiter(delim::COLON | delim::COMMA | delim::RIGHT_PAREN));
        self.is_word(RETURNING) && !operand
    }

    /// [§6.34] `<JSON name and value>` ::= \[KEY\] `<JSON name>` VALUE
    /// `<JSON input expression>` | `<JSON name>` : `<JSON input
    /// expression>`, the name a `<character value expression>`.
    fn json_member(&mut self) -> Result<JsonMember> {
        let keyed = self.accept_kw(kw::KEY);
        let name = self.common_value_expression()?;
        let colon = if keyed { false } else { self.accept_delim(delim::COLON) };
        let valued = if colon { true } else { self.accept_kw(kw::VALUE) };
        if !valued {
            return Err(self.syntax(if keyed { "expected VALUE" } else { "expected VALUE or \":\"" }));
        }
        Ok(JsonMember { name, value: self.json_input()?, colon })
    }

    /// [§6.34] `<JSON constructor null clause>` ::= NULL ON NULL | ABSENT
    /// ON NULL, if one is next.
    fn null_clause(&mut self) -> Result<Option<OnNull>> {
        let on_null = if self.accept_word(ABSENT) {
            OnNull::Absent
        } else if self.is_kw(kw::NULL) && self.next_is_kw(kw::ON) {
            self.advance();
            OnNull::Null
        } else {
            return Ok(None);
        };
        self.expect_kw(kw::ON)?;
        self.expect_kw(kw::NULL)?;
        Ok(Some(on_null))
    }

    /// [§8.22] `<JSON key uniqueness constraint>` ::= WITH UNIQUE \[KEYS\] |
    /// WITHOUT UNIQUE \[KEYS\], if one is next; WITH only before UNIQUE, so
    /// WITH CHECK OPTION after a predicate stays the view's.
    pub(super) fn uniqueness(&mut self) -> Result<Option<Keys>> {
        let keys = if self.is_kw(kw::WITH) && self.next_is_kw(kw::UNIQUE) {
            self.advance();
            Keys::Unique
        } else if self.accept_word(WITHOUT) {
            Keys::Any
        } else {
            return Ok(None);
        };
        self.expect_kw(kw::UNIQUE)?;
        self.accept_word(KEYS);
        Ok(Some(keys))
    }

    /// [§6.34] `<JSON array constructor>`, its name next: by query, of a
    /// `<query expression>` or a lone `<scalar subquery>` \[SR7d\]; else by
    /// enumeration, JSON_ARRAY ( \[`<JSON input expression>` \[{ , `<JSON
    /// input expression>` }...\] \[`<JSON constructor null clause>`\]\]
    /// \[`<JSON output clause>`\] ).
    pub(super) fn json_array(&mut self) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let made = if matches!(self.peek(), Tok::KeyWord(kw::SELECT | kw::VALUES | kw::TABLE | kw::WITH)) {
            let query = Subquery { at: self.here(), query: self.query_expression()? };
            let format = if self.accept_word(FORMAT) { Some(self.json_representation()?) } else { None };
            let on_null = self.null_clause()?;
            JsonConstructor::ArrayQuery { query: Box::new(query), format, on_null, returning: self.json_output()? }
        } else {
            self.json_array_elements()?
        };
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(ValueExpression::Json(Box::new(JsonExpression::Constructor(made)), at))
    }

    /// The elements and clauses of a JSON_ARRAY of no `<query expression>`
    /// first, by query if they are a lone `<scalar subquery>` \[SR7d\].
    fn json_array_elements(&mut self) -> Result<JsonConstructor> {
        let mut elements = Vec::new();
        let mut on_null = None;
        if !self.at_constructor_end() {
            elements.push(self.json_input()?);
            while self.accept_delim(delim::COMMA) {
                elements.push(self.json_input()?);
            }
            on_null = self.null_clause()?;
        }
        let returning = self.json_output()?;
        let lone = <[JsonInput; 1]>::try_from(elements);
        Ok(match lone {
            Ok([JsonInput { value: ValueExpression::Subquery(query), format }]) => {
                JsonConstructor::ArrayQuery { query, format, on_null, returning }
            }
            Ok([one]) => JsonConstructor::Array { elements: vec![one], on_null, returning },
            Err(many) => JsonConstructor::Array { elements: many, on_null, returning },
        })
    }

    /// [§10.11] `<JSON object aggregate constructor>` ::= JSON_OBJECTAGG (
    /// `<JSON name and value>` \[`<JSON constructor null clause>`\] \[`<JSON
    /// key uniqueness constraint>`\] \[`<JSON output clause>`\] ), its name
    /// next: a set function of the ROW of its name and value.
    pub(super) fn json_objectagg(&mut self) -> Result<SetFunctionSpecification> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let member = self.json_member()?;
        let on_null = self.null_clause()?;
        let keys = self.uniqueness()?;
        let returning = self.json_output()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let json = JsonAggregate { format: member.value.format, colon: member.colon, on_null, keys, returning };
        let row = ValueExpression::Composite(Box::new(Composite::Row(vec![member.name, member.value.value])), at);
        Ok(aggregate(JSON_OBJECTAGG, (SetArg::All(Box::new(row)), Vec::new()), json, at))
    }

    /// [§10.11] `<JSON array aggregate constructor>` ::= JSON_ARRAYAGG (
    /// `<JSON input expression>` \[ORDER BY `<sort specification list>`\]
    /// \[`<JSON constructor null clause>`\] \[`<JSON output clause>`\] ),
    /// its name next.
    pub(super) fn json_arrayagg(&mut self) -> Result<SetFunctionSpecification> {
        let at = self.here();
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let input = self.json_input()?;
        let order_by = if self.accept_kw(kw::ORDER) {
            self.expect_kw(kw::BY)?;
            self.sort_specification_list()?
        } else {
            Vec::new()
        };
        let on_null = self.null_clause()?;
        let returning = self.json_output()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        let json = JsonAggregate { format: input.format, colon: false, on_null, keys: None, returning };
        Ok(aggregate(JSON_ARRAYAGG, (SetArg::All(Box::new(input.value)), order_by), json, at))
    }

    /// Whether a `<JSON predicate>`'s part after its input is next: IS
    /// \[NOT\] JSON, or its `<JSON input clause>`, FORMAT JSON \[ENCODING
    /// encoding\], before IS; FORMAT JSON before anything else is a `<JSON
    /// input expression>`'s, as of an argument.
    pub(in crate::parser) fn at_is_json(&self) -> bool {
        let word_at = |n: usize, word: &str| matches!(self.peek_at(n), Tok::Identifier(w) if w == word);
        let is_at = |n: usize| matches!(self.peek_at(n), Tok::KeyWord(kw::IS));
        if self.is_word(FORMAT) {
            let after = if word_at(FORMAT_JSON, ENCODING) { FORMAT_JSON_ENCODING } else { FORMAT_JSON };
            return word_at(1, JSON) && is_at(after);
        }
        let after = if self.next_is_kw(kw::NOT) { IS_NOT } else { 1 };
        is_at(0) && word_at(after, JSON)
    }

    /// [§8.22] The rest of a `<JSON predicate>` whose `<JSON predicate
    /// input>` is `values`, one value: \[`<JSON input clause>`\] IS \[NOT\]
    /// JSON \[VALUE | ARRAY | OBJECT | SCALAR\] \[`<JSON key uniqueness
    /// constraint>`\].
    pub(in crate::parser) fn is_json(&mut self, mut values: RowValue) -> Result<Predicate> {
        let (Some(value), None) = (values.pop(), values.pop()) else {
            return Err(self.syntax("[ISO/IEC 9075-2:2023 §8.22 F] the input of IS JSON is one <string value expression>, not a row"));
        };
        let format = if self.accept_word(FORMAT) { Some(self.json_representation()?) } else { None };
        self.expect_kw(kw::IS)?;
        let not = self.accept_kw(kw::NOT);
        self.expect_word(JSON)?;
        let kind = if self.accept_kw(kw::VALUE) {
            Some(JsonType::Value)
        } else if self.accept_word(ARRAY_NAME) {
            Some(JsonType::Array)
        } else if self.accept_word(OBJECT) {
            Some(JsonType::Object)
        } else if self.accept_word(SCALAR) {
            Some(JsonType::Scalar)
        } else {
            None
        };
        let keys = self.uniqueness()?;
        Ok(Predicate::IsJson(Box::new(IsJson { input: JsonInput { value, format }, not, kind, keys })))
    }
}

/// The `<set function specification>` of the JSON aggregate `name`, of
/// its argument and sort keys, its clauses `json`, written at `at`.
fn aggregate(
    name: &'static str,
    (arg, order_by): (SetArg, Vec<crate::ast::ValueOrder>),
    json: JsonAggregate,
    at: usize,
) -> SetFunctionSpecification {
    SetFunctionSpecification { name, arg, order_by, listagg: None, json: Some(Box::new(json)), filter: None, semantics: None, at }
}

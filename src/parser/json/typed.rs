//! [ISO/IEC 9075-2:2023 §6.37, §6.40, §6.41] The SQL/JSON functions of
//! the JSON type, each its name before "(": JSON ( ), `<JSON parse>`;
//! JSON_SCALAR, `<JSON scalar>`; and JSON_SERIALIZE, `<JSON serialize>`.
//! None of the names is a key word of SQL-92, as the query operators'
//! are not (`parser/json.rs`). The simplified accessor is in
//! `parser/json/accessor.rs`.

use super::super::Parser;
use crate::ast::{JsonExpression, JsonTyped, ValueExpression};
use crate::error::Result;
use crate::json::{JSON_PARSE, JSON_SCALAR, JSON_SERIALIZE};
use crate::lexer::delim;

impl Parser {
    /// A function of the JSON type, if one is next.
    pub(super) fn json_typed(&mut self) -> Result<Option<ValueExpression>> {
        let at = self.here();
        let typed = if self.at_json(JSON_PARSE) {
            self.json_parse()?
        } else if self.at_json(JSON_SCALAR) {
            self.advance();
            self.expect_delim(delim::LEFT_PAREN)?;
            let value = self.value_expression()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            JsonTyped::Scalar(value)
        } else if self.at_json(JSON_SERIALIZE) {
            self.advance();
            self.expect_delim(delim::LEFT_PAREN)?;
            let value = self.value_expression()?;
            let returning = self.json_output()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            JsonTyped::Serialize { value, returning }
        } else {
            return Ok(None);
        };
        Ok(Some(ValueExpression::Json(Box::new(JsonExpression::Typed(typed)), at)))
    }

    /// [§6.40] `<JSON parse>` ::= JSON ( `<string value expression>`
    /// \[`<JSON input clause>`\] \[`<JSON key uniqueness constraint>`\] ),
    /// its name next.
    fn json_parse(&mut self) -> Result<JsonTyped> {
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let input = self.json_input()?;
        let keys = self.uniqueness()?;
        self.expect_delim(delim::RIGHT_PAREN)?;
        Ok(JsonTyped::Parse { input, keys })
    }
}

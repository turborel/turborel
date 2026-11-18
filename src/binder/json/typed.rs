//! [ISO/IEC 9075-2:2023 §6.36 to §6.41] The functions of the JSON type
//! bound, each a call of a function of its own: JSON ( ) (`JsonParse`),
//! JSON_SCALAR (`JsonScalar`) and JSON_SERIALIZE (`JsonSerialize`); and
//! the simplified accessor, bound as the JSON_VALUE or JSON_QUERY it is
//! equivalent to \[§6.36 SR3\] (`binder/json/accessor.rs`).

use std::sync::Arc;

use super::super::{Binder, Scope, rule};
use crate::ast::{JsonOutput, JsonTyped, ValueExpression};
use crate::error::Result;
use crate::functions::{ScalarRef, json_text, output_type};
use crate::json::{JSON_SERIALIZE, JsonParse, JsonScalar, JsonSerialize, Keys, scalar_operand};
use crate::logical::Expr;
use crate::value::DataType;

/// The Syntax Rule the operand of JSON ( ) breaks if of no string type.
const PARSE_RULE: &str = "[ISO/IEC 9075-2:2023 §6.40 F] the operand of JSON ( ) is a <string value expression>";

impl Binder<'_> {
    /// The function of the JSON type `typed`, or the accessor.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if an operand is of no type the function takes, or
    /// JSON_SERIALIZE returns no character string type; -900 of a binary
    /// string's JSON text.
    pub(super) fn json_typed(&self, typed: &JsonTyped, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        match typed {
            JsonTyped::Parse { input, keys } => {
                // [§6.40 SR4] WITHOUT UNIQUE KEYS if not written.
                let f = ScalarRef(Arc::new(JsonParse::new(keys.unwrap_or(Keys::Any))));
                let (args, operand) = self.arguments(&*f.0, std::slice::from_ref(&input.value), scope)?;
                operand.iter().try_for_each(|t| json_text(t, input.format, PARSE_RULE))?;
                Ok((Expr::Call(f, args), DataType::Json))
            }
            JsonTyped::Scalar(value) => {
                let f = ScalarRef(Arc::new(JsonScalar));
                let (args, operand) = self.arguments(&*f.0, std::slice::from_ref(value), scope)?;
                if let Some(t) = operand.iter().find(|t| !scalar_operand(t)) {
                    return Err(rule(format!(
                        "[ISO/IEC 9075-2:2023 §6.41 SR2] the operand of JSON_SCALAR is a character string, a number, a truth value or a datetime, not {t}"
                    )));
                }
                Ok((Expr::Call(f, args), DataType::Json))
            }
            JsonTyped::Serialize { value, returning } => self.json_serialize(value, returning.as_ref(), scope),
            JsonTyped::Accessor { primary, chain, sql } => self.json_accessor(primary, chain, sql.as_ref(), scope),
        }
    }

    /// [§6.37] JSON_SERIALIZE of `value` RETURNING `returning`.
    fn json_serialize(&self, value: &ValueExpression, returning: Option<&JsonOutput>, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        // [SR1, SR3] RETURNING the implicit string type if not written, a
        // string type [§9.43 SR2a].
        let to = output_type(returning, JSON_SERIALIZE)?;
        if let DataType::Json = to {
            return Err(rule("[ISO/IEC 9075-2:2023 §6.37 SR3, §9.43 SR2a] JSON_SERIALIZE returns a string type, not JSON"));
        }
        let f = ScalarRef(Arc::new(JsonSerialize::new(to.clone())));
        let (args, types) = self.arguments(&*f.0, std::slice::from_ref(value), scope)?;
        if let Some(t) = types.iter().find(|t| !matches!(t, DataType::Json)) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.38 SR1] the operand of JSON_SERIALIZE is of the JSON type, not {t}")));
        }
        Ok((Expr::Call(f, args), to))
    }
}

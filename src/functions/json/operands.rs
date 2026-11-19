//! The Syntax Rules of the operands of the SQL/JSON functions, on their
//! declared types [ISO/IEC 9075-2:2023 §10.12, §10.13]: a `<JSON input
//! expression>` of JSON text, FORMAT JSON, or of a value made an item, and
//! a `<JSON output clause>`'s type. The binder checks them as a statement
//! is bound, so a statement that breaks one is refused as it is prepared;
//! the library checks the values again as its own guard (`json.rs`).

use crate::ast::{JsonFormat, JsonOutput, TypeName};
use crate::error::{Code, Result, SqlError};
use crate::json::IMPLICIT_RETURNING;
use crate::value::DataType;
use crate::value::lob::LobKind;

/// The error of the Syntax Rule `cited` broken.
fn rule(cited: String) -> SqlError {
    SqlError::new(Code::SyntaxRule, cited)
}

/// [§6.34 SR2b, §6.35 SR1b, §10.11 SR1b, §10.13 SR2] The type `function`
/// returns: that of its `<JSON output clause>`, `written`, a character
/// string type, FORMAT JSON explicit or implicit \[§10.13 SR1\]; without
/// one, the implementation-defined `IMPLICIT_RETURNING` (ID098, ID101,
/// ID134).
///
/// # Errors
///
/// SQLCODE -103 if `written` is of no string type \[§10.13 SR2\] or a
/// character string type of ENCODING \[§10.13 SR2a\]; -900 of a binary
/// string type, whose encodings turborel does not implement.
pub(crate) fn output_type(written: Option<&JsonOutput>, function: &str) -> Result<DataType> {
    let Some((written, format)) = written else { return Ok(IMPLICIT_RETURNING) };
    let TypeName::Data(t) = written else {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §10.13 SR2] the <JSON output clause> of {function} is of a string type")));
    };
    if t.is_binary() {
        return Err(binary(&format!("{function} RETURNING {t}")));
    }
    // [§10.13 SR2] RETURNING JSON, FORMAT JSON explicit or not (Features
    // T801, T851), of no ENCODING.
    if let DataType::Json = t {
        return no_encoding(t, format.map(|f| f.encoding.is_some()), function).map(|()| t.clone());
    }
    if !t.is_character() {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §10.13 SR2] the <JSON output clause> of {function} is of a string type, not {t}")));
    }
    if format.is_some_and(|f| f.encoding.is_some()) {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §10.13 SR2a] {function} RETURNING {t}, a character string type, has no ENCODING")));
    }
    Ok(t.clone())
}

/// [§10.12 SR6, SR8] That JSON text of the type `t`, with the `<JSON input
/// clause>` `format`, is a character string's, of no ENCODING; `cited` is
/// the rule that makes it a string.
///
/// # Errors
///
/// SQLCODE -900 if `t` is a binary string type, whose encodings turborel
/// does not implement; -103 if it is of no string type, or ENCODING is
/// written \[SR8\].
pub(crate) fn json_text(t: &DataType, format: Option<JsonFormat>, cited: &str) -> Result<()> {
    // [§10.12 SR5, SR6] A value of the JSON type, FORMAT JSON implicit.
    if let DataType::Json = t {
        return no_encoding(t, format.map(|f| f.encoding.is_some()), "JSON text");
    }
    if t.is_binary() {
        return Err(binary(&format!("JSON text of {t}")));
    }
    if !t.is_character() {
        return Err(rule(format!("{cited}, not {t}")));
    }
    if format.is_some_and(|f| f.encoding.is_some()) {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §10.12 SR8] JSON text of {t}, a character string type, has no ENCODING")));
    }
    Ok(())
}

/// [§10.12 SR3, SR6 to SR8] That `what`, a `<JSON input expression>` of
/// type `t` and the `<JSON input clause>` `format`, is of a type it may
/// be: FORMAT JSON, a character string's JSON text; else a character
/// string, a number, a datetime, a truth value or an interval, which is
/// kept or cast to a character string [§6.34 GR2d ii 1 F IV, §9.46 GR11b i
/// 2].
///
/// # Errors
///
/// As `json_text` of FORMAT JSON; else SQLCODE -103 of a binary string
/// \[SR7\], an array, a multiset or a row, which CAST takes to no
/// character string \[SR3\]; -900 of a user-defined type, a reference or a
/// type beyond the standard, whose casts turborel does not apply here.
pub(crate) fn input_type(t: &DataType, format: Option<JsonFormat>, what: &str) -> Result<()> {
    if format.is_some() {
        return json_text(t, format, &format!("[ISO/IEC 9075-2:2023 §10.12 SR6] {what} FORMAT JSON is of a string type"));
    }
    match t {
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        // [§10.12 SR5] of the JSON type, FORMAT JSON implicit.
        | DataType::Json
        | DataType::Lob { kind: LobKind::Character, .. } => Ok(()),
        DataType::Binary { .. } | DataType::VarBinary { .. } | DataType::Lob { kind: LobKind::Binary, .. } => {
            Err(rule(format!("[ISO/IEC 9075-2:2023 §10.12 SR7] {what} of the binary string type {t} is FORMAT JSON")))
        }
        DataType::Array(_) | DataType::Multiset(_) | DataType::Row(_) | DataType::Xml(_) => {
            Err(rule(format!("[ISO/IEC 9075-2:2023 §10.12 SR3] {what} is of a type CAST takes to a character string, not {t}")))
        }
        DataType::Distinct(_) | DataType::Structured(_) | DataType::Reference(_) | DataType::Extension(_) => Err(SqlError::new(
            Code::Unsupported,
            format!("[DESIGN] {what} of {t}, cast to a character string, is valid SQL but is not implemented (RULES.md D18)"),
        )),
    }
}

/// [§10.12 SR8, §10.13 SR2] That JSON of the type `t`, a JSON type, has no
/// ENCODING, which `encoded` says is written.
///
/// # Errors
///
/// SQLCODE -103 if it is.
fn no_encoding(t: &DataType, encoded: Option<bool>, what: &str) -> Result<()> {
    if encoded == Some(true) {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §10.12 SR8, §10.13 SR2] {what} of {t} has no ENCODING")));
    }
    Ok(())
}

/// [§6.34 F, §10.11 F] That a `<JSON name>`, of type `t`, is a
/// `<character value expression>`.
///
/// # Errors
///
/// SQLCODE -103 if `t` is no character string type.
pub(crate) fn name_type(t: &DataType, function: &str) -> Result<()> {
    if t.is_character() {
        return Ok(());
    }
    Err(rule(format!("[ISO/IEC 9075-2:2023 §6.34 F] a <JSON name> of {function} is a character string, not {t}")))
}

/// The error of `what`, of a binary string type, valid but not
/// implemented (RULES.md D18).
fn binary(what: &str) -> SqlError {
    SqlError::new(
        Code::Unsupported,
        format!("[DESIGN] {what}, a binary string type, FORMAT JSON ENCODING, is valid SQL but is not implemented (RULES.md D18)"),
    )
}

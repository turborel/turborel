//! The `<JSON predicate>` IS JSON [ISO/IEC 9075-2:2023 §8.22]: whether a
//! string is a JSON text, of the item kind its `<JSON predicate type
//! constraint>` names, under its `<JSON key uniqueness constraint>`; and
//! IS NOT JSON, its negation \[SR4\].
//!
//! \[DESIGN\] turborel has no JSON type (Feature T801), so the
//! `<JSON predicate input>` is a `<string value expression>`, FORMAT JSON
//! explicit or implicit \[SR1\], and \[GR2c\] applies: the string is parsed
//! [§9.42]. A failed parse is False \[GR3a\] only if its STATUS is an
//! exception condition of §9.42's, SQLSTATE 22032 or 22037. A limit of
//! turborel's reached is no answer the rules give and is raised: a text
//! nested deeper than `MAX_NESTING` (SQLCODE -900, RULES.md D18), or a
//! number no number of turborel's holds exactly (SQLSTATE 22003), which
//! is still of RFC 8259's grammar, so still a JSON text.

use super::binary::NodeKind;
use super::query::json_text;
use super::{Item, Keys, parse};
use crate::error::{Result, SqlError, SqlState};
use crate::value::Value;

/// [§8.22] A `<JSON predicate type constraint>`: the kind of SQL/JSON item
/// the text is to be. \[SR2\] Without one, VALUE is implicit,
/// `JsonType::Value`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonType {
    /// `VALUE`: any item; the default \[SR2\].
    Value,
    /// `ARRAY`: an SQL/JSON array \[GR3b\].
    Array,
    /// `OBJECT`: an SQL/JSON object \[GR3c\].
    Object,
    /// `SCALAR`: neither an array nor an object \[GR3d\], so a scalar or
    /// the SQL/JSON null.
    Scalar,
}

/// The Syntax Rule the `<JSON predicate input>` breaks if it is no
/// string.
const INPUT_RULE: &str = "[ISO/IEC 9075-2:2023 §8.22 F] the input of IS JSON is a string";

/// [§8.22 GR] `value IS JSON kind keys`, of the value `value` of the
/// `<JSON predicate input>`, its `<JSON predicate type constraint>` `kind`
/// and its `<JSON key uniqueness constraint>` `keys` (\[SR3\] without one,
/// WITHOUT UNIQUE KEYS, `Keys::Any`): Unknown, the null value, if `value`
/// is the null value \[GR2a\]; False if it is no JSON text \[GR3a\], has
/// two members of one key in an object under `Keys::Unique` \[GR3a\], or
/// is not of `kind` \[GR3b to GR3d\]; else True \[GR3e\]. A character
/// large object is read whole.
///
/// # Errors
///
/// SQLCODE -103 if `value` is no string \[F\]; -900 if it is a binary
/// string, whose encodings turborel does not implement, a character large
/// object longer than one chunk, or a text beyond turborel's limits, as
/// the module's documentation says.
pub fn is_json(value: &Value, kind: JsonType, keys: Keys) -> Result<Value> {
    // [§8.22 GR2b] A value of the JSON type is no text to parse: its
    // kind alone is judged, its keys unique or not.
    if let Value::Json(j) = value {
        let node = j.root()?.kind()?;
        return Ok(Value::Bool(node_is_of(node, kind)));
    }
    let Some(text) = json_text(value, INPUT_RULE)? else { return Ok(Value::Null) };
    let item = match parse(&text, keys) {
        Ok(item) => item,
        Err(error) => return if not_json(&error) { Ok(Value::Bool(false)) } else { Err(error) },
    };
    Ok(Value::Bool(is_of(&item, kind)))
}

/// [§8.22 GR3a] Whether `error`, of a parse, is a STATUS of §9.42's:
/// SQLSTATE 22032 of no JSON text \[§9.42 GR3a iii 1\] or 22037 of
/// non-unique keys \[§9.42 GR3a iii 2\].
fn not_json(error: &SqlError) -> bool {
    let state = error.sqlstate();
    state == SqlState::INVALID_JSON_TEXT || state == SqlState::NON_UNIQUE_KEYS_IN_A_JSON_OBJECT
}

/// [§8.22 SR4] `value IS NOT JSON kind keys`, which is `NOT (value IS JSON
/// kind keys)`: the negation of `is_json`, Unknown of Unknown.
///
/// # Errors
///
/// As `is_json`.
pub fn is_not_json(value: &Value, kind: JsonType, keys: Keys) -> Result<Value> {
    let truth = is_json(value, kind, keys)?;
    Ok(if let Value::Bool(b) = truth { Value::Bool(!b) } else { truth })
}

/// [§8.22 GR3b to GR3e] `is_of` of an item of the JSON type, its root
/// node of the kind `node`.
fn node_is_of(node: NodeKind, kind: JsonType) -> bool {
    match kind {
        JsonType::Value => true,
        JsonType::Array => node == NodeKind::Array,
        JsonType::Object => node == NodeKind::Object,
        JsonType::Scalar => node != NodeKind::Array && node != NodeKind::Object,
    }
}

/// [§8.22 GR3b to GR3e] Whether `item` is of the kind `kind`.
fn is_of(item: &Item, kind: JsonType) -> bool {
    let structured = matches!(item, Item::Array(_) | Item::Object(_));
    match kind {
        JsonType::Value => true,
        JsonType::Array => matches!(item, Item::Array(_)),
        JsonType::Object => matches!(item, Item::Object(_)),
        JsonType::Scalar => !structured,
    }
}

#[cfg(test)]
mod tests;

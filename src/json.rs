//! SQL/JSON [ISO/IEC 9075-2:2023 §4.48]: the SQL/JSON data model the
//! SQL/JSON functions and the SQL/JSON path language work on [§4.48.3].
//!
//! An SQL/JSON item is a scalar, the SQL/JSON null, an array or an object
//! (`json/item.rs`). A scalar is an SQL value, so that items are compared
//! and computed with as SQL values are (`value.rs`). A JSON text is parsed
//! to the item it is by RFC 8259's grammar (`json/text.rs`) [§9.42].
//! An SQL/JSON path expression, which says what the SQL/JSON query
//! operators take of an item, is parsed to its syntax tree by the lexical
//! rules and Format of the SQL/JSON path language (`json/path.rs`) [§9.45,
//! §9.46], and evaluated by its General Rules to the SQL/JSON sequence it
//! gives of an item (`Path::evaluate`) [§9.46 GR]. An item is serialized
//! as JSON text (`json/serialize.rs`) [§9.43].
//!
//! The SQL/JSON query operators JSON_EXISTS [§8.23], JSON_VALUE [§6.28]
//! and JSON_QUERY [§6.35] are functions of their inputs already evaluated
//! (`json/query.rs`): `json_exists`, `json_value` and `json_query`, of a
//! `CommonSyntax` [§10.14], its context item's JSON text, parsed path and
//! PASSING arguments, and of each operator's clauses, every clause the
//! value its Syntax Rules make implicit if it is not written. They apply
//! the rules the operators share: processing the common syntax
//! (`process`) [§9.47], casting a sequence to an SQL type
//! (`cast_sequence`) [§9.48], converting one to an item
//! (`sequence_to_item`) [§9.44] and serializing one to a string type
//! (`serialize_sequence`) [§9.49]. \[DESIGN\] Of these, and of parsing
//! and serializing JSON text and the path's syntax tree, only what another
//! crate needs is public; the rest is the crate's own.
//!
//! The `<JSON predicate>` IS JSON [§8.22] is `is_json` and `is_not_json`
//! (`json/predicate.rs`), and the constructor functions JSON_OBJECT and
//! JSON_ARRAY [§6.34] and JSON_OBJECTAGG and JSON_ARRAYAGG [§10.11] are
//! `json_object`, `json_array`, `json_array_by_query` and the accumulator
//! `JsonObjectAgg` (`json/construct.rs`), of their inputs already
//! evaluated, returning JSON text.

pub mod binary;
mod construct;
mod item;
mod parse_fn;
mod path;
mod predicate;
mod query;
mod scalar;
mod serialize;
mod serialize_fn;
mod table;
mod text;

pub use construct::{
    ArrayOptions, Input, JsonObjectAgg, MAX_ELEMENTS, MAX_MEMBERS, NameAndValue, ObjectOptions, OnNull, json_array, json_array_by_query,
    json_object,
};
pub use item::{Item, Member, Scalar};
pub use parse_fn::{JSON_PARSE, JsonParse};
pub(crate) use path::{Accessor, Comparison, Expr, Method, Predicate, Subscript};
pub use path::{Path, parse_path};
pub use predicate::{JsonType, is_json, is_not_json};
pub use query::{
    CommonSyntax, ExistsBehavior, IMPLICIT_RETURNING, Passing, QueryBehavior, QueryOptions, Quotes, ValueBehavior, Warned, Wrapper,
    json_exists, json_query, json_value, sequence_to_item,
};
pub(crate) use query::{Taken, cast_sequence, is_exception, json_value_taken, serialize_sequence};
pub use scalar::{JSON_SCALAR, JsonScalar, scalar_operand};
pub(crate) use serialize::{serialize, serialize_as};
pub use serialize_fn::{JSON_SERIALIZE, JsonSerialize};
pub use table::{Cursor, Formatted, JsonTable, PlanNode, Primitive, Slot, TableColumn, TableErrorBehavior, TableRow};
pub use text::Keys;
pub(crate) use text::parse;

use crate::error::{Code, SqlError};

/// \[DESIGN\] How deep arrays and objects nest at most in an SQL/JSON item,
/// so every walk of one, parsing JSON text, comparing and serializing, is
/// bounded (RULES.md R-CF-6, D84). RFC 8259 §9 lets a parser limit the
/// depth of nesting and §9.42 sets none, so one deeper is SQLCODE -900
/// (D18). The value is no measurement: it is twice the
/// path language's bound, `json::path::MAX_PATH_NESTING`, 64; no stack has
/// been measured for an item this deep.
pub(crate) const MAX_NESTING: usize = 128;

/// The error of an SQL/JSON item beyond turborel's limits, `why` (RULES.md
/// D18).
fn beyond(why: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[DESIGN] the SQL/JSON item is beyond turborel's limits: {why}"))
}

#[cfg(test)]
mod tests;

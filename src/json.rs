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
//! the rules the operators share, which are public too: processing the
//! common syntax (`process`) [§9.47], casting a sequence to an SQL type
//! (`cast_sequence`) [§9.48], converting one to an item
//! (`sequence_to_item`) [§9.44] and serializing one to a string type
//! (`serialize_sequence`) [§9.49].
//!
//! The `<JSON predicate>` IS JSON [§8.22] is `is_json` and `is_not_json`
//! (`json/predicate.rs`), and the constructor functions JSON_OBJECT and
//! JSON_ARRAY [§6.34] and JSON_OBJECTAGG and JSON_ARRAYAGG [§10.11] are
//! `json_object`, `json_array`, `json_array_by_query` and the accumulators
//! `JsonObjectAgg` and `JsonArrayAgg` (`json/construct.rs`), of their
//! inputs already evaluated, returning JSON text.

mod construct;
mod item;
mod path;
mod predicate;
mod query;
mod serialize;
mod text;

pub use construct::{
    ArrayOptions, Input, JsonArrayAgg, JsonObjectAgg, MAX_ELEMENTS, MAX_MEMBERS, NameAndValue, ObjectOptions, OnNull, json_array,
    json_array_by_query, json_object,
};
pub use item::{Item, Member, Scalar};
pub use path::{Accessor, Comparison, Expr, Method, Mode, Operator, Path, Predicate, Sign, Subscript, parse_path};
pub use predicate::{JsonType, is_json, is_not_json};
pub use query::{
    CommonSyntax, ExistsBehavior, IMPLICIT_RETURNING, Passing, QueryBehavior, QueryOptions, Quotes, ValueBehavior, Warned, Wrapper,
    cast_sequence, is_exception, json_exists, json_query, json_value, process, sequence_to_item, serialize_sequence,
};
pub use serialize::{serialize, serialize_as};
pub use text::{Keys, parse};

use crate::error::{Code, SqlError};

/// \[DESIGN\] How deep arrays and objects nest at most in an SQL/JSON item,
/// so every walk of one is bounded (RULES.md R-CF-6).
pub const MAX_NESTING: usize = 128;

/// The error of an SQL/JSON item beyond turborel's limits, `why` (RULES.md
/// D18).
fn beyond(why: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[DESIGN] the SQL/JSON item is beyond turborel's limits: {why}"))
}

#[cfg(test)]
mod tests;

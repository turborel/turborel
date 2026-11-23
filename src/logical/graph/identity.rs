//! \[EXTENSION\] [GS §5 R5.1] An element's identity, its element table
//! and its key, as ELEMENT_ID gives it: the canonical JSON text
//! `{"table":"ACCOUNT","key":[6]}`, its table the element table's alias
//! and its key the values of its key's columns in order. Two elements are
//! one exactly when their identities are equal strings, so SAME,
//! ALL_DIFFERENT and the endpoint predicates compare them as strings.
//! VERTEX_ID and EDGE_ID give it as a JSON value.

use crate::error::Result;
use crate::functions::ScalarFunction;
use crate::json::{Item, Member, Scalar, serialize};
use crate::value::json::Json;
use crate::value::{CharUnits, DataType, Value};

/// \[DESIGN\] The longest identity, in characters.
pub const IDENTITY_LENGTH: u32 = 8192;

/// The member of an identity naming its element table.
const TABLE_MEMBER: &str = "table";
/// The member of an identity holding its key.
const KEY_MEMBER: &str = "key";

/// The type of an identity: CHARACTER VARYING.
pub fn identity_type() -> DataType {
    DataType::Varying { length: IDENTITY_LENGTH, units: CharUnits::Characters }
}

/// The JSON item of `value`, a key's: its scalar, or its text if it is of
/// a type no SQL/JSON scalar has.
fn key_item(value: &Value) -> Item {
    if value.is_null() {
        return Item::Null;
    }
    Item::Scalar(Scalar::new(value.clone()).unwrap_or_else(|| Scalar::from(value.to_string())))
}

/// The identity of the element of element table `alias` whose key is
/// `key`, as an SQL/JSON item.
pub fn identity_item(alias: &str, key: &[Value]) -> Item {
    Item::Object(vec![
        Member { key: TABLE_MEMBER.to_owned(), value: Item::Scalar(Scalar::from(alias.to_owned())) },
        Member { key: KEY_MEMBER.to_owned(), value: Item::Array(key.iter().map(key_item).collect()) },
    ])
}

/// The identity of the element of element table `alias` whose key is
/// `key`, as its text.
///
/// # Errors
///
/// SQLCODE -910 if it cannot be serialized.
pub fn identity_text(alias: &str, key: &[Value]) -> Result<String> {
    serialize(&identity_item(alias, key))
}

/// The identity of the element of element table `alias` whose key is
/// `key`, as its text; of a node or edge table's element, if `native`,
/// its pseudo-column `$NODE_ID` or `$EDGE_ID`, its key, so a variable's
/// `$NODE_ID` is its row's [GS §4.2, R5.1].
///
/// # Errors
///
/// SQLCODE -910 if it cannot be serialized.
pub fn element_text(alias: &str, native: bool, key: &[Value]) -> Result<String> {
    if let (true, [Value::Char(id)]) = (native, key) {
        return Ok(id.to_string());
    }
    identity_text(alias, key)
}

/// \[EXTENSION\] The identity of an element of one element table, of its
/// key's values: called by no name, made by the binder alone.
#[derive(Debug)]
pub struct ElementIdentity {
    /// The element table's alias.
    pub alias: String,
    /// Whether it is a node or edge table keyed by its pseudo-column.
    pub native: bool,
}

impl ScalarFunction for ElementIdentity {
    fn name(&self) -> &'static str {
        "ELEMENT_ID"
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(identity_type())
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        // A key of a null is the identity of no element: an edge's end
        // whose columns are null joins none (§4.1 R4.2).
        if args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        Ok(Value::Char(element_text(&self.alias, self.native, args)?.into()))
    }
}

/// \[EXTENSION\] VERTEX_ID and EDGE_ID: an identity as a JSON value.
#[derive(Debug)]
pub struct IdentityJson;

impl ScalarFunction for IdentityJson {
    fn name(&self) -> &'static str {
        "VERTEX_ID"
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Json)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        match args.first() {
            Some(Value::Char(text)) => Ok(Value::Json(Json::parse(text, crate::json::Keys::Any)?)),
            Some(_) | None => Ok(Value::Null),
        }
    }
}

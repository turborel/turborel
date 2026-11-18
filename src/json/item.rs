//! SQL/JSON items [ISO/IEC 9075-2:2023 §4.48.3]: scalars, the SQL/JSON
//! null, arrays and objects, and when two of them are equivalent.

use crate::error::Result;
use crate::value::{DecFloat, Decimal, Value};

/// [§4.48.3] An SQL/JSON item.
#[derive(Debug, Clone)]
pub enum Item {
    /// The SQL/JSON null, "distinct from any value of any SQL type".
    Null,
    /// An SQL/JSON scalar.
    Scalar(Scalar),
    /// An SQL/JSON array: its elements, in order.
    Array(Vec<Item>),
    /// An SQL/JSON object: its members. The standard gives them no order;
    /// they are kept in the order they were given, two of one key both.
    Object(Vec<Member>),
}

/// [§4.48.3] An SQL/JSON scalar: a non-null character string, number,
/// truth value or datetime.
#[derive(Debug, Clone)]
pub struct Scalar(Value);

/// [§4.48.3] An SQL/JSON member: a key and the value bound to it.
#[derive(Debug, Clone)]
pub struct Member {
    /// The key.
    pub key: String,
    /// The bound value.
    pub value: Item,
}

impl Scalar {
    /// `value` as an SQL/JSON scalar; none if it is the null value or of a
    /// type no SQL/JSON scalar has.
    pub fn new(value: Value) -> Option<Scalar> {
        match value {
            Value::Char(_) | Value::Exact(_) | Value::Approx(_) | Value::DecFloat(_) | Value::Bool(_) | Value::Datetime(_) => {
                Some(Scalar(value))
            }
            Value::Null
            | Value::Binary(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Row(_)
            | Value::Lob(_)
            | Value::Multiset(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Json(_)
            | Value::Xml(_)
            | Value::Extension(_) => None,
        }
    }

    /// The SQL value this scalar is.
    pub fn value(&self) -> &Value {
        &self.0
    }
}

impl From<String> for Scalar {
    fn from(string: String) -> Scalar {
        Scalar(Value::Char(string))
    }
}

impl From<bool> for Scalar {
    fn from(truth: bool) -> Scalar {
        Scalar(Value::Bool(truth))
    }
}

impl From<Decimal> for Scalar {
    fn from(number: Decimal) -> Scalar {
        Scalar(Value::Exact(number))
    }
}

impl From<DecFloat> for Scalar {
    fn from(number: DecFloat) -> Scalar {
        Scalar(Value::DecFloat(number))
    }
}

impl Item {
    /// The scalar this item is, if it is one.
    pub fn as_scalar(&self) -> Option<&Scalar> {
        if let Item::Scalar(s) = self { Some(s) } else { None }
    }

    /// The elements of the array this item is, if it is one.
    pub fn as_array(&self) -> Option<&[Item]> {
        if let Item::Array(elements) = self { Some(elements) } else { None }
    }

    /// The members of the object this item is, if it is one.
    pub fn as_object(&self) -> Option<&[Member]> {
        if let Item::Object(members) = self { Some(members) } else { None }
    }

    /// [§4.48.3] Whether the two items are equivalent: equal scalars, two
    /// SQL/JSON nulls, arrays whose corresponding elements are equivalent,
    /// or objects whose members pair off, one to one, by equivalent keys
    /// and bound values. Exactly then are their comparison keys
    /// (`binary::document_key`) the same: of a string its code points, no
    /// space padded; of a number its value; of an object its members
    /// ordered by key and value, so a bijection is found whatever order
    /// either keeps.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if the items nest deeper than `MAX_NESTING`.
    pub fn equivalent(&self, other: &Item) -> Result<bool> {
        let key = |item: &Item| super::binary::encode(item).and_then(|bytes| super::binary::document_key(&bytes));
        Ok(key(self)? == key(other)?)
    }
}

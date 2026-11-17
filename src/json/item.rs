//! SQL/JSON items [ISO/IEC 9075-2:2023 §4.48.3]: scalars, the SQL/JSON
//! null, arrays and objects, and when two of them are equivalent.

use std::cmp::Ordering;

use super::{MAX_NESTING, beyond};
use crate::error::Result;
use crate::value::{DecFloat, Decimal, Value, compare};

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
            | Value::Extension(_) => None,
        }
    }

    /// The SQL value this scalar is.
    pub fn value(&self) -> &Value {
        &self.0
    }

    /// Whether the two are comparable and equal, as SQL values are.
    fn equals(&self, other: &Scalar) -> bool {
        compare(&self.0, &other.0) == Some(Ordering::Equal)
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
    /// and bound values.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if the items nest deeper than `MAX_NESTING`.
    pub fn equivalent(&self, other: &Item) -> Result<bool> {
        equivalent(self, other, 0)
    }
}

/// Whether `a` and `b`, `depth` arrays and objects within the items
/// compared, are equivalent.
fn equivalent(a: &Item, b: &Item, depth: usize) -> Result<bool> {
    if depth > MAX_NESTING {
        return Err(beyond("arrays and objects nest too deep"));
    }
    match a {
        Item::Null => Ok(matches!(b, Item::Null)),
        Item::Scalar(x) => Ok(b.as_scalar().is_some_and(|y| x.equals(y))),
        Item::Array(x) => b.as_array().map_or(Ok(false), |y| arrays(x, y, depth.saturating_add(1))),
        Item::Object(x) => b.as_object().map_or(Ok(false), |y| objects(x, y, depth.saturating_add(1))),
    }
}

/// Whether two arrays are "of the same length, and corresponding elements
/// … are equivalent".
fn arrays(x: &[Item], y: &[Item], depth: usize) -> Result<bool> {
    if x.len() != y.len() {
        return Ok(false);
    }
    for (a, b) in x.iter().zip(y) {
        if !equivalent(a, b, depth)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether two objects "have the same number of members and there exists
/// a bijection" between them of equivalent members. Equivalence is
/// transitive, so each member of `x` may take the first member of `y`
/// left that it is equivalent to.
fn objects(x: &[Member], y: &[Member], depth: usize) -> Result<bool> {
    if x.len() != y.len() {
        return Ok(false);
    }
    let mut taken = vec![false; y.len()];
    for member in x {
        if !paired(member, y, &mut taken, depth)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether a member of `y` not yet `taken` is equivalent to `member`, the
/// first such taken. Keys are equivalent when "all pairs of corresponding
/// code points are equal".
fn paired(member: &Member, y: &[Member], taken: &mut [bool], depth: usize) -> Result<bool> {
    for (other, used) in y.iter().zip(taken.iter_mut()) {
        if *used || member.key != other.key {
            continue;
        }
        if equivalent(&member.value, &other.value, depth)? {
            *used = true;
            return Ok(true);
        }
    }
    Ok(false)
}

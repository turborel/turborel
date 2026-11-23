//! [ISO/IEC 9075-2:2023 §4.9.3] A structured type: a user-defined type
//! whose representation is a list of attributes, each of its own type,
//! inherited from its supertype or its own [§11.51 GR1f]. Its values are
//! `Value::Structured`: each its most specific type [§4.9.3.3], which
//! method invocation dispatches on [§9.18 GR3b v] and TREAT and the type
//! predicate test [§6.17 GR2, §8.19 GR3], and its attributes' values in
//! their ordinal positions, a supertype's first.
//!
//! \[DESIGN\] A structured type is known by its name alone: what else it
//! is, its attributes, its methods and its ordering, the catalog holds, so
//! the binder alone reads them, and a type declared of it is never stale as
//! ALTER TYPE changes them (RULES.md D81). A value names its most specific
//! type, so it is read without the catalog, as a log record is.
//!
//! Its values compare as its ordering says [§4.9.5], which the binder makes
//! each operand of a comparison, grouping or ordering operation carry: its
//! key, the value of the map function of MAP, the rank among the operation's
//! values of RELATIVE, or of STATE its specific type and each attribute's
//! key [§11.65 GR1]. A value with a key compares, hashes and is a key's
//! part by it; one without, as its specific type and attributes do, in
//! the identity of §9.10 GR2e.
//!
//! ```text
//! value:  its type's schema and name, then its attributes as a row's parts
//!         (`composite`), then 0, or 1 and its key, of a spilled operand
//! record: the same, of no key: an attribute's type is of the catalog
//! ```

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use super::composite::{self, mismatch};
use super::{Descriptor, SqlType, USER_DEFINED_NAME};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::value::cast::uncastable;
use crate::value::{Bucket, DataType, NormKey, Value, unassignable};

/// [ISO/IEC 9075-2:2023 §4.9.1] The name of a user-defined type: its
/// schema's `<authorization identifier>` and its own `<identifier>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UdtName {
    /// The schema's name.
    pub schema: String,
    /// Its `<identifier>` in that schema.
    pub name: String,
}

impl UdtName {
    /// The type `name` of schema `schema`.
    pub fn new(schema: &str, name: &str) -> UdtName {
        UdtName { schema: schema.to_owned(), name: name.to_owned() }
    }
}

impl fmt::Display for UdtName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.schema, self.name)
    }
}

/// [ISO/IEC 9075-2:2023 §4.9.3] A structured type, as a type declared of
/// it holds it: its name, and its proper supertypes' names, its direct
/// supertype first [§9.7 SR4], which no statement changes once the type is
/// defined [§11.51 SR8j] (RULES.md D81).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StructuredType {
    /// Its name.
    pub name: UdtName,
    /// Its proper supertypes, the nearest first.
    pub supertypes: Vec<UdtName>,
}

impl StructuredType {
    /// [ISO/IEC 9075-2:2023 §9.7 SR4] Its type precedence list: itself,
    /// then each proper supertype, the nearest first.
    pub fn precedence(&self) -> impl Iterator<Item = &UdtName> {
        std::iter::once(&self.name).chain(&self.supertypes)
    }

    /// [ISO/IEC 9075-2:2023 §4.9.3.1] Whether it is a subtype of the type
    /// `of`: itself or one of its proper subtypes.
    pub fn is_subtype_of(&self, of: &UdtName) -> bool {
        self.precedence().any(|n| n == of)
    }

    /// The maximal supertype of its subtype family [§4.9.3.1].
    pub fn maximal(&self) -> &UdtName {
        self.supertypes.last().unwrap_or(&self.name)
    }
}

/// [ISO/IEC 9075-2:2023 §4.9.3] A value of a structured type.
#[derive(Debug, Clone)]
pub struct Structured {
    /// Its most specific type [§4.9.3.3].
    pub of: Arc<UdtName>,
    /// Its attributes' values, in their ordinal positions [§11.52 GR4c].
    pub attributes: Arc<[Value]>,
    /// \[DESIGN\] Of an operand of a comparison, grouping or ordering
    /// operation, its key, which it compares by (RULES.md D81).
    pub key: Option<Value>,
}

impl Structured {
    /// The value of most specific type `of` and `attributes`, of no key.
    pub fn new(of: Arc<UdtName>, attributes: Arc<[Value]>) -> Structured {
        Structured { of, attributes, key: None }
    }
}

impl DataType {
    /// The structured type `name` of proper supertypes `supertypes`, the
    /// nearest first.
    pub fn structured(name: UdtName, supertypes: Vec<UdtName>) -> DataType {
        DataType::Structured(Arc::new(StructuredType { name, supertypes }))
    }

    /// The structured type it is, if it is one.
    pub fn structured_type(&self) -> Option<&StructuredType> {
        if let DataType::Structured(s) = self { Some(s) } else { None }
    }

    /// The name of the user-defined type it is, structured or distinct.
    pub fn udt_name(&self) -> Option<UdtName> {
        match self {
            DataType::Structured(s) => Some(s.name.clone()),
            DataType::Distinct(d) => Some(UdtName::new(&d.schema, &d.name)),
            DataType::Character { .. }
            | DataType::Varying { .. }
            | DataType::Exact { .. }
            | DataType::Approximate { .. }
            | DataType::DecFloat { .. }
            | DataType::Binary { .. }
            | DataType::VarBinary { .. }
            | DataType::Lob { .. }
            | DataType::Boolean
            | DataType::Date
            | DataType::Time { .. }
            | DataType::Timestamp { .. }
            | DataType::Interval(_)
            | DataType::Array(_)
            | DataType::Multiset(_)
            | DataType::Row(_)
            | DataType::Reference(_)
            | DataType::Json
            | DataType::Extension(_)
            | DataType::Xml(_) => None,
        }
    }
}

impl Value {
    /// The structured value it is, if it is one.
    pub fn structured(&self) -> Option<&Structured> {
        if let Value::Structured(s) = self { Some(s) } else { None }
    }
}

/// The flag of a value written without a key, and of one with.
const UNKEYED: u8 = 0;
const KEYED: u8 = 1;

/// The kind of every structured type.
#[derive(Debug)]
pub struct StructuredKind;

/// The one value of it.
pub static STRUCTURED: StructuredKind = StructuredKind;

/// The name `t` is of, if it is a structured type.
fn structured_type(t: &DataType) -> Option<&UdtName> {
    t.structured_type().map(|s| &s.name)
}

/// [ISO/IEC 9075-2:2023 §9.10 GR2e] How two structured values compare
/// without keys: by their most specific types, then their attributes, as
/// a row's fields; by their keys if both have one.
pub(crate) fn compare_structured(a: &Structured, b: &Structured) -> Option<Ordering> {
    if let (Some(x), Some(y)) = (&a.key, &b.key) {
        return crate::value::compare(x, y);
    }
    let of = a.of.cmp(&b.of);
    if of.is_eq() { composite::compare_parts(&a.attributes, &b.attributes) } else { Some(of) }
}

/// The parts of `s` as compared without a key: its type's name, then its
/// attributes.
fn identity_parts(s: &Structured) -> Vec<Value> {
    let mut parts = Vec::with_capacity(s.attributes.len().saturating_add(1));
    parts.push(Value::Char(s.of.to_string().into()));
    parts.extend(s.attributes.iter().cloned());
    parts
}

/// The bucket of `s`: its key's, else its identity's.
pub(crate) fn bucket_structured(s: &Structured) -> Option<Bucket> {
    match &s.key {
        Some(k) => crate::value::bucket(k),
        None => composite::bucket_parts(&identity_parts(s)),
    }
}

/// The form `Value::norm` gives `s`: its key's, else its identity's.
pub(crate) fn norm_structured(s: &Structured) -> NormKey {
    match &s.key {
        Some(k) => k.norm(),
        None => composite::norm_parts(&identity_parts(s)),
    }
}

/// Appends a value's bytes: its type's name, then its attributes.
fn put_structured(v: &Value, out: &mut Vec<u8>) {
    let Some(s) = v.structured() else {
        out.put_str("");
        out.put_str("");
        composite::put_parts(&[], out);
        out.put_u8(UNKEYED);
        return;
    };
    out.put_str(&s.of.schema);
    out.put_str(&s.of.name);
    composite::put_parts(&s.attributes, out);
    if let Some(k) = &s.key {
        out.put_u8(KEYED);
        crate::value::codec::put_value(out, k);
    } else {
        out.put_u8(UNKEYED);
    }
}

/// Reads a value `put_structured` wrote.
fn read_structured(r: &mut Reader<'_>) -> Result<Value> {
    let (schema, name) = (r.str()?, r.str()?);
    let attributes = composite::read_parts(r)?;
    let key = match r.u8()? {
        UNKEYED => None,
        KEYED => Some(crate::value::codec::value(r)?),
        _ => return Err(r.corrupt("a structured value's key flag is 0 or 1")),
    };
    Ok(Value::Structured(Arc::new(Structured { of: Arc::new(UdtName { schema, name }), attributes, key })))
}

/// Appends the name `name` and the names `supertypes`, their count first.
pub(crate) fn put_names(name: &UdtName, supertypes: &[UdtName], out: &mut Vec<u8>) {
    out.put_str(&name.schema);
    out.put_str(&name.name);
    out.put_u32(u32::try_from(supertypes.len()).unwrap_or(u32::MAX));
    for n in supertypes {
        out.put_str(&n.schema);
        out.put_str(&n.name);
    }
}

/// Reads the names `put_names` wrote.
pub(crate) fn read_names(r: &mut Reader<'_>) -> Result<(UdtName, Vec<UdtName>)> {
    let name = UdtName { schema: r.str()?, name: r.str()? };
    let count = r.u32()?;
    // Pushed one by one, so a corrupt count fails at the first missing.
    let mut supertypes = Vec::new();
    for _ in 0..count {
        supertypes.push(UdtName { schema: r.str()?, name: r.str()? });
    }
    Ok((name, supertypes))
}

impl SqlType for StructuredKind {
    fn name(&self) -> &'static str {
        USER_DEFINED_NAME
    }

    /// [ISO/IEC 9075-2:2023 §6.1] Its `<path-resolved user-defined type
    /// name>`, its schema's name made explicit.
    fn written(&self, t: &DataType) -> String {
        structured_type(t).map_or_else(|| USER_DEFINED_NAME.to_owned(), ToString::to_string)
    }

    /// [ISO/IEC 9075-2:2023 §4.9.5] With a structured type: whether of one
    /// subtype family, with comparison types, the binder alone knows, of
    /// the catalog, as it binds each comparison.
    fn comparable(&self, other: &DataType) -> bool {
        matches!(other, DataType::Structured(_))
    }

    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        compare_structured(a.structured()?, b.structured()?)
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        bucket_structured(v.structured()?)
    }

    /// [ISO/IEC 9075-2:2023 §9.2 GR3b xxiv] The value itself, its most
    /// specific type kept: that it is of a subtype of the target, the
    /// binder checked \[SR3\].
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        match (&v, structured_type(t)) {
            (Value::Structured(s), Some(_)) => {
                Ok(Value::Structured(Arc::new(Structured::new(Arc::clone(&s.of), Arc::clone(&s.attributes)))))
            }
            (Value::Null, _) => Ok(v),
            _ => unassignable(&v, t, rule),
        }
    }

    /// [ISO/IEC 9075-2:2023 §6.13 GR3a] The value itself, to a supertype of
    /// its declared type, as the binder allows alone \[SR12\].
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        match (&v, structured_type(t)) {
            (Value::Structured(_), Some(_)) => self.assign(v, t, "ISO/IEC 9075-2:2023 §6.13 GR3a"),
            // [ISO/IEC 9075-2:2023 §6.13 GR2] the null value, of any type.
            (Value::Null, _) => Ok(v),
            _ => uncastable(&v, t),
        }
    }

    /// \[DESIGN\] No `<literal>` is of a structured type: its type's name,
    /// then its attributes' literals in parentheses, as NEW and its
    /// mutators would make it.
    fn literal(&self, v: &Value) -> String {
        v.structured().map_or_else(|| "NULL".to_owned(), |s| format!("{}({})", s.of, composite::literals(&s.attributes)))
    }

    fn text(&self, v: &Value) -> String {
        self.literal(v)
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    /// [ISO/IEC 9075-2:2023 §4.9.1] USER-DEFINED, and its name.
    fn descriptor(&self, t: &DataType) -> Descriptor {
        let named = structured_type(t).map(|n| (n.schema.clone(), n.name.clone()));
        Descriptor { user_defined: named, ..Descriptor::of(USER_DEFINED_NAME) }
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        put_structured(v, out);
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        r.nested(read_structured)
    }

    /// Its schema's name and its own, then its proper supertypes', their
    /// count first.
    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        if let Some(s) = t.structured_type() {
            put_names(&s.name, &s.supertypes, out);
        }
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        let (name, supertypes) = read_names(r)?;
        Ok(DataType::structured(name, supertypes))
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        if v.structured().is_none() {
            return Err(mismatch(v, t));
        }
        let mut out = Vec::new();
        put_structured(v, &mut out);
        Ok(out)
    }

    fn decode_record(&self, field: &[u8], _: &DataType) -> Option<Value> {
        let mut r = Reader::new(field, "record");
        let v = read_structured(&mut r).ok()?;
        r.finish().ok()?;
        Some(v)
    }

    /// None: a constraint's or an index's key holds the comparison key
    /// of a structured value, a column of its own, not the value
    /// (RULES.md D81).
    fn encode_key(&self, v: &Value, t: &DataType, _: &mut Vec<u8>) -> Result<()> {
        Err(SqlError::new(Code::Api, format!("[DESIGN] {v} of {t} is in no key: its comparison key is (RULES.md D81)")))
    }

    fn decode_key(&self, _: &mut &[u8], _: &DataType) -> Result<Value> {
        Err(super::malformed())
    }

    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        v.structured().map(|_| (v.clone(), true))
    }
}

// Unit tests: the kind's text, comparison by keys, CAST and assignment,
// and its stored form.
#[cfg(test)]
mod tests;

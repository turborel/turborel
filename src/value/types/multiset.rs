//! [ISO/IEC 9075-2:2023 §4.12.3] MULTISET: an unordered collection whose
//! elements are each a value of its element type, no element at a place.
//! Its values are `Value::Multiset`, at most `MAX_MULTISET_CARDINALITY`
//! elements, kept in the order they were made, which means nothing.
//!
//! Multisets take part in equality operations alone [§9.11, §9.12 SR3,
//! §9.14 SR3], so no multiset is in a key; two are equal as `multisets`
//! pairs their elements, and a multiset's bytes in a record are its
//! elements', as `composite` writes them (RULES.md D80).

use std::cmp::Ordering;
use std::sync::Arc;

use super::composite::{self, mismatch};
use super::{Descriptor, SqlType, tags};
use crate::bytes::Reader;
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::value::cast::{cast, uncastable};
use crate::value::{Bucket, DataType, Value, assign, unassignable};

/// \[DESIGN\] The most elements a multiset holds: an array's maximum
/// cardinality (IL008), so each multiset is unnested as an array of it
/// [ISO/IEC 9075-2:2023 §7.6 SR8e i] and cast to one.
pub const MAX_MULTISET_CARDINALITY: u32 = super::MAX_ARRAY_CARDINALITY;

/// [ISO/IEC 9075-2:2023 §6.1] `<multiset type>` ::= `<data type>`
/// MULTISET, its name.
pub const MULTISET_NAME: &str = "MULTISET";

/// [ISO/IEC 9075-2:2023 §4.12.1] A multiset type's descriptor: its
/// element type.
#[derive(Debug, Clone, PartialEq)]
pub struct MultisetType {
    /// The element type.
    pub element: DataType,
}

impl DataType {
    /// The multiset type of `element`.
    pub fn multiset(element: DataType) -> DataType {
        DataType::Multiset(Arc::new(MultisetType { element }))
    }

    /// [ISO/IEC 9075-2:2023 §6.50 SR1] The multiset type it is, or the
    /// source of the distinct type it is, if that is one.
    pub fn multiset_type(&self) -> Option<&MultisetType> {
        if let DataType::Multiset(m) = self.represented() { Some(m) } else { None }
    }
}

/// The kind of MULTISET.
#[derive(Debug)]
pub struct MultisetKind;

/// The one value of it.
pub static MULTISET: MultisetKind = MultisetKind;

/// The multiset type `t` is, if it is one.
fn multiset_type(t: &DataType) -> Option<&MultisetType> {
    if let DataType::Multiset(m) = t { Some(m) } else { None }
}

/// The elements of `v`, if it is a multiset.
fn elements(v: &Value) -> Option<&[Value]> {
    if let Value::Multiset(e) = v { Some(e) } else { None }
}

/// The most elements as a count.
pub(crate) fn most() -> usize {
    usize::try_from(MAX_MULTISET_CARDINALITY).unwrap_or(usize::MAX)
}

/// [ISO/IEC 9075-2:2023 §10.9 GR3b] A multiset of `cardinality` elements,
/// more than this implementation holds, made as the rule `rule` makes it.
///
/// # Errors
///
/// Always: SQLSTATE 2200Q.
pub(crate) fn overflow<T>(rule: &str, cardinality: usize) -> Result<T> {
    fail_with(
        Code::Data,
        SqlState::MULTISET_VALUE_OVERFLOW,
        format!(
            "[ISO/IEC 9075-2:2023 {rule}] a multiset of {cardinality} elements exceeds this implementation's limit of {MAX_MULTISET_CARDINALITY}"
        ),
    )
}

/// The multiset of `elements`, if no more than this implementation holds,
/// made as the rule `rule` makes it.
///
/// # Errors
///
/// SQLSTATE 2200Q if they are more.
pub(crate) fn made(elements: Vec<Value>, rule: &str) -> Result<Value> {
    if elements.len() > most() {
        return overflow(rule, elements.len());
    }
    Ok(Value::Multiset(elements.into()))
}

impl SqlType for MultisetKind {
    fn name(&self) -> &'static str {
        MULTISET_NAME
    }

    /// [ISO/IEC 9075-2:2023 §4.12.1] Its element type's designator, then
    /// MULTISET.
    fn written(&self, t: &DataType) -> String {
        multiset_type(t).map_or_else(|| MULTISET_NAME.to_owned(), |m| format!("{} {MULTISET_NAME}", m.element))
    }

    /// [ISO/IEC 9075-2:2023 §4.12.4] Multisets with multisets alone; whose
    /// element types are comparable, `DataType::comparable` says.
    fn comparable(&self, other: &DataType) -> bool {
        matches!(other, DataType::Multiset(_))
    }

    /// [ISO/IEC 9075-2:2023 §8.2 GR1b iii] As `value::compare` says: equal
    /// or not, none if unknown.
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        crate::value::compare(a, b)
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        crate::value::multisets::bag_bucket(elements(v)?)
    }

    /// [ISO/IEC 9075-2:2023 §9.2 GR3b xxiii] The multiset, each element
    /// assigned to the element type \[SR6c\].
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        let (Some(m), Some(given)) = (multiset_type(t), elements(&v)) else { return unassignable(&v, t, rule) };
        let assigned = given.iter().map(|e| assign(e.clone(), &m.element, rule)).collect::<Result<Vec<_>>>()?;
        Ok(Value::Multiset(assigned.into()))
    }

    /// [ISO/IEC 9075-2:2023 §6.13 GR5d, GR6c] Element by element, of a
    /// multiset or an array.
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        let given = match &v {
            Value::Multiset(e) | Value::Array(e) => e,
            Value::Null => return Ok(v),
            Value::Char(_)
            | Value::Exact(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Lob(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Row(_)
            | Value::Extension(_)
            | Value::Xml(_) => return uncastable(&v, t),
        };
        let Some(m) = multiset_type(t) else { return uncastable(&v, t) };
        let cast = given.iter().map(|e| cast(e.clone(), &m.element)).collect::<Result<Vec<_>>>()?;
        Ok(Value::Multiset(cast.into()))
    }

    /// [ISO/IEC 9075-2:2023 §6.52] `<multiset value constructor by
    /// enumeration>` of its elements' literals; an `<empty specification>`
    /// [§6.5] of none.
    fn literal(&self, v: &Value) -> String {
        format!("{MULTISET_NAME}[{}]", elements(v).map(composite::literals).unwrap_or_default())
    }

    fn text(&self, v: &Value) -> String {
        self.literal(v)
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of(MULTISET_NAME)
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        composite::put_parts(elements(v).unwrap_or_default(), out);
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        composite::read_parts(r).map(Value::Multiset)
    }

    /// Its element type.
    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        if let Some(m) = multiset_type(t) {
            tags::put_type(out, &m.element);
        }
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        r.nested(|r| Ok(DataType::multiset(tags::data_type(r)?)))
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        let (Some(m), Some(given)) = (multiset_type(t), elements(v)) else { return Err(mismatch(v, t)) };
        composite::encode_parts(given, |_| Some(&m.element), t)
    }

    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value> {
        let m = multiset_type(t)?;
        composite::decode_parts(field, |_| Some(&m.element)).filter(|e| e.len() <= most()).map(Value::Multiset)
    }

    /// None: no multiset is in a key [ISO/IEC 9075-2:2023 §9.12 SR3].
    fn encode_key(&self, v: &Value, t: &DataType, _: &mut Vec<u8>) -> Result<()> {
        Err(SqlError::new(Code::Api, format!("[ISO/IEC 9075-2:2023 §9.12 SR3] {v} of {t} is in no key, as no multiset is")))
    }

    fn decode_key(&self, _: &mut &[u8], _: &DataType) -> Result<Value> {
        Err(super::malformed())
    }

    /// Itself, of a multiset: no multiset is in a key to anchor among.
    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        elements(v).map(|_| (v.clone(), true))
    }
}

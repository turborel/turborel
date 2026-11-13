//! [ISO/IEC 9075-2:2023 §4.12.2] ARRAY: a collection whose elements, each
//! a value of its element type, are at the ordinal positions 1 to its
//! cardinality, which its maximum cardinality bounds. Its values are
//! `Value::Array`.
//!
//! Arrays take part in equality operations alone [§9.11, §9.12 SR3,
//! §9.14 SR3], so no array is in a key, and an array's bytes in a record
//! are its elements', as `composite` writes them (RULES.md D68).

use std::cmp::Ordering;
use std::sync::Arc;

use super::composite::{self, mismatch};
use super::{Descriptor, SqlType, tags};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError, SqlState, fail_with};
use crate::value::cast::{cast, uncastable};
use crate::value::{Bucket, DataType, Value, assign, unassignable};

/// \[DESIGN\] The maximum cardinality of an array type [ISO/IEC
/// 9075-2:2023 §4.12.2] (IL008), and of one written without it [§6.1
/// SR47] (ID072).
pub const MAX_ARRAY_CARDINALITY: u32 = 65_535;

/// [ISO/IEC 9075-2:2023 §6.1] `<array type>` ::= `<data type>` ARRAY [
/// `[` `<maximum cardinality>` `]` ], its name.
pub const ARRAY_NAME: &str = "ARRAY";

/// [ISO/IEC 9075-2:2023 §4.12.1] An array type's descriptor: its element
/// type and its maximum cardinality.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrayType {
    /// The element type.
    pub element: DataType,
    /// The maximum cardinality, 1 to `MAX_ARRAY_CARDINALITY`.
    pub max: u32,
}

impl DataType {
    /// The array type of `element`, of maximum cardinality `max`.
    pub fn array(element: DataType, max: u32) -> DataType {
        DataType::Array(Arc::new(ArrayType { element, max }))
    }
}

/// The kind of ARRAY.
#[derive(Debug)]
pub struct ArrayKind;

/// The one value of it.
pub static ARRAY: ArrayKind = ArrayKind;

/// The array type `t` is, if it is one.
fn array_type(t: &DataType) -> Option<&ArrayType> {
    if let DataType::Array(a) = t { Some(a) } else { None }
}

/// The elements of `v`, if it is an array.
fn elements(v: &Value) -> Option<&[Value]> {
    if let Value::Array(e) = v { Some(e) } else { None }
}

/// The maximum cardinality `max` as a count of elements.
fn bound(max: u32) -> usize {
    usize::try_from(max).unwrap_or(usize::MAX)
}

/// [ISO/IEC 9075-2:2023 §9.2 GR3b xxii 2B, §6.13 GR5c i] An array of
/// `cardinality` elements does not fit a site of maximum cardinality `max`.
///
/// # Errors
///
/// Always: SQLSTATE 2202F.
pub(crate) fn truncated<T>(rule: &str, cardinality: usize, max: u32) -> Result<T> {
    fail_with(
        Code::Data,
        SqlState::ARRAY_DATA_RIGHT_TRUNCATION,
        format!("[ISO/IEC 9075-2:2023 {rule}] an array of {cardinality} elements exceeds the maximum cardinality {max}"),
    )
}

impl SqlType for ArrayKind {
    fn name(&self) -> &'static str {
        ARRAY_NAME
    }

    /// [ISO/IEC 9075-2:2023 §4.12.1] Its element type's designator, then
    /// ARRAY and its maximum cardinality.
    fn written(&self, t: &DataType) -> String {
        array_type(t).map_or_else(|| ARRAY_NAME.to_owned(), |a| format!("{} {ARRAY_NAME}[{}]", a.element, a.max))
    }

    /// [ISO/IEC 9075-2:2023 §4.12.4] Arrays with arrays alone; whose
    /// element types are comparable, `DataType::comparable` says.
    fn comparable(&self, other: &DataType) -> bool {
        matches!(other, DataType::Array(_))
    }

    /// [ISO/IEC 9075-2:2023 §8.2 GR1b ii] As `value::compare` says.
    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        crate::value::compare(a, b)
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        composite::bucket_parts(elements(v)?)
    }

    /// [ISO/IEC 9075-2:2023 §9.2 GR3b xxii] Element by element, those past
    /// the maximum cardinality dropped if each is null \[2A\].
    fn assign(&self, v: Value, t: &DataType, rule: &str) -> Result<Value> {
        let (Some(a), Some(given)) = (array_type(t), elements(&v)) else { return unassignable(&v, t, rule) };
        let max = bound(a.max);
        if given.iter().skip(max).any(|e| !e.is_null()) {
            return truncated("§9.2 GR3b xxii 2B", given.len(), a.max);
        }
        let assigned = given.iter().take(max).map(|e| assign(e.clone(), &a.element, rule)).collect::<Result<Vec<_>>>()?;
        Ok(Value::Array(assigned.into()))
    }

    /// [ISO/IEC 9075-2:2023 §6.13 GR5] Element by element \[GR5b\], an
    /// array longer than the maximum cardinality refused \[GR5c i\].
    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        let (Some(a), Some(given)) = (array_type(t), elements(&v)) else {
            return if v.is_null() { Ok(v) } else { uncastable(&v, t) };
        };
        if given.len() > bound(a.max) {
            return truncated("§6.13 GR5c i", given.len(), a.max);
        }
        let cast = given.iter().map(|e| cast(e.clone(), &a.element)).collect::<Result<Vec<_>>>()?;
        Ok(Value::Array(cast.into()))
    }

    /// [ISO/IEC 9075-2:2023 §6.49] `<array value constructor by
    /// enumeration>` of its elements' literals.
    fn literal(&self, v: &Value) -> String {
        format!("{ARRAY_NAME}[{}]", elements(v).map(composite::literals).unwrap_or_default())
    }

    fn text(&self, v: &Value) -> String {
        self.literal(v)
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    fn descriptor(&self, t: &DataType) -> Descriptor {
        Descriptor { cardinality: array_type(t).map(|a| a.max), ..Descriptor::of(ARRAY_NAME) }
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        composite::put_parts(elements(v).unwrap_or_default(), out);
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        composite::read_parts(r).map(Value::Array)
    }

    /// Its maximum cardinality, then its element type.
    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        if let Some(a) = array_type(t) {
            out.put_u32(a.max);
            tags::put_type(out, &a.element);
        }
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        r.nested(|r| {
            let max = r.u32()?;
            if !(1..=MAX_ARRAY_CARDINALITY).contains(&max) {
                return Err(r.corrupt("an array type's maximum cardinality is out of range"));
            }
            Ok(DataType::array(tags::data_type(r)?, max))
        })
    }

    fn record_width(&self, _: &DataType) -> Option<usize> {
        None
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        let (Some(a), Some(given)) = (array_type(t), elements(v)) else { return Err(mismatch(v, t)) };
        composite::encode_parts(given, |_| Some(&a.element), t)
    }

    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value> {
        let a = array_type(t)?;
        composite::decode_parts(field, |_| Some(&a.element)).filter(|e| e.len() <= bound(a.max)).map(Value::Array)
    }

    /// None: no array is in a key [ISO/IEC 9075-2:2023 §9.12 SR3].
    fn encode_key(&self, v: &Value, t: &DataType, _: &mut Vec<u8>) -> Result<()> {
        Err(SqlError::new(Code::Api, format!("[ISO/IEC 9075-2:2023 §9.12 SR3] {v} of {t} is in no key, as no array is")))
    }

    fn decode_key(&self, _: &mut &[u8], _: &DataType) -> Result<Value> {
        Err(super::malformed())
    }

    /// Itself, of an array: no array is in a key to anchor among.
    fn anchor(&self, v: &Value, _: &DataType) -> Option<(Value, bool)> {
        elements(v).map(|_| (v.clone(), true))
    }
}

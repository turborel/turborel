//! \[EXTENSION\] VECTOR through the one type contract (RULES.md D53). A
//! value's bytes are its canonical image (`vector.rs`), so the contract's
//! comparison and buckets are of those bytes, and equal vectors are
//! equal bytes, the bit-identical equality of VECTOR_EQUALS (R3.12). The
//! binder, not this type, refuses the ordering comparisons the design
//! document forbids (R3.11).
//!
//! A character string is stored in a VECTOR by store assignment (R5.1),
//! read as its text form against the target's dimension count, format and
//! layout. A column fixing all three keeps a one-byte header in its
//! record, the version, and the elements (R4.4); the catalog has the rest.

use std::cmp::Ordering;
use std::sync::Arc;

use super::build::{Vector, canonical, dense_image};
use super::params::{self, OPEN, Shape, shape_of};
use super::{Element, Layout, NAME, VERSION, VectorRef, image, mismatch, text, value_of, vector};
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::value::types::{Descriptor, ExtensionType, ExtensionValue, Kind, TypeParams};
use crate::value::{Binary, Bucket, DataType, SqlType, Value};

/// The most octets of a character large object read as a vector's text.
const MOST_TEXT: u64 = 16_777_216;

/// The VECTOR type.
#[derive(Debug)]
pub struct VectorType;

/// The kind of every VECTOR value.
pub static VECTOR: VectorType = VectorType;
/// VECTOR as a kind.
pub static VECTOR_KIND: &dyn SqlType = &VECTOR;

/// VECTOR's kind, as the type registry holds kinds.
pub fn kind() -> Kind {
    &VECTOR_KIND
}

/// Plain VECTOR: any dimension count, format and layout.
pub fn data_type() -> DataType {
    DataType::Extension(ExtensionType { kind: &VECTOR_KIND, params: TypeParams::default() })
}

fn shape(t: &DataType) -> Shape {
    shape_of(t).unwrap_or(OPEN)
}

/// Whether `v` is already of shape `s`.
fn fits(v: &VectorRef<'_>, s: Shape) -> bool {
    s.dims.is_none_or(|d| d == v.dims) && s.element.is_none_or(|e| e == v.element) && s.layout.is_none_or(|l| l == v.layout())
}

/// The fixed dense shape of `t`, whose records are of fixed width.
fn fixed(t: &DataType) -> Option<(u32, Element)> {
    params::fixed_shape(t)
}

/// `v`, a VECTOR value, made one of `t`: its format and layout converted
/// where `t` fixes them (R5.3, R5.4), its dimension count never (R5.2).
///
/// # Errors
///
/// SQLCODE -301 for another dimension count, a conversion to or from
/// BINARY, or an element out of the new format's range.
pub(crate) fn conform(v: Value, t: &DataType) -> Result<Value> {
    let s = shape(t);
    let view = vector(&v)?;
    if fits(&view, s) {
        return Ok(v);
    }
    if let Some(dims) = s.dims
        && dims != view.dims
    {
        return Err(mismatch(&format!("dimension mismatch: expected {dims}, got {}", view.dims)));
    }
    let mut made = Vector::of(view).converted(s.element.unwrap_or(view.element))?;
    if let Some(layout) = s.layout {
        made = made.laid(layout)?;
    }
    made.value()
}

fn not_of(t: &DataType) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] the value is not one of {}", params::written_of(t)))
}

/// `v` checked to be of `t` exactly.
fn checked<'a>(v: &'a Value, t: &DataType) -> Result<VectorRef<'a>> {
    let view = vector(v)?;
    if fits(&view, shape(t)) { Ok(view) } else { Err(not_of(t)) }
}

/// A character string's text, of a CHARACTER or a CLOB value.
pub(crate) fn text_of(v: &Value) -> Result<Option<String>> {
    if let Value::Char(s) = v {
        return Ok(Some(s.clone()));
    }
    let Value::Lob(lob) = v else { return Ok(None) };
    if !matches!(lob.kind(), crate::value::lob::LobKind::Character) {
        return Ok(None);
    }
    let bytes = lob.bounded(MOST_TEXT)?.ok_or_else(|| super::out_of_range("the vector's text is too long"))?;
    String::from_utf8(bytes).map(Some).map_err(|_| super::bad_literal("not UTF-8"))
}

impl SqlType for VectorType {
    fn name(&self) -> &'static str {
        NAME
    }

    fn param_word(&self, at: usize, word: &str) -> Option<u32> {
        params::param_word(at, word)
    }

    fn pack_params(&self, written: &[u32]) -> Option<Vec<u32>> {
        params::pack_params(written)
    }

    fn written(&self, t: &DataType) -> String {
        params::written_of(t)
    }

    fn comparable(&self, other: &DataType) -> bool {
        shape_of(other).is_some()
    }

    fn compare(&self, a: &Value, b: &Value) -> Option<Ordering> {
        Some(image(a)?.cmp(image(b)?))
    }

    fn bucket(&self, v: &Value) -> Option<Bucket> {
        Some(Bucket::Extension(image(v)?.into()))
    }

    fn assign(&self, v: Value, t: &DataType, _: &str) -> Result<Value> {
        self.cast(v, t)
    }

    fn assigns_text(&self) -> bool {
        true
    }

    fn cast(&self, v: Value, t: &DataType) -> Result<Value> {
        if v.is_null() {
            return Ok(v);
        }
        if let Some(text) = text_of(&v)? {
            return text::vector(text.trim_end_matches(' '), shape(t))?.value();
        }
        if image(&v).is_none() {
            return Err(mismatch(&format!("the value cannot be cast to {}", params::written_of(t))));
        }
        conform(v, t)
    }

    fn literal(&self, v: &Value) -> String {
        let Some(view) = super::vector_of(v) else { return String::new() };
        let layout = match view.layout() {
            Layout::Dense => String::new(),
            Layout::Sparse => format!(", '{}'", Layout::Sparse.name()),
        };
        format!("TO_VECTOR('{}', {}, '{}'{layout})", text::write(view), view.dims, view.element.name())
    }

    fn text(&self, v: &Value) -> String {
        super::vector_of(v).map(text::write).unwrap_or_default()
    }

    fn position(&self, _: &Value) -> Option<f64> {
        None
    }

    fn descriptor(&self, _: &DataType) -> Descriptor {
        Descriptor::of(NAME)
    }

    fn put_value(&self, v: &Value, out: &mut Vec<u8>) {
        out.put_bytes(image(v).unwrap_or_default());
    }

    fn value(&self, r: &mut Reader<'_>) -> Result<Value> {
        let raw = r.bytes()?;
        canonical(raw).map_err(|_| r.corrupt("invalid VECTOR image"))?;
        Ok(value_of(raw.to_vec()))
    }

    fn check_params(&self, given: &[u32]) -> Result<()> {
        params::shape(given).map(|_| ())
    }

    fn put_params(&self, t: &DataType, out: &mut Vec<u8>) {
        params::put(t, out);
    }

    fn params(&self, r: &mut Reader<'_>) -> Result<DataType> {
        params::read(r)
    }

    fn record_width(&self, t: &DataType) -> Option<usize> {
        fixed(t).map(|(dims, element)| element.packed(dims).saturating_add(1))
    }

    fn encode_record(&self, v: &Value, t: &DataType) -> Result<Vec<u8>> {
        let view = checked(v, t)?;
        if fixed(t).is_none() {
            return Ok(image(v).unwrap_or_default().to_vec());
        }
        let mut out = Vec::with_capacity(view.bytes.len().saturating_add(1));
        out.push(VERSION);
        out.extend_from_slice(view.bytes);
        Ok(out)
    }

    fn decode_record(&self, field: &[u8], t: &DataType) -> Option<Value> {
        let Some((dims, element)) = fixed(t) else {
            let v = canonical(field).ok()?;
            return fits(&v, shape(t)).then(|| value_of(field.to_vec()));
        };
        let (version, bytes) = field.split_first()?;
        (*version == VERSION && bytes.len() == element.packed(dims)).then(|| value_of(dense_image(element, dims, bytes)))
    }

    fn encode_key(&self, v: &Value, t: &DataType, out: &mut Vec<u8>) -> Result<()> {
        checked(v, t)?;
        Binary::new(image(v).unwrap_or_default().to_vec()).put_key(out);
        Ok(())
    }

    fn decode_key(&self, rest: &mut &[u8], t: &DataType) -> Result<Value> {
        let b = Binary::take_key(rest).ok_or_else(|| SqlError::new(Code::Corrupt, "[DESIGN] invalid VECTOR index key"))?;
        let v = canonical(b.bytes())?;
        if !fits(&v, shape(t)) {
            return Err(SqlError::new(Code::Corrupt, "[DESIGN] a VECTOR index key not of its column's type"));
        }
        Ok(Value::Extension(ExtensionValue { kind: &VECTOR, bytes: Arc::from(b.bytes()) }))
    }

    fn anchor(&self, v: &Value, t: &DataType) -> Option<(Value, bool)> {
        checked(v, t).ok().map(|_| (v.clone(), true))
    }
}

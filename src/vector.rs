//! \[EXTENSION\] VECTOR, a type beyond the standard through the one type
//! contract (RULES.md D53): a list of numbers of one element format,
//! dense or sparse, its image the design document's dense (§4.1) and
//! sparse (§4.2) layouts. This module holds the element formats, the
//! image and a reader of it that copies nothing (`VectorRef`); the type
//! itself is `kind.rs`, its parameters `params.rs`, its text forms
//! `text.rs`, distances `kernels.rs`, `terms.rs` and `metric.rs`,
//! compression `quantizer.rs`, and its routines `functions.rs` and
//! `aggregate.rs`.
//!
//! The image is a 12-byte header, then the packed little-endian
//! elements: version 1; the format code; flags (bit 0 normalized, bit 1
//! sparse, bit 2 has-norm); a reserved zero; the dimension count, u32;
//! the L2 norm as an f32. A sparse image then has the count of stored
//! elements, u32, their dimensions, ascending u32 from 0, and their
//! values. The header is canonical. The norm is always computed and the
//! normalized flag follows from it, so equal vectors have equal bytes,
//! which comparison and hashing rely on.

mod aggregate;
mod bq;
mod build;
mod builtin;
pub mod chunk;
pub mod embed;
mod float;
pub mod functions;
pub(crate) mod kernels;
mod kind;
pub mod metric;
mod params;
mod pq;
pub mod quantizer;
mod random;
mod sq8;
mod terms;
#[cfg(test)]
mod tests;
mod text;

use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::types::ExtensionValue;
use crate::value::{SqlType, Value};

pub use aggregate::{Aggregate, aggregates};
pub use build::Vector;
pub(crate) use builtin::is_built_in;
pub use float::{half, half_bits, integral, single};
pub use functions::{functions, functions_with};
pub use kind::{VECTOR, VECTOR_KIND, VectorType, data_type, kind};
pub use params::{ANY, FORMAT_PARAM, LAYOUT_PARAM, LAYOUT_STEP, Shape, fixed_shape, pack_params, param_word, shape_of, typed};

/// The types of VECTOR, built in as full text's are: VECTOR alone.
pub fn types() -> [crate::value::types::Kind; 1] {
    [kind()]
}

/// The type's name, as a `<data type>` writes it.
pub const NAME: &str = "VECTOR";

// ---------------------------------------------------------------------------
// The image (design document §4.1, §4.2)
// ---------------------------------------------------------------------------

/// The image's version (format versions stay 1 in alpha).
pub const VERSION: u8 = 1;
/// The header's length in bytes.
pub const HEADER: usize = 12;
/// Flag bit 0: the vector's L2 norm is 1, to [`NORMALIZED_TOLERANCE`].
pub const FLAG_NORMALIZED: u8 = 1;
/// Flag bit 1: the sparse layout.
pub const FLAG_SPARSE: u8 = 2;
/// Flag bit 2: the header's norm is valid.
pub const FLAG_HAS_NORM: u8 = 4;
/// Every flag bit defined.
const FLAGS: u8 = FLAG_NORMALIZED | FLAG_SPARSE | FLAG_HAS_NORM;
/// How far from 1 a norm may be for the normalized flag: a few units in
/// the last place of an f32 near 1.
pub const NORMALIZED_TOLERANCE: f64 = 1e-6;
/// The bytes of a u32 in the image.
const WORD: usize = 4;
/// The header's offsets.
const AT_FORMAT: usize = 1;
const AT_FLAGS: usize = 2;
const AT_RESERVED: usize = 3;
const AT_DIMS: usize = 4;
const AT_NORM: usize = 8;

// ---------------------------------------------------------------------------
// Limits (design document §3.1 R3.4)
// ---------------------------------------------------------------------------

/// The most dimensions of a dense vector.
pub const MAX_DIMS: u32 = 65_535;
/// The most dimensions of a dense BINARY vector, a multiple of 8.
pub const MAX_BINARY_DIMS: u32 = 65_528;
/// The bits in a byte, the multiple a BINARY dimension count is.
pub const BITS_PER_BYTE: u32 = 8;
/// The most dimensions of a sparse vector, its vocabulary size.
pub const MAX_SPARSE_DIMS: u32 = 2_147_483_647;

/// An element format (design document §3.2): a closed set, as its codes
/// are part of the stored image and kernels are compiled for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Element {
    /// IEEE 754 binary32, code 1, the default.
    Float32,
    /// IEEE 754 binary16, code 2.
    Float16,
    /// IEEE 754 binary64, code 3.
    Float64,
    /// A signed byte, -128 to 127, code 4.
    Int8,
    /// One bit a dimension, eight to a byte, the first in the high bit,
    /// code 5.
    Binary,
}

/// The format names (design document §3.1 `format_spec`).
pub const FLOAT32: &str = "FLOAT32";
/// FLOAT16.
pub const FLOAT16: &str = "FLOAT16";
/// FLOAT64.
pub const FLOAT64: &str = "FLOAT64";
/// INT8.
pub const INT8: &str = "INT8";
/// BINARY.
pub const BINARY: &str = "BINARY";

impl Element {
    /// Every format, in code order.
    pub const ALL: [Element; 5] = [Element::Float32, Element::Float16, Element::Float64, Element::Int8, Element::Binary];

    /// Its code in the image.
    pub fn code(self) -> u8 {
        match self {
            Element::Float32 => 1,
            Element::Float16 => 2,
            Element::Float64 => 3,
            Element::Int8 => 4,
            Element::Binary => 5,
        }
    }

    /// The format of `code`, if one has it.
    pub fn of_code(code: u8) -> Option<Element> {
        Element::ALL.into_iter().find(|e| e.code() == code)
    }

    /// Its name, as written.
    pub fn name(self) -> &'static str {
        match self {
            Element::Float32 => FLOAT32,
            Element::Float16 => FLOAT16,
            Element::Float64 => FLOAT64,
            Element::Int8 => INT8,
            Element::Binary => BINARY,
        }
    }

    /// The format named `word`, in any case (R3.3).
    pub fn named(word: &str) -> Option<Element> {
        Element::ALL.into_iter().find(|e| e.name().eq_ignore_ascii_case(word.trim()))
    }

    /// Its bits per element.
    pub fn bits(self) -> u32 {
        match self {
            Element::Float32 => 32,
            Element::Float16 => 16,
            Element::Float64 => 64,
            Element::Int8 => 8,
            Element::Binary => 1,
        }
    }

    /// The bytes `count` elements of it pack into.
    pub fn packed(self, count: u32) -> usize {
        let bits = u64::from(count).saturating_mul(u64::from(self.bits()));
        usize::try_from(bits.div_ceil(u64::from(BITS_PER_BYTE))).unwrap_or(usize::MAX)
    }

    /// The wider of two formats (R6.9): FLOAT64 over FLOAT32 over
    /// FLOAT16; INT8 and BINARY promote to FLOAT32.
    #[must_use]
    pub fn wider(self, other: Element) -> Element {
        let rank = |e: Element| match e {
            Element::Float64 => 2_u8,
            Element::Float32 | Element::Int8 | Element::Binary => 1_u8,
            Element::Float16 => 0_u8,
        };
        let pick = if rank(other) > rank(self) { other } else { self };
        match pick {
            Element::Int8 | Element::Binary => Element::Float32,
            Element::Float32 | Element::Float16 | Element::Float64 => pick,
        }
    }
}

/// A layout (design document §3.1 `layout_spec`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layout {
    /// Every dimension stored.
    Dense,
    /// Only the dimensions not zero, with their indexes.
    Sparse,
}

/// DENSE.
pub const DENSE: &str = "DENSE";
/// SPARSE.
pub const SPARSE: &str = "SPARSE";

impl Layout {
    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Layout::Dense => DENSE,
            Layout::Sparse => SPARSE,
        }
    }

    /// The layout named `word`, in any case.
    pub fn named(word: &str) -> Option<Layout> {
        [Layout::Dense, Layout::Sparse].into_iter().find(|l| l.name().eq_ignore_ascii_case(word.trim()))
    }
}

/// A vector's image, read in place.
#[derive(Debug, Clone, Copy)]
pub struct VectorRef<'a> {
    /// Its element format.
    pub element: Element,
    /// Its dimension count.
    pub dims: u32,
    /// Its L2 norm, if the image holds it.
    pub norm: Option<f32>,
    /// Whether its norm is 1.
    pub normalized: bool,
    /// Its packed values: one for each dimension if dense, else one for
    /// each index in `indices`.
    pub bytes: &'a [u8],
    /// A sparse vector's dimensions, ascending little-endian u32 from 0.
    pub indices: Option<&'a [u8]>,
}

impl<'a> VectorRef<'a> {
    /// The image `image` read in place: its header and lengths checked,
    /// not its values.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the bytes are not an image.
    pub fn decode(image: &'a [u8]) -> Result<VectorRef<'a>> {
        let (head, body) = image.split_at_checked(HEADER).ok_or_else(|| corrupt("a short header"))?;
        let byte = |at: usize| head.get(at).copied().unwrap_or_default();
        let word = |at: usize| head.get(at..at.saturating_add(WORD)).and_then(|w| <[u8; WORD]>::try_from(w).ok()).unwrap_or_default();
        if byte(0) != VERSION || byte(AT_RESERVED) != 0 || byte(AT_FLAGS) & !FLAGS != 0 {
            return Err(corrupt("an unknown version or flag"));
        }
        let element = Element::of_code(byte(AT_FORMAT)).ok_or_else(|| corrupt("an unknown element format"))?;
        let flags = byte(AT_FLAGS);
        let dims = u32::from_le_bytes(word(AT_DIMS));
        let norm = (flags & FLAG_HAS_NORM != 0).then(|| f32::from_le_bytes(word(AT_NORM)));
        let normalized = flags & FLAG_NORMALIZED != 0;
        if flags & FLAG_SPARSE == 0 {
            if body.len() != element.packed(dims) {
                return Err(corrupt("a dense body of the wrong length"));
            }
            return Ok(VectorRef { element, dims, norm, normalized, bytes: body, indices: None });
        }
        let (count, rest) = body.split_at_checked(WORD).ok_or_else(|| corrupt("no stored count"))?;
        let count = u32::from_le_bytes(<[u8; WORD]>::try_from(count).map_err(|_| corrupt("no stored count"))?);
        let index_bytes = usize::try_from(count).unwrap_or(usize::MAX).saturating_mul(WORD);
        let (indices, bytes) = rest.split_at_checked(index_bytes).ok_or_else(|| corrupt("short indexes"))?;
        if bytes.len() != element.packed(count) || count > dims {
            return Err(corrupt("a sparse body of the wrong length"));
        }
        Ok(VectorRef { element, dims, norm, normalized, bytes, indices: Some(indices) })
    }

    /// Its layout.
    pub fn layout(&self) -> Layout {
        if self.indices.is_some() { Layout::Sparse } else { Layout::Dense }
    }

    /// The count of values it stores: its dimensions if dense.
    pub fn stored(&self) -> u32 {
        self.indices.map_or(self.dims, |i| u32::try_from(i.len().checked_div(WORD).unwrap_or_default()).unwrap_or(u32::MAX))
    }

    /// The `i`th stored value, from 0.
    pub fn value(&self, i: u32) -> Option<f64> {
        let at = usize::try_from(i).ok()?;
        element_at(self.element, self.bytes, at)
    }

    /// The dimension of the `i`th stored value, from 0.
    pub fn index(&self, i: u32) -> Option<u32> {
        let Some(indices) = self.indices else { return (i < self.dims).then_some(i) };
        let at = usize::try_from(i).ok()?.checked_mul(WORD)?;
        let word = indices.get(at..at.checked_add(WORD)?)?;
        Some(u32::from_le_bytes(<[u8; WORD]>::try_from(word).ok()?))
    }

    /// The value at dimension `dim`, from 0: zero where a sparse vector
    /// stores none; none past its dimensions.
    pub fn at(&self, dim: u32) -> Option<f64> {
        if dim >= self.dims {
            return None;
        }
        if self.indices.is_none() {
            return self.value(dim);
        }
        let (mut low, mut high) = (0_u32, self.stored());
        while low < high {
            let mid = low.saturating_add(high.saturating_sub(low) >> 1_u32);
            let at = self.index(mid)?;
            if at == dim {
                return self.value(mid);
            }
            if at < dim { low = mid.saturating_add(1) } else { high = mid }
        }
        Some(0.0)
    }

    /// The count of dimensions not zero (design document §6.4
    /// VECTOR_NNZ).
    pub fn nonzero(&self) -> u32 {
        let count = (0..self.stored()).filter(|&i| self.value(i).is_some_and(|v| v != 0.0_f64)).count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }
}

/// The value of the `at`th packed element of `bytes`.
pub(crate) fn element_at(element: Element, bytes: &[u8], at: usize) -> Option<f64> {
    let take = |n: usize| bytes.get(at.checked_mul(n)?..at.checked_mul(n)?.checked_add(n)?);
    match element {
        Element::Float32 => Some(f64::from(f32::from_le_bytes(take(WORD)?.try_into().ok()?))),
        Element::Float16 => Some(f64::from(half(u16::from_le_bytes(take(2)?.try_into().ok()?)))),
        Element::Float64 => Some(f64::from_le_bytes(take(8)?.try_into().ok()?)),
        Element::Int8 => Some(f64::from(i8::from_le_bytes(take(1)?.try_into().ok()?))),
        Element::Binary => {
            let byte = bytes.get(at >> 3_u32)?;
            let bit = u32::try_from(at & 7).ok()?;
            Some(f64::from((byte >> (7_u32.saturating_sub(bit))) & 1))
        }
    }
}

/// The image of `v`, if it is a VECTOR value.
pub fn image(v: &Value) -> Option<&[u8]> {
    let Value::Extension(e) = v else { return None };
    (e.kind.name() == NAME).then_some(e.bytes.as_ref())
}

/// `v` read in place, if it is a VECTOR value with a valid header.
pub fn vector_of(v: &Value) -> Option<VectorRef<'_>> {
    VectorRef::decode(image(v)?).ok()
}

/// `v`, a VECTOR value, read in place.
///
/// # Errors
///
/// SQLCODE -301 if `v` is not a VECTOR value; -916 if its image is bad.
pub fn vector(v: &Value) -> Result<VectorRef<'_>> {
    VectorRef::decode(image(v).ok_or_else(|| mismatch("expected a VECTOR value"))?)
}

/// A VECTOR value of `image`, which the caller has built.
pub fn value_of(image: Vec<u8>) -> Value {
    let kind: &'static dyn SqlType = &VECTOR;
    Value::Extension(ExtensionValue { kind, bytes: image.into() })
}

/// Its values as `f32`s, each dimension's, into `out` (cleared first).
pub fn to_f32(v: VectorRef<'_>, out: &mut Vec<f32>) {
    out.clear();
    out.extend((0..v.dims).map(|d| single(v.at(d).unwrap_or_default())));
}

/// Its values as `f64`s, each dimension's, into `out` (cleared first).
pub fn to_f64(v: VectorRef<'_>, out: &mut Vec<f64>) {
    out.clear();
    out.extend((0..v.dims).map(|d| v.at(d).unwrap_or_default()));
}

// ---------------------------------------------------------------------------
// Errors (design document §18's conditions, as SQLCODE and SQLSTATE)
// ---------------------------------------------------------------------------

/// VEC-0003, VEC-0004, VEC-0008, VEC-0013: operands or arguments that do
/// not fit, SQLCODE -301, SQLSTATE 22000.
pub(crate) fn mismatch(detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] {detail}"))
}

/// VEC-0005, VEC-0016: a vector literal that is not one, SQLCODE -301,
/// SQLSTATE 22018.
pub(crate) fn bad_literal(detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] invalid vector literal: {detail}"))
        .with_state(SqlState::INVALID_CHARACTER_VALUE_FOR_CAST)
}

/// VEC-0006, VEC-0007, VEC-0012: an element out of its format's range,
/// NaN or infinite, or an overflow, SQLCODE -301, SQLSTATE 22003.
pub(crate) fn out_of_range(detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] {detail}")).with_state(SqlState::NUMERIC_VALUE_OUT_OF_RANGE)
}

/// VEC-0001, VEC-0002: a VECTOR type written wrong, SQLCODE -103.
pub(crate) fn bad_type(detail: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] invalid VECTOR type: {detail}"))
}

/// A stored image that is not one, SQLCODE -916.
fn corrupt(detail: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] invalid VECTOR image: {detail}"))
}

//! \[EXTENSION\] A vector held as `f64`s while it is made or converted,
//! and its encoding into the canonical image (design document §4.1,
//! §4.2): each element checked against its format (R3.9, §3.2), sparse
//! dimensions ascending with zeros dropped (R4.1), the norm computed.

use super::{
    BITS_PER_BYTE, Element, FLAG_HAS_NORM, FLAG_NORMALIZED, FLAG_SPARSE, HEADER, Layout, MAX_BINARY_DIMS, MAX_DIMS, MAX_SPARSE_DIMS,
    NORMALIZED_TOLERANCE, VERSION, VectorRef, bad_literal, float, half, half_bits, mismatch, out_of_range, single, value_of,
};
use crate::error::Result;
use crate::value::Value;

/// A vector's values, as `f64`s.
#[derive(Debug, Clone, PartialEq)]
pub enum Entries {
    /// One for each dimension.
    Dense(Vec<f64>),
    /// Dimensions from 0, ascending, each with its value, not zero.
    Sparse(Vec<(u32, f64)>),
}

/// A vector being made: its format, dimension count and values.
#[derive(Debug, Clone, PartialEq)]
pub struct Vector {
    /// Its element format.
    pub element: Element,
    /// Its dimension count.
    pub dims: u32,
    /// Its values.
    pub entries: Entries,
}

/// Why `dims` dimensions do not fit `element` and `layout` (R3.4, R3.8).
pub(crate) fn dims_fault(dims: u32, element: Element, layout: Layout) -> Option<String> {
    let binary = matches!(element, Element::Binary);
    match layout {
        Layout::Sparse if binary => Some("BINARY vectors are dense only".to_owned()),
        Layout::Sparse => {
            (dims == 0 || dims > MAX_SPARSE_DIMS).then(|| format!("a sparse vector has 1 to {MAX_SPARSE_DIMS} dimensions, not {dims}"))
        }
        Layout::Dense if binary => (dims == 0 || dims > MAX_BINARY_DIMS || !dims.is_multiple_of(BITS_PER_BYTE))
            .then(|| format!("a BINARY vector has 8 to {MAX_BINARY_DIMS} dimensions, a multiple of 8, not {dims}")),
        Layout::Dense => (dims == 0 || dims > MAX_DIMS).then(|| format!("a vector has 1 to {MAX_DIMS} dimensions, not {dims}")),
    }
}

impl Vector {
    /// A dense vector of `values`.
    ///
    /// # Errors
    ///
    /// SQLCODE -301, SQLSTATE 22003, if there are too many or too few.
    pub fn dense(element: Element, values: Vec<f64>) -> Result<Vector> {
        let dims = u32::try_from(values.len()).unwrap_or(u32::MAX);
        if let Some(fault) = dims_fault(dims, element, Layout::Dense) {
            return Err(out_of_range(&fault));
        }
        Ok(Vector { element, dims, entries: Entries::Dense(values) })
    }

    /// A sparse vector of `dims` dimensions and `pairs`, dimensions from 0
    /// strictly ascending; zeros are dropped (R4.1).
    ///
    /// # Errors
    ///
    /// SQLCODE -301: SQLSTATE 22003 if `dims` is out of range; 22018 if a
    /// dimension repeats, descends or is past `dims` (VEC-0016).
    pub fn sparse(element: Element, dims: u32, pairs: Vec<(u32, f64)>) -> Result<Vector> {
        if let Some(fault) = dims_fault(dims, element, Layout::Sparse) {
            return Err(out_of_range(&fault));
        }
        let ascending = pairs.windows(2).all(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| a.0 < b.0));
        if !ascending || pairs.last().is_some_and(|(at, _)| *at >= dims) {
            return Err(bad_literal("sparse indexes must ascend, without repeats, below the dimension count"));
        }
        let kept = pairs.into_iter().filter(|(_, v)| *v != 0.0_f64).collect();
        Ok(Vector { element, dims, entries: Entries::Sparse(kept) })
    }

    /// The vector `v` holds.
    pub fn of(v: VectorRef<'_>) -> Vector {
        let entries = match v.layout() {
            Layout::Dense => Entries::Dense((0..v.dims).map(|d| v.value(d).unwrap_or_default()).collect()),
            Layout::Sparse => {
                Entries::Sparse((0..v.stored()).filter_map(|i| Some((v.index(i)?, v.value(i).unwrap_or_default()))).collect())
            }
        };
        Vector { element: v.element, dims: v.dims, entries }
    }

    /// Its layout.
    pub fn layout(&self) -> Layout {
        match self.entries {
            Entries::Dense(_) => Layout::Dense,
            Entries::Sparse(_) => Layout::Sparse,
        }
    }

    /// Its value at each dimension.
    pub fn values(&self) -> Vec<f64> {
        match &self.entries {
            Entries::Dense(values) => values.clone(),
            Entries::Sparse(pairs) => {
                let mut out = vec![0.0; usize::try_from(self.dims).unwrap_or_default()];
                for (at, v) in pairs {
                    if let Some(slot) = usize::try_from(*at).ok().and_then(|at| out.get_mut(at)) {
                        *slot = *v;
                    }
                }
                out
            }
        }
    }

    /// It in `layout` (R5.4).
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if its dimensions do not fit the layout.
    pub fn laid(self, layout: Layout) -> Result<Vector> {
        match (self.layout(), layout) {
            (Layout::Dense, Layout::Dense) | (Layout::Sparse, Layout::Sparse) => Ok(self),
            (Layout::Sparse, Layout::Dense) => Vector::dense(self.element, self.values()),
            (Layout::Dense, Layout::Sparse) => {
                let pairs = (0..self.dims).zip(self.values()).filter(|(_, v)| *v != 0.0_f64).collect();
                Vector::sparse(self.element, self.dims, pairs)
            }
        }
    }

    /// It in `element` (R5.3): among the float formats and INT8, its
    /// values checked as it is encoded; to or from BINARY only through
    /// VECTOR_QUANTIZE (R5.5).
    ///
    /// # Errors
    ///
    /// SQLCODE -301 for a conversion to or from BINARY.
    pub fn converted(self, element: Element) -> Result<Vector> {
        let binary = (matches!(self.element, Element::Binary), matches!(element, Element::Binary));
        if binary.0 != binary.1 {
            return Err(mismatch("a vector converts to or from BINARY only by VECTOR_QUANTIZE"));
        }
        Ok(Vector { element, ..self })
    }

    /// It with `f` applied to each stored value. Only a sparse vector's
    /// stored ones are given to `f`, so `f(0)` must be 0 for it.
    #[must_use]
    pub fn mapped(self, f: impl Fn(f64) -> f64) -> Vector {
        let entries = match self.entries {
            Entries::Dense(values) => Entries::Dense(values.into_iter().map(&f).collect()),
            Entries::Sparse(pairs) => Entries::Sparse(pairs.into_iter().map(|(at, v)| (at, f(v))).collect()),
        };
        Vector { entries, ..self }
    }

    /// Its canonical image.
    ///
    /// # Errors
    ///
    /// SQLCODE -301, SQLSTATE 22003, if an element is NaN, infinite or out
    /// of its format's range (R3.9, §3.2).
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![0_u8; HEADER];
        let mut squares = 0.0_f64;
        let mut flags = 0_u8;
        match &self.entries {
            Entries::Dense(values) => pack(self.element, values.iter().copied(), &mut out, &mut squares)?,
            Entries::Sparse(pairs) => {
                flags |= FLAG_SPARSE;
                out.extend_from_slice(&u32::try_from(pairs.len()).unwrap_or(u32::MAX).to_le_bytes());
                for (at, _) in pairs {
                    out.extend_from_slice(&at.to_le_bytes());
                }
                pack(self.element, pairs.iter().map(|(_, v)| *v), &mut out, &mut squares)?;
            }
        }
        let norm = single(squares.sqrt());
        if norm.is_finite() {
            flags |= FLAG_HAS_NORM;
        }
        if (f64::from(norm) - 1.0).abs() <= NORMALIZED_TOLERANCE {
            flags |= FLAG_NORMALIZED;
        }
        let header = [VERSION, self.element.code(), flags, 0];
        out.splice(0..HEADER, header.into_iter().chain(self.dims.to_le_bytes()).chain(norm.to_le_bytes()));
        Ok(out)
    }

    /// It as a VECTOR value.
    ///
    /// # Errors
    ///
    /// As `encode`.
    pub fn value(&self) -> Result<Value> {
        self.encode().map(value_of)
    }
}

/// Appends `values` packed in `element` to `out`, adding the squares of
/// the stored values to `squares`.
fn pack(element: Element, values: impl Iterator<Item = f64>, out: &mut Vec<u8>, squares: &mut f64) -> Result<()> {
    let mut bits = 0_u8;
    let mut filled = 0_u32;
    for x in values {
        if !x.is_finite() {
            return Err(out_of_range("NaN and infinity are not vector elements"));
        }
        let stored = match element {
            Element::Float32 => {
                let s = single(x);
                out.extend_from_slice(&s.to_le_bytes());
                f64::from(s)
            }
            Element::Float16 => {
                let h = half_bits(x);
                out.extend_from_slice(&h.to_le_bytes());
                f64::from(half(h))
            }
            Element::Float64 => {
                out.extend_from_slice(&x.to_le_bytes());
                x
            }
            Element::Int8 => {
                let i = float::integral(x).and_then(|i| i8::try_from(i).ok()).ok_or_else(|| int8_range(x))?;
                out.extend_from_slice(&i.to_le_bytes());
                f64::from(i)
            }
            Element::Binary => {
                let bit = float::integral(x).and_then(|i| u8::try_from(i).ok()).filter(|b| *b <= 1).ok_or_else(|| bit_range(x))?;
                bits = (bits << 1_u32) | bit;
                filled = filled.saturating_add(1);
                if filled == BITS_PER_BYTE {
                    out.push(bits);
                    (bits, filled) = (0, 0);
                }
                f64::from(bit)
            }
        };
        if !stored.is_finite() {
            return Err(out_of_range(&format!("{x} is beyond the range of {}", element.name())));
        }
        *squares += stored * stored;
    }
    if filled != 0 {
        out.push(bits << (BITS_PER_BYTE.saturating_sub(filled)));
    }
    Ok(())
}

fn int8_range(x: f64) -> crate::error::SqlError {
    out_of_range(&format!("{x} is not an INT8 element, an integer from -128 to 127"))
}

fn bit_range(x: f64) -> crate::error::SqlError {
    out_of_range(&format!("{x} is not a BINARY element, 0 or 1"))
}

/// The image of a dense vector of `dims` elements of `element` packed in
/// `bytes`, as `encode` makes it: its norm summed in the same order.
pub(crate) fn dense_image(element: Element, dims: u32, bytes: &[u8]) -> Vec<u8> {
    let view = VectorRef { element, dims, norm: None, normalized: false, bytes, indices: None };
    let squares = (0..dims).map(|d| view.value(d).unwrap_or_default()).fold(0.0_f64, |sum, x| sum + x * x);
    let norm = single(squares.sqrt());
    let mut flags = if norm.is_finite() { FLAG_HAS_NORM } else { 0 };
    if (f64::from(norm) - 1.0).abs() <= NORMALIZED_TOLERANCE {
        flags |= FLAG_NORMALIZED;
    }
    let mut out = Vec::with_capacity(HEADER.saturating_add(bytes.len()));
    out.extend_from_slice(&[VERSION, element.code(), flags, 0]);
    out.extend_from_slice(&dims.to_le_bytes());
    out.extend_from_slice(&norm.to_le_bytes());
    out.extend_from_slice(bytes);
    out
}

/// `image` read, if it is a canonical image: as `encode` would make it.
///
/// # Errors
///
/// SQLCODE -916 if it is not.
pub(crate) fn canonical(image: &[u8]) -> Result<VectorRef<'_>> {
    let v = VectorRef::decode(image)?;
    let again = Vector::of(v);
    let again = match again.entries {
        Entries::Sparse(pairs) => Vector::sparse(again.element, again.dims, pairs),
        Entries::Dense(_) => Ok(again),
    };
    if again.and_then(|a| a.encode()).is_ok_and(|bytes| bytes == image) {
        return Ok(v);
    }
    Err(crate::error::SqlError::new(crate::error::Code::Corrupt, "[DESIGN] a VECTOR image that is not canonical"))
}

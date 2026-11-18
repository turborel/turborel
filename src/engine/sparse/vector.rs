//! \[EXTENSION\] The sparse vector the inverted index reads: a dimension
//! count, strictly ascending positions below it, and a non-zero finite
//! value at each. It reads and writes the sparse image of the vector
//! design's §4.2: the 12-byte header of §4.1 with its sparse flag set,
//! then the non-zero count, the positions as u32 and the values in the
//! element format, all little-endian. Positions count from 0.
//!
//! The vector type proper is the VECTOR data type's, whose reader of the
//! image (`VectorRef`) this reads it with; this is the view the index
//! needs, its values single precision as its postings' are.

use crate::error::{Code, Result, SqlError};
use crate::hash::murmur::murmur3_x64_128;
use crate::vector::{Element, Layout, VectorRef};

/// The image's format version (§4.1 offset 0).
pub const IMAGE_VERSION: u8 = 1;
/// The bytes of the header of §4.1, before the non-zero count.
pub const IMAGE_HEADER: usize = 12;
/// §4.1's element format codes.
pub const FORMAT_FLOAT32: u8 = 1;
/// IEEE 754 half precision.
pub const FORMAT_FLOAT16: u8 = 2;
/// IEEE 754 double precision.
pub const FORMAT_FLOAT64: u8 = 3;
/// A signed byte, -128 to 127.
pub const FORMAT_INT8: u8 = 4;
/// One bit a dimension, which a sparse vector never has (R3.8).
pub const FORMAT_BINARY: u8 = 5;
/// §4.1's flag of a sparse image, bit 1.
pub const SPARSE_FLAG: u8 = 0b10;
/// §4.1's flag of a cached L2 norm, bit 2.
pub const NORM_FLAG: u8 = 0b100;
/// The flag of a vector whose norm is 1, to `NORMALIZED_TOLERANCE`; set
/// by the norm alone, so that equal vectors have equal images.
pub const NORMALIZED_FLAG: u8 = 0b1;
/// How far from 1 a norm may be for `NORMALIZED_FLAG`.
pub const NORMALIZED_TOLERANCE: f64 = 1e-6;
/// The most dimensions a sparse vector has (R3.4): 2^31 - 1.
pub const MOST_DIMENSIONS: u32 = 0x7fff_ffff;
/// The bytes of a vector's hash, by which the index counts it.
pub const HASH_BYTES: usize = 16;
/// \[DESIGN\] The seed of a vector's hash, fixed for the index's life.
const HASH_SEED: u32 = 0x5350_5653;
/// The bytes of a position.
const POSITION_BYTES: usize = 4;
/// The bytes of a position and a FLOAT32 value.
const ENTRY_BYTES: usize = 8;

/// A sparse vector: `dims` dimensions, the non-zero ones at `indices`,
/// strictly ascending, with their `values`.
#[derive(Debug, Clone, PartialEq)]
pub struct SparseVector {
    dims: u32,
    indices: Vec<u32>,
    values: Vec<f32>,
}

fn invalid(why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] a sparse vector {why}"))
}

impl SparseVector {
    /// The vector of `dims` dimensions with the values of `entries` at
    /// their positions; an explicit zero is dropped (R4.1).
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if `dims` is not from 1 to 2^31 - 1 (R3.4), if a
    /// position is not below it or not above the one before (VEC-0016),
    /// or if a value is NaN or infinite (R3.9).
    pub fn new(dims: u32, entries: impl IntoIterator<Item = (u32, f32)>) -> Result<SparseVector> {
        if !(1..=MOST_DIMENSIONS).contains(&dims) {
            return Err(invalid("has from 1 to 2147483647 dimensions"));
        }
        let (mut indices, mut values) = (Vec::new(), Vec::new());
        for (index, value) in entries {
            if index >= dims || indices.last().is_some_and(|&last| index <= last) {
                return Err(invalid("has positions strictly ascending below its dimensions"));
            }
            if !value.is_finite() {
                return Err(invalid("has no NaN or infinite value"));
            }
            if value != 0.0_f32 {
                indices.push(index);
                values.push(value);
            }
        }
        Ok(SparseVector { dims, indices, values })
    }

    /// The vector a §4.2 image holds.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if the image is not a sparse one of a known format,
    /// its length is not its count's, or its contents break `new`'s rules.
    pub fn from_image(bytes: &[u8]) -> Result<SparseVector> {
        let image = VectorRef::decode(bytes).map_err(|_| invalid("image is not one of version 1 of its length"))?;
        if image.layout() != Layout::Sparse || image.element == Element::Binary {
            return Err(invalid("image is not a sparse one of FLOAT32, FLOAT16, FLOAT64 or INT8 (R3.8)"));
        }
        let mut entries = Vec::with_capacity(usize::try_from(image.stored()).unwrap_or(0));
        for i in 0..image.stored() {
            let (Some(index), Some(wide)) = (image.index(i), image.value(i)) else { return Err(invalid("image is cut short")) };
            if !wide.is_finite() {
                return Err(invalid("has no NaN or infinite value"));
            }
            entries.push((index, narrow(wide)));
        }
        SparseVector::new(image.dims, entries)
    }

    /// Its §4.2 image, of FLOAT32 elements, with its L2 norm cached.
    pub fn image(&self) -> Vec<u8> {
        let count = self.indices.len();
        let mut out = Vec::with_capacity(IMAGE_HEADER.saturating_add(POSITION_BYTES).saturating_add(count.saturating_mul(ENTRY_BYTES)));
        let norm = self.norm();
        let mut flags = SPARSE_FLAG;
        if norm.is_finite() {
            flags |= NORM_FLAG;
        }
        if (f64::from(norm) - 1.0_f64).abs() <= NORMALIZED_TOLERANCE {
            flags |= NORMALIZED_FLAG;
        }
        out.extend_from_slice(&[IMAGE_VERSION, FORMAT_FLOAT32, flags, 0]);
        out.extend_from_slice(&self.dims.to_le_bytes());
        out.extend_from_slice(&norm.to_le_bytes());
        out.extend_from_slice(&u32::try_from(count).unwrap_or(u32::MAX).to_le_bytes());
        for index in &self.indices {
            out.extend_from_slice(&index.to_le_bytes());
        }
        for value in &self.values {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out
    }

    /// Its dimensions.
    pub fn dims(&self) -> u32 {
        self.dims
    }

    /// The positions of its non-zero values, ascending.
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    /// Its non-zero values, at `indices`.
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// How many values are not zero.
    pub fn nnz(&self) -> usize {
        self.indices.len()
    }

    /// Its positions and values, ascending.
    pub fn entries(&self) -> impl Iterator<Item = (u32, f32)> + '_ {
        self.indices.iter().copied().zip(self.values.iter().copied())
    }

    /// Its dot product with `other`, of the positions both hold, summed in
    /// double precision in ascending position order (R3.10, R4.2).
    pub fn dot(&self, other: &SparseVector) -> f64 {
        let (mut a, mut b) = (self.entries().peekable(), other.entries().peekable());
        let mut sum = 0.0_f64;
        while let (Some(&(i, x)), Some(&(j, y))) = (a.peek(), b.peek()) {
            if i == j {
                sum += f64::from(x) * f64::from(y);
            }
            if i <= j {
                a.next();
            }
            if j <= i {
                b.next();
            }
        }
        sum
    }

    /// Its L2 norm, in single precision.
    pub fn norm(&self) -> f32 {
        let squares: f64 = self.values.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
        narrow(squares.sqrt())
    }

    /// \[DESIGN\] The hash the index counts it by: of its dimensions,
    /// positions and values' bits, so equal vectors hash alike.
    pub fn hash(&self) -> [u8; HASH_BYTES] {
        let mut bytes = Vec::with_capacity(self.indices.len().saturating_mul(ENTRY_BYTES).saturating_add(POSITION_BYTES));
        bytes.extend_from_slice(&self.dims.to_le_bytes());
        for (index, value) in self.entries() {
            bytes.extend_from_slice(&index.to_le_bytes());
            bytes.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        let (high, low) = murmur3_x64_128(&bytes, HASH_SEED);
        let mut out = [0_u8; HASH_BYTES];
        for (slot, byte) in out.iter_mut().zip(high.to_be_bytes().into_iter().chain(low.to_be_bytes())) {
            *slot = byte;
        }
        out
    }
}

/// The single nearest `wide`, ties to even, by the bits: a double's
/// mantissa rounded to a single's, its exponent rebased; past the singles'
/// range, the greatest finite single of its sign, as an index weight must
/// stay finite.
pub fn narrow(wide: f64) -> f32 {
    const DOUBLE_MANTISSA_BITS: u32 = 52;
    const DROPPED: u32 = 29;
    const DOUBLE_EXPONENT: u64 = 0x7ff;
    const REBIAS: u64 = 896;
    const LEAST_NORMAL: u64 = 897;
    const MOST_FINITE: u64 = 1150;
    let bits = wide.to_bits();
    let negative = bits >> 63_u32 == 1;
    let exponent = (bits >> DOUBLE_MANTISSA_BITS) & DOUBLE_EXPONENT;
    let mantissa = bits & ((1_u64 << DOUBLE_MANTISSA_BITS).wrapping_sub(1));
    let magnitude = if exponent > MOST_FINITE {
        f32::MAX
    } else if exponent >= LEAST_NORMAL {
        let kept = ((exponent.wrapping_sub(REBIAS)) << 23_u32) | (mantissa >> DROPPED);
        let rest = mantissa & ((1_u64 << DROPPED).wrapping_sub(1));
        let half_way = 1_u64 << DROPPED.wrapping_sub(1);
        let rounded = if rest > half_way || rest == half_way && kept & 1 == 1 { kept.wrapping_add(1) } else { kept };
        u32::try_from(rounded).map_or(f32::MAX, |b| f32::from_bits(b).min(f32::MAX))
    } else {
        // Below the least normal single: its multiple of the least
        // subnormal, 2^-149, which a double holds exactly.
        subnormal(wide.abs())
    };
    if negative { -magnitude } else { magnitude }
}

/// `x`, below 2^-126, as the nearest single, a multiple of 2^-149.
fn subnormal(x: f64) -> f32 {
    const LEAST: f64 = 1.401_298_464_324_817e-45_f64;
    let steps = (x / LEAST).round();
    // Fewer than 2^23 steps: the low bits of a double offset by 2^52 are
    // the count exactly.
    let offset = (steps + 4_503_599_627_370_496.0_f64).to_bits() & 0x000f_ffff_ffff_ffff;
    u32::try_from(offset).map_or(0.0_f32, f32::from_bits)
}

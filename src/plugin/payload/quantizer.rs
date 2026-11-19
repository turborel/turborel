//! \[DESIGN\] The payload of a quantizer's one call, TRAIN (RULES.md
//! D221): a sample in, a product codebook out, which the database encodes,
//! decodes and measures by itself, so a vector index's search makes no
//! call.
//!
//! ```text
//! TRAIN  dims u32 | code_bytes u32 | seed u64 | count u32 | (f32s)...
//!        -> k u32 | count u32 | width u32... | (f32s)... one a subspace, k centroids of its width
//! ```
//!
//! A single float crosses as its bits, a u32, `f32s` counted.

use crate::bytes::{Put, Reader};
use crate::error::Result;

use super::{count, f32s, put_f32s};

/// What TRAIN asks: the dimensions, the code's bytes (0 for the
/// program's default), the seed of its pseudo-random sequence, and the
/// sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Training {
    /// The dimensions of each vector.
    pub dims: u32,
    /// The bytes of a code: its subspaces; 0 for the program's choice.
    pub code_bytes: u32,
    /// The seed training is deterministic by.
    pub seed: u64,
    /// The sample.
    pub sample: Vec<Vec<f32>>,
}

/// What TRAIN answers: `k` centroids in each subspace of `widths`.
#[derive(Debug, Clone, PartialEq)]
pub struct Trained {
    /// The centroids of each subspace, at most 256.
    pub k: u32,
    /// The width of each subspace, in order, adding to the dimensions.
    pub widths: Vec<u32>,
    /// Each subspace's `k` centroids of its width, one after another.
    pub centroids: Vec<Vec<f32>>,
}

/// `t`, written.
pub fn put_training(out: &mut Vec<u8>, t: &Training) {
    out.put_u32(t.dims);
    out.put_u32(t.code_bytes);
    out.put_u64(t.seed);
    out.put_u32(count(t.sample.len()));
    t.sample.iter().for_each(|v| put_f32s(out, v));
}

/// What `put_training` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn training(r: &mut Reader<'_>) -> Result<Training> {
    let (dims, code_bytes, seed) = (r.u32()?, r.u32()?, r.u64()?);
    let sample = (0..r.u32()?).map(|_| f32s(r)).collect::<Result<_>>()?;
    Ok(Training { dims, code_bytes, seed, sample })
}

/// `t`, written.
pub fn put_trained(out: &mut Vec<u8>, t: &Trained) {
    out.put_u32(t.k);
    out.put_u32(count(t.widths.len()));
    t.widths.iter().for_each(|w| out.put_u32(*w));
    t.centroids.iter().for_each(|c| put_f32s(out, c));
}

/// What `put_trained` wrote.
///
/// # Errors
///
/// `Reader`'s.
pub fn trained(r: &mut Reader<'_>) -> Result<Trained> {
    let k = r.u32()?;
    let widths: Vec<u32> = (0..r.u32()?).map(|_| r.u32()).collect::<Result<_>>()?;
    let centroids = widths.iter().map(|_| f32s(r)).collect::<Result<_>>()?;
    Ok(Trained { k, widths, centroids })
}

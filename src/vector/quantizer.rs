//! \[EXTENSION\] Vector compression for indexes (design document §7.2):
//! the `Quantizer` extension point, which trains a `Codebook` on a sample
//! and reads one back; a codebook encodes vectors as short codes and
//! gives, for one query, a `CodeDistance` that measures codes without
//! decoding them. Built in: PQ (`pq.rs`), SQ8 (`sq8.rs`) and BQ
//! (`bq.rs`).
//!
//! Contract (RULES.md R-SAFE-3), for every implementation:
//! * Training is deterministic given the sample's order and the seed: no
//!   clock, no operating-system randomness.
//! * `encode` writes exactly `code_bytes` bytes; a code distance is
//!   finite; nothing panics.
//! * `write` gives bytes `read` turns back into an equal codebook.
//! * No method calls back into a `Database` or holds a lock across a
//!   call (RULES.md R-ERR-4).

use std::sync::Arc;

use super::metric::{COSINE, DOT, EUCLIDEAN, EUCLIDEAN_SQUARED, MANHATTAN, VectorMetric};
use super::{Element, VectorRef, single};
use crate::error::{Code, Result, SqlError};

/// PQ, product quantization.
pub const PQ: &str = "PQ";
/// SQ8, scalar quantization to a byte a dimension.
pub const SQ8: &str = "SQ8";
/// BQ, binary quantization to a bit a dimension.
pub const BQ: &str = "BQ";
/// The version of every built-in codebook's bytes.
pub(crate) const CODEBOOK_VERSION: u8 = 1;
/// The distance of a code a metric could not measure, so it ranks last.
pub(crate) const UNMEASURED: f32 = f32::MAX;

/// A vector compressor, an extension point: see the module's contract.
pub trait Quantizer: Send + Sync {
    /// Its name: fixed, upper case, unique.
    fn name(&self) -> &str;
    /// A codebook for vectors of `dims` dimensions with codes of
    /// `code_bytes` bytes (0 for its default), trained on `sample` with
    /// the pseudo-random sequence of `seed`.
    ///
    /// # Errors
    ///
    /// A sample's error; SQLCODE -301 for an empty sample, a vector not
    /// of `dims` dimensions or a code length it cannot make.
    fn train(
        &self,
        sample: &mut dyn Iterator<Item = Result<Vec<f32>>>,
        dims: u32,
        code_bytes: usize,
        seed: u64,
    ) -> Result<Box<dyn Codebook>>;
    /// The codebook `Codebook::write` wrote.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the bytes are not one.
    fn read(&self, bytes: &[u8]) -> Result<Box<dyn Codebook>>;
}

/// A trained codebook.
pub trait Codebook: Send + Sync {
    /// The bytes of a code.
    fn code_bytes(&self) -> usize;
    /// The dimensions of the vectors it encodes.
    fn dims(&self) -> u32;
    /// Writes the code of `v`, `dims` long, into `out`, `code_bytes` long.
    fn encode(&self, v: &[f32], out: &mut [u8]);
    /// The vector `code` stands for, into `out` (cleared first).
    fn decode(&self, code: &[u8], out: &mut Vec<f32>);
    /// A measure of codes from `query` by `metric`.
    fn table<'a>(&'a self, query: &[f32], metric: &'a dyn VectorMetric) -> Box<dyn CodeDistance + 'a>;
    /// Appends its bytes to `out`, for `Quantizer::read`.
    fn write(&self, out: &mut Vec<u8>);
}

/// The approximate distance from one query to codes.
pub trait CodeDistance {
    /// The distance to the vector `code` stands for.
    fn distance(&self, code: &[u8]) -> f32;
}

/// The quantizers a database knows: the built-in ones, then those
/// registered.
pub type Quantizers = Arc<dyn Fn() -> Vec<Arc<dyn Quantizer>> + Send + Sync>;

fn built_ins() -> Vec<Arc<dyn Quantizer>> {
    vec![Arc::new(super::pq::ProductQuantizer), Arc::new(super::sq8::ScalarQuantizer), Arc::new(super::bq::BinaryQuantizer)]
}

/// The built-in quantizers alone.
pub fn built_in_quantizers() -> Quantizers {
    Arc::new(built_ins)
}

/// The built-in quantizers, then `extra`.
pub fn quantizers_with(extra: Vec<Arc<dyn Quantizer>>) -> Quantizers {
    Arc::new(move || {
        let mut all = built_ins();
        all.extend(extra.iter().cloned());
        all
    })
}

/// The quantizer named `name`, in any case.
pub fn quantizer_named(quantizers: &Quantizers, name: &str) -> Option<Arc<dyn Quantizer>> {
    quantizers().into_iter().find(|q| q.name().eq_ignore_ascii_case(name.trim()))
}

/// A sample, each vector checked to have `dims` dimensions.
pub(crate) fn collect(sample: &mut dyn Iterator<Item = Result<Vec<f32>>>, dims: u32) -> Result<Vec<Vec<f32>>> {
    let width = usize::try_from(dims).unwrap_or(usize::MAX);
    let mut out = Vec::new();
    for v in sample {
        let v = v?;
        if v.len() != width {
            return Err(training(&format!("a sample vector has {} dimensions, not {dims}", v.len())));
        }
        out.push(v);
    }
    if out.is_empty() {
        return Err(training("the sample is empty"));
    }
    Ok(out)
}

/// A training failure, SQLCODE -301.
pub(crate) fn training(detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] cannot train the codebook: {detail}"))
}

/// Codebook bytes that are not one, SQLCODE -916.
pub(crate) fn bad_codebook(name: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] invalid {name} codebook"))
}

/// The metrics a codebook measures from tables of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Measure {
    /// EUCLIDEAN_SQUARED.
    Square,
    /// EUCLIDEAN.
    Euclidean,
    /// DOT.
    Dot,
    /// MANHATTAN.
    Gap,
    /// COSINE.
    Cosine,
}

/// The measure of `metric`, if it is one a codebook tabulates.
pub(crate) fn measure(metric: &dyn VectorMetric) -> Option<Measure> {
    let name = metric.name();
    [
        (EUCLIDEAN_SQUARED, Measure::Square),
        (EUCLIDEAN, Measure::Euclidean),
        (DOT, Measure::Dot),
        (MANHATTAN, Measure::Gap),
        (COSINE, Measure::Cosine),
    ]
    .into_iter()
    .find_map(|(n, m)| (n == name).then_some(m))
}

/// The distance a measure gives from its sums: of a term over the
/// dimensions, and for COSINE the reconstruction's squared norm and the
/// query's norm.
pub(crate) fn finish(m: Measure, sum: f64, squares: f64, query_norm: f64) -> f32 {
    let d = match m {
        Measure::Square | Measure::Gap => sum,
        Measure::Euclidean => sum.max(0.0_f64).sqrt(),
        Measure::Dot => -sum,
        Measure::Cosine => {
            let scale = squares.sqrt() * query_norm;
            if scale > 0.0_f64 { 1.0_f64 - sum / scale } else { 1.0_f64 }
        }
    };
    let d = single(d);
    if d.is_finite() { d } else { UNMEASURED }
}

/// A measure of codes by decoding each and asking the metric: for a
/// metric a codebook does not tabulate, a user-defined one among them.
pub(crate) struct Decoding<'a, B: Codebook + ?Sized> {
    /// The codebook.
    pub(crate) book: &'a B,
    /// The query's FLOAT32 elements.
    pub(crate) query: Vec<u8>,
    /// The metric.
    pub(crate) metric: &'a dyn VectorMetric,
}

/// `v` as packed FLOAT32 elements.
pub(crate) fn packed(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

impl<B: Codebook + ?Sized> CodeDistance for Decoding<'_, B> {
    fn distance(&self, code: &[u8]) -> f32 {
        let mut values = Vec::new();
        self.book.decode(code, &mut values);
        let bytes = packed(&values);
        let view = |packed_bytes| VectorRef {
            element: Element::Float32,
            dims: self.book.dims(),
            norm: None,
            normalized: false,
            bytes: packed_bytes,
            indices: None,
        };
        match super::metric::distance(self.metric, view(&self.query), view(&bytes)) {
            Ok(Some(d)) => {
                let d = single(d);
                if d.is_finite() { d } else { UNMEASURED }
            }
            Ok(None) | Err(_) => UNMEASURED,
        }
    }
}

/// The norm of `v`, in `f64`.
pub(crate) fn norm_of(v: &[f32]) -> f64 {
    v.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>().sqrt()
}

/// The byte nearest `x`, clamped to 0 to 255.
pub(crate) fn byte_of(x: f64) -> u8 {
    super::integral(x.round().clamp(0.0, f64::from(u8::MAX))).and_then(|b| u8::try_from(b).ok()).unwrap_or_default()
}

/// The little-endian u32 at `at` in `bytes`.
pub(crate) fn word_at(bytes: &[u8], at: usize) -> Option<u32> {
    let w = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(<[u8; 4]>::try_from(w).ok()?))
}

/// The little-endian f32s of `bytes`.
pub(crate) fn floats(bytes: &[u8]) -> Vec<f32> {
    bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
}

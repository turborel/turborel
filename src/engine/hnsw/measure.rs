//! \[DESIGN\] What the vector index engines measure with and compress
//! with. A metric is a named distance over two `f32` slices of one length.
//! A quantizer trains a codec, which turns a vector into a fixed number of
//! code bytes and back. Both are taken as small objects here, so the
//! engines depend on no particular registry; the index methods adapt the
//! database's metrics and quantizers to them.
//!
//! The built-in codec is SQ8, the per-dimension scalar quantization of
//! the vector design document §7.2: each dimension's minimum and step, a
//! byte a dimension, four times smaller than FLOAT32.

use std::sync::Arc;

use crate::bytes::Reader;
use crate::error::{Code, Result, SqlError};
use crate::vector::kernels::squared_norm;

/// A distance of two vectors of one length: finite, smaller nearer.
pub type DistanceFn = dyn Fn(&[f32], &[f32]) -> f32 + Send + Sync;

/// A vector and its squared norm (`kernels::squared_norm`).
pub type Probe<'a> = (&'a [f32], f32);

/// A distance of two vectors each given with its squared norm, the
/// `DistanceFn`'s bit for bit.
pub type NormedFn = dyn Fn(Probe<'_>, Probe<'_>) -> f32 + Send + Sync;

/// \[DESIGN\] A named distance. Its contract is the metric registry's:
/// deterministic, finite, no callback into the database and no lock held.
/// A result that is not finite is taken as infinitely far, so a broken
/// metric can misplace entries but never panic or loop.
///
/// \[DESIGN\] A metric measured by norms (COSINE) is given a `NormedFn`
/// too. The engines keep each vector's squared norm with it and work out
/// a query's once (`probe`), so a distance sums `a·b` alone.
#[derive(Clone)]
pub struct Metric {
    name: String,
    distance: Arc<DistanceFn>,
    normed: Option<Arc<NormedFn>>,
}

impl Metric {
    /// The metric `name`, measuring by `distance`.
    pub fn new(name: &str, distance: Arc<DistanceFn>) -> Metric {
        Metric { name: name.trim().to_ascii_uppercase(), distance, normed: None }
    }

    /// The metric, measuring by `normed` where the norms are known.
    #[must_use]
    pub fn with_norms(self, normed: Option<Arc<NormedFn>>) -> Metric {
        Metric { normed, ..self }
    }

    /// `q` with its squared norm, worked out once for a search.
    pub fn probe<'a>(&self, q: &'a [f32]) -> Probe<'a> {
        (q, squared_norm(q))
    }

    /// The distance of `a` and `b` given their squared norms, as
    /// `distance` gives it.
    #[inline]
    pub fn between(&self, a: Probe<'_>, b: Probe<'_>) -> f32 {
        let d = match &self.normed {
            Some(normed) => normed(a, b),
            None => (self.distance)(a.0, b.0),
        };
        if d.is_finite() { d } else { f32::INFINITY }
    }

    /// Its name, upper case.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The distance of `a` and `b`, infinite if the metric gave no finite one.
    #[inline]
    pub fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        let d = (self.distance)(a, b);
        if d.is_finite() { d } else { f32::INFINITY }
    }

    /// Refuses a vector the metric cannot measure: one of the wrong
    /// length, a component not finite, or a distance to itself not finite.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 naming the metric or the length.
    pub fn check(&self, vector: &[f32], dims: usize) -> Result<()> {
        if vector.len() != dims || vector.iter().any(|x| !x.is_finite()) {
            return Err(SqlError::new(
                Code::Api,
                format!("[DESIGN] a vector index of {dims} dimensions was given a vector it cannot hold"),
            ));
        }
        if !(self.distance)(vector, vector).is_finite() {
            return Err(SqlError::new(
                Code::Api,
                format!("[DESIGN] the metric {} broke its contract: a distance was not finite", self.name),
            ));
        }
        Ok(())
    }
}

/// \[DESIGN\] A trained codec: a vector to `code_bytes` bytes and back.
/// Encoding is deterministic, so a vector encoded twice gives one code.
pub trait Codec: Send + Sync {
    /// The bytes of each code.
    fn code_bytes(&self) -> usize;
    /// Writes the code of `vector` to `out`, `code_bytes` long.
    fn encode(&self, vector: &[f32], out: &mut [u8]);
    /// Writes the vector `code` stands for to `out`, as long as a vector.
    fn decode(&self, code: &[u8], out: &mut [f32]);
    /// Its trained state, as `Quantizer::read` reads it.
    fn write(&self, out: &mut Vec<u8>);
}

/// \[DESIGN\] A kind of codec, by name, trained on a sample.
pub trait Quantizer: Send + Sync {
    /// Its name, upper case, kept in an index's metadata.
    fn name(&self) -> &str;
    /// A codec trained on `sample`, vectors of `dims` one after another.
    ///
    /// # Errors
    ///
    /// The sample cannot train one.
    fn train(&self, sample: &[f32], dims: usize, seed: u64) -> Result<Arc<dyn Codec>>;
    /// The codec `Codec::write` wrote.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the bytes are malformed.
    fn read(&self, bytes: &[u8], dims: usize) -> Result<Arc<dyn Codec>>;
}

/// The name of the built-in scalar quantizer.
pub const SQ8: &str = "SQ8";
/// The greatest code of a dimension: a byte's.
const SQ8_LEVELS: f32 = 255.0;
/// The highest bit of a byte, where finding the nearest code starts.
const SQ8_TOP_BIT: u8 = 0x80;
/// Half a step, so a value rounds to its nearest code.
const SQ8_HALF_LEVEL: f32 = 0.5;

/// \[DESIGN\] SQ8: each dimension's least value and step from a sample.
pub struct Sq8;

/// A trained SQ8 codec: each dimension's least value, then its step.
struct Sq8Codec {
    low: Vec<f32>,
    step: Vec<f32>,
}

impl Quantizer for Sq8 {
    fn name(&self) -> &str {
        SQ8
    }
    fn train(&self, sample: &[f32], dims: usize, _seed: u64) -> Result<Arc<dyn Codec>> {
        if dims == 0 || sample.len() < dims {
            return Err(SqlError::new(Code::Api, "[DESIGN] SQ8 needs at least one vector to train on"));
        }
        let mut low = vec![f32::INFINITY; dims];
        let mut high = vec![f32::NEG_INFINITY; dims];
        for vector in sample.chunks_exact(dims) {
            for ((x, l), h) in vector.iter().zip(low.iter_mut()).zip(high.iter_mut()) {
                *l = l.min(*x);
                *h = h.max(*x);
            }
        }
        let step = low.iter().zip(&high).map(|(l, h)| if h > l { (h - l) / SQ8_LEVELS } else { 1.0 }).collect();
        Ok(Arc::new(Sq8Codec { low, step }))
    }
    fn read(&self, bytes: &[u8], dims: usize) -> Result<Arc<dyn Codec>> {
        let mut r = Reader::new(bytes, "an SQ8 codec");
        let mut floats = || -> Result<Vec<f32>> { (0..dims).map(|_| Ok(f32::from_bits(r.u32()?))).collect() };
        let low = floats()?;
        let step = floats()?;
        if step.iter().any(|s| !(s.is_finite() && *s > 0.0)) || low.iter().any(|l| !l.is_finite()) {
            return Err(SqlError::new(Code::Corrupt, "[DESIGN] an SQ8 codec is malformed: a step is not positive"));
        }
        Ok(Arc::new(Sq8Codec { low, step }))
    }
}

impl Codec for Sq8Codec {
    fn code_bytes(&self) -> usize {
        self.low.len()
    }
    fn encode(&self, vector: &[f32], out: &mut [u8]) {
        for (((x, l), s), o) in vector.iter().zip(&self.low).zip(&self.step).zip(out.iter_mut()) {
            *o = nearest_level((x - l) / s);
        }
    }
    fn decode(&self, code: &[u8], out: &mut [f32]) {
        for (((c, l), s), o) in code.iter().zip(&self.low).zip(&self.step).zip(out.iter_mut()) {
            *o = l + s * f32::from(*c);
        }
    }
    fn write(&self, out: &mut Vec<u8>) {
        for x in self.low.iter().chain(&self.step) {
            out.extend_from_slice(&x.to_bits().to_le_bytes());
        }
    }
}

/// The byte nearest `x`, clamped to 0..=255: found bit by bit, as no
/// float converts to an integer without a cast (R-NUM-2).
fn nearest_level(x: f32) -> u8 {
    let mut level = 0_u8;
    let mut bit = SQ8_TOP_BIT;
    while bit > 0 {
        let tried = level | bit;
        if f32::from(tried) - SQ8_HALF_LEVEL <= x {
            level = tried;
        }
        bit >>= 1_u32;
    }
    level
}

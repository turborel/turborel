//! \[EXTENSION\] SQ8, scalar quantization (design document §7.2): each
//! dimension's range in the sample, its least to its greatest value, cut
//! into 255 even steps, so a code is a byte a dimension, a quarter of
//! FLOAT32. A value outside the range is clamped to it.

use super::quantizer::{
    CODEBOOK_VERSION, CodeDistance, Codebook, Decoding, Measure, Quantizer, SQ8, bad_codebook, byte_of, collect, finish, floats, measure,
    norm_of, packed, training, word_at,
};
use crate::error::Result;
use crate::vector::metric::VectorMetric;

/// A codebook's header: version and dimensions.
const HEADER: usize = 5;

/// The SQ8 quantizer.
#[derive(Debug, Clone, Copy)]
pub struct ScalarQuantizer;

/// A trained SQ8 codebook: each dimension's least value and step.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScalarCodebook {
    low: Vec<f32>,
    step: Vec<f32>,
}

impl ScalarCodebook {
    fn value(&self, d: usize, code: u8) -> f64 {
        let low = f64::from(self.low.get(d).copied().unwrap_or_default());
        low + f64::from(code) * f64::from(self.step.get(d).copied().unwrap_or_default())
    }
}

impl Quantizer for ScalarQuantizer {
    fn name(&self) -> &'static str {
        SQ8
    }

    fn train(
        &self,
        sample: &mut dyn Iterator<Item = Result<Vec<f32>>>,
        dims: u32,
        code_bytes: usize,
        _seed: u64,
    ) -> Result<Box<dyn Codebook>> {
        let vectors = collect(sample, dims)?;
        let width = usize::try_from(dims).unwrap_or_default();
        if code_bytes != 0 && code_bytes != width {
            return Err(training("SQ8 codes are a byte a dimension"));
        }
        let mut low = vec![f32::INFINITY; width];
        let mut high = vec![f32::NEG_INFINITY; width];
        for v in &vectors {
            for ((lo, hi), x) in low.iter_mut().zip(high.iter_mut()).zip(v) {
                *lo = lo.min(*x);
                *hi = hi.max(*x);
            }
        }
        let step = low.iter().zip(&high).map(|(lo, hi)| super::single((f64::from(*hi) - f64::from(*lo)) / f64::from(u8::MAX))).collect();
        Ok(Box::new(ScalarCodebook { low, step }))
    }

    fn read(&self, bytes: &[u8]) -> Result<Box<dyn Codebook>> {
        let (Some(&CODEBOOK_VERSION), Some(dims)) = (bytes.first(), word_at(bytes, 1)) else { return Err(bad_codebook(SQ8)) };
        let width = usize::try_from(dims).map_err(|_| bad_codebook(SQ8))?;
        let all = floats(bytes.get(HEADER..).unwrap_or_default());
        if all.len() != width.saturating_mul(2) || bytes.len() != HEADER.saturating_add(width.saturating_mul(8)) {
            return Err(bad_codebook(SQ8));
        }
        let (low, step) = all.split_at(width);
        Ok(Box::new(ScalarCodebook { low: low.to_vec(), step: step.to_vec() }))
    }
}

/// A query's measure of SQ8 codes, each element rebuilt as it is read.
struct Table<'a> {
    book: &'a ScalarCodebook,
    measure: Measure,
    query: Vec<f32>,
    query_norm: f64,
}

impl CodeDistance for Table<'_> {
    fn distance(&self, code: &[u8]) -> f32 {
        let (mut sum, mut squares) = (0.0_f64, 0.0_f64);
        for (d, (c, q)) in code.iter().zip(&self.query).enumerate() {
            let (x, q) = (self.book.value(d, *c), f64::from(*q));
            sum += match self.measure {
                Measure::Square | Measure::Euclidean => (q - x) * (q - x),
                Measure::Dot | Measure::Cosine => q * x,
                Measure::Gap => (q - x).abs(),
            };
            squares += x * x;
        }
        finish(self.measure, sum, squares, self.query_norm)
    }
}

impl Codebook for ScalarCodebook {
    fn code_bytes(&self) -> usize {
        self.low.len()
    }

    fn dims(&self) -> u32 {
        u32::try_from(self.low.len()).unwrap_or_default()
    }

    fn encode(&self, v: &[f32], out: &mut [u8]) {
        for (d, (slot, x)) in out.iter_mut().zip(v).enumerate() {
            let low = f64::from(self.low.get(d).copied().unwrap_or_default());
            let step = f64::from(self.step.get(d).copied().unwrap_or_default());
            *slot = if step > 0.0_f64 { byte_of((f64::from(*x) - low) / step) } else { 0 };
        }
    }

    fn decode(&self, code: &[u8], out: &mut Vec<f32>) {
        out.clear();
        out.extend(code.iter().enumerate().map(|(d, c)| super::single(self.value(d, *c))));
    }

    fn table<'a>(&'a self, query: &[f32], metric: &'a dyn VectorMetric) -> Box<dyn CodeDistance + 'a> {
        match measure(metric) {
            Some(m) => Box::new(Table { book: self, measure: m, query: query.to_vec(), query_norm: norm_of(query) }),
            None => Box::new(Decoding { book: self, query: packed(query), metric }),
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.push(CODEBOOK_VERSION);
        out.extend_from_slice(&self.dims().to_le_bytes());
        out.extend(self.low.iter().chain(&self.step).flat_map(|x| x.to_le_bytes()));
    }
}

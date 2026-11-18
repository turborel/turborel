//! \[EXTENSION\] BQ, binary quantization (design document §7.2): a bit a
//! dimension, set where the value is above that dimension's mean in the
//! sample, so a code is a 32nd of FLOAT32. Codes are ranked by Hamming
//! distance from the query's own code, whatever the metric: the design
//! document's "Hamming pre-ranking", for a re-rank to correct. A code
//! decodes to each dimension's mean plus or minus its mean absolute
//! deviation.

use super::float;
use super::kernels::bits;
use super::quantizer::{BQ, CODEBOOK_VERSION, CodeDistance, Codebook, Quantizer, bad_codebook, collect, floats, training, word_at};
use crate::error::Result;
use crate::vector::metric::VectorMetric;

/// A codebook's header: version and dimensions.
const HEADER: usize = 5;
/// The bit of the first dimension in a code byte.
const HIGH_BIT: u8 = 0x80;

/// The BQ quantizer.
#[derive(Debug, Clone, Copy)]
pub struct BinaryQuantizer;

/// A trained BQ codebook: each dimension's mean and mean absolute
/// deviation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BinaryCodebook {
    mean: Vec<f32>,
    spread: Vec<f32>,
}

/// The bytes of a code of `width` bits.
fn code_len(width: usize) -> usize {
    width.div_ceil(8)
}

fn mean_of(vectors: &[Vec<f32>], width: usize, of: impl Fn(usize, f32) -> f64) -> Vec<f32> {
    let mut sums = vec![0.0_f64; width];
    for v in vectors {
        for (d, (sum, x)) in sums.iter_mut().zip(v).enumerate() {
            *sum += of(d, *x);
        }
    }
    let n = float::count(vectors.len());
    sums.into_iter().map(|s| super::single(s / n)).collect()
}

impl Quantizer for BinaryQuantizer {
    fn name(&self) -> &'static str {
        BQ
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
        if code_bytes != 0 && code_bytes != code_len(width) {
            return Err(training("BQ codes are a bit a dimension"));
        }
        let mean = mean_of(&vectors, width, |_, x| f64::from(x));
        let spread = mean_of(&vectors, width, |d, x| (f64::from(x) - f64::from(mean.get(d).copied().unwrap_or_default())).abs());
        Ok(Box::new(BinaryCodebook { mean, spread }))
    }

    fn read(&self, bytes: &[u8]) -> Result<Box<dyn Codebook>> {
        let (Some(&CODEBOOK_VERSION), Some(dims)) = (bytes.first(), word_at(bytes, 1)) else { return Err(bad_codebook(BQ)) };
        let width = usize::try_from(dims).map_err(|_| bad_codebook(BQ))?;
        let all = floats(bytes.get(HEADER..).unwrap_or_default());
        if bytes.len() != HEADER.saturating_add(width.saturating_mul(8)) {
            return Err(bad_codebook(BQ));
        }
        let (mean, spread) = all.split_at(width.min(all.len()));
        Ok(Box::new(BinaryCodebook { mean: mean.to_vec(), spread: spread.to_vec() }))
    }
}

/// A query's own code, against which codes are counted.
struct Table {
    code: Vec<u8>,
}

impl CodeDistance for Table {
    fn distance(&self, code: &[u8]) -> f32 {
        let differ = bits(&self.code, code, |p, q| p ^ q);
        f32::from(u16::try_from(differ).unwrap_or(u16::MAX))
    }
}

impl Codebook for BinaryCodebook {
    fn code_bytes(&self) -> usize {
        code_len(self.mean.len())
    }

    fn dims(&self) -> u32 {
        u32::try_from(self.mean.len()).unwrap_or_default()
    }

    fn encode(&self, v: &[f32], out: &mut [u8]) {
        out.fill(0);
        for (d, (x, m)) in v.iter().zip(&self.mean).enumerate() {
            let bit = u32::try_from(d & 7).unwrap_or_default();
            if let Some(byte) = out.get_mut(d >> 3_u32)
                && x > m
            {
                *byte |= HIGH_BIT >> bit;
            }
        }
    }

    fn decode(&self, code: &[u8], out: &mut Vec<f32>) {
        out.clear();
        for (d, (m, s)) in self.mean.iter().zip(&self.spread).enumerate() {
            let bit = u32::try_from(d & 7).unwrap_or_default();
            let set = code.get(d >> 3_u32).is_some_and(|b| b & (HIGH_BIT >> bit) != 0);
            out.push(if set { m + s } else { m - s });
        }
    }

    fn table<'a>(&'a self, query: &[f32], _metric: &'a dyn VectorMetric) -> Box<dyn CodeDistance + 'a> {
        let mut code = vec![0_u8; self.code_bytes()];
        self.encode(query, &mut code);
        Box::new(Table { code })
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.push(CODEBOOK_VERSION);
        out.extend_from_slice(&self.dims().to_le_bytes());
        out.extend(self.mean.iter().chain(&self.spread).flat_map(|x| x.to_le_bytes()));
    }
}

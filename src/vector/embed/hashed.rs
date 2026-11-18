//! \[EXTENSION\] The built-in models: feature hashing (Weinberger,
//! Dasgupta, Langford, Smola and Attenberg, "Feature Hashing for Large
//! Scale Multitask Learning", ICML 2009). A text's tokens, and each run
//! of `ngrams` tokens, are its features; a feature's hash picks its
//! dimension, and for a dense vector a second hash bit its sign, so that
//! collisions cancel on average and the inner product of two vectors is
//! an unbiased estimate of that of their feature counts (§2 of the
//! paper).
//!
//! * HASH_DENSE: 384 FLOAT32 dimensions, tokens and pairs of them, each
//!   feature +1 or -1, the vector made of norm 1 (so COSINE and DOT agree).
//! * HASH_SPARSE: a SPARSE vocabulary of 2^20, tokens alone, each weight
//!   1 + ln(count), the vector of norm 1, for lexical search through an
//!   inverted index.
//!
//! A token is a run of letters and digits, in lower case. Equal texts
//! give equal vectors on every machine, as the hash is MurmurHash3 x64 128
//! of the feature's UTF-8 with a fixed seed, and sums run in token order.

use std::collections::BTreeMap;

use super::{Embedder, ModelShape};
use crate::error::Result;
use crate::hash::murmur::murmur3_x64_128;
use crate::vector::chunk::Span;
use crate::vector::{Element, Layout, Vector};

/// The dense model's name and dimensions.
pub const HASH_DENSE: &str = "HASH_DENSE";
/// Its dimensions, as small sentence models have.
pub const HASH_DENSE_DIMS: u32 = 384;
/// The sparse model's name.
pub const HASH_SPARSE: &str = "HASH_SPARSE";
/// Its vocabulary.
pub const HASH_SPARSE_DIMS: u32 = 1 << 20;
/// \[DESIGN\] The seed of every feature's hash, fixed for all time.
const SEED: u32 = 0x454d_4244;
/// What parts the tokens of an n-gram in its hashed bytes.
const JOINER: u8 = 0x1f;
/// The most tokens an n-gram has.
pub const MOST_NGRAMS: usize = 4;

/// \[EXTENSION\] A feature-hashing model.
#[derive(Debug, Clone)]
pub struct HashedEmbedder {
    name: String,
    shape: ModelShape,
    ngrams: usize,
}

impl HashedEmbedder {
    /// A model `name` of `dims` dimensions, dense or sparse, its features
    /// runs of 1 to `ngrams` tokens (at most `MOST_NGRAMS`).
    pub fn new(name: &str, dims: u32, layout: Layout, ngrams: usize) -> HashedEmbedder {
        let shape = ModelShape { dims: dims.max(1), element: Element::Float32, layout };
        HashedEmbedder { name: name.to_uppercase(), shape, ngrams: ngrams.clamp(1, MOST_NGRAMS) }
    }

    /// HASH_DENSE.
    pub fn dense() -> HashedEmbedder {
        HashedEmbedder::new(HASH_DENSE, HASH_DENSE_DIMS, Layout::Dense, 2)
    }

    /// HASH_SPARSE.
    pub fn sparse() -> HashedEmbedder {
        HashedEmbedder::new(HASH_SPARSE, HASH_SPARSE_DIMS, Layout::Sparse, 1)
    }

    /// Each feature of `text`: its dimension and, dense, its sign.
    fn features(&self, text: &str) -> Vec<(u32, bool)> {
        let chars: Vec<char> = text.chars().collect();
        let tokens: Vec<String> = token_spans(&chars).iter().map(|&(from, to)| lowered(&chars, (from, to))).collect();
        let mut out = Vec::new();
        for n in 1..=self.ngrams {
            for run in tokens.windows(n) {
                let joined: Vec<&[u8]> = run.iter().map(String::as_bytes).collect();
                let bytes = joined.join(&JOINER);
                let (high, low) = murmur3_x64_128(&bytes, SEED);
                let dim = u32::try_from(high.rem_euclid(u64::from(self.shape.dims))).unwrap_or(0);
                out.push((dim, low & 1 == 1));
            }
        }
        out
    }

    /// The vector of `text`.
    fn vector(&self, text: &str) -> Result<Vector> {
        let features = self.features(text);
        match self.shape.layout {
            Layout::Dense => {
                let mut sums = vec![0.0_f64; usize::try_from(self.shape.dims).unwrap_or(0)];
                for (dim, negative) in features {
                    let sign = if negative { -1.0_f64 } else { 1.0_f64 };
                    usize::try_from(dim).ok().and_then(|d| sums.get_mut(d)).into_iter().for_each(|slot| *slot += sign);
                }
                Vector::dense(Element::Float32, normalized(sums))
            }
            Layout::Sparse => {
                let mut counts: BTreeMap<u32, u32> = BTreeMap::new();
                for (dim, _) in features {
                    let count = counts.entry(dim).or_insert(0);
                    *count = count.saturating_add(1);
                }
                let (dims, weights): (Vec<u32>, Vec<f64>) = counts.into_iter().map(|(d, c)| (d, 1.0_f64 + f64::from(c).ln())).unzip();
                Vector::sparse(Element::Float32, self.shape.dims, dims.into_iter().zip(normalized(weights)).collect())
            }
        }
    }
}

/// `values` of norm 1, or as they are if all are 0.
fn normalized(values: Vec<f64>) -> Vec<f64> {
    let norm = values.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm > 0.0_f64 { values.into_iter().map(|v| v / norm).collect() } else { values }
}

/// The token of `span`, in lower case.
fn lowered(chars: &[char], (from, to): Span) -> String {
    chars.get(from..to).unwrap_or_default().iter().flat_map(|c| c.to_lowercase()).collect()
}

/// The spans of the tokens of `chars`: runs of letters and digits.
pub fn token_spans(chars: &[char]) -> Vec<Span> {
    let mut out = Vec::new();
    let mut start = None;
    for (at, c) in chars.iter().enumerate() {
        match (c.is_alphanumeric(), start) {
            (true, None) => start = Some(at),
            (false, Some(from)) => {
                out.push((from, at));
                start = None;
            }
            (true, Some(_)) | (false, None) => {}
        }
    }
    if let Some(from) = start {
        out.push((from, chars.len()));
    }
    out
}

impl Embedder for HashedEmbedder {
    fn name(&self) -> &str {
        &self.name
    }
    fn shape(&self) -> ModelShape {
        self.shape
    }
    fn tokens(&self, text: &str) -> Vec<Span> {
        token_spans(&text.chars().collect::<Vec<_>>())
    }
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vector>> {
        texts.iter().map(|text| self.vector(text)).collect()
    }
}

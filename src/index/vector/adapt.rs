//! \[EXTENSION\] The database's metrics and quantizers (`vector/metric.rs`,
//! `vector/quantizer.rs`, the extension points of the vector design
//! document §6.2, §7.2, §8) adapted to the small traits the engines
//! compute with, which know no registry: a distance over two `f32`
//! slices; DiskANN's `Quantizer`, `Codes` and `CodeTable`; and HNSW's
//! and IVF's `Quantizer` and `Codec`.
//!
//! A built-in metric is measured on the slices at once, by the vector
//! module's kernels: an index orders candidates only, and the executor
//! measures each again by the SQL call (D-APPROX), so the `f32` sum of a
//! FLOAT16 or INT8 column (R3.10) serves. A user-defined metric is given
//! the two vectors in the column's own format, as a SQL call gives them;
//! a failure or a null is taken as farthest, so a broken metric misplaces
//! entries but never fails a write.

use std::sync::Arc;

use crate::engine::diskann::{self, CodeTable, Codes};
use crate::engine::hnsw::{self, Codec};
use crate::error::{Code, Result, SqlError};
use crate::vector::kernels::{cosine_terms, fold_f32, gap, product, square};
use crate::vector::metric::{COSINE, DOT, EUCLIDEAN, EUCLIDEAN_SQUARED, HAMMING, JACCARD, MANHATTAN, VectorMetric};
use crate::vector::quantizer::{CodeDistance, Codebook, Quantizer};
use crate::vector::{Element, VectorRef, half_bits, single};

use super::Column;

/// A distance the engines compute with.
pub(crate) type Distance = Arc<dyn Fn(&[f32], &[f32]) -> f32 + Send + Sync>;

/// The distance of a metric that gave none, or failed: farthest, finite.
const FARTHEST: f32 = f32::MAX;
/// Half of one: an element of a BINARY vector at least this is a set bit.
const HALF: f32 = 0.5;
/// The bits of a byte.
const BYTE_BITS: usize = 8;
/// The highest bit of a byte, a BINARY vector's first element.
const HIGH_BIT: u8 = 0x80;

/// `metric` as a distance of two vectors of `column`'s shape, read as
/// `f32`s.
pub(crate) fn distance(metric: &Arc<dyn VectorMetric>, column: Column) -> Distance {
    match metric.name() {
        EUCLIDEAN_SQUARED => Arc::new(|a: &[f32], b: &[f32]| fold_f32(a, b, square)),
        EUCLIDEAN => Arc::new(|a: &[f32], b: &[f32]| fold_f32(a, b, square).sqrt()),
        DOT => Arc::new(|a: &[f32], b: &[f32]| -fold_f32(a, b, product)),
        MANHATTAN => Arc::new(|a: &[f32], b: &[f32]| fold_f32(a, b, gap)),
        COSINE => Arc::new(cosine),
        HAMMING => Arc::new(hamming),
        JACCARD => Arc::new(jaccard),
        _ => {
            let metric = Arc::clone(metric);
            Arc::new(move |a: &[f32], b: &[f32]| called(&*metric, column, (a, b)))
        }
    }
}

/// COSINE: `1 - a·b / (|a| |b|)`; of a zero vector, whose SQL distance is
/// null (R6.7), the middle of its range.
///
/// Its three sums are taken in one pass (`cosine_terms`), each as a
/// separate fold would sum it, so the distance is bit for bit the same.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (aa, bb, ab) = cosine_terms(a, b);
    let scale = (aa * bb).sqrt();
    if scale > 0.0_f32 { 1.0_f32 - ab / scale } else { 1.0_f32 }
}

/// HAMMING: the count of elements that differ, bits of a BINARY column.
fn hamming(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).fold(0.0_f32, |n, (x, y)| if x.to_bits() == y.to_bits() { n } else { n + 1.0_f32 })
}

/// JACCARD of BINARY columns: `1 - |a ∧ b| / |a ∨ b|`, and 0 of two
/// vectors with no bit set.
fn jaccard(a: &[f32], b: &[f32]) -> f32 {
    let (both, either) = a.iter().zip(b).fold((0.0_f32, 0.0_f32), |(both, either), (x, y)| {
        let (p, q) = (*x >= HALF, *y >= HALF);
        (if p && q { both + 1.0_f32 } else { both }, if p || q { either + 1.0_f32 } else { either })
    });
    if either > 0.0_f32 { 1.0_f32 - both / either } else { 0.0_f32 }
}

/// A user-defined metric's distance of `a` and `b`, given in `column`'s
/// format.
fn called(metric: &dyn VectorMetric, column: Column, (a, b): (&[f32], &[f32])) -> f32 {
    let (left, right) = (packed(column.element, a), packed(column.element, b));
    let view = |bytes| VectorRef { element: column.element, dims: column.dims, norm: None, normalized: false, bytes, indices: None };
    match metric.distance(view(&left), view(&right)) {
        Ok(Some(d)) => Some(single(d)).filter(|d| d.is_finite()).unwrap_or(FARTHEST),
        Ok(None) | Err(_) => FARTHEST,
    }
}

/// `v`, read from a vector of format `element`, packed in that format
/// again; FLOAT64, which no index takes, as FLOAT32.
fn packed(element: Element, v: &[f32]) -> Vec<u8> {
    match element {
        Element::Float32 | Element::Float64 => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        Element::Float16 => v.iter().flat_map(|x| half_bits(f64::from(*x)).to_le_bytes()).collect(),
        Element::Int8 => v.iter().map(|x| byte(*x)).collect(),
        Element::Binary => {
            let mut out = vec![0_u8; v.len().div_ceil(BYTE_BITS)];
            for (bits, chunk) in out.iter_mut().zip(v.chunks(BYTE_BITS)) {
                *bits = chunk.iter().zip(0_u32..).fold(0_u8, |b, (x, at)| if *x >= HALF { b | (HIGH_BIT >> at) } else { b });
            }
            out
        }
    }
}

/// The INT8 element `x`, a whole number in range as it was read.
fn byte(x: f32) -> u8 {
    let whole = crate::vector::integral(f64::from(x)).unwrap_or_default();
    i8::try_from(whole).unwrap_or_default().to_le_bytes().first().copied().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// DiskANN's codes
// ---------------------------------------------------------------------------

/// `quantizer` as DiskANN's: its codebooks measure by `metric`, with
/// codes of `code_bytes` (0 for the quantizer's default).
pub(crate) fn disk_quantizer(
    quantizer: Arc<dyn Quantizer>,
    metric: Arc<dyn VectorMetric>,
    code_bytes: usize,
) -> Arc<dyn diskann::Quantizer> {
    Arc::new(DiskQuantizer { quantizer, metric, code_bytes })
}

/// A database quantizer as DiskANN's.
struct DiskQuantizer {
    quantizer: Arc<dyn Quantizer>,
    metric: Arc<dyn VectorMetric>,
    code_bytes: usize,
}

impl diskann::Quantizer for DiskQuantizer {
    fn train(&self, sample: &[f32], dims: usize, seed: u64) -> Result<Arc<dyn Codes>> {
        let book = trained(&*self.quantizer, sample, dims, (self.code_bytes, seed))?;
        Ok(Arc::new(DiskCodes { book, metric: Arc::clone(&self.metric) }))
    }
    fn read(&self, bytes: &[u8]) -> Result<Arc<dyn Codes>> {
        Ok(Arc::new(DiskCodes { book: self.quantizer.read(bytes)?, metric: Arc::clone(&self.metric) }))
    }
}

/// A codebook trained by `quantizer` on `sample`, vectors of `dims` one
/// after another.
fn trained(quantizer: &dyn Quantizer, sample: &[f32], dims: usize, (code_bytes, seed): (usize, u64)) -> Result<Box<dyn Codebook>> {
    let mut vectors = sample.chunks_exact(dims.max(1)).map(|v| Ok(v.to_vec()));
    let dims = u32::try_from(dims).map_err(|_| SqlError::new(Code::Api, "[DESIGN] a vector index's dimensions exceed a u32"))?;
    quantizer.train(&mut vectors, dims, code_bytes, seed)
}

/// A database codebook as DiskANN's codes.
struct DiskCodes {
    book: Box<dyn Codebook>,
    metric: Arc<dyn VectorMetric>,
}

impl Codes for DiskCodes {
    fn code_bytes(&self) -> usize {
        self.book.code_bytes()
    }
    fn encode(&self, vector: &[f32], out: &mut [u8]) {
        self.book.encode(vector, out);
    }
    fn table<'a>(&'a self, query: &[f32]) -> Box<dyn CodeTable + 'a> {
        Box::new(Table(self.book.table(query, &*self.metric)))
    }
    fn write(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.book.write(&mut out);
        out
    }
}

/// A codebook's measure of one query, as DiskANN's.
struct Table<'a>(Box<dyn CodeDistance + 'a>);

impl CodeTable for Table<'_> {
    fn distance(&self, code: &[u8]) -> f32 {
        self.0.distance(code)
    }
}

// ---------------------------------------------------------------------------
// HNSW's and IVF's codecs
// ---------------------------------------------------------------------------

/// The quantizer HNSW and IVF keep codes of, by name: their own SQ8, or a
/// database quantizer's codebooks with codes of `code_bytes` (PQ_BYTES, 0
/// for the quantizer's default).
pub(crate) fn codec_quantizer(quantizer: Arc<dyn Quantizer>, code_bytes: usize) -> Arc<dyn hnsw::Quantizer> {
    if quantizer.name() == hnsw::SQ8 {
        return Arc::new(hnsw::Sq8);
    }
    Arc::new(CodecQuantizer(quantizer, code_bytes))
}

/// A database quantizer as HNSW's and IVF's, and its codes' bytes.
struct CodecQuantizer(Arc<dyn Quantizer>, usize);

impl hnsw::Quantizer for CodecQuantizer {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn train(&self, sample: &[f32], dims: usize, seed: u64) -> Result<Arc<dyn Codec>> {
        Ok(Arc::new(BookCodec(trained(&*self.0, sample, dims, (self.1, seed))?)))
    }
    fn read(&self, bytes: &[u8], dims: usize) -> Result<Arc<dyn Codec>> {
        let book = self.0.read(bytes)?;
        if usize::try_from(book.dims()).ok() != Some(dims) {
            return Err(SqlError::new(Code::Corrupt, "[DESIGN] a vector index's codebook is of other dimensions"));
        }
        Ok(Arc::new(BookCodec(book)))
    }
}

/// A database codebook as a codec.
struct BookCodec(Box<dyn Codebook>);

impl Codec for BookCodec {
    fn code_bytes(&self) -> usize {
        self.0.code_bytes()
    }
    fn encode(&self, vector: &[f32], out: &mut [u8]) {
        self.0.encode(vector, out);
    }
    fn decode(&self, code: &[u8], out: &mut [f32]) {
        let mut values = Vec::with_capacity(out.len());
        self.0.decode(code, &mut values);
        out.iter_mut().zip(values).for_each(|(o, v)| *o = v);
    }
    fn write(&self, out: &mut Vec<u8>) {
        self.0.write(out);
    }
}

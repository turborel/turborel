//! \[EXTENSION\] What the approximate vector index methods "diskann",
//! "hnsw" and "ivf" share (the vector design document §10): the column
//! they index and its keys (`column.rs`), the database's metrics and
//! quantizers adapted to the engines' own small traits (`adapt.rs`), the
//! request an approximate search makes of them (`ask.rs`), the place
//! their pages and memory come from (`place.rs`), and one `Index` over
//! any of the three engines (`index.rs`).
//!
//! Each method indexes one dense VECTOR column of a fixed dimension count
//! and format (R3.6, R10.3), never NULL (R10.6), by the metric its
//! options name, read from the database's metrics. Its indexes answer
//! approximate nearest-neighbour searches of a call of that metric alone
//! (R13.1, RULES.md D-APPROX), and no comparison, as vectors have no
//! order (R3.11).

mod adapt;
mod ask;
mod column;
mod index;
mod options;
mod place;

pub(crate) use adapt::{codec_quantizer, disk_quantizer, distance};
pub(crate) use ask::Ask;
pub(crate) use column::{Column, approximate_support, column_of, default_desc, plain, projection};
pub(crate) use index::{Engine, Held, VectorIndex, located};
pub(crate) use options::{Shared, serves, shared, shared_of};
pub(crate) use place::{Named, Place};

use super::{DISKANN_METHOD, HNSW_METHOD, IVF_METHOD};
use crate::disk::buffer::{BufferPool, FileId};
use crate::vector::metric::Metrics;
use crate::vector::quantizer::Quantizers;

/// Whether `method` names a vector index method: "diskann", "hnsw" or
/// "ivf".
pub(crate) fn is_vector_method(method: &str) -> bool {
    [DISKANN_METHOD, HNSW_METHOD, IVF_METHOD].contains(&method)
}

/// The vector index methods, in the registered file `file` of `pool`, of
/// the metrics and quantizers `plugins` gives.
pub(crate) fn vector_methods(
    pool: &std::sync::Arc<BufferPool>,
    file: FileId,
    (metrics, quantizers): &(Metrics, Quantizers),
) -> Vec<Box<dyn super::IndexMethod>> {
    let pool = || std::sync::Arc::clone(pool);
    let (m, q) = (|| std::sync::Arc::clone(metrics), || std::sync::Arc::clone(quantizers));
    vec![
        Box::new(super::DiskAnnMethod::new(pool(), file).with_metrics(m()).with_quantizers(q())),
        Box::new(super::HnswMethod::new(pool(), file).with_metrics(m()).with_quantizers(q())),
        Box::new(super::IvfMethod::new(pool(), file).with_metrics(m()).with_quantizers(q())),
    ]
}

#[cfg(test)]
mod tests;

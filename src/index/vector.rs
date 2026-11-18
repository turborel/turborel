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
mod place;

pub(crate) use adapt::{codec_quantizer, disk_quantizer, distance};
pub(crate) use ask::Ask;
pub(crate) use column::{Column, approximate_support, column_of, default_desc, plain, projection};
pub(crate) use index::{Engine, Held, VectorIndex, located};
pub(crate) use place::Place;

use super::{DISKANN_METHOD, HNSW_METHOD, IVF_METHOD};
use crate::disk::buffer::{BufferPool, FileId};

/// Whether `method` names a vector index method: "diskann", "hnsw" or
/// "ivf".
pub(crate) fn is_vector_method(method: &str) -> bool {
    [DISKANN_METHOD, HNSW_METHOD, IVF_METHOD].contains(&method)
}

/// The vector index methods, in the registered file `file` of `pool`.
pub(crate) fn vector_methods(pool: &std::sync::Arc<BufferPool>, file: FileId) -> Vec<Box<dyn super::IndexMethod>> {
    let pool = || std::sync::Arc::clone(pool);
    vec![
        Box::new(super::DiskAnnMethod::new(pool(), file)),
        Box::new(super::HnswMethod::new(pool(), file)),
        Box::new(super::IvfMethod::new(pool(), file)),
    ]
}

#[cfg(test)]
mod tests;

//! The interchange formats of ISO/IEC 13249-3:2016 (DIS, SC32 N2593):
//! the well-known text representation (§5.1.67) and GML (§5.1.60,
//! §5.1.61, Table 14), for every type and every coordinate dimension.
//! Each reads into and writes from the value [`tree`], which [`model`]
//! moves to and from the geometry model; the model holds the well-known
//! binary representation (§5.1.68). Each canonical writer's output reads
//! back to the same value.

pub mod gml;
pub mod model;
pub mod tree;
pub mod wkt;

#[cfg(test)]
pub mod samples;

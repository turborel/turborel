//! \[DESIGN\] The layout of a sparse inverted index's tree. Every key
//! starts with a tag byte, so each kind of entry is a range of its own,
//! and big-endian numbers after it, so bytewise order is numeric order:
//!
//! | Key | Value | What |
//! |---|---|---|
//! | `D` hash(16) row(8) | count u32 | a vector of a row, inserted `count` times |
//! | `P` dimension(4) first(8) | a posting block (`block.rs`) | postings of rows from `first` |
//! | `S` dimension(4) | format(1) greatest f32, least f32 | bounds of a dimension's weights |
//!
//! A block's key is at most its first row, and below every row of the
//! next block of its dimension: a posting is put in the block of the
//! greatest key at most its row. The tags sort `D` < `P` < `S`, the order
//! a bulk load writes them in.

use super::block::corrupt;
use super::vector::HASH_BYTES;
use crate::error::Result;

/// The tag of a document's count.
pub const DOCUMENT: u8 = b'D';
/// The tag of a posting block.
pub const POSTINGS: u8 = b'P';
/// The tag of a dimension's bounds.
pub const SUMMARY: u8 = b'S';
/// The bytes of a document key.
pub const DOCUMENT_KEY: usize = 25;
/// The bytes of a block key.
pub const BLOCK_KEY: usize = 13;
/// The bytes of a row id.
const ROW_BYTES: usize = 8;
/// The bytes of a dimension.
const DIMENSION_BYTES: usize = 4;
/// The bytes of a weight.
const WEIGHT_BYTES: usize = 4;
/// The format of a dimension's bounds.
const SUMMARY_FORMAT: u8 = 1;
/// The bytes of a dimension's bounds.
const SUMMARY_BYTES: usize = 9;

/// The key of `row`'s vector of hash `hash`.
pub fn document_key(hash: &[u8; HASH_BYTES], row: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(DOCUMENT_KEY);
    key.push(DOCUMENT);
    key.extend_from_slice(hash);
    key.extend_from_slice(&row.to_be_bytes());
    key
}

/// The hash and row of a document key.
///
/// # Errors
///
/// SQLCODE -916 if it is not one.
pub fn document_of(key: &[u8]) -> Result<([u8; HASH_BYTES], u64)> {
    let malformed = || corrupt("a document key is malformed");
    let (tag, rest) = key.split_first().ok_or_else(malformed)?;
    let (hash, row) = rest.split_at_checked(HASH_BYTES).ok_or_else(malformed)?;
    if *tag != DOCUMENT {
        return Err(malformed());
    }
    Ok((hash.try_into().map_err(|_| malformed())?, u64::from_be_bytes(row.try_into().map_err(|_| malformed())?)))
}

/// The keys of every document of hash `hash`, from the first to below
/// the one past.
pub fn hash_range(hash: &[u8; HASH_BYTES]) -> (Vec<u8>, Vec<u8>) {
    (document_key(hash, 0), past(&document_key(hash, u64::MAX)))
}

/// The key of the block of `dimension` from `first`.
pub fn block_key(dimension: u32, first: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(BLOCK_KEY);
    key.push(POSTINGS);
    key.extend_from_slice(&dimension.to_be_bytes());
    key.extend_from_slice(&first.to_be_bytes());
    key
}

/// The dimension and first row of a block key.
///
/// # Errors
///
/// SQLCODE -916 if it is not one.
pub fn block_of(key: &[u8]) -> Result<(u32, u64)> {
    let malformed = || corrupt("a block key is malformed");
    let (tag, rest) = key.split_first().ok_or_else(malformed)?;
    let (dimension, first) = rest.split_at_checked(DIMENSION_BYTES).ok_or_else(malformed)?;
    if *tag != POSTINGS || first.len() != ROW_BYTES {
        return Err(malformed());
    }
    Ok((u32::from_be_bytes(dimension.try_into().map_err(|_| malformed())?), u64::from_be_bytes(first.try_into().map_err(|_| malformed())?)))
}

/// The keys of `dimension`'s blocks, from the first to below the one past.
pub fn dimension_range(dimension: u32) -> (Vec<u8>, Vec<u8>) {
    (block_key(dimension, 0), past(&block_key(dimension, u64::MAX)))
}

/// The key of `dimension`'s bounds.
pub fn summary_key(dimension: u32) -> Vec<u8> {
    let mut key = vec![SUMMARY];
    key.extend_from_slice(&dimension.to_be_bytes());
    key
}

/// The dimension of a bounds key.
///
/// # Errors
///
/// SQLCODE -916 if it is not one.
pub fn summary_of(key: &[u8]) -> Result<u32> {
    match key {
        [SUMMARY, a, b, c, d] => Ok(u32::from_be_bytes([*a, *b, *c, *d])),
        _ => Err(corrupt("a dimension's key is malformed")),
    }
}

/// The value of a dimension's bounds: the greatest and least weights.
pub fn summary_value(max: f32, min: f32) -> Vec<u8> {
    let mut value = Vec::with_capacity(SUMMARY_BYTES);
    value.push(SUMMARY_FORMAT);
    value.extend_from_slice(&max.to_le_bytes());
    value.extend_from_slice(&min.to_le_bytes());
    value
}

/// The greatest and least weights a dimension's bounds value holds.
///
/// # Errors
///
/// SQLCODE -916 if it is not one.
pub fn bounds_of(value: &[u8]) -> Result<(f32, f32)> {
    match value {
        [SUMMARY_FORMAT, rest @ ..] => {
            let (max, min) = rest.split_first_chunk::<WEIGHT_BYTES>().ok_or_else(|| corrupt("a dimension's bounds are malformed"))?;
            let min = <[u8; WEIGHT_BYTES]>::try_from(min).map_err(|_| corrupt("a dimension's bounds are malformed"))?;
            Ok((f32::from_le_bytes(*max), f32::from_le_bytes(min)))
        }
        _ => Err(corrupt("a dimension's bounds are malformed")),
    }
}

/// The value of a document's count.
pub fn count_value(count: u32) -> Vec<u8> {
    count.to_le_bytes().to_vec()
}

/// The count a document's value holds.
///
/// # Errors
///
/// SQLCODE -916 if it is not one.
pub fn count_of(value: &[u8]) -> Result<u32> {
    let bytes = <[u8; 4]>::try_from(value).map_err(|_| corrupt("a document's count is malformed"))?;
    Ok(u32::from_le_bytes(bytes))
}

/// Every key of tag `tag`, from the first to below the next tag's.
pub fn tag_range(tag: u8) -> (Vec<u8>, Vec<u8>) {
    (vec![tag], vec![tag.saturating_add(1)])
}

/// The least key past every key starting with `key`: itself with a byte
/// added, as no key extends a full-length one.
fn past(key: &[u8]) -> Vec<u8> {
    let mut out = key.to_vec();
    out.push(0);
    out
}

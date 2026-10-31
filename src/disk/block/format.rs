//! \[DESIGN\] The format of a block file (RULES.md D30), the same on every
//! platform: every field little-endian, at a fixed place.
//!
//! ```text
//! header (HEADER_LEN bytes):
//!   magic "TBBLOCKS" | version u16 | reserved u16 (0) | hash id u32 |
//!   encodings u32 | MAX_ENCODINGS encoding ids u32 (unused ones 0) |
//!   digest of the bytes before it | zeros to HEADER_LEN
//! block, at its id (its byte offset):
//!   length u32 | digest of length and payload | payload | length u32
//! ```
//!
//! The digest's size is the hash function's. The payload is as the file's
//! encoding pipeline left it, and the digest covers it so; the length after
//! the payload lets a reader step back from the end of a block to its start.

use std::path::Path;
use std::sync::Arc;

use super::super::encoding::{self, EncodingId, Pipeline};
use crate::error::{Code, Result, SqlError};
use crate::hash::{HashFunction, HashId};

/// Bytes of the file header; the first block starts here. Room for the
/// fields and a digest of up to `MAX_DIGEST_BYTES`, 512 bits.
pub const HEADER_LEN: u64 = 128;
/// `HEADER_LEN` as a buffer size.
pub(super) const HEADER_BYTES: usize = 128;
/// Identifies a block file.
const MAGIC: [u8; 8] = *b"TBBLOCKS";
/// The format this code writes and reads.
pub const FORMAT_VERSION: u16 = 1;
/// Where the header's fields start.
const VERSION_AT: usize = 8;
const HASH_AT: usize = 12;
const ENCODINGS_AT: usize = 16;
const DIGEST_AT: usize = ENCODINGS_AT + encoding::ENCODINGS_FIELD;
// The header holds the widest digest, SHA-512's (RULES.md D26), before the
// first block.
const _: () = assert!(DIGEST_AT + crate::hash::MAX_DIGEST_BYTES <= HEADER_BYTES);
/// Bytes of a block's length field, before and after its payload.
pub(super) const LEN_BYTES: usize = 4;

/// The `N` bytes of `bytes` at `at`, if there are that many.
fn field<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..at.checked_add(N)?).and_then(|b| <[u8; N]>::try_from(b).ok())
}

/// The little-endian `u32` at `at`.
pub(super) fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    field(bytes, at).map(u32::from_le_bytes)
}

/// The header of a new file checksummed with `hash`, its blocks encoded by
/// `pipeline`.
pub(super) fn header(hash: &dyn HashFunction, pipeline: &Pipeline) -> Vec<u8> {
    let mut h = Vec::with_capacity(HEADER_BYTES);
    h.extend_from_slice(&MAGIC);
    h.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    h.extend_from_slice(&[0; 2]);
    h.extend_from_slice(&hash.id().0.to_le_bytes());
    h.extend_from_slice(&encoding::ids_field(pipeline));
    let digest = hash.hash(&h);
    h.extend_from_slice(digest.as_bytes());
    h.resize(HEADER_BYTES, 0);
    h
}

/// The hash function a header names and the ids of its encodings, after
/// checking the header.
///
/// # Errors
///
/// SQLCODE -916 if it is not a block file header of this format, or its
/// digest is wrong; SQLCODE -910 if no function in `hashes` has its id.
pub(super) fn read_header(bytes: &[u8], hashes: &[Arc<dyn HashFunction>], path: &Path) -> Result<(Arc<dyn HashFunction>, Vec<EncodingId>)> {
    let corrupt = |why: &str| SqlError::new(Code::Corrupt, format!("[DESIGN] {} is not a block file: {why}", path.display()));
    if field::<8>(bytes, 0) != Some(MAGIC) {
        return Err(corrupt("its magic number is wrong"));
    }
    let version = field(bytes, VERSION_AT).map(u16::from_le_bytes);
    if version != Some(FORMAT_VERSION) {
        return Err(corrupt(&format!("its format version is {version:?}, and only {FORMAT_VERSION} is known")));
    }
    let id = HashId(le_u32(bytes, HASH_AT).unwrap_or_default());
    let hash = hashes.iter().find(|h| h.id() == id).ok_or_else(|| {
        SqlError::new(
            Code::Api,
            format!("[DESIGN] {} is checksummed with hash function {}, which is not given", path.display(), crate::hash::described(id)),
        )
    })?;
    let stored = bytes.get(DIGEST_AT..DIGEST_AT.saturating_add(hash.digest_len()));
    if stored != Some(hash.hash(bytes.get(..DIGEST_AT).unwrap_or_default()).as_bytes()) {
        return Err(corrupt("its header fails its checksum"));
    }
    let ids = encoding::read_ids_field(bytes.get(ENCODINGS_AT..DIGEST_AT).unwrap_or_default())
        .ok_or_else(|| corrupt("its list of encodings is malformed"))?;
    Ok((Arc::clone(hash), ids))
}

/// The bytes of a block holding `payload`, checksummed with `hash`.
///
/// # Errors
///
/// SQLCODE -910 if the payload is longer than `max`.
pub(super) fn frame(hash: &dyn HashFunction, payload: &[u8], max: u32) -> Result<Vec<u8>> {
    let len = u32::try_from(payload.len()).ok().filter(|&n| n <= max).ok_or_else(|| {
        SqlError::new(Code::Api, format!("[DESIGN] a block of {} bytes is longer than the limit of {max}", payload.len()))
    })?;
    let len_bytes = len.to_le_bytes();
    let digest = hash.hash_parts(&[&len_bytes, payload]);
    let mut out = Vec::with_capacity(frame_len(payload.len(), hash.digest_len()));
    out.extend_from_slice(&len_bytes);
    out.extend_from_slice(digest.as_bytes());
    out.extend_from_slice(payload);
    out.extend_from_slice(&len_bytes);
    Ok(out)
}

/// The bytes of a block whose payload has `len` bytes.
pub(super) fn frame_len(len: usize, digest_len: usize) -> usize {
    LEN_BYTES.saturating_mul(2).saturating_add(digest_len).saturating_add(len)
}

/// Whether `rest`, the bytes after a block's prefix (`len` and `digest`),
/// holds a payload of `len` bytes that matches the digest and is followed
/// by the same length.
pub(super) fn verify(hash: &dyn HashFunction, len: u32, digest: &[u8], rest: &[u8]) -> bool {
    let payload_len = usize::try_from(len).unwrap_or(usize::MAX);
    let Some(payload) = rest.get(..payload_len) else { return false };
    le_u32(rest, payload_len) == Some(len) && hash.hash_parts(&[&len.to_le_bytes(), payload]).as_bytes() == digest
}

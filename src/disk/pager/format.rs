//! \[DESIGN\] The format of a pager file (RULES.md D30), the same on every
//! platform: every field little-endian, at a fixed place.
//!
//! ```text
//! header slot (the first page-sized slot):
//!   at 0:    magic "TBPAGES\0" | version u16 | reserved u16 | hash id u32 |
//!            page size u32 | reserved u32 | encodings u32 |
//!            MAX_ENCODINGS encoding ids u32 (unused ones 0) |
//!            digest of the bytes before it
//!   at 1024 and 2048, two meta slots, written in turn by `sync`:
//!            sequence u64 | pages u64 | first trunk page u64 | digest
//! page n, at offset (n + 1) * page size:
//!   body | page number u64 | digest of body and page number
//! bodies, by kind (byte 0):
//!   data:      kind | 3 reserved | extents u32 | content length u64 |
//!              first directory u64 | (start u64, count u64)* | content
//!   overflow:  kind | 7 reserved | content
//!   directory, trunk:
//!              kind | 3 reserved | entries u32 | next page u64 | (start u64, count u64)*
//! ```
//!
//! A data page's content is as the file's encoding pipeline left it; a
//! scratch file's begins with a byte naming which of the pipelines it was
//! written through did (`live.rs`). A data
//! page lists the first `INLINE_EXTENTS` extents of its overflow
//! itself, so most overflowing content needs no directory page; directories
//! list the rest. A trunk lists free runs. `NONE` stands for no page. A body of all zeros, never
//! written, is a new data page with no content.

use std::path::Path;
use std::sync::Arc;

use super::super::encoding::{self, EncodingId, Pipeline};
use super::{Extent, PageId};
use crate::error::{Code, Result, SqlError};
use crate::hash::{HashFunction, HashId, MAX_DIGEST_BYTES};

/// Identifies a pager file.
const MAGIC: [u8; 8] = *b"TBPAGES\0";
/// The format this code writes and reads.
pub const FORMAT_VERSION: u16 = 1;
/// Where the header's fields start.
const VERSION_AT: usize = 8;
const HASH_AT: usize = 12;
const PAGE_SIZE_AT: usize = 16;
const ENCODINGS_AT: usize = 24;
const DIGEST_AT: usize = ENCODINGS_AT + encoding::ENCODINGS_FIELD;
/// Where the two meta slots start: in separate disk sectors, so a torn
/// write of one leaves the other whole.
pub(super) const META_AT: [usize; 2] = [1024, 2048];
/// Bytes of a meta slot's fields, before its digest.
const META_FIELDS: usize = 24;
// The header and each meta slot hold the widest digest, SHA-512's
// (RULES.md D26), each before the next; the last within the header slot
// of the smallest page, as a unit test checks.
const _: () = assert!(DIGEST_AT + MAX_DIGEST_BYTES <= META_AT[0]);
const _: () = assert!(META_AT[0] + META_FIELDS + MAX_DIGEST_BYTES <= META_AT[1]);
/// Bytes of the page number in a page's trailer.
pub(super) const PAGE_NUMBER_BYTES: usize = 8;
/// No page.
pub(super) const NONE: u64 = u64::MAX;

/// The kinds of page body.
pub(super) const KIND_NEW: u8 = 0;
pub(super) const KIND_DATA: u8 = 1;
pub(super) const KIND_OVERFLOW: u8 = 2;
pub(super) const KIND_DIRECTORY: u8 = 3;
pub(super) const KIND_TRUNK: u8 = 4;
/// Bytes before the content of a data page, of an overflow page, and
/// before the entries of a directory or trunk page.
pub(super) const DATA_HEADER: usize = 24;
pub(super) const OVERFLOW_HEADER: usize = 8;
pub(super) const LIST_HEADER: usize = 16;
/// Bytes of a directory or trunk entry.
pub(super) const ENTRY: usize = 16;
/// \[DESIGN\] Overflow extents a data page lists itself: enough for content
/// allocated in a few runs, without spending a directory page on it.
pub(super) const INLINE_EXTENTS: usize = 4;
/// Where fields of a body start.
const LENGTH_AT: usize = 8;
const FIRST_DIRECTORY_AT: usize = 16;
const ENTRIES_AT: usize = 4;
const NEXT_AT: usize = 8;

/// The `N` bytes of `bytes` at `at`, if there are that many.
pub(super) fn field<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..at.checked_add(N)?).and_then(|b| <[u8; N]>::try_from(b).ok())
}

/// The little-endian `u64` at `at`, if there is one.
pub(super) fn le_u64(bytes: &[u8], at: usize) -> Option<u64> {
    field(bytes, at).map(u64::from_le_bytes)
}

/// The header of a new file whose content `pipeline` encodes, with an
/// empty meta slot 0: a header slot of `size` bytes.
pub(super) fn header(hash: &dyn HashFunction, pipeline: &Pipeline, page_size: u32, size: usize) -> Vec<u8> {
    let mut h = Vec::with_capacity(size);
    h.extend_from_slice(&MAGIC);
    h.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    h.extend_from_slice(&[0; 2]);
    h.extend_from_slice(&hash.id().0.to_le_bytes());
    h.extend_from_slice(&page_size.to_le_bytes());
    h.extend_from_slice(&[0; 4]);
    h.extend_from_slice(&encoding::ids_field(pipeline));
    let digest = hash.hash(&h);
    h.extend_from_slice(digest.as_bytes());
    h.resize(size, 0);
    let [first, _] = META_AT;
    let meta = encode_meta(hash, Meta { sequence: 0, pages: 0, trunk: NONE });
    if let Some(slot) = h.get_mut(first..first.saturating_add(meta.len())) {
        slot.copy_from_slice(&meta);
    }
    h
}

/// What a header slot records, after checking it.
pub(super) struct Header {
    pub(super) hash: Arc<dyn HashFunction>,
    pub(super) page_size: u32,
    pub(super) encodings: Vec<EncodingId>,
}

/// The header a header slot holds, after checking it.
pub(super) fn read_header(bytes: &[u8], hashes: &[Arc<dyn HashFunction>], path: &Path) -> Result<Header> {
    let corrupt = |why: &str| SqlError::new(Code::Corrupt, format!("[DESIGN] {} is not a pager file: {why}", path.display()));
    if field::<8>(bytes, 0) != Some(MAGIC) {
        return Err(corrupt("its magic number is wrong"));
    }
    let version = field(bytes, VERSION_AT).map(u16::from_le_bytes);
    if version != Some(FORMAT_VERSION) {
        return Err(corrupt(&format!("its format version is {version:?}, and only {FORMAT_VERSION} is known")));
    }
    let id = HashId(field(bytes, HASH_AT).map(u32::from_le_bytes).unwrap_or_default());
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
    let encodings = encoding::read_ids_field(bytes.get(ENCODINGS_AT..DIGEST_AT).unwrap_or_default())
        .ok_or_else(|| corrupt("its list of encodings is malformed"))?;
    Ok(Header { hash: Arc::clone(hash), page_size: field(bytes, PAGE_SIZE_AT).map(u32::from_le_bytes).unwrap_or_default(), encodings })
}

/// What a meta slot records: the state of the last `sync`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Meta {
    /// Counts the syncs; the slot with the higher one is current.
    pub(super) sequence: u64,
    /// Pages in the file.
    pub(super) pages: u64,
    /// The first trunk page of the free list, or `NONE`.
    pub(super) trunk: u64,
}

/// A meta slot's bytes.
pub(super) fn encode_meta(hash: &dyn HashFunction, meta: Meta) -> Vec<u8> {
    let mut m = Vec::with_capacity(META_FIELDS.saturating_add(hash.digest_len()));
    m.extend_from_slice(&meta.sequence.to_le_bytes());
    m.extend_from_slice(&meta.pages.to_le_bytes());
    m.extend_from_slice(&meta.trunk.to_le_bytes());
    let digest = hash.hash(&m);
    m.extend_from_slice(digest.as_bytes());
    m
}

/// The meta slot at `at` of a header slot, if it is whole.
pub(super) fn decode_meta(hash: &dyn HashFunction, header: &[u8], at: usize) -> Option<Meta> {
    let fields = header.get(at..at.checked_add(META_FIELDS)?)?;
    let digest = header.get(at.checked_add(META_FIELDS)?..at.checked_add(META_FIELDS)?.checked_add(hash.digest_len())?)?;
    if hash.hash(fields).as_bytes() != digest {
        return None;
    }
    Some(Meta {
        sequence: le_u64(fields, 0)?,
        pages: le_u64(fields, PAGE_NUMBER_BYTES)?,
        trunk: le_u64(fields, META_FIELDS.checked_sub(8)?)?,
    })
}

/// The body of a data page holding `inline` of `length` content bytes, the
/// rest in overflow pages listed by `extents` (at most `INLINE_EXTENTS`) and
/// then by the directory at `directory`.
pub(super) fn data_body(length: u64, extents: &[Extent], directory: u64, inline: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(DATA_HEADER.saturating_add(extents.len().saturating_mul(ENTRY)).saturating_add(inline.len()));
    b.push(KIND_DATA);
    b.extend_from_slice(&[0; 3]);
    b.extend_from_slice(&u32::try_from(extents.len()).unwrap_or(u32::MAX).to_le_bytes());
    b.extend_from_slice(&length.to_le_bytes());
    b.extend_from_slice(&directory.to_le_bytes());
    push_entries(&mut b, extents);
    b.extend_from_slice(inline);
    b
}

/// Appends `entries` to `b`.
fn push_entries(b: &mut Vec<u8>, entries: &[Extent]) {
    for e in entries {
        b.extend_from_slice(&e.start.0.to_le_bytes());
        b.extend_from_slice(&e.count.to_le_bytes());
    }
}

/// A data page's content length, its overflow extents and first directory,
/// and where its content starts, from its body.
pub(super) fn data_header(body: &[u8]) -> Option<(u64, Vec<Extent>, u64, usize)> {
    let count = usize::try_from(field(body, ENTRIES_AT).map(u32::from_le_bytes)?).ok().filter(|&n| n <= INLINE_EXTENTS)?;
    let length = le_u64(body, LENGTH_AT)?;
    let directory = le_u64(body, FIRST_DIRECTORY_AT)?;
    let entries = entries_at(body, DATA_HEADER, count)?;
    Some((length, entries, directory, DATA_HEADER.checked_add(count.checked_mul(ENTRY)?)?))
}

/// `count` entries of `body` from `at`.
fn entries_at(body: &[u8], at: usize, count: usize) -> Option<Vec<Extent>> {
    let mut entries = Vec::with_capacity(count.min(body.len()));
    for n in 0..count {
        let e = at.checked_add(n.checked_mul(ENTRY)?)?;
        entries.push(Extent { start: PageId(le_u64(body, e)?), count: le_u64(body, e.checked_add(PAGE_NUMBER_BYTES)?)? });
    }
    Some(entries)
}

/// The body of an overflow page holding `content`.
pub(super) fn overflow_body(content: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(OVERFLOW_HEADER.saturating_add(content.len()));
    b.push(KIND_OVERFLOW);
    b.extend_from_slice(&[0; 7]);
    b.extend_from_slice(content);
    b
}

/// The body of a directory or trunk page (`kind`) listing `entries`, then
/// continued at `next`.
pub(super) fn list_body(kind: u8, entries: &[Extent], next: u64) -> Vec<u8> {
    let mut b = Vec::with_capacity(LIST_HEADER.saturating_add(entries.len().saturating_mul(ENTRY)));
    b.push(kind);
    b.extend_from_slice(&[0; 3]);
    b.extend_from_slice(&u32::try_from(entries.len()).unwrap_or(u32::MAX).to_le_bytes());
    b.extend_from_slice(&next.to_le_bytes());
    push_entries(&mut b, entries);
    b
}

/// The entries of a directory or trunk body, and the page it continues at.
pub(super) fn list_entries(body: &[u8]) -> Option<(Vec<Extent>, u64)> {
    let count = usize::try_from(field(body, ENTRIES_AT).map(u32::from_le_bytes)?).ok()?;
    Some((entries_at(body, LIST_HEADER, count)?, le_u64(body, NEXT_AT)?))
}

// Unit tests: the header slot has room for the widest digests (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::pager::MIN_PAGE_SIZE;

    /// Verifies: [DESIGN] RULES.md D26, D30: the second meta slot, its
    /// digest of `MAX_DIGEST_BYTES` (SHA-512's 64 bytes) included, ends
    /// within the header slot of the smallest page.
    #[test]
    fn the_header_slot_holds_the_widest_digests() {
        let [_, second] = META_AT;
        assert!(second + META_FIELDS + MAX_DIGEST_BYTES <= usize::try_from(MIN_PAGE_SIZE).unwrap());
    }
}

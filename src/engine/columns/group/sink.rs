//! \[DESIGN\] A group written as an extent of a segment (RULES.md D1146):
//! a load's group is framed in memory as it is written, each page a block
//! of its own, its number its offset within the extent, after a head block
//! at offset 0, so no page of it is page 0, which a chain reads as none.
//! The extent is appended to its segment as it is and logged as it is, so
//! where it lands does not change its bytes: a page is read at the
//! extent's place in its segment plus its number.
//!
//! ```text
//! extent:  head block | each page, sealed, a block
//! head:    first id u64 | last id u64 | rows u32 | pages u32 |
//!          header u64 (the first page of the group's header) | length
//!          u64 (the extent's bytes) | XXH3-64 of the bytes after the head
//!          block u64
//! ```
//!
//! Every block carries its own digest, as a block file's does; the head
//! names the extent's length and the digest of all of it, so a sweep after
//! recovery walks a segment an extent at a time and finds a torn one.

use std::sync::{Arc, Mutex};

use super::corrupt;
use crate::bytes::{Put, Reader};
use crate::disk::block::{frame_onto, framed_len};
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::hash::HashFunction;
use crate::hash::xxhash::xxh3_64;
use crate::storage::RowId;
use crate::sync::lock_builtin;

/// Bytes of a head block's payload.
const HEAD: usize = 48;

/// What an extent's head says of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Head {
    /// The group's least row id.
    pub first: RowId,
    /// Its greatest.
    pub last: RowId,
    /// Its rows.
    pub rows: u32,
    /// Its pages.
    pub pages: u32,
    /// The first page of its header.
    pub header: u64,
    /// The extent's bytes, its head's among them.
    pub length: u64,
}

/// \[DESIGN\] A group's extent as it is written: its blocks framed end to
/// end in one buffer, its head's place kept for it.
pub struct Sink {
    hash: Arc<dyn HashFunction>,
    frames: Mutex<(Vec<u8>, u32)>,
}

impl Sink {
    /// An extent of no page yet, checksummed with `hash`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if `hash` cannot frame a block.
    pub fn new(hash: Arc<dyn HashFunction>) -> Result<Sink> {
        let mut frames = Vec::new();
        frame_onto(&*hash, &[0; HEAD], &mut frames)?;
        Ok(Sink { hash, frames: Mutex::new((frames, 0)) })
    }

    /// The number the next page put takes: its place in the extent.
    pub fn next(&self) -> u64 {
        u64::try_from(lock_builtin(&self.frames).0.len()).unwrap_or(u64::MAX)
    }

    /// Adds `sealed`, a page's bytes as stored: its number, as `next` gave
    /// it before.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the page is longer than a block.
    pub fn put(&self, sealed: &[u8]) -> Result<PageId> {
        let mut held = lock_builtin(&self.frames);
        let (frames, pages) = &mut *held;
        let at = u64::try_from(frames.len()).unwrap_or(u64::MAX);
        frame_onto(&*self.hash, sealed, frames)?;
        *pages = pages.saturating_add(1);
        Ok(PageId(at))
    }

    /// The extent, its head written: of the rows `first` to `last`, `rows`
    /// of them, its header beginning at page `header`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the head cannot be framed.
    pub fn finish(self, (first, last, rows): (RowId, RowId, u32), header: PageId) -> Result<Vec<u8>> {
        let (mut frames, pages) = self.frames.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
        let head_len = framed_len(&*self.hash, HEAD);
        let digest = xxh3_64(frames.get(head_len..).unwrap_or_default());
        let mut payload = Vec::with_capacity(HEAD);
        payload.put_u64(first);
        payload.put_u64(last);
        payload.put_u32(rows);
        payload.put_u32(pages);
        payload.put_u64(header.0);
        payload.put_u64(u64::try_from(frames.len()).unwrap_or(u64::MAX));
        payload.put_u64(digest);
        let mut head = Vec::with_capacity(head_len);
        frame_onto(&*self.hash, &payload, &mut head)?;
        frames.get_mut(..head_len).ok_or_else(|| corrupt("an extent is shorter than its head"))?.copy_from_slice(&head);
        Ok(frames)
    }
}

/// The head of `extent`, a whole extent checksummed with `hash`, if its
/// length and digest are its own.
///
/// # Errors
///
/// SQLCODE -916 if the extent is torn or not what its head says.
pub fn head_of(hash: &dyn HashFunction, extent: &[u8]) -> Result<Head> {
    let head_len = framed_len(hash, HEAD);
    let prefix = head_len.saturating_sub(HEAD).saturating_sub(4);
    let payload = extent.get(prefix..prefix.saturating_add(HEAD)).ok_or_else(|| corrupt("an extent is shorter than its head"))?;
    let mut framed = Vec::with_capacity(head_len);
    frame_onto(hash, payload, &mut framed)?;
    if extent.get(..head_len) != Some(&framed[..]) {
        return Err(corrupt("an extent's head fails its checksum"));
    }
    let mut r = Reader::new(payload, "an extent's head");
    let head = Head { first: r.u64()?, last: r.u64()?, rows: r.u32()?, pages: r.u32()?, header: r.u64()?, length: r.u64()? };
    let digest = r.u64()?;
    let whole = usize::try_from(head.length).ok().filter(|&n| n == extent.len() && n >= head_len);
    let rest = whole.and_then(|_| extent.get(head_len..)).ok_or_else(|| corrupt("an extent is not as long as its head says"))?;
    if xxh3_64(rest) != digest {
        return Err(corrupt("an extent fails its digest"));
    }
    Ok(head)
}

#[cfg(test)]
mod tests;

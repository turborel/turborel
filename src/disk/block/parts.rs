//! \[DESIGN\] Blocks framed by the caller, and a block of parts (RULES.md
//! D1146). A load's group is framed as a run of blocks in one buffer as it
//! is written, each page a block, then appended to its segment as it is
//! (`append_frames`), and the same buffer is the payload of its log record
//! (`append_parts`), so the group's bytes are never copied to be written.
//! A scan's read-ahead takes a run of blocks near one another with one
//! read (`read_near`).

use std::borrow::Cow;
use std::sync::atomic::Ordering;

use super::counts::add;
use super::{BlockFile, BlockId, DEFAULT_MAX_BLOCK, format};
use crate::error::{Code, Result, SqlError};
use crate::hash::HashFunction;

/// \[DESIGN\] The most bytes `read_near` takes in one read: a megabyte, the
/// most a transaction's log run takes at once (`txn/pending.rs`), so a
/// read ahead is one large request and its buffer stays small.
const NEAR_BYTES: u64 = 1024 * 1024;

/// Frames `payload` as a block checksummed with `hash` onto the end of
/// `out`, as a file of no pipeline frames one.
///
/// # Errors
///
/// SQLCODE -910 if the payload is longer than `DEFAULT_MAX_BLOCK`; `out`
/// is then unchanged.
pub fn frame_onto(hash: &dyn HashFunction, payload: &[u8], out: &mut Vec<u8>) -> Result<()> {
    format::frame_into(hash, payload, DEFAULT_MAX_BLOCK, out)
}

/// The bytes a block of `len` bytes takes, checksummed with `hash`.
pub fn framed_len(hash: &dyn HashFunction, len: usize) -> usize {
    format::frame_len(len, hash.digest_len())
}

impl BlockFile {
    /// The error for blocks given that this file cannot take as they are.
    fn not_taken(&self, why: &str) -> SqlError {
        SqlError::new(Code::Api, format!("[DESIGN] {} takes no such blocks: {why}", self.path.display()))
    }

    /// Writes `parts`, end to end, at `start..end`, reserved, and publishes
    /// them.
    fn write_reserved(&self, start: u64, end: u64, parts: &[&[u8]]) -> Result<()> {
        let mut written = self.fill_ahead(end);
        let mut at = start;
        for part in parts {
            if written.is_err() {
                break;
            }
            written = self.file.write_all_at(part, at);
            at = at.saturating_add(u64::try_from(part.len()).unwrap_or(u64::MAX));
        }
        self.publish(start, end, written)
    }

    /// Appends `frames`, a run of whole blocks framed by `frame_onto` with
    /// this file's hash function, as they are, written at once: the id of
    /// the first. Each block keeps its offset within the run past it.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the file has a pipeline, or the run does not end
    /// where its last block does; SQLCODE -915 if the write fails, or an
    /// earlier one did.
    pub fn append_frames(&self, frames: &[u8]) -> Result<BlockId> {
        if !self.pipeline.is_empty() {
            return Err(self.not_taken("it encodes its blocks"));
        }
        let digest = self.hash.digest_len();
        let mut at = 0_usize;
        while at < frames.len() {
            let len = format::le_u32(frames, at).ok_or_else(|| self.not_taken("a block's length is cut"))?;
            at = at.saturating_add(format::frame_len(usize::try_from(len).unwrap_or(usize::MAX), digest));
        }
        if at != frames.len() {
            return Err(self.not_taken("the run does not end where a block does"));
        }
        if self.failed.load(Ordering::SeqCst) {
            return Err(self.broken());
        }
        let start = self.reserve(frames.len())?;
        let end = start.saturating_add(u64::try_from(frames.len()).unwrap_or(u64::MAX));
        self.write_reserved(start, end, &[frames])?;
        add(&self.counters.appends, 1);
        add(&self.counters.appended_bytes, frames.len());
        add(&self.counters.stored_bytes, frames.len());
        Ok(BlockId(start))
    }

    /// Appends one block whose payload is `parts` end to end, as `append`
    /// appends one, with no copy of them made where the file has no
    /// pipeline: its id.
    ///
    /// # Errors
    ///
    /// As `append`.
    pub fn append_parts(&self, parts: &[&[u8]]) -> Result<BlockId> {
        if !self.pipeline.is_empty() {
            return self.append(&parts.concat());
        }
        let bytes = parts.iter().fold(0_usize, |n, p| n.saturating_add(p.len()));
        let len = u32::try_from(bytes).ok().filter(|&n| n <= self.options.max_block).ok_or_else(|| {
            SqlError::new(Code::Api, format!("[DESIGN] a block of {bytes} bytes is longer than the limit of {}", self.options.max_block))
        })?;
        let len_bytes = len.to_le_bytes();
        let mut hashed: Vec<&[u8]> = Vec::with_capacity(parts.len().saturating_add(1));
        hashed.push(&len_bytes);
        hashed.extend_from_slice(parts);
        let digest = self.hash.hash_parts(&hashed);
        let mut head = len_bytes.to_vec();
        head.extend_from_slice(digest.as_bytes());
        if self.failed.load(Ordering::SeqCst) {
            return Err(self.broken());
        }
        let frame = format::frame_len(bytes, self.hash.digest_len());
        let start = self.reserve(frame)?;
        let end = start.saturating_add(u64::try_from(frame).unwrap_or(u64::MAX));
        let mut all: Vec<&[u8]> = Vec::with_capacity(parts.len().saturating_add(2));
        all.push(&head);
        all.extend_from_slice(parts);
        all.push(&len_bytes);
        self.write_reserved(start, end, &all)?;
        add(&self.counters.appends, 1);
        add(&self.counters.appended_bytes, bytes);
        add(&self.counters.stored_bytes, bytes);
        Ok(BlockId(start))
    }

    /// The payloads of the blocks at `ids`, in ascending order, those near
    /// one another taken with one read of up to `NEAR_BYTES`, each checked
    /// and decoded; none for one that cannot be read or is not valid, and
    /// for those after it in its read.
    pub fn read_near(&self, ids: &[u64]) -> Vec<Option<Vec<u8>>> {
        let end = self.end.load(Ordering::Acquire);
        let mut out = Vec::with_capacity(ids.len());
        let mut rest = ids;
        while let Some(&first) = rest.first() {
            let reach = first.saturating_add(NEAR_BYTES).min(end);
            let taken = rest.iter().take_while(|&&id| id < reach).count().max(1);
            let (now, later) = rest.split_at(taken.min(rest.len()));
            self.read_span(first, reach, now, &mut out);
            rest = later;
        }
        out
    }

    /// Reads `first..reach` once, and the payload of each of `ids` within it,
    /// onto `out`; one past what was read is read alone.
    fn read_span(&self, first: u64, reach: u64, ids: &[u64], out: &mut Vec<Option<Vec<u8>>>) {
        let mut span = vec![0_u8; usize::try_from(reach.saturating_sub(first)).unwrap_or_default()];
        let read = self.file.read_exact_at(&mut span, first).is_ok();
        for &id in ids {
            let within = usize::try_from(id.saturating_sub(first)).unwrap_or(usize::MAX);
            let found = if read { self.in_span(&span, within) } else { None };
            let found = found.or_else(|| self.read(BlockId(id)).ok());
            out.push(found);
        }
    }

    /// The payload of the block at `within` of `span`, if it is whole there
    /// and valid, decoded.
    fn in_span(&self, span: &[u8], within: usize) -> Option<Vec<u8>> {
        let digest_len = self.hash.digest_len();
        let prefix = format::LEN_BYTES.saturating_add(digest_len);
        let len = format::le_u32(span, within)?;
        let payload_len = usize::try_from(len).ok()?;
        let frame = span.get(within..within.checked_add(format::frame_len(payload_len, digest_len))?)?;
        let digest = frame.get(format::LEN_BYTES..prefix)?;
        if !format::verify(&*self.hash, len, digest, frame.get(prefix..)?) {
            return None;
        }
        let payload = frame.get(prefix..prefix.checked_add(payload_len)?)?;
        add(&self.counters.reads, 1);
        add(&self.counters.read_bytes, payload_len);
        match self.pipeline.decode(0, payload).ok()? {
            Cow::Borrowed(b) => Some(b.to_vec()),
            Cow::Owned(decoded) => Some(decoded),
        }
    }
}

#[cfg(test)]
mod tests;

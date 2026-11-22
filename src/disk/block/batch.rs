//! \[DESIGN\] Blocks appended together (RULES.md D25, D30): each framed
//! and checksummed as `append` frames one, all of them in one buffer,
//! reserved with one atomic step, written with one positional write and
//! published once. A transaction's records go so at its commit, one write
//! where each record took its own (`txn/logging.rs`). Each block keeps
//! its own id, so a reader meets the same blocks either way; a crash
//! tears the run as it tears any write, leaving a prefix of it valid, or
//! the zeros an unwritten sector shows (`tail.rs`).

use std::sync::atomic::Ordering;

use super::counts::add;
use super::{BlockFile, BlockId, format};
use crate::error::Result;

impl BlockFile {
    /// Appends a block for each of `payloads`, in order, as `append` does
    /// one, written at once: their ids, in the same order.
    ///
    /// # Errors
    ///
    /// As `append`: SQLCODE -910 if a payload is longer than `max_block`,
    /// nothing then written; SQLCODE -915 if the write fails, or an earlier
    /// one did.
    pub fn append_all(&self, payloads: &[Vec<u8>]) -> Result<Vec<BlockId>> {
        if payloads.is_empty() {
            return Ok(Vec::new());
        }
        // Room for every frame at once, as the log's group commits write
        // many: payloads stored as given take their own bytes and a frame's.
        let digest = self.hash.digest_len();
        let room = payloads.iter().fold(0_usize, |sum, p| sum.saturating_add(format::frame_len(p.len(), digest)));
        let mut frames = Vec::with_capacity(room);
        let mut starts = Vec::with_capacity(payloads.len());
        let mut stored_bytes = 0_usize;
        for payload in payloads {
            // Block ids are known only once written, so the context is 0
            // (`Encoding`), as `append` encodes.
            let stored = self.pipeline.encode(0, payload)?;
            starts.push(u64::try_from(frames.len()).unwrap_or(u64::MAX));
            format::frame_into(&*self.hash, &stored, self.options.max_block, &mut frames)?;
            stored_bytes = stored_bytes.saturating_add(stored.len());
        }
        if self.failed.load(Ordering::SeqCst) {
            return Err(self.broken());
        }
        let start = self.reserve(frames.len())?;
        let end = start.saturating_add(u64::try_from(frames.len()).unwrap_or(u64::MAX));
        let written = self.fill_ahead(end).and_then(|()| self.file.write_all_at(&frames, start));
        self.publish(start, end, written)?;
        add(&self.counters.appends, payloads.len());
        add(&self.counters.appended_bytes, payloads.iter().map(Vec::len).sum());
        add(&self.counters.stored_bytes, stored_bytes);
        Ok(starts.into_iter().map(|at| BlockId(start.saturating_add(at))).collect())
    }
}

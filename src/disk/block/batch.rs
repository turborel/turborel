//! \[DESIGN\] Blocks appended together (RULES.md D25, D30): each framed
//! and checksummed as `append` frames one, all of them in one buffer,
//! reserved with one atomic step, written with one positional write and
//! published once. A transaction's records go so at its commit, one write
//! where each record took its own (`txn/logging.rs`). Each block keeps
//! its own id, so a reader meets the same blocks either way; a crash
//! tears the run as it tears any write, leaving a prefix of it valid, or
//! the zeros an unwritten sector shows (`tail.rs`).

use std::cell::RefCell;
use std::sync::atomic::Ordering;

use super::counts::add;
use super::{BlockFile, BlockId, format};
use crate::error::Result;

thread_local! {
    /// The buffer the last run of blocks appended on this thread was
    /// framed in, kept for the next, which clears it: a commit's run then
    /// allocates none for its frames.
    static FRAMES: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// \[DESIGN\] The most bytes of a buffer of frames a thread keeps for its
/// next run: a megabyte, the most a transaction holds before it writes
/// (`txn/pending.rs`), so a run past it gives its buffer back.
const KEPT_FRAMES: usize = 1024 * 1024;

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
        self.append_each(payloads.iter().map(Vec::as_slice))
    }

    /// As `append_all`, of payloads lent one after another: those of a
    /// run held end to end in one buffer (`Log::append_run`).
    ///
    /// # Errors
    ///
    /// As `append_all`.
    pub fn append_each<'p>(&self, payloads: impl ExactSizeIterator<Item = &'p [u8]> + Clone) -> Result<Vec<BlockId>> {
        if payloads.len() == 0 {
            return Ok(Vec::new());
        }
        let mut frames = FRAMES.take();
        frames.clear();
        let appended = self.append_framed(payloads, &mut frames);
        if frames.capacity() <= KEPT_FRAMES {
            FRAMES.set(frames);
        }
        appended
    }

    /// `append_each`, its frames made in `frames`, empty.
    fn append_framed<'p>(&self, payloads: impl ExactSizeIterator<Item = &'p [u8]> + Clone, frames: &mut Vec<u8>) -> Result<Vec<BlockId>> {
        // Room for every frame at once, as the log's group commits write
        // many: payloads stored as given take their own bytes and a frame's.
        let digest = self.hash.digest_len();
        let (count, bytes) = (payloads.len(), payloads.clone().fold(0_usize, |sum, p| sum.saturating_add(p.len())));
        frames.reserve(bytes.saturating_add(count.saturating_mul(format::frame_len(0, digest))));
        let mut starts = Vec::with_capacity(count);
        let mut stored_bytes = 0_usize;
        for payload in payloads {
            // Block ids are known only once written, so the context is 0
            // (`Encoding`), as `append` encodes.
            let stored = self.pipeline.encode(0, payload)?;
            starts.push(u64::try_from(frames.len()).unwrap_or(u64::MAX));
            format::frame_into(&*self.hash, &stored, self.options.max_block, frames)?;
            stored_bytes = stored_bytes.saturating_add(stored.len());
        }
        if self.failed.load(Ordering::SeqCst) {
            return Err(self.broken());
        }
        let start = self.reserve(frames.len())?;
        let end = start.saturating_add(u64::try_from(frames.len()).unwrap_or(u64::MAX));
        let written = self.fill_ahead(end).and_then(|()| self.file.write_all_at(frames, start));
        self.publish(start, end, written)?;
        add(&self.counters.appends, count);
        add(&self.counters.appended_bytes, bytes);
        add(&self.counters.stored_bytes, stored_bytes);
        Ok(starts.into_iter().map(|at| BlockId(start.saturating_add(at))).collect())
    }
}

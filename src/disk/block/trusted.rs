//! \[DESIGN\] Blocks read and written over by a reader that knows them
//! (RULES.md D30, D471): a shadow file's side file, which knows where each
//! block is and how long, and whose content is checked where it is used,
//! as the file's own bytes are. Its blocks are still framed and checked
//! as any other, so a crash's torn block is found as the file is opened.

use std::sync::atomic::Ordering;

use super::{BlockFile, BlockId, format};
use crate::error::{Code, Result, SqlError};

impl BlockFile {
    /// Where block `id`'s payload starts in the file.
    fn payload_start(&self, id: BlockId) -> u64 {
        let prefix = format::LEN_BYTES.saturating_add(self.hash.digest_len());
        id.0.saturating_add(u64::try_from(prefix).unwrap_or(u64::MAX))
    }

    /// The error for a block a caller asked for wrongly.
    fn misused(&self, id: BlockId, why: &str) -> SqlError {
        SqlError::new(Code::Api, format!("[DESIGN] the block at offset {} of {} {why}", id.0, self.path.display()))
    }

    /// Fills `out` from block `id`'s payload, from byte `within` of it,
    /// with one read and no check of the block: for a file of no pipeline
    /// whose reader appended the block and knows its length, and checks
    /// what it reads where it uses it.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the file has a pipeline, or the bytes asked for lie
    /// past its appended blocks; SQLCODE -915 if the file cannot be read.
    pub fn read_payload_at(&self, id: BlockId, within: usize, out: &mut [u8]) -> Result<()> {
        let from = self.payload_start(id).saturating_add(u64::try_from(within).unwrap_or(u64::MAX));
        let to = from.saturating_add(u64::try_from(out.len()).unwrap_or(u64::MAX));
        if !self.pipeline.is_empty() || to > self.end.load(Ordering::Acquire) {
            return Err(self.misused(id, "is not one to read so"));
        }
        self.file.read_exact_at(out, from)
    }

    /// Writes `payload` over block `id`, whose payload its caller knows is
    /// `was` bytes long, as this one must be. A crash can tear it among
    /// valid blocks, which an append cannot, so this is for a file thrown
    /// away unless synced whole (`disk/shadow.rs`).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the file has a pipeline or the lengths differ;
    /// SQLCODE -915 if the block cannot be written, which leaves the file
    /// refusing appends, or an earlier write failed.
    pub fn overwrite(&self, id: BlockId, payload: &[u8], was: usize) -> Result<()> {
        if !self.pipeline.is_empty() || payload.len() != was {
            return Err(self.misused(id, "is of another length"));
        }
        let frame = format::frame(&*self.hash, payload, self.options.max_block)?;
        if self.failed.load(Ordering::SeqCst) {
            return Err(self.broken());
        }
        let written = self.file.write_all_at(&frame, id.0);
        if written.is_err() {
            self.failed.store(true, Ordering::SeqCst);
            self.wake();
        }
        written
    }
}

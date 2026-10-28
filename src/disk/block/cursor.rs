//! \[DESIGN\] A cursor over a block file (RULES.md D30): a position between
//! blocks that moves forwards or backwards one block at a time. Each step
//! reads into a buffer the caller keeps, so a scan allocates once.

use super::{BlockFile, BlockId};
use crate::error::Result;

/// A position in a block file: before the block at `position`, or at the
/// end. Blocks appended after the cursor was made are reached too.
pub struct Cursor<'a> {
    file: &'a BlockFile,
    at: BlockId,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(file: &'a BlockFile, at: BlockId) -> Self {
        Cursor { file, at }
    }

    /// Where the cursor is: the id of the next block forwards, or the end.
    pub fn position(&self) -> BlockId {
        self.at
    }

    /// Reads the block at the position into `payload` and moves past it;
    /// `None` at the end.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if no valid block is there; SQLCODE -915 if the file
    /// cannot be read. The cursor does not move.
    pub fn next(&mut self, payload: &mut Vec<u8>) -> Result<Option<BlockId>> {
        if self.at >= self.file.end() {
            return Ok(None);
        }
        let id = self.at;
        self.at = self.file.read_into(id, payload)?;
        Ok(Some(id))
    }

    /// Reads the block before the position into `payload` and moves to its
    /// start; `None` at the first block.
    ///
    /// # Errors
    ///
    /// As `next`.
    pub fn prev(&mut self, payload: &mut Vec<u8>) -> Result<Option<BlockId>> {
        let Some(id) = self.file.before(self.at)? else { return Ok(None) };
        self.file.read_into(id, payload)?;
        self.at = id;
        Ok(Some(id))
    }
}

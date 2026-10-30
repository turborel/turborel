//! \[DESIGN\] What follows the valid blocks of a block file as it opens
//! (RULES.md D25, D30): a write a crash interrupted, cut off, or damage,
//! refused.
//!
//! The valid blocks end at the first block that is incomplete or fails its
//! checksum. The bytes after it are scanned for a valid block that follows
//! (`Recovery::follows`): at each offset, a length within the limit, a
//! trailing length that matches it and a digest that matches the payload,
//! read a chunk at a time, so the scan holds a bounded buffer whatever the
//! file's size. None follows: the bytes are a torn end. One follows: the
//! file holds blocks past the invalid ones, which a crash leaves only if
//! the writes before them never reached the disk while theirs did, as the
//! system may write a file's pages back in any order before a sync. A
//! write that never reached the disk leaves what a file grown only by
//! appends held there before, zeros; so the invalid bytes show an
//! interrupted write (`Recovery::interrupted`) if a sector they overlap
//! reads as zeros, if they end in a zero length, as a block's write cut
//! short ahead of one written does, or if the block they begin with runs
//! past the end of the file. Valid blocks after invalid bytes showing no
//! interrupted write are damage: blocks once whole changed, which cutting
//! would lose with what follows them, so the file is refused (SQLCODE
//! -916), unchanged.

use std::path::Path;

use super::format::{self, HEADER_BYTES, LEN_BYTES};
use super::{BlockFile, BlockId, BlockOptions, Found, ReadWith, Recovery, TailPolicy};
use crate::disk::encoding::Pipeline;
use crate::disk::io::{Durability, FileSystem, OpenMode};
use crate::error::{Code, Result, SqlError};

/// \[DESIGN\] The unit a write reaches the disk in, or does not: a sector,
/// 512 bytes, the smallest a disk writes; a larger one is made of these.
const SECTOR: u64 = 512;
/// \[DESIGN\] Bytes read at a time scanning invalid bytes: a multiple of
/// `SECTOR`.
const SCAN_CHUNK: u64 = 64 * 1024;

impl Recovery {
    /// Whether the file is damaged: a valid block follows invalid bytes
    /// that show no interrupted write. The offset of that block if so.
    pub fn damage(&self) -> Option<BlockId> {
        self.follows.filter(|_| !self.interrupted)
    }
}

/// The error for a file at `path` damaged as `recovery` says, a valid block
/// following invalid bytes at `next`.
pub(crate) fn damaged(path: &Path, recovery: &Recovery, next: BlockId) -> SqlError {
    SqlError::new(
        Code::Corrupt,
        format!(
            "[DESIGN] {} is damaged at offset {}: the bytes there are not a valid block, but a valid block follows at offset {}, so it is not opened, and nothing of it is cut",
            path.display(),
            recovery.end.0,
            next.0
        ),
    )
}

impl BlockFile {
    /// Opens the block file at `path` as `open_from` does, but cuts
    /// nothing: the file is taken to end at its last valid block, and
    /// `Recovery` says what follows. The caller cuts it (`cut_tail`)
    /// before appending, or drops the file, unchanged.
    ///
    /// # Errors
    ///
    /// As `open_from`'s, but for a tail.
    pub(crate) fn open_uncut(
        fs: &dyn FileSystem,
        path: &Path,
        (hashes, encodings): ReadWith<'_>,
        options: BlockOptions,
        from: BlockId,
    ) -> Result<(BlockFile, Recovery)> {
        let file = fs.open(path, OpenMode::ReadWrite)?;
        let size = file.size()?;
        let mut header = vec![0; HEADER_BYTES];
        file.read_exact_at(&mut header, 0).map_err(|e| {
            SqlError::new(Code::Corrupt, format!("[DESIGN] {} is too short to be a block file: {}", path.display(), e.described()))
        })?;
        let (hash, ids) = format::read_header(&header, hashes, path)?;
        let mut blocks = BlockFile::new(file, path, hash, options, size);
        blocks.pipeline = Pipeline::resolve(&ids, encodings, path)?;
        let end = blocks.valid_end(blocks.known(from, size)?, size)?;
        let cut = size.saturating_sub(end);
        let mut recovery = Recovery { end: BlockId(end), cut, follows: None, interrupted: false };
        if cut > 0 {
            recovery.follows = blocks.next_valid(end, size)?.map(BlockId);
            let stop = recovery.follows.map_or(size, |b| b.0);
            recovery.interrupted = blocks.runs_past(end, size)? || blocks.zero_end(end, stop)? || blocks.zero_sector(end, stop, size)?;
            blocks.reset_end(end);
        }
        Ok((blocks, recovery))
    }

    /// Refuses a file whose tail `recovery` describes, opened under
    /// `policy`, if it must be: any tail under `TailPolicy::Refuse`, damage
    /// under either.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is refused.
    pub(crate) fn refuse_tail(&self, recovery: &Recovery, policy: TailPolicy) -> Result<()> {
        if recovery.cut == 0 {
            return Ok(());
        }
        if let Some(next) = recovery.damage() {
            return Err(damaged(&self.path, recovery, next));
        }
        match policy {
            TailPolicy::Truncate => Ok(()),
            TailPolicy::Refuse => Err(SqlError::new(
                Code::Corrupt,
                format!(
                    "[DESIGN] the last {} bytes of {} are not valid blocks, and the tail policy is Refuse",
                    recovery.cut,
                    self.path.display()
                ),
            )),
        }
    }

    /// Cuts the tail `recovery` describes off the file, durably, before
    /// any append: a later crash could otherwise bring back old blocks
    /// after the new ones.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the file cannot be cut or synced.
    pub(crate) fn cut_tail(&self, recovery: &Recovery) -> Result<()> {
        if recovery.cut == 0 {
            return Ok(());
        }
        self.file.set_size(recovery.end.0)?;
        self.file.sync(Durability::Full)
    }

    /// The offset of the first valid block after `after`, before `size`.
    fn next_valid(&self, after: u64, size: u64) -> Result<Option<u64>> {
        let mut window = Vec::new();
        let mut scratch = Vec::new();
        let mut chunk = after.saturating_add(1);
        // Each window holds a chunk's offsets and a first read's bytes past
        // them, so a small block is checked from it.
        let ahead = u64::try_from(super::FIRST_READ).unwrap_or(u64::MAX);
        while chunk < size {
            let span = size.saturating_sub(chunk).min(SCAN_CHUNK.saturating_add(ahead));
            window.resize(usize::try_from(span).unwrap_or(usize::MAX), 0);
            self.file.read_exact_at(&mut window, chunk)?;
            let stop = size.min(chunk.saturating_add(SCAN_CHUNK));
            for (i, at) in (chunk..stop).enumerate() {
                if self.valid_at(&window, i, (at, size), &mut scratch)? {
                    return Ok(Some(at));
                }
            }
            chunk = stop;
        }
        Ok(None)
    }

    /// Whether a valid block starts at `at`, of a file of `size` bytes,
    /// whose bytes from there `window` holds from `i`: checked in the window
    /// when the block lies wholly in it, else read.
    fn valid_at(&self, window: &[u8], i: usize, (at, size): (u64, u64), scratch: &mut Vec<u8>) -> Result<bool> {
        let Some(len) = format::le_u32(window, i) else { return Ok(false) };
        let digest_len = self.hash.digest_len();
        let frame = format::frame_len(usize::try_from(len).unwrap_or(usize::MAX), digest_len);
        if len > self.options.max_block || u64::try_from(frame).unwrap_or(u64::MAX) > size.saturating_sub(at) {
            return Ok(false);
        }
        let prefix = LEN_BYTES.saturating_add(digest_len);
        if let Some(bytes) = window.get(i..i.saturating_add(frame)) {
            let digest = bytes.get(LEN_BYTES..prefix).unwrap_or_default();
            return Ok(format::verify(&*self.hash, len, digest, bytes.get(prefix..).unwrap_or_default()));
        }
        Ok(matches!(self.block_at(at, size, scratch)?, Found::Block(_)))
    }

    /// Whether the block at `at` runs past `size`, the end of the file: its
    /// length within the limit, its bytes not all written.
    fn runs_past(&self, at: u64, size: u64) -> Result<bool> {
        let prefix = u64::try_from(LEN_BYTES.saturating_add(self.hash.digest_len())).unwrap_or(u64::MAX);
        if size.saturating_sub(at) < prefix {
            return Ok(true);
        }
        let mut len = [0; LEN_BYTES];
        self.file.read_exact_at(&mut len, at)?;
        let len = u32::from_le_bytes(len);
        let frame = u64::try_from(format::frame_len(usize::try_from(len).unwrap_or(usize::MAX), self.hash.digest_len()));
        Ok(len <= self.options.max_block && frame.unwrap_or(u64::MAX) > size.saturating_sub(at))
    }

    /// Whether the bytes from `from` to `to` end in a zero length: the
    /// trailing length of a block whose write was cut short.
    fn zero_end(&self, from: u64, to: u64) -> Result<bool> {
        let len = u64::try_from(LEN_BYTES).unwrap_or(u64::MAX);
        let Some(at) = to.checked_sub(len).filter(|&at| at >= from) else { return Ok(false) };
        let mut last = [0; LEN_BYTES];
        self.file.read_exact_at(&mut last, at)?;
        Ok(last == [0; LEN_BYTES])
    }

    /// Whether a sector overlapping the bytes from `from` to `to`, of a
    /// file of `size` bytes, reads as zeros: one never written.
    fn zero_sector(&self, from: u64, to: u64, size: u64) -> Result<bool> {
        let mut chunk = from.saturating_sub(from.checked_rem(SECTOR).unwrap_or(0));
        // The end of the last sector overlapping them, or of the file.
        let past = to.checked_rem(SECTOR).map_or(0, |r| SECTOR.saturating_sub(r).checked_rem(SECTOR).unwrap_or(0));
        let limit = size.min(to.saturating_add(past));
        let mut buffer = Vec::new();
        while chunk < limit {
            let span = limit.saturating_sub(chunk).min(SCAN_CHUNK);
            buffer.resize(usize::try_from(span).unwrap_or(usize::MAX), 0);
            self.file.read_exact_at(&mut buffer, chunk)?;
            let sector = usize::try_from(SECTOR).unwrap_or(usize::MAX);
            // A sector read whole, or the file's last, shorter.
            if buffer.chunks(sector).any(|s| s.iter().all(|&b| b == 0)) {
                return Ok(true);
            }
            chunk = chunk.saturating_add(SCAN_CHUNK);
        }
        Ok(false)
    }
}

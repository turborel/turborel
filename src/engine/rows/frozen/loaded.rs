//! \[DESIGN\] A load's groups written straight to their segment (RULES.md
//! D1146). Each group a load makes is framed in memory as an extent
//! (`group/sink.rs`), appended to the segment its stream holds, and given
//! back to the load whole, the same buffer, for its log record: so the
//! group's bytes are written to the segment and to the log, and copied for
//! neither. The log carries them because a standby, and a recovery to a
//! point in time from the archive, has only the log to make the group from.
//!
//! Redone from its record, a committed load's group is appended to a
//! segment of the table's again, wherever it lands, as its pages are
//! numbered within its extent, and put in the directory committed.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::super::RowTable;
use super::super::options::Columns;
use super::extents::{SEGMENT_BYTES, Segment};
use super::records::Batch;
use crate::disk::block::{BlockFile, BlockOptions};
use crate::disk::block::{DEFAULT_MAX_BLOCK, HEADER_LEN};
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::PageId;
use crate::engine::columns::group::sink::{Head, Sink, head_of};
use crate::engine::columns::group::{self, Group, Pages, Placed, Summary};
use crate::engine::columns::scan::rows;
use crate::error::Result;
use crate::hash::HashFunction;
use crate::memory::Reservation;
use crate::storage::RowId;
use crate::sync::{lock_builtin, read};
use crate::txn::TxId;

/// \[DESIGN\] The most bytes of an extent: half the longest block of the
/// log (`DEFAULT_MAX_BLOCK`), so its record, the extent and a head of a few
/// dozen bytes, is one block whatever the log's own limit was set to above
/// that half. A group past it, of a table of rows of some 500 bytes and
/// more at 65,536 rows a group, is written to pages and logged as rows.
const EXTENT_MOST: u32 = DEFAULT_MAX_BLOCK >> 1;

/// A group written as an extent: the group, its entry's summary, its
/// segment, and the extent's bytes and their charge to the budget.
pub(super) type Made = (Group, Summary, Arc<Segment>, (Vec<u8>, Option<Reservation>));

impl RowTable {
    /// The segment `tx`'s stream appends to, taken as its first group is
    /// written; none if the table has no segments.
    fn stream_segment(&self, tx: TxId, bytes: u64) -> Result<Option<Arc<Segment>>> {
        let mut streams = lock_builtin(&self.frozen.loads.streams);
        let held = streams.iter().position(|(by, _)| *by == tx);
        if let Some(at) = held {
            let full = streams
                .get(at)
                .is_some_and(|(_, s)| s.blocks.end().0 > HEADER_LEN && s.blocks.end().0.saturating_add(bytes) > SEGMENT_BYTES);
            if !full {
                return Ok(streams.get(at).map(|(_, s)| Arc::clone(s)));
            }
            let (_, done) = streams.swap_remove(at);
            done.give_back();
        }
        let Some(taken) = self.take_segment(bytes)? else { return Ok(None) };
        streams.push((tx, Arc::clone(&taken)));
        Ok(Some(taken))
    }

    /// Gives back the segment `tx`'s stream holds, if any, its load ended.
    pub(super) fn end_stream(&self, tx: TxId) {
        let mut streams = lock_builtin(&self.frozen.loads.streams);
        if let Some(at) = streams.iter().position(|(by, _)| *by == tx) {
            let (_, done) = streams.swap_remove(at);
            done.give_back();
        }
    }

    /// Writes the group of `batch`, given `ids`, sorted as `columns` says,
    /// as an extent, appended to `tx`'s segment: the group, its entry's
    /// summary and the extent; none if the table has no segments.
    pub(super) fn extent_from(&self, tx: TxId, (ids, batch): (Vec<RowId>, Batch<'_>), columns: &Columns) -> Result<Option<Made>> {
        let Some((place, _)) = self.frozen.extents.placed() else { return Ok(None) };
        let sink = Sink::new(place.hash())?;
        let group = self.group_from(&Pages { sink: Some(&sink), ..self.group_pages()? }, ids, batch, columns)?;
        let header = *group.header.first().ok_or_else(|| super::super::corrupt("a group has no header"))?;
        let extent = sink.finish((group.first, group.last, group.rows), header)?;
        // A group too large for one record of the log is written to pages.
        if u32::try_from(extent.len()).map_or(true, |n| n > EXTENT_MOST) {
            return Ok(None);
        }
        let held = self.frozen.extents.charge(extent.len())?;
        let length = u64::try_from(extent.len()).unwrap_or(u64::MAX);
        let Some(segment) = self.stream_segment(tx, length)? else { return Ok(None) };
        let base = segment.append(&extent)?;
        let summary = Summary { extent: Some(Placed { segment: segment.n, base, length }), ..Summary::of(&group, header)? };
        Ok(Some((group, summary, segment, (extent, held))))
    }

    /// Redoes a committed load's group, `extent` as its record holds it,
    /// of rows from `first`: appended to a segment of the table, and put in
    /// its directory committed before every snapshot, unless a group of
    /// its range is there.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the extent is not whole; as appending it and
    /// storing the directory.
    pub(in crate::engine::rows) fn redo_extent(&self, first: RowId, extent: &[u8]) -> Result<()> {
        let hashes = self.frozen.extents.placed().map_or_else(crate::hash::builtin, |(p, _)| p.hashes());
        let (head, hash) = head_any(&hashes, extent)?;
        if head.first != first || self.frozen.directory()?.holding(first).is_some() {
            return Ok(());
        }
        let length = u64::try_from(extent.len()).unwrap_or(u64::MAX);
        match self.take_segment(length)? {
            Some(segment) => {
                let placed = segment.append(extent);
                segment.give_back();
                let base = placed?;
                let summary = self.summary_at(&segment, (base, length), PageId(head.header))?;
                let entry = self.entry_in(summary, Some((Arc::clone(&segment), base)));
                self.publish_entry((None, Some((entry, Vec::new()))), Vec::new())?;
            }
            None => self.redo_on_pages(extent, hash, PageId(head.header))?,
        }
        self.next.fetch_max(head.last.saturating_add(1), Ordering::SeqCst);
        self.reserve_past(head.last)?;
        self.counted.live.fetch_add(usize::try_from(head.rows).unwrap_or(usize::MAX), Ordering::AcqRel);
        Ok(())
    }

    /// Redoes the group of `extent`, checksummed with `hash`, its header at
    /// `header`, on pages of the pool, as a table of no segments, of a
    /// database in memory following one on disk, keeps its groups: read
    /// from the extent held in memory a while, and written again.
    fn redo_on_pages(&self, extent: &[u8], hash: Arc<dyn HashFunction>, header: PageId) -> Result<()> {
        let fs = MemoryFileSystem::new();
        let path = std::path::Path::new("/extent");
        let blocks = Arc::new(BlockFile::create(&fs, path, hash, Pipeline::none(), BlockOptions::default())?);
        let base = blocks.append_frames(extent)?.0;
        let held = Pages { file: self.pool.add_held_blocks(&blocks), base, ..self.group_pages()? };
        let types = self.format.types();
        let group = group::read(&held, header, types, None)?;
        let ids = group.row_ids(&held)?;
        let all: Vec<usize> = (0..types.len()).collect();
        let rows = rows(&group, &held, types, &all, (0, group.rows))?;
        let columns = (*read(&self.frozen.columns)?).unwrap_or_default();
        let written = self.group_from(&self.group_pages()?, ids, Batch::Rows(&rows), &columns)?;
        let pages = written.pages()?;
        let entry = self.entry_of(written)?;
        self.publish_entry((None, Some((entry, pages))), Vec::new())
    }

    /// The summary of the group of the extent at `base` of `segment`,
    /// `length` bytes, read from its header at `header`.
    fn summary_at(&self, segment: &Segment, (base, length): (u64, u64), header: PageId) -> Result<Summary> {
        let pages = Pages { file: segment.file, base, ..self.group_pages()? };
        let group = group::read(&pages, header, self.format.types(), None)?;
        Ok(Summary { extent: Some(Placed { segment: segment.n, base, length }), ..Summary::of(&group, header)? })
    }
}

/// The head of `extent`, and the one of `hashes` its blocks are
/// checksummed with.
///
/// # Errors
///
/// As `head_of`, if no function of `hashes` reads it whole.
fn head_any(hashes: &[Arc<dyn HashFunction>], extent: &[u8]) -> Result<(Head, Arc<dyn HashFunction>)> {
    let mut failed = None;
    for hash in hashes {
        match head_of(&**hash, extent) {
            Ok(head) => return Ok((head, Arc::clone(hash))),
            Err(e) => failed = Some(e),
        }
    }
    Err(failed.unwrap_or_else(|| super::super::corrupt("an extent of no known checksum")))
}

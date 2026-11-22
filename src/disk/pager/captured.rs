//! \[DESIGN\] Pages a checkpoint captures after its writers are let go
//! (RULES.md D590): each written as it was when the checkpoint was taken,
//! into the batch the checkpoint sealed (`DiskFile::write_captured_at`),
//! while the pages written since go on to the file as before.
//!
//! A page is captured so only if writing it needs no other page: its
//! content fits in it, and it is known to have no overflow pages to free.
//! A capture then allocates and frees nothing, so the free list the
//! checkpoint recorded as it was taken stays true of the pages it
//! captures, whatever writers allocate and free meanwhile; and it leaves
//! what the pager knows of the page's overflow alone, as that is of the
//! page as written since. The pipeline is the one the checkpoint was taken
//! with, so a page that fit then fits as it is captured.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::super::encoding::Pipeline;
use super::format::{self, DATA_HEADER, NONE};
use super::live::Encoded;
use super::{PageId, Pager, add};
use crate::error::{Code, Result, SqlError};

/// What a checkpoint captures pages with: the pipeline its file wrote
/// through when it was taken, and the most content a page then holds
/// without overflow.
pub struct Capturing {
    pipeline: Arc<Pipeline>,
    capacity: usize,
}

impl Pager {
    /// How a checkpoint taken now captures pages, if this file's may be
    /// captured: a durable file's, whose pages name their pipeline.
    pub fn capturing(&self) -> Option<Capturing> {
        let Encoded::Named(named) = &self.pipeline else { return None };
        let pipeline = named.current();
        let capacity = self.body_size().saturating_sub(DATA_HEADER).saturating_sub(pipeline.overhead());
        Some(Capturing { pipeline, capacity })
    }

    /// Whether page `id`, of `len` bytes of content, may be captured after
    /// writers are let go: it fits in its page, and is known to have no
    /// overflow pages.
    pub fn deferrable(&self, id: PageId, len: usize, capturing: &Capturing) -> bool {
        len <= capturing.capacity && self.known.of(id.0).is_some_and(|extents| extents.is_empty())
    }

    /// Makes the file hold page `last`, as a checkpoint taken now, before
    /// it is sealed, for the pages it captures after, none past `last`: a
    /// page allocated and not yet written is past the file's end.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the file's size cannot be found out or set.
    pub fn cover(&self, last: PageId) -> Result<()> {
        let end = self.offset(last.0.saturating_add(1))?;
        if end > self.file.size()? {
            self.file.set_size(end)?;
        }
        self.on_file.fetch_max(last.0.saturating_add(1), Ordering::AcqRel);
        Ok(())
    }

    /// Writes `content` as page `id` into what the checkpoint captures
    /// (`DiskFile::write_captured_at`), through the pipeline of
    /// `capturing`, in place, allocating and freeing nothing.
    ///
    /// # Errors
    ///
    /// The pipeline's; SQLCODE -910 if the content does not fit in the
    /// page, or the file takes no capture.
    pub fn captured(&self, id: PageId, content: &[u8], capturing: &Capturing) -> Result<()> {
        let stored = capturing.pipeline.encode(id.0, content)?;
        if stored.len() > self.body_size().saturating_sub(DATA_HEADER) {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] RULES.md D590: page {} does not fit to be captured", id.0)));
        }
        let length = u64::try_from(stored.len()).unwrap_or(u64::MAX);
        let body = format::data_body(length, (&[], NONE), &capturing.pipeline.ids(), &stored);
        let mut sealed = Vec::with_capacity(self.page_size);
        self.seal(id.0, &body, &mut sealed);
        self.file.write_captured_at(&sealed, self.offset(id.0)?)?;
        self.on_file.fetch_max(id.0.saturating_add(1), Ordering::AcqRel);
        add(&self.counters.writes, 1);
        Ok(())
    }
}

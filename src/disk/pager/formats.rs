//! \[DESIGN\] The formats a durable pager file's pages are in (RULES.md
//! D48): the encodings and the checksum's key new pages take now
//! (`Pager::set_pipeline`, `Pager::set_hash`), and those its header
//! names, which every page is in once a sweep has written again each
//! page still in others (`behind`) and the header is written again
//! (`settle`). A page names its own formats in the clear, in its body's
//! header and its trailer, so the sweep reads pages raw, in runs, and
//! decodes none.

use std::sync::atomic::Ordering;

use super::format::{self, KIND_DATA, PAGE_NUMBER_BYTES};
use super::{Pager, add};
use crate::disk::encoding::EncodingId;
use crate::error::{Code, Result, SqlError};
use crate::hash::read_key_field;
use crate::sync::lock_builtin;

/// \[DESIGN\] The formats pages are in: the ids of the encodings their
/// content passed through, and the name of the key their checksums were
/// made with, empty of a function not keyed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Formats {
    /// The encodings' ids, in order.
    pub encodings: Vec<EncodingId>,
    /// The key's name.
    pub key: String,
}

/// What a run of pages looked at holds: those still in other formats, to
/// write again, and the page after the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Behind {
    /// The data pages of the run in formats other than the current.
    pub again: Vec<u64>,
    /// The page after the run: the file's end once every page was looked
    /// at.
    pub next: u64,
}

impl Pager {
    /// The formats pages written from now on take.
    pub fn current_formats(&self) -> Formats {
        let hash = self.hash.current();
        Formats { encodings: self.pipeline.current_ids(), key: hash.key_name().unwrap_or_default().to_owned() }
    }

    /// The formats the header names: those every page is in, unless one
    /// was written since they changed (`behind`).
    pub fn settled_formats(&self) -> Formats {
        lock_builtin(&self.settled).clone()
    }

    /// Looks at `count` pages from `from`, in one read: the data pages
    /// among them in other formats than the current, to write again, and
    /// where to look next. A page never written, a free page (zeroed) and
    /// the pager's own pages (written again with the data page they
    /// belong to, or at each sync) need no writing again.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the pages cannot be read.
    pub fn behind(&self, from: u64, count: u64) -> Result<Behind> {
        let next = from.saturating_add(count).min(self.page_count()).max(from);
        // Pages past those on file read as new.
        let readable = next.min(self.on_file.load(Ordering::Acquire));
        if from >= readable {
            return Ok(Behind { again: Vec::new(), next });
        }
        let current = self.current_formats();
        let run = self.read_run(from, readable.saturating_sub(from))?;
        let keyed = self.hash.current().keyed();
        let again = run
            .chunks(self.page_size)
            .zip(from..)
            .filter(|(page, _)| self.in_other_formats(page, keyed, &current))
            .map(|(_, id)| id)
            .collect();
        add(&self.counters.reads, readable.saturating_sub(from));
        Ok(Behind { again, next })
    }

    /// Whether `page`, read raw, is a data page in formats other than
    /// `current`: by the ids its body names and, if the function is
    /// `keyed`, the key its trailer names. A page malformed is not: it is
    /// refused as it is read, not here.
    fn in_other_formats(&self, page: &[u8], keyed: bool, current: &Formats) -> bool {
        if page.first() != Some(&KIND_DATA) {
            return false;
        }
        let Some(ids) = format::data_ids(page) else { return false };
        let key = if keyed { read_key_field(page, self.body_size().saturating_add(PAGE_NUMBER_BYTES)) } else { Some("") };
        key.is_some_and(|key| ids != current.encodings || key != current.key)
    }

    /// Writes the header again naming `target`, the formats every page
    /// is in (`behind` listing none), if they are the current ones still:
    /// whether it did. The file is then opened by `target`'s key alone,
    /// and the encodings it names, once the next sync writes the meta slot
    /// again.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 of a scratch file, or if `target`'s key is not one the
    /// function takes; SQLCODE -915 if the header cannot be written.
    pub fn settle(&self, target: &Formats) -> Result<bool> {
        let mut settled = lock_builtin(&self.settled);
        if *target != self.current_formats() {
            return Ok(false);
        }
        if !self.pipeline.is_named() {
            return Err(SqlError::new(Code::Api, "[DESIGN] a scratch file's formats are never settled: it is not opened again"));
        }
        let hash = self.hash.verifier(&target.key)?;
        let page_size = u32::try_from(self.page_size).unwrap_or(u32::MAX);
        self.file.write_all_at(&format::header_fields(&*hash, &target.encodings, page_size), 0)?;
        *settled = target.clone();
        // The meta slot names its key too: written again at the next sync.
        self.resettled.store(true, Ordering::Release);
        Ok(true)
    }
}

#[cfg(test)]
mod tests;

//! \[DESIGN\] The encodings a pager file's pages pass through (RULES.md D30,
//! D48): a durable file's, fixed when it is made and named in its header;
//! a scratch file's, those a `LivePipeline` holds as each page is written,
//! so a spill area made before a table is encrypted writes its pages
//! encrypted from then on, those still in its pool among them. A scratch
//! file's stored content begins with a byte naming which of the pipelines
//! it was written through encoded it, the first the one its header names;
//! nothing but the file's maker reads it, so they are kept in memory, at
//! most `MAX_PIPELINES`. Split from pager.rs (RULES.md R-SIZE-2).

use std::borrow::Cow;
use std::sync::{Arc, Mutex};

use super::super::encoding::{EncodingId, LivePipeline, Pipeline};
use crate::error::{Code, Result, SqlError};
use crate::sync::lock_builtin;

/// \[DESIGN\] The most pipelines one scratch file is written through: those
/// a byte names. Pipelines of the same encodings count once, so only as
/// many changes of SPILL.ENCODINGS and of the first encryption in use as
/// that, in the life of one spill area, exceed it.
const MAX_PIPELINES: usize = 256;

/// The encodings of a pager file's pages.
pub(super) enum Encoded {
    /// A durable file's, named in its header.
    Fixed(Pipeline),
    /// A scratch file's.
    Live(Live),
}

/// A scratch file's encodings: where it takes them, and those it took.
pub(super) struct Live {
    source: Arc<LivePipeline>,
    taken: Mutex<Taken>,
}

/// The pipelines a scratch file was written through, each with its ids,
/// read without a call into a plug-in under the lock (RULES.md R-ERR-4);
/// the one it writes through now; and the setting of the source it is.
struct Taken {
    used: Vec<(Vec<EncodingId>, Arc<Pipeline>)>,
    now: usize,
    seen: u64,
}

impl Encoded {
    /// A scratch file's, taken from `source`; and the pipeline its header
    /// names, the one `source` holds now.
    pub(super) fn live(source: &Arc<LivePipeline>) -> (Encoded, Pipeline) {
        let (seen, first) = source.current();
        let used = vec![(first.ids(), Arc::clone(&first))];
        let taken = Mutex::new(Taken { used, now: 0, seen });
        (Encoded::Live(Live { source: Arc::clone(source), taken }), Pipeline::clone(&first))
    }

    /// Whether it leaves content as it is.
    pub(super) fn is_empty(&self) -> bool {
        match self {
            Encoded::Fixed(pipeline) => pipeline.is_empty(),
            Encoded::Live(_) => false,
        }
    }

    /// The most bytes it adds to content: a scratch file's tag and the
    /// growth of the pipeline its header names; what a later one adds past
    /// that goes to overflow pages.
    pub(super) fn overhead(&self) -> usize {
        match self {
            Encoded::Fixed(pipeline) => pipeline.overhead(),
            Encoded::Live(live) => {
                let first = lock_builtin(&live.taken).used.first().map(|(_, p)| Arc::clone(p));
                first.map_or(0, |p| p.overhead()).saturating_add(1)
            }
        }
    }

    /// `input`, of the page `context`, encoded.
    ///
    /// # Errors
    ///
    /// The encodings'; SQLCODE -910 if a scratch file would pass
    /// `MAX_PIPELINES`.
    pub(super) fn encode<'a>(&self, context: u64, input: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        match self {
            Encoded::Fixed(pipeline) => pipeline.encode(context, input),
            Encoded::Live(live) => {
                let (tag, pipeline) = live.now()?;
                let encoded = pipeline.encode(context, input)?;
                let mut out = Vec::with_capacity(encoded.len().saturating_add(1));
                out.push(tag);
                out.extend_from_slice(&encoded);
                Ok(Cow::Owned(out))
            }
        }
    }

    /// `input`, of the page `context`, decoded.
    ///
    /// # Errors
    ///
    /// The encodings'; SQLCODE -916 if a scratch file's content names no
    /// pipeline it was written through.
    pub(super) fn decode<'a>(&self, context: u64, input: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        match self {
            Encoded::Fixed(pipeline) => pipeline.decode(context, input),
            Encoded::Live(live) => {
                let unnamed = || SqlError::new(Code::Corrupt, "[DESIGN] a scratch file's page names no pipeline it was written through");
                let (&tag, rest) = input.split_first().ok_or_else(unnamed)?;
                let pipeline = lock_builtin(&live.taken).used.get(usize::from(tag)).map(|(_, p)| Arc::clone(p)).ok_or_else(unnamed)?;
                pipeline.decode(context, rest)
            }
        }
    }
}

impl Live {
    /// The pipeline to write through now, and its tag: the source's, taken
    /// again only once it is set again. One of the encodings of one taken
    /// before replaces it under its tag, as its encodings' output records
    /// what decoding needs (RULES.md D48).
    fn now(&self) -> Result<(u8, Arc<Pipeline>)> {
        let (setting, current) = self.source.current();
        let seen = lock_builtin(&self.taken).seen;
        if seen != setting {
            self.take(setting, current)?;
        }
        let taken = lock_builtin(&self.taken);
        let pipeline = taken.used.get(taken.now).map(|(_, p)| Arc::clone(p));
        let tag = u8::try_from(taken.now).ok();
        tag.zip(pipeline).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a scratch file's pipeline is not among those it took"))
    }

    /// Writes through `current`, the source's `setting`th, from now on.
    fn take(&self, setting: u64, current: Arc<Pipeline>) -> Result<()> {
        let ids = current.ids();
        let mut taken = lock_builtin(&self.taken);
        let at = match taken.used.iter().position(|(used, _)| *used == ids) {
            Some(at) => at,
            None => taken.used.len(),
        };
        if at >= MAX_PIPELINES {
            return Err(SqlError::new(
                Code::Api,
                format!("[DESIGN] a scratch file is written through at most {MAX_PIPELINES} pipelines (RULES.md D48)"),
            ));
        }
        match taken.used.get_mut(at) {
            Some(slot) => *slot = (ids, current),
            None => taken.used.push((ids, current)),
        }
        (taken.now, taken.seen) = (at, setting);
        Ok(())
    }
}

#[cfg(test)]
mod tests;

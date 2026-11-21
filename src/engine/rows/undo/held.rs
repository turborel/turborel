//! \[DESIGN\] The bytes of a row version as a reader holds them: a tree
//! entry read into its own vector, or an older version shared with the
//! undo store's memory (`rows/undo.rs`). A reader going back through a
//! row's versions, or along a table, reads many versions of one batch: each
//! is a count taken of the batch, not a copy of its bytes, and the record a
//! version holds is the same bytes narrowed, not copied out.

use std::ops::Deref;
use std::sync::Arc;

/// Where held bytes are.
#[derive(Debug, Clone)]
enum Source {
    /// A vector of the reader's own.
    Owned(Vec<u8>),
    /// A version gathered into a batch not yet written, alone.
    Version(Arc<[u8]>),
    /// A batch's record read from the log, holding many versions.
    Record(Arc<Vec<u8>>),
}

/// \[DESIGN\] Bytes of a row version, or of the record it holds: the range
/// `from..to` of its source.
#[derive(Debug, Clone)]
pub(in crate::engine::rows) struct Held {
    source: Source,
    from: usize,
    to: usize,
}

impl Held {
    /// The version gathered `version`, shared.
    pub(super) fn version(version: &Arc<[u8]>) -> Held {
        Held { to: version.len(), source: Source::Version(Arc::clone(version)), from: 0 }
    }

    /// The bytes `from..to` of a batch's `record`, shared.
    pub(super) fn in_record(record: &Arc<Vec<u8>>, (from, to): (usize, usize)) -> Held {
        Held { source: Source::Record(Arc::clone(record)), from, to }
    }

    /// The last `len` bytes of these alone: of a version, the record it
    /// ends with (`version.rs`). All of them if they are fewer.
    pub(in crate::engine::rows) fn last(mut self, len: usize) -> Held {
        self.from = self.from.max(self.to.saturating_sub(len));
        self
    }
}

impl From<Vec<u8>> for Held {
    fn from(bytes: Vec<u8>) -> Held {
        Held { to: bytes.len(), source: Source::Owned(bytes), from: 0 }
    }
}

impl Deref for Held {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        let all: &[u8] = match &self.source {
            Source::Owned(bytes) => bytes,
            Source::Version(bytes) => bytes,
            Source::Record(bytes) => bytes,
        };
        all.get(self.from..self.to).unwrap_or_default()
    }
}

impl AsRef<[u8]> for Held {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

/// \[DESIGN\] The record of a version a reader sees: borrowed from the
/// newest version read in place, as most reads see it, or held.
#[derive(Debug)]
pub(in crate::engine::rows) enum Seen<'b> {
    Borrowed(&'b [u8]),
    Held(Held),
}

impl Deref for Seen<'_> {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            Seen::Borrowed(bytes) => bytes,
            Seen::Held(held) => held,
        }
    }
}

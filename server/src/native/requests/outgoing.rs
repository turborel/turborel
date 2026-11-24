//! A request's replies as a worker gathers them, and what of them is
//! sent ahead of the rest (`native/requests.rs`).

use std::mem;

use turborel::error::Result;
use turborel::storage::Row;
use turborel_protocol::Reply;

use super::Cut;
use crate::settings::DEFAULT_LOB_INLINE_BYTES;

/// A request's replies: those gathered, and what a batch's worth is sent
/// through, ahead of the rest.
pub(in crate::native) struct Outgoing<'a> {
    pub(in crate::native) sink: &'a mut dyn FnMut(Vec<Reply>) -> Result<()>,
    /// The bytes a batch of rows is.
    pub(in crate::native) batch: usize,
    /// SERVER.LOB_INLINE_BYTES: the longest large object sent whole.
    pub(in crate::native) inline: usize,
    /// Whether a locator was sent (`native/lobs.rs`).
    pub(in crate::native) located: bool,
    pub(in crate::native) gathered: Vec<Reply>,
    /// How many times what was gathered was sent ahead.
    pub(in crate::native) sends: usize,
}

impl<'a> Outgoing<'a> {
    /// None gathered yet, batches of `batch` bytes sent through `sink`.
    /// Room is made for one reply, a request's last, which most requests
    /// gather alone: a vector grown from none took room for four.
    pub(in crate::native) fn new(sink: &'a mut dyn FnMut(Vec<Reply>) -> Result<()>, batch: usize) -> Outgoing<'a> {
        Outgoing { sink, batch, inline: DEFAULT_LOB_INLINE_BYTES, located: false, gathered: Vec::with_capacity(1), sends: 0 }
    }

    /// As it is, large objects of at most `bytes` octets sent whole.
    #[must_use]
    pub(in crate::native) fn inlining(self, bytes: usize) -> Outgoing<'a> {
        Outgoing { inline: bytes, ..self }
    }

    /// The replies gathered and not sent ahead: the request's last, for
    /// the wire to send, or to queue before the next pipelined request's.
    pub(in crate::native) fn finish(self) -> Vec<Reply> {
        self.gathered
    }

    /// Sends what is gathered, ahead of the rest.
    pub(super) fn send(&mut self) -> std::result::Result<(), Cut> {
        self.sends = self.sends.saturating_add(1);
        (self.sink)(mem::take(&mut self.gathered)).map_err(Cut::Gone)
    }

    /// Gathers the rows of `rows`, if any.
    pub(super) fn rows(&mut self, rows: &mut Vec<Row>) {
        if !rows.is_empty() {
            self.gathered.push(Reply::Rows(mem::take(rows)));
        }
    }
}

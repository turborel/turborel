//! A connection's frames: each request encoded and sent, compressed once
//! compression is granted (RULES.md D196), and each reply read and
//! decoded, inflated if it came compressed; a Notice or a Status before a
//! statement's reply taken on the way (`told.rs`).

use std::io::{Read, Write};

use turborel_protocol::compress::{COMPRESSED, compress_frames, decompress};
use turborel_protocol::{Failure, HEADER, Reply, Request};

use crate::connection::{Connection, Result};
use crate::timeouts::io_failed;

/// \[DESIGN\] The most of a reply's body reserved before its bytes
/// arrive; past it, the buffer grows as they do.
const RESERVED: usize = 64 * 1024;

impl<S: Read + Write> Connection<S> {
    pub(crate) fn send(&mut self, request: &Request) -> Result<()> {
        if let Some(lost) = &self.lost {
            return Err(lost.clone());
        }
        let mut bytes = Vec::new();
        request.encode(&mut bytes);
        if let Some(threshold) = self.told.compress {
            let plain = std::mem::take(&mut bytes);
            compress_frames(&mut bytes, &plain, threshold);
        }
        let stream = self.stream.get_mut();
        let sent = stream.write_all(&bytes).and_then(|()| stream.flush());
        sent.map_err(|e| self.lost_by(io_failed("send", &e)))
    }

    /// Loses the connection, for `failure`.
    pub(crate) fn lost_by(&mut self, failure: Failure) -> Failure {
        self.lost = Some(failure.clone());
        failure
    }

    /// The server's next reply, each Notice and Status before it taken
    /// (`told.rs`), a compressed one inflated.
    pub(crate) fn receive(&mut self) -> Result<Reply> {
        loop {
            match self.next_reply()? {
                Reply::Notice(notice) => self.told.notice(notice),
                Reply::Status { name, value } => drop(self.told.settings.insert(name, value)),
                reply @ (Reply::Challenge(_)
                | Reply::Ready { .. }
                | Reply::Prepared { .. }
                | Reply::Description(_)
                | Reply::Rows(_)
                | Reply::Suspended
                | Reply::Complete(_)
                | Reply::Error(_)
                | Reply::Lob(_)
                | Reply::Pong) => return Ok(self.noted(reply)),
            }
        }
    }

    /// `reply`, of a statement, its end noted: a query's rows are read in a
    /// transaction; a statement's end says whether it goes on, whether it
    /// warned, and where its commit is.
    fn noted(&mut self, reply: Reply) -> Reply {
        match &reply {
            Reply::Complete(c) => {
                (self.active, self.warning) = (c.active, c.is_warning().then(|| c.sqlstate.clone()));
                self.told.position = c.position.or(self.told.position);
                self.told.completed = Some(c.clone());
            }
            Reply::Error(f) => (self.active, self.warning) = (f.active, None),
            Reply::Description(_) => (self.active, self.warning) = (true, None),
            // A locator is valid within a transaction alone.
            Reply::Rows(_) | Reply::Suspended | Reply::Lob(_) => self.active = true,
            Reply::Challenge(_) | Reply::Ready { .. } | Reply::Prepared { .. } | Reply::Notice(_) | Reply::Status { .. } | Reply::Pong => {}
        }
        reply
    }

    /// The next frame the server sends, decoded: inflated first if it is
    /// compressed, and compression was granted.
    fn next_reply(&mut self) -> Result<Reply> {
        let mut head = [0_u8; HEADER];
        if let Err(e) = self.stream.read_exact(&mut head) {
            return Err(self.lost_by(io_failed("receive", &e)));
        }
        let (kind, length) = turborel_protocol::header(head, self.max_body).map_err(|e| self.failed(Failure::from(&e)))?;
        // Read as its bytes arrive: a header alone reserves little.
        let mut body = Vec::with_capacity(length.min(RESERVED));
        let wanted = u64::try_from(length).unwrap_or(u64::MAX);
        if let Err(e) = self.stream.by_ref().take(wanted).read_to_end(&mut body) {
            return Err(self.lost_by(io_failed("receive", &e)));
        }
        if body.len() != length {
            return Err(self.lose("[DESIGN] cannot receive: the connection closed within a reply"));
        }
        if kind == COMPRESSED && self.told.compress.is_some() {
            let (inner, inflated) = decompress(&body, self.max_body).map_err(|e| self.failed(Failure::from(&e)))?;
            return Reply::decode(inner, &inflated).map_err(|e| self.failed(Failure::from(&e)));
        }
        Reply::decode(kind, &body).map_err(|e| self.failed(Failure::from(&e)))
    }
}

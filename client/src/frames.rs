//! A connection's frames: each request encoded and sent, compressed once
//! compression is granted (RULES.md D196), and each reply read and
//! decoded, inflated if it came compressed; a Notice or a Status before a
//! statement's reply taken on the way (`told.rs`); a reply that holds a
//! value the client cannot take read past with the rest of its request's,
//! a malformed one losing the connection (RULES.md D393).

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
        let mut bytes = Vec::new();
        request.encode(&mut bytes);
        self.write_encoded(&bytes)
    }

    /// Writes `bytes`, requests already encoded, at once: one request, or
    /// a pipeline's batch (`pipeline.rs`); each frame past the threshold
    /// compressed, once compression is granted.
    ///
    /// # Errors
    ///
    /// The failure that lost the connection, before or as they are
    /// written.
    pub(crate) fn write_encoded(&mut self, bytes: &[u8]) -> Result<()> {
        if let Some(lost) = &self.lost {
            return Err(lost.clone());
        }
        let mut compressed = Vec::new();
        let bytes = if let Some(threshold) = self.told.compress {
            compress_frames(&mut compressed, bytes, threshold);
            compressed.as_slice()
        } else {
            bytes
        };
        let stream = self.stream.get_mut();
        let sent = stream.write_all(bytes).and_then(|()| stream.flush());
        sent.map_err(|e| self.lost_by(io_failed("send", &e)))
    }

    /// Loses the connection, for `failure`.
    pub(crate) fn lost_by(&mut self, failure: Failure) -> Failure {
        self.lost = Some(failure.clone());
        failure
    }

    /// The server's next reply, each Notice and Status before it taken
    /// (`told.rs`), a compressed one inflated; a reply with a value it
    /// cannot take read past with the rest of its request's (`drained`).
    pub(crate) fn receive(&mut self) -> Result<Reply> {
        self.receive_of(Answer::Replies)
    }

    /// The reply to a request `answer` says is answered by one, as
    /// `receive` takes it.
    pub(crate) fn receive_of(&mut self, answer: Answer) -> Result<Reply> {
        loop {
            let reply = self.next_reply(answer)?;
            if let Some(reply) = self.took(reply) {
                return Ok(reply);
            }
        }
    }

    /// `reply`, unless it is a Notice or a Status, which is taken
    /// (`told.rs`); a reply of a statement, its end noted.
    fn took(&mut self, reply: Reply) -> Option<Reply> {
        match reply {
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
            | Reply::Pong) => return Some(self.noted(reply)),
        }
        None
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
            // Ready ends a login or a ChangeUser, which renews the session
            // as Reset does, its transaction rolled back.
            Reply::Ready { .. } => (self.active, self.warning) = (false, None),
            Reply::Challenge(_) | Reply::Prepared { .. } | Reply::Notice(_) | Reply::Status { .. } | Reply::Pong => {}
        }
        reply
    }

    /// The next frame the server sends, decoded. A frame read whole that
    /// does not decode is either malformed, which loses the connection
    /// (SQLSTATE 08006), or holds a value the client cannot take, which
    /// fails the request alone once the rest of its replies are read past
    /// (RULES.md D393), so the next request takes its own replies.
    pub(crate) fn next_reply(&mut self, answer: Answer) -> Result<Reply> {
        let (kind, body) = self.next_frame()?;
        match Reply::decode(kind, &body) {
            Ok(reply) => Ok(reply),
            Err(e) => {
                let failure = Failure::from(&e);
                self.skipped(kind, &body, &failure)?;
                if answer == Answer::Replies {
                    self.drained(kind, body)?;
                }
                Err(failure)
            }
        }
    }

    /// The next frame the server sends, its kind and body, inflated first
    /// if it is compressed and compression was granted.
    ///
    /// # Errors
    ///
    /// A connection failure, the connection lost, if no frame whole comes
    /// or its header or compression is malformed.
    fn next_frame(&mut self) -> Result<(u8, Vec<u8>)> {
        let mut head = [0_u8; HEADER];
        if let Err(e) = self.stream.read_exact(&mut head) {
            return Err(self.lost_by(io_failed("receive", &e)));
        }
        let (kind, length) = turborel_protocol::header(head, self.max_body).map_err(|e| self.lost_by(Failure::from(&e)))?;
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
            return decompress(&body, self.max_body).map_err(|e| self.lost_by(Failure::from(&e)));
        }
        Ok((kind, body))
    }

    /// Takes a frame of kind `kind` and body `body` that did not decode,
    /// for `failure`. The connection is lost if `failure` ends it, as a
    /// malformed frame's does; else of a Complete, what its head says is
    /// noted as its whole would be (`Reply::completed_head`).
    ///
    /// # Errors
    ///
    /// `failure`, if it ends the connection.
    fn skipped(&mut self, kind: u8, body: &[u8], failure: &Failure) -> Result<()> {
        if failure.ends_connection() {
            return Err(self.lost_by(failure.clone()));
        }
        if let Some(head) = Reply::completed_head(kind, body) {
            self.noted(Reply::Complete(head));
        }
        Ok(())
    }

    /// \[DESIGN\] RULES.md D393: reads past the rest of a request's
    /// replies once one, of kind `kind` and body `body`, held a value the
    /// client cannot take. It reads up to the one that ends them
    /// (`Reply::ends_request`), each taken as `receive` takes it, or
    /// skipped as that one was.
    ///
    /// # Errors
    ///
    /// A connection failure, the connection lost, if one comes before the
    /// last reply.
    fn drained(&mut self, mut kind: u8, mut body: Vec<u8>) -> Result<()> {
        while !Reply::ends_request(kind, &body) {
            (kind, body) = self.next_frame()?;
            match Reply::decode(kind, &body) {
                Ok(reply) => drop(self.took(reply)),
                Err(e) => self.skipped(kind, &body, &Failure::from(&e))?,
            }
        }
        Ok(())
    }
}

/// \[DESIGN\] How a request is answered, as a reader knows it, so a reply
/// that holds a value the client cannot take is read past with the rest
/// of its request's (RULES.md D393).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// By replies in a statement's order, the last told by its kind
    /// (`Reply::ends_request`).
    Replies,
    /// By one reply, after any Notice and Status, as a Describe of a
    /// cursor is, by a Description that no rows follow.
    One,
}

// Unit tests against a scripted server stream (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;

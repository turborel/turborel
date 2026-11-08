//! A connection's messages, framed: requests read, replies written. The
//! wire is lent with the session to the worker running a request
//! (`native/lent.rs`), which reads and writes it apart from the runtime
//! where its transport allows (`Transport::direct`), and waits so for a
//! fast client's next request; the connection's task reads and writes it
//! through the runtime, which takes the transport back from the thread
//! as it does.
//!
//! What the client sends is taken into memory as its bytes arrive, never
//! reserved whole as a header declares it: a client that declares a long
//! body and sends little of it holds little, and a connection holds
//! `INBOX` bytes between requests.

use std::io::ErrorKind;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use turborel::error::{Result, SqlError};
use turborel::value::types::TypeRegistry;
use turborel_protocol::{Failure, HEADER, Reply, Request, broken};

use crate::transport::Transport;

/// \[DESIGN\] The bytes a connection holds for what its client sends: the
/// inbox grows past them for a longer request, by at most `RESERVED` at a
/// time as its bytes arrive, and shrinks back once it is taken.
const INBOX: usize = 8 * 1024;
const RESERVED: usize = 64 * 1024;
/// \[DESIGN\] The bytes taken at a time from a refused client, until it
/// closes (`Wire::close_refusing`): thrown away, so few.
const DRAINED: usize = 1024;

/// A connection's messages.
pub(super) struct Wire {
    transport: Box<dyn Transport>,
    /// What the client has sent and is not taken yet: `inbox[start..end]`.
    inbox: Vec<u8>,
    start: usize,
    end: usize,
    max_body: u32,
    /// The types beyond the standard a request's values may be of.
    types: TypeRegistry,
    /// The replies encoded last, and how many of their bytes are sent.
    out: Vec<u8>,
    sent: usize,
}

/// A whole request in the inbox, as `Wire::arrived` finds it.
pub(super) enum Arrived<T> {
    /// Taken, as asked.
    Taken(T),
    /// Left: not asked for, or malformed, so its failure comes in its
    /// turn, from `Wire::receive`.
    Left,
    /// None is whole yet.
    Awaited,
}

/// The connection failed receiving: `why`.
fn cannot_receive(why: &dyn std::fmt::Display) -> SqlError {
    broken(format!("[DESIGN] cannot receive: {why}"))
}

/// The connection failed sending: `why`.
fn cannot_send(why: &dyn std::fmt::Display) -> SqlError {
    broken(format!("[DESIGN] cannot send: {why}"))
}

/// Whether `e` says nothing is there yet: nothing came, or the wait for
/// it ran out.
fn nothing_yet(e: &std::io::Error) -> bool {
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// The client closed the connection: within a message, if part of one
/// had come.
fn closed(within_message: bool) -> SqlError {
    if within_message {
        return cannot_receive(&"the connection closed within a message");
    }
    cannot_receive(&"the client closed the connection")
}

impl Wire {
    /// The messages of `transport`: its requests, of at most `max_body`
    /// bytes, and its replies.
    pub(super) fn new(transport: Box<dyn Transport>, max_body: u32, types: TypeRegistry) -> Wire {
        Wire { transport, inbox: vec![0; INBOX], start: 0, end: 0, max_body, types, out: Vec::new(), sent: 0 }
    }

    /// Takes requests of at most `max_body` bytes from now on.
    pub(super) fn set_max_body(&mut self, max_body: u32) {
        self.max_body = max_body;
    }

    /// Whether a thread can read and write it apart from the runtime.
    pub(super) fn is_direct(&mut self) -> bool {
        self.transport.direct().is_some()
    }

    /// The client's next request.
    pub(super) async fn receive(&mut self) -> Result<Request> {
        loop {
            let whole = self.whole()?;
            if let Some(request) = whole {
                return Ok(request);
            }
            self.make_room();
            let Wire { transport, inbox, start, end, .. } = self;
            let n = transport.read(inbox.get_mut(*end..).unwrap_or_default()).await.map_err(|e| cannot_receive(&e))?;
            if n == 0 {
                return Err(closed(*start < *end));
            }
            *end = end.saturating_add(n);
        }
    }

    /// Reads what the client sends, apart from the runtime, waiting at
    /// most `within` for it: whether anything came. Nothing, too, if the
    /// transport cannot be read so.
    pub(super) fn fill_within(&mut self, within: Duration) -> Result<bool> {
        self.make_room();
        let Wire { transport, inbox, start, end, .. } = self;
        let Some(direct) = transport.direct() else { return Ok(false) };
        direct.wait(within).map_err(|e| cannot_receive(&e))?;
        loop {
            let read = direct.read(inbox.get_mut(*end..).unwrap_or_default());
            match read {
                Ok(0) => return Err(closed(*start < *end)),
                Ok(n) => {
                    *end = end.saturating_add(n);
                    return Ok(true);
                }
                Err(e) => {
                    if nothing_yet(&e) {
                        return Ok(false);
                    }
                    if e.kind() != ErrorKind::Interrupted {
                        return Err(cannot_receive(&e));
                    }
                }
            }
        }
    }

    /// The next request, if it has come whole and `take` gives something
    /// of it: a client's requests sent together, read without waiting.
    pub(super) fn arrived<T>(&mut self, take: impl FnOnce(Request) -> Option<T>) -> Arrived<T> {
        let (kind, len) = match self.framed() {
            Ok(Some(framed)) => framed,
            Ok(None) => return Arrived::Awaited,
            Err(_) => return Arrived::Left,
        };
        let Ok(request) = Request::decode_with(kind, self.body(len), &self.types) else { return Arrived::Left };
        match take(request) {
            Some(taken) => {
                self.consume(len);
                Arrived::Taken(taken)
            }
            None => Arrived::Left,
        }
    }

    /// The next request, taken, if it has come whole.
    fn whole(&mut self) -> Result<Option<Request>> {
        let Some((kind, len)) = self.framed()? else { return Ok(None) };
        let request = Request::decode_with(kind, self.body(len), &self.types);
        self.consume(len);
        request.map(Some)
    }

    /// The kind and whole length of the next request, if it has come
    /// whole; refused as its header is, if that is malformed or declares
    /// more than `max_body`.
    fn framed(&self) -> Result<Option<(u8, usize)>> {
        let waiting = self.inbox.get(self.start..self.end).unwrap_or_default();
        let Some(head) = waiting.get(..HEADER) else { return Ok(None) };
        let head = <[u8; HEADER]>::try_from(head).map_err(|e| cannot_receive(&e))?;
        let (kind, length) = turborel_protocol::header(head, self.max_body)?;
        let len = HEADER.checked_add(length).ok_or_else(|| cannot_receive(&"a message too long"))?;
        Ok((waiting.len() >= len).then_some((kind, len)))
    }

    /// The body of the next request, whole at `len` bytes with its header.
    fn body(&self, len: usize) -> &[u8] {
        self.inbox.get(self.start.saturating_add(HEADER)..self.start.saturating_add(len)).unwrap_or_default()
    }

    /// Takes the next request's `len` bytes: the inbox, emptied, shrinks
    /// back to `INBOX`.
    fn consume(&mut self, len: usize) {
        self.start = self.start.saturating_add(len);
        if self.start == self.end {
            (self.start, self.end) = (0, 0);
            if self.inbox.len() > INBOX {
                self.inbox.truncate(INBOX);
                self.inbox.shrink_to_fit();
            }
        }
    }

    /// Makes room past `end` for the next bytes: the inbox compacted, or
    /// grown by at most `RESERVED`, if it is full.
    fn make_room(&mut self) {
        if self.start == self.end {
            (self.start, self.end) = (0, 0);
        }
        if self.end < self.inbox.len() {
            return;
        }
        if self.start > 0 {
            self.inbox.copy_within(self.start..self.end, 0);
            self.end = self.end.saturating_sub(self.start);
            self.start = 0;
        } else {
            let more = self.lacking().clamp(INBOX, RESERVED);
            self.inbox.resize(self.inbox.len().saturating_add(more), 0);
        }
    }

    /// The bytes the request begun in the inbox lacks, if its header has
    /// come; else `INBOX`.
    fn lacking(&self) -> usize {
        let head = self.inbox.get(self.start..self.start.saturating_add(HEADER)).and_then(|h| <[u8; HEADER]>::try_from(h).ok());
        let Some(Ok((_, length))) = head.map(|head| turborel_protocol::header(head, self.max_body)) else { return INBOX };
        HEADER.saturating_add(length).saturating_sub(self.end.saturating_sub(self.start))
    }

    /// Encodes `replies` as the next write.
    fn encode(&mut self, replies: &[Reply]) {
        self.out.clear();
        for reply in replies {
            reply.encode(&mut self.out);
        }
        self.sent = 0;
    }

    /// Writes `replies` apart from the runtime, as much as the transport
    /// takes at once, or within its wait: whether all of it; the rest is
    /// for `send_rest`. None of it if the transport cannot be written so.
    pub(super) fn sent_now(&mut self, replies: &[Reply]) -> Result<bool> {
        self.encode(replies);
        let Wire { transport, out, sent, .. } = self;
        let Some(direct) = transport.direct() else { return Ok(false) };
        while *sent < out.len() {
            let written = direct.write(out.get(*sent..).unwrap_or_default());
            match written {
                Ok(0) => return Err(cannot_send(&"the connection takes nothing")),
                Ok(n) => *sent = sent.saturating_add(n),
                Err(e) => {
                    if nothing_yet(&e) {
                        return Ok(false);
                    }
                    if e.kind() != ErrorKind::Interrupted {
                        return Err(cannot_send(&e));
                    }
                }
            }
        }
        Ok(true)
    }

    /// Sends the rest of the replies encoded last, through the runtime.
    pub(super) async fn send_rest(&mut self) -> Result<()> {
        let rest = self.out.get(self.sent..).unwrap_or_default();
        self.transport.write_all(rest).await.map_err(|e| cannot_send(&e))?;
        self.sent = self.out.len();
        self.transport.flush().await.map_err(|e| cannot_send(&e))
    }

    /// Sends `replies` at once, in one write if they fit one.
    pub(super) async fn send_all(&mut self, replies: &[Reply]) -> Result<()> {
        self.encode(replies);
        self.send_rest().await
    }

    /// Sends `reply` now.
    pub(super) async fn send_now(&mut self, reply: &Reply) -> Result<()> {
        self.send_all(std::slice::from_ref(reply)).await
    }

    /// Closes the connection, its replies sent.
    pub(super) async fn close(&mut self) {
        // The connection ends whatever happens: a client gone hears nothing.
        let _ = self.transport.shutdown().await;
    }

    /// Tells the client of `why`, if it still listens, and closes.
    pub(super) async fn close_with(&mut self, why: &SqlError) {
        // Told if it still listens: a client gone hears nothing.
        let _ = self.send_now(&Reply::Error(Failure::from(why))).await;
        self.close().await;
    }

    /// Tells the client of `why`, closes its side, and takes what the
    /// client sends until it closes too, or `within` runs out: a
    /// connection closed with a message unread is reset, and a reset loses
    /// what was sent before it on Windows, the refusal with it; taken, the
    /// close is clean, and the client reads why. A client that has read its
    /// refusal closes at once, so this waits on none but one that neither
    /// sends nor closes.
    pub(super) async fn close_refusing(&mut self, why: &SqlError, within: Duration) {
        self.close_with(why).await;
        let mut sink = [0_u8; DRAINED];
        let drained = async {
            while let Ok(n) = self.transport.read(&mut sink).await {
                if n == 0 {
                    break;
                }
            }
        };
        // Waited for only this long: a client that never closes is dropped.
        let _ = tokio::time::timeout(within, drained).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wire of one end of a pipe of `capacity` bytes, taking bodies of
    /// at most `max_body`, and the other end.
    fn piped(capacity: usize, max_body: u32) -> (Wire, tokio::io::DuplexStream) {
        let (near, far) = tokio::io::duplex(capacity);
        (Wire::new(Box::new(near), max_body, TypeRegistry::default()), far)
    }

    /// `request`, encoded.
    fn encoded(request: &Request) -> Vec<u8> {
        let mut out = Vec::new();
        request.encode(&mut out);
        out
    }

    /// Verifies: [DESIGN] RULES.md D42: a request is read whole from the
    /// bytes it arrives in, however they are split, and the next sent with
    /// it is found arrived, taken if asked for and left if not, or if it is
    /// not whole; a wire that no thread reads itself reads nothing apart
    /// from the runtime, and sends nothing so, the runtime sending it all.
    #[test]
    fn requests_are_framed_from_the_bytes_that_arrive() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let (mut wire, mut far) = piped(1024, 1024);
            let mut bytes = encoded(&Request::Query { sql: "SELECT 1".into(), ends: false });
            bytes.extend(encoded(&Request::Terminate));
            bytes.extend(encoded(&Request::Fetch { statement: 1, rows: 2 }));
            let rest = bytes.split_off(3);
            far.write_all(&bytes).await.unwrap();
            let far = tokio::spawn(async move {
                far.write_all(&rest).await.unwrap();
                far
            });
            assert!(matches!(wire.receive().await.unwrap(), Request::Query { sql, .. } if sql == "SELECT 1"));
            assert!(!wire.is_direct());
            assert!(!wire.fill_within(Duration::from_millis(1)).unwrap(), "not read apart from the runtime");
            assert!(matches!(wire.arrived(|_| None::<()>), Arrived::Left), "whole, not taken");
            let terminate = wire.arrived(|request| matches!(request, Request::Terminate).then_some(()));
            assert!(matches!(terminate, Arrived::Taken(())));
            assert!(matches!(wire.receive().await.unwrap(), Request::Fetch { statement: 1, rows: 2 }));
            assert!(matches!(wire.arrived(|_| Some(())), Arrived::Awaited), "nothing there");
            assert!(!wire.sent_now(&[Reply::Suspended]).unwrap(), "not written apart from the runtime");
            wire.send_rest().await.unwrap();
            let mut far = far.await.unwrap();
            let mut expected = Vec::new();
            Reply::Suspended.encode(&mut expected);
            let mut got = vec![0; expected.len()];
            far.read_exact(&mut got).await.unwrap();
            assert_eq!(got, expected);
            wire.close().await;
            assert_eq!(far.read(&mut got).await.unwrap(), 0, "closed");
        });
    }

    /// Whether `receiving` is still waiting after a moment.
    async fn still_waiting(receiving: impl Future<Output = Result<Request>>) -> bool {
        tokio::time::timeout(Duration::from_millis(50), receiving).await.is_err()
    }

    /// Verifies: [DESIGN] RULES.md D42: a body longer than
    /// SERVER.MAX_MESSAGE_BYTES is refused by its header, before it is
    /// read; a long body allowed is taken as its bytes arrive, the inbox
    /// grown by at most `RESERVED` at a time, never reserved whole, and
    /// shrunk back once the request is taken; a client closing within a
    /// message fails the connection.
    #[test]
    fn a_body_is_taken_as_it_arrives_never_reserved_whole() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let (mut short, mut client) = piped(1 << 20, 16);
            let long = Request::Query { sql: "x".repeat(200_000), ends: false };
            let bytes = encoded(&long);
            client.write_all(&bytes[..HEADER]).await.unwrap();
            let refused = short.receive().await.unwrap_err();
            assert!(refused.msg.contains("longer than"), "{}", refused.msg);
            let (mut wire, mut far) = piped(1 << 20, u32::MAX);
            far.write_all(&bytes[..RESERVED]).await.unwrap();
            assert!(still_waiting(wire.receive()).await);
            assert!(wire.inbox.len() <= bytes.len() && wire.inbox.len() < 2 * RESERVED, "grown by steps: {}", wire.inbox.len());
            far.write_all(&bytes[RESERVED..]).await.unwrap();
            assert!(matches!(wire.receive().await.unwrap(), Request::Query { sql, .. } if sql.len() == 200_000));
            assert_eq!(wire.inbox.len(), INBOX, "shrunk back");
            far.write_all(&bytes[..HEADER + 10]).await.unwrap();
            drop(far);
            assert!(wire.receive().await.unwrap_err().msg.contains("closed within a message"));
        });
    }
}

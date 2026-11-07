//! A connection's messages, framed: requests read, replies written and
//! flushed. The reader stays with the connection's task; the writer is
//! lent with the session to the worker running a request, so the replies
//! are written by the thread that made them (`native/lent.rs`).
//!
//! A request's body is read into memory as its bytes arrive, never
//! reserved whole as its header declares it: a client that declares a
//! long body and sends little of it holds little.

use std::pin::Pin;
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter, ReadHalf, WriteHalf};
use turborel::error::{Result, SqlError};
use turborel::value::types::TypeRegistry;
use turborel_protocol::{Failure, HEADER, Reply, Request, broken};

use crate::transport::Transport;

/// \[DESIGN\] The most of a body reserved before its bytes arrive; past
/// it, the buffer grows as they do.
const RESERVED: usize = 64 * 1024;

/// A connection's requests.
pub(super) struct Wire {
    reader: BufReader<ReadHalf<Box<dyn Transport>>>,
    max_body: u32,
    /// The types beyond the standard a request's values may be of.
    types: TypeRegistry,
}

/// A connection's replies.
pub(super) struct Writer {
    writer: BufWriter<WriteHalf<Box<dyn Transport>>>,
    out: Vec<u8>,
}

impl Wire {
    /// The messages of `transport`: its requests, of at most `max_body`
    /// bytes, and its replies.
    pub(super) fn new(transport: Box<dyn Transport>, max_body: u32, types: TypeRegistry) -> (Wire, Writer) {
        let (reader, writer) = tokio::io::split(transport);
        (Wire { reader: BufReader::new(reader), max_body, types }, Writer { writer: BufWriter::new(writer), out: Vec::new() })
    }

    /// Takes requests of at most `max_body` bytes from now on.
    pub(super) fn set_max_body(&mut self, max_body: u32) {
        self.max_body = max_body;
    }

    /// The client's next request.
    pub(super) async fn receive(&mut self) -> Result<Request> {
        let mut head = [0_u8; HEADER];
        self.reader.read_exact(&mut head).await.map_err(|e| broken(format!("[DESIGN] cannot receive: {e}")))?;
        let (kind, length) = turborel_protocol::header(head, self.max_body)?;
        let mut body = Vec::with_capacity(length.min(RESERVED));
        let wanted = u64::try_from(length).unwrap_or(u64::MAX);
        let read = (&mut self.reader).take(wanted).read_to_end(&mut body).await;
        read.map_err(|e| broken(format!("[DESIGN] cannot receive: {e}")))?;
        if body.len() != length {
            return Err(broken("[DESIGN] cannot receive: the connection closed within a message"));
        }
        Request::decode_with(kind, &body, &self.types)
    }

    /// The next request, if it has come whole and `take` gives something
    /// of it: a client's requests sent together, read without waiting.
    /// One that `take` refuses, or that is malformed, stays to be received,
    /// so its failure comes in its turn.
    pub(super) fn arrived<T>(&mut self, take: impl FnOnce(Request) -> Option<T>) -> Option<T> {
        let waiting = self.reader.buffer();
        let head = <[u8; HEADER]>::try_from(waiting.get(..HEADER)?).ok()?;
        let (kind, length) = turborel_protocol::header(head, self.max_body).ok()?;
        let end = HEADER.checked_add(length)?;
        let taken = take(Request::decode_with(kind, waiting.get(HEADER..end)?, &self.types).ok()?)?;
        Pin::new(&mut self.reader).consume(end);
        Some(taken)
    }

    /// Takes what the client sends until it closes, or `within` runs out.
    async fn drained(&mut self, within: Duration) {
        let mut sink = [0_u8; 1024];
        let drained = async {
            while let Ok(n) = self.reader.read(&mut sink).await {
                if n == 0 {
                    break;
                }
            }
        };
        let _ = tokio::time::timeout(within, drained).await;
    }
}

impl Writer {
    /// Writes `reply`, to be sent at the next flush.
    pub(super) async fn send(&mut self, reply: &Reply) -> Result<()> {
        self.out.clear();
        reply.encode(&mut self.out);
        self.writer.write_all(&self.out).await.map_err(|e| broken(format!("[DESIGN] cannot send: {e}")))
    }

    /// Sends `replies` at once, in one write if they fit one.
    pub(super) async fn send_all(&mut self, replies: &[Reply]) -> Result<()> {
        self.out.clear();
        for reply in replies {
            reply.encode(&mut self.out);
        }
        self.writer.write_all(&self.out).await.map_err(|e| broken(format!("[DESIGN] cannot send: {e}")))?;
        self.flush().await
    }

    /// Sends what was written.
    pub(super) async fn flush(&mut self) -> Result<()> {
        self.writer.flush().await.map_err(|e| broken(format!("[DESIGN] cannot send: {e}")))
    }

    /// Sends `reply` now.
    pub(super) async fn send_now(&mut self, reply: &Reply) -> Result<()> {
        self.send(reply).await?;
        self.flush().await
    }

    /// Closes the connection, its replies sent.
    pub(super) async fn close(&mut self) {
        // The connection ends whatever happens: a client gone hears nothing.
        let _ = self.writer.shutdown().await;
    }

    /// Tells the client of `why`, if it still listens, and closes.
    pub(super) async fn close_with(&mut self, why: &SqlError) {
        let _ = self.send_now(&Reply::Error(Failure::from(why))).await;
        self.close().await;
    }
}

/// Tells the client of `why`, closes its side, and takes what the client
/// sends until it closes too, or `within` runs out: a connection closed
/// with a message unread is reset, and a reset loses what was sent before
/// it on Windows, the refusal with it; taken, the close is clean, and the
/// client reads why. A client that has read its refusal closes at once,
/// so this waits on none but one that neither sends nor closes.
pub(super) async fn close_refusing(wire: &mut Wire, writer: &mut Writer, why: &SqlError, within: Duration) {
    writer.close_with(why).await;
    wire.drained(within).await;
}

//! A connection's messages, framed: requests read, replies written and
//! flushed. Split from native.rs (RULES.md R-SIZE-4).

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter, ReadHalf, WriteHalf};
use turborel::error::{Result, SqlError};
use turborel::value::types::TypeRegistry;
use turborel_protocol::{Failure, HEADER, Reply, Request, broken};

use crate::transport::Transport;

/// A connection's messages.
pub(super) struct Wire {
    reader: BufReader<ReadHalf<Box<dyn Transport>>>,
    writer: BufWriter<WriteHalf<Box<dyn Transport>>>,
    max_body: u32,
    out: Vec<u8>,
    /// The types beyond the standard a request's values may be of.
    types: TypeRegistry,
}

impl Wire {
    /// The messages of `transport`, taking requests of at most `max_body`
    /// bytes.
    pub(super) fn new(transport: Box<dyn Transport>, max_body: u32, types: TypeRegistry) -> Wire {
        let (reader, writer) = tokio::io::split(transport);
        Wire { reader: BufReader::new(reader), writer: BufWriter::new(writer), max_body, out: Vec::new(), types }
    }

    /// The client's next request.
    pub(super) async fn receive(&mut self) -> Result<Request> {
        let mut head = [0_u8; HEADER];
        self.reader.read_exact(&mut head).await.map_err(|e| broken(format!("[DESIGN] cannot receive: {e}")))?;
        let (kind, length) = turborel_protocol::header(head, self.max_body)?;
        let mut body = vec![0_u8; length];
        self.reader.read_exact(&mut body).await.map_err(|e| broken(format!("[DESIGN] cannot receive: {e}")))?;
        Request::decode_with(kind, &body, &self.types)
    }

    /// Writes `reply`, to be sent at the next flush.
    pub(super) async fn send(&mut self, reply: &Reply) -> Result<()> {
        self.out.clear();
        reply.encode(&mut self.out);
        self.writer.write_all(&self.out).await.map_err(|e| broken(format!("[DESIGN] cannot send: {e}")))
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

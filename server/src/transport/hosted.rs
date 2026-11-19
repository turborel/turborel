//! \[EXTENSION\] A host plug-in's listener (RULES.md D254): connections a
//! program takes however it takes them, their bytes relayed in protocol
//! frames, no socket passed between processes.
//!
//! Each connection keeps a process of the program for as long as it is
//! open, so PLUGINS.PROCESSES bounds how many are open at once. ACCEPT is
//! one call that waits in the program for a client. The bytes then pass
//! in RELAY calls, each carrying what the server wrote and answered by
//! what the client sent, or by nothing once the program waited WAIT_MS
//! for it: one call an exchange while the client talks, one each WAIT_MS
//! while it is quiet.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use turborel::bytes::{Put, Reader};
use turborel::error::Result;
use turborel::plugin::hosted::{HostLease, HostLink};
use turborel::plugin::payload::hosted::{LISTENER_ACCEPT, LISTENER_RELAY};

use super::{Accepting, Listener, Transport};
use crate::auth::Peer;

/// How long, in milliseconds, a relay waits in the program for the client
/// when the server has nothing to send.
pub const WAIT_MS: u32 = 50;
/// How long the server is given to answer what the client sent, before
/// the relay waits on the client again.
const ANSWER_WAIT: Duration = Duration::from_millis(50);
/// The bytes a relayed connection buffers each way.
const BUFFER: usize = 1 << 16;

/// A host plug-in's listener.
pub struct HostListener {
    link: HostLink,
}

impl HostListener {
    /// The listener `link` reaches.
    pub fn new(link: HostLink) -> HostListener {
        HostListener { link }
    }
}

impl Listener for HostListener {
    fn accept(&self) -> Accepting<'_> {
        let link = self.link.clone();
        Box::pin(async move {
            let running = link.clone();
            let taken = tokio::task::spawn_blocking(move || accepted(&link)).await.map_err(io::Error::other)?;
            if running.running().is_err() {
                // The node stopped: no connection comes again, and the
                // server is stopped as its operator hears (RULES.md D229).
                std::future::pending::<()>().await;
            }
            let (lease, peer) = taken.map_err(|e| io::Error::other(e.to_string()))?;
            let (ours, theirs) = tokio::io::duplex(BUFFER);
            tokio::spawn(pump(Arc::new(lease), ours));
            let stream: Box<dyn Transport> = Box::new(Relayed(theirs));
            Ok((stream, peer))
        })
    }

    fn address(&self) -> String {
        format!("host plug-in {}.{}", self.link.plugin().manifest.name, self.link.object())
    }
}

/// A connection the program took, on the process kept for it, and who
/// from: a TCP address if the program names one, else this machine.
fn accepted(link: &HostLink) -> Result<(HostLease, Peer)> {
    let lease = link.lease()?;
    let got = lease.call(LISTENER_ACCEPT, Vec::new())?;
    let mut r = Reader::received(&got, "plug-in result");
    let from = r.str()?;
    let mut peer = from.parse::<SocketAddr>().map_or_else(|_| Peer::local(), Peer::tcp);
    peer.secure = false;
    Ok((lease, peer))
}

/// Relays a connection's bytes until either side closes.
async fn pump(lease: Arc<HostLease>, mut ours: DuplexStream) {
    let mut out = vec![0_u8; BUFFER];
    let mut answering = false;
    loop {
        let wait = if answering { ANSWER_WAIT } else { Duration::ZERO };
        let n = match tokio::time::timeout(wait, ours.read(&mut out)).await {
            Ok(Ok(0) | Err(_)) => return,
            Ok(Ok(n)) => n,
            Err(_) => 0,
        };
        let sent = out.get(..n).unwrap_or_default().to_vec();
        let waits = if n == 0 { WAIT_MS } else { 0 };
        let relayed = Arc::clone(&lease);
        let Ok(Ok((closed, back))) = tokio::task::spawn_blocking(move || relay(&relayed, waits, &sent)).await else {
            return;
        };
        answering = !back.is_empty();
        if ours.write_all(&back).await.is_err() || closed {
            return;
        }
    }
}

/// One RELAY: `sent` to the client, after `waits` milliseconds at most
/// for it; whether it closed, and what it sent.
fn relay(lease: &HostLease, waits: u32, sent: &[u8]) -> Result<(bool, Vec<u8>)> {
    let mut payload = Vec::with_capacity(sent.len().saturating_add(8));
    payload.put_u32(waits);
    payload.put_bytes(sent);
    let got = lease.call(LISTENER_RELAY, payload)?;
    let mut r = Reader::received(&got, "plug-in result");
    Ok((r.u8()? == 1, r.bytes()?.to_vec()))
}

/// The server's end of a relayed connection.
struct Relayed(DuplexStream);

impl AsyncRead for Relayed {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for Relayed {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

impl Transport for Relayed {}

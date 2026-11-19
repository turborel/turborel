//! A client that speaks the protocol by hand, for what the client library
//! does not do: requests pipelined, many sent before their replies are
//! read, and replies counted by their frames, not decoded, so the
//! figures are the server's and the transport's.

use std::io::{BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, StreamOwned};
use turborel_protocol::{HEADER, Request, VERSION};

use crate::served::{Kind, Served};

/// A reply's kind that ends a request's replies: Complete.
const COMPLETE: u8 = b'K';
/// Error, which ends them too.
const ERROR: u8 = b'E';
/// Rows.
const ROWS: u8 = b'B';
/// Ready.
const READY: u8 = b'R';
/// Suspended, which ends an Execute's rows when its cursor has more.
const SUSPENDED: u8 = b'S';
/// Prepared, the one reply to Prepare.
const PREPARED: u8 = b'P';

/// A stream of any of the transports.
pub enum Stream {
    Tcp(TcpStream),
    Unix(std::os::unix::net::UnixStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.read(buf),
            Stream::Unix(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.write(buf),
            Stream::Unix(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Tcp(s) => s.flush(),
            Stream::Unix(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

/// A connection spoken to by hand, logged in as X, trusted.
pub struct Raw {
    stream: BufReader<Stream>,
    out: Vec<u8>,
    body: Vec<u8>,
    /// The bytes of the replies read.
    pub bytes: u64,
    /// The rows of the Rows read.
    pub rows: u64,
}

/// What the replies to one request were.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    Complete,
    Error,
    Suspended,
    Prepared,
}

fn tls(tcp: TcpStream, config: &Arc<ClientConfig>) -> Stream {
    let name = ServerName::try_from("localhost").unwrap();
    let connection = ClientConnection::new(Arc::clone(config), name).unwrap();
    Stream::Tls(Box::new(StreamOwned::new(connection, tcp)))
}

impl Raw {
    /// A connection of `kind` to `served`, logged in.
    pub fn connect(served: &Served, kind: Kind) -> Raw {
        let stream = match kind {
            Kind::Tcp | Kind::Tls => {
                let tcp = TcpStream::connect(served.address(kind)).unwrap();
                tcp.set_nodelay(true).unwrap();
                if let Kind::Tls = kind { tls(tcp, &served.config) } else { Stream::Tcp(tcp) }
            }
            Kind::Unix => Stream::Unix(std::os::unix::net::UnixStream::connect(&served.socket).unwrap()),
        };
        let mut raw = Raw { stream: BufReader::with_capacity(64 * 1024, stream), out: Vec::new(), body: Vec::new(), bytes: 0, rows: 0 };
        let hello = Request::Hello { version: VERSION, user: "X".into(), mechanism: "trust".into(), initial: vec![], capabilities: vec![] };
        raw.queue(&hello);
        raw.flush();
        assert_eq!(raw.frame(), READY, "logged in");
        raw
    }

    /// Queues `request`, to be sent with the others at the next flush.
    pub fn queue(&mut self, request: &Request) {
        request.encode(&mut self.out);
    }

    /// Sends what is queued, in one write.
    pub fn flush(&mut self) {
        let stream = self.stream.get_mut();
        stream.write_all(&self.out).unwrap();
        stream.flush().unwrap();
        self.out.clear();
    }

    /// The next reply's kind, its body read and counted.
    fn frame(&mut self) -> u8 {
        let mut head = [0_u8; HEADER];
        self.stream.read_exact(&mut head).unwrap();
        let length = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
        self.body.resize(length, 0);
        self.stream.read_exact(&mut self.body).unwrap();
        self.bytes += (HEADER + length) as u64;
        if head[0] == ROWS {
            self.rows += u64::from(u32::from_le_bytes([self.body[0], self.body[1], self.body[2], self.body[3]]));
        }
        head[0]
    }

    /// Reads the replies to one request, to its last.
    pub fn answered(&mut self) -> Ended {
        loop {
            match self.frame() {
                COMPLETE => return Ended::Complete,
                ERROR => return Ended::Error,
                SUSPENDED => return Ended::Suspended,
                PREPARED => return Ended::Prepared,
                _ => {}
            }
        }
    }

    /// Says Terminate and waits for the server to close.
    pub fn terminate(mut self) {
        self.queue(&Request::Terminate);
        self.flush();
        let mut rest = Vec::new();
        let _ = self.stream.read_to_end(&mut rest);
    }
}

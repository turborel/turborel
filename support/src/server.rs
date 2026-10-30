//! A server for tests to connect to: run on a thread of its own, bound as
//! `turboreld` binds, by its database's settings; and a stream to it, to
//! speak the protocol by hand. With the feature `server`.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::thread::JoinHandle;

use tokio::sync::oneshot;
use turborel::db::Database;
use turborel::value::{Decimal, Value};
use turborel_client::connection::{self, Connection, Trust};
use turborel_protocol::{Failure, HEADER, Reply, Request, header};
use turborel_server::auth::{Authenticator, Trust as Trusted};
use turborel_server::listening;
use turborel_server::native::Native;
use turborel_server::server::{Protocol, Server};
use turborel_server::settings::PORT;

/// Sets `db`'s whole-number setting `name` to `n`.
pub fn set(db: &Database, name: &str, n: i128) {
    db.set_setting(name, &Value::Exact(Decimal::new(n, 0))).unwrap();
}

/// A server running on a thread of its own, on a runtime of its own,
/// listening where its settings say, at 127.0.0.1 on a port the system
/// chose unless they say otherwise; stopped when dropped.
pub struct Running {
    /// Its first TCP address.
    pub address: SocketAddr,
    /// Where it listens.
    pub names: Vec<String>,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

/// A server of `db`, its settings `set` as named, trusting clients of this
/// machine, with `more` authenticators beside; bound as `turboreld` binds,
/// by the settings, which a test may have set before.
pub fn start_with(db: &Database, settings: &[(&str, i128)], more: Vec<Arc<dyn Authenticator>>) -> Running {
    let (stop, stopped) = oneshot::channel::<()>();
    let (tell, told) = std::sync::mpsc::channel();
    let mut server = Server::new(db.clone()).unwrap();
    set(db, PORT, 0);
    for (name, n) in settings {
        set(db, name, *n);
    }
    server.add_authenticator(Arc::new(Trusted)).unwrap();
    for authenticator in more {
        server.add_authenticator(authenticator).unwrap();
    }
    let thread = std::thread::spawn(move || {
        // As turboreld's, of two I/O threads.
        let runtime = turborel_server::runtime::runtime(Some(2)).unwrap();
        runtime.block_on(async move {
            let native: Arc<dyn Protocol> = Arc::new(Native);
            let endpoints = listening::bind(server.database(), &native).await.unwrap();
            tell.send(endpoints.iter().map(|e| e.listener.address()).collect::<Vec<_>>()).unwrap();
            server.serve(endpoints, async { drop(stopped.await) }).await;
        });
    });
    let names = told.recv().unwrap();
    Running { address: names.first().unwrap().parse().unwrap(), names, stop: Some(stop), thread: Some(thread) }
}

/// A server of `db`, its settings `set` as named, trusting clients of this
/// machine.
pub fn start(db: &Database, settings: &[(&str, i128)]) -> Running {
    start_with(db, settings, vec![])
}

impl Running {
    /// A connection as `user`, trusted.
    pub fn connect(&self, user: &str) -> Connection<TcpStream> {
        connection::connect(self.address, user, &mut Trust).unwrap()
    }

    /// A raw stream to the server, to speak the protocol by hand.
    pub fn raw(&self) -> Raw {
        Raw(TcpStream::connect(self.address).unwrap())
    }

    /// Stops the server, and waits until it has.
    pub fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        if let Some(stop) = self.stop.take() {
            // The server may have stopped already.
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.halt();
    }
}

/// A stream to a server, spoken to message by message.
pub struct Raw(pub TcpStream);

impl Raw {
    /// Sends `request`.
    pub fn send(&mut self, request: &Request) {
        let mut bytes = Vec::new();
        request.encode(&mut bytes);
        self.0.write_all(&bytes).unwrap();
    }

    /// Sends `bytes` as they are.
    pub fn send_bytes(&mut self, bytes: &[u8]) {
        self.0.write_all(bytes).unwrap();
    }

    /// The next reply; `None` once the server has closed the connection.
    pub fn receive(&mut self) -> Option<Reply> {
        let mut head = [0_u8; HEADER];
        self.0.read_exact(&mut head).ok()?;
        let (kind, length) = header(head, u32::MAX).unwrap();
        let mut body = vec![0_u8; length];
        self.0.read_exact(&mut body).unwrap();
        Some(Reply::decode(kind, &body).unwrap())
    }

    /// The failure the server sends, then closes the connection after.
    pub fn refusal(&mut self) -> Failure {
        let Some(Reply::Error(failure)) = self.receive() else { panic!("no error") };
        assert!(self.receive().is_none(), "closed after");
        failure
    }

    /// Says Hello as `user`, trusted, and takes Ready.
    pub fn hello(&mut self, user: &str) {
        self.send(&Request::Hello { version: 1, user: user.into(), mechanism: "trust".into(), initial: vec![], capabilities: vec![] });
        assert!(matches!(self.receive(), Some(Reply::Ready { .. })));
    }
}

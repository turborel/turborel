//! A client's limits: how long it waits on a server to connect and log
//! in, for a reply, and for Terminate's close; and the longest reply it
//! takes.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::time::Duration;

use turborel_client::connection::{self, Connection, Trust};
use turborel_client::target::{Target, connect_to_within};
use turborel_client::timeouts::Timeouts;
use turborel_protocol::{CancelKey, Reply};

use crate::common::{start, state};

/// \[DESIGN\] The limit the tests wait by: short, as nothing is sent.
const SHORT: Duration = Duration::from_millis(100);

/// A server of one connection that reads each message and answers the
/// first `answered` of them with Ready, then answers nothing more, nor
/// closes, until the client has gone.
fn stalling(answered: usize) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let ready = Reply::Ready { authorization: "X".into(), key: CancelKey([0; 16]), capabilities: vec![], server: turborel::product() };
        let mut seen = 0;
        let mut head = [0_u8; 5];
        while stream.read_exact(&mut head).is_ok() {
            let mut body = vec![0_u8; u32::from_le_bytes(head[1..].try_into().unwrap()) as usize];
            stream.read_exact(&mut body).unwrap();
            seen += 1;
            if seen <= answered {
                let mut bytes = Vec::new();
                ready.encode(&mut bytes);
                stream.write_all(&bytes).unwrap();
            }
        }
    });
    address
}

/// Verifies: [DESIGN] RULES.md D42: a client waits on its server as its
/// `Timeouts` say: a server that never answers Hello fails the connection
/// within `connect` (SQLSTATE 08006, saying the server did not answer in
/// time); one that never answers a statement fails it within `read`, the
/// connection lost; and Terminate unanswered, nor closed, gives up within
/// `read`. A read timeout changed on a connection takes effect at once.
#[test]
fn a_client_waits_as_its_timeouts_say() {
    let quick = Timeouts { connect: Some(SHORT), read: Some(SHORT) };
    let refused = connect_to_within(&Target::Tcp(stalling(0)), "X", &mut Trust, quick).err().unwrap();
    assert_eq!(state(&refused), ("08006", -919));
    assert!(refused.message.contains("did not answer in time"), "{}", refused.message);
    let mut c = connect_to_within(&Target::Tcp(stalling(1)), "X", &mut Trust, quick).unwrap();
    let late = c.query("VALUES 1").unwrap_err();
    assert!(late.message.contains("did not answer in time"), "{}", late.message);
    assert!(c.is_lost());
    let c = connect_to_within(&Target::Tcp(stalling(1)), "X", &mut Trust, quick).unwrap();
    assert!(c.terminate().unwrap_err().message.contains("did not answer in time"));
    let mut c = connection::connect(stalling(1), "X", &mut Trust).unwrap();
    c.set_read_timeout(Some(SHORT)).unwrap();
    assert!(c.query("VALUES 1").unwrap_err().message.contains("did not answer in time"));
}

/// Verifies: [DESIGN] RULES.md D42: a connection made over a stream its
/// program gives is bounded by its maker: its read timeout is not the
/// client's to set. A reply longer than the connection takes, as
/// `set_max_body` says, loses it (SQLSTATE 08006).
#[test]
fn a_client_takes_replies_as_long_as_it_says() {
    let db = turborel::db::Database::new();
    let server = start(&db, &[]);
    let stream = std::net::TcpStream::connect(server.address).unwrap();
    let mut given = Connection::open(stream, "X", &mut Trust).unwrap();
    assert_eq!(state(&given.set_read_timeout(Some(SHORT)).unwrap_err()).0, "08006");
    assert_eq!(crate::common::rows(&given.query("VALUES 1").unwrap()), ["1"], "it goes on");
    given.set_max_body(8);
    let long = given.query("VALUES 'a string longer than eight bytes'").unwrap_err();
    assert_eq!(state(&long), ("08006", -919));
    assert!(long.message.contains("longer than the 8 taken"), "{}", long.message);
    assert!(given.is_lost());
}

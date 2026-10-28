//! Authentication: mechanisms by name, exchanges of several steps, and
//! refusals, each with its SQLSTATE.

use std::sync::Arc;

use turborel::db::Database;
use turborel::error::Code;
use turborel_client::connection::{self, Mechanism, Trust};
use turborel_protocol::{Failure, Request};
use turborel_server::auth::{Authenticator, Exchange, Peer, Step, Trust as Trusted};
use turborel_server::server::Server;
use turborel_server::settings::WORKERS;

use crate::common::{start, start_with, state};

/// An authenticator, `echo`, that challenges a client to send back a
/// nonce followed by its user name, then accepts it as that user in lower
/// case. A test-local plug-in (RULES.md D6).
struct Echo;

struct Echoing {
    user: String,
    challenged: bool,
}

impl Authenticator for Echo {
    fn mechanism(&self) -> &'static str {
        "echo"
    }
    fn begin(&self, user: &str, _: &Peer) -> Box<dyn Exchange> {
        Box::new(Echoing { user: user.into(), challenged: false })
    }
}

impl Exchange for Echoing {
    fn step(&mut self, input: &[u8]) -> Step {
        if !self.challenged {
            self.challenged = true;
            return Step::Challenge(b"nonce".to_vec());
        }
        if input == [b"nonce".as_slice(), self.user.as_bytes()].concat() {
            return Step::Accept(self.user.to_lowercase());
        }
        Step::Refuse("wrong".into())
    }
}

/// The client's side of `echo`, answering with `prefix`.
struct Echoer {
    prefix: &'static [u8],
    user: &'static str,
}

impl Mechanism for Echoer {
    fn name(&self) -> &'static str {
        "echo"
    }
    fn initial(&mut self) -> Vec<u8> {
        Vec::new()
    }
    fn answer(&mut self, challenge: &[u8]) -> connection::Result<Vec<u8>> {
        assert_eq!(challenge, b"nonce");
        Ok([self.prefix, self.user.as_bytes()].concat())
    }
}

/// Verifies: [DESIGN] RULES.md D42: a mechanism of challenge and response
/// takes as many steps as it needs, each answered by the client; the
/// session's authorization identifier is the one it accepts, not the
/// one the client said; a wrong answer is refused (SQLSTATE 28000), and
/// the connection closed; a challenge not answered breaks the protocol
/// (08006).
#[test]
fn a_mechanism_may_challenge_the_client() {
    let db = Database::new();
    let server = start_with(&db, &[(WORKERS, 1)], vec![Arc::new(Echo)]);
    let c = connection::connect(server.address, "ann", &mut Echoer { prefix: b"nonce", user: "ANN" }).unwrap();
    assert_eq!(c.authorization(), "ann", "the identifier it accepts");
    let refused = connection::connect(server.address, "ann", &mut Echoer { prefix: b"other", user: "ANN" }).err().unwrap();
    assert_eq!(state(&refused), ("28000", -919));
    assert!(refused.ends_connection());
    let mut raw = server.raw();
    raw.send(&Request::Hello { version: 1, user: "ann".into(), mechanism: "echo".into(), initial: vec![], capabilities: vec![] });
    assert!(matches!(raw.receive(), Some(turborel_protocol::Reply::Challenge(_))));
    raw.send(&Request::Query("SELECT 1".into()));
    assert_eq!(state(&raw.refusal()), ("08006", -919), "a challenge is answered");
}

/// Verifies: [DESIGN] RULES.md D42: a client is refused, the connection
/// then closed, for a mechanism the server does not serve or a user not
/// named (SQLSTATE 28000), and for a protocol version it does not speak
/// (08004).
#[test]
fn a_client_is_refused_saying_why() {
    let db = Database::new();
    let server = start(&db, &[(WORKERS, 1)]);
    let hello = |version, user: &str, mechanism: &str| Request::Hello {
        version,
        user: user.into(),
        mechanism: mechanism.into(),
        initial: vec![],
        capabilities: vec![],
    };
    for (request, expected) in [(hello(1, "X", "kerberos"), "28000"), (hello(1, "", "trust"), "28000"), (hello(2, "X", "trust"), "08004")] {
        let mut raw = server.raw();
        raw.send(&request);
        let failure = raw.refusal();
        assert_eq!(state(&failure), (expected, -919), "{request:?}: {failure}");
    }
}

/// Verifies: [DESIGN] RULES.md D42: a client whose mechanism has no answer
/// to a challenge gives the connection up: trust, challenged by a server
/// whose `trust` is another's.
#[test]
fn a_client_without_an_answer_gives_up() {
    struct Impostor;
    impl Authenticator for Impostor {
        fn mechanism(&self) -> &'static str {
            "trust"
        }
        fn begin(&self, user: &str, peer: &Peer) -> Box<dyn Exchange> {
            Echo.begin(user, peer)
        }
    }
    let db = Database::new();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let tcp = runtime.block_on(turborel_server::transport::Tcp::bind("127.0.0.1:0".parse().unwrap())).unwrap();
    let address = tcp.local_addr().unwrap();
    let mut server = Server::new(db).unwrap();
    server.add_authenticator(Arc::new(Impostor)).unwrap();
    let listener: Box<dyn turborel_server::transport::Listener> = Box::new(tcp);
    let endpoint = turborel_server::server::Endpoint { listener, protocol: Arc::new(turborel_server::native::Native), tls: None };
    let serving = runtime.spawn(server.serve(vec![endpoint], async { drop(stopped.await) }));
    let given_up: Failure = connection::connect(address, "X", &mut Trust).err().unwrap();
    assert_eq!(state(&given_up), ("08006", -919));
    stop.send(()).unwrap();
    runtime.block_on(serving).unwrap();
}

/// Verifies: [DESIGN] RULES.md D42: a server has one authenticator for
/// each mechanism: a second is refused (SQLCODE -910).
#[test]
fn a_mechanism_has_one_authenticator() {
    let mut server = Server::new(Database::new()).unwrap();
    server.add_authenticator(Arc::new(Trusted)).unwrap();
    assert_eq!(server.add_authenticator(Arc::new(Trusted)).err().map(|e| e.code), Some(Code::Api));
    server.add_authenticator(Arc::new(Echo)).unwrap();
}

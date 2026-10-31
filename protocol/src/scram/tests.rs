//! Unit tests of SCRAM-SHA-256: RFC 7677's example exchange, each side's
//! messages checked against it; and messages malformed, tampered with or
//! replayed refused (RULES.md R-TEST-1, R-TEST-2). Split from scram.rs
//! (RULES.md R-SIZE-4).

use super::*;

/// RFC 7677 §3's example: its user, password, nonces, salt and iterations.
const USER: &str = "user";
const PASSWORD: &str = "pencil";
const CLIENT_NONCE: &str = "rOprNGfwEbeRWgbNEkqO";
const SERVER_NONCE: &str = "%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0";
const SALT: &str = "W22ZaJ0SNY7soEsUEjb6gQ==";
const ITERATIONS: NonZeroU32 = NonZeroU32::new(4096).unwrap();

/// The keys a server keeps of `password`, with RFC 7677's salt.
fn kept(password: &str) -> Keys {
    Keys::of(&salted_password(password, &decode(SALT).unwrap(), ITERATIONS))
}

/// The server's side of an exchange begun by `first`, of RFC 7677's server
/// nonce and salt, its keys of `password`.
fn server(first: &[u8], password: &str) -> Server {
    Server::begin(First::read(first, PLAIN).unwrap(), SERVER_NONCE, (&decode(SALT).unwrap(), ITERATIONS), kept(password))
}

/// Verifies: [DESIGN] RFC 7677 §3, RFC 5802 §3: the example exchange of
/// SCRAM-SHA-256, each message as the RFC gives it: client-first,
/// server-first, client-final with its proof, and server-final with the
/// server's signature, which the client checks. Neither side shows a
/// password or a key.
#[test]
fn the_rfc_7677_example_runs_as_printed() {
    let mut client = Client::new(USER, PASSWORD);
    let first = client.first(CLIENT_NONCE);
    assert_eq!(String::from_utf8(first.clone()).unwrap(), "n,,n=user,r=rOprNGfwEbeRWgbNEkqO");
    let server = server(&first, PASSWORD);
    let server_first = server.first();
    assert_eq!(
        String::from_utf8(server_first.clone()).unwrap(),
        "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096"
    );
    let client_final = client.answer(&server_first).unwrap();
    assert_eq!(
        String::from_utf8(client_final.clone()).unwrap(),
        "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ="
    );
    let server_final = server.finish(&client_final).unwrap();
    assert_eq!(String::from_utf8(server_final.clone()).unwrap(), "v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=");
    assert!(!client.verified());
    client.verify(&server_final).unwrap();
    assert!(client.verified());
    let shown = format!("{client:?} {server:?} {:?}", kept(PASSWORD));
    assert!(!shown.contains(PASSWORD) && shown.contains("Keys(..)"), "{shown}");
}

/// Verifies: [DESIGN] RULES.md D43: a wrong password's proof is refused;
/// so is a proof tampered with, and a client-final replayed into another
/// exchange, whose server nonce differs (SQLSTATE 28000). A salted
/// password matches the keys of its password alone.
#[test]
fn a_wrong_tampered_or_replayed_proof_is_refused() {
    let mut client = Client::new(USER, "pencils");
    let first = client.first(CLIENT_NONCE);
    let wrong = client.answer(&server(&first, PASSWORD).first()).unwrap();
    let refused = server(&first, PASSWORD).finish(&wrong).unwrap_err();
    assert_eq!((refused.sqlstate().as_str(), refused.msg.contains("proof is wrong")), ("28000", true), "{}", refused.msg);
    let mut right = Client::new(USER, PASSWORD);
    let begun = right.first(CLIENT_NONCE);
    let proved = right.answer(&server(&begun, PASSWORD).first()).unwrap();
    let mut tampered = proved.clone();
    let at = tampered.len() - 3;
    tampered[at] = if tampered[at] == b'A' { b'B' } else { b'A' };
    assert!(server(&begun, PASSWORD).finish(&tampered).is_err(), "tampered");
    let other = Server::begin(First::read(&begun, PLAIN).unwrap(), "another", (&decode(SALT).unwrap(), ITERATIONS), kept(PASSWORD));
    assert!(other.finish(&proved).unwrap_err().msg.contains("nonce"), "replayed");
    assert!(server(&begun, PASSWORD).finish(&proved).is_ok(), "in its own exchange");
    let salt = decode(SALT).unwrap();
    assert!(kept(PASSWORD).match_salted(&salted_password(PASSWORD, &salt, ITERATIONS)));
    assert!(!kept(PASSWORD).match_salted(&salted_password("pencils", &salt, ITERATIONS)));
}

/// Verifies: [DESIGN] RULES.md D43: a client does not take a server that
/// does not prove it knows the verifier: a signature of other keys, an
/// error of the server's, a malformed one, or one out of turn.
#[test]
fn a_client_refuses_a_server_that_does_not_prove_itself() {
    let mut client = Client::new(USER, PASSWORD);
    assert!(client.answer(b"r=x,s=AA==,i=1").is_err(), "out of turn");
    assert!(client.verify(b"v=AA==").is_err(), "out of turn");
    let first = client.first(CLIENT_NONCE);
    let impostor = Server::begin(First::read(&first, PLAIN).unwrap(), SERVER_NONCE, (&decode(SALT).unwrap(), ITERATIONS), kept("guess"));
    let answer = client.clone().answer(&impostor.first()).unwrap();
    assert!(impostor.finish(&answer).is_err());
    let mut proved = client.clone();
    proved.answer(&server(&first, PASSWORD).first()).unwrap();
    for wrong in [&b"v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G5="[..], b"e=invalid-proof", b"v=!", b"x=1", &[0xff]] {
        assert!(proved.clone().verify(wrong).is_err(), "{}", String::from_utf8_lossy(wrong));
    }
    assert!(format!("{client:?}").starts_with("Client(user"));
}

/// Verifies: [DESIGN] RFC 5802 §7, RULES.md R-ERR-1: a malformed message
/// of either side is refused, never a panic: a client-first asking for
/// channel binding or another identity, of extensions, of a nonce with a
/// comma or none, of a user with a bare `=`; a server-first whose nonce
/// does not extend the client's, of a salt not base 64, of iterations
/// zero or no number, or of more attributes; a client-final of another
/// header, of no proof, a short proof or more attributes. A `y` header
/// is taken; a user's `=` and `,` are escaped.
#[test]
fn malformed_messages_are_refused() {
    let firsts =
        ["p=tls-unique,,n=u,r=abc", "n,a=admin,n=u,r=abc", "n,,n=u,r=abc,x=1", "n,,n=u,r=", "n,,n=u", "n,,n=a=b,r=abc", "n,,m=1,n=u,r=a"];
    for first in firsts {
        assert!(First::read(first.as_bytes(), PLAIN).is_err(), "{first}");
    }
    assert!(First::read(&[0xff], PLAIN).is_err());
    assert!(First::read(b"y,,n=,r=abc", PLAIN).is_ok(), "a client that could bind, and an empty user");
    assert_eq!(String::from_utf8(Client::new("a=b,c", "x").first("n")).unwrap(), "n,,n=a=3Db=2Cc,r=n");
    assert!(First::read(b"n,,n=a=3Db=2Cc,r=n", PLAIN).is_ok());
    let server_firsts =
        ["r=other,s=AA==,i=1", "r=abc,s=AA==,i=1", "r=abcd,s=!,i=1", "r=abcd,s=AA==,i=0", "r=abcd,s=AA==,i=x", "r=abcd,s=AA==,i=1,x=1"];
    for server_first in server_firsts {
        let mut client = Client::new(USER, PASSWORD);
        client.first("abc");
        assert!(client.answer(server_first.as_bytes()).is_err(), "{server_first}");
    }
    let server = server(b"y,,n=user,r=abc", PASSWORD);
    let both = format!("abc{SERVER_NONCE}");
    let finals = [
        format!("c=biws,r={both},p=AAAA"),
        format!("c=eSws,r={both}"),
        format!("c=eSws,r={both},p=AA=="),
        format!("c=eSws,r={both},x=1,p=AA=="),
        "c=eSws".to_owned(),
    ];
    for client_final in finals {
        assert!(server.finish(client_final.as_bytes()).is_err(), "{client_final}");
    }
    assert!(nonce().unwrap().len() == 24 && is_nonce(&nonce().unwrap()));
}

/// The offer of SCRAM-SHA-256 where there is no channel to bind.
const PLAIN: Offer<'static> = Offer { plus: false, end_point: None };
/// A certificate's hash, as `end_point` gives it.
const HASH: &[u8] = &[7; 32];

/// The server's side of an exchange begun by `first`, of what `offer`
/// says, as `server`'s.
fn offered(first: &[u8], offer: Offer<'_>) -> Server {
    Server::begin(First::read(first, offer).unwrap(), SERVER_NONCE, (&decode(SALT).unwrap(), ITERATIONS), kept(PASSWORD))
}

/// The exchange of `client` with a server of what `offer` says, to the
/// client's final message: the server, client-first and client-final.
fn exchanged(client: &mut Client, offer: Offer<'_>) -> (Server, Vec<u8>, Vec<u8>) {
    let first = client.first(CLIENT_NONCE);
    let server = offered(&first, offer);
    let client_final = client.answer(&server.first()).unwrap();
    (server, first, client_final)
}

/// The binding a client-final gives, `c=`, decoded.
fn bound_of(client_final: &[u8]) -> Vec<u8> {
    let text = String::from_utf8(client_final.to_vec()).unwrap();
    decode(text.split(',').next().unwrap().strip_prefix("c=").unwrap()).unwrap()
}

/// Verifies: [DESIGN] RFC 5802 §6, §7; RFC 5929 §4: SCRAM-SHA-256-PLUS:
/// the client's header is `p=tls-server-end-point`, its `c=` that header
/// then the certificate's hash, which the server checks against its
/// connection's, and the exchange proves. A client bound to another
/// certificate's hash, as one is through a server in the middle presenting
/// its own, is refused, its proof otherwise right. A client of `y` where
/// none is offered proves, its `c=` the header alone.
#[test]
fn a_bound_exchange_proves_on_its_own_channel_alone() {
    let offer = Offer { plus: true, end_point: Some(HASH) };
    let mut client = Client::new(USER, PASSWORD).bound(Channel::EndPoint(HASH.to_vec()));
    let (server, first, client_final) = exchanged(&mut client, offer);
    assert!(first.starts_with(b"p=tls-server-end-point,,n=user,r="), "{}", String::from_utf8_lossy(&first));
    assert_eq!(bound_of(&client_final), [&b"p=tls-server-end-point,,"[..], HASH].concat());
    client.verify(&server.finish(&client_final).unwrap()).unwrap();
    let mut relayed = Client::new(USER, PASSWORD).bound(Channel::EndPoint(vec![8; 32]));
    let (relay, _, relayed_final) = exchanged(&mut relayed, offer);
    let refused = relay.finish(&relayed_final).unwrap_err();
    assert_eq!((refused.sqlstate().as_str(), refused.msg.contains("channel binding")), ("28000", true), "{}", refused.msg);
    let mut unoffered = Client::new(USER, PASSWORD).bound(Channel::Unoffered);
    let (plain, _, unoffered_final) = exchanged(&mut unoffered, PLAIN);
    assert_eq!(bound_of(&unoffered_final), b"y,,");
    unoffered.verify(&plain.finish(&unoffered_final).unwrap()).unwrap();
}

/// A case of a header weighed: the header, whether the mechanism is
/// PLUS, the connection's binding, and the refusal expected, if any.
type Case<'a> = (&'a str, bool, Option<&'a [u8]>, Option<&'a str>);

/// Verifies: [DESIGN] RFC 5802 §6; RFC 5801 §3, §5: a header is weighed
/// against what the server offers: `n` is taken with a channel or
/// without; `y` only where the server offers no binding, else refused as
/// a downgrade; `p=tls-server-end-point` only of SCRAM-SHA-256-PLUS where
/// the connection has a binding; SCRAM-SHA-256-PLUS only with it; another
/// binding type, or another identity, refused (SQLSTATE 28000).
#[test]
fn a_header_is_weighed_against_the_offer() {
    let tls = Some(HASH);
    let cases: [Case<'_>; 11] = [
        ("n,,", false, None, None),
        ("n,,", false, tls, None),
        ("y,,", false, None, None),
        ("y,,", false, tls, Some("a downgrade")),
        ("p=tls-server-end-point,,", true, tls, None),
        ("p=tls-server-end-point,,", true, None, Some("offers no channel binding")),
        ("p=tls-server-end-point,,", false, tls, Some("asks for scram-sha-256-plus")),
        ("n,,", true, tls, Some("binds the channel")),
        ("y,,", true, tls, Some("binds the channel")),
        ("p=tls-unique,,", true, tls, Some("other than tls-server-end-point")),
        ("p=tls-server-end-point,a=x,", true, tls, Some("another identity")),
    ];
    for (header, plus, end_point, refusal) in cases {
        let read = First::read(format!("{header}n=u,r=abc").as_bytes(), Offer { plus, end_point });
        match refusal {
            None => assert!(read.is_ok(), "{header} {plus}"),
            Some(why) => {
                let refused = read.unwrap_err();
                assert!(refused.msg.contains(why) && refused.sqlstate().as_str() == "28000", "{header} {plus}: {}", refused.msg);
            }
        }
    }
    assert!(First::read(b"n,a=x,n=u,r=abc", PLAIN).unwrap_err().msg.contains("another identity"));
}

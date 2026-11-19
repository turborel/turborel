//! Unit tests of the client's SCRAM: HMAC-SHA-256 against RFC 4231's
//! vectors, exchanges checked by the protocol's own server side, and the
//! key cache's hits, misses, bound and forgetting (RULES.md D205).

use std::num::NonZeroU32;

use turborel::plugin::package::hex;
use turborel_protocol::Failure;
use turborel_protocol::scram::{First, Keys, Offer, Server, salted_password};

use super::*;

/// The salt and iterations of the server's verifier in these tests.
const SALT: &[u8] = b"a salt of the server's";
const ITERATIONS: NonZeroU32 = NonZeroU32::new(4096).unwrap();
/// A server's offer over plain TCP: no channel binding.
const PLAIN: Offer<'static> = Offer { plus: false, end_point: None, exporter: None };

/// One login of `scram` against a server whose verifier is of `password`,
/// salted by `salt`: whether it ended with the server proved.
fn login(scram: &mut Scram, password: &str, salt: &[u8]) -> Result<()> {
    let keys = Keys::of(&salted_password(password, salt, ITERATIONS));
    let read = First::read(&scram.initial(), PLAIN).map_err(|e| Failure::from(&e))?;
    let server = Server::begin(read, "servernonce", (salt, ITERATIONS), keys);
    let client_final = scram.answer(&server.first())?;
    let Ok(server_final) = server.finish(&client_final) else {
        scram.refused();
        return Err(refused("the server refused the proof"));
    };
    scram.answer(&server_final)?;
    scram.ready()
}

/// Verifies: [DESIGN] RFC 2104 §2, RFC 4231 §4.2, §4.3, §4.7: HMAC-SHA-256
/// made of the protocol's SHA-256 gives RFC 4231's test cases 1, 2 and 6,
/// the last of a key longer than SHA-256's block, hashed first.
#[test]
fn hmac_gives_the_rfc_4231_vectors() {
    assert_eq!(hex(&exchange::hmac(&[0x0b; 20], b"Hi There")), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    assert_eq!(
        hex(&exchange::hmac(b"Jefe", b"what do ya want for nothing?")),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    let long = exchange::hmac(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First");
    assert_eq!(hex(&long), "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
}

/// Verifies: [DESIGN] RULES.md D43, D205: the client's exchange, proved
/// from derived keys, is taken by the protocol's server side and proves
/// the server in turn; a wrong password's proof is refused (SQLSTATE
/// 28000); the keys show nothing of themselves.
#[test]
fn an_exchange_of_derived_keys_is_taken_by_the_server() {
    login(&mut Scram::new("right"), "right", SALT).unwrap();
    let wrong = login(&mut Scram::new("wrong"), "right", SALT).unwrap_err();
    assert_eq!(wrong.sqlstate, "28000");
    assert_eq!(format!("{:?}", derive("right", SALT, ITERATIONS)), "Derived(..)");
}

/// Verifies: [DESIGN] RULES.md D205: a second login by the same password
/// to a server of the same salt and iterations takes its keys from the
/// cache, a hit; another salt, or another password, misses; a wrong
/// password finds nothing its right one left, and fails; the cache keeps
/// no password, and its clones share it.
#[test]
fn the_key_cache_hits_the_same_salt_and_password_alone() {
    let cache = KeyCache::new();
    let shared = cache.clone();
    login(&mut Scram::new("right").with_cache(&cache), "right", SALT).unwrap();
    assert_eq!((cache.lookups(), cache.len()), ((0, 1), 1));
    login(&mut Scram::new("right").with_cache(&shared), "right", SALT).unwrap();
    assert_eq!(cache.lookups(), (1, 1), "a hit");
    login(&mut Scram::new("right").with_cache(&cache), "right", b"another salt").unwrap();
    assert_eq!((cache.lookups(), cache.len()), ((1, 2), 2), "another salt misses");
    assert!(login(&mut Scram::new("wrong").with_cache(&cache), "right", SALT).is_err());
    assert_eq!((cache.lookups(), cache.len()), ((1, 3), 2), "a miss, and its keys forgotten as it failed");
    assert!(!format!("{cache:?}").contains("right"));
    cache.clear();
    assert!(shared.is_empty());
}

/// Verifies: [DESIGN] RULES.md D205: an exchange whose keys came from the
/// cache and failed, as against a verifier changed under the same salt,
/// forgets them, so the next login derives them again; the cache holds
/// `KEY_CACHE_ENTRIES` at most, dropping its oldest.
#[test]
fn a_failed_exchange_forgets_its_keys_and_the_cache_is_bounded() {
    let cache = KeyCache::new();
    login(&mut Scram::new("old").with_cache(&cache), "old", SALT).unwrap();
    assert!(login(&mut Scram::new("old").with_cache(&cache), "new", SALT).is_err());
    assert_eq!((cache.lookups(), cache.len()), ((1, 1), 0), "a hit, then forgotten");
    for n in 0..=KEY_CACHE_ENTRIES {
        cache.derived("p", format!("salt {n}").as_bytes(), NonZeroU32::MIN);
    }
    assert_eq!(cache.len(), KEY_CACHE_ENTRIES);
    cache.derived("p", b"salt 0", NonZeroU32::MIN);
    assert_eq!(cache.lookups().0, 1, "the oldest was dropped, so salt 0 missed");
}

/// Verifies: [DESIGN] RULES.md D204: under `ChannelBinding::Require` an
/// exchange not bound to TLS is refused before its proof is sent (SQLSTATE
/// 28000); under `Disable` a TLS channel is not bound, the mechanism
/// staying `scram-sha-256`; under `Prefer` it is bound.
#[test]
fn channel_binding_is_disabled_preferred_or_required() {
    let mut required = Scram::new("right").channel_binding(ChannelBinding::Require);
    let first = required.initial();
    let server = Server::begin(First::read(&first, PLAIN).unwrap(), "n", (SALT, ITERATIONS), Keys::of(&[0; 32]));
    assert_eq!(required.answer(&server.first()).unwrap_err().sqlstate, "28000");
    let channel = TlsChannel { certificate: None, exporter: Some(vec![7; 32]) };
    let mut disabled = Scram::new("right").channel_binding(ChannelBinding::Disable);
    disabled.bind(&channel);
    assert_eq!(disabled.name(), scram::MECHANISM);
    let mut preferred = Scram::new("right");
    preferred.bind(&channel);
    assert_eq!(preferred.name(), scram::MECHANISM_PLUS);
}

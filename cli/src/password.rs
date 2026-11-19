//! \[DESIGN\] A password's SCRAM-SHA-256 verifier, made by the client
//! (RULES.md D43, D217), so `\password` and `turborel admin password`
//! send the server what it keeps, never the password:
//!
//! ```text
//! verifier: $scram-sha-256$i=<iterations>$<salt, hex>$<StoredKey, hex>$<ServerKey, hex>
//! salted:   PBKDF2-HMAC-SHA256(password, salt, iterations), 32 bytes   [RFC 8018 §5.2]
//! keys:     StoredKey = SHA-256(HMAC(salted, "Client Key"))             [RFC 5802 §3]
//!           ServerKey = HMAC(salted, "Server Key")
//! ```
//!
//! the server's own layout (`turborel_server::password`). HMAC is RFC
//! 2104's, over the SHA-256 the client already links
//! (`turborel_client::packages`). The salt is 16 bytes of the standard
//! library's per-process random keys, through SipHash, as a salt need be
//! unique, not secret [RFC 5802 §5.1].

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;

use turborel::plugin::package::crypto::PackageCrypto;
use turborel_client::packages::Crypto;

/// The scheme a verifier names, as the server's.
pub const SCHEME: &str = "scram-sha-256";
/// \[DESIGN\] The iterations of PBKDF2: the server's default,
/// SERVER.PASSWORD_ITERATIONS.
pub const ITERATIONS: u32 = 600_000;
/// The bytes of a salt: 128 bits, as NIST SP 800-132 asks at least.
pub const SALT_BYTES: usize = 16;
/// The bytes of a SHA-256 digest.
const DIGEST: usize = 32;
/// The bytes of a block of SHA-256 [RFC 2104 §2].
const BLOCK: usize = 64;
/// HMAC's inner pad [RFC 2104 §2].
const IPAD: u8 = 0x36;
/// HMAC's outer pad [RFC 2104 §2].
const OPAD: u8 = 0x5c;
/// What ClientKey is the HMAC of [RFC 5802 §3].
const CLIENT_KEY: &[u8] = b"Client Key";
/// What ServerKey is the HMAC of [RFC 5802 §3].
const SERVER_KEY: &[u8] = b"Server Key";
/// The index of PBKDF2's one block, big-endian [RFC 8018 §5.2].
const FIRST_BLOCK: [u8; 4] = [0, 0, 0, 1];

/// A SHA-256 digest.
type Digest = [u8; DIGEST];

/// SHA-256 of `bytes`.
fn sha256(bytes: &[u8]) -> Digest {
    Crypto.sha256(bytes)
}

/// [RFC 2104 §2] HMAC-SHA-256 of `message` under `key`.
pub fn hmac(key: &[u8], message: &[u8]) -> Digest {
    let mut block = [0_u8; BLOCK];
    let short = if key.len() > BLOCK { sha256(key).to_vec() } else { key.to_vec() };
    for (b, k) in block.iter_mut().zip(&short) {
        *b = *k;
    }
    let inner: Vec<u8> = block.iter().map(|b| b ^ IPAD).chain(message.iter().copied()).collect();
    let outer: Vec<u8> = block.iter().map(|b| b ^ OPAD).chain(sha256(&inner)).collect();
    sha256(&outer)
}

/// [RFC 8018 §5.2] PBKDF2-HMAC-SHA-256 of `password` and `salt`,
/// `iterations` times, its first block: SaltedPassword, as Hi of RFC 5802
/// §2.2.
pub fn pbkdf2(password: &[u8], salt: &[u8], iterations: u32) -> Digest {
    let first: Vec<u8> = salt.iter().copied().chain(FIRST_BLOCK).collect();
    let mut u = hmac(password, &first);
    let mut out = u;
    for _ in 1..iterations {
        u = hmac(password, &u);
        for (o, x) in out.iter_mut().zip(u) {
            *o ^= x;
        }
    }
    out
}

/// `bytes` in hex, two lower-case digits each.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<String>>().concat()
}

/// A new salt, unique to this process and call.
pub fn salt() -> [u8; SALT_BYTES] {
    let keys = RandomState::new();
    let mut out = [0_u8; SALT_BYTES];
    for (n, chunk) in (0_u64..).zip(out.chunks_mut(size_of::<u64>())) {
        let word = keys.hash_one((n, std::process::id())).to_le_bytes();
        for (o, w) in chunk.iter_mut().zip(word) {
            *o = w;
        }
    }
    out
}

/// The verifier of `password`, of `salt`, at `iterations`.
pub fn verifier(password: &str, salt: &[u8], iterations: u32) -> String {
    let salted = pbkdf2(password.as_bytes(), salt, iterations);
    let stored = sha256(&hmac(&salted, CLIENT_KEY));
    let server = hmac(&salted, SERVER_KEY);
    format!("${SCHEME}$i={iterations}${}${}${}", hex(salt), hex(&stored), hex(&server))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(text: &str) -> Vec<u8> {
        turborel::plugin::package::unhex(text).unwrap()
    }

    /// Verifies: [DESIGN] RULES.md D217: HMAC-SHA-256 gives RFC 4231's
    /// test cases 1 and 2, and one of a key longer than a block, case 6.
    #[test]
    fn hmac_gives_the_rfc_4231_cases() {
        let one = hmac(&[0x0b; 20], b"Hi There");
        assert_eq!(hex(&one), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
        let two = hmac(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(hex(&two), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        let six = hmac(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First");
        assert_eq!(hex(&six), "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
    }

    /// Verifies: [DESIGN] RULES.md D217: PBKDF2-HMAC-SHA-256 gives the
    /// published vectors of "password" and "salt" at 1, 2 and 4096
    /// iterations.
    #[test]
    fn pbkdf2_gives_the_published_vectors() {
        assert_eq!(hex(&pbkdf2(b"password", b"salt", 1)), "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b");
        assert_eq!(hex(&pbkdf2(b"password", b"salt", 2)), "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43");
        assert_eq!(hex(&pbkdf2(b"password", b"salt", 4096)), "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a");
    }

    /// Verifies: [DESIGN] RULES.md D43, D217: a verifier names its scheme
    /// and iterations and holds the salt and both keys in hex, in the
    /// server's layout; salts differ call to call.
    #[test]
    fn verifiers_are_in_the_servers_layout() {
        let salt = unhex("000102030405060708090a0b0c0d0e0f");
        let made = verifier("pencil", &salt, 4096);
        let parts: Vec<&str> = made.split('$').collect();
        assert_eq!(parts[..4], ["", SCHEME, "i=4096", "000102030405060708090a0b0c0d0e0f"]);
        assert_eq!((parts[4].len(), parts[5].len()), (64, 64));
        assert_ne!(salt_bytes(), salt_bytes());
    }

    fn salt_bytes() -> [u8; SALT_BYTES] {
        salt()
    }
}

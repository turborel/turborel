//! Unit tests of ChaCha20-Poly1305: RFC 8439's known answer, the encoding
//! contract (RULES.md R-SAFE-3), data changed, moved or of another cipher
//! refused (R-TEST-2), and a database's files and a table's rows through
//! it (R-TEST-1).

use std::sync::Arc;

use turborel::db::{Database, OpenOptions};
use turborel::disk::encoding::Encoding;
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::Code;
use turborel::value::Value;

use super::{code, encrypted, held};
use crate::{AesGcm, CHACHA_ID, ChaCha, ChaCha20Poly1305, Cipher, KEY_BYTES};

/// [RFC 8439 §2.8.2] The AEAD's example: its plaintext.
const PLAINTEXT: &[u8] =
    b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
/// Its additional authenticated data.
const AAD: [u8; 12] = [0x50, 0x51, 0x52, 0x53, 0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7];
/// Its nonce: the constant 7, then the IV.
const NONCE: [u8; 12] = [0x07, 0x00, 0x00, 0x00, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47];
/// Its ciphertext, then its tag.
const SEALED: &str = concat!(
    "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d6",
    "3dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b36",
    "92ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc",
    "3ff4def08e4b7a9de576d26586cec64b6116",
    "1ae10b594f09e26a7e902ecbd0600691",
);

/// The bytes the hexadecimal digits `hex` write.
pub(super) fn bytes(hex: &str) -> Vec<u8> {
    hex.as_bytes().chunks(2).map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect()
}

/// ChaCha20-Poly1305 with the keys of `held`.
fn chacha() -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(held())
}

/// Verifies: [EXTENSION] RULES.md D26, D48 [RFC 8439 §2.8.2]: sealed with
/// the example's key (0x80 to 0x9f), nonce and additional data, its
/// plaintext gives its ciphertext and tag, byte for byte, and opens back;
/// a byte of either changed, it opens no more.
#[test]
fn it_gives_rfc_8439s_known_answer() {
    let key: [u8; KEY_BYTES] = std::array::from_fn(|n| 0x80_u8 + u8::try_from(n).unwrap());
    let key = ChaCha::key(&key).unwrap();
    let mut sealed = PLAINTEXT.to_vec();
    ChaCha::seal_with(&key, NONCE, &AAD, &mut sealed).unwrap();
    assert_eq!(sealed, bytes(SEALED));
    let mut opened = sealed.clone();
    assert_eq!(ChaCha::open(&key, NONCE, &AAD, &mut opened).unwrap(), PLAINTEXT);
    for at in [0, PLAINTEXT.len(), sealed.len() - 1] {
        let mut changed = sealed.clone();
        changed[at] ^= 1;
        assert!(ChaCha::open(&key, NONCE, &AAD, &mut changed).is_err(), "byte {at}");
    }
}

/// Verifies: [EXTENSION] RULES.md D48, encoding contract: what is
/// encrypted decrypts to itself, empty or long, adding exactly its
/// overhead, with none of it in the clear and a nonce of its own each
/// time; data moved, changed or cut is refused (SQLCODE -916); it is
/// confidential, its id 3 and its name fixed.
#[test]
fn what_is_encrypted_decrypts_to_itself() {
    let chacha = chacha();
    let text = b"a secret row, repeated; ".repeat(100);
    for input in [&b""[..], b"x", &text] {
        let sealed = encrypted(&chacha, 7, "K", input);
        assert_eq!(sealed.len(), input.len() + chacha.overhead("K"));
        let mut out = Vec::new();
        chacha.decode(7, &sealed, &mut out).unwrap();
        assert_eq!(out, input);
    }
    let (one, two) = (encrypted(&chacha, 7, "K", &text), encrypted(&chacha, 7, "K", &text));
    assert_ne!(one, two, "a nonce of its own each time");
    assert!(!one.windows(10).any(|w| w == b"secret row"), "nothing in the clear");
    assert_eq!(code(&chacha, 8, &one), Some(Code::Corrupt), "moved");
    for at in [0, 2, 20, 35, one.len() - 1] {
        let mut changed = one.clone();
        changed[at] ^= 1;
        assert!(matches!(code(&chacha, 7, &changed), Some(Code::Corrupt | Code::Api)), "byte {at}");
    }
    assert_eq!(code(&chacha, 7, &one[..30]), Some(Code::Corrupt), "cut");
    assert!(chacha.confidential());
    assert_eq!((chacha.id(), chacha.name()), (CHACHA_ID, "ChaCha20-Poly1305"));
    assert_eq!(chacha.accepts("M").err().map(|e| e.code), Some(Code::Api), "a key the keyring lacks");
    assert_eq!(chacha.accepts("../K").err().map(|e| e.code), Some(Code::Api), "no key's name");
}

/// Verifies: [EXTENSION] RULES.md D48: one key named by both ciphers
/// gives each subkeys of its own, so what one encrypted the other does not
/// decrypt (SQLCODE -916); with no key named, ChaCha20-Poly1305 encrypts
/// with its own ephemeral key.
#[test]
fn each_cipher_has_subkeys_of_its_own() {
    let (chacha, aes) = (chacha(), AesGcm::new(held()));
    assert_eq!(code(&aes, 0, &encrypted(&chacha, 0, "K", b"row")), Some(Code::Corrupt));
    assert_eq!(code(&chacha, 0, &encrypted(&aes, 0, "K", b"row")), Some(Code::Corrupt));
    let mut spilled = Vec::new();
    chacha.encode(3, b"spilled rows", &mut spilled).unwrap();
    let mut out = Vec::new();
    chacha.decode(3, &spilled, &mut out).unwrap();
    assert_eq!(out, b"spilled rows");
}

/// The database in `dir`, ChaCha20-Poly1305 registered with the keys of
/// `held`, its files passing through it with the key K.
fn encrypted_database(dir: &std::path::Path) -> Database {
    let db = Database::new();
    db.register_encoding(Box::new(chacha())).unwrap();
    db.set_setting("STORAGE.ENCODINGS", &Value::Char("chacha20-poly1305:K".into())).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(OsFileSystem);
    db.open_files(&fs, dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D30, D48: a database created with
/// STORAGE.ENCODINGS naming ChaCha20-Poly1305 and a key writes none of its
/// rows in the clear, and reopens, its rows read back; a table's rows pass
/// through it too, by its option ENCODINGS.
#[test]
fn a_database_is_created_with_it_and_reopened() {
    let scratch = turborel_test_support::Scratch::new("chacha-database");
    let dir = scratch.path("db");
    let db = encrypted_database(&dir);
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER, S VARCHAR(40)) WITH (ENCODINGS = 'ChaCha20-Poly1305:L')").unwrap();
    let s = db.session("S");
    s.run("INSERT INTO T VALUES (1, 'a secret in chacha')").unwrap();
    s.commit().unwrap();
    drop(s);
    db.close().unwrap();
    let mut stored = Vec::new();
    for entry in walk(&dir) {
        stored.extend(std::fs::read(entry).unwrap());
    }
    assert!(!stored.windows(9).any(|w| w == b"a secret "), "nothing in the clear");
    let again = encrypted_database(&dir);
    let reader = again.session("S");
    let mut cursor = reader.open_cursor("SELECT S FROM T").unwrap();
    assert_eq!(cursor.fetch().unwrap().unwrap()[0].to_string(), "a secret in chacha");
    drop((cursor, reader));
    again.close().unwrap();
}

/// Every file under `dir`.
fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}

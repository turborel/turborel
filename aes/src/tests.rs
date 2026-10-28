//! Unit tests of AES-256-GCM: its contract (RULES.md R-SAFE-3), data
//! changed, moved or of another key refused (R-TEST-2), subkeys drawn
//! anew, the key directory, and a table's rows through it end to end
//! (RULES.md R-TEST-1).

use std::collections::HashMap;

use turborel::db::Database;
use turborel::error::Code;

use super::*;

/// A keyring of keys held in memory.
struct Held(HashMap<String, [u8; KEY_BYTES]>);

impl Keyring for Held {
    fn name(&self) -> &'static str {
        "held"
    }
    fn key(&self, name: &str) -> Result<Option<[u8; KEY_BYTES]>> {
        Ok(self.0.get(name).copied())
    }
}

/// AES-256-GCM with the keys K, of 1s, and L, of 2s.
fn aes() -> AesGcm {
    AesGcm::new(Arc::new(Held(HashMap::from([("K".to_owned(), [1_u8; KEY_BYTES]), ("L".to_owned(), [2_u8; KEY_BYTES])]))))
}

fn encrypted(aes: &AesGcm, context: u64, key: &str, input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    aes.encode_with(context, key, input, &mut out).unwrap();
    out
}

fn code(aes: &AesGcm, context: u64, input: &[u8]) -> Option<Code> {
    aes.decode(context, input, &mut Vec::new()).err().map(|e| e.code)
}

/// Verifies: [EXTENSION] RULES.md D26, D48, encoding contract: what is
/// encrypted decrypts to itself, empty or long, adding exactly its
/// overhead; it holds none of its input in the clear, and differs each
/// time; it is confidential, its id and name fixed.
#[test]
fn what_is_encrypted_decrypts_to_itself() {
    let aes = aes();
    let text = b"a secret row, repeated; ".repeat(100);
    for input in [&b""[..], b"x", &text] {
        let sealed = encrypted(&aes, 7, "K", input);
        assert_eq!(sealed.len(), input.len() + aes.overhead("K"));
        let mut out = b"kept".to_vec();
        aes.decode(7, &sealed, &mut out).unwrap();
        assert_eq!(&out[4..], input);
    }
    let (one, two) = (encrypted(&aes, 7, "K", &text), encrypted(&aes, 7, "K", &text));
    assert_ne!(one, two, "a nonce of its own each time");
    assert!(!one.windows(10).any(|w| w == b"secret row"), "nothing in the clear");
    assert!(aes.confidential());
    assert_eq!((aes.id(), aes.name()), (AES_ID, "AES-256-GCM"));
}

/// Verifies: [EXTENSION] RULES.md D48, robustness: data decrypted at
/// another place, any byte of it changed, cut short, of an unknown
/// version, or naming no valid key, is refused (SQLCODE -916); naming a
/// key the keyring lacks, it is refused (SQLCODE -910).
#[test]
fn data_changed_moved_or_cut_is_refused() {
    let aes = aes();
    let sealed = encrypted(&aes, 7, "K", b"payload");
    assert_eq!(code(&aes, 8, &sealed), Some(Code::Corrupt), "moved");
    for at in [2, 3, 20, 35, sealed.len() - 1] {
        let mut changed = sealed.clone();
        changed[at] ^= 1;
        assert!(matches!(code(&aes, 7, &changed), Some(Code::Corrupt | Code::Api)), "byte {at}");
    }
    for cut in [0, 1, 2, 3, 10, 25] {
        assert_eq!(code(&aes, 7, &sealed[..cut]), Some(Code::Corrupt), "cut at {cut}");
    }
    let mut version = sealed.clone();
    version[0] = 9;
    assert_eq!(code(&aes, 7, &version), Some(Code::Corrupt));
    let mut name = sealed.clone();
    name[2] = b'/';
    assert_eq!(code(&aes, 7, &name), Some(Code::Corrupt), "no key's name");
    let other = AesGcm::new(Arc::new(Held(HashMap::new())));
    assert_eq!(code(&other, 7, &sealed), Some(Code::Api), "a key not in the keyring");
}

/// Verifies: [EXTENSION] RULES.md D48: an argument is a key the keyring
/// has: none, one not a key's name, or one it lacks is refused (SQLCODE
/// -910); another key's data is not read as this key's.
#[test]
fn a_key_is_named_and_must_be_known() {
    let aes = aes();
    aes.accepts("K").unwrap();
    for bad in ["", "M", "../K", ".K", &"K".repeat(65)] {
        assert_eq!(aes.accepts(bad).err().map(|e| e.code), Some(Code::Api), "{bad:?}");
    }

    assert_eq!(aes.encode_with(0, "../K", b"x", &mut Vec::new()).err().map(|e| e.code), Some(Code::Api));
    let of_l = encrypted(&aes, 0, "L", b"x");
    let mut as_k = of_l.clone();
    as_k[2] = b'K';
    assert_eq!(code(&aes, 0, &as_k), Some(Code::Corrupt), "the name is authenticated");
}

/// Verifies: [DESIGN] RULES.md D48: a key's writes share a subkey, its
/// salt, until it has been used `SUBKEY_USES` times; then a new salt is
/// drawn, and data of either salt decrypts.
#[test]
fn subkeys_are_drawn_anew() {
    let aes = aes();
    let salt_of = |sealed: &[u8]| sealed[3..3 + SALT].to_vec();
    let first = encrypted(&aes, 0, "K", b"one");
    let second = encrypted(&aes, 0, "K", b"two");
    assert_eq!(salt_of(&first), salt_of(&second));
    aes.current.lock().unwrap().get_mut("K").unwrap().uses = SUBKEY_USES;
    let third = encrypted(&aes, 0, "K", b"three");
    assert_ne!(salt_of(&third), salt_of(&first), "a new salt");
    for (sealed, plain) in [(&first, &b"one"[..]), (&third, b"three")] {
        let mut out = Vec::new();
        aes.decode(0, sealed, &mut out).unwrap();
        assert_eq!(out, plain);
    }
}

/// Verifies: [EXTENSION] RULES.md D48, `KeyDirectory`: a key is a file of
/// 64 hexadecimal digits, a line end allowed; one not there, or of a name
/// that is not a key's, is none; one of other length, or not hexadecimal,
/// or that cannot be read, is refused.
#[test]
fn keys_are_files_of_a_directory() {
    let scratch = turborel_test_support::Scratch::new("aes-keys");
    let dir = scratch.path("keys");
    std::fs::create_dir_all(dir.join("UNREADABLE")).unwrap();
    std::fs::write(dir.join("K"), format!("{}\n", "0f".repeat(32))).unwrap();
    std::fs::write(dir.join("SHORT"), "0f0f").unwrap();
    std::fs::write(dir.join("ODD"), "zz".repeat(32)).unwrap();
    let keys = KeyDirectory::new(&dir);
    assert_eq!(keys.key("K").unwrap(), Some([0x0f; KEY_BYTES]));
    assert_eq!(keys.key("MISSING").unwrap(), None);
    assert_eq!(keys.key("../keys/K").unwrap(), None, "never a path");
    assert_eq!(keys.key("SHORT").err().map(|e| e.code), Some(Code::Api));
    assert_eq!(keys.key("ODD").err().map(|e| e.code), Some(Code::Api));
    assert_eq!(keys.key("UNREADABLE").err().map(|e| e.code), Some(Code::Io));
    assert_eq!(keys.name(), "key directory");
}

/// Verifies: [EXTENSION] RULES.md D41, D48: registered, AES-256-GCM is
/// named with its key by a table's option ENCODINGS, and the table's rows,
/// short and long, read back through it; a key the keyring lacks is refused
/// as the option is set (SQLCODE -910).
#[test]
fn a_tables_rows_pass_through_it() {
    let db = Database::new();
    db.register_encoding(Box::new(aes())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER, S CHARACTER(4000)) WITH (ENCODINGS = 'aes-256-gcm:K')").unwrap();
    let s = db.session("S");
    let long = "words of a secret ".repeat(200);
    s.run(&format!("INSERT INTO T VALUES (1, 'short'), (2, '{long}')")).unwrap();
    let mut cursor = s.open_cursor("SELECT K, S FROM T ORDER BY K").unwrap();
    let mut found = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        found.push(row[1].to_string().trim_end().to_owned());
    }
    assert_eq!(found, vec!["short".to_owned(), long.trim_end().to_owned()]);
    drop(cursor);
    let refused = db.schema("CREATE SCHEMA AUTHORIZATION U CREATE TABLE T (K INTEGER) WITH (ENCODINGS = 'AES-256-GCM:NONE')");
    assert_eq!(refused.err().map(|e| e.code), Some(Code::Api));
}

/// Verifies: [EXTENSION] RULES.md D48: with no key named, data is
/// encrypted with the process's ephemeral key, needing no keyring: read
/// back by the same encoding, and by no other, whose ephemeral key is its
/// own (SQLCODE -916).
#[test]
fn no_key_named_is_the_ephemeral_key() {
    let none = AesGcm::new(Arc::new(Held(HashMap::new())));
    let mut sealed = Vec::new();
    none.encode(3, b"spilled rows", &mut sealed).unwrap();
    assert!(!sealed.windows(7).any(|w| w == b"spilled"));
    assert_eq!(sealed.len(), b"spilled rows".len() + none.overhead(""));
    let mut out = Vec::new();
    none.decode(3, &sealed, &mut out).unwrap();
    assert_eq!(out, b"spilled rows");
    assert_eq!(code(&aes(), 3, &sealed), Some(Code::Corrupt), "another process's");
}

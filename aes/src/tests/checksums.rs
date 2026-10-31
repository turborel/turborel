//! Unit tests of SHA-256 and HMAC-SHA-256: known answers of FIPS 180-4's
//! and RFC 4231's examples, the hash function contract (RULES.md
//! R-SAFE-3), keys refused (R-TEST-2), and a database's files checksummed
//! with each, reopened, changed and refused (R-TEST-1).

use std::path::Path;
use std::sync::Arc;

use turborel::db::{Database, OpenOptions};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::Code;
use turborel::hash::{HashFunction, MAX_DIGEST_BYTES};
use turborel::value::Value;

use super::chacha::bytes;
use super::held;
use crate::{HMAC_SHA256_ID, HmacSha256, SHA256_ID, Sha256};

/// Verifies: [EXTENSION] RULES.md D26, D30 [FIPS 180-4]: SHA-256 gives the
/// digests of the standard's examples, "abc" and the 448-bit message, and
/// of the empty message; a digest of parts is that of their concatenation;
/// a byte changed changes it; its id 11, name and length fixed.
#[test]
fn sha_256_gives_the_known_answers() {
    let digest = |data: &[u8]| Sha256.hash(data).as_bytes().to_vec();
    assert_eq!(digest(b"abc"), bytes("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
    let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    assert_eq!(digest(long), bytes("248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"));
    assert_eq!(digest(b""), bytes("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"));
    assert_eq!(Sha256.hash_parts(&[b"ab", b"", b"c"]).as_bytes(), digest(b"abc"));
    assert_ne!(digest(b"abd"), digest(b"abc"));
    assert_eq!((Sha256.id(), Sha256.name(), Sha256.digest_len()), (SHA256_ID, "SHA-256", MAX_DIGEST_BYTES));
}

/// Verifies: [EXTENSION] RULES.md D26, D30 [RFC 4231 §4.2, §4.3]:
/// HMAC-SHA-256 keyed by the examples' keys gives their digests, of parts
/// as of their concatenation; another key gives another; its id 12, name
/// and length fixed.
#[test]
fn hmac_sha_256_gives_the_known_answers() {
    let first = HmacSha256::of_bytes(&[0x0b_u8; 20]);
    assert_eq!(first.hash(b"Hi There").as_bytes(), bytes("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"));
    let second = HmacSha256::of_bytes(b"Jefe");
    let digest = bytes("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    assert_eq!(second.hash(b"what do ya want for nothing?").as_bytes(), digest);
    assert_eq!(second.hash_parts(&[b"what do ya ", b"want for nothing?"]).as_bytes(), digest);
    assert_ne!(first.hash(b"what do ya want for nothing?").as_bytes(), digest, "another key");
    assert_eq!((second.id(), second.name(), second.digest_len()), (HMAC_SHA256_ID, "HMAC-SHA-256", MAX_DIGEST_BYTES));
}

/// Verifies: [EXTENSION] RULES.md D26: HMAC-SHA-256 is keyed by a key of
/// its keyring, through HKDF: each key its own digests, none of them the
/// HMAC of the key itself, the same for the same key; a key the keyring
/// lacks, or a name that is no key's, is refused (SQLCODE -910).
#[test]
fn hmac_sha_256_is_keyed_by_a_key_of_the_keyring() {
    let keys = held();
    let (k, l) = (HmacSha256::new(&*keys, "K").unwrap(), HmacSha256::new(&*keys, "L").unwrap());
    assert_eq!(k.hash(b"page").as_bytes(), HmacSha256::new(&*keys, "K").unwrap().hash(b"page").as_bytes());
    assert_ne!(k.hash(b"page").as_bytes(), l.hash(b"page").as_bytes());
    assert_ne!(k.hash(b"page").as_bytes(), HmacSha256::of_bytes(&[1_u8; 32]).hash(b"page").as_bytes(), "derived, not the key");
    for name in ["M", "", "../K"] {
        assert_eq!(HmacSha256::new(&*keys, name).err().map(|e| e.code), Some(Code::Api), "{name:?}");
    }
}

/// The database in `dir`, its files checksummed with `hash`.
fn checksummed(dir: &Path, hash: Box<dyn HashFunction>) -> turborel::error::Result<Database> {
    let db = Database::new();
    let name = hash.name();
    db.register_hash_function(hash)?;
    db.set_setting("STORAGE.CHECKSUM", &Value::Char(name.to_ascii_lowercase()))?;
    let fs: Arc<dyn FileSystem> = Arc::new(OsFileSystem);
    db.open_files(&fs, dir, &OpenOptions::default())?;
    Ok(db)
}

/// A database made in `dir`, its files checksummed with `hash`, holding a
/// row, closed.
fn made(dir: &Path, hash: Box<dyn HashFunction>) {
    let db = checksummed(dir, hash).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (S VARCHAR(20))").unwrap();
    let s = db.session("S");
    s.run("INSERT INTO T VALUES ('kept')").unwrap();
    s.commit().unwrap();
    drop(s);
    db.close().unwrap();
}

/// The row of the database in `dir`, opened checksummed with `hash`.
fn row(dir: &Path, hash: Box<dyn HashFunction>) -> String {
    let db = checksummed(dir, hash).unwrap();
    let s = db.session("S");
    let mut cursor = s.open_cursor("SELECT S FROM T").unwrap();
    let row = cursor.fetch().unwrap().unwrap()[0].to_string();
    drop((cursor, s));
    db.close().unwrap();
    row
}

/// The first file under `dir` of the block manager's format: one whose
/// header is checksummed.
fn block_file(dir: &Path) -> std::path::PathBuf {
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(at) = dirs.pop() {
        for entry in std::fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if std::fs::read(&path).unwrap().starts_with(b"TBBLOCKS") {
                return path;
            }
        }
    }
    panic!("no block file under {}", dir.display());
}

/// Verifies: [EXTENSION] RULES.md D26, D30: a database created with
/// STORAGE.CHECKSUM naming SHA-256, or HMAC-SHA-256 keyed by K, reopens
/// with it, its rows read back; opened with HMAC-SHA-256 keyed by another
/// key, it is refused (SQLCODE -916), as is one whose file's header was
/// changed, a reserved byte of it.
#[test]
fn a_database_is_checksummed_with_each_and_reopened() {
    let scratch = turborel_test_support::Scratch::new("crypto-checksums");
    let sha = scratch.path("sha");
    made(&sha, Box::new(Sha256));
    assert_eq!(row(&sha, Box::new(Sha256)), "kept");

    let keys = held();
    let keyed = |name: &str| -> Box<dyn HashFunction> { Box::new(HmacSha256::new(&*keys, name).unwrap()) };
    let hmac = scratch.path("hmac");
    made(&hmac, keyed("K"));
    assert_eq!(row(&hmac, keyed("K")), "kept");
    assert_eq!(checksummed(&hmac, keyed("L")).err().map(|e| e.code), Some(Code::Corrupt), "another key");

    let file = block_file(&hmac);
    let mut changed = std::fs::read(&file).unwrap();
    changed[10] ^= 1;
    std::fs::write(&file, changed).unwrap();
    assert_eq!(checksummed(&hmac, keyed("K")).err().map(|e| e.code), Some(Code::Corrupt), "a header changed");
}

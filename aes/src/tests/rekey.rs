//! The files' keys retired (RULES.md D48): STORAGE.ENCODINGS and
//! STORAGE.CHECKSUM_KEY changed to another key, the files written again
//! by the sweep (`Database::rekey`), and the database then opened without
//! the old key (R-TEST-1).

use std::path::Path;
use std::sync::Arc;

use turborel::db::{Database, OpenOptions};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::Code;
use turborel::value::Value;

use super::Held;
use super::reseal::keys;
use crate::{AesGcm, HmacSha256, Keyring};

/// The database in `dir`, AES-256-GCM and HMAC-SHA-256 keyed by `key`
/// registered with `keys`, its background work left to the test.
fn open(dir: &Path, keys: &Arc<Held>, key: &str) -> turborel::error::Result<Database> {
    let db = Database::new();
    let keyring: Arc<dyn Keyring> = Arc::<Held>::clone(keys);
    db.register_encoding(Box::new(AesGcm::new(Arc::clone(&keyring))))?;
    db.register_hash_function(Box::new(HmacSha256::new(keyring, key)?))?;
    db.set_setting("STORAGE.CHECKSUM", &Value::Char("hmac-sha-256".into()))?;
    db.set_setting("JOBS.POLICY", &Value::Char("MANUAL".into()))?;
    let fs: Arc<dyn FileSystem> = Arc::new(OsFileSystem);
    db.open_files(&fs, dir, &OpenOptions::default())?;
    Ok(db)
}

/// Verifies: [EXTENSION] RULES.md D48: a database encrypted by the key K
/// and checksummed by HMAC-SHA-256 keyed by K, its STORAGE.ENCODINGS and
/// STORAGE.CHECKSUM_KEY changed to L, is refused without K (SQLCODE -910)
/// until the sweep writes its files again; swept and closed, it is opened
/// and read with L alone.
#[test]
fn the_files_keys_are_retired_once_the_files_are_written_again() {
    let scratch = turborel_test_support::Scratch::new("crypto-rekey");
    let dir = scratch.path("db");
    let (both, l_only) = (keys(&["K", "L"]), keys(&["L"]));
    let db = Database::new();
    let keyring: Arc<dyn Keyring> = Arc::<Held>::clone(&both);
    db.register_encoding(Box::new(AesGcm::new(Arc::clone(&keyring)))).unwrap();
    db.register_hash_function(Box::new(HmacSha256::new(keyring, "K").unwrap())).unwrap();
    db.set_setting("STORAGE.CHECKSUM", &Value::Char("hmac-sha-256".into())).unwrap();
    db.set_setting("STORAGE.ENCODINGS", &Value::Char("AES-256-GCM:K".into())).unwrap();
    db.set_setting("JOBS.POLICY", &Value::Char("MANUAL".into())).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(OsFileSystem);
    db.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER, S VARCHAR(40))").unwrap();
    let s = db.session("S");
    s.run("INSERT INTO T VALUES (1, 'under K')").unwrap();
    s.commit().unwrap();
    db.checkpoint().unwrap();
    db.set_setting("STORAGE.ENCODINGS", &Value::Char("AES-256-GCM:L".into())).unwrap();
    db.set_setting("STORAGE.CHECKSUM_KEY", &Value::Char("L".into())).unwrap();
    s.run("INSERT INTO T VALUES (2, 'under L')").unwrap();
    s.commit().unwrap();
    drop(s);
    db.close().unwrap();
    assert_eq!(open(&dir, &l_only, "L").err().map(|e| e.code), Some(Code::Api), "pages under K still");

    let sweeping = open(&dir, &both, "K").unwrap();
    sweeping.rekey().unwrap();
    sweeping.close().unwrap();
    let retired = open(&dir, &l_only, "L").unwrap();
    let reader = retired.session("S");
    let mut cursor = reader.open_cursor("SELECT K, S FROM T ORDER BY K").unwrap();
    let mut found = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        found.push(row[1].to_string());
    }
    assert_eq!(found, ["under K", "under L"]);
    drop((cursor, reader));
    retired.close().unwrap();
}

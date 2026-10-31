//! A key retired (RULES.md D48): a table's encodings changed to another
//! key, its rows sealed again (`Session::reseal_table`), and the database
//! then read without the old key (R-TEST-1).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use turborel::db::{Database, OpenOptions};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::Code;

use super::Held;
use crate::{AesGcm, ChaCha20Poly1305, KEY_BYTES, Keyring};

/// The database in `dir`, both encryptions registered with `keys`.
fn open(dir: &Path, keys: &Arc<Held>) -> Database {
    let db = Database::new();
    let keyring: Arc<dyn Keyring> = Arc::<Held>::clone(keys);
    db.register_encoding(Box::new(AesGcm::new(Arc::clone(&keyring)))).unwrap();
    db.register_encoding(Box::new(ChaCha20Poly1305::new(keyring))).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(OsFileSystem);
    db.open_files(&fs, dir, &OpenOptions::default()).unwrap();
    db
}

/// The keys named, each all of the byte of its name's first letter.
fn keys(names: &[&str]) -> Arc<Held> {
    Arc::new(Held(names.iter().map(|name| ((*name).to_owned(), [name.as_bytes()[0]; KEY_BYTES])).collect::<HashMap<_, _>>()))
}

/// The rows of T, in order of K; or the SQLCODE reading them failed with.
fn rows(db: &Database) -> Result<Vec<String>, Code> {
    let s = db.session("S");
    let mut cursor = s.open_cursor("SELECT K, S FROM T ORDER BY K").map_err(|e| e.code)?;
    let mut found = Vec::new();
    while let Some(row) = cursor.fetch().map_err(|e| e.code)? {
        found.push(row[1].to_string().trim_end().to_owned());
    }
    Ok(found)
}

/// Verifies: [EXTENSION] RULES.md D48: a table's rows written under the
/// key K, short and kept out of line, stay sealed under it after its
/// option ENCODINGS names ChaCha20-Poly1305 with the key L, so without K
/// they are refused (SQLCODE -910); sealed again by `reseal_table`, then
/// committed and the database closed, they are read with L alone, and the
/// rows written after under L with them.
#[test]
fn a_key_is_retired_once_the_rows_are_sealed_again() {
    let scratch = turborel_test_support::Scratch::new("crypto-reseal");
    let dir = scratch.path("db");
    let (both, l_only) = (keys(&["K", "L"]), keys(&["L"]));
    let long = "a long secret ".repeat(400);
    let db = open(&dir, &both);
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER, S VARCHAR(6000)) WITH (ENCODINGS = 'AES-256-GCM:K')").unwrap();
    let s = db.session("S");
    s.run(&format!("INSERT INTO T VALUES (1, 'short secret'), (2, '{long}')")).unwrap();
    s.run("ALTER TABLE T SET (ENCODINGS = 'ChaCha20-Poly1305:L')").unwrap();
    s.run("INSERT INTO T VALUES (3, 'written under L')").unwrap();
    s.commit().unwrap();
    drop(s);
    db.close().unwrap();
    let expected = vec!["short secret".to_owned(), long.trim_end().to_owned(), "written under L".to_owned()];

    let without = open(&dir, &l_only);
    assert_eq!(rows(&without), Err(Code::Api), "rows of K are read with K alone");
    without.close().unwrap();

    let resealing = open(&dir, &both);
    let owner = resealing.session("S");
    owner.reseal_table("S.T").unwrap();
    owner.commit().unwrap();
    drop(owner);
    assert_eq!(rows(&resealing), Ok(expected.clone()));
    resealing.close().unwrap();
    let retired = open(&dir, &l_only);
    assert_eq!(rows(&retired), Ok(expected));
    retired.close().unwrap();
}

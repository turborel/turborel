//! \[EXTENSION\] A host plug-in's encoding and checksum (RULES.md D246):
//! every page and record written through the program, read back through
//! it after the database is opened again, and a write the program dies
//! in stopping the node, recovery after giving every commit back.

use turborel::db::{Database, OpenOptions};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::value::Value;

use super::{crash, hosted, started};
use crate::common::rows;

/// The host plug-in's encoding and checksum.
const PAGES: &str = "HOST ENCODING XOR (ID = 9001, OVERHEAD = 1); HOST HASH FUNCTION FNV (ID = 9001, LENGTH = 8);";

/// `db` opened from `dir` on the system's files.
fn opened(db: &Database, dir: &std::path::Path) -> turborel::error::Result<()> {
    let fs: std::sync::Arc<dyn FileSystem> = std::sync::Arc::new(OsFileSystem);
    db.open_files(&fs, dir, &OpenOptions::default()).map(drop)
}

/// Whether any file under `dir` holds `needle`.
fn any_holds(dir: &std::path::Path, needle: &[u8]) -> bool {
    std::fs::read_dir(dir).unwrap().filter_map(Result::ok).any(|e| {
        let path = e.path();
        if path.is_dir() {
            return any_holds(&path, needle);
        }
        std::fs::read(&path).is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle))
    })
}

/// Verifies: [EXTENSION] RULES.md D246, D229: with STORAGE.ENCODINGS and
/// STORAGE.CHECKSUM naming a host plug-in's encoding and checksum, no
/// file holds a row's text as written; the database opened again reads
/// every page through the program; and a page write the program dies in
/// stops the node, every committed row given back by the next opening.
#[test]
fn pages_pass_through_a_host_plugins_encoding() {
    let (scratch, plugin, spec) = hosted("host_pages", PAGES);
    let dir = scratch.path("db");
    let db = started(&spec);
    db.set_setting("STORAGE.ENCODINGS", &Value::Char("XOR".into())).unwrap();
    db.set_setting("STORAGE.CHECKSUM", &Value::Char("FNV".into())).unwrap();
    opened(&db, &dir).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION U CREATE TABLE T (K INTEGER, S VARCHAR(100))").unwrap();
    let s = db.session("U");
    for k in 0..200 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'plain text {k}')")).unwrap();
    }
    s.commit().unwrap();
    db.checkpoint().unwrap();
    assert!(!any_holds(&dir, b"plain text"), "written through XOR, in every file");
    s.run("INSERT INTO T VALUES (200, 'plain text 200')").unwrap();
    s.commit().unwrap();
    crash(&plugin, "encode", true);
    let refused = db.checkpoint().unwrap_err();
    assert_eq!(refused.sqlstate().as_str(), "38T11", "{refused:?}");
    assert!(db.host_stopped().unwrap().is_some());
    drop((s, db));

    crash(&plugin, "encode", false);
    let again = started(&spec);
    opened(&again, &dir).unwrap();
    let s = again.session("U");
    assert_eq!(rows(&s, "SELECT COUNT(*), MAX(S) FROM T").unwrap(), ["201,plain text 99"]);
}

//! The contract of a backup target (RULES.md D166, `backup/target.rs`),
//! checked for every built-in one. An object exists once its upload is
//! finished, and never before, is never replaced, is read in ranges, is
//! listed by prefix, sorted, without uploads, and is removed, or not
//! there, and names are checked. A database opens targets by their locations'
//! schemes, a provider registered among them.

use std::sync::Arc;

use turborel::backup::{BackupTarget, DirectoryTarget, MemoryTarget, TargetProvider};
use turborel::db::Database;
use turborel::error::{Code, Result};

use crate::common::Scratch;

/// Checks the contract on `target`, empty.
fn keeps_the_contract(target: &dyn BackupTarget) {
    let upload = target.begin("a/b").unwrap();
    target.write(upload, b"hello ").unwrap();
    target.write(upload, b"world").unwrap();
    assert_eq!(target.list("").unwrap(), Vec::<String>::new(), "no object before its upload is finished");
    assert_eq!(target.read("a/b", 0, 10).unwrap_err().code, Code::Api);
    target.finish(upload).unwrap();
    assert_eq!(target.read("a/b", 0, 5).unwrap(), b"hello");
    assert_eq!(target.read("a/b", 6, 100).unwrap(), b"world");
    assert_eq!(target.read("a/b", 11, 100).unwrap(), Vec::<u8>::new());
    assert_eq!(target.read("a/b", 50, 100).unwrap(), Vec::<u8>::new());
    let again = target.begin("a/b").unwrap();
    target.write(again, b"other").unwrap();
    assert_eq!(target.finish(again).unwrap_err().code, Code::Api, "never replaced");
    assert_eq!(target.read("a/b", 0, 100).unwrap(), b"hello world");
    let dropped = target.begin("a/c").unwrap();
    target.write(dropped, b"x").unwrap();
    target.abandon(dropped).unwrap();
    for name in ["a/a", "b", "a/d/e"] {
        let u = target.begin(name).unwrap();
        target.finish(u).unwrap();
    }
    assert_eq!(target.list("a/").unwrap(), ["a/a", "a/b", "a/d/e"]);
    assert_eq!(target.list("").unwrap(), ["a/a", "a/b", "a/d/e", "b"]);
    target.remove("a/b").unwrap();
    target.remove("a/b").unwrap();
    assert_eq!(target.list("a/").unwrap(), ["a/a", "a/d/e"]);
    for bad in ["", "a//b", "../x", "a/./b", "a b", "/a"] {
        assert_eq!(target.begin(bad).unwrap_err().code, Code::Api, "{bad:?}");
    }
    assert_eq!(target.write(9_999, b"x").unwrap_err().code, Code::Api);
    assert_eq!(target.finish(9_999).unwrap_err().code, Code::Api);
}

/// Verifies: [EXTENSION] RULES.md D166: the built-in directory target keeps
/// the contract, and holds no upload's file once it is finished or
/// abandoned.
#[test]
fn a_directory_target_keeps_the_contract() {
    let scratch = Scratch::new("target-dir");
    let target = DirectoryTarget::new("dir:t", &scratch.path("t")).unwrap();
    keeps_the_contract(&target);
    let left: Vec<String> = walk(&scratch.path("t")).into_iter().filter(|n| n.contains("upload")).collect();
    assert_eq!(left, Vec::<String>::new());
}

/// The files under `dir`, by their names.
fn walk(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .flat_map(|e| {
            let path = e.unwrap().path();
            if path.is_dir() { walk(&path) } else { vec![path.file_name().unwrap().to_string_lossy().into_owned()] }
        })
        .collect()
}

/// Verifies: [EXTENSION] RULES.md D166: the built-in memory target keeps the
/// contract.
#[test]
fn a_memory_target_keeps_the_contract() {
    keeps_the_contract(&MemoryTarget::new("memory:t"));
}

/// A provider of the scheme "test", each location a memory target.
struct Provider;

impl TargetProvider for Provider {
    fn scheme(&self) -> &'static str {
        "test"
    }
    fn open(&self, location: &str) -> Result<Arc<dyn BackupTarget>> {
        Ok(Arc::new(MemoryTarget::new(location)))
    }
}

/// Verifies: [EXTENSION] RULES.md D166: a database opens a location of no
/// scheme, or of `dir:`, as a directory; `memory:` as one store for the
/// database's life; a registered provider's scheme through it; and refuses
/// a scheme no provider has, or an empty location.
#[test]
fn a_database_opens_targets_by_scheme() {
    let scratch = Scratch::new("target-schemes");
    let db = Database::new();
    let plain = db.open_target(&scratch.path("plain").display().to_string()).unwrap();
    let u = plain.begin("x").unwrap();
    plain.finish(u).unwrap();
    assert!(scratch.path("plain").join("x").exists());
    let memory = db.open_target("memory:m").unwrap();
    let u = memory.begin("y").unwrap();
    memory.finish(u).unwrap();
    assert_eq!(db.open_target("memory:m").unwrap().list("").unwrap(), ["y"], "the same store");
    assert_eq!(db.open_target("test:z").err().map(|e| e.code), Some(Code::Api));
    db.register_target_provider(Arc::new(Provider)).unwrap();
    assert_eq!(db.open_target("test:z").unwrap().location(), "test:z");
    assert_eq!(db.open_target(" ").err().map(|e| e.code), Some(Code::Api));
}

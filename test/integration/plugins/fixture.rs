//! The example plug-in, `plugin/examples/plugin_example`, installed
//! for the tests of extensions: its program, built with the tests, linked
//! or copied into a directory of PLUGINS.DIRECTORIES beside the manifest
//! a test writes.

use std::path::{Path, PathBuf};

use turborel::db::{Database, Session};
use turborel::value::Value;

use crate::common::Scratch;

/// The example's objects, as its manifest declares them.
pub const OBJECTS: &str = "\
FUNCTION TWICE(INTEGER) RETURNS INTEGER DETERMINISTIC BATCH 100;
FUNCTION SHOUT(VARCHAR(200)) RETURNS VARCHAR(200) DETERMINISTIC;
FUNCTION PID() RETURNS BIGINT NOT DETERMINISTIC;
FUNCTION FAIL(VARCHAR(200)) RETURNS INTEGER;
FUNCTION CRASH() RETURNS INTEGER;
FUNCTION HANG() RETURNS INTEGER;
FUNCTION GARBAGE() RETURNS INTEGER;
FUNCTION HUGE(INTEGER) RETURNS VARCHAR(30000);
AGGREGATE PRODUCT(DOUBLE PRECISION) RETURNS DOUBLE PRECISION;
TABLE FUNCTION SERIES(INTEGER) RETURNS TABLE (N INTEGER);
TABLE FUNCTION ROW_COUNT(VARCHAR(128)) RETURNS TABLE (TABLE_NAME VARCHAR(128), N BIGINT) READS SQL DATA;
PROCEDURE NOTE(VARCHAR(200)) MODIFIES SQL DATA;
PROCEDURE PEEK(VARCHAR(200));
METRIC TAXICAB FORMATS (FLOAT32, FLOAT64) PROPERTIES (SYMMETRIC, NON_NEGATIVE, TRIANGLE);
EMBEDDING MODEL VOWELS DIMENSIONS 5 DETERMINISTIC;
AUTHENTICATOR 'EXAMPLE';
TARGET 'example';
PARTITION STRATEGY MODULUS;
";

/// The example's program, as the tests' build made it.
///
/// # Panics
///
/// If it was not built: `cargo test --workspace` builds the examples
/// with the tests.
pub fn program() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let debug = exe.parent().and_then(Path::parent).unwrap();
    let built = debug.join("examples").join(format!("plugin_example{}", std::env::consts::EXE_SUFFIX));
    assert!(
        built.exists(),
        "{} is not built: cargo test --workspace builds it, or cargo build -p turborel-plugin --example plugin_example",
        built.display()
    );
    built
}

/// The manifest of extension `name` at `version`, of `extra` items and
/// `objects`.
pub fn manifest(name: &str, version: &str, extra: &str, objects: &str) -> String {
    let exe = format!("plugin_example{}", std::env::consts::EXE_SUFFIX);
    format!("NAME {name}; VERSION '{version}'; PROTOCOL 1; EXECUTABLE '{exe}';\n{extra}\n{objects}")
}

/// Installs `manifest`, of extension `name` at `version`, in `root`, as a
/// package installed is laid out: its directory.
pub fn install(root: &Path, (name, version): (&str, &str), manifest: &str) -> PathBuf {
    let dir = root.join(name).join(version);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("turborel-plugin.manifest"), manifest).unwrap();
    let exe = dir.join(format!("plugin_example{}", std::env::consts::EXE_SUFFIX));
    if std::fs::hard_link(program(), &exe).is_err() {
        std::fs::copy(program(), &exe).unwrap();
    }
    dir
}

/// A scratch directory, its directory of plug-ins `plugins` holding the
/// example at 1.0.0, and a database in memory allowed it.
pub fn plugged(test: &str) -> (Scratch, Database) {
    let scratch = Scratch::new(test);
    let root = scratch.path("plugins");
    install(&root, ("EXAMPLE", "1.0.0"), &manifest("EXAMPLE", "1.0.0", "", OBJECTS));
    let db = Database::new();
    allow(&db, &root);
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    (scratch, db)
}

/// PLUGINS.DIRECTORIES set to `root`.
pub fn allow(db: &Database, root: &Path) {
    db.set_setting("PLUGINS.DIRECTORIES", &Value::Char(root.display().to_string())).unwrap();
}

/// The processes of extension `name` running now.
pub fn running(s: &Session, name: &str) -> String {
    let query = format!("SELECT PROCESSES_RUNNING FROM INFORMATION_SCHEMA.EXTENSIONS WHERE EXTENSION_NAME = '{name}'");
    crate::common::rows(s, &query).unwrap().join(",")
}

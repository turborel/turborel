//! The examples of extensions, run as printed: the example plug-in,
//! `examples/plugin_example.rs`, built with the tests, installed under
//! PLUGINS.DIRECTORIES of a database of its own for each page.

use std::path::{Path, PathBuf};

use turborel::db::Database;
use turborel::value::Value;

use super::Client;
use crate::common::Scratch;

/// The example's objects the pages call.
const OBJECTS: &str = "FUNCTION TWICE(INTEGER) RETURNS INTEGER DETERMINISTIC BATCH 100;
FUNCTION SHOUT(VARCHAR(200)) RETURNS VARCHAR(200) DETERMINISTIC;
FUNCTION FAIL(VARCHAR(200)) RETURNS INTEGER;
PROCEDURE NOTE(VARCHAR(200)) MODIFIES SQL DATA;
METRIC TAXICAB FORMATS (FLOAT32, FLOAT64) PROPERTIES (SYMMETRIC, NON_NEGATIVE, TRIANGLE);";

/// The example's program, as the workspace's tests built it.
fn program() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let debug = exe.parent().and_then(Path::parent).unwrap();
    let built = debug.join("examples").join(format!("plugin_example{}", std::env::consts::EXE_SUFFIX));
    assert!(built.exists(), "{} is not built: cargo test --workspace builds it", built.display());
    built
}

/// Installs the example at `version` under `root`, with `extra` items.
fn install(root: &Path, version: &str, extra: &str) -> PathBuf {
    let dir = root.join("EXAMPLE").join(version);
    std::fs::create_dir_all(&dir).unwrap();
    let exe = format!("plugin_example{}", std::env::consts::EXE_SUFFIX);
    let manifest = format!("NAME EXAMPLE; VERSION '{version}'; PROTOCOL 1; EXECUTABLE '{exe}'; {extra}\n{OBJECTS}");
    std::fs::write(dir.join("turborel-plugin.manifest"), manifest).unwrap();
    if std::fs::hard_link(program(), dir.join(&exe)).is_err() {
        std::fs::copy(program(), dir.join(&exe)).unwrap();
    }
    dir
}

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D168: the examples of CREATE,
/// ALTER and DROP EXTENSION and of the chapter on plug-ins run as
/// printed, as ANN, the example installed.
#[test]
fn the_extension_examples_run_as_printed() {
    let pages = [
        "client/04-plugins.md",
        "sql/statements/create-extension.md",
        "sql/statements/alter-extension.md",
        "sql/statements/drop-extension.md",
    ];
    for page in pages {
        let scratch = Scratch::new("manual-extensions");
        let root = scratch.path("plugins");
        install(&root, "1.0.0", "");
        if page.contains("alter") {
            let up = install(&root, "1.1.0", "UPGRADE FROM '<1.1.0' SCRIPT 'up.sql';");
            std::fs::write(up.join("up.sql"), "CALL NOTE('updated to 1.1.0')").unwrap();
        }
        let db = Database::new();
        db.set_setting("PLUGINS.DIRECTORIES", &Value::Char(root.display().to_string())).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
        assert!(Client::of(&db, "ANN").page(page) > 0, "{page} shows its examples' output");
    }
}

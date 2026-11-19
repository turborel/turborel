//! The examples of extensions, run as printed: the example plug-in,
//! `plugin/examples/plugin_example`, built with the tests, installed
//! under PLUGINS.DIRECTORIES of a database of its own for each page; and
//! the sessions of `turborel extension` of the pages of packages and of
//! writing a plug-in, each in a directory of its own.

use std::path::{Path, PathBuf};

use turborel::db::Database;
use turborel::value::Value;

use super::Client;
use crate::common::{Scratch, plugin_example};

/// The example's objects the pages call.
const OBJECTS: &str = "FUNCTION TWICE(INTEGER) RETURNS INTEGER DETERMINISTIC BATCH 100;
FUNCTION SHOUT(VARCHAR(200)) RETURNS VARCHAR(200) DETERMINISTIC;
FUNCTION FAIL(VARCHAR(200)) RETURNS INTEGER;
PROCEDURE NOTE(VARCHAR(200)) MODIFIES SQL DATA;
METRIC TAXICAB FORMATS (FLOAT32, FLOAT64) PROPERTIES (SYMMETRIC, NON_NEGATIVE, TRIANGLE);";

/// The objects of the later kinds the page of them shows.
const KINDS: &str = "RANKING COUNTED; QUANTIZER FIRSTS; COLLATION FOLDED VERSION '1';
SYSTEM TABLE EXAMPLE_FACTS (NAME VARCHAR(40), N INTEGER);
XQUERY FUNCTION '{http://example.com/fn}twice' (INTEGER) RETURNS INTEGER;
TYPE PAIR ORDERED; STORAGE ENGINE FILES BATCH 100;
INDEX METHOD SORTED SUPPORTS (EQUALITY, RANGE) BATCH 50; INDEX METHOD NEAR SUPPORTS (NEAREST);";

/// The objects the page of optimizer rules and statistics shows.
const PLANNER: &str = "OPTIMIZER RULE CONTRADICTION PHASE BOUND; OPTIMIZER RULE ERRING;
STORAGE ENGINE FILES; INDEX METHOD SORTED SUPPORTS (EQUALITY, RANGE);
PROCEDURE SKEW(VARCHAR(128), VARCHAR(128)) MODIFIES SQL DATA;";

/// Installs the example at `version` under `root`, with `extra` items.
fn install(root: &Path, version: &str, extra: &str) -> PathBuf {
    let dir = root.join("EXAMPLE").join(version);
    std::fs::create_dir_all(&dir).unwrap();
    let exe = format!("plugin_example{}", std::env::consts::EXE_SUFFIX);
    let manifest = format!("NAME EXAMPLE; VERSION '{version}'; PROTOCOL 1; EXECUTABLE '{exe}'; {extra}\n{OBJECTS}");
    std::fs::write(dir.join("turborel-plugin.manifest"), manifest).unwrap();
    if std::fs::hard_link(plugin_example(), dir.join(&exe)).is_err() {
        std::fs::copy(plugin_example(), dir.join(&exe)).unwrap();
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
        "client/04-plugins-kinds.md",
        "client/04-plugins-planner.md",
        "sql/statements/create-extension.md",
        "sql/statements/alter-extension.md",
        "sql/statements/drop-extension.md",
    ];
    for page in pages {
        let scratch = Scratch::new("manual-extensions");
        let root = scratch.path("plugins");
        let extra = [("kinds", KINDS), ("planner", PLANNER)].into_iter().find(|(name, _)| page.contains(name)).map_or("", |(_, objects)| objects);
        install(&root, "1.0.0", extra);
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

/// The seed of the author's key the pages sign with, so the public key
/// they print is the same each run.
const AUTHOR_SEED: &str = "0707070707070707070707070707070707070707070707070707070707070707";

/// What `turborel extension` shows run with `args`, in `scratch`, as a
/// terminal shows it: its output, or its failure as the program writes
/// it.
pub(super) fn ran(args: &str, scratch: &Scratch) -> String {
    let words: Vec<String> = args.split_whitespace().map(str::to_owned).collect();
    let mut inv = turborel_cli::extension::parse(&words, &|_| None).unwrap().unwrap();
    inv.context.cwd.clone_from(&scratch.dir);
    let mut out = Vec::new();
    match turborel_cli::extension::run(&inv, &mut out) {
        Ok(()) => String::from_utf8(out).unwrap(),
        Err(e) => format!("{}turborel extension: {e}\n", String::from_utf8(out).unwrap()),
    }
}

/// A database whose plug-ins are installed under `scratch`, its packages
/// checked, ANN's schema in it.
fn packaged(scratch: &Scratch) -> Database {
    let db = Database::new();
    db.set_package_crypto(std::sync::Arc::new(turborel_client::packages::Crypto)).unwrap();
    db.set_setting("PLUGINS.DIRECTORIES", &Value::Char(scratch.path("plugins").display().to_string())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    std::fs::write(scratch.path("author.key"), AUTHOR_SEED).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D263, D264, D267: the
/// walkthrough of writing a plug-in runs as printed: the project made by
/// `new`, packed signed, installed, and its function called by ANN. The
/// example plug-in's program, built with the tests, stands in for the
/// one `cargo build --release` builds, as a test runs no cargo.
#[test]
fn the_sdk_walkthrough_runs_as_printed() {
    let page = "client/04-plugins-sdk.md";
    let scratch = Scratch::new("manual-sdk");
    let db = packaged(&scratch);
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.section(page, "## Starting a project", &scratch) > 0);
    let release = scratch.path("hello").join("target").join("release");
    std::fs::create_dir_all(&release).unwrap();
    std::fs::copy(plugin_example(), release.join(format!("hello{}", std::env::consts::EXE_SUFFIX))).unwrap();
    assert!(ann.section(page, "## Packing", &scratch) > 0);
    assert!(ann.section(page, "## Installing", &scratch) > 0);
}

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D260 to D262, D265 to D267: the
/// examples of packages run as printed: the projects' manifests written,
/// packed, one signed and licensed, one not; a requirement missing
/// refused naming the file to add; both installed from their files; the
/// lock exported and reproduced elsewhere; and the packages told.
#[test]
fn the_package_examples_run_as_printed() {
    let page = "client/04-plugins-packages.md";
    let scratch = Scratch::new("manual-packages");
    let db = packaged(&scratch);
    for dir in ["geo_tools/bin", "geo_tools/upgrade", "units/bin", "packages", "plugins", "replica"] {
        std::fs::create_dir_all(scratch.path(dir)).unwrap();
    }
    for program in ["geo_tools/bin/geo-tools", "geo_tools/bin/geo-tools.exe", "geo_tools/bin/geo-tools-macos", "units/bin/units"] {
        std::fs::write(scratch.path(program), "a program").unwrap();
    }
    std::fs::write(scratch.path("geo_tools/upgrade/1.4.sql"), "VALUES (1)").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.section(page, "## For authors", &scratch) > 0);
    assert!(ann.section(page, "## For operators", &scratch) > 0);
}

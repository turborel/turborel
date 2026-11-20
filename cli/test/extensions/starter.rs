//! Tests of the author's side of `turborel extension` (RULES.md D263 to
//! D267): a starter project made by `new`, its program built, packed with
//! its license, installed from the file and called by CREATE EXTENSION;
//! and what `new` and `pack` refuse.
//!
//! The starter's program is not compiled here, as a test may not run
//! cargo. The example plug-in's program, built with the tests, stands in
//! for it at `target/release/<crate>`: it serves TWICE as the starter's
//! scalar does, under the name the manifest gives it. That the starter of
//! every kind builds and passes its own tests is checked by building one.

use std::sync::Arc;

use turborel::db::{Database, Session};
use turborel::value::Value;

use super::{Fixture, ext_in, source};
use crate::common::plugin_example;

/// Verifies: [EXTENSION] RULES.md D263, D264, D266, D267: `new HELLO`
/// writes a project of the scalar TWICE with a LICENSE and its file, a
/// README of build, test and pack, and a Cargo.toml on the SDK; its
/// program built, pack makes a package that holds the program, the
/// manifest and the LICENSE file and not the sources; install tells the
/// license; CREATE EXTENSION hello makes TWICE, which doubles 21;
/// `license` writes the license's text, `info` and EXTENSION_PACKAGES
/// show the expression and its file, and the lock export carries both.
#[test]
fn a_starter_is_made_packed_installed_and_called() {
    let f = Fixture::new("starter");
    let root = &f.scratch.dir;
    let made = ext_in(root, &["new", "hello"]).unwrap();
    assert!(made.starts_with("made the project of HELLO in hello, serving scalar"), "{made}");
    let project = root.join("hello");
    let manifest = std::fs::read_to_string(project.join("turborel-plugin.manifest")).unwrap();
    assert!(manifest.contains("LICENSE 'LicenseRef-Proprietary';\nLICENSE FILE 'LICENSE';"), "{manifest}");
    assert!(manifest.contains("FUNCTION TWICE(INTEGER) RETURNS INTEGER DETERMINISTIC;"), "{manifest}");
    let cargo = std::fs::read_to_string(project.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("turborel-plugin = \"=") && cargo.contains("[workspace]"), "{cargo}");
    let readme = std::fs::read_to_string(project.join("README.md")).unwrap();
    assert!(readme.contains("cargo build --release") && readme.contains("cargo test") && readme.contains("turborel extension pack"));
    assert!(std::fs::read_to_string(project.join("src/main.rs")).unwrap().contains("Harness::new(plugin())"));
    assert!(ext_in(root, &["pack", "hello"]).unwrap_err().contains("build the program first"));
    let release = project.join("target").join("release");
    std::fs::create_dir_all(&release).unwrap();
    std::fs::copy(plugin_example(), release.join(format!("hello{}", std::env::consts::EXE_SUFFIX))).unwrap();
    let packed = ext_in(root, &["pack", "hello"]).unwrap();
    let into = std::path::Path::new("hello").join("hello-0.1.0.tbx");
    let told = format!("packed HELLO 0.1.0 into {}, unsigned, license LicenseRef-Proprietary (text in LICENSE)", into.display());
    assert!(packed.contains(&told), "{packed}");
    let file = project.join("hello-0.1.0.tbx").display().to_string();
    let said = f.on(&["install", &file]).unwrap();
    assert!(said.contains("installed HELLO 0.1.0 from") && said.ends_with("license LicenseRef-Proprietary (text in LICENSE)\n"), "{said}");
    let installed = f.plugins.join("HELLO").join("0.1.0");
    assert!(installed.join("LICENSE").exists() && !installed.join("src").exists() && !installed.join("Cargo.toml").exists());
    assert!(f.on(&["license", "hello"]).unwrap().starts_with("Copyright the authors of HELLO."));
    assert!(f.on(&["info", "hello"]).unwrap().contains("license LicenseRef-Proprietary (text in LICENSE)"));
    f.on(&["lock", "export", "hello.lock"]).unwrap();
    let lock = std::fs::read_to_string(root.join("hello.lock")).unwrap();
    assert!(lock.ends_with("license LicenseRef-Proprietary\nlicense file LICENSE\n"), "{lock}");
    let db = Database::new();
    db.set_package_crypto(Arc::new(turborel_client::packages::Crypto)).unwrap();
    db.set_setting("PLUGINS.DIRECTORIES", &Value::Char(f.plugins.display().to_string())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    let s = db.session("U");
    s.run("CREATE EXTENSION hello").unwrap();
    assert_eq!(first(&s, "VALUES (TWICE(21))"), "42");
    assert_eq!(first(&s, "SELECT LICENSE, LICENSE_FILE FROM INFORMATION_SCHEMA.EXTENSION_PACKAGES"), "LicenseRef-Proprietary,LICENSE");
}

/// The first row of `sql`, its values joined by commas.
fn first(s: &Session, sql: &str) -> String {
    let mut c = s.open_cursor(sql).unwrap();
    let row = c.fetch().unwrap().unwrap();
    row.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
}

/// Verifies: [EXTENSION] RULES.md D263, D283: `new` refuses a directory that
/// is there, a name that is not a plain identifier and a kind it does not
/// know, naming those it does; `--kind` writes a project of each kind
/// asked, once, in the order of their codes, at `--dir`; host kinds are
/// asked alone, their README naming PLUGINS.HOST, an eviction policy
/// with the setting it reads; a language is no host kind.
#[test]
fn new_refuses_what_it_cannot_make() {
    let f = Fixture::new("new");
    let root = &f.scratch.dir;
    std::fs::create_dir_all(root.join("taken")).unwrap();
    assert!(ext_in(root, &["new", "taken"]).unwrap_err().contains("is there already"));
    assert!(root.join("taken").read_dir().unwrap().next().is_none(), "nothing written into it");
    assert!(ext_in(root, &["new", "9lives"]).unwrap_err().contains("not a plain identifier"));
    let unknown = ext_in(root, &["new", "x", "--kind", "gadget"]).unwrap_err();
    assert!(unknown.contains("gadget is no kind of plug-in") && unknown.contains("index-method"), "{unknown}");
    let made = ext_in(root, &["new", "multi", "--kind", "metric", "--kind", "scalar", "--kind", "metric", "--dir", "m"]).unwrap();
    assert!(made.contains("serving scalar, metric"), "{made}");
    let manifest = std::fs::read_to_string(root.join("m").join("turborel-plugin.manifest")).unwrap();
    assert_eq!(manifest.matches("METRIC TAXICAB").count(), 1, "{manifest}");
    assert!(std::fs::read_to_string(root.join("m/src/main.rs")).unwrap().contains(".kinds(\"MULTI\"), [1, 5]"));
    let mixed = ext_in(root, &["new", "mixed", "--kind", "clock", "--kind", "scalar"]).unwrap_err();
    assert!(mixed.contains("host kinds alone"), "{mixed}");
    let hosted = ext_in(root, &["new", "vault", "--kind", "log-storage", "--kind", "clock", "--kind", "archive"]).unwrap();
    assert!(hosted.contains("serving clock, log-storage, archive"), "{hosted}");
    let manifest = std::fs::read_to_string(root.join("vault").join("turborel-plugin.manifest")).unwrap();
    assert!(manifest.contains("HOST CLOCK SYSTEM (EVERY = 10);\nHOST LOG STORAGE LOG (BATCH = 256);\nHOST ARCHIVE VAULT;"), "{manifest}");
    assert!(turborel::plugin::manifest::Manifest::parse(&manifest).is_ok_and(|m| m.hosted.len() == 3));
    assert!(std::fs::read_to_string(root.join("vault").join("README.md")).unwrap().contains("PLUGINS.HOST=VAULT="));
    ext_in(root, &["new", "scans", "--kind", "buffer-eviction"]).unwrap();
    let manifest = std::fs::read_to_string(root.join("scans").join("turborel-plugin.manifest")).unwrap();
    let parsed = turborel::plugin::manifest::Manifest::parse(&manifest).unwrap();
    assert_eq!((parsed.hosted.len(), parsed.settings[0].qualified(&parsed.name)), (1, "SCANS.ONCE".to_owned()), "{manifest}");
    assert!(std::fs::read_to_string(root.join("scans/src/main.rs")).unwrap().contains(".host_eviction(\"SCANS\", Scans)"));
    let worded = ext_in(root, &["new", "words", "--kind", "language", "--kind", "scalar"]).unwrap();
    assert!(worded.contains("serving scalar, language"), "{worded}");
    let manifest = std::fs::read_to_string(root.join("words").join("turborel-plugin.manifest")).unwrap();
    assert_eq!(turborel::plugin::manifest::Manifest::parse(&manifest).unwrap().kinds(), [1, 39], "{manifest}");
    assert!(std::fs::read_to_string(root.join("words/src/main.rs")).unwrap().contains(".language(\"SPACED\", Spaced)"));
}

/// Verifies: [EXTENSION] RULES.md D264, D265, D266: pack refuses a
/// LICENSE FILE that is not in the project and a LICENSE that is not an
/// SPDX expression (38T06); a package of no license packs, warned of;
/// what pack takes is the manifest, its program, the files it names and
/// those under doc/ and data/.
#[test]
fn pack_refuses_a_license_it_cannot_carry() {
    let f = Fixture::new("pack-license");
    let root = &f.scratch.dir;
    let pack = |dir: &std::path::Path| ext_in(root, &["pack", &dir.display().to_string(), "--out", "x.tbx"]);
    let missing = source(root, "NOFILE", "1.0.0", "LICENSE 'MIT'; LICENSE FILE 'COPYING';");
    assert!(pack(&missing).unwrap_err().contains("COPYING"));
    let bad = source(root, "BAD", "1.0.0", "LICENSE 'MIT AND';");
    assert!(pack(&bad).unwrap_err().contains("38T06"));
    let listed = source(root, "LIST", "1.0.0", "LICENSE 'MIT, Apache-2.0';");
    assert!(pack(&listed).unwrap_err().contains("38T06"));
    let good = source(root, "GOOD", "1.0.0", "LICENSE '(MIT OR Apache-2.0) AND LicenseRef-Extra WITH LLVM-exception';");
    std::fs::write(good.join("notes.txt"), "not named").unwrap();
    let said = pack(&good).unwrap();
    assert!(!said.contains("warning") && said.contains("license (MIT OR Apache-2.0) AND LicenseRef-Extra WITH LLVM-exception"), "{said}");
    let shown = f.on(&["install", "x.tbx"]).unwrap();
    assert!(shown.contains("installed GOOD 1.0.0"), "{shown}");
    let dir = f.plugins.join("GOOD").join("1.0.0");
    assert!(dir.join("doc/README").exists() && !dir.join("notes.txt").exists());
    assert!(f.on(&["license", "good"]).unwrap().contains("holds no text of it"));
}

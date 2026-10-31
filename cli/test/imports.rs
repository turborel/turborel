//! Imports: the rows of files `--import` names inserted into a table, in
//! turn with the statements named, what each did said, its failures told
//! (RULES.md D59).

use std::path::PathBuf;

use turborel::db::Database;
use turborel_cli::{Importing, Input, Io};
use turborel_client::import::{Dialect, Import};

use crate::common::{Closed, Running, Scratch, command, options, ran, start};

/// A server of a database where X has table T, of a key K and a string S.
fn served() -> (Database, Running) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER PRIMARY KEY, S VARCHAR(5))").unwrap();
    let server = start(&db, &[]);
    (db, server)
}

/// The file `name` of `scratch`, holding `text`.
fn file(scratch: &Scratch, name: &str, text: &str) -> PathBuf {
    let path = scratch.path(name);
    std::fs::write(&path, text).unwrap();
    path
}

/// An import into T, as CSV with a header, stopping at a bad row unless
/// `skip`.
fn into_t(skip: bool) -> Importing {
    Importing { into: Import { skip, ..Import::into("T") }, dialect: Dialect::csv().with_header() }
}

/// Verifies: [DESIGN] RULES.md D59: a file's rows are imported in turn
/// with the statements named, what it imported said; one with rows
/// skipped tells each, says how many, and fails the run.
#[test]
fn files_are_imported_in_turn() {
    let (_db, server) = served();
    let scratch = Scratch::new("cli-import-turn");
    let good = file(&scratch, "good.csv", "K,S\n1,a\n2,\"b,c\"\n");
    let bad = file(&scratch, "bad.csv", "K,S\n3,d\nx,e\n4,toolong\n");
    let mut given = options(&server, vec![Input::Import(good), command("SELECT COUNT(*) FROM T")]);
    given.import = Some(into_t(false));
    let run = ran(&given, "", false);
    assert!(run.succeeded, "{}", run.err);
    assert!(run.out.starts_with("OK, 2 rows imported\n") && run.out.ends_with(" 2\n(1 row)\n"), "{}", run.out);
    let mut skipping = options(&server, vec![Input::Import(bad.clone())]);
    skipping.import = Some(into_t(true));
    let run = ran(&skipping, "", false);
    assert!(!run.succeeded, "a row skipped fails the run");
    assert_eq!(run.out, "OK, 1 row imported, 2 skipped\n");
    let told: Vec<&str> = run.err.lines().collect();
    let shown = bad.display();
    assert_eq!(told.len(), 2, "{}", run.err);
    assert!(told[0].starts_with(&format!("turborel: {shown}: record 2, line 3: SQLCODE -301 (SQLSTATE 22018): ")), "{}", told[0]);
    assert!(told[1].starts_with(&format!("turborel: {shown}: record 3, line 4: SQLCODE -301 (SQLSTATE 22001): ")), "{}", told[1]);
    assert!(told[1].ends_with("; skipped"), "{}", told[1]);
}

/// Verifies: [DESIGN] RULES.md D59, D46: an import that fails is told, its
/// record and line, what was committed and rolled back, and the run goes
/// on, or with `--stop-on-error` ends there; a file that cannot be read
/// ends it; an import named with no table to import into is refused.
#[test]
fn an_import_that_fails_is_told() {
    let (_db, server) = served();
    let scratch = Scratch::new("cli-import-fails");
    let bad = file(&scratch, "bad.csv", "K,S\n1,a\n1,b\n");
    let mut given = options(&server, vec![Input::Import(bad.clone()), command("SELECT COUNT(*) FROM T")]);
    given.import = Some(into_t(false));
    let run = ran(&given, "", false);
    assert!(!run.succeeded);
    let said = format!("turborel: {}: record 2, line 3: SQLCODE ", bad.display());
    assert!(run.err.starts_with(&said), "{}", run.err);
    assert!(run.err.ends_with("; 0 rows committed, 1 rolled back\n"), "{}", run.err);
    assert!(run.out.ends_with(" 0\n(1 row)\n"), "the run goes on: {}", run.out);
    given.stop_on_error = true;
    let run = ran(&given, "", false);
    assert_eq!(run.out, "", "the run ends there");
    let missing = scratch.path("missing.csv");
    let mut unread = options(&server, vec![Input::Import(missing.clone()), command("SELECT 1")]);
    unread.import = Some(into_t(false));
    let run = ran(&unread, "", false);
    assert!(run.err.starts_with(&format!("turborel: cannot read {}: ", missing.display())), "{}", run.err);
    assert_eq!(run.out, "");
    let tableless = ran(&options(&server, vec![Input::Import(missing)]), "", false);
    assert_eq!(tableless.err, "turborel: [DESIGN] --import needs --table\n");
}

/// Verifies: [DESIGN] RULES.md D59, D46: a run over a connection of its
/// own imports as an SQL-agent's does, and what it says that cannot be
/// written ends the run; an SQL-agent with no current connection imports
/// nothing, saying so [ISO/IEC 9075:1992 §15.3 GR9].
#[test]
fn an_import_needs_a_connection_and_its_output() {
    let (_db, server) = served();
    let scratch = Scratch::new("cli-import-connection");
    let rows = file(&scratch, "rows.csv", "K\n1\n");
    let mut given = options(&server, vec![Input::Import(rows.clone()), command("SELECT 1")]);
    given.import = Some(into_t(false));
    let mut connection = turborel_cli::connect(&given, || Ok(String::new())).unwrap();
    let mut err = Vec::new();
    let io = Io { input: &mut std::io::empty(), typed: false, out: &mut Closed, err: &mut err, interrupts: None };
    assert!(!turborel_cli::run(&mut connection, &given, io));
    assert_eq!(String::from_utf8(err).unwrap(), "turborel: cannot write: closed\n", "the run ends there");
    connection.terminate().unwrap();
    let mut gone = options(&server, vec![command("DISCONNECT CURRENT"), Input::Import(rows.clone())]);
    gone.import = Some(into_t(false));
    let run = ran(&gone, "", false);
    let said = format!("turborel: {}: SQLCODE -919 (SQLSTATE 08003): ", rows.display());
    assert!(run.err.starts_with(&said), "{}", run.err);
    assert!(run.err.ends_with("; 0 rows committed, 0 rolled back\n"), "{}", run.err);
}

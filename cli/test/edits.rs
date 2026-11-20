//! `\e`: the statement begun, or a file, edited in the editor the
//! environment names, then read as if typed (RULES.md D210).

use turborel::db::Database;
use turborel_cli::Outcome;

use crate::common::{Running, Scratch, Script, Seat, session, start};

/// A server of a database where X has table T.
fn served() -> (Database, Running) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let server = start(&db, &[]);
    (db, server)
}

/// Runs `text` as a script, `editor` the editor TURBOREL_EDITOR names:
/// this program itself, given `args`: `admin --help` prints its usage and
/// succeeds, leaving the file as it was,
/// on any platform.
fn edited(server: &Running, args: &str, text: &str) -> Script {
    let editor = format!("{} {args}", env!("CARGO_BIN_EXE_turborel"));
    let seat = Seat { environment: vec![("TURBOREL_EDITOR", editor)], ..Seat::default() };
    session(server, &["-t", "-A"], text, false, seat)
}

/// Verifies: [DESIGN] RULES.md D210: `\e` writes the statement begun to a
/// file of the client's own, runs the editor on it, and reads back what
/// the editor left as if typed, its statements run and the rest the
/// statement begun; with none begun, the last statement run; `\e FILE`
/// edits FILE and runs it. An editor that fails leaves nothing run, and
/// says how it ended.
#[test]
fn the_statement_begun_or_a_file_is_edited_and_run() {
    let (_db, server) = served();
    let begun = edited(&server, "admin --help", "VALUES 1 +\n1\n\\e\n");
    assert_eq!((begun.outcome, begun.out.as_str(), begun.err.as_str()), (Outcome::Succeeded, "2\n", ""));
    let last = edited(&server, "admin --help", "VALUES 3;\n\\e\n;\n");
    assert_eq!(last.out, "3\n3\n", "{}", last.err);
    let scratch = Scratch::new("cli-edit");
    let file = scratch.path("edit.sql");
    std::fs::write(&file, "VALUES 4;\nVALUES\n5;\n").unwrap();
    let named = edited(&server, "admin --help", &format!("\\e {}\n", file.display()));
    assert_eq!(named.out, "4\n5\n", "{}", named.err);
    assert!(file.exists(), "a file named is the user's, kept");
    let failed = edited(&server, "--verbose", "VALUES 6\n\\e\n");
    assert_eq!(failed.outcome, Outcome::Failed);
    assert!(failed.err.starts_with("turborel: \\e: the editor ended with "), "{}", failed.err);
    assert_eq!(failed.out, "", "nothing run");
}

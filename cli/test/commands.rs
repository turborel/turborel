//! Backslash commands of a session that change how it runs and writes:
//! variables, the statement begun, expanded and aligned output, `\format`,
//! `\tee`, `\o`, conditionals left open, and each command's refusals
//! (RULES.md D210 to D216).

use turborel::db::Database;
use turborel_cli::Outcome;

use crate::common::{Running, Scratch, script, start};

/// A server of a database X owns, where X has table T, holding 1 and 2.
fn served() -> (Database, Running) {
    let db = Database::new();
    db.create_user("X", b"x").unwrap();
    db.set_owner("X").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER, S CHARACTER VARYING(5))").unwrap();
    let server = start(&db, &[]);
    assert_eq!(script(&server, &["-q"], "INSERT INTO T VALUES (1, 'a'), (2, 'b');").outcome, Outcome::Succeeded);
    (db, server)
}

/// Verifies: [DESIGN] RULES.md D210, D212: `\set` alone lists every
/// variable as a literal; `\set` of a name that is no variable's is
/// refused; `\unset` removes one, so it is no longer read; `\?` lists
/// the commands; `\q` ends the run, the lines after it unread.
#[test]
fn variables_are_listed_set_and_unset() {
    let (_db, server) = served();
    let ran = script(&server, &[], "\\set V 'one'\n\\set\n\\set a-b 2\n\\unset V\n\\echo [:V]\n\\?\n\\q\n\\echo ENDED\n");
    assert_eq!(ran.outcome, Outcome::Failed);
    assert!(ran.out.contains("AUTOCOMMIT = 'off'\n"), "{}", ran.out);
    assert!(ran.out.contains("V = 'one'\n"), "{}", ran.out);
    assert!(ran.out.contains("[:V]\n"), "unset, so not read: {}", ran.out);
    assert!(ran.out.contains("General\n  \\q "), "{}", ran.out);
    assert!(!ran.out.contains("ENDED"), "{}", ran.out);
    assert_eq!(ran.err, "turborel: \\set: \"a-b\" is no variable's name\n");
}

/// Verifies: [DESIGN] RULES.md D210, D216: `\p` writes the statement
/// begun, `\r` throws it away, so the line after begins anew; `\g` with
/// no statement begun and none run is refused; a command no command
/// names, or given arguments it takes none of, is refused, and the run
/// goes on.
#[test]
fn the_statement_begun_is_printed_and_thrown_away() {
    let (_db, server) = served();
    let ran = script(&server, &["-t", "-A"], "\\g\nVALUES\n(1)\n\\p\n\\r\nVALUES 2;\n\\nothing\n\\p now\n\\q\n");
    assert_eq!(ran.outcome, Outcome::Failed);
    assert_eq!(ran.out, "VALUES\n(1)\n2\n");
    assert_eq!(
        ran.err,
        "turborel: \\g: no statement is begun, and none was run\n\
         turborel: \\nothing is no command: \\? lists them\n\
         turborel: \\p takes no arguments\n"
    );
}

/// Verifies: [DESIGN] RULES.md D213: `\x` toggles expanded rows, or sets
/// them on, off or auto; `\a` toggles aligned and unaligned; `\t`
/// toggles the rows alone; each says what it set, and a value that is
/// neither on nor off is refused.
#[test]
fn expanded_aligned_and_tuples_are_toggled() {
    let (_db, server) = served();
    let ran = script(&server, &[], "\\x\n\\x\n\\x auto\n\\x\n\\x on\n\\x off\n\\x maybe\n\\x on off\n\\a\n\\a\n\\t\n\\t\n");
    assert_eq!(ran.outcome, Outcome::Failed);
    assert_eq!(
        ran.out,
        "Expanded output is on.\nExpanded output is off.\nExpanded output is auto.\nExpanded output is off.\n\
         Expanded output is on.\nExpanded output is off.\nOutput is unaligned.\nOutput is aligned.\n\
         Rows alone are on.\nRows alone are off.\n"
    );
    assert_eq!(ran.err, "turborel: \\x takes on or off, not maybe\nturborel: \\x takes on or off\n");
}

/// Verifies: [DESIGN] RULES.md D213: `\format` alone lists every option
/// and its value; with an option, shows its value; an option it has not
/// is refused, naming those it has; a value an option does not take is
/// refused.
#[test]
fn format_options_are_listed_shown_and_refused() {
    let (_db, server) = served();
    let ran = script(&server, &[], "\\format\n\\format border\n\\format colour\n\\format border 9\n");
    assert_eq!(ran.outcome, Outcome::Failed);
    let lines: Vec<&str> = ran.out.lines().collect();
    assert_eq!(lines.first(), Some(&"style aligned"), "{}", ran.out);
    assert!(lines.contains(&"border 1"), "{}", ran.out);
    assert_eq!(lines.last(), Some(&"border 1"), "{}", ran.out);
    let told: Vec<&str> = ran.err.lines().collect();
    assert_eq!(told.len(), 2, "{}", ran.err);
    assert!(told[0].starts_with("turborel: colour is no option of \\format: style, "), "{}", ran.err);
    assert!(told[1].starts_with("turborel: "), "{}", ran.err);
}

/// Verifies: [DESIGN] RULES.md D216: `\tee` copies results and failures
/// to its file as they are written, and what `\echo` writes too;
/// `\notee` stops the copy; a file that cannot be made is refused, as is
/// one `\o` cannot write to. A regression test (R-TEST-7): what a command
/// wrote to the output, as `\echo`, was not copied, though `\tee` copies
/// all that is written.
#[test]
fn tee_copies_output_until_notee() {
    let (_db, server) = served();
    let scratch = Scratch::new("cli-tee");
    let copy = scratch.path("copy.out");
    let text = format!(
        "\\tee {}\nSELECT K FROM T ORDER BY K;\nSELECT NOTHING FROM T;\n\\echo copied\n\\notee\n\\echo alone\n\\tee {}\n\\o {}\n",
        copy.display(),
        scratch.path("no/such/dir/f").display(),
        scratch.path("no/such/dir/g").display()
    );
    let ran = script(&server, &["-t", "-A"], &text);
    assert_eq!(ran.outcome, Outcome::Failed);
    assert_eq!(ran.out, "1\n2\ncopied\nalone\n");
    let copied = std::fs::read_to_string(&copy).unwrap();
    assert!(copied.starts_with("1\n2\nturborel: SQLCODE -"), "{copied}");
    assert!(copied.ends_with("copied\n"), "{copied}");
    let told: Vec<&str> = ran.err.lines().collect();
    assert_eq!(told.len(), 3, "{}", ran.err);
    assert!(told[1].starts_with("turborel: \\tee: cannot write "), "{}", ran.err);
    assert!(told[2].starts_with("turborel: \\o: cannot write to "), "{}", ran.err);
}

/// Verifies: [DESIGN] RULES.md D212, D214: `\if` of a value neither true
/// nor false is refused and its block opened as false; a block left
/// open at the end of the input is told; `\elif` of a wrong value is
/// refused too.
#[test]
fn conditionals_refuse_wrong_values_and_open_blocks() {
    let (_db, server) = served();
    let ran = script(&server, &[], "\\if perhaps\n\\echo no\n\\elif nor\n\\echo none\n\\else\n\\echo else\n\\endif\n\\if on\n");
    assert_eq!(ran.outcome, Outcome::Failed);
    assert_eq!(ran.out, "else\n");
    assert_eq!(
        ran.err,
        "turborel: \\if: \"perhaps\" is neither true nor false\n\
         turborel: \\elif: \"nor\" is neither true nor false\n\
         turborel: \\if: a block is not closed by \\endif\n"
    );
}

/// Verifies: [DESIGN] RULES.md D215: `\gset` unsets the variable of a
/// null column, and refuses a prefix that makes no variable's name;
/// `\gexec` skips a null value.
#[test]
fn gset_and_gexec_handle_nulls_and_wrong_names() {
    let (_db, server) = served();
    let text = "\\set M 1\nSELECT NULLIF(K, K) AS M FROM T WHERE K = 1 \\gset\n\\echo [:M]\n\
                SELECT K AS N FROM T WHERE K = 1 \\gset x-\n\
                SELECT NULLIF('VALUES 1', 'VALUES 1') FROM T WHERE K = 1 \\gexec\n";
    let ran = script(&server, &["-q", "-t", "-A"], text);
    assert_eq!(ran.outcome, Outcome::Failed);
    assert_eq!(ran.out, "[:M]\n");
    assert_eq!(ran.err, "turborel: \\gset: \"x-N\" is no variable's name\n");
}

/// Verifies: [DESIGN] RULES.md D212: with FETCH_COUNT set, a query's rows
/// are fetched that many at a time through a cursor, all of them shown;
/// a statement that is no query, or cannot be prepared, runs as it
/// would without it.
#[test]
fn fetch_count_fetches_rows_through_a_cursor() {
    let (_db, server) = served();
    let text = "\\set FETCH_COUNT 1\nSELECT K FROM T ORDER BY K;\nINSERT INTO T VALUES (3, 'c');\nSELECT K FROM NOWHERE;\nSELECT COUNT(*) FROM T;\n";
    let ran = script(&server, &["-t", "-A"], text);
    assert_eq!(ran.outcome, Outcome::Failed);
    assert_eq!(ran.out, "1\n2\nOK\n3\n");
    assert!(ran.err.starts_with("turborel: SQLCODE -"), "{}", ran.err);
}

/// Verifies: [DESIGN] RULES.md D59, D216: `\import` takes its options,
/// a delimiter, a null text, a batch and skip; one it has not, one
/// missing its value, and a batch that is no number are refused, nothing
/// imported.
#[test]
fn import_options_are_read_and_refused() {
    let (_db, server) = served();
    let scratch = Scratch::new("cli-import-options");
    let file = scratch.path("rows.csv");
    std::fs::write(&file, "K;S\n7;~\n8;h\nx;y\n").unwrap();
    let path = file.display();
    let text = format!(
        "\\import T {path} csv header delimiter ; null ~ batch 1 skip\n\
         \\import T {path} sideways\n\
         \\import T {path} null\n\
         \\import T {path} batch many\n\
         \\import T\n\
         SELECT K, S FROM T WHERE K > 6 ORDER BY K;\n"
    );
    let ran = script(&server, &["-t", "-A"], &text);
    assert_eq!(ran.outcome, Outcome::Failed);
    assert_eq!(ran.out, "OK, 2 rows imported, 1 skipped\n7|\n8|h\n", "{}", ran.err);
    let told: Vec<&str> = ran.err.lines().filter(|l| !l.contains("rows.csv")).collect();
    assert_eq!(
        told,
        [
            "turborel: \\import: sideways is no option",
            "turborel: \\import: null needs a value",
            "turborel: \\import: batch takes a number of rows",
            "turborel: \\import TABLE FILE [OPTION...]",
        ]
    );
}

/// Verifies: [DESIGN] RULES.md D216, R-CF-6: a file that includes itself
/// is read at most MAX_INCLUDES deep, then the run stops, saying so; an
/// `\export` whose arguments are wrong, or whose query fails, is
/// refused.
#[test]
fn includes_are_bounded_and_exports_refused() {
    let (_db, server) = served();
    let scratch = Scratch::new("cli-include-depth");
    let file = scratch.path("self.sql");
    std::fs::write(&file, format!("\\i {}\n", file.display())).unwrap();
    let ran = script(&server, &[], &format!("\\i {}\n\\echo after\n", file.display()));
    assert_eq!(ran.outcome, Outcome::Failed);
    assert!(ran.err.contains("files are included at most 16 deep"), "{}", ran.err);
    let out = scratch.path("t.csv").display().to_string();
    let exported = script(&server, &[], &format!("\\export\n\\export (SELECT NOTHING FROM T) {out}\n"));
    assert_eq!(exported.outcome, Outcome::Failed);
    let told: Vec<&str> = exported.err.lines().collect();
    assert_eq!(told.len(), 2, "{}", exported.err);
    assert!(told[1].starts_with("turborel: \\export: "), "{}", exported.err);
}

/// Verifies: [DESIGN] RULES.md D212, D215: with ECHO set to errors, a
/// statement that fails is written before its failure, one that succeeds
/// is not; a query of `\gset` that fails is told so, its variables left
/// as they were; with `--single-transaction`, a run whose statements all
/// succeed is committed at its end.
#[test]
fn errors_are_echoed_and_a_single_transaction_commits() {
    let (_db, server) = served();
    let text = "\\set ECHO errors\n\\set N kept\nVALUES 1;\nSELECT NOTHING FROM T;\nSELECT NOTHING AS N FROM T \\gset\n\\echo :N\n";
    let ran = script(&server, &["-t", "-A"], text);
    assert_eq!(ran.outcome, Outcome::Failed);
    assert_eq!(ran.out, "1\nkept\n");
    let told: Vec<&str> = ran.err.lines().collect();
    assert_eq!(told.len(), 4, "{}", ran.err);
    assert_eq!((told[0], told[2]), ("SELECT NOTHING FROM T", "SELECT NOTHING AS N FROM T"));
    assert!(told[1].starts_with("turborel: SQLCODE -103") && told[3].starts_with("turborel: SQLCODE -103"), "{}", ran.err);
    let one = script(&server, &["-1", "-q"], "INSERT INTO T VALUES (3, 'c');\n");
    assert_eq!(one.outcome, Outcome::Succeeded, "{}", one.err);
    assert_eq!(script(&server, &["-t", "-A"], "SELECT COUNT(*) FROM T WHERE K = 3;").out, "1\n", "committed");
}

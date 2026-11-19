//! Scripts run against a real server: backslash commands, variables,
//! conditionals, output formats, describe commands, files and outcomes
//! (RULES.md D210 to D219).

use turborel::db::Database;
use turborel_cli::{Command, Dialer, Io, Outcome, Terminal};
use turborel_client::agent::Agent;

use crate::common::{Running, Scratch, start};

/// A server of a database X owns, where X has table T, holding 1 and 2,
/// and U, whose rows reference T's.
fn served() -> (Database, Running) {
    let db = Database::new();
    db.create_user("X", b"x").unwrap();
    db.set_owner("X").unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION X \
         CREATE TABLE T (K INTEGER PRIMARY KEY, S CHARACTER VARYING(5) DEFAULT 'none' NOT NULL) \
         CREATE TABLE U (ID INTEGER, K INTEGER REFERENCES T (K))",
    )
    .unwrap();
    let server = start(&db, &[]);
    assert_eq!(script(&server, &["-q"], "INSERT INTO T VALUES (1, 'a'), (2, 'b');").outcome, Outcome::Succeeded);
    (db, server)
}

/// What a script run printed and told, and how it ended.
struct Script {
    outcome: Outcome,
    out: String,
    err: String,
}

/// Runs `text` as standard input, not typed, with the command line
/// `args` besides the connection's, over an SQL-agent as the program does.
fn script(server: &Running, args: &[&str], text: &str) -> Script {
    let port = server.address.port().to_string();
    let mut line: Vec<String> = ["-U", "X", "-p", port.as_str()].iter().map(|a| (*a).to_owned()).collect();
    line.extend(args.iter().map(|a| (*a).to_owned()));
    let Ok(Command::Run(options)) = turborel_cli::parse(&line, &|_| None) else { panic!("{line:?}") };
    let connection = turborel_cli::connect(&options, || Ok(String::new())).unwrap();
    let mut agent = Agent::connected(Dialer::new((*options).clone(), Box::new(|| Ok(String::new()))), connection);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let io =
        Io { input: &mut text.as_bytes(), typed: false, out: &mut out, err: &mut err, interrupts: None, terminal: Terminal::default() };
    let outcome = turborel_cli::outcome(&mut agent, &options, io);
    agent.terminate().unwrap();
    Script { outcome, out: String::from_utf8(out).unwrap(), err: String::from_utf8(err).unwrap() }
}

/// Verifies: [DESIGN] RULES.md D212: variables set by `\set` and `-v` are
/// read into statements as text, literals and identifiers; `\if` runs the
/// first branch whose value is true, nested; ERROR, SQLSTATE and ROW_COUNT
/// tell how the last statement went.
#[test]
fn variables_and_conditionals_steer_a_script() {
    let (_db, server) = served();
    let text = "\\set name b\n\
                SELECT K FROM T WHERE S = :'name';\n\
                \\if :{?wanted}\n\\echo wanted\n\\elif true\n\\if false\n\\echo no\n\\else\n\\echo nested\n\\endif\n\\else\n\\echo never\n\\endif\n\
                SELECT NOPE FROM T;\n\
                \\echo :ERROR :SQLSTATE\n\
                SELECT * FROM :\"table\";\n\
                \\echo :ERROR :ROW_COUNT\n";
    let ran = script(&server, &["-v", "table=T", "-q"], text);
    assert_eq!(
        ran.out,
        "            K\n--------------\n            2\n(1 row)\nnested\ntrue 2A000\n            K | S\n--------------+-------\n            1 | a\n            2 | b\n(2 rows)\nfalse 2\n"
    );
    assert!(ran.err.contains("SQLSTATE 2A000"), "{}", ran.err);
    assert_eq!(ran.outcome, Outcome::Failed);
}

/// Verifies: [DESIGN] RULES.md D215: exit statuses: 0 when every statement
/// succeeded, 1 when one failed and the run went on, 3 when STOP_ON_ERROR
/// ended it there, its statements after not run.
#[test]
fn outcomes_give_exit_statuses() {
    let (_db, server) = served();
    let fine = script(&server, &[], "VALUES 1;");
    assert_eq!((fine.outcome, fine.outcome.code()), (Outcome::Succeeded, 0));
    let failed = script(&server, &[], "SELECT NOPE FROM T;\n\\echo after\n");
    assert_eq!((failed.outcome, failed.outcome.code(), failed.out.as_str()), (Outcome::Failed, 1, "after\n"));
    let stopped = script(&server, &[], "\\set STOP_ON_ERROR on\nSELECT NOPE FROM T;\n\\echo after\n");
    assert_eq!((stopped.outcome, stopped.outcome.code(), stopped.out.as_str()), (Outcome::Stopped, 3, ""));
}

/// Verifies: [DESIGN] RULES.md D215, R-ERR-5: with `--single-transaction`
/// every statement is one transaction, rolled back when one fails.
/// Without it a failed statement leaves no effect, so the transaction
/// goes on and what it did before the failure is committed.
#[test]
fn transactions_are_kept_whole_or_rolled_back_a_statement_at_a_time() {
    let (_db, server) = served();
    let one = script(&server, &["-1", "-q"], "INSERT INTO T VALUES (3, 'c');\nINSERT INTO T VALUES (1, 'dup');\n");
    assert_eq!(one.outcome, Outcome::Failed);
    let after = script(&server, &["-t", "-A"], "SELECT COUNT(*) FROM T;");
    assert_eq!(after.out, "2\n", "rolled back");
    let saved = "INSERT INTO T VALUES (4, 'd');\nINSERT INTO T VALUES (1, 'dup');\nCOMMIT;\n";
    assert_eq!(script(&server, &["-q"], saved).outcome, Outcome::Failed);
    assert_eq!(script(&server, &["-t", "-A"], "SELECT COUNT(*) FROM T;").out, "3\n", "the good insert kept");
}

/// Verifies: [DESIGN] RULES.md D213, D219: unaligned, CSV, JSON, HTML,
/// tab-separated and expanded rows; `\G` expands one statement's; `\x`
/// and `\format` set the layout.
#[test]
fn rows_are_written_in_every_format() {
    let (_db, server) = served();
    let sql = "SELECT K, S FROM T ORDER BY K;";
    assert_eq!(script(&server, &["-A"], sql).out, "K|S\n1|a\n2|b\n(2 rows)\n");
    assert_eq!(script(&server, &["--csv"], sql).out, "K,S\n1,a\n2,b\n");
    assert_eq!(script(&server, &["--json"], sql).out, "[\n{\"K\":1,\"S\":\"a\"},\n{\"K\":2,\"S\":\"b\"}\n]\n");
    assert_eq!(script(&server, &["-B"], sql).out, "K\tS\n1\ta\n2\tb\n");
    assert!(script(&server, &["--html"], sql).out.starts_with("<table border=\"1\">\n  <tr><th align=\"right\">K</th>"));
    let expanded = "-[ ROW 1 ]\nK | 1\nS | a\n-[ ROW 2 ]\nK | 2\nS | b\n(2 rows)\n";
    assert_eq!(script(&server, &["-x"], sql).out, expanded);
    assert_eq!(script(&server, &[], "SELECT K, S FROM T ORDER BY K\\G\n").out, expanded);
    let set = script(
        &server,
        &[],
        "\\format style unaligned\n\\format separator ;\n\\t\n\\format null -\nSELECT K, CAST(NULL AS INTEGER) FROM T WHERE K = 1;",
    );
    assert_eq!(set.out, "Rows alone are on.\n1;-\n");
}

/// Verifies: [DESIGN] RULES.md D214: `\d` describes a table's columns,
/// types, defaults and nullability, its constraints and foreign keys and
/// the tables that reference it; `\dt` lists tables; `\dn` schemas; a
/// pattern that matches nothing is refused.
#[test]
fn describe_commands_read_the_information_schema() {
    let (_db, server) = served();
    let t = script(&server, &["--null", "NULL"], "\\d t\n");
    assert!(t.out.starts_with("Table X.T\n Column"), "{}", t.out);
    assert!(t.out.contains(" S      | CHARACTER VARYING"), "{}", t.out);
    assert!(t.out.contains("'none'"), "{}", t.out);
    assert!(t.out.contains("Referenced by\n"), "{}", t.out);
    assert!(t.out.contains("PRIMARY KEY"), "{}", t.out);
    let u = script(&server, &[], "\\d X.U\n");
    assert!(u.out.contains("Foreign keys\n"), "{}", u.out);
    let tables = script(&server, &["-A", "-t"], "\\dt\n");
    assert_eq!(tables.out, "X|T|BASE TABLE\nX|U|BASE TABLE\n", "{}", tables.err);
    assert!(script(&server, &["-A", "-t"], "\\dn\n").out.contains("X|X\n"));
    let none = script(&server, &[], "\\d nothing\n");
    assert!(none.err.contains("no table or view matches nothing"), "{}", none.err);
}

/// Verifies: [DESIGN] RULES.md D215, D216: `\i` and `\ir` run files, the
/// latter named from the current file's directory; `\o` sends results to
/// a file; `\export` writes rows that `\import` reads back.
#[test]
fn files_are_included_written_and_read_back() {
    let (_db, server) = served();
    let scratch = Scratch::new("cli-scripts-files");
    std::fs::create_dir_all(scratch.path("sub")).unwrap();
    std::fs::write(scratch.path("sub/outer.sql"), "\\ir inner.sql\n").unwrap();
    std::fs::write(scratch.path("sub/inner.sql"), "\\echo inner\n").unwrap();
    let outer = scratch.path("sub/outer.sql");
    assert_eq!(script(&server, &[], &format!("\\i {}\n", outer.display())).out, "inner\n");
    let into = scratch.path("out.txt");
    let redirected = script(&server, &["-A", "-t"], &format!("\\o {}\nVALUES 7;\n\\o\nVALUES 8;\n", into.display()));
    assert_eq!((redirected.out.as_str(), std::fs::read_to_string(&into).unwrap().as_str()), ("8\n", "7\n"));
    let csv = scratch.path("t.csv");
    let exported = script(&server, &[], &format!("\\export (SELECT K + 10 AS K, S FROM T ORDER BY K) {} header\n", csv.display()));
    assert_eq!(exported.out, "OK, 2 rows exported\n");
    assert_eq!(std::fs::read_to_string(&csv).unwrap(), "K,S\n11,a\n12,b\n");
    let imported = script(&server, &[], &format!("\\import T {} header\n", csv.display()));
    assert_eq!(imported.out, "OK, 2 rows imported\n");
    assert_eq!(script(&server, &["-t", "-A"], "SELECT COUNT(*) FROM T;").out, "4\n");
}

/// Verifies: [DESIGN] RULES.md D215: `\gset` stores a row's columns as
/// variables; `\gexec` runs each value as a statement; `\watch` runs a
/// statement again, COUNT times.
#[test]
fn results_become_variables_and_statements() {
    let (_db, server) = served();
    let set = script(&server, &["-q"], "SELECT K AS N, S FROM T WHERE K = 2 \\gset v_\n\\echo :v_N :v_S\n");
    assert_eq!(set.out, "2 b\n");
    let many = script(&server, &[], "SELECT K FROM T \\gset\n");
    assert!(many.err.contains("gave 2 rows, not one"), "{}", many.err);
    let exec = script(&server, &["-q", "-t", "-A"], "SELECT 'VALUES ' || CAST(K AS VARCHAR(5)) FROM T ORDER BY K \\gexec\n");
    assert_eq!(exec.out, "1\n2\n");
    let watched = script(&server, &["-t", "-A"], "VALUES 5 \\watch 0 2\n");
    assert_eq!(watched.out, "watch every 0 s, run 1\n5\nwatch every 0 s, run 2\n5\n");
}

/// Verifies: [DESIGN] RULES.md D219: SAFE_UPDATES refuses an UPDATE
/// without WHERE, before it is sent; `\delimiter` ends statements with
/// another text, so a semicolon may be within one.
#[test]
fn safe_updates_and_delimiters() {
    let (_db, server) = served();
    let refused = script(&server, &["--safe-updates"], "UPDATE T SET S = 'z';\n");
    assert!(refused.err.contains("SAFE_UPDATES"), "{}", refused.err);
    assert_eq!(script(&server, &["-t", "-A"], "SELECT COUNT(*) FROM T WHERE S = 'z';").out, "0\n");
    let delimited = script(&server, &["-t", "-A"], "\\delimiter //\nVALUES 1;\n\\echo between\n//\n\\delimiter ;\nVALUES 2;\n");
    assert_eq!(delimited.out, "between\n1\n2\n", "{}", delimited.err);
}

/// Verifies: [DESIGN] RULES.md D217, D219: `\conninfo` names the server,
/// user and address; `\c` connects anew and ends the connection before;
/// `\h` writes a statement's syntax.
#[test]
fn the_session_is_shown_and_renewed() {
    let (_db, server) = served();
    let port = server.address.port();
    let info = script(&server, &[], "\\conninfo\n\\c - X\n\\conninfo\n\\echo :USER\n");
    let lines: Vec<&str> = info.out.lines().collect();
    assert!(lines[0].starts_with("Connected to turborel") && lines[0].ends_with(&format!("as X, at 127.0.0.1:{port}.")), "{}", info.out);
    assert!(lines[1].ends_with("as X, by the connection TURBOREL_1."), "{}", info.out);
    assert!(lines[2].ends_with("by the connection TURBOREL_1."), "{}", info.out);
    assert_eq!(lines[3], "X");
    assert!(script(&server, &[], "\\h drop table\n").out.starts_with("DROP TABLE: "));
}

/// Runs `turborel admin` with `words` against `server`, a password
/// typed `typed` and every question answered yes: what it wrote, or why it
/// failed.
fn admin(server: &Running, words: &str, typed: Option<&str>) -> Result<String, String> {
    let port = server.address.port().to_string();
    let mut line: Vec<String> = ["admin", "-U", "X", "-p", port.as_str()].iter().map(|a| (*a).to_owned()).collect();
    line.extend(words.split(' ').map(str::to_owned));
    let Ok(Command::Admin(invocation)) = turborel_cli::parse(&line, &|_| None) else { panic!("{line:?}") };
    let connection = turborel_cli::connect(&invocation.options, || Ok(String::new())).unwrap();
    let mut agent = Agent::connected(Dialer::new(invocation.options.clone(), Box::new(|| Ok(String::new()))), connection);
    let mut out = Vec::new();
    let mut yes = |_: &str| Ok(true);
    let mut secret = |_: &str| Ok(typed.unwrap_or_default().to_owned());
    let mut console = turborel_cli::admin::Console {
        out: &mut out,
        confirm: Some(&mut yes),
        secret: typed.is_some().then_some(&mut secret as &mut dyn FnMut(&str) -> std::io::Result<String>),
    };
    let done = turborel_cli::admin::run(&mut agent, &invocation, &mut console);
    agent.terminate().unwrap();
    done.map(|()| String::from_utf8(out).unwrap())
}

/// Verifies: [DESIGN] RULES.md D218: `turborel admin` shows the server's
/// state, its connections and settings, and its version; creates and
/// drops a schema, asking first; ends and cancels connections by ID; a
/// password is not sent over plain TCP when the server takes no verifier.
#[test]
fn admin_commands_run_as_statements() {
    let (_db, server) = served();
    assert!(admin(&server, "ping", None).unwrap().contains(" answers, in "));
    let status = admin(&server, "status", None).unwrap();
    assert!(status.starts_with("-[ ROW 1 ]"), "{status}");
    let processes = admin(&server, "processlist", None).unwrap();
    assert!(processes.contains(" X "), "{processes}");
    let settings = admin(&server, "variables server.tls_*", None).unwrap();
    assert!(settings.contains("SERVER.TLS_CERTIFICATE") && !settings.contains("SESSION."), "{settings}");
    assert!(admin(&server, "version", None).unwrap().starts_with("turborel"));
    assert_eq!(admin(&server, "create-schema S2", None).unwrap(), "Schema S2 is created.\n");
    assert_eq!(admin(&server, "drop-schema S2", None).unwrap(), "Schema S2 is dropped.\n");
    assert!(admin(&server, "kill 999999", None).is_err());
    assert!(admin(&server, "cancel 999999", None).is_err());
    let refused = admin(&server, "password", Some("new one")).unwrap_err();
    assert!(refused.contains("only over TLS or a local socket"), "{refused}");
    assert!(admin(&server, "password", None).unwrap_err().contains("typed at a terminal"));
}

/// Verifies: [DESIGN] RULES.md D214: the listings of routines, users,
/// types, domains, extensions, property graphs, privileges, indexes,
/// sequences and the catalog each run against the Information Schema.
#[test]
fn every_listing_runs() {
    let (_db, server) = served();
    for command in ["\\df", "\\du", "\\dT", "\\dD", "\\dx", "\\dG", "\\dp", "\\di+", "\\ds", "\\dv", "\\l", "\\d", "\\dtS T*"] {
        let ran = script(&server, &[], &format!("{command}\n"));
        assert_eq!((ran.outcome, ran.err.as_str()), (Outcome::Succeeded, ""), "{command}");
    }
    assert!(script(&server, &["-A", "-t"], "\\du X\n").out.starts_with("X|YES|"));
}

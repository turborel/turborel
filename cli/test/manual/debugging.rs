//! The manual's examples of the routine debugger and its DEBUG privilege
//! (`doc/`), run as printed.

use turborel::db::Database;

use super::Client;

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D1000 to D1002: the examples of
/// GRANT DEBUG run as printed, as ANN.
#[test]
fn the_grant_debug_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/statements/grant-debug.md") > 0, "it shows its examples' output");
}

/// The examples of Debugging Routines, in the page's order: the first
/// and the third the target session's, the rest the debugger's.
const TARGET: [usize; 2] = [0, 2];

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D1030 to D1036: the User's
/// Guide's two-session example of the debugger runs as printed, ANN's
/// debugging client, connection 1, driving the CALL of ANN's other
/// session, connection 2, run by a client on a thread of its own.
#[test]
fn the_debugging_chapter_runs_as_printed() {
    const PAGE: &str = "user/11-debugging.md";
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut debugger = Client::of(&db, "ANN");
    let all = super::examples(PAGE);
    let blocks: Vec<(String, String)> = all
        .iter()
        .map(|e| match e {
            super::Example::Sql { sql, shown } => (sql.clone(), shown.clone().unwrap_or_default()),
            super::Example::Shell(_) => panic!("{PAGE} has no shell session"),
        })
        .collect();
    assert_eq!(blocks.len(), 6, "{PAGE}'s examples");
    let mut given = super::options(&debugger.server, vec![]);
    "ANN".clone_into(&mut given.user);
    let target_sql: Vec<String> = TARGET.iter().map(|&n| blocks[n].0.clone()).collect();
    let (shown_tx, shown_rx) = std::sync::mpsc::channel::<String>();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let target = std::thread::spawn(move || {
        let connection = turborel_cli::connect(&given, || Ok(String::new())).unwrap();
        let dialer = turborel_cli::Dialer::new(given.clone(), Box::new(|| Ok(String::new())));
        let mut agent = turborel_client::agent::Agent::connected(dialer, connection);
        for sql in &target_sql {
            go_rx.recv().unwrap();
            shown_tx.send(super::shown_over(&mut agent, given.clone(), sql)).unwrap();
        }
    });
    for (n, (sql, printed)) in blocks.iter().enumerate() {
        if TARGET.contains(&n) {
            go_tx.send(()).unwrap();
            if n == 0 {
                assert_eq!(&shown_rx.recv().unwrap(), printed, "{PAGE}, example {n}:\n{sql}");
            }
            continue;
        }
        assert_eq!(&debugger.shown(sql), printed, "{PAGE}, example {n}:\n{sql}");
    }
    let (call, printed) = &blocks[TARGET[1]];
    assert_eq!(&shown_rx.recv().unwrap(), printed, "{PAGE}, the CALL:\n{call}");
    target.join().unwrap();
}

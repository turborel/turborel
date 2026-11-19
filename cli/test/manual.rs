//! The manual's examples (`doc/`), run as printed: each block of SQL of a
//! page run by the client, in one session, in the page's order, and what
//! the client shows compared with the block of output printed after it,
//! which every example of a page tested shows.
//! The client runs with `--null NULL`, as the manual says its examples do.

mod backup;
mod embedding;
mod extensions;
mod hybrid;
mod information;
mod partitioning;
mod vector;
mod xml;

use std::cell::RefCell;
use std::io::{self, Write};
use std::path::Path;
use std::rc::Rc;

use turborel::db::Database;
use turborel_cli::{Command, Dialer, Input, Io};
use turborel_client::agent::Agent;

use crate::common::{Running, Scratch, command, options, start};

mod graph;

/// What the manual's examples show a null value as.
const NULL_SHOWN: &str = "NULL";

/// Verifies: [DESIGN] RULES.md R-DOC-8, D42: the examples of CANCEL
/// CONNECTION and END CONNECTION run as printed, by the database's owner,
/// ANN, as connection 1, beside BOB's, connection 2.
#[test]
fn the_connection_statements_run_as_printed() {
    let db = Database::new();
    db.create_user("ANN", b"x").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let bob = ann.server.connect("BOB");
    for page in ["sql/statements/cancel-connection.md", "sql/statements/end-connection.md"] {
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
    drop(bob);
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D43: the examples of ALTER
/// DATABASE run as printed, by the database's owner, ANN, beside the user
/// BOB.
#[test]
fn the_owners_hand_over_runs_as_printed() {
    let db = Database::new();
    db.create_user("ANN", b"x").unwrap();
    db.create_user("BOB", b"x").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/statements/alter-database.md") > 0, "it shows its examples' output");
    assert_eq!(db.owner().unwrap().as_deref(), Some("BOB"));
}

/// The fence that begins a block of SQL.
const SQL_FENCE: &str = "```sql";
/// The fence that begins a block of output, and ends any block.
const FENCE: &str = "```";

/// What begins a line typed at a shell's prompt.
const SHELL_PROMPT: &str = "$ ";

/// One example of a page.
enum Example {
    /// Its SQL, and the output printed after it, if any is.
    Sql { sql: String, shown: Option<String> },
    /// A session at a shell's prompt: the lines typed, each `$ ` and a
    /// command, and what each printed after it.
    Shell(String),
}

/// The lines of a block whose opening fence was just read, to its close.
fn block<'a>(lines: &mut impl Iterator<Item = &'a str>) -> String {
    lines.by_ref().take_while(|l| *l != FENCE).flat_map(|l| [l, "\n"]).collect()
}

/// The text of `page`, a path under `doc/`.
fn text_of(page: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("doc").join(page);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    // A checkout that ends lines as Windows does reads as the page is.
    text.replace("\r\n", "\n")
}

/// The examples of `page`, a path under `doc/`.
fn examples(page: &str) -> Vec<Example> {
    examples_in(&text_of(page))
}

/// The examples of `text`: each block of SQL, and the block of output
/// that follows it, past blank lines alone, if one does; and each block
/// of a session at a shell's prompt.
fn examples_in(text: &str) -> Vec<Example> {
    let mut lines = text.lines().peekable();
    let mut found = Vec::new();
    while let Some(line) = lines.next() {
        if line == FENCE && lines.peek().is_some_and(|l| l.starts_with(SHELL_PROMPT)) {
            found.push(Example::Shell(block(&mut lines)));
            continue;
        }
        if line != SQL_FENCE {
            continue;
        }
        let sql = block(&mut lines);
        while lines.peek() == Some(&"") {
            lines.next();
        }
        let shown = (lines.peek() == Some(&FENCE)).then(|| {
            lines.next();
            block(&mut lines)
        });
        found.push(Example::Sql { sql, shown });
    }
    found
}

/// Output and failures written to one place, in the order written, as a
/// terminal shows them.
#[derive(Clone, Default)]
struct Screen(Rc<RefCell<Vec<u8>>>);

impl Write for Screen {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The client, connected to `server` as `user`, its session kept from one
/// example to the next; dropped, its connection before its server.
struct Client {
    agent: Agent<Dialer>,
    user: String,
    server: Running,
}

impl Client {
    /// The client of `db`'s server, as `user`.
    fn of(db: &Database, user: &str) -> Client {
        let server = start(db, &[]);
        let mut given = options(&server, vec![]);
        user.clone_into(&mut given.user);
        let connection = turborel_cli::connect(&given, || Ok(String::new())).unwrap();
        let agent = Agent::connected(Dialer::new(given, Box::new(|| Ok(String::new()))), connection);
        Client { agent, user: user.to_owned(), server }
    }

    /// What the client shows as it runs `sql`, null shown as the manual
    /// shows it.
    fn shown(&mut self, sql: &str) -> String {
        let mut given = options(&self.server, vec![command(sql)]);
        given.user.clone_from(&self.user);
        NULL_SHOWN.clone_into(&mut given.null);
        let screen = Screen::default();
        let (mut out, mut err) = (screen.clone(), screen.clone());
        let io = Io { input: &mut "".as_bytes(), typed: false, out: &mut out, err: &mut err, interrupts: None };
        turborel_cli::run(&mut self.agent, &given, io);
        String::from_utf8(screen.0.take()).unwrap()
    }

    /// Runs each example of SQL of `page`, in order: each shows the output
    /// printed after it. How many there were.
    fn page(&mut self, page: &str) -> usize {
        self.run(page, &examples(page), None)
    }

    /// Runs `all`, examples of `page`, in order: each shows the output
    /// printed after it; a session at a shell's prompt is run if
    /// `scratch` holds its files, else passed over. How many there were.
    fn run(&mut self, page: &str, all: &[Example], scratch: Option<&Scratch>) -> usize {
        for (n, example) in all.iter().enumerate() {
            match (example, scratch) {
                (Example::Sql { sql, shown: printed }, _) => {
                    let shown = self.shown(sql);
                    let printed = printed.as_deref().unwrap_or_else(|| panic!("{page}, example {n}, shows no output; it shows:\n{shown}"));
                    assert_eq!(shown, printed, "{page}, example {n}:\n{sql}");
                }
                (Example::Shell(session), Some(files)) => self.session(session, files),
                (Example::Shell(_), None) => {}
            }
        }
        all.len()
    }

    /// Runs `session`, typed at a shell's prompt: each `cat` shows a file
    /// it writes in `scratch`; each `turborel` runs as its command line
    /// says, at the client's server, and shows, its output and failures as
    /// a terminal shows them, the lines printed after it.
    fn session(&mut self, session: &str, scratch: &Scratch) {
        let mut lines = session.lines().peekable();
        while let Some(typed) = lines.next() {
            let printed: String = std::iter::from_fn(|| lines.next_if(|l| !l.starts_with(SHELL_PROMPT))).flat_map(|l| [l, "\n"]).collect();
            if let Some(name) = typed.strip_prefix("$ cat ") {
                std::fs::write(scratch.path(name), &printed).unwrap();
                continue;
            }
            let args = typed.strip_prefix("$ turborel ").unwrap_or_else(|| panic!("not run: {typed}"));
            assert_eq!(self.ran(args, scratch), printed, "{typed}");
        }
    }

    /// What `turborel` shows run with `args`, at the client's server, the
    /// files it names in `scratch`, named as given.
    fn ran(&self, args: &str, scratch: &Scratch) -> String {
        let port = self.server.address.port().to_string();
        let line: Vec<String> = args.split_whitespace().chain(["-p", &port]).map(str::to_owned).collect();
        let Ok(Command::Run(mut given)) = turborel_cli::parse(&line, &|_| None) else { panic!("{args}") };
        for input in &mut given.inputs {
            if let Input::Import(path) = input {
                *path = scratch.path(&path.display().to_string());
            }
        }
        let connection = turborel_cli::connect(&given, || Ok(String::new())).unwrap();
        let mut agent = Agent::connected(Dialer::new((*given).clone(), Box::new(|| Ok(String::new()))), connection);
        let screen = Screen::default();
        let (mut out, mut err) = (screen.clone(), screen.clone());
        let io = Io { input: &mut "".as_bytes(), typed: false, out: &mut out, err: &mut err, interrupts: None };
        turborel_cli::run(&mut agent, &given, io);
        agent.terminate().unwrap();
        String::from_utf8(screen.0.take()).unwrap().replace(&format!("{}{}", scratch.dir.display(), std::path::MAIN_SEPARATOR), "")
    }

    /// Runs the examples of the section `heading` of `page`, SQL and
    /// sessions at a shell's prompt alike, their files in `scratch`. How
    /// many there were.
    fn section(&mut self, page: &str, heading: &str, scratch: &Scratch) -> usize {
        let text = text_of(page);
        let (_, from) = text.split_once(&format!("\n{heading}\n")).unwrap_or_else(|| panic!("{page} has no {heading}"));
        let within = from.split("\n## ").next().unwrap_or_default();
        self.run(page, &examples_in(within), Some(scratch))
    }
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D59: the Client Reference's
/// examples of importing run as printed, as ANN, each file shown made.
#[test]
fn the_import_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let scratch = Scratch::new("manual-import");
    assert!(ann.section("client/01-turborel.md", "## Importing", &scratch) > 0, "the section shows its examples");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8: the User's Guide's examples, of
/// Getting Started on, run as printed, one session's, as ANN.
#[test]
fn the_users_guide_runs_as_printed() {
    let db = Database::new();
    let mut ann = Client::of(&db, "ANN");
    for page in
        ["user/02-getting-started.md", "user/03-concepts.md", "user/04-queries.md", "user/05-changing-data.md", "user/07-constraints.md"]
    {
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
}

/// Verifies: [DESIGN] RULES.md R-DOC-8: the SQL Reference Manual's
/// examples of value expressions, predicates, query expressions and
/// JSON_TABLE, of the tables the first makes, and INSERT's with UPDATE's, and with
/// DELETE's, of the table the first makes, run as printed, as ANN.
#[test]
fn the_reference_manuals_examples_run_as_printed() {
    let chapters = ["sql/04-expressions.md", "sql/05-predicates.md", "sql/06-queries.md", "sql/13-json-table.md"];
    let deleted = ["sql/statements/insert.md", "sql/statements/delete.md"];
    for pages in [chapters.as_slice(), &["sql/statements/insert.md", "sql/statements/update.md"], &deleted] {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
        let mut ann = Client::of(&db, "ANN");
        for page in pages {
            assert!(ann.page(page) > 0, "{page} shows its examples' output");
        }
    }
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D84: the SQL Reference Manual's
/// chapter of the SQL/JSON path language runs as printed, as ANN, and the
/// guide to JSON data, which makes ANN's schema itself, does too.
#[test]
fn the_json_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/11-json-path.md") > 0, "the chapter shows its examples' output");
    let db = Database::new();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("data/03-json.md") > 0, "the guide shows its examples' output");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D64: the examples of TRUNCATE
/// TABLE run as printed, as ANN.
#[test]
fn the_truncate_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/statements/truncate-table.md") > 0, "it shows its examples' output");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D88. The examples of SAVEPOINT,
/// RELEASE SAVEPOINT and ROLLBACK run as printed, in that order, as ANN,
/// whose ACCOUNTS holds one row of BALANCE 100.00, then those of the
/// User's Guide's section on savepoints.
#[test]
fn the_savepoint_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE ACCOUNTS (ID INTEGER, BALANCE DECIMAL(9,2))").unwrap();
    let setup = db.session("ANN");
    setup.run("INSERT INTO ACCOUNTS VALUES (1, 100.00)").unwrap();
    setup.commit().unwrap();
    drop(setup);
    let mut ann = Client::of(&db, "ANN");
    for page in ["sql/statements/savepoint.md", "sql/statements/release-savepoint.md", "sql/statements/rollback.md"] {
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
    let scratch = Scratch::new("manual-savepoints");
    assert!(ann.section("user/06-transactions.md", "## Savepoints", &scratch) > 0, "the section shows its examples");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8: the examples of EXPLAIN run as
/// printed, as ANN, the client showing its plan's lines as rows.
#[test]
fn the_explain_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/statements/explain.md") > 0, "it shows its examples' output");
}

/// Verifies: [EXTENSION] RULES.md R-DOC-8, D92: the examples of
/// CREATE INDEX of key parts that are expressions run as printed, as ANN:
/// those of table PARCELS, made, read through its indexes and refused.
#[test]
fn the_expression_index_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let page = "sql/statements/create-index.md";
    let parcels: Vec<Example> =
        examples(page).into_iter().filter(|e| matches!(e, Example::Sql { sql, .. } if sql.contains("PARCELS"))).collect();
    assert_eq!(ann.run(page, &parcels, None), 2, "it shows its examples' output");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D65, D66: the examples of CREATE
/// TABLE run as printed, as ANN, those of LIKE and AS, of UNIQUE NULLS
/// NOT DISTINCT, and of node and edge tables (D143), among
/// them.
#[test]
fn the_create_table_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/statements/create-table.md") > 0, "it shows its examples' output");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D36: the Operator's Guide's
/// examples of settings and of the server's connections run as printed, by
/// the database's owner, ANN.
#[test]
fn the_operators_guide_runs_as_printed() {
    let db = Database::new();
    db.create_user("ANN", b"x").unwrap();
    db.set_owner("ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    for page in ["operator/04-settings.md", "operator/06-network.md"] {
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D58: the SQL Reference Manual's
/// examples of dynamic SQL that direct SQL runs, of SQL descriptor areas
/// and EXECUTE IMMEDIATE, run as printed, as ANN, one page after another.
#[test]
fn the_dynamic_sql_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    for page in [
        "sql/statements/allocate-descriptor.md",
        "sql/statements/set-descriptor.md",
        "sql/statements/deallocate-descriptor.md",
        "sql/statements/execute-immediate.md",
    ] {
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D89: the examples of the
/// JSON type run as printed, through the client, as ANN, so a value of it
/// crosses the wire both ways.
#[test]
fn the_json_type_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    assert!(ann.page("sql/12-json-type.md") > 0, "it shows its examples' output");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D69: the examples of CREATE TYPE,
/// then those of DROP TYPE of its types and table, run as printed, as ANN.
#[test]
fn the_distinct_type_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    for page in ["sql/statements/create-type.md", "sql/statements/drop-type.md"] {
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D81: the examples of the
/// statements of structured types, each page in a session of its own, as
/// ANN: ALTER TYPE, CREATE METHOD, the casts and the orderings.
#[test]
fn the_structured_type_examples_run_as_printed() {
    let pages = [
        "sql/statements/alter-type.md",
        "sql/statements/create-method.md",
        "sql/statements/create-cast.md",
        "sql/statements/drop-cast.md",
        "sql/statements/create-ordering.md",
        "sql/statements/drop-ordering.md",
    ];
    for page in pages {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
        assert!(Client::of(&db, "ANN").page(page) > 0, "{page} shows its examples' output");
    }
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D81: the examples of the transform
/// statements and SET TRANSFORM GROUP, each page in a session of its own,
/// as ANN.
#[test]
fn the_transform_examples_run_as_printed() {
    let pages = [
        "sql/statements/create-transform.md",
        "sql/statements/alter-transform.md",
        "sql/statements/drop-transform.md",
        "sql/statements/set-transform-group.md",
    ];
    for page in pages {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
        assert!(Client::of(&db, "ANN").page(page) > 0, "{page} shows its examples' output");
    }
}

/// Verifies: [DESIGN] RULES.md R-DOC-8: the examples of CREATE TRIGGER,
/// then those of DROP TRIGGER, run as printed, as ANN.
#[test]
fn the_trigger_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    for page in ["sql/statements/create-trigger.md", "sql/statements/drop-trigger.md"] {
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
}

/// Verifies: [DESIGN] RULES.md R-DOC-8: the examples of CREATE FUNCTION,
/// CREATE PROCEDURE, CALL, RETURN and DROP ROUTINE run as printed, in
/// that order, as ANN.
#[test]
fn the_routine_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let pages = [
        "sql/statements/create-function.md",
        "sql/statements/create-procedure.md",
        "sql/statements/call.md",
        "sql/statements/return.md",
        "sql/statements/drop-routine.md",
    ];
    for page in pages {
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
}

/// Verifies: [DESIGN] RULES.md R-DOC-8: the examples of the SQL/PSM
/// statements, BEGIN, DECLARE HANDLER, SET, CASE, IF, LOOP, WHILE,
/// REPEAT, FOR, LEAVE, ITERATE, SIGNAL and RESIGNAL, and of the PTF
/// statements, PIPE ROW and COPY DESCRIPTOR, each page in a database of
/// its own, run as printed, as ANN.
#[test]
fn the_psm_examples_run_as_printed() {
    let pages = [
        "sql/statements/begin.md",
        "sql/statements/declare-handler.md",
        "sql/statements/set-variable.md",
        "sql/statements/case-statement.md",
        "sql/statements/if.md",
        "sql/statements/loop.md",
        "sql/statements/while.md",
        "sql/statements/repeat.md",
        "sql/statements/for.md",
        "sql/statements/leave.md",
        "sql/statements/iterate.md",
        "sql/statements/signal.md",
        "sql/statements/resignal.md",
        "sql/statements/pipe-row.md",
        "sql/statements/copy-descriptor.md",
    ];
    for page in pages {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
        let mut ann = Client::of(&db, "ANN");
        assert!(ann.page(page) > 0, "{page} shows its examples' output");
    }
}

/// A database of the geometry types and their functions, which are built
/// in (RULES.md D120).
fn spatial_database() -> Database {
    Database::new()
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D85: the SQL Reference Manual's
/// chapter of spatial data, and the guide to it, each in a database of
/// its own with GEOMETRY registered, run as printed, as ANN.
#[test]
fn the_spatial_examples_run_as_printed() {
    for page in ["sql/10-spatial.md", "data/02-spatial.md"] {
        let db = spatial_database();
        assert!(Client::of(&db, "ANN").page(page) > 0, "{page} shows its examples' output");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11] RULES.md
/// R-DOC-8: the SQL Reference Manual's chapter of topology-geometry runs
/// as printed, as ANN.
#[test]
fn the_topology_examples_run_as_printed() {
    let db = spatial_database();
    assert!(Client::of(&db, "ANN").page("sql/16-topology.md") > 0, "it shows its examples' output");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15] RULES.md
/// R-DOC-8: the SQL Reference Manual's chapter of linear referencing runs
/// as printed, as ANN.
#[test]
fn the_linear_referencing_examples_run_as_printed() {
    let db = spatial_database();
    assert!(Client::of(&db, "ANN").page("sql/17-linear-referencing.md") > 0, "it shows its examples' output");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D129: the SQL
/// Reference Manual's chapter of spatial networks and shortest paths runs
/// as printed, as ANN.
#[test]
fn the_spatial_network_examples_run_as_printed() {
    let db = spatial_database();
    assert!(Client::of(&db, "ANN").page("sql/18-spatial-networks.md") > 0, "it shows its examples' output");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D85: the examples of CREATE INDEX
/// run as printed, as ANN, of its table STAFF and a view V of it, with
/// GEOMETRY registered for its R-tree of locations.
#[test]
fn the_create_index_examples_run_as_printed() {
    let db = spatial_database();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION ANN \
         CREATE TABLE STAFF (ID INTEGER PRIMARY KEY, NAME VARCHAR(30), DEPT CHAR(3)) \
         CREATE VIEW V AS SELECT ID FROM STAFF",
    )
    .unwrap();
    assert!(Client::of(&db, "ANN").page("sql/statements/create-index.md") > 0, "it shows its examples' output");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8: the SQL Reference Manual's
/// chapters of full-text search and its patterns, and the guide to it,
/// each in a database of its own, run as printed, as ANN, each file a
/// page shows made imported.
#[test]
fn the_fulltext_examples_run_as_printed() {
    let scratch = Scratch::new("manual-fulltext");
    for page in ["sql/08-fulltext.md", "sql/09-fulltext-patterns.md", "data/01-fulltext.md"] {
        let db = Database::new();
        assert!(Client::of(&db, "ANN").run(page, &examples(page), Some(&scratch)) > 0, "{page} shows its examples' output");
    }
}

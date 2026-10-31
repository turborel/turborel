//! The manual's examples (`doc/`), run as printed: each block of SQL of a
//! page run by the client, in one session, in the page's order, and what
//! the client shows compared with the block of output printed after it,
//! which every example of a page tested shows.
//! The client runs with `--null NULL`, as the manual says its examples do.

use std::cell::RefCell;
use std::io::{self, Write};
use std::path::Path;
use std::rc::Rc;

use turborel::db::Database;
use turborel_cli::{Command, Dialer, Input, Io};
use turborel_client::agent::Agent;

use crate::common::{Running, Scratch, command, options, start};

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
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
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
/// examples of value expressions and query expressions, of the tables the
/// first makes, and INSERT's and UPDATE's, of the table the first makes,
/// run as printed, as ANN.
#[test]
fn the_reference_manuals_examples_run_as_printed() {
    for pages in [["sql/04-expressions.md", "sql/06-queries.md"], ["sql/statements/insert.md", "sql/statements/update.md"]] {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
        let mut ann = Client::of(&db, "ANN");
        for page in pages {
            assert!(ann.page(page) > 0, "{page} shows its examples' output");
        }
    }
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

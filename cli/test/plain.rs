//! Runs over a connection that is no SQL-agent's, and over one whose
//! server tells no session status, so the client asks the Information
//! Schema itself (RULES.md D46, D190).

use std::io::Write;
use std::net::TcpStream;

use turborel::db::Database;
use turborel_cli::{FileSource, Input, Io, Outcome, Statements, Terminal};
use turborel_client::connection::{Connection, Sink, Streamed};
use turborel_client::import::{Import, ImportError, Imported, Rejected};
use turborel_client::{Canceller, Failure};

use crate::common::{Running, Scratch, options, start};

/// A server of a database X owns, where X has table T, holding 1 to 3.
fn served() -> (Database, Running) {
    let db = Database::new();
    db.create_user("X", b"x").unwrap();
    db.set_owner("X").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let server = start(&db, &[]);
    let mut filling = server.connect("X");
    filling.query("INSERT INTO T VALUES (1), (2), (3)").unwrap();
    filling.query("COMMIT").unwrap();
    filling.terminate().unwrap();
    (db, server)
}

/// Runs `text`, typed if `typed`, over `connection`: how it ended, its
/// output and its failures.
fn over<S: Statements + ?Sized>(server: &Running, connection: &mut S, text: &str, typed: bool) -> (Outcome, String, String) {
    let mut given = options(server, vec![Input::Standard]);
    given.settings.layout.tuples_only = true;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut input = text.as_bytes();
    let io = Io { input: &mut input, typed, out: &mut out, err: &mut err, interrupts: None, terminal: Terminal::default() };
    let outcome = turborel_cli::outcome(connection, &given, io);
    (outcome, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

/// Verifies: [DESIGN] RULES.md D46, D195, D212: over a connection that is
/// no SQL-agent's, FETCH_COUNT fetches a query's rows through a cursor,
/// and a statement that is no query runs as it would; `\export ...
/// native` writes the protocol's frames; `\conninfo` names the address and
/// the transaction the export left active.
/// as there is no connection's name.
#[test]
fn a_plain_connection_fetches_exports_and_tells_where() {
    let (_db, server) = served();
    let scratch = Scratch::new("cli-plain");
    let native = scratch.path("t.native");
    let text = format!(
        "\\set FETCH_COUNT 2\nSELECT K FROM T ORDER BY K;\nDELETE FROM T WHERE K = 9;\n\\export T {} native\n\\conninfo\n",
        native.display()
    );
    let mut connection = server.connect("X");
    let (outcome, out, err) = over(&server, &mut connection, &text, false);
    assert_eq!(outcome, Outcome::Succeeded, "{err}");
    let port = server.address.port();
    assert!(out.starts_with("            1\n            2\n            3\nno data\nOK, 3 rows exported\n"), "{out}");
    assert!(out.ends_with(&format!("as X, at 127.0.0.1:{port}, a transaction active.\n")), "{out}");
    let (_, rows) = turborel_client::export::read_native(&mut std::fs::File::open(&native).unwrap()).unwrap();
    assert_eq!(rows.len(), 3);
    assert!(connection.canceller().is_some());
}

/// A connection whose server tells no session status, server time,
/// notices or Ping: the trait's defaults. Its server ends it, as an
/// operator would, as the statement or `\import` it names begins.
struct Untold<'a>(Connection<TcpStream>, Option<(&'a Running, &'static str)>);

impl Untold<'_> {
    /// Ends the connection if `what` is what ends it.
    fn ending(&self, what: &str) {
        if let Some((server, at)) = self.1
            && at == what
        {
            let (id, _) = server.operator.connections().into_iter().find(|(_, u)| u.as_deref() == Some("X")).unwrap();
            assert!(server.operator.end(id));
        }
    }
}

impl Statements for Untold<'_> {
    fn query_into(&mut self, sql: &str, sink: &mut dyn Sink) -> Result<Streamed, Failure> {
        self.ending(sql);
        self.0.query_into(sql, sink)
    }
    fn fetch_into(&mut self, sql: &str, rows: u32, sink: &mut dyn Sink) -> Result<Streamed, Failure> {
        Statements::fetch_into(&mut self.0, sql, rows, sink)
    }
    fn canceller(&self) -> Option<Canceller> {
        None
    }
    fn is_lost(&self) -> bool {
        self.0.is_lost()
    }
    fn warning(&self) -> Option<&str> {
        self.0.warning()
    }
    fn export_native(&mut self, sql: &str, out: &mut dyn Write) -> Result<u64, Failure> {
        Statements::export_native(&mut self.0, sql, out)
    }
    fn in_transaction(&mut self) -> bool {
        self.0.in_transaction()
    }
    fn set_autocommit(&mut self, on: bool) {
        self.0.set_autocommit(on);
    }
    fn session(&mut self) -> Option<(String, String)> {
        Statements::session(&mut self.0)
    }
    fn connection_name(&self) -> Option<String> {
        Statements::connection_name(&self.0)
    }
    fn import(&mut self, source: &mut FileSource, how: &Import, rejected: &mut dyn FnMut(Rejected)) -> Result<Imported, ImportError> {
        self.ending("import");
        Statements::import(&mut self.0, source, how, rejected)
    }
}

/// Verifies: [DESIGN] RULES.md D190: of a server that tells no session
/// status, a prompt that shows the catalog and whether the user is the
/// owner asks the Information Schema once, unseen, no transaction begun
/// by it; a prompt that shows neither asks nothing; the default schema a
/// SET SCHEMA set is shown as the client kept it.
#[test]
fn an_untold_status_is_probed() {
    let (_db, server) = served();
    let mut untold = Untold(server.connect("X"), None);
    let text = "\\set PROMPT '%c%#>'\nVALUES 1;\n\\set PROMPT '%s>'\nSET SCHEMA 'X';\nCOMMIT;\n";
    let (outcome, out, err) = over(&server, &mut untold, text, true);
    assert_eq!(outcome, Outcome::Succeeded, "{err}");
    assert!(out.starts_with("turborel> TURBOREL#>   1\nTURBOREL#>X>OK\nX>OK\nX>"), "{out}");
    assert!(!untold.0.in_transaction(), "the probes began no transaction");
    assert_eq!(untold.server_time(), None);
    assert_eq!(untold.ping(), None);
    assert_eq!(untold.status("CATALOG"), None);
    assert_eq!(untold.take_notices().len(), 0);
}

/// Verifies: [DESIGN] RULES.md D215: a connection lost as a statement
/// runs ends the run there, its outcome Lost, exit status 2, the
/// statements after it not run; so does one lost as `\import` runs.
#[test]
fn a_lost_connection_ends_the_run() {
    let (_db, server) = served();
    let mut ended = Untold(server.connect("X"), Some((&server, "VALUES 'end'")));
    let (outcome, out, err) = over(&server, &mut ended, "VALUES 1;\nVALUES 'end';\nVALUES 2;\n", false);
    assert_eq!((outcome, outcome.code()), (Outcome::Lost, 2));
    assert_eq!(out, "   1\n");
    assert!(err.starts_with("turborel: SQLCODE -919"), "{err}");
    let scratch = Scratch::new("cli-plain-lost");
    let file = scratch.path("rows.csv");
    std::fs::write(&file, "9\n").unwrap();
    let mut importing = Untold(server.connect("X"), Some((&server, "import")));
    let (outcome, _, err) = over(&server, &mut importing, &format!("\\import T {} csv\nVALUES 2;\n", file.display()), false);
    assert_eq!(outcome, Outcome::Lost, "{err}");
}

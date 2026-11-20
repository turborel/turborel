//! Sessions typed at a terminal: lines read by a line editor, completed
//! by tab from the Information Schema and kept in its history; the
//! prompt; `\password` typed twice; `\conninfo` over TLS.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;

use turborel::db::Database;
use turborel_cli::{Completion, Credentials, Dialer, Io, Lines, Outcome, Server, Terminal, Tls};
use turborel_client::agent::Agent;
use turborel_client::connection::Password;

use crate::common::{Running, Scratch, Seat, Typed, command, options, pki, session, start};
use crate::sessions::with_passwords;

/// A server of a database X owns, where X has table T.
fn served() -> (Database, Running) {
    let db = Database::new();
    db.create_user("X", b"x").unwrap();
    db.set_owner("X").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER, KIND INTEGER)").unwrap();
    let server = start(&db, &[]);
    (db, server)
}

/// What a line editor is asked to do next.
enum Step {
    /// Complete this text before the cursor, then go on.
    Complete(&'static str),
    /// Give this line as typed.
    Line(String),
    /// Fail to read the terminal.
    Broken,
}

/// What a scripted line editor saw.
#[derive(Default)]
struct Seen {
    prompts: Vec<String>,
    completions: Vec<Completion>,
    remembered: Vec<(String, usize, Option<PathBuf>)>,
}

/// A line editor that types its steps, then ends the input.
struct Typist {
    steps: VecDeque<Step>,
    seen: Rc<RefCell<Seen>>,
}

impl Lines for Typist {
    fn read_line(&mut self, prompt: &str, complete: &mut dyn FnMut(&str) -> Completion) -> io::Result<Option<String>> {
        self.seen.borrow_mut().prompts.push(prompt.to_owned());
        loop {
            match self.steps.pop_front() {
                Some(Step::Complete(before)) => {
                    let found = complete(before);
                    self.seen.borrow_mut().completions.push(found);
                }
                Some(Step::Line(line)) => return Ok(Some(line)),
                Some(Step::Broken) => return Err(io::Error::other("the terminal is gone")),
                None => return Ok(None),
            }
        }
    }

    fn remember(&mut self, entry: &str, size: usize, file: Option<&std::path::Path>) {
        self.seen.borrow_mut().remembered.push((entry.to_owned(), size, file.map(std::path::Path::to_path_buf)));
    }
}

/// A seat whose line editor types `steps`, and what it saw.
fn typist(steps: Vec<Step>) -> (Seat, Rc<RefCell<Seen>>) {
    let seen = Rc::new(RefCell::new(Seen::default()));
    let editor = Typist { steps: steps.into(), seen: Rc::clone(&seen) };
    (Seat { editor: Some(Box::new(editor)), ..Seat::default() }, seen)
}

/// Verifies: [DESIGN] RULES.md D211: a word is completed by what its
/// context allows, the names read from the Information Schema once: a
/// backslash command after `\`, a table after FROM, by its name and its
/// schema's, a column after WHERE among the key words, a schema after
/// SCHEMA, a user after TO, and a key word elsewhere, in lower case if
/// typed so.
#[test]
fn words_are_completed_from_the_information_schema() {
    let (_db, server) = served();
    let steps = vec![
        Step::Complete("\\conn"),
        Step::Complete("SELECT K FROM "),
        Step::Complete("SELECT K FROM T WHERE KI"),
        Step::Complete("SET SCHEMA "),
        Step::Complete("GRANT SELECT ON T TO "),
        Step::Complete("sel"),
        Step::Complete("SELECT K FROM T GROUP B"),
        Step::Line("VALUES 1;".into()),
    ];
    let (seat, seen) = typist(steps);
    let ran = session(&server, &["-t", "-A"], "", true, seat);
    assert_eq!(ran.outcome, Outcome::Succeeded, "{}", ran.err);
    let seen = seen.borrow();
    let words: Vec<(usize, Vec<String>)> = seen.completions.iter().map(|c| (c.start, c.candidates.clone())).collect();
    assert_eq!(words[0], (1, vec!["conninfo".to_owned()]));
    assert_eq!(words[1].0, 14);
    assert!(words[1].1.contains(&"T".to_owned()) && words[1].1.contains(&"X.T".to_owned()), "{:?}", words[1]);
    assert_eq!(words[2], (22, vec!["KIND".to_owned()]));
    assert_eq!(words[3], (11, vec!["X".to_owned()]), "the schemas X may see");
    assert!(words[4].1.contains(&"X".to_owned()), "{:?}", words[4]);
    assert_eq!(words[5], (0, vec!["select".to_owned()]));
    assert!(words[6].1.contains(&"BY".to_owned()), "a key word after one no context names: {:?}", words[6]);
}

/// Verifies: [DESIGN] RULES.md D211: each statement or command typed is
/// kept in the line editor's history once it is whole, a statement of
/// several lines as one entry, at most HISTORY_SIZE entries, in the file
/// HISTORY_FILE names; the editor is prompted for each line, the prompt
/// for the rest of a statement begun its own.
#[test]
fn typed_entries_are_remembered_whole() {
    let (_db, server) = served();
    let scratch = Scratch::new("cli-typed-history");
    let file = scratch.path("history");
    let set = format!("\\set HISTORY_FILE {}", file.display());
    let steps = vec![Step::Line("\\set HISTORY_SIZE 7".into()), Step::Line(set), Step::Line("VALUES".into()), Step::Line("(1);".into())];
    let (at, seen) = typist(steps);
    let ran = session(&server, &["-t", "-A"], "", true, at);
    assert_eq!(ran.outcome, Outcome::Succeeded, "{}", ran.err);
    assert_eq!(ran.out, "1\n\n");
    let seen = seen.borrow();
    let last = seen.remembered.last().unwrap();
    assert_eq!((last.0.as_str(), last.1, last.2.as_deref()), ("VALUES\n(1);", 7, Some(file.as_path())));
    assert_eq!(seen.remembered.len(), 3);
    assert_eq!(seen.prompts, ["turborel> ", "turborel> ", "turborel> ", "turborel- ", "turborel> "]);
}

/// Verifies: [DESIGN] RULES.md D46, D190: a terminal that cannot be read
/// stops the run, saying so; a prompt asking for the catalog and the
/// user's standing shows them, as the server tells them.
#[test]
fn an_unreadable_terminal_stops_the_run() {
    let (_db, server) = served();
    let (seat, seen) = typist(vec![Step::Line("\\set PROMPT '%c%# '".into()), Step::Broken, Step::Line("VALUES 1;".into())]);
    let ran = session(&server, &[], "", true, seat);
    assert_eq!(ran.outcome, Outcome::Failed);
    assert_eq!(ran.err, "turborel: cannot read: the terminal is gone\n");
    assert!(!ran.out.contains("(1 row)"), "{}", ran.out);
    let prompts = seen.borrow().prompts.clone();
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    assert!(prompts[1].ends_with("# "), "the owner's prompt: {prompts:?}");
}

/// Runs `text`, typed, over TLS to `server`, as ANN, whose password is
/// "ann's", `secret` reading what is typed for `\password`.
fn as_ann(server: &Running, pki: &crate::common::Pki, text: &str, secret: Option<Box<Typed>>) -> (String, String) {
    let mut given = options(server, vec![turborel_cli::Input::Standard]);
    given.user = "ann".into();
    given.credentials = Credentials::Given(Password("ann's".into()));
    let tls = Tls { roots: Some(pki.authority.clone()), system: false, name: Some("localhost".into()), identity: None };
    given.server = Server::Tcp { host: "127.0.0.1".into(), port: server.address.port(), tls: Some(tls) };
    let connection = turborel_cli::connect(&given, || panic!("not typed")).unwrap();
    let mut agent = Agent::connected(Dialer::new(given.clone(), Box::new(|| Ok(String::new()))), connection);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut secret = secret;
    let typing = secret.as_deref_mut().map(|s| s as &mut dyn FnMut(&str) -> io::Result<String>);
    let terminal = Terminal { secret: typing, ..Terminal::default() };
    let mut input = text.as_bytes();
    let io = Io { input: &mut input, typed: false, out: &mut out, err: &mut err, interrupts: None, terminal };
    turborel_cli::outcome(&mut agent, &given, io);
    agent.terminate().unwrap();
    (String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

/// Verifies: [DESIGN] RULES.md D43, D217: `\password` asks for the new
/// password twice: with nothing to type it at, two that differ, or a
/// terminal that cannot be read, it is not changed; typed alike over
/// TLS, where a server that takes no verifier is sent the password, it
/// is, and the old one no longer proves the user. `\conninfo` says the
/// connection is secured.
#[test]
fn password_is_typed_twice_and_set() {
    let pki = pki("cli-typed-password");
    let (_db, server) = with_passwords(&pki);
    let (_, untyped) = as_ann(&server, &pki, "\\password\n", None);
    assert_eq!(untyped, "turborel: \\password: a password is typed at a terminal\n");
    let mut typed = vec!["two", "one"];
    let differ = move |_: &str| Ok(typed.pop().unwrap().to_owned());
    let (_, differs) = as_ann(&server, &pki, "\\password\n", Some(Box::new(differ)));
    assert_eq!(differs, "turborel: \\password: the two do not match; the password is not changed\n");
    let broken = |_: &str| Err(io::Error::other("no tty"));
    let (_, unread) = as_ann(&server, &pki, "\\password ANN\n", Some(Box::new(broken)));
    assert_eq!(unread, "turborel: \\password: cannot read it: no tty\n");
    let asked = Rc::new(RefCell::new(Vec::new()));
    let prompts = Rc::clone(&asked);
    let alike = move |prompt: &str| {
        prompts.borrow_mut().push(prompt.to_owned());
        Ok("ann's new".to_owned())
    };
    let (out, err) = as_ann(&server, &pki, "\\conninfo\n\\password\n", Some(Box::new(alike)));
    assert_eq!(err, "");
    let port = server.address.port();
    assert!(out.contains(&format!("as ANN, at 127.0.0.1:{port}, secured.\n")), "{out}");
    assert!(out.ends_with("The password of ANN is changed.\n"), "{out}");
    assert_eq!(*asked.borrow(), ["new password for ANN: ", "again: "]);
    let mut given = options(&server, vec![command("VALUES 1")]);
    given.user = "ann".into();
    given.credentials = Credentials::Given(Password("ann's".into()));
    let tls = Tls { roots: Some(pki.authority.clone()), system: false, name: Some("localhost".into()), identity: None };
    given.server = Server::Tcp { host: "127.0.0.1".into(), port, tls: Some(tls) };
    assert!(turborel_cli::connect(&given, || panic!("not typed")).is_err(), "the old password no longer proves ANN");
}

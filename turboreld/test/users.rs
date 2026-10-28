//! Users managed by `turboreld --dir PATH user ...`, on a database no
//! server holds; then served, clients proving themselves by password.

use std::io::Cursor;
use std::path::Path;

use turborel_client::connection::{self, Password, Trust};
use turborel_test_support::Scratch;
use turborel_test_support::client::state;
use turboreld::{Command, Config, UserCommand, parse};

/// The configuration of `line`, a command line.
fn config(line: &str) -> Config {
    let args: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
    match parse(&args).unwrap() {
        Command::Serve(config) | Command::User(config, _) => config,
        Command::Help => panic!("help"),
    }
}

/// Runs `command` on the database in `dir`, `input` its standard input:
/// what it wrote, or why it failed.
fn user(dir: &Path, command: &UserCommand, input: &str) -> Result<String, String> {
    let config = config(&format!("--dir {} --set SERVER.PASSWORD_ITERATIONS=10000", dir.display()));
    let mut out = Vec::new();
    turboreld::run_user(&config, command, (&mut Cursor::new(input.as_bytes()), &mut out))?;
    Ok(String::from_utf8(out).unwrap())
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4]; [DESIGN] RULES.md D43:
/// an operator adds users, each name an identifier, its password read from
/// standard input, the first made the owner; lists them, the owner
/// marked; gives one a new password, makes another the owner, and drops
/// one; each kept with the database. Refused, saying why: a user twice, one
/// not there, the owner dropped, a name not an identifier, no password,
/// and a database kept nowhere.
#[test]
fn an_operator_manages_users() {
    let scratch = Scratch::new("turboreld-users");
    let dir = scratch.path("db");
    let add = |name: &str, owner: bool| UserCommand::Add { name: name.into(), owner };
    assert_eq!(user(&dir, &add("ann", true), "ann's\n").unwrap(), "user ANN added, the owner\n");
    assert_eq!(user(&dir, &add("bob", false), "bob's\r\n").unwrap(), "user BOB added\n");
    assert_eq!(user(&dir, &UserCommand::List, "").unwrap(), "ANN (owner)\nBOB\n");
    assert_eq!(user(&dir, &UserCommand::Password { name: "bob".into() }, "new").unwrap(), "user BOB has a new password\n");
    assert_eq!(user(&dir, &UserCommand::Owner { name: "bob".into() }, "").unwrap(), "user BOB owns the database\n");
    assert!(user(&dir, &UserCommand::Drop { name: "bob".into() }, "").unwrap_err().contains("owns the database"));
    assert_eq!(user(&dir, &UserCommand::Drop { name: "ann".into() }, "").unwrap(), "user ANN dropped\n");
    assert_eq!(user(&dir, &UserCommand::List, "").unwrap(), "BOB (owner)\n");
    assert!(user(&dir, &add("bob", false), "x").unwrap_err().contains("exists already"));
    assert!(user(&dir, &UserCommand::Password { name: "carol".into() }, "x").unwrap_err().contains("no user CAROL"));
    assert!(user(&dir, &add("carol dave", false), "x").unwrap_err().contains("not an <authorization identifier>"));
    assert!(user(&dir, &add("carol", false), "\n").unwrap_err().contains("not empty"));
    assert!(user(&dir, &add("carol", false), "").unwrap_err().contains("not empty"));
    let memory = config("--memory user list");
    let refused = turboreld::run_user(&memory, &UserCommand::List, (&mut Cursor::new(&b""[..]), &mut Vec::new())).unwrap_err();
    assert!(refused.contains("--dir"), "{refused}");
}

/// Verifies: [DESIGN] RULES.md D43: a database whose users an operator
/// added is served: with SERVER.TRUST OFF, a client proves itself by its
/// password over a Unix socket, and trust is not served (SQLSTATE 28000).
/// User commands are refused on a database a server holds.
#[cfg(unix)]
#[test]
fn users_an_operator_added_connect_by_password() {
    let scratch = Scratch::new("turboreld-served");
    let dir = scratch.path("db");
    let socket = scratch.path("s");
    user(&dir, &UserCommand::Add { name: "ann".into(), owner: true }, "ann's").unwrap();
    let line = format!("--dir {} --set SERVER.TRUST=OFF --set SERVER.HOST= --set SERVER.SOCKET={}", dir.display(), socket.display());
    let served = config(&line);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (tell, told) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || turboreld::run(&served, |_| tell.send(()).unwrap(), async { drop(stopped.await) }));
    told.recv().unwrap();
    let c = connection::connect_local(&socket, "ann", &mut Password("ann's".into())).unwrap();
    assert_eq!(c.authorization(), "ANN");
    c.terminate().unwrap();
    assert_eq!(state(&connection::connect_local(&socket, "ann", &mut Trust).err().unwrap()).0, "28000", "no trust");
    assert!(user(&dir, &UserCommand::List, "").unwrap_err().contains("locked"), "held by the server");
    stop.send(()).unwrap();
    thread.join().unwrap().unwrap();
}

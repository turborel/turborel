//! \[DESIGN\] `turborel admin COMMAND` (RULES.md D218): a server's
//! administration from the command line, each command one or two
//! statements the server already runs, over a session the connection
//! options open as any other. What they show is the Information Schema's
//! (`SERVER`, `CONNECTIONS`, `SETTINGS`); what they change, the operator
//! statements END CONNECTION, CANCEL CONNECTION and SET DATABASE, and
//! CREATE SCHEMA, DROP SCHEMA and ALTER USER. Dropping a schema asks
//! first, unless `--yes` is given; without a terminal to ask at, it is
//! refused.
//!
//! `ping` runs a statement that reads nothing; a message of the protocol
//! that does so without a statement, once it has one, takes its place in
//! `ping`. `tls` names new files for the server to take for the next
//! connection; files named again, changed in place, are taken when
//! `turboreld` is sent SIGHUP.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use turborel_client::connection::Sink as _;

use crate::args::Options;
use crate::format::{Expanded, Layout, Printer};
use crate::run::{Kept, Statements};
use crate::vars::{identifier, literal};

/// The usage text of `turborel admin`.
pub const ADMIN_USAGE: &str = "usage: turborel admin [connection options] COMMAND

  ping                   whether the server answers, and how soon
  status                 the server's state: its workers, statements waiting and
                         connections
  processlist            the connections the server holds
  kill ID                end the connection ID
  cancel ID              cancel the statement the connection ID runs
  variables [PATTERN]    the settings, or those whose names match PATTERN
                         (* any text, ? one character)
  password [USER]        set a user's password, typed twice; its verifier is
                         made here and sent, never the password
  create-schema NAME     create a schema
  drop-schema NAME [--cascade] [--yes]
                         drop a schema, asked first unless --yes; with --cascade,
                         with all it holds
  version                the server's product and release
  tls CERTIFICATE KEY    the PEM files of the server's certificate chain and key,
                         taken for the connections after

The connection options are turborel's: turborel --help tells them. The
program exits with 0 if the command succeeded, 1 if it failed, and 2 if the
connection was lost.";

/// The statement `ping` runs.
const PING: &str = "VALUES 1";

/// An administrative command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admin {
    /// `ping`.
    Ping,
    /// `status`.
    Status,
    /// `processlist`.
    Processes,
    /// `kill ID`.
    Kill(u64),
    /// `cancel ID`.
    Cancel(u64),
    /// `variables [PATTERN]`.
    Variables(Option<String>),
    /// `password [USER]`.
    Password(Option<String>),
    /// `create-schema NAME`.
    CreateSchema(String),
    /// `drop-schema NAME`, with all it holds if `cascade`, unasked if
    /// `yes`.
    DropSchema {
        /// The schema.
        name: String,
        /// Whether all it holds goes with it.
        cascade: bool,
        /// Whether it is dropped unasked.
        yes: bool,
    },
    /// `version`.
    Version,
    /// `tls CERTIFICATE KEY`.
    Tls(String, String),
}

/// A command, and the session it runs in.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// The session's options.
    pub options: Options,
    /// The command.
    pub command: Admin,
}

/// The connection `text` names.
fn id(text: Option<&String>) -> Result<u64, String> {
    let text = text.ok_or("the connection's ID is needed: turborel admin processlist lists them")?;
    text.parse().map_err(|_| format!("{text} is no connection's ID"))
}

impl Invocation {
    /// The command `words` name, in the session `options` open.
    ///
    /// # Errors
    ///
    /// The words name no command, or not as it is written.
    pub fn new(options: Options, words: Vec<String>) -> Result<Invocation, String> {
        let (flags, args): (Vec<&String>, Vec<&String>) = words.iter().skip(1).partition(|w| w.starts_with("--"));
        let flag = |name: &str| flags.iter().any(|f| *f == name);
        let first = args.first().copied();
        let command = match words.first().map_or("", String::as_str) {
            "ping" => Admin::Ping,
            "status" => Admin::Status,
            "processlist" => Admin::Processes,
            "kill" => Admin::Kill(id(first)?),
            "cancel" => Admin::Cancel(id(first)?),
            "variables" => Admin::Variables(first.cloned()),
            "password" => Admin::Password(first.cloned()),
            "create-schema" => Admin::CreateSchema(first.cloned().ok_or("create-schema needs the schema's name")?),
            "drop-schema" => {
                let name = first.cloned().ok_or("drop-schema needs the schema's name")?;
                Admin::DropSchema { name, cascade: flag("--cascade"), yes: flag("--yes") }
            }
            "version" => Admin::Version,
            "tls" => match args.as_slice() {
                [certificate, key] => Admin::Tls((*certificate).clone(), (*key).clone()),
                _ => return Err("tls needs the files of the certificate chain and of the key".to_owned()),
            },
            other => return Err(format!("{other} is no command of turborel admin\n\n{ADMIN_USAGE}")),
        };
        let known = ["--cascade", "--yes"];
        if let Some(unknown) = flags.iter().find(|f| !known.contains(&f.as_str())) {
            return Err(format!("{unknown} is no option of turborel admin {}", words.first().map_or("", String::as_str)));
        }
        Ok(Invocation { options, command })
    }
}

/// What a command reads and writes.
pub struct Console<'a> {
    /// Where results go.
    pub out: &'a mut dyn Write,
    /// What asks a question, typed `y` or not; none without a terminal.
    pub confirm: Option<&'a mut dyn FnMut(&str) -> io::Result<bool>>,
    /// What reads a password typed; none without a terminal.
    pub secret: Option<&'a mut dyn FnMut(&str) -> io::Result<String>>,
}

/// The pattern `pattern` of a setting's name, as LIKE takes it.
fn setting_pattern(pattern: &str) -> String {
    pattern.to_uppercase().replace('%', "\\%").replace('_', "\\_").replace('*', "%").replace('?', "_")
}

/// Runs `command` over `connection`, its results to `console`.
///
/// # Errors
///
/// Why it failed, for the user.
pub fn run<S: Statements + ?Sized>(connection: &mut S, invocation: &Invocation, console: &mut Console<'_>) -> Result<(), String> {
    connection.set_autocommit(true);
    match &invocation.command {
        Admin::Ping => ping(connection, console),
        Admin::Status => show(connection, "SELECT * FROM INFORMATION_SCHEMA.SERVER", true, console),
        Admin::Processes => show(connection, "SELECT ID, USER_NAME, PEER, SECURE, STATE FROM INFORMATION_SCHEMA.CONNECTIONS ORDER BY ID", false, console),
        Admin::Kill(id) => done(connection, &format!("END CONNECTION {id}"), &format!("Connection {id} is ended."), console),
        Admin::Cancel(id) => done(connection, &format!("CANCEL CONNECTION {id}"), &format!("The statement of connection {id} is cancelled."), console),
        Admin::Variables(pattern) => {
            let filter = pattern.as_deref().map_or_else(String::new, |p| format!(" WHERE SETTING_NAME LIKE {} ESCAPE '\\'", literal(&setting_pattern(p))));
            show(connection, &format!("SELECT SETTING_NAME, SETTING_VALUE FROM INFORMATION_SCHEMA.SETTINGS{filter} ORDER BY SETTING_NAME"), false, console)
        }
        Admin::Password(user) => password(connection, user.as_deref(), invocation, console),
        Admin::CreateSchema(name) => done(connection, &format!("CREATE SCHEMA {}", identifier(name)), &format!("Schema {name} is created."), console),
        Admin::DropSchema { name, cascade, yes } => drop_schema(connection, (name, *cascade, *yes), console),
        Admin::Version => {
            let (server, _) = connection.session().unwrap_or_default();
            writeln!(console.out, "{server}").map_err(|e| e.to_string())
        }
        Admin::Tls(certificate, key) => {
            done(connection, &format!("SET DATABASE SERVER.TLS_CERTIFICATE = {}", literal(certificate)), "", console)?;
            done(connection, &format!("SET DATABASE SERVER.TLS_KEY = {}", literal(key)), "The files are taken for the connections after.", console)
        }
    }
}

/// `ping`: the statement run, and how soon it was answered.
fn ping<S: Statements + ?Sized>(connection: &mut S, console: &mut Console<'_>) -> Result<(), String> {
    let began = Instant::now();
    connection.query_into(PING, &mut Kept::default()).map_err(|f| f.to_string())?;
    let took: Duration = began.elapsed();
    let (server, _) = connection.session().unwrap_or_default();
    writeln!(console.out, "{server} answers, in {} ms.", took.as_millis()).map_err(|e| e.to_string())
}

/// Runs `sql`, its rows written, expanded if `expanded`.
fn show<S: Statements + ?Sized>(connection: &mut S, sql: &str, expanded: bool, console: &mut Console<'_>) -> Result<(), String> {
    let mut kept = Kept::default();
    connection.query_into(sql, &mut kept).map_err(|f| f.to_string())?;
    let layout = Layout { expanded: if expanded { Expanded::On } else { Expanded::Off }, ..Layout::default() };
    let mut printer = Printer::new(&mut *console.out, &layout, None, false);
    printer.columns(&kept.columns);
    printer.rows(&kept.rows);
    printer.finish(u64::try_from(kept.rows.len()).unwrap_or(u64::MAX), None).map_err(|e| e.to_string())
}

/// Runs `sql`, then says `said`, if anything.
fn done<S: Statements + ?Sized>(connection: &mut S, sql: &str, said: &str, console: &mut Console<'_>) -> Result<(), String> {
    connection.query_into(sql, &mut Kept::default()).map_err(|f| f.to_string())?;
    if said.is_empty() {
        return Ok(());
    }
    writeln!(console.out, "{said}").map_err(|e| e.to_string())
}

/// `drop-schema`: asked first unless `yes`, then dropped.
fn drop_schema<S: Statements + ?Sized>(connection: &mut S, (name, cascade, yes): (&str, bool, bool), console: &mut Console<'_>) -> Result<(), String> {
    let how = if cascade { "CASCADE" } else { "RESTRICT" };
    if !yes {
        let Some(confirm) = console.confirm.as_deref_mut() else {
            return Err(format!("drop-schema asks before it drops {name}; give --yes to drop it unasked"));
        };
        let all = if cascade { " and all it holds" } else { "" };
        if !confirm(&format!("Drop the schema {name}{all}? Type y to drop it: ")).map_err(|e| e.to_string())? {
            return Err(format!("{name} is not dropped"));
        }
    }
    done(connection, &format!("DROP SCHEMA {} {how}", identifier(name)), &format!("Schema {name} is dropped."), console)
}

/// `password`: the new password typed twice, its verifier sent.
fn password<S: Statements + ?Sized>(connection: &mut S, user: Option<&str>, invocation: &Invocation, console: &mut Console<'_>) -> Result<(), String> {
    let who = user.map(str::to_owned).or_else(|| connection.session().map(|(_, u)| u)).unwrap_or_default();
    let secret = console.secret.as_deref_mut().ok_or("a password is typed at a terminal")?;
    let first = secret(&format!("new password for {who}: ")).map_err(|e| e.to_string())?;
    let again = secret("again: ").map_err(|e| e.to_string())?;
    if first != again {
        return Err("the two do not match; the password is not changed".to_owned());
    }
    let secure = crate::run::secured(&invocation.options.server);
    crate::run::set_password(connection, &who, &first, secure)?;
    writeln!(console.out, "The password of {who} is changed.").map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation(words: &str) -> Result<Admin, String> {
        Invocation::new(Options::default(), words.split(' ').map(str::to_owned).collect()).map(|i| i.command)
    }

    /// Verifies: [DESIGN] RULES.md D218: each command is read with its
    /// arguments and flags; one missing, wrong or unknown is refused.
    #[test]
    fn commands_are_read() {
        assert_eq!(invocation("ping"), Ok(Admin::Ping));
        assert_eq!(invocation("kill 7"), Ok(Admin::Kill(7)));
        assert_eq!(invocation("cancel 8"), Ok(Admin::Cancel(8)));
        assert!(invocation("kill").is_err());
        assert!(invocation("kill x").is_err());
        assert_eq!(invocation("variables SERVER.*"), Ok(Admin::Variables(Some("SERVER.*".into()))));
        assert_eq!(invocation("drop-schema S --yes"), Ok(Admin::DropSchema { name: "S".into(), cascade: false, yes: true }));
        assert_eq!(invocation("drop-schema S --cascade"), Ok(Admin::DropSchema { name: "S".into(), cascade: true, yes: false }));
        assert!(invocation("drop-schema S --force").is_err());
        assert!(invocation("drop-schema").is_err());
        assert_eq!(invocation("tls c.pem k.pem"), Ok(Admin::Tls("c.pem".into(), "k.pem".into())));
        assert!(invocation("tls c.pem").is_err());
        assert!(invocation("frobnicate").is_err());
        assert_eq!(invocation("create-schema S"), Ok(Admin::CreateSchema("S".into())));
        assert_eq!(invocation("password"), Ok(Admin::Password(None)));
        assert_eq!(invocation("version"), Ok(Admin::Version));
        assert_eq!(invocation("status"), Ok(Admin::Status));
        assert_eq!(invocation("processlist"), Ok(Admin::Processes));
    }

    /// Verifies: [DESIGN] RULES.md D218: a setting's pattern is upper case,
    /// `*` and `?` its wild cards, `%` and `_` themselves.
    #[test]
    fn setting_patterns_are_like_patterns() {
        assert_eq!(setting_pattern("server.tls_*"), "SERVER.TLS\\_%");
        assert_eq!(setting_pattern("a?%"), "A_\\%");
    }
}

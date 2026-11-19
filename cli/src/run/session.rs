//! \[DESIGN\] The session's own commands (RULES.md D214, D217, D219), the
//! prompt, `\conninfo`, `\c`, `\password`, the describe family and `\h`.

use std::time::Duration;

use turborel::storage::Row;
use turborel::value::Value;
use turborel_client::connection::Sink as _;

use super::statement::Kept;
use super::{Next, Shell, Statements};
use crate::args::Server;
use crate::describe::{self, Plan, Section};
use crate::format::{Printer, cell};
use crate::prompt::{self, Context, Status};
use crate::split::Splitter;
use crate::vars::{self, identifier, literal};
use crate::{help, password};

/// The query of the catalog's name [ISO/IEC 9075-11:2006 draft §5.2].
const CATALOG_QUERY: &str = "SELECT CATALOG_NAME FROM INFORMATION_SCHEMA.INFORMATION_SCHEMA_CATALOG_NAME";
/// The query of whether the session's user is the database's owner.
const OWNER_QUERY: &str = "SELECT IS_OWNER FROM INFORMATION_SCHEMA.USERS WHERE USER_NAME = CURRENT_USER";
/// The session statuses of the catalog, whether the user is the owner,
/// the user and the default schema (RULES.md D190).
const CATALOG: &str = "CATALOG";
const OWNER: &str = "OWNER";
const AUTHORIZATION: &str = "AUTHORIZATION";
const SCHEMA: &str = "SCHEMA";
/// What the names of connections `\c` makes begin with.
const CONNECTION_NAME: &str = "TURBOREL_";
/// The SQLCODE of a syntax error, as a server that does not know a
/// statement's form refuses it.
const SYNTAX_ERROR: i32 = -102;

/// The address of `server`, as CONNECT names it (`dial.rs`).
fn address(server: &Server) -> String {
    match server {
        Server::Tcp { host, port, .. } => {
            if host.contains(':') {
                format!("[{host}]:{port}")
            } else {
                format!("{host}:{port}")
            }
        }
        #[cfg(any(unix, windows))]
        Server::Socket(path) => path.display().to_string(),
    }
}

/// Whether a password sent to `server` is secured, as it is over TLS or a
/// local socket (RULES.md D43).
pub(crate) fn secured(server: &Server) -> bool {
    match server {
        Server::Tcp { tls, .. } => tls.is_some(),
        #[cfg(any(unix, windows))]
        Server::Socket(_) => true,
    }
}

impl<S: Statements + ?Sized, C: FnMut() -> Duration> Shell<'_, S, C> {
    /// Runs `sql` unseen, no transaction begun by it, and returns its
    /// rows, or none if it failed.
    pub(super) fn probe(&mut self, sql: &str) -> Option<Vec<Row>> {
        let alone = !self.connection.in_transaction();
        if alone {
            self.connection.set_autocommit(true);
        }
        let mut kept = Kept::default();
        let ran = self.connection.query_into(sql, &mut kept);
        if alone {
            self.connection.set_autocommit(self.vars.on(vars::AUTOCOMMIT));
        }
        ran.ok().map(|_| kept.rows)
    }

    /// The prompt's status as the server's session status last told it
    /// (RULES.md D190), with no statement; none of a server that tells
    /// none, whose status is probed.
    fn told_status(&self) -> Option<Status> {
        let catalog = self.connection.status(CATALOG)?;
        let owner = self.connection.status(OWNER)? == "YES";
        Some(Status { catalog, owner })
    }

    /// The prompt for the line after the text `splitter` holds.
    pub(super) fn prompt(&mut self, splitter: &Splitter) -> String {
        let pending = splitter.pending();
        let template = self.vars.get(if pending { vars::PROMPT_MORE } else { vars::PROMPT }).unwrap_or_default().to_owned();
        if let Some(told) = self.told_status() {
            self.status = Some(told);
        } else if prompt::needs_status(&template) && self.status.is_none() {
            let catalog = self.probe(CATALOG_QUERY).and_then(|rows| first_text(&rows)).unwrap_or_default();
            let owner = self.probe(OWNER_QUERY).and_then(|rows| first_text(&rows)).is_some_and(|o| o == "YES");
            self.status = Some(Status { catalog, owner });
        }
        let (server, login) = self.connection.session().unwrap_or_default();
        let user = self.connection.status(AUTHORIZATION).unwrap_or(login);
        let told_schema = self.connection.status(SCHEMA);
        let schema = told_schema.or_else(|| self.schema.clone()).unwrap_or_else(|| user.clone());
        let active = self.connection.in_transaction();
        let context = Context { user: &user, server: &server, schema: &schema, status: self.status.as_ref(), active, pending };
        prompt::expand(&template, &context)
    }

    /// Keeps the default schema a SET SCHEMA statement `sql` set.
    pub(super) fn track_schema(&mut self, sql: &str) {
        let words: Vec<String> = crate::words::words(sql).into_iter().map(|(_, w)| w).collect();
        if words.first().map(String::as_str) != Some("SET") || words.get(1).map(String::as_str) != Some("SCHEMA") {
            return;
        }
        let user = self.connection.session().map(|(_, u)| u).unwrap_or_default();
        let value = sql.split_once('\'').and_then(|(_, rest)| literal_body(rest));
        self.schema = Some(value.map_or_else(|| user.clone(), |v| prompt::schema_of(&v, &user)));
    }

    /// `\conninfo` shows the server, the user, where, and the connection.
    pub(super) fn conninfo(&mut self) -> Next {
        let Some((server, user)) = self.connection.session() else { return self.tell("\\conninfo: there is no connection", Next::Go) };
        let by = self.connection.connection_name().map_or_else(
            || format!("at {}{}", address(&self.options.server), if secured(&self.options.server) { ", secured" } else { "" }),
            |name| format!("by the connection {name}"),
        );
        let active = if self.connection.in_transaction() { ", a transaction active" } else { "" };
        self.say(&format!("Connected to {server} as {user}, {by}{active}."))
    }

    /// `\c` makes a new connection to `server`, else the default server,
    /// as `user`, else the command line's; the connection before is ended
    /// once it is made.
    pub(super) fn reconnect(&mut self, server: Option<&str>, user: Option<&str>) -> Next {
        let before = self.connection.connection_name();
        self.connections = self.connections.saturating_add(1);
        let name = format!("{CONNECTION_NAME}{}", self.connections);
        let to = server.map_or_else(|| address(&self.options.server), str::to_owned);
        let as_user = user.map_or_else(String::new, |u| format!(" USER {}", literal(u)));
        let connect = format!("CONNECT TO {} AS {}{as_user}", literal(&to), literal(&name));
        if let Err(failure) = self.connection.query_into(&connect, &mut Kept::default()) {
            return self.tell(&format!("\\c: {failure}"), Next::Go);
        }
        let disconnect = before.map_or_else(|| "DISCONNECT DEFAULT".to_owned(), |n| format!("DISCONNECT {}", literal(&n)));
        if let Err(failure) = self.connection.query_into(&disconnect, &mut Kept::default()) {
            self.tell(&format!("\\c: the connection before is kept: {failure}"), Next::Go);
        }
        if let Some(interrupts) = self.interrupts {
            interrupts.cancel_by(self.connection.canceller());
        }
        (self.names, self.status, self.schema) = (None, None, None);
        let (server_name, user_name) = self.connection.session().unwrap_or_default();
        self.vars.set(vars::SERVER, &server_name);
        self.vars.set(vars::USER, &user_name);
        self.say(&format!("Connected to {server_name} as {user_name}, by the connection {name}."))
    }

    /// `\password` sets a new password for `user`, else the session's,
    /// typed twice; its verifier is made here and sent.
    pub(super) fn password(&mut self, user: Option<&str>) -> Next {
        let who = user.map(str::to_owned).or_else(|| self.connection.session().map(|(_, u)| u)).unwrap_or_default();
        let Some(secret) = self.terminal.secret.as_deref_mut() else {
            return self.tell("\\password: a password is typed at a terminal", Next::Go);
        };
        let typed = secret(&format!("new password for {who}: ")).and_then(|first| secret("again: ").map(|again| (first, again)));
        let (first, again) = match typed {
            Ok(both) => both,
            Err(e) => return self.tell(&format!("\\password: cannot read it: {e}"), Next::Go),
        };
        if first != again {
            return self.tell("\\password: the two do not match; the password is not changed", Next::Go);
        }
        let secure = secured(&self.options.server);
        match set_password(&mut *self.connection, &who, &first, secure) {
            Ok(()) => self.say(&format!("The password of {who} is changed.")),
            Err(e) => self.tell(&format!("\\password: {e}"), Next::Go),
        }
    }

    /// The describe command `name`, of `pattern`.
    pub(super) fn describe(&mut self, name: &str, pattern: Option<&str>) -> Next {
        let plan = match describe::plan(name, pattern) {
            Ok(plan) => plan,
            Err(e) => return self.tell(&e, Next::Go),
        };
        let sections = match plan {
            Plan::Sections(sections) => sections,
            Plan::Relation(lookup) => {
                let Some(rows) = self.kept(&lookup.sql) else { return Next::Go };
                match describe::resolved(&lookup, &rows.rows) {
                    Ok(sections) => sections,
                    Err(e) => return self.tell(&e, Next::Go),
                }
            }
        };
        for section in sections {
            if self.section(&section) == Next::Stop {
                return Next::Stop;
            }
        }
        Next::Go
    }

    /// Writes `section`, its title and rows, or nothing if it has no rows
    /// and is left out so.
    fn section(&mut self, section: &Section) -> Next {
        let Some(kept) = self.kept(&section.sql) else { return Next::Go };
        if kept.rows.is_empty() && section.skip_empty {
            return Next::Go;
        }
        if let Some(title) = &section.title
            && self.result_line(title).is_err()
        {
            return Next::Stop;
        }
        let width = self.terminal.screen.map(|s| s.width);
        let main: &mut dyn std::io::Write = match &mut self.redirect {
            Some(r) => r.writer(),
            None => &mut *self.out,
        };
        let mut out = super::output::Out { main, tee: self.tee.as_mut(), pager: None };
        let mut printer = Printer::new(&mut out, &self.layout, width, false).fitted(&kept.rows);
        printer.columns(&kept.columns);
        printer.rows(&kept.rows);
        let count = u64::try_from(kept.rows.len()).unwrap_or(u64::MAX);
        match printer.finish(count, None) {
            Ok(()) => Next::Go,
            Err(e) => self.tell(&format!("cannot write: {e}"), Next::Stop),
        }
    }

    /// `\h` shows a statement's syntax, or lists the statements there are.
    pub(super) fn syntax(&mut self, topic: Option<&str>) -> Next {
        match help::help(topic) {
            Ok(text) => self.say(text.trim_end()),
            Err(e) => self.tell(&e, Next::Go),
        }
    }
}

/// The text of the first column of the first of `rows`, if it is not
/// null.
fn first_text(rows: &[Row]) -> Option<String> {
    rows.first()?.first().filter(|v| !matches!(v, Value::Null)).map(|v| cell(v, ""))
}

/// The body of a literal whose text after its opening quote is `rest`,
/// its doubled quotes one; none if it is not closed.
fn literal_body(rest: &str) -> Option<String> {
    let mut body = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\'' {
            body.push(c);
            continue;
        }
        if chars.peek() != Some(&'\'') {
            return Some(body);
        }
        body.push('\'');
        chars.next();
    }
    None
}

/// \[DESIGN\] RULES.md D43, D217: sets the password of `user` to
/// `password` over `connection`, by the verifier the client makes, so the
/// password is never sent. A server that does not take a verifier
/// (`ALTER USER ... PASSWORD VERIFIER`) refuses its syntax; then, over a
/// connection `secure`, by TLS or a local socket, the password is sent as
/// ALTER USER takes it, and over plain TCP it is not sent at all.
///
/// # Errors
///
/// Why the password is not changed.
pub(crate) fn set_password<S: Statements + ?Sized>(connection: &mut S, user: &str, password: &str, secure: bool) -> Result<(), String> {
    let made = password::verifier(password, &password::salt(), password::ITERATIONS);
    let by_verifier = format!("ALTER USER {} PASSWORD VERIFIER {}", identifier(user), literal(&made));
    let Err(failure) = connection.query_into(&by_verifier, &mut Kept::default()) else { return Ok(()) };
    if failure.sqlcode != SYNTAX_ERROR {
        return Err(failure.to_string());
    }
    if !secure {
        return Err("the server takes no verifier, and a password is sent only over TLS or a local socket: it is not changed".to_owned());
    }
    let by_password = format!("ALTER USER {} PASSWORD {}", identifier(user), literal(password));
    connection.query_into(&by_password, &mut Kept::default()).map(drop).map_err(|f| f.to_string())
}

//! \[DESIGN\] `turboreld --dir PATH user ...` (RULES.md D43): users managed
//! on the database's directory while no server holds it, as an operator
//! sets a database up, its first user and owner among them. A running
//! server's owner manages users in SQL instead.
//!
//! A password is typed without its echo at a terminal, or else read from
//! standard input (`main.rs`); never taken from the command line, where
//! other users of the machine could read it in the list of processes.
//!
//! A database kept nowhere, `--memory`, has no directory to set up
//! before it is served: `--owner NAME` makes its first user, and so its
//! owner, as the server starts, the password read the same way. Without
//! it, such a database has no owner, so no one could manage its users or
//! set its settings in SQL.

use std::io::{BufRead, Write};

use turborel::db::Database;
use turborel::lexer::authorization_identifier;
use turborel_server::password::Pbkdf2;

use crate::{Config, Store, within};

/// \[DESIGN\] The owner `--owner` makes of a database kept nowhere as the
/// server starts: its name, as written, and its password, once read.
#[derive(Clone, PartialEq, Eq)]
pub struct Owner {
    /// The user, as written.
    pub name: String,
    /// Its password: read before the server starts (`read_owner_password`).
    pub password: Option<String>,
}

impl std::fmt::Debug for Owner {
    /// Never the password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Owner({})", self.name)
    }
}

/// The owner's password of `config`, if it names one, read from `input`:
/// its first line.
///
/// # Errors
///
/// What is wrong, for the operator: the password cannot be read, or is
/// empty.
pub fn read_owner_password(config: &mut Config, input: &mut dyn BufRead) -> Result<(), String> {
    if let Some(owner) = &mut config.owner {
        owner.password = Some(password(input)?);
    }
    Ok(())
}

/// Makes `owner` a user of `db`, of its password, and the database's
/// owner.
///
/// # Errors
///
/// What failed, for the operator: the password not read, the name not an
/// authorization identifier, a verifier not made.
pub(crate) fn make_owner(db: &Database, owner: &Owner) -> Result<(), String> {
    let failed = |e: turborel::error::SqlError| e.to_string();
    let typed = owner.password.as_deref().ok_or_else(|| "--owner: the owner's password is read before the server starts".to_owned())?;
    let name = user(&owner.name)?;
    let verifier = Pbkdf2::of(db).make(typed).map_err(failed)?;
    db.create_user(&name, verifier.as_bytes()).map_err(failed)?;
    db.set_owner(&name).map_err(failed)
}

/// What a user command does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserCommand {
    /// `add NAME [--owner]`: a user, its password read, made the owner if
    /// asked.
    Add {
        /// The user, as written.
        name: String,
        /// Whether it owns the database.
        owner: bool,
    },
    /// `password NAME`: a user's new password, read.
    Password {
        /// The user, as written.
        name: String,
    },
    /// `drop NAME`.
    Drop {
        /// The user, as written.
        name: String,
    },
    /// `owner NAME`: the user made the database's owner.
    Owner {
        /// The user, as written.
        name: String,
    },
    /// `list`: every user, the owner marked.
    List,
}

/// The user command `words`, after `user`.
///
/// # Errors
///
/// What is wrong with it, for the operator.
pub fn parse_user(words: &[String]) -> Result<UserCommand, String> {
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let named = |name: &str| name.to_owned();
    match words.as_slice() {
        ["add", name] => Ok(UserCommand::Add { name: named(name), owner: false }),
        ["add", name, "--owner"] => Ok(UserCommand::Add { name: named(name), owner: true }),
        ["password", name] => Ok(UserCommand::Password { name: named(name) }),
        ["drop", name] => Ok(UserCommand::Drop { name: named(name) }),
        ["owner", name] => Ok(UserCommand::Owner { name: named(name) }),
        ["list"] => Ok(UserCommand::List),
        _ => Err(format!("user takes add NAME [--owner], password NAME, drop NAME, owner NAME or list, not {}", words.join(" "))),
    }
}

/// A password, the first line of `input`, its line end dropped.
fn password(input: &mut dyn BufRead) -> Result<String, String> {
    let mut line = String::new();
    input.read_line(&mut line).map_err(|e| format!("cannot read a password: {e}"))?;
    let password = line.strip_suffix('\n').unwrap_or(&line);
    let password = password.strip_suffix('\r').unwrap_or(password);
    if password.is_empty() {
        return Err("a password is not empty: give it on standard input".into());
    }
    Ok(password.to_owned())
}

/// `name`, an `<authorization identifier>`, as the database keeps it.
fn user(name: &str) -> Result<String, String> {
    authorization_identifier(name).map_err(|e| e.msg)
}

/// Does `command` to `db`, a password read from `input`; what it did
/// written to `out`.
fn act(db: &Database, command: &UserCommand, (input, out): (&mut dyn BufRead, &mut dyn Write)) -> Result<(), String> {
    let failed = |e: turborel::error::SqlError| e.to_string();
    let said = match command {
        UserCommand::Add { name, owner } => {
            let name = user(name)?;
            let verifier = Pbkdf2::of(db).make(&password(input)?).map_err(failed)?;
            db.create_user(&name, verifier.as_bytes()).map_err(failed)?;
            if *owner {
                db.set_owner(&name).map_err(failed)?;
            }
            format!("user {name} added{}", if *owner { ", the owner" } else { "" })
        }
        UserCommand::Password { name } => {
            let name = user(name)?;
            let verifier = Pbkdf2::of(db).make(&password(input)?).map_err(failed)?;
            db.set_verifier(&name, verifier.as_bytes()).map_err(failed)?;
            format!("user {name} has a new password")
        }
        UserCommand::Drop { name } => {
            let name = user(name)?;
            db.drop_user(&name).map_err(failed)?;
            format!("user {name} dropped")
        }
        UserCommand::Owner { name } => {
            let name = user(name)?;
            db.set_owner(&name).map_err(failed)?;
            format!("user {name} owns the database")
        }
        UserCommand::List => {
            let owner = db.owner().map_err(failed)?;
            let users = db.users().map_err(failed)?;
            users.iter().map(|u| if owner.as_ref() == Some(u) { format!("{u} (owner)") } else { u.clone() }).collect::<Vec<_>>().join("\n")
        }
    };
    if !said.is_empty() {
        writeln!(out, "{said}").map_err(|e| format!("cannot write: {e}"))?;
    }
    Ok(())
}

/// Does `command` to the database `config` names, kept in a directory, a
/// password read from `input`, what it did written to `out`; the database
/// closed after.
///
/// # Errors
///
/// What failed, for the operator: the database not kept in a directory,
/// or held by a server; a user not there, or one there already; a name not
/// an authorization identifier; a password not given.
pub fn run_user(config: &Config, command: &UserCommand, (input, out): (&mut dyn BufRead, &mut dyn Write)) -> Result<(), String> {
    if config.store == Store::Memory {
        return Err("user works on a database kept in a directory: name it with --dir".into());
    }
    within(config, None, |server| act(server.database(), command, (input, out)))
}

//! \[DESIGN\] A backslash command's name and arguments read as a
//! [`Command`] (RULES.md D210).

use std::path::PathBuf;
use std::time::Duration;

use super::{Command, Expansion, Target, seconds, target};
use crate::describe;
use crate::vars::truth;

/// The argument of `\x` that expands only results wider than the screen.
pub const AUTO: &str = "auto";
/// The interval `\watch` waits between runs unless one is given.
const WATCH_EVERY: Duration = Duration::from_secs(2);

/// The command `name` names, of `args` and `rest`, the line after the
/// name, as typed.
///
/// # Errors
///
/// No command has the name, or its arguments are wrong.
pub fn parse(name: &str, args: Vec<String>, rest: &str) -> Result<Command, String> {
    if let Some(found) = simple(name, &args)? {
        return Ok(found);
    }
    if describe::is_describe(name) {
        return Ok(Command::Describe(name.to_owned(), args.into_iter().next()));
    }
    match name {
        "set" => Ok(Command::Set(set(args))),
        "unset" => one(name, args).map(Command::Unset),
        "if" => Ok(Command::If(args.join(" "))),
        "elif" => Ok(Command::Elif(args.join(" "))),
        "echo" => Ok(Command::Echo(args.join(" "))),
        "i" | "include" => one(name, args).map(|p| Command::Include { path: PathBuf::from(p), relative: false }),
        "ir" | "include_relative" => one(name, args).map(|p| Command::Include { path: PathBuf::from(p), relative: true }),
        "o" | "out" => Ok(Command::Output(target(&args))),
        "g" => Ok(Command::Go { target: target(&args), expanded: false }),
        "gx" => Ok(Command::Go { target: target(&args), expanded: true }),
        "gset" => Ok(Command::Gset(args.into_iter().next().unwrap_or_default())),
        "watch" => watch(&args),
        "e" | "edit" => Ok(Command::Edit(args.into_iter().next().map(PathBuf::from))),
        "t" => toggle(name, &args).map(Command::Tuples),
        "x" => expansion(&args).map(Command::Expanded),
        "format" => Ok(Command::Format(format(args))),
        "import" => Ok(Command::Import(args)),
        "export" => Ok(Command::Export(export(rest, args))),
        "c" | "connect" => Ok(connect(args)),
        "password" => Ok(Command::Password(args.into_iter().next())),
        "tee" => one(name, args).map(|p| Command::Tee(PathBuf::from(p))),
        "delimiter" => Ok(Command::Delimiter(args.into_iter().next())),
        other => Err(format!("\\{other} is no command: \\? lists them")),
    }
}

/// The commands that take no arguments, and `\h`, `\G`, if `name` names
/// one.
fn simple(name: &str, args: &[String]) -> Result<Option<Command>, String> {
    let found = match name {
        "q" | "quit" => Command::Quit,
        "?" => Command::Commands,
        "h" | "help" => Command::Syntax((!args.is_empty()).then(|| args.join(" "))),
        "else" => Command::Else,
        "endif" => Command::Endif,
        "G" => Command::Go { target: Target::Default, expanded: true },
        "gexec" => Command::Gexec,
        "p" | "print" => Command::Print,
        "r" | "reset" => Command::Reset,
        "a" => Command::Aligned,
        "conninfo" => Command::ConnInfo,
        "notee" => Command::NoTee,
        _ => return Ok(None),
    };
    let takes = matches!(found, Command::Syntax(_));
    if !takes && !args.is_empty() {
        return Err(format!("\\{name} takes no arguments"));
    }
    Ok(Some(found))
}

/// The one argument of the command `name`.
fn one(name: &str, args: Vec<String>) -> Result<String, String> {
    let mut each = args.into_iter();
    match (each.next(), each.next()) {
        (Some(arg), None) => Ok(arg),
        (None, _) | (Some(_), Some(_)) => Err(format!("\\{name} takes one argument")),
    }
}

/// `\set`'s variable and value, its words after the name joined, or none
/// to list them.
fn set(args: Vec<String>) -> Option<(String, String)> {
    let mut each = args.into_iter();
    let name = each.next()?;
    Some((name, each.collect::<Vec<_>>().concat()))
}

/// The truth a toggle's argument sets, none to toggle.
fn toggle(name: &str, args: &[String]) -> Result<Option<bool>, String> {
    match args {
        [] => Ok(None),
        [value] => truth(value).map(Some).ok_or_else(|| format!("\\{name} takes on or off, not {value}")),
        [_, _, ..] => Err(format!("\\{name} takes on or off")),
    }
}

/// How `\x`'s argument sets expanded output.
fn expansion(args: &[String]) -> Result<Expansion, String> {
    if let [value] = args
        && value.eq_ignore_ascii_case(AUTO)
    {
        return Ok(Expansion::Auto);
    }
    toggle("x", args).map(|t| t.map_or(Expansion::Toggle, Expansion::Set))
}

/// `\watch`'s interval and count.
fn watch(args: &[String]) -> Result<Command, String> {
    let every = args.first().map_or(Ok(WATCH_EVERY), |s| seconds(s))?;
    let count = args.get(1).map(|c| c.parse().map_err(|_| format!("{c} is no count"))).transpose()?;
    if args.len() > 2 {
        return Err("\\watch takes at most an interval and a count".to_owned());
    }
    Ok(Command::Watch { every, count })
}

/// `\format`'s option and value, if given.
fn format(args: Vec<String>) -> Option<(String, Option<String>)> {
    let mut each = args.into_iter();
    let option = each.next()?;
    Some((option, each.next()))
}

/// `\export`'s arguments, a query in parentheses kept whole, as typed,
/// then the rest.
fn export(rest: &str, args: Vec<String>) -> Vec<String> {
    let Some(inner) = rest.trim_start().strip_prefix('(') else { return args };
    let mut depth = 1_usize;
    for (at, c) in inner.char_indices() {
        depth = match c {
            '(' => depth.saturating_add(1),
            ')' => depth.saturating_sub(1),
            _ => depth,
        };
        if depth == 0 {
            let query = inner.get(..at).unwrap_or_default().trim().to_owned();
            let after = inner.get(at.saturating_add(1)..).unwrap_or_default();
            return std::iter::once(format!("({query})")).chain(after.split_whitespace().map(str::to_owned)).collect();
        }
    }
    args
}

/// `\c`'s server and user, where `-` for the server is the default one.
fn connect(args: Vec<String>) -> Command {
    let mut each = args.into_iter();
    let server = each.next().filter(|s| s != "-");
    Command::Connect(server, each.next())
}

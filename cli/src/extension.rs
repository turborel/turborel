//! \[EXTENSION\] `turborel extension`: the package manager of extensions
//! (design document §13), run on the machine of a server, writing into
//! its plug-in directory, the first of PLUGINS.DIRECTORIES. A package is a
//! file, which the operator copies there as they like; there is no
//! registry and nothing is fetched (RULES.md D260).
//!
//! * `args.rs`: its command line.
//! * `installed.rs`: what the plug-in directory holds, and its lock.
//! * `files.rs`: the package files given to install.
//! * `commands.rs`: install, remove, autoremove and the lock's export;
//!   `queries.rs`: list, info, license, verify and why.
//! * `starter.rs`: new, a starter project; `author.rs`: pack and keygen.

mod args;
mod author;
mod commands;
mod files;
mod installed;
mod queries;
mod starter;

use std::io::Write;
use std::path::{Path, PathBuf};

use turborel::plugin::limits::{DIRECTORY_SEPARATOR, Directories};
use turborel::plugin::package::Trust;

pub use args::{EXTENSION_USAGE, Invocation, Verb, parse};

/// What every command of `turborel extension` works with.
#[derive(Debug, Clone)]
pub struct Context {
    /// PLUGINS.DIRECTORIES as written. Packages are installed into the
    /// first, and found in all.
    pub directories: Directories,
    /// PLUGINS.TRUSTED_KEYS and PLUGINS.REQUIRE_SIGNED.
    pub trust: Trust,
    /// The directory paths given are taken from: `.` from the command
    /// line, another of a program that runs the commands in place.
    pub cwd: PathBuf,
}

impl Context {
    /// `path` as given, taken from `cwd`.
    pub fn at(&self, path: &Path) -> PathBuf {
        self.cwd.join(path)
    }

    /// PLUGINS.DIRECTORIES, each taken from `cwd`.
    pub fn dirs(&self) -> Directories {
        let taken: Vec<String> = self.directories.paths().iter().map(|d| self.at(d).display().to_string()).collect();
        Directories::parse(&taken.join(&DIRECTORY_SEPARATOR.to_string()))
    }

    /// The plug-in directory packages are installed into.
    ///
    /// # Errors
    ///
    /// If PLUGINS.DIRECTORIES names none.
    pub fn home(&self) -> Result<PathBuf, String> {
        self.dirs().paths().first().cloned().ok_or_else(|| "name the plug-in directory: --set PLUGINS.DIRECTORIES=DIR".to_owned())
    }
}

/// Runs `invocation`, what it reports written to `out`.
///
/// # Errors
///
/// Why it failed, for the user.
pub fn run(invocation: &Invocation, out: &mut dyn Write) -> Result<(), String> {
    let (words, cx) = (&invocation.words, &invocation.context);
    match invocation.verb {
        Verb::Install => commands::install(cx, words, invocation.lock.as_deref(), out),
        Verb::Remove => commands::remove(cx, one(words)?, out),
        Verb::Autoremove => commands::autoremove(cx, out),
        Verb::List => queries::list(cx, out),
        Verb::Info => queries::info(cx, one(words)?, out),
        Verb::License => queries::license(cx, one(words)?, out),
        Verb::Verify => queries::verify(cx, out),
        Verb::Why => queries::why(cx, one(words)?, out),
        Verb::LockExport => commands::lock_export(cx, one(words)?, out),
        Verb::New => starter::new(invocation, one(words)?, out),
        Verb::Pack => author::pack(invocation, at_most_one(words)?, out),
        Verb::Keygen => author::keygen(invocation, out),
    }
}

/// The one word a command takes.
fn one(words: &[String]) -> Result<&str, String> {
    match words {
        [word] => Ok(word),
        _ => Err("the command takes one argument; see turborel extension --help".to_owned()),
    }
}

/// The word a command may take, if given.
fn at_most_one(words: &[String]) -> Result<Option<&str>, String> {
    match words {
        [] => Ok(None),
        [word] => Ok(Some(word)),
        _ => Err("the command takes at most one argument; see turborel extension --help".to_owned()),
    }
}

/// A package's name as typed, in upper case, as names are SQL
/// identifiers, upper case unless quoted in the manifest.
pub fn package_name(typed: &str) -> String {
    typed.to_ascii_uppercase()
}

/// Writes `line` to `out`.
fn say(out: &mut dyn Write, line: &str) -> Result<(), String> {
    writeln!(out, "{line}").map_err(|e| format!("cannot write the report: {e}"))
}

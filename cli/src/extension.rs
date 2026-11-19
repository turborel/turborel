//! \[EXTENSION\] `turborel extension`: the package manager of extensions
//! (design document §13), run on the machine of a server, writing into
//! its plug-in directory, the first of PLUGINS.DIRECTORIES.
//!
//! * `args.rs`: its command line.
//! * `registry.rs`: the registries' signed indexes, fetching packages and
//!   the cache of those fetched.
//! * `installed.rs`: what the plug-in directory holds, and its lock.
//! * `commands.rs`: install, upgrade, remove, autoremove and the lock;
//!   `queries.rs`: list, verify, outdated, why, search and info;
//!   `author.rs`: pack, publish and keygen.

mod args;
mod author;
mod commands;
mod installed;
mod queries;
mod registry;

use std::io::Write;
use std::path::PathBuf;

use turborel::plugin::limits::Directories;
use turborel::plugin::package::Trust;
use turborel::plugin::package::index::Registries;

pub use args::{EXTENSION_USAGE, Invocation, Verb, parse};

/// What every command of `turborel extension` works with.
#[derive(Debug, Clone)]
pub struct Context {
    /// PLUGINS.DIRECTORIES: packages are installed into the first, and
    /// found in all.
    pub directories: Directories,
    /// PLUGINS.REGISTRIES and PLUGINS.PINS.
    pub registries: Registries,
    /// PLUGINS.TRUSTED_KEYS and PLUGINS.REQUIRE_SIGNED.
    pub trust: Trust,
    /// Where packages fetched are kept: by default `.cache` in the
    /// plug-in directory.
    pub cache: Option<PathBuf>,
    /// The PEM file of the authorities a registry's certificate is
    /// checked against; the platform's if none.
    pub roots: Option<PathBuf>,
    /// Whether plain HTTP is allowed: never from the command line, only
    /// by a test of a registry served by its own process.
    pub allow_http: bool,
}

impl Context {
    /// The plug-in directory packages are installed into.
    ///
    /// # Errors
    ///
    /// If PLUGINS.DIRECTORIES names none.
    pub fn home(&self) -> Result<PathBuf, String> {
        self.directories.paths().first().cloned().ok_or_else(|| "name the plug-in directory: --set PLUGINS.DIRECTORIES=DIR".to_owned())
    }

    /// The directory of the cache.
    ///
    /// # Errors
    ///
    /// As `home`'s.
    pub fn cache_dir(&self) -> Result<PathBuf, String> {
        match &self.cache {
            Some(dir) => Ok(dir.clone()),
            None => Ok(self.home()?.join(".cache")),
        }
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
        Verb::Search => queries::search(cx, words, out),
        Verb::Info => queries::info(cx, one(words)?, out),
        Verb::Install => commands::install(cx, words, out),
        Verb::Upgrade => commands::upgrade(cx, words, invocation.latest, out),
        Verb::Remove => commands::remove(cx, one(words)?, out),
        Verb::Autoremove => commands::autoremove(cx, out),
        Verb::List => queries::list(cx, out),
        Verb::Verify => queries::verify(cx, out),
        Verb::Outdated => queries::outdated(cx, out),
        Verb::Why => queries::why(cx, one(words)?, out),
        Verb::Clean => commands::clean(cx, out),
        Verb::LockExport => commands::lock_export(cx, one(words)?, out),
        Verb::LockImport => commands::lock_import(cx, one(words)?, out),
        Verb::Pack => author::pack(invocation, one(words)?, out),
        Verb::Publish => author::publish(invocation, one(words)?, out),
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

/// A package's name as typed: names are SQL identifiers, upper case
/// unless quoted in the manifest.
pub fn package_name(typed: &str) -> String {
    typed.to_ascii_uppercase()
}

/// Writes `line` to `out`.
fn say(out: &mut dyn Write, line: &str) -> Result<(), String> {
    writeln!(out, "{line}").map_err(|e| format!("cannot write the report: {e}"))
}

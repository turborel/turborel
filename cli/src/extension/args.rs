//! \[EXTENSION\] The command line of `turborel extension` (design document
//! §13.5): a verb, its words, and options. The settings it reads are the
//! server's, named by `--set NAME=VALUE` or by the environment variable
//! of the name, `.` written `_` and `TURBOREL_` before it.

use std::path::PathBuf;

use turborel::plugin::limits::Directories;
use turborel::plugin::package::Trust;
use turborel::plugin::package::index::Registries;

use super::Context;

/// The usage text of `turborel extension`.
pub const EXTENSION_USAGE: &str = "usage: turborel extension COMMAND [options]

  search WORDS...        the packages of the registries whose names hold a word
  info NAME              a package's versions, what each requires, and its signer
  install [NAME[@RANGE] | FILE.tbpkg]...
                         install packages and what they require; with none, install
                         exactly what the lock names
  upgrade [NAME]...      upgrade to the newest versions their ranges allow;
                         --latest goes beyond the ranges
  remove NAME            remove a package nothing installed requires
  autoremove             remove packages installed only as others' dependencies that
                         nothing requires now, and versions the lock does not name
  list                   the packages installed and locked
  verify                 check every file installed against its package's digests
                         and signature
  outdated               the packages with newer versions, in their range and beyond
  why NAME               which packages required NAME
  clean                  empty the cache of packages fetched
  lock export FILE       write the lock to FILE
  lock import FILE       install exactly what the lock FILE names, and keep it
  pack DIR               build a package of the plug-in in DIR, signed with --key
  publish DIR            pack DIR, signed with --key, into the registry directory
                         --registry-dir, and sign its index again
  keygen                 write a new signing key to --output and its public key to
                         --output with .pub after

options:
  --set NAME=VALUE       a setting: PLUGINS.DIRECTORIES (the first is where packages
                         are installed), PLUGINS.REGISTRIES, PLUGINS.PINS,
                         PLUGINS.TRUSTED_KEYS, PLUGINS.REQUIRE_SIGNED (ON or OFF)
  --cache DIR            where packages fetched are kept (default DIRECTORY/.cache)
  --tls FILE             trust the registries' certificates of the PEM file FILE,
                         instead of the platform's authorities
  --latest               upgrade beyond the ranges asked for
  --key FILE             the signing key of pack and publish
  --output FILE          where pack and keygen write
  --registry-dir DIR     the registry directory publish writes into
  --expires-days DAYS    how long the index publish signs is trusted (default 30)

The environment variable TURBOREL_PLUGINS_DIRECTORIES gives PLUGINS.DIRECTORIES,
and so on for each setting.";

/// The settings `turborel extension` reads.
const SETTINGS: [&str; 5] = ["PLUGINS.DIRECTORIES", "PLUGINS.REGISTRIES", "PLUGINS.PINS", "PLUGINS.TRUSTED_KEYS", "PLUGINS.REQUIRE_SIGNED"];
/// The default of `--expires-days`.
pub const DEFAULT_EXPIRES_DAYS: u64 = 30;

/// A command of `turborel extension`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// search.
    Search,
    /// info.
    Info,
    /// install.
    Install,
    /// upgrade.
    Upgrade,
    /// remove.
    Remove,
    /// autoremove.
    Autoremove,
    /// list.
    List,
    /// verify.
    Verify,
    /// outdated.
    Outdated,
    /// why.
    Why,
    /// clean.
    Clean,
    /// lock export.
    LockExport,
    /// lock import.
    LockImport,
    /// pack.
    Pack,
    /// publish.
    Publish,
    /// keygen.
    Keygen,
}

/// What `turborel extension` is asked.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// The command.
    pub verb: Verb,
    /// Its words.
    pub words: Vec<String>,
    /// What it works with.
    pub context: Context,
    /// `--latest`.
    pub latest: bool,
    /// `--key`.
    pub key: Option<PathBuf>,
    /// `--output`.
    pub output: Option<PathBuf>,
    /// `--registry-dir`.
    pub registry_dir: Option<PathBuf>,
    /// `--expires-days`.
    pub expires_days: u64,
}

/// The command `word` names, and the second word it takes, if any.
fn verb(word: &str, rest: &mut std::slice::Iter<'_, String>) -> Result<Verb, String> {
    let verbs = [
        ("search", Verb::Search),
        ("info", Verb::Info),
        ("install", Verb::Install),
        ("upgrade", Verb::Upgrade),
        ("remove", Verb::Remove),
        ("autoremove", Verb::Autoremove),
        ("list", Verb::List),
        ("verify", Verb::Verify),
        ("outdated", Verb::Outdated),
        ("why", Verb::Why),
        ("clean", Verb::Clean),
        ("pack", Verb::Pack),
        ("publish", Verb::Publish),
        ("keygen", Verb::Keygen),
    ];
    if word == "lock" {
        return match rest.next().map(String::as_str) {
            Some("export") => Ok(Verb::LockExport),
            Some("import") => Ok(Verb::LockImport),
            _ => Err("lock takes export or import".to_owned()),
        };
    }
    verbs
        .into_iter()
        .find(|(w, _)| *w == word)
        .map(|(_, v)| v)
        .ok_or_else(|| format!("{word} is no command of turborel extension; see turborel extension --help"))
}

/// The settings named so far.
#[derive(Debug, Default)]
struct Named {
    settings: Vec<(String, String)>,
    cache: Option<PathBuf>,
    roots: Option<PathBuf>,
}

/// What `args`, the words after `extension`, ask for; `env` gives the
/// environment's variables. None if they ask for the usage text.
///
/// # Errors
///
/// What is wrong with them, for the user.
pub fn parse(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Result<Option<Invocation>, String> {
    let mut rest = args.iter();
    let Some(first) = rest.next() else { return Ok(None) };
    if first == "--help" {
        return Ok(None);
    }
    let verb = verb(first, &mut rest)?;
    let mut named = Named::default();
    let mut inv = Invocation {
        verb,
        words: Vec::new(),
        context: empty_context(),
        latest: false,
        key: None,
        output: None,
        registry_dir: None,
        expires_days: DEFAULT_EXPIRES_DAYS,
    };
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--help" => return Ok(None),
            "--set" => named.settings.push(setting(&value()?)?),
            "--cache" => named.cache = Some(PathBuf::from(value()?)),
            "--tls" => named.roots = Some(PathBuf::from(value()?)),
            "--latest" => inv.latest = true,
            "--key" => inv.key = Some(PathBuf::from(value()?)),
            "--output" => inv.output = Some(PathBuf::from(value()?)),
            "--registry-dir" => inv.registry_dir = Some(PathBuf::from(value()?)),
            "--expires-days" => inv.expires_days = value()?.parse().map_err(|_| "--expires-days takes a number of days".to_owned())?,
            other => {
                if other.starts_with("--") {
                    return Err(format!("{other} is no option of turborel extension; see turborel extension --help"));
                }
                inv.words.push(other.to_owned());
            }
        }
    }
    inv.context = context(&named, env)?;
    Ok(Some(inv))
}

/// A context of no settings.
fn empty_context() -> Context {
    Context {
        directories: Directories::default(),
        registries: Registries::default(),
        trust: Trust::default(),
        cache: None,
        roots: None,
        allow_http: false,
    }
}

/// `NAME=VALUE`, its name one of `SETTINGS`.
fn setting(text: &str) -> Result<(String, String), String> {
    let (name, value) = text.split_once('=').ok_or_else(|| format!("--set takes NAME=VALUE, not {text}"))?;
    let name = name.trim().to_ascii_uppercase();
    if !SETTINGS.contains(&name.as_str()) {
        return Err(format!("{name} is no setting of turborel extension; it reads {}", SETTINGS.join(", ")));
    }
    Ok((name, value.trim().to_owned()))
}

/// The context `named` and `env` give, `--set` before the environment.
fn context(named: &Named, env: &dyn Fn(&str) -> Option<String>) -> Result<Context, String> {
    let get = |name: &str| -> String {
        let set = named.settings.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        set.or_else(|| env(&format!("TURBOREL_{}", name.replace('.', "_")))).unwrap_or_default()
    };
    let [directories, registries, pins, keys, require] = SETTINGS;
    let required = match get(require).to_ascii_uppercase().as_str() {
        "ON" | "TRUE" => true,
        "" | "OFF" | "FALSE" => false,
        other => return Err(format!("PLUGINS.REQUIRE_SIGNED is ON or OFF, not {other}")),
    };
    Ok(Context {
        directories: Directories::parse(&get(directories)),
        registries: Registries::parse(&get(registries), &get(pins)).map_err(|e| e.to_string())?,
        trust: Trust::parse(&get(keys), required).map_err(|e| e.to_string())?,
        cache: named.cache.clone(),
        roots: named.roots.clone(),
        allow_http: false,
    })
}

//! \[EXTENSION\] The command line of `turborel extension` (design document
//! §13.5): a verb, its words, and options. The settings it reads are the
//! server's, named by `--set NAME=VALUE` or by the environment variable
//! of the name, `.` written `_` and `TURBOREL_` before it.

use std::path::PathBuf;

use turborel::plugin::limits::Directories;
use turborel::plugin::package::Trust;

use super::Context;

/// The usage text of `turborel extension`.
pub const EXTENSION_USAGE: &str = "usage: turborel extension COMMAND [options]

Packages are .tbpkg files. turborel has no registry, so the operator copies them
to the server's machine as they like.

  install FILE|DIR...    install package files, or directories of them, choosing
                         among them and the packages installed what each requires;
                         with --lock, exactly what a lock names, from those files
  remove NAME            remove a package nothing installed requires
  autoremove             remove packages installed only as others' dependencies that
                         nothing requires now, and versions the lock does not name
  list                   the packages installed and locked
  info NAME              a package's versions installed, what each requires, its
                         signer and its license
  license NAME           the text of a package's license
  verify                 check every file installed against its package's digests
                         and signature
  why NAME               which packages required NAME
  lock export FILE       write the lock, with each package's digest and license
  new NAME               write a starter project of a plug-in on the plug-in SDK,
                         into --dir (default NAME in lower case), serving an object
                         of each --kind, scalar if none
  pack [DIR]             build the package of the project in DIR (default .), signed
                         with --key, written to --out
  keygen                 write a new signing key to --out and its public key to
                         --out with .pub after

options:
  --set NAME=VALUE       a setting: PLUGINS.DIRECTORIES (the first is where packages
                         are installed), PLUGINS.TRUSTED_KEYS, PLUGINS.REQUIRE_SIGNED
                         (ON or OFF)
  --lock FILE            the lock install reproduces
  --kind KIND            a kind of object new serves: scalar, aggregate, table,
                         procedure, metric, embedding, authenticator, target,
                         partition, ranking, quantizer, collation, system-table,
                         xquery, type, engine, index-method, credentials, diagnostics;
                         or host kinds alone: encoding, hash-function, clock,
                         log-storage, catalog-storage, spill, file-system,
                         served-connections, listener, package-crypto, keyring,
                         background-job, replica-transport, journal,
                         state-transfer, consensus, archive, buffer-eviction
  --dir DIR              where new writes the project
  --sdk DIR              a turborel source tree the project of new depends on by
                         path; by default it depends on this release's crates
  --key FILE             the signing key of pack
  --out FILE             where pack and keygen write

The environment variable TURBOREL_PLUGINS_DIRECTORIES gives PLUGINS.DIRECTORIES,
and so on for each setting. pack finds the program cargo built under
CARGO_TARGET_DIR if it is set.";

/// The settings `turborel extension` reads.
const SETTINGS: [&str; 3] = ["PLUGINS.DIRECTORIES", "PLUGINS.TRUSTED_KEYS", "PLUGINS.REQUIRE_SIGNED"];
/// The environment variable of where cargo builds.
const CARGO_TARGET_DIR: &str = "CARGO_TARGET_DIR";

/// A command of `turborel extension`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// install.
    Install,
    /// remove.
    Remove,
    /// autoremove.
    Autoremove,
    /// list.
    List,
    /// info.
    Info,
    /// license.
    License,
    /// verify.
    Verify,
    /// why.
    Why,
    /// lock export.
    LockExport,
    /// new.
    New,
    /// pack.
    Pack,
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
    /// `--lock`.
    pub lock: Option<PathBuf>,
    /// Each `--kind`.
    pub kinds: Vec<String>,
    /// `--dir`.
    pub dir: Option<PathBuf>,
    /// `--sdk`.
    pub sdk: Option<PathBuf>,
    /// `--key`.
    pub key: Option<PathBuf>,
    /// `--out`.
    pub out: Option<PathBuf>,
    /// Where cargo builds, as CARGO_TARGET_DIR says, if it does.
    pub target_dir: Option<PathBuf>,
}

/// The command `word` names, the second word taken if it has one.
fn verb(word: &str, rest: &mut std::slice::Iter<'_, String>) -> Result<Verb, String> {
    let verbs = [
        ("install", Verb::Install),
        ("remove", Verb::Remove),
        ("autoremove", Verb::Autoremove),
        ("list", Verb::List),
        ("info", Verb::Info),
        ("license", Verb::License),
        ("verify", Verb::Verify),
        ("why", Verb::Why),
        ("new", Verb::New),
        ("pack", Verb::Pack),
        ("keygen", Verb::Keygen),
    ];
    if word == "lock" {
        return match rest.next().map(String::as_str) {
            Some("export") => Ok(Verb::LockExport),
            Some(_) | None => Err("lock takes export; install --lock FILE installs what a lock names".to_owned()),
        };
    }
    verbs
        .into_iter()
        .find(|(w, _)| *w == word)
        .map(|(_, v)| v)
        .ok_or_else(|| format!("{word} is no command of turborel extension; see turborel extension --help"))
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
    let mut settings = Vec::new();
    let mut inv = Invocation {
        verb,
        words: Vec::new(),
        context: Context { directories: Directories::default(), trust: Trust::default(), cwd: PathBuf::from(".") },
        lock: None,
        kinds: Vec::new(),
        dir: None,
        sdk: None,
        key: None,
        out: None,
        target_dir: env(CARGO_TARGET_DIR).map(PathBuf::from),
    };
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--help" => return Ok(None),
            "--set" => settings.push(setting(&value()?)?),
            "--lock" => inv.lock = Some(PathBuf::from(value()?)),
            "--kind" => inv.kinds.push(value()?),
            "--dir" => inv.dir = Some(PathBuf::from(value()?)),
            "--sdk" => inv.sdk = Some(PathBuf::from(value()?)),
            "--key" => inv.key = Some(PathBuf::from(value()?)),
            "--out" => inv.out = Some(PathBuf::from(value()?)),
            other => {
                if other.starts_with("--") {
                    return Err(format!("{other} is no option of turborel extension; see turborel extension --help"));
                }
                inv.words.push(other.to_owned());
            }
        }
    }
    let cwd = inv.context.cwd.clone();
    inv.context = context(&settings, env, cwd)?;
    Ok(Some(inv))
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

/// The context `settings` and `env` give, `--set` before the environment,
/// paths taken from `cwd`.
fn context(settings: &[(String, String)], env: &dyn Fn(&str) -> Option<String>, cwd: PathBuf) -> Result<Context, String> {
    let get = |name: &str| -> String {
        let set = settings.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        set.or_else(|| env(&format!("TURBOREL_{}", name.replace('.', "_")))).unwrap_or_default()
    };
    let [directories, keys, require] = SETTINGS;
    let required = match get(require).to_ascii_uppercase().as_str() {
        "ON" | "TRUE" => true,
        "" | "OFF" | "FALSE" => false,
        other => return Err(format!("PLUGINS.REQUIRE_SIGNED is ON or OFF, not {other}")),
    };
    let trust = Trust::parse(&get(keys), required).map_err(|e| e.to_string())?;
    Ok(Context { directories: Directories::parse(&get(directories)), trust, cwd })
}

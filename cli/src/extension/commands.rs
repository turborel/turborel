//! \[EXTENSION\] The commands that change the plug-in directory (design
//! document §13.3, §13.6): install, upgrade, remove, autoremove, clean,
//! and the lock's export and import.
//!
//! Install and upgrade resolve the whole set again: every package asked
//! for by name, with its range, and those they require, the versions
//! locked kept where they still fit. What is chosen is fetched, checked
//! and unpacked, and the lock is written as the set chosen. Install with
//! no package, and the lock's import, install exactly what the lock
//! names, each package file checked against the digest locked.

use std::io::Write;
use std::path::Path;

use turborel::plugin::package::archive::Package;
use turborel::plugin::package::index::IndexSource;
use turborel::plugin::package::install::{Installed, install_package, mark};
use turborel::plugin::package::lock::{Lock, Locked};
use turborel::plugin::package::resolve::{Candidate, InstalledSource, Origin, Request, Source, resolve, this_release};
use turborel::plugin::package::semver::{Range, Version};
use turborel::plugin::package::{DIGESTS_FILE, PACKAGE_EXTENSION, PackageCrypto, verify_installed};
use turborel_client::packages::Crypto;

use super::installed::{self, read_lock, write_lock};
use super::{Context, package_name, registry, say};

/// The digests of package files given, by the name of each package.
type FileDigests = [(String, [u8; 32])];

/// The versions of the registries, then those installed.
struct Everywhere<'a> {
    registries: IndexSource,
    installed: InstalledSource<'a>,
}

impl Source for Everywhere<'_> {
    fn versions(&self, name: &str) -> turborel::error::Result<Vec<Candidate>> {
        let mut all = self.registries.versions(name)?;
        for c in self.installed.versions(name)? {
            if !all.iter().any(|a| a.version == c.version) {
                all.push(c);
            }
        }
        Ok(all)
    }
}

/// `NAME[@RANGE]` as typed.
fn wanted(typed: &str) -> Result<(String, Range), String> {
    let (name, range) = typed.split_once('@').map_or((typed, "*"), |(n, r)| (n, r));
    Ok((package_name(name), Range::parse(range).map_err(|e| e.to_string())?))
}

/// Installs `words`: names with ranges, or package files; with none,
/// exactly what the lock names.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn install(cx: &Context, words: &[String], out: &mut dyn Write) -> Result<(), String> {
    let lock = read_lock(cx)?;
    if words.is_empty() {
        return reproduce(cx, &lock, out);
    }
    let mut roots: Vec<(String, Range)> = Vec::new();
    let mut files = Vec::new();
    for word in words {
        if word.ends_with(&format!(".{PACKAGE_EXTENSION}")) || Path::new(word).is_file() {
            let (name, version, digest) = install_file(cx, Path::new(word), out)?;
            roots.push((name.clone(), Range::parse(&format!("^{version}")).map_err(|e| e.to_string())?));
            files.push((name, digest));
        } else {
            roots.push(wanted(word)?);
        }
    }
    let asked: Vec<String> = roots.iter().map(|(n, _)| n.clone()).collect();
    let pinned: Vec<(String, Version)> =
        lock.packages.iter().filter(|p| !asked.contains(&p.name)).map(|p| (p.name.clone(), p.version.clone())).collect();
    settle(cx, (lock, roots), (&pinned, &files), out)
}

/// Installs the package file `file`, by name: its name, version and
/// digest.
fn install_file(cx: &Context, file: &Path, out: &mut dyn Write) -> Result<(String, Version, [u8; 32]), String> {
    let bytes = std::fs::read(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let package = Package::read(&bytes, &Crypto).map_err(|e| format!("{}: {e}", file.display()))?;
    let dir = install_package(&package, &cx.directories, (&Crypto, &cx.trust), Installed::Explicit).map_err(|e| e.to_string())?;
    registry::keep(cx, &bytes)?;
    let manifest = package.manifest().map_err(|e| e.to_string())?;
    let version = Version::parse(&manifest.version).map_err(|e| e.to_string())?;
    say(out, &format!("unpacked {} {} into {}", manifest.name, version, dir.display()))?;
    Ok((manifest.name, version, Crypto.sha256(&bytes)))
}

/// Resolves `roots`, with the packages already `lock`ed, those `pinned`
/// kept, and installs what is chosen; the digests of package files
/// installed are `files`'.
fn settle(
    cx: &Context,
    (lock, mut roots): (Lock, Vec<(String, Range)>),
    (pinned, files): (&[(String, Version)], &FileDigests),
    out: &mut dyn Write,
) -> Result<(), String> {
    for p in lock.packages.iter().filter(|p| p.wanted.is_some()) {
        if !roots.iter().any(|(n, _)| *n == p.name) {
            roots.push((p.name.clone(), p.wanted.clone().unwrap_or_else(Range::any)));
        }
    }
    let source = Everywhere { registries: registry::indexes(cx)?, installed: InstalledSource(&cx.directories) };
    let explicit_pins: Vec<(String, Version)> =
        pinned.iter().filter(|(n, _)| lock.get(n).is_some_and(|l| l.wanted.is_some())).cloned().collect();
    let request = |pins: &[(String, Version)]| Request { roots: roots.clone(), pinned: pins.to_vec(), turborel: this_release() };
    // The versions locked are kept if they fit; if not, those of the
    // packages asked for by name alone.
    let chosen = resolve(&source, &request(pinned)).or_else(|_| resolve(&source, &request(&explicit_pins))).map_err(|e| e.to_string())?;
    let mut next = Lock::default();
    for c in &chosen {
        let range = roots.iter().find(|(n, _)| *n == c.name).map(|(_, r)| r.clone());
        next.put(take(cx, c, (&lock, files), range, out)?);
    }
    write_lock(cx, &next)
}

/// Installs the version `c` chosen, unless it is: its lock entry, asked
/// for by name with `range` if it was.
fn take(
    cx: &Context,
    c: &Candidate,
    (lock, files): (&Lock, &FileDigests),
    range: Option<Range>,
    out: &mut dyn Write,
) -> Result<Locked, String> {
    let how = if range.is_some() { Installed::Explicit } else { Installed::Dependency };
    let locked = lock.get(&c.name).filter(|l| l.version == c.version);
    let (dir, digest, registry) = match &c.origin {
        Origin::Registry { registry, file, digest, .. } => {
            let dir = if let Some(present) = installed::find(cx, &c.name, &c.version)? {
                present.dir
            } else {
                let bytes = registry::package(cx, (registry, file), digest)?;
                let package = Package::read(&bytes, &Crypto).map_err(|e| e.to_string())?;
                let dir = install_package(&package, &cx.directories, (&Crypto, &cx.trust), how).map_err(|e| e.to_string())?;
                say(out, &format!("installed {} {} from {registry}", c.name, c.version))?;
                dir
            };
            (dir, *digest, Some(registry.clone()))
        }
        Origin::Installed(dir) => {
            let from_file = files.iter().find(|(n, _)| *n == c.name).map(|(_, d)| *d);
            let digest = locked.map(|l| l.digest).or(from_file).unwrap_or_else(|| digest_of_installed(dir));
            (dir.clone(), digest, locked.and_then(|l| l.registry.clone()))
        }
    };
    verify_installed(&dir, &Crypto, &cx.trust).map_err(|e| e.to_string())?;
    if how == Installed::Explicit || locked.is_none_or(|l| l.wanted.is_none()) {
        mark(&dir, how).map_err(|e| e.to_string())?;
    }
    Ok(Locked { name: c.name.clone(), version: c.version.clone(), digest, registry, wanted: range })
}

/// What stands for the digest of a package installed with no file
/// fetched or given: the digest of its digests file, which names every
/// file's.
fn digest_of_installed(dir: &Path) -> [u8; 32] {
    Crypto.sha256(&std::fs::read(dir.join(DIGESTS_FILE)).unwrap_or_default())
}

/// Installs exactly what `lock` names: each package installed already
/// checked, the others from the cache or their registry, each file's
/// digest the one locked.
fn reproduce(cx: &Context, lock: &Lock, out: &mut dyn Write) -> Result<(), String> {
    for p in &lock.packages {
        let how = if p.wanted.is_some() { Installed::Explicit } else { Installed::Dependency };
        if let Some(present) = installed::find(cx, &p.name, &p.version)? {
            verify_installed(&present.dir, &Crypto, &cx.trust).map_err(|e| e.to_string())?;
            say(out, &format!("kept {} {}", p.name, p.version))?;
            continue;
        }
        let bytes = match registry::from_cache(cx, &p.digest) {
            Some(bytes) => bytes,
            None => fetch_locked(cx, p)?,
        };
        let package = Package::read(&bytes, &Crypto).map_err(|e| e.to_string())?;
        install_package(&package, &cx.directories, (&Crypto, &cx.trust), how).map_err(|e| e.to_string())?;
        say(out, &format!("installed {} {}", p.name, p.version))?;
    }
    write_lock(cx, lock)
}

/// The package file `p` locks, from its registry: the index must list it
/// with the digest locked.
fn fetch_locked(cx: &Context, p: &Locked) -> Result<Vec<u8>, String> {
    let url = p.registry.as_deref().ok_or_else(|| format!("{} {} is in no registry and not in the cache", p.name, p.version))?;
    let index = registry::index(cx, url)?;
    let listed = index
        .packages
        .iter()
        .find(|l| l.name == p.name && l.version == p.version)
        .ok_or_else(|| format!("{url} no longer lists {} {}", p.name, p.version))?;
    if listed.digest != p.digest {
        return Err(turborel::plugin::package::errors::changed(&format!(
            "{url} lists {} {} with another digest than the lock's",
            p.name, p.version
        ))
        .to_string());
    }
    registry::package(cx, (url, &listed.file), &p.digest)
}

/// Upgrades `words`, or every package asked for by name: to the newest
/// versions their ranges allow, or with `latest` beyond them, the range
/// kept then the newest's caret.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn upgrade(cx: &Context, words: &[String], latest: bool, out: &mut dyn Write) -> Result<(), String> {
    let mut lock = read_lock(cx)?;
    let targets: Vec<String> = if words.is_empty() {
        lock.packages.iter().filter(|p| p.wanted.is_some()).map(|p| p.name.clone()).collect()
    } else {
        words.iter().map(|w| package_name(w)).collect()
    };
    for t in &targets {
        if lock.get(t).is_none() {
            return Err(format!("{t} is not installed"));
        }
    }
    let mut roots = Vec::new();
    for p in lock.packages.iter_mut().filter(|p| p.wanted.is_some()) {
        let range = if latest && targets.contains(&p.name) { Range::any() } else { p.wanted.clone().unwrap_or_else(Range::any) };
        p.wanted = Some(range.clone());
        roots.push((p.name.clone(), range));
    }
    let pinned: Vec<(String, Version)> = lock
        .packages
        .iter()
        .filter(|p| p.wanted.is_some() && !targets.contains(&p.name))
        .map(|p| (p.name.clone(), p.version.clone()))
        .collect();
    settle(cx, (lock, roots), (&pinned, &[]), out)?;
    if latest {
        let mut relocked = read_lock(cx)?;
        for p in relocked.packages.iter_mut().filter(|p| targets.contains(&p.name)) {
            p.wanted = Range::parse(&format!("^{}", p.version)).ok();
        }
        write_lock(cx, &relocked)?;
    }
    Ok(())
}

/// The names of the packages installed that require `name`.
fn dependants(cx: &Context, lock: &Lock, name: &str) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    for p in &lock.packages {
        if let Some(present) = installed::find(cx, &p.name, &p.version)?
            && present.requires().iter().any(|(n, _)| n == name)
        {
            found.push(format!("{} {}", p.name, p.version));
        }
    }
    Ok(found)
}

/// Removes the package `typed`, which nothing installed may require.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn remove(cx: &Context, typed: &str, out: &mut dyn Write) -> Result<(), String> {
    let name = package_name(typed);
    let mut lock = read_lock(cx)?;
    let locked = lock.get(&name).cloned().ok_or_else(|| format!("{name} is not installed"))?;
    let by = dependants(cx, &lock, &name)?;
    if !by.is_empty() {
        return Err(format!("{name} is required by {}; remove them first", by.join(", ")));
    }
    lock.packages.retain(|p| p.name != name);
    take_away(cx, &name, &locked.version, out)?;
    write_lock(cx, &lock)
}

/// Deletes the directory of `name` of `version` in the plug-in directory.
fn take_away(cx: &Context, name: &str, version: &Version, out: &mut dyn Write) -> Result<(), String> {
    let dir = cx.home()?.join(name).join(version.to_string());
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("cannot remove {}: {e}", dir.display()))?;
    }
    say(out, &format!("removed {name} {version}"))
}

/// Removes what nothing asked for by name requires: the packages locked
/// as dependencies no package requires now, those installed as
/// dependencies and not locked, and every version of a package locked
/// other than the one locked.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn autoremove(cx: &Context, out: &mut dyn Write) -> Result<(), String> {
    let mut lock = read_lock(cx)?;
    let mut needed: Vec<String> = lock.packages.iter().filter(|p| p.wanted.is_some()).map(|p| p.name.clone()).collect();
    let mut at = 0;
    while let Some(name) = needed.get(at).cloned() {
        at = at.saturating_add(1);
        let Some(p) = lock.get(&name) else { continue };
        for (required, _) in installed::find(cx, &p.name, &p.version)?.map(|i| i.requires()).unwrap_or_default() {
            if !needed.contains(&required) {
                needed.push(required);
            }
        }
    }
    lock.packages.retain(|p| needed.contains(&p.name));
    let home = cx.home()?;
    for present in installed::all(cx)?.into_iter().filter(|p| p.dir.starts_with(&home)) {
        let kept = lock.get(&present.name).is_some_and(|l| l.version == present.version);
        let unlocked_explicit = lock.get(&present.name).is_none() && present.how == Installed::Explicit;
        if !kept && !unlocked_explicit {
            take_away(cx, &present.name, &present.version, out)?;
        }
    }
    write_lock(cx, &lock)
}

/// Empties the cache.
///
/// # Errors
///
/// If it cannot be removed.
pub(super) fn clean(cx: &Context, out: &mut dyn Write) -> Result<(), String> {
    let dir = cx.cache_dir()?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("cannot empty {}: {e}", dir.display()))?;
    }
    say(out, &format!("emptied {}", dir.display()))
}

/// Writes the lock to `file`.
///
/// # Errors
///
/// If it cannot be written.
pub(super) fn lock_export(cx: &Context, file: &str, out: &mut dyn Write) -> Result<(), String> {
    std::fs::write(file, read_lock(cx)?.write()).map_err(|e| format!("cannot write {file}: {e}"))?;
    say(out, &format!("wrote the lock to {file}"))
}

/// Installs exactly what the lock `file` names, and keeps it as the lock.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn lock_import(cx: &Context, file: &str, out: &mut dyn Write) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let lock = Lock::parse(&text).map_err(|e| format!("{file}: {e}"))?;
    reproduce(cx, &lock, out)
}

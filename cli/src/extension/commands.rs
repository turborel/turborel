//! \[EXTENSION\] The commands that change the plug-in directory (design
//! document §13.3, §13.6, RULES.md D261, D262): install, remove,
//! autoremove, and the lock's export.
//!
//! Install resolves the whole set again: every package the files given
//! ask for by name, those already asked for by name, and what they
//! require, chosen among the files given and the packages installed, the
//! versions locked kept where they still fit. What is chosen is checked
//! and unpacked, its license told, and the lock is written as the set
//! chosen. With `--lock`, install takes exactly what a lock names, each
//! from a package installed or a file given whose digest is the one
//! locked.

use std::io::Write;
use std::path::Path;

use turborel::plugin::package::errors::{changed, conflict};
use turborel::plugin::package::install::{Installed, install_package, mark};
use turborel::plugin::package::lock::{Lock, Locked};
use turborel::plugin::package::resolve::{Candidate, InstalledSource, Origin, Request, Source, resolve, this_release};
use turborel::plugin::package::semver::{Range, Version};
use turborel::plugin::package::{DIGESTS_FILE, License, PackageCrypto, verify_installed};
use turborel_client::packages::Crypto;

use super::files::{Given, GivenSource, given, roots};
use super::installed::{self, license_of, read_lock, write_lock};
use super::{Context, package_name, say};

/// The versions of the files given, then those installed.
struct Everywhere<'a> {
    given: GivenSource<'a>,
    installed: InstalledSource<'a>,
}

impl Source for Everywhere<'_> {
    fn versions(&self, name: &str) -> turborel::error::Result<Vec<Candidate>> {
        let mut all = self.given.versions(name)?;
        for c in self.installed.versions(name)? {
            if !all.iter().any(|a| a.version == c.version) {
                all.push(c);
            }
        }
        Ok(all)
    }
}

/// Installs the package files `words` name, or with `lock_file` exactly
/// what that lock names.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn install(cx: &Context, words: &[String], lock_file: Option<&Path>, out: &mut dyn Write) -> Result<(), String> {
    if words.is_empty() && lock_file.is_none() {
        return Err("name the package files to install, or directories of them; see turborel extension --help".to_owned());
    }
    let files = given(cx, words)?;
    if let Some(file) = lock_file {
        let text = std::fs::read_to_string(cx.at(file)).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let locked = Lock::parse(&text).map_err(|e| format!("{}: {e}", file.display()))?;
        return reproduce(cx, &locked, &files, out);
    }
    let lock = read_lock(cx)?;
    let roots = roots(&files)?;
    let asked: Vec<String> = roots.iter().map(|(n, _)| n.clone()).collect();
    let pinned: Vec<(String, Version)> =
        lock.packages.iter().filter(|p| !asked.contains(&p.name)).map(|p| (p.name.clone(), p.version.clone())).collect();
    settle(cx, (lock, roots), (&pinned, &files), out)
}

/// Resolves `roots`, with the packages already `lock`ed, those `pinned`
/// kept, among the files `given` and the packages installed, and installs
/// what is chosen.
fn settle(
    cx: &Context,
    (lock, mut roots): (Lock, Vec<(String, Range)>),
    (pinned, given): (&[(String, Version)], &[Given]),
    out: &mut dyn Write,
) -> Result<(), String> {
    for p in lock.packages.iter().filter(|p| p.wanted.is_some()) {
        if !roots.iter().any(|(n, _)| *n == p.name) {
            roots.push((p.name.clone(), p.wanted.clone().unwrap_or_else(Range::any)));
        }
    }
    let dirs = cx.dirs();
    let source = Everywhere { given: GivenSource(given), installed: InstalledSource(&dirs) };
    let explicit_pins: Vec<(String, Version)> =
        pinned.iter().filter(|(n, _)| lock.get(n).is_some_and(|l| l.wanted.is_some())).cloned().collect();
    let request = |pins: &[(String, Version)]| Request { roots: roots.clone(), pinned: pins.to_vec(), turborel: this_release() };
    // The versions locked are kept if they fit; if not, those of the
    // packages asked for by name alone.
    let chosen = resolve(&source, &request(pinned)).or_else(|_| resolve(&source, &request(&explicit_pins))).map_err(|e| e.to_string())?;
    let mut next = Lock::default();
    for c in &chosen {
        let range = roots.iter().find(|(n, _)| *n == c.name).map(|(_, r)| r.clone());
        next.put(take(cx, c, (&lock, given), range, out)?);
    }
    for g in given.iter().filter(|g| !chosen.iter().any(|c| c.name == g.manifest.name && c.version == g.version)) {
        say(out, &format!("passed over {} {} of {}, which nothing chosen needs", g.manifest.name, g.version, g.file.display()))?;
    }
    write_lock(cx, &next)
}

/// Installs the version `c` chosen, unless it is installed, and gives
/// its lock entry, asked for by name with `range` if it was.
fn take(
    cx: &Context,
    c: &Candidate,
    (lock, given): (&Lock, &[Given]),
    range: Option<Range>,
    out: &mut dyn Write,
) -> Result<Locked, String> {
    let how = if range.is_some() { Installed::Explicit } else { Installed::Dependency };
    let locked = lock.get(&c.name).filter(|l| l.version == c.version);
    let (dir, digest) = match &c.origin {
        Origin::File { file, digest } => {
            let g = given.iter().find(|g| g.file == *file).ok_or_else(|| format!("{} was not read", file.display()))?;
            let present = installed::find(cx, &c.name, &c.version)?.is_some();
            let dir = install_package(&g.package, &cx.dirs(), (&Crypto, &cx.trust), how).map_err(|e| e.to_string())?;
            let verb = if present { "kept" } else { "installed" };
            say(out, &format!("{verb} {} {} from {}, {}", c.name, c.version, file.display(), License::of(&g.manifest).told()))?;
            (dir, *digest)
        }
        Origin::Installed(dir) => {
            let digest = locked.map_or_else(|| digest_of_installed(dir), |l| l.digest);
            (dir.clone(), digest)
        }
    };
    verify_installed(&dir, &Crypto, &cx.trust).map_err(|e| e.to_string())?;
    if how == Installed::Explicit || locked.is_none_or(|l| l.wanted.is_none()) {
        mark(&dir, how).map_err(|e| e.to_string())?;
    }
    let license = license_of(&dir);
    Ok(Locked {
        name: c.name.clone(),
        version: c.version.clone(),
        digest,
        wanted: range,
        license: license.expression,
        license_file: license.file,
    })
}

/// What stands for the digest of a package installed with no file given:
/// the digest of its digests file, which names every file's.
fn digest_of_installed(dir: &Path) -> [u8; 32] {
    Crypto.sha256(&std::fs::read(dir.join(DIGESTS_FILE)).unwrap_or_default())
}

/// Installs exactly what `lock` names: each package installed already
/// checked, the others from the files `given`, each file's digest the one
/// locked; then keeps `lock` as the lock.
fn reproduce(cx: &Context, lock: &Lock, given: &[Given], out: &mut dyn Write) -> Result<(), String> {
    for p in &lock.packages {
        let how = if p.wanted.is_some() { Installed::Explicit } else { Installed::Dependency };
        let file = given.iter().find(|g| g.manifest.name == p.name && g.version == p.version);
        if let Some(g) = file.filter(|g| g.digest != p.digest) {
            let why = format!("{} holds {} {} with another digest than the lock's", g.file.display(), p.name, p.version);
            return Err(changed(&why).to_string());
        }
        if let Some(present) = installed::find(cx, &p.name, &p.version)? {
            verify_installed(&present.dir, &Crypto, &cx.trust).map_err(|e| e.to_string())?;
            say(out, &format!("kept {} {}", p.name, p.version))?;
            continue;
        }
        let g = file.ok_or_else(|| {
            let why = format!("the lock names {} {}, which is not installed; add its package file to the files given", p.name, p.version);
            conflict(&why).to_string()
        })?;
        install_package(&g.package, &cx.dirs(), (&Crypto, &cx.trust), how).map_err(|e| e.to_string())?;
        say(out, &format!("installed {} {} from {}, {}", p.name, p.version, g.file.display(), License::of(&g.manifest).told()))?;
    }
    write_lock(cx, lock)
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

/// Writes the lock to `file`, each package's license as its manifest
/// installed declares it.
///
/// # Errors
///
/// If it cannot be written.
pub(super) fn lock_export(cx: &Context, file: &str, out: &mut dyn Write) -> Result<(), String> {
    let mut lock = read_lock(cx)?;
    for p in &mut lock.packages {
        if let Some(present) = installed::find(cx, &p.name, &p.version)? {
            let license = License::of(&present.manifest);
            (p.license, p.license_file) = (license.expression, license.file);
        }
    }
    std::fs::write(cx.at(Path::new(file)), lock.write()).map_err(|e| format!("cannot write {file}: {e}"))?;
    say(out, &format!("wrote the lock to {file}"))
}

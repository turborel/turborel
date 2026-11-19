//! \[EXTENSION\] The commands that only tell (design document §13.5,
//! §13.6): list, verify, outdated, why, search and info.

use std::io::Write;

use turborel::plugin::package::index::Listed;
use turborel::plugin::package::install::Installed;
use turborel::plugin::package::semver::Version;
use turborel::plugin::package::verify_installed;
use turborel_client::packages::Crypto;

use super::installed::{self, read_lock};
use super::{Context, package_name, registry, say};

/// Tells the packages locked, then those installed and not locked.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn list(cx: &Context, out: &mut dyn Write) -> Result<(), String> {
    let lock = read_lock(cx)?;
    for p in &lock.packages {
        let how = p.wanted.as_ref().map_or_else(|| "dependency".to_owned(), |r| format!("explicit {r}"));
        say(out, &format!("{} {} {how} {}", p.name, p.version, p.registry.as_deref().unwrap_or("-")))?;
    }
    for p in installed::all(cx)? {
        if lock.get(&p.name).is_none_or(|l| l.version != p.version) {
            say(out, &format!("{} {} not locked {}", p.name, p.version, p.dir.display()))?;
        }
    }
    Ok(())
}

/// Checks every package installed against its digests and signature.
///
/// # Errors
///
/// Naming each that fails.
pub(super) fn verify(cx: &Context, out: &mut dyn Write) -> Result<(), String> {
    let mut failed = Vec::new();
    for p in installed::all(cx)? {
        match verify_installed(&p.dir, &Crypto, &cx.trust) {
            Ok(signer) => say(out, &format!("ok {} {} signed by {}", p.name, p.version, signer.as_deref().unwrap_or("no key trusted")))?,
            Err(e) => failed.push(format!("{} {}: {e}", p.name, p.version)),
        }
    }
    if failed.is_empty() { Ok(()) } else { Err(failed.join("\n")) }
}

/// The versions the registries list of `name`, newest first.
fn listed(cx: &Context, name: &str) -> Result<Vec<(String, Listed)>, String> {
    let mut all = Vec::new();
    for (url, index) in registry::indexes(cx)?.indexes {
        all.extend(index.packages.into_iter().filter(|p| p.name == name).map(|p| (url.clone(), p)));
    }
    all.sort_by(|a, b| b.1.version.cmp(&a.1.version));
    Ok(all)
}

/// Tells each package locked with a newer version: the newest its range
/// allows and the newest of all.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn outdated(cx: &Context, out: &mut dyn Write) -> Result<(), String> {
    let lock = read_lock(cx)?;
    let source = registry::indexes(cx)?;
    say(out, "name current wanted latest")?;
    for p in &lock.packages {
        let mut versions: Vec<Version> = Vec::new();
        for (_, index) in &source.indexes {
            versions.extend(index.packages.iter().filter(|l| l.name == p.name && !l.version.is_prerelease()).map(|l| l.version.clone()));
        }
        let latest = versions.iter().max().cloned().unwrap_or_else(|| p.version.clone());
        let fits = |v: &&Version| p.wanted.as_ref().is_none_or(|r| r.matches(v));
        let wanted = versions.iter().filter(fits).max().cloned().unwrap_or_else(|| p.version.clone()).max(p.version.clone());
        if latest > p.version || wanted > p.version {
            say(out, &format!("{} {} {wanted} {latest}", p.name, p.version))?;
        }
    }
    Ok(())
}

/// Tells why `typed` is installed: asked for by name, or required by
/// which packages, each in turn back to one asked for.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn why(cx: &Context, typed: &str, out: &mut dyn Write) -> Result<(), String> {
    let name = package_name(typed);
    let lock = read_lock(cx)?;
    let mut pending = vec![(name.clone(), 0_usize)];
    let mut seen: Vec<String> = Vec::new();
    lock.get(&name).ok_or_else(|| format!("{name} is not installed"))?;
    while let Some((at, depth)) = pending.pop() {
        if seen.contains(&at) {
            continue;
        }
        seen.push(at.clone());
        let indent = "  ".repeat(depth);
        if let Some(l) = lock.get(&at).filter(|l| l.wanted.is_some()) {
            say(out, &format!("{indent}{at} {} was installed by name", l.version))?;
        }
        for p in &lock.packages {
            let Some(present) = installed::find(cx, &p.name, &p.version)? else { continue };
            if let Some((_, range)) = present.requires().into_iter().find(|(n, _)| *n == at) {
                say(out, &format!("{indent}{at} is required by {} {} ({range})", p.name, p.version))?;
                pending.push((p.name.clone(), depth.saturating_add(1)));
            }
        }
    }
    Ok(())
}

/// Tells the packages of the registries whose names hold one of `words`:
/// each once, its newest version and its registry.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn search(cx: &Context, words: &[String], out: &mut dyn Write) -> Result<(), String> {
    let wanted: Vec<String> = words.iter().map(|w| w.to_ascii_uppercase()).collect();
    let mut told: Vec<String> = Vec::new();
    for (url, index) in registry::indexes(cx)?.indexes {
        let mut found: Vec<&Listed> =
            index.packages.iter().filter(|p| wanted.is_empty() || wanted.iter().any(|w| p.name.contains(w.as_str()))).collect();
        found.sort_by(|a, b| (&a.name, &b.version).cmp(&(&b.name, &a.version)));
        for p in found {
            if !told.contains(&p.name) {
                told.push(p.name.clone());
                say(out, &format!("{} {} {url}", p.name, p.version))?;
            }
        }
    }
    Ok(())
}

/// Tells the versions of `typed`: each registry's, with what it requires
/// and its signer, then those installed.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn info(cx: &Context, typed: &str, out: &mut dyn Write) -> Result<(), String> {
    let name = package_name(typed);
    say(out, &name)?;
    for (url, p) in listed(cx, &name)? {
        let requires: Vec<String> = p.requires.iter().map(|(n, r)| format!("{n} {r}")).collect();
        let requires = if requires.is_empty() { "nothing".to_owned() } else { requires.join(", ") };
        say(out, &format!("  {} from {url}, signed by {}, requires {requires}", p.version, p.signer.as_deref().unwrap_or("no key")))?;
    }
    for p in installed::all(cx)?.into_iter().filter(|p| p.name == name) {
        let how = match p.how {
            Installed::Explicit => "by name",
            Installed::Dependency => "as a dependency",
        };
        say(out, &format!("  {} installed {how} in {}", p.version, p.dir.display()))?;
    }
    Ok(())
}

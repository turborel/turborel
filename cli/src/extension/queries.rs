//! \[EXTENSION\] The commands that only tell (design document §13.5,
//! §13.6, RULES.md D267): list, info, license, verify and why.

use std::io::Write;

use turborel::plugin::package::install::Installed;
use turborel::plugin::package::{package_license, verify_installed};
use turborel_client::packages::Crypto;

use super::installed::{self, Present, read_lock};
use super::{Context, package_name, say};

/// Tells the packages locked, then those installed and not locked.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn list(cx: &Context, out: &mut dyn Write) -> Result<(), String> {
    let lock = read_lock(cx)?;
    for p in &lock.packages {
        let how = p.wanted.as_ref().map_or_else(|| "dependency".to_owned(), |r| format!("explicit {r}"));
        say(out, &format!("{} {} {how}", p.name, p.version))?;
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

/// Tells the versions of `typed` installed: how each was, what it
/// requires, its signer and its license.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn info(cx: &Context, typed: &str, out: &mut dyn Write) -> Result<(), String> {
    let name = package_name(typed);
    let all: Vec<Present> = installed::all(cx)?.into_iter().filter(|p| p.name == name).collect();
    if all.is_empty() {
        return Err(format!("{name} is not installed"));
    }
    say(out, &name)?;
    for p in all {
        let how = match p.how {
            Installed::Explicit => "by name",
            Installed::Dependency => "as a dependency",
        };
        let requires: Vec<String> = p.manifest.requires.iter().map(|(n, r)| format!("{n} {r}")).collect();
        let requires = if requires.is_empty() { "nothing".to_owned() } else { requires.join(", ") };
        let signer = verify_installed(&p.dir, &Crypto, &cx.trust).map_err(|e| e.to_string())?;
        let signer = signer.as_deref().unwrap_or("no key trusted");
        let license = installed::license_of(&p.dir).told();
        say(out, &format!("  {} installed {how}, requires {requires}, signed by {signer}, {license}", p.version))?;
    }
    Ok(())
}

/// Writes the license of `typed`, of the version locked or else the
/// newest installed: the text of its LICENSE FILE, else its expression,
/// else that it declares none.
///
/// # Errors
///
/// If it is not installed, or its license's file cannot be read.
pub(super) fn license(cx: &Context, typed: &str, out: &mut dyn Write) -> Result<(), String> {
    let name = package_name(typed);
    let lock = read_lock(cx)?;
    let all: Vec<Present> = installed::all(cx)?.into_iter().filter(|p| p.name == name).collect();
    let locked = lock.get(&name).and_then(|l| all.iter().find(|p| p.version == l.version));
    let p = locked.or_else(|| all.last()).ok_or_else(|| format!("{name} is not installed"))?;
    let license = package_license(&p.dir).map_err(|e| e.to_string())?;
    match (&license.text, &license.expression) {
        (Some(text), _) => write!(out, "{text}").map_err(|e| format!("cannot write the report: {e}")),
        (None, Some(expression)) => {
            say(out, &format!("{name} {} is licensed {expression}, and its package holds no text of it", p.version))
        }
        (None, None) => say(out, &format!("{name} {} declares no license", p.version)),
    }
}

//! \[EXTENSION\] Installing a package, and checking one installed (design
//! document §13.3, §13.4).
//!
//! A package is unpacked into `<directory>/<name>/<version>/`, the
//! directory the first of PLUGINS.DIRECTORIES, beside a file of its
//! entries' digests and its signature (`DIGESTS_FILE`), format 1:
//!
//! ```text
//! turborel-package 1
//! signature <public key hex> <signature hex>     (if signed)
//! <SHA-256 hex> <path>                           (each entry, in order)
//! ```
//!
//! and one that says whether it was installed by name or as another's
//! dependency (`STATE_FILE`). Every file is checked against its digest
//! as a package is installed and each time its program starts, so a file
//! changed after is refused.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{Code, Result, SqlError};
use crate::plugin::MANIFEST_FILE;
use crate::plugin::limits::Directories;
use crate::plugin::manifest::Manifest;

use super::archive::{Entry, EntryKind, Package, check_path, place, read_error, signed_message};
use super::crypto::{Digest, PackageCrypto, PublicKey, Signature};
use super::errors::{changed, malformed, untrusted};
use super::semver::{Range, Version};
use super::trust::Trust;
use super::{DIGESTS_FILE, FORMAT, STATE_FILE, hex, unhex_array};

/// The first line of a digests file.
const DIGESTS_HEADER: &str = "turborel-package";
/// What a state file says of a package installed by name.
pub const EXPLICIT: &str = "explicit";
/// What it says of one installed only as another's dependency.
pub const DEPENDENCY: &str = "dependency";

/// Counts the directories an install writes into, so two at once name
/// theirs apart.
static STAGED: AtomicU64 = AtomicU64::new(0);

/// How a package came to be installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    /// By its name.
    Explicit,
    /// Only as another's dependency.
    Dependency,
}

/// The error of `path` that cannot be written.
fn write_error(path: &Path, e: &std::io::Error) -> SqlError {
    SqlError::new(Code::Io, format!("[EXTENSION] {} cannot be written: {e}", path.display()))
}

/// Installs the package file `file` by name, as `install_package` does.
///
/// # Errors
///
/// As `install_package`'s; of reading the file.
pub fn install_file(file: &Path, dirs: &Directories, crypto: &dyn PackageCrypto, trust: &Trust) -> Result<PathBuf> {
    let bytes = std::fs::read(file).map_err(|e| read_error(file, &e))?;
    install_package(&Package::read(&bytes, crypto)?, dirs, (crypto, trust), Installed::Explicit)
}

/// Unpacks `package` into the first of `dirs`, its signature checked as
/// `trust` says, marked as `how` it is installed: its directory. One
/// installed already of the same digests is kept.
///
/// # Errors
///
/// SQLSTATE 38T08 if its signature does not verify or one required is
/// not there; 38T07 if the version is installed already with other
/// files; 38T05 if `dirs` names none; the manifest's errors; of writing.
pub fn install_package(
    package: &Package,
    dirs: &Directories,
    (crypto, trust): (&dyn PackageCrypto, &Trust),
    how: Installed,
) -> Result<PathBuf> {
    let signature = package.signature.as_ref().map(|(k, s)| (k, s));
    trust.check(crypto, signature, &package.signed_message()?, "the package")?;
    let manifest = package.manifest()?;
    let version = Version::parse(&manifest.version)?;
    check_path(&manifest.name)?;
    let root =
        dirs.paths().first().ok_or_else(|| crate::plugin::errors::unavailable("PLUGINS.DIRECTORIES names no directory to install into"))?;
    let target = root.join(&manifest.name).join(version.to_string());
    let digests = digests_text(package);
    if target.exists() {
        let theirs = std::fs::read_to_string(target.join(DIGESTS_FILE)).unwrap_or_default();
        if theirs != digests {
            return Err(changed(&format!("{} is installed already, with other files", target.display())));
        }
        if how == Installed::Explicit {
            mark(&target, how)?;
        }
        return Ok(target);
    }
    let staging = root.join(&manifest.name).join(format!(".staging-{}-{}", std::process::id(), STAGED.fetch_add(1, Ordering::Relaxed)));
    let written =
        unpack(package, &staging, &digests, how).and_then(|()| std::fs::rename(&staging, &target).map_err(|e| write_error(&target, &e)));
    if written.is_err() {
        // What was written in part is taken away; a failure to is not the error told.
        let _ = std::fs::remove_dir_all(&staging);
    }
    written.map(|()| target)
}

/// Writes `package`'s entries, its digests and its state into `into`.
fn unpack(package: &Package, into: &Path, digests: &str, how: Installed) -> Result<()> {
    std::fs::create_dir_all(into).map_err(|e| write_error(into, &e))?;
    for entry in &package.entries {
        write_entry(into, entry)?;
    }
    let file = into.join(DIGESTS_FILE);
    std::fs::write(&file, digests).map_err(|e| write_error(&file, &e))?;
    mark(into, how)
}

/// Writes one entry under `into`, an executable given the permissions of
/// the running program's own file, which the standard library carries
/// across without knowing the platform's.
fn write_entry(into: &Path, entry: &Entry) -> Result<()> {
    let path = place(into, &entry.path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| write_error(parent, &e))?;
    }
    std::fs::write(&path, &entry.content).map_err(|e| write_error(&path, &e))?;
    if entry.kind == EntryKind::Executable {
        let own = std::env::current_exe().and_then(std::fs::metadata).map_err(|e| write_error(&path, &e))?;
        std::fs::set_permissions(&path, own.permissions()).map_err(|e| write_error(&path, &e))?;
    }
    Ok(())
}

/// Records how the package in `dir` is installed.
///
/// # Errors
///
/// Of writing its state file.
pub fn mark(dir: &Path, how: Installed) -> Result<()> {
    let file = dir.join(STATE_FILE);
    let text = match how {
        Installed::Explicit => EXPLICIT,
        Installed::Dependency => DEPENDENCY,
    };
    std::fs::write(&file, format!("{text}\n")).map_err(|e| write_error(&file, &e))
}

/// How the package in `dir` is installed: by name unless its state file
/// says it is a dependency.
pub fn installed_how(dir: &Path) -> Installed {
    let text = std::fs::read_to_string(dir.join(STATE_FILE)).unwrap_or_default();
    if text.trim() == DEPENDENCY { Installed::Dependency } else { Installed::Explicit }
}

/// The digests file of `package`.
fn digests_text(package: &Package) -> String {
    let mut text = format!("{DIGESTS_HEADER} {FORMAT}\n");
    if let Some((key, signature)) = &package.signature {
        let _ = writeln!(text, "signature {} {}", hex(key), hex(signature));
    }
    for e in &package.entries {
        let _ = writeln!(text, "{} {}", hex(&e.digest), e.path);
    }
    text
}

/// What a digests file says: the signature, and each entry's digest and
/// path.
struct Digests {
    signature: Option<(PublicKey, Signature)>,
    entries: Vec<(Digest, String)>,
}

/// The digests file `text`.
fn read_digests(text: &str) -> Result<Digests> {
    let mut lines = text.lines();
    if lines.next() != Some(&format!("{DIGESTS_HEADER} {FORMAT}")) {
        return Err(malformed(&format!("its {DIGESTS_FILE} is not of format {FORMAT}")));
    }
    let mut d = Digests { signature: None, entries: Vec::new() };
    for line in lines.filter(|l| !l.trim().is_empty()) {
        let bad = || malformed(&format!("its {DIGESTS_FILE} has the line {line}"));
        let (first, rest) = line.split_once(' ').ok_or_else(bad)?;
        if first == "signature" {
            let (key, signature) = rest.split_once(' ').ok_or_else(bad)?;
            d.signature = Some((unhex_array(key).ok_or_else(bad)?, unhex_array(signature).ok_or_else(bad)?));
        } else {
            check_path(rest)?;
            d.entries.push((unhex_array(first).ok_or_else(bad)?, rest.to_owned()));
        }
    }
    Ok(d)
}

/// Checks the plug-in installed in `dir`: each file its digests file names
/// is what its digest says, and its signature as `trust` says. A
/// directory no package installed has no digests file, and passes unless
/// a signature is required.
///
/// The signer, in hexadecimal, if a key trusted signed it.
///
/// # Errors
///
/// SQLSTATE 38T07 if a file changed or is missing; 38T08 if its
/// signature does not verify, or none by a key trusted is there and one is
/// required.
pub fn verify_installed(dir: &Path, crypto: &dyn PackageCrypto, trust: &Trust) -> Result<Option<String>> {
    let file = dir.join(DIGESTS_FILE);
    let Ok(text) = std::fs::read_to_string(&file) else {
        if trust.require_signed {
            return Err(untrusted(&format!("{} was installed from no signed package, and PLUGINS.REQUIRE_SIGNED is ON", dir.display())));
        }
        return Ok(None);
    };
    let digests = read_digests(&text)?;
    let mut manifest = None;
    for (digest, path) in &digests.entries {
        let content = std::fs::read(place(dir, path)).map_err(|e| changed(&format!("{path} cannot be read: {e}")))?;
        if crypto.sha256(&content) != *digest {
            return Err(changed(&format!("{path} in {} is not what it was when installed", dir.display())));
        }
        if path == MANIFEST_FILE {
            manifest = Some(content);
        }
    }
    let manifest = manifest.ok_or_else(|| malformed(&format!("its {DIGESTS_FILE} names no {MANIFEST_FILE}")))?;
    let listed: Vec<(&str, &Digest)> = digests.entries.iter().map(|(d, p)| (p.as_str(), d)).collect();
    let signature = digests.signature.as_ref().map(|(k, s)| (k, s));
    trust.check(crypto, signature, &signed_message(&manifest, &listed), &format!("the package in {}", dir.display()))
}

/// The scripts of SQL that upgrade `manifest`'s package from the version
/// `from`: of each UPGRADE FROM whose range holds `from`, in the order of
/// the lowest version each range names, then as written.
///
/// # Errors
///
/// SQLCODE -301 if a range is not one.
pub fn upgrade_scripts(manifest: &Manifest, from: &Version) -> Result<Vec<String>> {
    let mut scripts = Vec::new();
    for (written, script) in &manifest.upgrades {
        let range = Range::parse(written)?;
        if range.matches(from) {
            let lowest = range.alternatives.iter().flatten().map(|c| c.version.clone()).min().unwrap_or_else(|| Version::new(0, 0, 0));
            scripts.push((lowest, script.clone()));
        }
    }
    scripts.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(scripts.into_iter().map(|(_, s)| s).collect())
}

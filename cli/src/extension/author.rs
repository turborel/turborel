//! \[EXTENSION\] The commands of a package's author (design document
//! §13.5, RULES.md D264, D266): keygen writes a signing key, and pack
//! builds the package of a plug-in's project, signed if asked, which the
//! author hands to operators as a file.

use std::io::Write;
use std::path::{Path, PathBuf};

use turborel::plugin::package::crypto::Signer;
use turborel::plugin::package::project::{Project, pack_project};
use turborel::plugin::package::semver::Version;
use turborel::plugin::package::{License, PACKAGE_EXTENSION, hex};
use turborel_client::packages::{Crypto, SigningKey};

use super::args::Invocation;
use super::say;

/// Where cargo puts what it builds, under a project or CARGO_TARGET_DIR.
const TARGET: &str = "target";
/// The profile `cargo build --release` builds.
const RELEASE: &str = "release";

/// The key of `--key`, if named.
fn key(inv: &Invocation) -> Result<Option<SigningKey>, String> {
    inv.key.as_deref().map(|k| SigningKey::read(&inv.context.at(k)).map_err(|e| e.to_string())).transpose()
}

/// Writes `bytes` to `path`, as typed.
fn write(inv: &Invocation, path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(inv.context.at(path), bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Builds the package of the project in `dir`, `.` if none, signed by
/// `--key` if named, written to `--out` or into the project under its
/// file's name. It warns of a package that declares no license.
///
/// # Errors
///
/// Why it cannot, for the user: a LICENSE FILE or another file the
/// manifest names that is not there among them.
pub(super) fn pack(inv: &Invocation, dir: Option<&str>, out: &mut dyn Write) -> Result<(), String> {
    let typed = PathBuf::from(dir.unwrap_or("."));
    let project = inv.context.at(&typed);
    let built = inv.target_dir.as_ref().map_or_else(|| project.join(TARGET), |t| inv.context.at(t)).join(RELEASE);
    let mut package =
        pack_project(Project { dir: &project, built: &built }, &Crypto).map_err(|e| format!("{}: {e}", typed.display()))?;
    let key = key(inv)?;
    if let Some(key) = &key {
        package.sign(key).map_err(|e| e.to_string())?;
    }
    let manifest = package.manifest().map_err(|e| e.to_string())?;
    let version = Version::parse(&manifest.version).map_err(|e| e.to_string())?;
    let bytes = package.write();
    let file = format!("{}-{version}.{PACKAGE_EXTENSION}", manifest.name.to_ascii_lowercase());
    let to = inv.out.clone().unwrap_or_else(|| typed.join(file));
    write(inv, &to, &bytes)?;
    let license = License::of(&manifest);
    if !license.declared() {
        say(out, "warning: the manifest declares no LICENSE; operators will be told the package has none")?;
    }
    let signed = key.map_or_else(|| "unsigned".to_owned(), |k| format!("signed by {}", hex(&k.public_key())));
    say(out, &format!("packed {} {version} into {}, {signed}, {}", manifest.name, to.display(), license.told()))
}

/// Writes a new signing key to `--out`, and its public key to the same
/// name with `.pub` after.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn keygen(inv: &Invocation, out: &mut dyn Write) -> Result<(), String> {
    let to = inv.out.clone().ok_or("name the key's file with --out")?;
    if inv.context.at(&to).exists() {
        return Err(format!("{} is there already; a key is never written over", to.display()));
    }
    let key = SigningKey::generate().map_err(|e| e.to_string())?;
    write(inv, &to, key.file_text().as_bytes())?;
    let public = hex(&key.public_key());
    let mut pub_file = to.clone().into_os_string();
    pub_file.push(".pub");
    write(inv, Path::new(&pub_file), format!("{public}\n").as_bytes())?;
    say(out, &format!("wrote the key {} and its public key {public}", to.display()))
}

//! \[EXTENSION\] The commands of a package's author (design document
//! §13.5, §13.6): keygen writes a signing key, pack builds and signs a
//! package of a plug-in's directory, and publish writes it into a
//! registry directory with the index rewritten and signed, so a static
//! registry is that directory served over HTTPS.

use std::io::Write;
use std::path::{Path, PathBuf};

use turborel::plugin::package::archive::pack as pack_directory;
use turborel::plugin::package::crypto::Signer;
use turborel::plugin::package::index::{INDEX_FILE, Index, Listed, SIGNATURE_FILE, now};
use turborel::plugin::package::semver::Version;
use turborel::plugin::package::{PACKAGE_EXTENSION, PackageCrypto, hex};
use turborel_client::packages::{Crypto, SigningKey};

use super::args::Invocation;
use super::say;

/// Seconds in a day.
const DAY: u64 = 86_400;

/// The key of `--key`, if named.
fn key(inv: &Invocation) -> Result<Option<SigningKey>, String> {
    inv.key.as_deref().map(|k| SigningKey::read(k).map_err(|e| e.to_string())).transpose()
}

/// Writes `bytes` to `path`.
fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// The package of the plug-in in `dir`, signed by `key` if given: its
/// bytes, name, version, and its manifest's requirements.
fn build(dir: &str, key: Option<&SigningKey>) -> Result<(Vec<u8>, Listed), String> {
    let mut package = pack_directory(Path::new(dir), &Crypto).map_err(|e| format!("{dir}: {e}"))?;
    if let Some(key) = key {
        package.sign(key).map_err(|e| e.to_string())?;
    }
    let manifest = package.manifest().map_err(|e| e.to_string())?;
    let version = Version::parse(&manifest.version).map_err(|e| e.to_string())?;
    let bytes = package.write();
    let listed = Listed {
        file: format!("{}-{version}.{PACKAGE_EXTENSION}", manifest.name.to_ascii_lowercase()),
        name: manifest.name,
        version,
        digest: Crypto.sha256(&bytes),
        signer: key.map(|k| hex(&k.public_key())),
        requires: manifest.requires,
        turborel: manifest.turborel,
    };
    Ok((bytes, listed))
}

/// Builds the package of `dir`, signed by `--key` if named, written to
/// `--output` or to its file's name here.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn pack(inv: &Invocation, dir: &str, out: &mut dyn Write) -> Result<(), String> {
    let (bytes, listed) = build(dir, key(inv)?.as_ref())?;
    let to = inv.output.clone().unwrap_or_else(|| PathBuf::from(&listed.file));
    write(&to, &bytes)?;
    let signed = listed.signer.as_deref().map_or_else(|| "unsigned".to_owned(), |s| format!("signed by {s}"));
    say(out, &format!("packed {} {} into {}, {signed}, sha256 {}", listed.name, listed.version, to.display(), hex(&listed.digest)))
}

/// Packs `dir`, signed by `--key`, into `--registry-dir`, and writes its
/// index again, the version replaced if listed, signed by the same key and
/// trusted for `--expires-days`.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn publish(inv: &Invocation, dir: &str, out: &mut dyn Write) -> Result<(), String> {
    let key = key(inv)?.ok_or("publish signs the package and the index: name the key with --key")?;
    let registry = inv.registry_dir.clone().ok_or("name the registry directory with --registry-dir")?;
    std::fs::create_dir_all(&registry).map_err(|e| format!("cannot make {}: {e}", registry.display()))?;
    let (bytes, listed) = build(dir, Some(&key))?;
    let mut index = match std::fs::read_to_string(registry.join(INDEX_FILE)) {
        Ok(text) => Index::parse(&text).map_err(|e| format!("{}: {e}", registry.display()))?,
        Err(_) => Index { expires: 0, packages: Vec::new() },
    };
    index.packages.retain(|p| p.name != listed.name || p.version != listed.version);
    write(&registry.join(&listed.file), &bytes)?;
    say(out, &format!("published {} {} as {}", listed.name, listed.version, listed.file))?;
    index.packages.push(listed);
    index.packages.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    index.expires = now().saturating_add(inv.expires_days.saturating_mul(DAY));
    let text = index.write();
    write(&registry.join(INDEX_FILE), text.as_bytes())?;
    write(&registry.join(SIGNATURE_FILE), Index::sign(&text, &key).as_bytes())?;
    say(
        out,
        &format!(
            "signed the index of {}, trusted until {}",
            registry.display(),
            turborel::plugin::package::index::format_time(index.expires)
        ),
    )
}

/// Writes a new signing key to `--output`, and its public key to the
/// same name with `.pub` after.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn keygen(inv: &Invocation, out: &mut dyn Write) -> Result<(), String> {
    let to = inv.output.clone().ok_or("name the key's file with --output")?;
    if to.exists() {
        return Err(format!("{} is there already; a key is never written over", to.display()));
    }
    let key = SigningKey::generate().map_err(|e| e.to_string())?;
    write(&to, key.file_text().as_bytes())?;
    let public = hex(&key.public_key());
    let mut pub_file = to.clone().into_os_string();
    pub_file.push(".pub");
    write(Path::new(&pub_file), format!("{public}\n").as_bytes())?;
    say(out, &format!("wrote the key {} and its public key {public}", to.display()))
}

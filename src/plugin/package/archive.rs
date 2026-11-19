//! \[EXTENSION\] The package file, format 1 (design document §13.1,
//! VERSIONING.md), turborel's own archive, little-endian as `bytes.rs`
//! writes:
//!
//! ```text
//! file      := "TBPKG\0" | format u8 (1) | count u32 | entry...
//!              | 0 u8, unsigned, or 1 u8 | public key bytes (32) | signature bytes (64)
//! entry     := path str | kind u8 | SHA-256 digest bytes (32) | content bytes
//! signed    := "TBPKG-SIGNED\0" | the manifest's content | each entry's path str and digest bytes
//! ```
//!
//! The signature is over `signed`, so it covers the manifest and, through
//! the digests, every file. Paths are relative, `/` between their parts,
//! and stay inside the package.

use std::path::{Path, PathBuf};

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::plugin::MANIFEST_FILE;
use crate::plugin::manifest::{Manifest, inside};

use super::crypto::{Digest, PackageCrypto, PublicKey, Signature, Signer};
use super::errors::{changed, malformed};
use super::{DIGESTS_FILE, FORMAT, STATE_FILE};

/// What a package file begins with.
pub const MAGIC: &[u8; 6] = b"TBPKG\0";
/// What the message signed begins with.
pub const SIGNED_MAGIC: &[u8; 13] = b"TBPKG-SIGNED\0";
/// The most entries a package holds.
pub const MAX_ENTRIES: u32 = 100_000;

/// What an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// The manifest.
    Manifest,
    /// An executable its manifest names.
    Executable,
    /// A document, under `doc/`.
    Doc,
    /// Any other file.
    Data,
    /// A script of UPGRADE FROM.
    Upgrade,
}

impl EntryKind {
    /// Its code in the file.
    pub fn code(self) -> u8 {
        match self {
            EntryKind::Manifest => 0,
            EntryKind::Executable => 1,
            EntryKind::Doc => 2,
            EntryKind::Data => 3,
            EntryKind::Upgrade => 4,
        }
    }

    /// The kind of code `code`, if any.
    pub fn of_code(code: u8) -> Option<EntryKind> {
        [EntryKind::Manifest, EntryKind::Executable, EntryKind::Doc, EntryKind::Data, EntryKind::Upgrade]
            .into_iter()
            .find(|k| k.code() == code)
    }
}

/// One file of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Its path, `/` between its parts.
    pub path: String,
    /// What it is.
    pub kind: EntryKind,
    /// The SHA-256 digest of its content.
    pub digest: Digest,
    /// Its content.
    pub content: Vec<u8>,
}

/// A package: its entries, the manifest first, and its signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// The entries.
    pub entries: Vec<Entry>,
    /// The public key that signed it, and the signature.
    pub signature: Option<(PublicKey, Signature)>,
}

/// That `path` is relative, inside the package, and written alike on
/// every platform.
///
/// # Errors
///
/// SQLSTATE 38T06 if not.
pub fn check_path(path: &str) -> Result<()> {
    inside(path, "the package entry")?;
    if path.contains(['\\', ':']) || path.split('/').any(|p| p.is_empty() || p == ".") {
        return Err(crate::plugin::errors::invalid_manifest(&format!("the package entry {path} is not a plain relative path")));
    }
    Ok(())
}

impl Package {
    /// The manifest's entry.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T07 if it has none.
    pub fn manifest_entry(&self) -> Result<&Entry> {
        self.entries.iter().find(|e| e.kind == EntryKind::Manifest).ok_or_else(|| malformed(&format!("it holds no {MANIFEST_FILE}")))
    }

    /// Its manifest.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T07 if it has none; the manifest's errors.
    pub fn manifest(&self) -> Result<Manifest> {
        let entry = self.manifest_entry()?;
        let text = std::str::from_utf8(&entry.content).map_err(|_| malformed("its manifest is not UTF-8"))?;
        Manifest::parse(text)
    }

    /// The message its signature is of.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T07 if it has no manifest.
    pub fn signed_message(&self) -> Result<Vec<u8>> {
        let listed: Vec<(&str, &Digest)> = self.entries.iter().map(|e| (e.path.as_str(), &e.digest)).collect();
        Ok(signed_message(&self.manifest_entry()?.content, &listed))
    }

    /// Signs it with `signer`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T07 if it has no manifest.
    pub fn sign(&mut self, signer: &dyn Signer) -> Result<()> {
        let message = self.signed_message()?;
        self.signature = Some((signer.public_key(), signer.sign(&message)));
        Ok(())
    }

    /// Its file's bytes.
    pub fn write(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.put_u8(FORMAT);
        out.put_u32(u32::try_from(self.entries.len()).unwrap_or(u32::MAX));
        for e in &self.entries {
            out.put_str(&e.path);
            out.put_u8(e.kind.code());
            out.put_bytes(&e.digest);
            out.put_bytes(&e.content);
        }
        match &self.signature {
            Some((key, signature)) => {
                out.put_u8(1);
                out.put_bytes(key);
                out.put_bytes(signature);
            }
            None => out.put_u8(0),
        }
        out
    }

    /// The package `bytes` hold, each entry's digest checked.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T07 if it is malformed, of another format, or an entry's
    /// content is not what its digest says; 38T06 of a path that leaves it.
    pub fn read(bytes: &[u8], crypto: &dyn PackageCrypto) -> Result<Package> {
        let body = bytes.strip_prefix(MAGIC.as_slice()).ok_or_else(|| malformed("it is not a package file"))?;
        let mut r = Reader::new(body, "package");
        let format = r.u8()?;
        if format != FORMAT {
            return Err(malformed(&format!("it is of format {format}, and this release reads format {FORMAT}")));
        }
        let count = r.u32()?;
        if count > MAX_ENTRIES {
            return Err(malformed(&format!("it holds {count} entries, more than {MAX_ENTRIES}")));
        }
        let mut entries = Vec::new();
        for _ in 0..count {
            entries.push(read_entry(&mut r, crypto)?);
        }
        let signature = match r.u8()? {
            0 => None,
            1 => Some((array(r.bytes()?)?, array(r.bytes()?)?)),
            other => return Err(malformed(&format!("its signature's tag is {other}"))),
        };
        r.finish()?;
        let package = Package { entries, signature };
        check_entries(&package)?;
        Ok(package)
    }
}

/// `bytes`, which must be `N` long.
fn array<const N: usize>(bytes: &[u8]) -> Result<[u8; N]> {
    bytes.try_into().map_err(|_| malformed(&format!("a key or signature is {} bytes, not {N}", bytes.len())))
}

/// One entry, its digest checked.
fn read_entry(r: &mut Reader<'_>, crypto: &dyn PackageCrypto) -> Result<Entry> {
    let path = r.str()?;
    check_path(&path)?;
    let code = r.u8()?;
    let kind = EntryKind::of_code(code).ok_or_else(|| malformed(&format!("the entry {path} is of kind {code}")))?;
    let digest: Digest = array(r.bytes()?)?;
    let content = r.bytes()?.to_vec();
    if crypto.sha256(&content) != digest {
        return Err(changed(&format!("the entry {path} is not what its digest says")));
    }
    Ok(Entry { path, kind, digest, content })
}

/// That a package holds one manifest, at its name, and no path twice.
fn check_entries(package: &Package) -> Result<()> {
    let manifests: Vec<&Entry> = package.entries.iter().filter(|e| e.kind == EntryKind::Manifest).collect();
    let one = manifests.len() == 1 && manifests.iter().all(|m| m.path == MANIFEST_FILE);
    if !one {
        return Err(malformed(&format!("it must hold one manifest, {MANIFEST_FILE}")));
    }
    let mut paths: Vec<&str> = package.entries.iter().map(|e| e.path.as_str()).collect();
    paths.sort_unstable();
    if paths.windows(2).any(|w| w.first() == w.get(1)) {
        return Err(malformed("it holds a path twice"));
    }
    Ok(())
}

/// The message a package's signature is of: its manifest and each
/// entry's path and digest, in order.
pub fn signed_message(manifest: &[u8], entries: &[(&str, &Digest)]) -> Vec<u8> {
    let mut message = SIGNED_MAGIC.to_vec();
    message.put_bytes(manifest);
    for (path, digest) in entries {
        message.put_str(path);
        message.put_bytes(digest.as_slice());
    }
    message
}

/// The package of the plug-in in `directory`: every file under it, the
/// manifest, the executables and scripts it names and those under `doc/`
/// told apart; the files an install writes passed over.
///
/// # Errors
///
/// The manifest's errors; SQLSTATE 38T06 of an executable or script it
/// names that is not there; the errors of reading the files.
pub fn pack(directory: &Path, crypto: &dyn PackageCrypto) -> Result<Package> {
    let manifest = Manifest::parse(&read_text(&directory.join(MANIFEST_FILE))?)?;
    super::semver::Version::parse(&manifest.version)?;
    let mut paths = Vec::new();
    files_under(directory, "", &mut paths)?;
    paths.retain(|p| p != DIGESTS_FILE && p != STATE_FILE);
    paths.sort();
    let executables: Vec<&str> = manifest.executables.iter().map(|(_, p)| p.trim_start_matches("./")).collect();
    let scripts: Vec<&str> = manifest.upgrades.iter().map(|(_, s)| s.trim_start_matches("./")).collect();
    for named in executables.iter().chain(&scripts) {
        if !paths.iter().any(|p| p == named) {
            return Err(crate::plugin::errors::invalid_manifest(&format!("{named}, which it names, is not in {}", directory.display())));
        }
    }
    let mut entries = Vec::new();
    for path in paths {
        let kind = kind_of(&path, &executables, &scripts);
        let content = std::fs::read(directory.join(&path)).map_err(|e| read_error(&directory.join(&path), &e))?;
        entries.push(Entry { digest: crypto.sha256(&content), path, kind, content });
    }
    entries.sort_by_key(|e| e.kind != EntryKind::Manifest);
    Ok(Package { entries, signature: None })
}

/// The kind of the entry `path`, of a manifest naming `executables` and
/// `scripts`.
fn kind_of(path: &str, executables: &[&str], scripts: &[&str]) -> EntryKind {
    if path == MANIFEST_FILE {
        EntryKind::Manifest
    } else if executables.contains(&path) {
        EntryKind::Executable
    } else if scripts.contains(&path) {
        EntryKind::Upgrade
    } else if path.starts_with("doc/") {
        EntryKind::Doc
    } else {
        EntryKind::Data
    }
}

/// The text of `file`.
fn read_text(file: &Path) -> Result<String> {
    std::fs::read_to_string(file).map_err(|e| read_error(file, &e))
}

/// The error of `file` that cannot be read.
pub fn read_error(file: &Path, e: &std::io::Error) -> crate::error::SqlError {
    crate::error::SqlError::new(crate::error::Code::Io, format!("[EXTENSION] {} cannot be read: {e}", file.display()))
}

/// Each file under `directory`, by its path from the package's root,
/// `prefix` the path of `directory`; names beginning `.` passed over.
fn files_under(directory: &Path, prefix: &str, paths: &mut Vec<String>) -> Result<()> {
    let listed = std::fs::read_dir(directory).map_err(|e| read_error(directory, &e))?;
    for entry in listed {
        let entry = entry.map_err(|e| read_error(directory, &e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let kind = entry.file_type().map_err(|e| read_error(&entry.path(), &e))?;
        if kind.is_dir() {
            files_under(&entry.path(), &path, paths)?;
        } else {
            check_path(&path)?;
            paths.push(path);
        }
    }
    Ok(())
}

/// Where the entry `path` goes under `root`.
pub fn place(root: &Path, path: &str) -> PathBuf {
    path.split('/').fold(root.to_path_buf(), |at, part| at.join(part))
}

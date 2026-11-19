//! \[EXTENSION\] The package files given to `install` (RULES.md D261):
//! each file named, and each `.tbpkg` file of a directory named, read
//! and its entries' digests checked, and the versions they hold as a
//! source resolution chooses from, beside the packages installed.

use std::path::{Path, PathBuf};

use turborel::plugin::manifest::Manifest;
use turborel::plugin::package::archive::Package;
use turborel::plugin::package::crypto::Digest;
use turborel::plugin::package::resolve::{Candidate, Origin, Source, candidate_of};
use turborel::plugin::package::semver::{Range, Version};
use turborel::plugin::package::{PACKAGE_EXTENSION, PackageCrypto};
use turborel_client::packages::Crypto;

use super::Context;

/// A package file given.
#[derive(Debug, Clone)]
pub(super) struct Given {
    /// The file, as given.
    pub(super) file: PathBuf,
    /// Its package.
    pub(super) package: Package,
    /// Its manifest.
    pub(super) manifest: Manifest,
    /// Its version.
    pub(super) version: Version,
    /// The SHA-256 digest of the file.
    pub(super) digest: Digest,
}

/// The package files `words` name: each a file, or a directory whose
/// `.tbpkg` files are taken, in the order of their names. One file twice
/// is taken once.
///
/// # Errors
///
/// If one cannot be read or is not a package (SQLSTATE 38T07), a
/// directory holds none, or two files hold the same version of a package
/// with other contents.
pub(super) fn given(cx: &Context, words: &[String]) -> Result<Vec<Given>, String> {
    let mut all: Vec<Given> = Vec::new();
    for word in words {
        for file in files_of(cx, Path::new(word))? {
            let g = read(cx, &file)?;
            if let Some(other) = all.iter().find(|o| o.manifest.name == g.manifest.name && o.version == g.version) {
                if other.digest != g.digest {
                    let what = format!("{} {}", g.manifest.name, g.version);
                    return Err(format!("{} and {} both hold {what}, with other contents", other.file.display(), g.file.display()));
                }
                continue;
            }
            all.push(g);
        }
    }
    Ok(all)
}

/// The files `typed` names: itself, or the package files of the
/// directory it is.
fn files_of(cx: &Context, typed: &Path) -> Result<Vec<PathBuf>, String> {
    let at = cx.at(typed);
    if !at.is_dir() {
        return Ok(vec![typed.to_path_buf()]);
    }
    let listed = std::fs::read_dir(&at).map_err(|e| format!("cannot read {}: {e}", typed.display()))?;
    let mut names: Vec<String> = listed
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| Path::new(n).extension().is_some_and(|x| x == PACKAGE_EXTENSION))
        .collect();
    if names.is_empty() {
        return Err(format!("{} holds no .{PACKAGE_EXTENSION} file", typed.display()));
    }
    names.sort();
    Ok(names.into_iter().map(|n| typed.join(n)).collect())
}

/// The package file `file`, read and checked.
fn read(cx: &Context, file: &Path) -> Result<Given, String> {
    let bytes = std::fs::read(cx.at(file)).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let package = Package::read(&bytes, &Crypto).map_err(|e| format!("{}: {e}", file.display()))?;
    let manifest = package.manifest().map_err(|e| format!("{}: {e}", file.display()))?;
    let version = Version::parse(&manifest.version).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(Given { file: file.to_path_buf(), digest: Crypto.sha256(&bytes), package, manifest, version })
}

/// What the files given ask for by name: each package no other file
/// given requires, with the caret range of its newest version given; if
/// each is required by another, as a cycle is, every one.
///
/// # Errors
///
/// If a version written is not one.
pub(super) fn roots(given: &[Given]) -> Result<Vec<(String, Range)>, String> {
    let required = |name: &str| given.iter().any(|g| g.manifest.requires.iter().any(|(n, _)| n == name));
    let mut names: Vec<&str> = given.iter().map(|g| g.manifest.name.as_str()).filter(|n| !required(n)).collect();
    if names.is_empty() {
        names = given.iter().map(|g| g.manifest.name.as_str()).collect();
    }
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let newest = given.iter().filter(|g| g.manifest.name == name).map(|g| &g.version).max();
            let range = newest.map_or_else(|| "*".to_owned(), |v| format!("^{v}"));
            Ok((name.to_owned(), Range::parse(&range).map_err(|e| e.to_string())?))
        })
        .collect()
}

/// The versions the files given hold, as a source resolution chooses
/// from.
pub(super) struct GivenSource<'a>(pub(super) &'a [Given]);

impl Source for GivenSource<'_> {
    fn versions(&self, name: &str) -> turborel::error::Result<Vec<Candidate>> {
        self.0
            .iter()
            .filter(|g| g.manifest.name == name)
            .map(|g| candidate_of(&g.manifest, g.version.clone(), Origin::File { file: g.file.clone(), digest: g.digest }))
            .collect()
    }
}

//! \[EXTENSION\] What the plug-in directories hold (design document
//! §13.3): each package as `<directory>/<name>/<version>/`, and the lock
//! of the first, `turborel-extensions.lock`.

use std::path::{Path, PathBuf};

use turborel::plugin::MANIFEST_FILE;
use turborel::plugin::manifest::Manifest;
use turborel::plugin::package::License;
use turborel::plugin::package::install::{Installed, installed_how};
use turborel::plugin::package::lock::{LOCK_FILE, Lock};
use turborel::plugin::package::semver::{Range, Version};

use super::Context;

/// A package installed.
#[derive(Debug, Clone)]
pub(super) struct Present {
    /// Its name.
    pub(super) name: String,
    /// Its version.
    pub(super) version: Version,
    /// Its directory.
    pub(super) dir: PathBuf,
    /// Its manifest.
    pub(super) manifest: Manifest,
    /// How it was installed.
    pub(super) how: Installed,
}

impl Present {
    /// What it requires.
    pub(super) fn requires(&self) -> Vec<(String, Range)> {
        self.manifest.requires.iter().filter_map(|(n, r)| Some((n.clone(), Range::parse(r).ok()?))).collect()
    }
}

/// The lock file of `cx`.
fn lock_file(cx: &Context) -> Result<PathBuf, String> {
    Ok(cx.home()?.join(LOCK_FILE))
}

/// The lock of `cx`, empty if it has none yet.
pub(super) fn read_lock(cx: &Context) -> Result<Lock, String> {
    let file = lock_file(cx)?;
    match std::fs::read_to_string(&file) {
        Ok(text) => Lock::parse(&text).map_err(|e| format!("{}: {e}", file.display())),
        Err(_) => Ok(Lock::default()),
    }
}

/// Writes `lock` as `cx`'s.
pub(super) fn write_lock(cx: &Context, lock: &Lock) -> Result<(), String> {
    let file = lock_file(cx)?;
    std::fs::write(&file, lock.write()).map_err(|e| format!("cannot write {}: {e}", file.display()))
}

/// Every package installed in the directories of `cx`, by name then
/// version.
pub(super) fn all(cx: &Context) -> Result<Vec<Present>, String> {
    let mut found = Vec::new();
    for root in cx.dirs().paths() {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        for entry in entries.flatten() {
            let package = entry.file_name().to_string_lossy().into_owned();
            if package.starts_with('.') || !entry.path().is_dir() {
                continue;
            }
            let Ok(versions) = std::fs::read_dir(entry.path()) else { continue };
            for version in versions.flatten() {
                if let Some(p) = present(&package, &version.path(), &version.file_name().to_string_lossy())? {
                    found.push(p);
                }
            }
        }
    }
    found.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(found)
}

/// The package in `dir`, of `name` and `written` version, if it is one.
fn present(name: &str, dir: &Path, written: &str) -> Result<Option<Present>, String> {
    let Ok(version) = Version::parse(written) else { return Ok(None) };
    let Ok(text) = std::fs::read_to_string(dir.join(MANIFEST_FILE)) else { return Ok(None) };
    let manifest = Manifest::parse(&text).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(Some(Present { name: name.to_owned(), version, dir: dir.to_path_buf(), manifest, how: installed_how(dir) }))
}

/// What the package in `dir` declares of its license: none if its manifest
/// cannot be read.
pub(super) fn license_of(dir: &Path) -> License {
    let text = std::fs::read_to_string(dir.join(MANIFEST_FILE)).unwrap_or_default();
    Manifest::parse(&text).map(|m| License::of(&m)).unwrap_or_default()
}

/// The package `name` of `version` installed, if it is.
pub(super) fn find(cx: &Context, name: &str, version: &Version) -> Result<Option<Present>, String> {
    Ok(all(cx)?.into_iter().find(|p| p.name == name && p.version == *version))
}

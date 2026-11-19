//! \[EXTENSION\] Packing a plug-in's project (RULES.md D264, D266): the
//! package of a directory holding a manifest, the program its build made,
//! and the files the manifest names, so its sources and its build's other
//! output stay out.
//!
//! The package holds the manifest; each executable it names, found at its
//! path in the project, else under the build's directory by its file's
//! name, as `cargo build --release` leaves it; each upgrade script; the
//! file of LICENSE FILE; and every file under `doc/` and `data/`. A file
//! the manifest names that is not there refuses the package, so a
//! package never lacks its license's text.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::plugin::MANIFEST_FILE;
use crate::plugin::errors::invalid_manifest;
use crate::plugin::manifest::Manifest;

use super::archive::{Entry, EntryKind, Package, check_path, place, read_error};
use super::crypto::PackageCrypto;
use super::semver::Version;

/// The directory of a project whose files go in whole, as documents.
pub const DOC_DIR: &str = "doc";
/// The directory of a project whose files go in whole, as data.
pub const DATA_DIR: &str = "data";
/// The deepest directories under `doc/` or `data/` are read (RULES.md
/// R-CF-6).
pub const MAX_DEPTH: usize = 16;

/// Where a project and its build are.
#[derive(Debug, Clone, Copy)]
pub struct Project<'a> {
    /// The project's directory, holding the manifest.
    pub dir: &'a Path,
    /// Where its build leaves its programs, as `target/release`.
    pub built: &'a Path,
}

/// A path the manifest names, as a package's entry.
fn named(path: &str) -> String {
    path.trim_start_matches("./").to_owned()
}

/// The package of `project`.
///
/// # Errors
///
/// The manifest's errors; SQLSTATE 38T06 of an executable, script or
/// LICENSE FILE it names that is not there; the errors of reading the
/// files.
pub fn pack_project(project: Project<'_>, crypto: &dyn PackageCrypto) -> Result<Package> {
    let manifest_file = project.dir.join(MANIFEST_FILE);
    let text = std::fs::read_to_string(&manifest_file).map_err(|e| read_error(&manifest_file, &e))?;
    let manifest = Manifest::parse(&text)?;
    Version::parse(&manifest.version)?;
    let mut files: Vec<(String, EntryKind, PathBuf)> = vec![(MANIFEST_FILE.to_owned(), EntryKind::Manifest, manifest_file)];
    for (_, path) in &manifest.executables {
        let path = named(path);
        files.push((path.clone(), EntryKind::Executable, executable(project, &path)?));
    }
    for (_, script) in &manifest.upgrades {
        files.push((named(script), EntryKind::Upgrade, present(project.dir, &named(script), "the upgrade script")?));
    }
    if let Some(license) = &manifest.license_file {
        files.push((named(license), EntryKind::Doc, present(project.dir, &named(license), "LICENSE FILE")?));
    }
    for (dir, kind) in [(DOC_DIR, EntryKind::Doc), (DATA_DIR, EntryKind::Data)] {
        let mut under = Vec::new();
        files_under(&project.dir.join(dir), (dir, 0), &mut under)?;
        under.sort();
        files.extend(under.into_iter().map(|p| (p.clone(), kind, place(project.dir, &p))));
    }
    let mut entries: Vec<Entry> = Vec::new();
    for (path, kind, source) in files {
        check_path(&path)?;
        if entries.iter().any(|e| e.path == path) {
            continue;
        }
        let content = std::fs::read(&source).map_err(|e| read_error(&source, &e))?;
        entries.push(Entry { digest: crypto.sha256(&content), path, kind, content });
    }
    Ok(Package { entries, signature: None })
}

/// The file of the executable `path`: at its path in the project, else
/// its file's name under the build's directory.
fn executable(project: Project<'_>, path: &str) -> Result<PathBuf> {
    let at = place(project.dir, path);
    if at.is_file() {
        return Ok(at);
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let built = project.built.join(name);
    if built.is_file() {
        return Ok(built);
    }
    Err(invalid_manifest(&format!(
        "the executable {path} is neither at {} nor at {}; build the program first",
        at.display(),
        built.display()
    )))
}

/// The file `path` of the project in `dir`, which must be there.
fn present(dir: &Path, path: &str, what: &str) -> Result<PathBuf> {
    let at = place(dir, path);
    if at.is_file() {
        return Ok(at);
    }
    Err(invalid_manifest(&format!("{what} {path}, which the manifest names, is not in {}", dir.display())))
}

/// Each file under `directory`, by its path from the project's root,
/// `prefix` the path of `directory`, `depth` below `doc/` or `data/`;
/// names beginning `.` passed over, and a directory not there holding
/// none.
fn files_under(directory: &Path, (prefix, depth): (&str, usize), paths: &mut Vec<String>) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(invalid_manifest(&format!("{} is deeper than {MAX_DEPTH} directories", directory.display())));
    }
    let Ok(listed) = std::fs::read_dir(directory) else { return Ok(()) };
    for entry in listed {
        let entry = entry.map_err(|e| read_error(directory, &e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = format!("{prefix}/{name}");
        let kind = entry.file_type().map_err(|e| read_error(&entry.path(), &e))?;
        if kind.is_dir() {
            files_under(&entry.path(), (&path, depth.saturating_add(1)), paths)?;
        } else {
            paths.push(path);
        }
    }
    Ok(())
}

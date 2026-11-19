//! \[EXTENSION\] Choosing the versions of a set of packages (design
//! document §13.2, RULES.md D171): each requirement, a name and a range,
//! met by the newest version of the name that keeps every other
//! requirement met, trying older ones where a newer leads to a conflict;
//! one version of each name; a version pinned taken alone; a package
//! whose TURBOREL range leaves out this release passed over. A set that
//! cannot be had, or whose requirements form a cycle, is refused with
//! SQLSTATE 38T09 naming why.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::plugin::MANIFEST_FILE;
use crate::plugin::limits::Directories;
use crate::plugin::manifest::Manifest;

use super::crypto::Digest;
use super::errors::conflict;
use super::semver::{Range, Version};

/// The most versions resolution tries before it gives up.
pub const MAX_STEPS: u32 = 100_000;

/// Where a version of a package is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Installed, in this directory.
    Installed(PathBuf),
    /// A package file given, not yet installed.
    File {
        /// The file.
        file: PathBuf,
        /// The SHA-256 digest of its bytes.
        digest: Digest,
    },
}

/// A version of a package that resolution may choose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The package's name.
    pub name: String,
    /// Its version.
    pub version: Version,
    /// The packages it requires, each with a range.
    pub requires: Vec<(String, Range)>,
    /// The releases of the database it runs on, if limited.
    pub turborel: Option<Range>,
    /// Where it is.
    pub origin: Origin,
}

/// Where versions are found.
pub trait Source {
    /// The versions of the package `name` there, in any order.
    ///
    /// # Errors
    ///
    /// Of reading them.
    fn versions(&self, name: &str) -> Result<Vec<Candidate>>;
}

/// What is to be resolved.
#[derive(Debug, Clone)]
pub struct Request {
    /// The packages asked for, each with a range.
    pub roots: Vec<(String, Range)>,
    /// The versions that must be kept, as one already running.
    pub pinned: Vec<(String, Version)>,
    /// This release of the database.
    pub turborel: Version,
}

/// This release of the database, as a version.
pub fn this_release() -> Version {
    Version::parse(crate::VERSION).unwrap_or_else(|_| Version::new(0, 0, 0))
}

/// A requirement waiting: a name, a range, and what required it.
#[derive(Debug, Clone)]
struct Need {
    name: String,
    range: Range,
    by: String,
}

/// What a resolution keeps as it goes.
struct Search<'a> {
    source: &'a dyn Source,
    request: &'a Request,
    found: HashMap<String, Vec<Candidate>>,
    steps: u32,
    /// Why the deepest path tried failed.
    why: Option<(usize, String)>,
}

/// The versions `request` asks for, of those `source` has: every package
/// of the set, each after those it requires.
///
/// # Errors
///
/// SQLSTATE 38T09 if no set meets every requirement, naming one that
/// cannot be met, or the requirements form a cycle; the source's errors.
pub fn resolve(source: &dyn Source, request: &Request) -> Result<Vec<Candidate>> {
    let pending: Vec<Need> =
        request.roots.iter().map(|(n, r)| Need { name: n.clone(), range: r.clone(), by: "the request".to_owned() }).collect();
    let mut search = Search { source, request, found: HashMap::new(), steps: 0, why: None };
    let mut chosen = Vec::new();
    if !search.solve(&mut chosen, &pending)? {
        let why = search.why.map_or_else(|| "no versions meet every requirement".to_owned(), |(_, w)| w);
        return Err(conflict(&why));
    }
    ordered(&chosen)
}

impl Search<'_> {
    /// Meets `pending` with what is `chosen`: whether it can.
    fn solve(&mut self, chosen: &mut Vec<Candidate>, pending: &[Need]) -> Result<bool> {
        let Some((need, rest)) = pending.split_first() else { return Ok(true) };
        if let Some(held) = chosen.iter().find(|c| c.name == need.name) {
            if need.range.matches(&held.version) {
                return self.solve(chosen, rest);
            }
            let why = format!("{} requires {} {}, and {} {} is required too", need.by, need.name, need.range, held.name, held.version);
            self.failed(chosen.len(), why);
            return Ok(false);
        }
        let candidates = self.candidates(need)?;
        if candidates.is_empty() {
            let why = format!(
                "{} requires {} {}, and no version of it found meets that; add a package file of {} of a version in {}",
                need.by, need.name, need.range, need.name, need.range
            );
            self.failed(chosen.len(), why);
            return Ok(false);
        }
        for c in candidates {
            self.steps = self.steps.saturating_add(1);
            if self.steps > MAX_STEPS {
                return Err(conflict(&format!("more than {MAX_STEPS} versions were tried")));
            }
            let by = format!("{} {}", c.name, c.version);
            let mut next: Vec<Need> = rest.to_vec();
            next.extend(c.requires.iter().map(|(n, r)| Need { name: n.clone(), range: r.clone(), by: by.clone() }));
            chosen.push(c);
            if self.solve(chosen, &next)? {
                return Ok(true);
            }
            chosen.pop();
        }
        Ok(false)
    }

    /// The versions that may meet `need`, newest first.
    fn candidates(&mut self, need: &Need) -> Result<Vec<Candidate>> {
        if !self.found.contains_key(&need.name) {
            let found = self.source.versions(&need.name)?;
            self.found.insert(need.name.clone(), found);
        }
        let pinned = self.request.pinned.iter().find(|(n, _)| *n == need.name).map(|(_, v)| v);
        let release = &self.request.turborel;
        let mut all: Vec<Candidate> = self
            .found
            .get(&need.name)
            .into_iter()
            .flatten()
            .filter(|c| need.range.matches(&c.version))
            .filter(|c| pinned.is_none_or(|p| *p == c.version))
            .filter(|c| c.turborel.as_ref().is_none_or(|r| r.matches(release)))
            .cloned()
            .collect();
        all.sort_by(|a, b| b.version.cmp(&a.version));
        all.dedup_by(|a, b| a.version == b.version);
        Ok(all)
    }

    /// Keeps `why` if no deeper path has failed.
    fn failed(&mut self, depth: usize, why: String) {
        if self.why.as_ref().is_none_or(|(d, _)| depth >= *d) {
            self.why = Some((depth, why));
        }
    }
}

/// `chosen`, each after those it requires.
///
/// # Errors
///
/// SQLSTATE 38T09 if their requirements form a cycle, naming it.
pub fn ordered(chosen: &[Candidate]) -> Result<Vec<Candidate>> {
    let mut done: Vec<Candidate> = Vec::new();
    let mut path: Vec<String> = Vec::new();
    for at in 0..chosen.len() {
        visit(chosen, at, &mut path, &mut done)?;
    }
    Ok(done)
}

/// Puts the package at `at` after those it requires, `path` those being
/// visited.
fn visit(all: &[Candidate], at: usize, path: &mut Vec<String>, done: &mut Vec<Candidate>) -> Result<()> {
    let Some(c) = all.get(at) else { return Ok(()) };
    if done.iter().any(|d| d.name == c.name) {
        return Ok(());
    }
    if let Some(start) = path.iter().position(|p| *p == c.name) {
        let mut cycle: Vec<String> = path.get(start..).unwrap_or_default().to_vec();
        cycle.push(c.name.clone());
        return Err(conflict(&format!("their requirements form a cycle: {}", cycle.join(" requires "))));
    }
    path.push(c.name.clone());
    for (name, _) in &c.requires {
        if let Some(next) = all.iter().position(|d| d.name == *name) {
            visit(all, next, path, done)?;
        }
    }
    path.pop();
    done.push(c.clone());
    Ok(())
}

/// The packages installed under the directories of PLUGINS.DIRECTORIES,
/// each as `<directory>/<name>/<version>/`.
#[derive(Debug, Clone)]
pub struct InstalledSource<'a>(pub &'a Directories);

impl Source for InstalledSource<'_> {
    fn versions(&self, name: &str) -> Result<Vec<Candidate>> {
        let mut all = Vec::new();
        for root in self.0.paths() {
            let Ok(listed) = std::fs::read_dir(root.join(name)) else { continue };
            for entry in listed.flatten() {
                let dir = entry.path();
                let written = entry.file_name().to_string_lossy().into_owned();
                let Ok(version) = Version::parse(&written) else { continue };
                if let Some(c) = installed_candidate(&dir, name, &version)? {
                    all.push(c);
                }
            }
        }
        Ok(all)
    }
}

/// The candidate installed in `dir`, if its manifest is there and names
/// `name` and `version`.
///
/// # Errors
///
/// The manifest's errors.
pub fn installed_candidate(dir: &Path, name: &str, version: &Version) -> Result<Option<Candidate>> {
    let Ok(text) = std::fs::read_to_string(dir.join(MANIFEST_FILE)) else { return Ok(None) };
    let manifest = Manifest::parse(&text)?;
    if manifest.name != name || Version::parse(&manifest.version).ok().as_ref() != Some(version) {
        return Ok(None);
    }
    Ok(Some(candidate_of(&manifest, version.clone(), Origin::Installed(dir.to_path_buf()))?))
}

/// The candidate `manifest` makes, of `version`, at `origin`.
///
/// # Errors
///
/// SQLCODE -301 if a range it writes is not one.
pub fn candidate_of(manifest: &Manifest, version: Version, origin: Origin) -> Result<Candidate> {
    let requires = manifest.requires.iter().map(|(n, r)| Ok((n.clone(), Range::parse(r)?))).collect::<Result<_>>()?;
    let turborel = manifest.turborel.as_deref().map(Range::parse).transpose()?;
    Ok(Candidate { name: manifest.name.clone(), version, requires, turborel, origin })
}

/// The packages installed under `dirs` that `name` and `range` (any
/// version but a prerelease if none) need, `pinned` kept: each name,
/// version and directory, each after those it requires.
///
/// # Errors
///
/// SQLSTATE 38T09 as `resolve`'s; the manifests' errors.
pub fn resolve_installed(
    dirs: &Directories,
    name: &str,
    range: Option<&Range>,
    pinned: &[(String, Version)],
) -> Result<Vec<(String, Version, PathBuf)>> {
    let request = Request {
        roots: vec![(name.to_owned(), range.cloned().unwrap_or_else(Range::any))],
        pinned: pinned.to_vec(),
        turborel: this_release(),
    };
    let chosen = resolve(&InstalledSource(dirs), &request)?;
    Ok(chosen
        .into_iter()
        .filter_map(|c| match c.origin {
            Origin::Installed(dir) => Some((c.name, c.version, dir)),
            Origin::File { .. } => None,
        })
        .collect())
}

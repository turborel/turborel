//! \[EXTENSION\] The lock file (design document §13.6), format 1: the
//! exact set of packages resolved, so one installed again, on this server
//! or another, is the same to the byte:
//!
//! ```text
//! turborel-lock 1
//! <name> <version> <SHA-256 hex> <registry or -> explicit <range>
//! <name> <version> <SHA-256 hex> <registry or -> dependency
//! ```
//!
//! A package installed by name keeps the range it was asked for, which
//! an upgrade honours.

use std::fmt::Write as _;

use crate::error::Result;

use super::crypto::Digest;
use super::errors::malformed;
use super::semver::{Range, Version};
use super::{FORMAT, hex, unhex_array};

/// The first word of a lock file.
const HEADER: &str = "turborel-lock";
/// The name of the lock file in a plug-in directory.
pub const LOCK_FILE: &str = "turborel-extensions.lock";

/// One package locked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locked {
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: Version,
    /// The SHA-256 digest of its package file.
    pub digest: Digest,
    /// The registry it came from, if one.
    pub registry: Option<String>,
    /// The range it was asked for by name, if it was.
    pub wanted: Option<Range>,
}

/// A lock: the packages, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lock {
    /// The packages.
    pub packages: Vec<Locked>,
}

impl Lock {
    /// The lock `text` is.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T07 if it is malformed.
    pub fn parse(text: &str) -> Result<Lock> {
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        if lines.next() != Some(&format!("{HEADER} {FORMAT}")) {
            return Err(malformed(&format!("the lock file is not of format {FORMAT}")));
        }
        let packages = lines.map(locked).collect::<Result<_>>()?;
        Ok(Lock { packages })
    }

    /// Its text.
    pub fn write(&self) -> String {
        let mut text = format!("{HEADER} {FORMAT}\n");
        for p in &self.packages {
            let registry = p.registry.as_deref().unwrap_or("-");
            let how = p.wanted.as_ref().map_or_else(|| "dependency".to_owned(), |r| format!("explicit {r}"));
            let _ = writeln!(text, "{} {} {} {registry} {how}", p.name, p.version, hex(&p.digest));
        }
        text
    }

    /// The package `name` locked, if it is.
    pub fn get(&self, name: &str) -> Option<&Locked> {
        self.packages.iter().find(|p| p.name == name)
    }

    /// Locks `p`, in place of any of its name.
    pub fn put(&mut self, p: Locked) {
        match self.packages.iter_mut().find(|q| q.name == p.name) {
            Some(q) => *q = p,
            None => self.packages.push(p),
        }
        self.packages.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

/// One line of a lock file.
fn locked(line: &str) -> Result<Locked> {
    let bad = || malformed(&format!("the lock file has the line {line}"));
    let mut words = line.splitn(6, ' ');
    let mut next = || words.next().ok_or_else(bad);
    let (name, version, digest, registry, how) = (next()?, next()?, next()?, next()?, next()?);
    let wanted = match how {
        "explicit" => Some(Range::parse(next().unwrap_or("*"))?),
        "dependency" => None,
        _ => return Err(bad()),
    };
    Ok(Locked {
        name: name.to_owned(),
        version: Version::parse(version)?,
        digest: unhex_array(digest).ok_or_else(bad)?,
        registry: (registry != "-").then(|| registry.to_owned()),
        wanted,
    })
}

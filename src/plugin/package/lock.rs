//! \[EXTENSION\] The lock file (design document §13.6, RULES.md D262),
//! format 1: the exact set of packages resolved, so one installed again,
//! on this server or another, from the same package files, is the same
//! to the byte:
//!
//! ```text
//! turborel-lock 1
//! <name> <version> <SHA-256 hex> explicit <range>
//! <name> <version> <SHA-256 hex> dependency
//! license <SPDX expression>       (of the package before, if declared)
//! license file <path>             (of the package before, if declared)
//! ```
//!
//! The digest is of the package file. A package installed by name keeps
//! the range it was asked for, which a later install honours. Each
//! license line is optional, so a package that declares none, or a lock
//! written before licenses were kept, reads.

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
/// What begins a line of a license.
const LICENSE: &str = "license ";
/// What begins a license line's file after `LICENSE`.
const FILE: &str = "file ";
/// The word of a package installed by name.
const EXPLICIT: &str = "explicit";
/// The word of a package installed as another's dependency.
const DEPENDENCY: &str = "dependency";

/// One package locked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locked {
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: Version,
    /// The SHA-256 digest of its package file.
    pub digest: Digest,
    /// The range it was asked for by name, if it was.
    pub wanted: Option<Range>,
    /// Its SPDX license expression, if it declares one.
    pub license: Option<String>,
    /// The path of its license's text in the package, if it declares one.
    pub license_file: Option<String>,
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
        let mut packages: Vec<Locked> = Vec::new();
        for line in lines {
            let Some(license) = line.strip_prefix(LICENSE) else {
                packages.push(locked(line)?);
                continue;
            };
            let last = packages.last_mut().ok_or_else(|| malformed(&format!("the lock file begins with the line {line}")))?;
            match license.strip_prefix(FILE) {
                Some(path) => last.license_file = Some(path.trim().to_owned()),
                None => last.license = Some(license.trim().to_owned()),
            }
        }
        Ok(Lock { packages })
    }

    /// Its text.
    pub fn write(&self) -> String {
        let mut text = format!("{HEADER} {FORMAT}\n");
        for p in &self.packages {
            let how = p.wanted.as_ref().map_or_else(|| DEPENDENCY.to_owned(), |r| format!("{EXPLICIT} {r}"));
            let _ = writeln!(text, "{} {} {} {how}", p.name, p.version, hex(&p.digest));
            if let Some(expression) = &p.license {
                let _ = writeln!(text, "{LICENSE}{expression}");
            }
            if let Some(path) = &p.license_file {
                let _ = writeln!(text, "{LICENSE}{FILE}{path}");
            }
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

/// One package's line of a lock file.
fn locked(line: &str) -> Result<Locked> {
    let bad = || malformed(&format!("the lock file has the line {line}"));
    let mut words = line.splitn(5, ' ');
    let mut next = || words.next().ok_or_else(bad);
    let (name, version, digest, how) = (next()?, next()?, next()?, next()?);
    let wanted = match how {
        EXPLICIT => Some(Range::parse(next().unwrap_or("*"))?),
        DEPENDENCY => None,
        _ => return Err(bad()),
    };
    Ok(Locked {
        name: name.to_owned(),
        version: Version::parse(version)?,
        digest: unhex_array(digest).ok_or_else(bad)?,
        wanted,
        license: None,
        license_file: None,
    })
}

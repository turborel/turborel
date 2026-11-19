//! \[EXTENSION\] Semantic versions and their ranges (design document
//! §13.2, RULES.md D171).
//!
//! A version is `MAJOR.MINOR.PATCH`, a prerelease after `-` and build
//! metadata after `+`, which is passed over. A range is alternatives
//! joined by `||`, each comparators joined by spaces or commas, all of
//! which a version meets:
//!
//! * `=1.2.3` or `1.2.3`, exactly; `>`, `>=`, `<`, `<=` before a version.
//! * `^1.2.3`, the same major version (of 0, the same minor; of 0.0, the
//!   same patch); `~1.2.3`, the same minor version.
//! * `1.x`, `1.2.x`, `1`, `1.2`, `*` or `x`, any of the versions written.
//! * `1.2.3 - 2.0.0`, from the first to the second, both included.
//!
//! A prerelease meets a range only if one of the comparators of the
//! alternative names a prerelease of its `MAJOR.MINOR.PATCH`, so a
//! prerelease is taken only when asked for.

use std::cmp::Ordering;
use std::fmt;

use crate::error::{Code, Result, SqlError};

/// One identifier of a prerelease.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Identifier {
    /// Digits alone, compared as a number.
    Number(u64),
    /// Anything else, compared as text, after every number.
    Text(String),
}

impl Ord for Identifier {
    fn cmp(&self, other: &Identifier) -> Ordering {
        match (self, other) {
            (Identifier::Number(a), Identifier::Number(b)) => a.cmp(b),
            (Identifier::Number(_), Identifier::Text(_)) => Ordering::Less,
            (Identifier::Text(_), Identifier::Number(_)) => Ordering::Greater,
            (Identifier::Text(a), Identifier::Text(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for Identifier {
    fn partial_cmp(&self, other: &Identifier) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A semantic version.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version {
    /// MAJOR.
    pub major: u64,
    /// MINOR.
    pub minor: u64,
    /// PATCH.
    pub patch: u64,
    /// The prerelease's identifiers: none of a release.
    pub pre: Vec<Identifier>,
}

impl Ord for Version {
    fn cmp(&self, other: &Version) -> Ordering {
        let core = (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch));
        core.then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => self.pre.cmp(&other.pre),
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        let pre: Vec<String> = self
            .pre
            .iter()
            .map(|i| match i {
                Identifier::Number(n) => n.to_string(),
                Identifier::Text(t) => t.clone(),
            })
            .collect();
        if !pre.is_empty() {
            write!(f, "-{}", pre.join("."))?;
        }
        Ok(())
    }
}

/// A version or range written wrong, `why`.
fn invalid(text: &str, why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] {text} is not a semantic version or range: {why}"))
}

/// A number of a version: digits, with no leading zero.
fn number(text: &str, part: &str) -> Result<u64> {
    let digits = !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
    if !digits || (part.len() > 1 && part.starts_with('0')) {
        return Err(invalid(text, &format!("{part} is not a number")));
    }
    part.parse().map_err(|_| invalid(text, &format!("{part} is too large")))
}

impl Version {
    /// The release `major.minor.patch`.
    pub fn new(major: u64, minor: u64, patch: u64) -> Version {
        Version { major, minor, patch, pre: Vec::new() }
    }

    /// The version `text` writes.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if it is not a semantic version.
    pub fn parse(text: &str) -> Result<Version> {
        let trimmed = text.trim();
        let without_build = trimmed.split_once('+').map_or(trimmed, |(v, _)| v);
        let (core, pre) = without_build.split_once('-').map_or((without_build, None), |(c, p)| (c, Some(p)));
        let parts: Vec<&str> = core.split('.').collect();
        let [major, minor, patch] = parts.as_slice() else { return Err(invalid(text, "it needs MAJOR.MINOR.PATCH")) };
        let mut v = Version::new(number(text, major)?, number(text, minor)?, number(text, patch)?);
        if let Some(pre) = pre {
            v.pre = pre.split('.').map(|i| prerelease(text, i)).collect::<Result<_>>()?;
        }
        Ok(v)
    }

    /// Whether it is a prerelease.
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    /// The release it precedes or is.
    fn release(&self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }
}

/// One identifier of a prerelease.
fn prerelease(text: &str, part: &str) -> Result<Identifier> {
    let allowed = !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !allowed {
        return Err(invalid(text, &format!("the prerelease {part} is not alphanumeric")));
    }
    if part.chars().all(|c| c.is_ascii_digit()) {
        return Ok(Identifier::Number(number(text, part)?));
    }
    Ok(Identifier::Text(part.to_owned()))
}

/// How a comparator compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `=`.
    Equal,
    /// `>`.
    Greater,
    /// `>=`.
    AtLeast,
    /// `<`.
    Less,
    /// `<=`.
    AtMost,
}

/// One comparator of a range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparator {
    /// How.
    pub op: Op,
    /// Against what.
    pub version: Version,
}

impl Comparator {
    /// Whether `v` meets it.
    fn holds(&self, v: &Version) -> bool {
        let order = v.cmp(&self.version);
        match self.op {
            Op::Equal => order == Ordering::Equal,
            Op::Greater => order == Ordering::Greater,
            Op::AtLeast => order != Ordering::Less,
            Op::Less => order == Ordering::Less,
            Op::AtMost => order != Ordering::Greater,
        }
    }
}

/// A range of versions: alternatives, each comparators all met.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    /// Its text.
    pub text: String,
    /// The alternatives.
    pub alternatives: Vec<Vec<Comparator>>,
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl Range {
    /// Any version but a prerelease: `*`.
    pub fn any() -> Range {
        Range { text: "*".to_owned(), alternatives: vec![Vec::new()] }
    }

    /// Exactly `v`.
    pub fn exactly(v: &Version) -> Range {
        Range { text: format!("={v}"), alternatives: vec![vec![Comparator { op: Op::Equal, version: v.clone() }]] }
    }

    /// The range `text` writes.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if it is not a range.
    pub fn parse(text: &str) -> Result<Range> {
        let alternatives = text.split("||").map(|a| alternative(text, a)).collect::<Result<_>>()?;
        Ok(Range { text: text.trim().to_owned(), alternatives })
    }

    /// Whether `v` is in it.
    pub fn matches(&self, v: &Version) -> bool {
        self.alternatives.iter().any(|all| {
            let asked = !v.is_prerelease() || all.iter().any(|c| c.version.is_prerelease() && c.version.release() == v.release());
            asked && all.iter().all(|c| c.holds(v))
        })
    }
}

/// One alternative's comparators.
fn alternative(text: &str, written: &str) -> Result<Vec<Comparator>> {
    let words: Vec<String> = written.replace(',', " ").split_whitespace().map(str::to_owned).collect();
    if let [from, dash, to] = words.as_slice()
        && dash == "-"
    {
        let mut all = partial(text, from)?.lower(Op::AtLeast);
        all.extend(partial(text, to)?.upper_inclusive());
        return Ok(all);
    }
    let mut all = Vec::new();
    let mut pending_op: Option<String> = None;
    for word in words {
        // An operator written apart from its version, as `>= 1.2.3`.
        if word.chars().all(|c| matches!(c, '<' | '>' | '=' | '^' | '~')) {
            pending_op = Some(word);
            continue;
        }
        let joined = pending_op.take().map_or(word.clone(), |op| format!("{op}{word}"));
        all.extend(comparators(text, &joined)?);
    }
    if pending_op.is_some() {
        return Err(invalid(text, "an operator has no version"));
    }
    Ok(all)
}

/// A version written in part: missing or wildcard parts none.
#[derive(Debug, Clone)]
struct Partial {
    major: Option<u64>,
    minor: Option<u64>,
    patch: Option<u64>,
    pre: Vec<Identifier>,
}

/// The partial version `word` writes.
fn partial(text: &str, word: &str) -> Result<Partial> {
    let word = word.strip_prefix('v').unwrap_or(word);
    let without_build = word.split_once('+').map_or(word, |(v, _)| v);
    let (core, pre) = without_build.split_once('-').map_or((without_build, None), |(c, p)| (c, Some(p)));
    let mut parts = core.split('.').map(|p| if matches!(p, "x" | "X" | "*") { Ok(None) } else { number(text, p).map(Some) });
    let major = parts.next().unwrap_or(Ok(None))?;
    let minor = parts.next().unwrap_or(Ok(None))?;
    let patch = parts.next().unwrap_or(Ok(None))?;
    if parts.next().is_some() {
        return Err(invalid(text, &format!("{word} has more than three parts")));
    }
    let wild_then_set = (major.is_none() && (minor.is_some() || patch.is_some())) || (minor.is_none() && patch.is_some());
    if wild_then_set {
        return Err(invalid(text, &format!("{word} has a number after a wildcard")));
    }
    let pre = match (pre, patch) {
        (Some(p), Some(_)) => p.split('.').map(|i| prerelease(text, i)).collect::<Result<_>>()?,
        (Some(_), None) => return Err(invalid(text, &format!("{word} has a prerelease of no whole version"))),
        (None, _) => Vec::new(),
    };
    Ok(Partial { major, minor, patch, pre })
}

impl Partial {
    /// The lowest version it writes.
    fn floor(&self) -> Version {
        let n = |p: Option<u64>| p.unwrap_or(0);
        Version { major: n(self.major), minor: n(self.minor), patch: n(self.patch), pre: self.pre.clone() }
    }

    /// The first release after every version it writes, if any is.
    fn ceiling(&self) -> Option<Version> {
        match (self.major, self.minor, self.patch) {
            (None, _, _) => None,
            (Some(m), None, _) => Some(Version::new(m.checked_add(1)?, 0, 0)),
            (Some(m), Some(n), None) => Some(Version::new(m, n.checked_add(1)?, 0)),
            (Some(m), Some(n), Some(p)) => Some(Version::new(m, n, p.checked_add(1)?)),
        }
    }

    /// `op` its floor, unless it is a wildcard alone.
    fn lower(&self, op: Op) -> Vec<Comparator> {
        if self.major.is_none() {
            return Vec::new();
        }
        vec![Comparator { op, version: self.floor() }]
    }

    /// At most every version it writes.
    fn upper_inclusive(&self) -> Vec<Comparator> {
        if self.patch.is_some() {
            return vec![Comparator { op: Op::AtMost, version: self.floor() }];
        }
        self.ceiling().map(|c| Comparator { op: Op::Less, version: c }).into_iter().collect()
    }

    /// Below every version it writes.
    fn below(&self) -> Vec<Comparator> {
        if self.major.is_none() {
            // Below any version: none is.
            return vec![Comparator { op: Op::Less, version: Version::new(0, 0, 0) }];
        }
        vec![Comparator { op: Op::Less, version: self.floor() }]
    }

    /// Every version it writes: `1.2` from 1.2.0 to before 1.3.0.
    fn exact(&self) -> Vec<Comparator> {
        if self.patch.is_some() {
            return vec![Comparator { op: Op::Equal, version: self.floor() }];
        }
        let mut all = self.lower(Op::AtLeast);
        all.extend(self.ceiling().map(|c| Comparator { op: Op::Less, version: c }));
        all
    }

    /// Above every version it writes.
    fn above(&self) -> Vec<Comparator> {
        if self.patch.is_some() {
            return vec![Comparator { op: Op::Greater, version: self.floor() }];
        }
        match self.ceiling() {
            Some(c) => vec![Comparator { op: Op::AtLeast, version: c }],
            None => vec![Comparator { op: Op::Less, version: Version::new(0, 0, 0) }],
        }
    }

    /// `^`: the same leftmost part that is not zero.
    fn caret(&self) -> Vec<Comparator> {
        let mut all = self.lower(Op::AtLeast);
        let ceiling = match (self.major, self.minor, self.patch) {
            (Some(0), Some(0), Some(p)) => p.checked_add(1).map(|p| Version::new(0, 0, p)),
            (Some(0), Some(n), _) => n.checked_add(1).map(|n| Version::new(0, n, 0)),
            (Some(m), _, _) => m.checked_add(1).map(|m| Version::new(m, 0, 0)),
            (None, _, _) => None,
        };
        all.extend(ceiling.map(|c| Comparator { op: Op::Less, version: c }));
        all
    }

    /// `~`: the same minor version, or major if no minor is written.
    fn tilde(&self) -> Vec<Comparator> {
        let mut all = self.lower(Op::AtLeast);
        let ceiling = match (self.major, self.minor) {
            (Some(m), Some(n)) => n.checked_add(1).map(|n| Version::new(m, n, 0)),
            (Some(m), None) => m.checked_add(1).map(|m| Version::new(m, 0, 0)),
            (None, _) => None,
        };
        all.extend(ceiling.map(|c| Comparator { op: Op::Less, version: c }));
        all
    }
}

/// What makes the comparators of an operator from its version.
type Make = fn(&Partial) -> Vec<Comparator>;

/// The comparators one word writes, its operator joined to it.
fn comparators(text: &str, word: &str) -> Result<Vec<Comparator>> {
    let ops: [(&str, Make); 7] = [
        (">=", |p| p.lower(Op::AtLeast)),
        ("<=", Partial::upper_inclusive),
        (">", Partial::above),
        ("<", Partial::below),
        ("=", Partial::exact),
        ("^", Partial::caret),
        ("~", Partial::tilde),
    ];
    for (op, make) in ops {
        if let Some(rest) = word.strip_prefix(op) {
            return Ok(make(&partial(text, rest)?));
        }
    }
    Ok(partial(text, word)?.exact())
}

#[cfg(test)]
mod tests;

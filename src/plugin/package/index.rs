//! \[EXTENSION\] A registry's index (design document §13.3, §13.6), format
//! 1, a file `index` of lines, signed as a whole by `index.sig`:
//!
//! ```text
//! turborel-registry 1
//! expires <YYYY-MM-DDTHH:MM:SSZ>
//! package <name> <version> <file> <SHA-256 hex> <signer hex or ->
//! requires <name> <range>          (of the package before, each)
//! turborel <range>                 (of the package before, if limited)
//! ```
//!
//! `index.sig` is `<public key hex> <signature hex>`, the Ed25519
//! signature of the index's bytes. An index is trusted only once its
//! signature verifies by a key of PLUGINS.TRUSTED_KEYS and it has not
//! expired, so one stale or replayed is refused; only then are the
//! digests it lists trusted. Registries are tried in the order
//! PLUGINS.REGISTRIES names them, and PLUGINS.PINS pins a name to one.

use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::Result;

use super::crypto::{Digest, PackageCrypto, Signer};
use super::errors::{malformed, untrusted};
use super::resolve::{Candidate, Origin, Source};
use super::semver::{Range, Version};
use super::trust::Trust;
use super::{FORMAT, hex, unhex_array};

/// The name of a registry's index file.
pub const INDEX_FILE: &str = "index";
/// The name of its signature file.
pub const SIGNATURE_FILE: &str = "index.sig";
/// The first word of an index.
const HEADER: &str = "turborel-registry";
/// What separates the entries of PLUGINS.REGISTRIES and PLUGINS.PINS.
pub const LIST_SEPARATOR: char = ';';
/// Seconds in a day.
const DAY: u64 = 86_400;

/// A package an index lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: Version,
    /// Its file, relative to the registry.
    pub file: String,
    /// The SHA-256 digest of its file.
    pub digest: Digest,
    /// The key that signed it, in hexadecimal, if any.
    pub signer: Option<String>,
    /// What it requires: names and ranges, as written.
    pub requires: Vec<(String, String)>,
    /// The releases of the database it runs on, as written.
    pub turborel: Option<String>,
}

/// An index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    /// When it expires, in seconds since 1970 UTC.
    pub expires: u64,
    /// The packages.
    pub packages: Vec<Listed>,
}

impl Index {
    /// The index `text` is, unchecked.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T07 if it is malformed.
    pub fn parse(text: &str) -> Result<Index> {
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        if lines.next() != Some(&format!("{HEADER} {FORMAT}")) {
            return Err(malformed(&format!("the registry's index is not of format {FORMAT}")));
        }
        let mut index = Index { expires: 0, packages: Vec::new() };
        for line in lines {
            index.line(line)?;
        }
        Ok(index)
    }

    /// Takes one line after the header.
    fn line(&mut self, line: &str) -> Result<()> {
        let bad = || malformed(&format!("the registry's index has the line {line}"));
        let (word, rest) = line.split_once(' ').ok_or_else(bad)?;
        match word {
            "expires" => self.expires = parse_time(rest).ok_or_else(bad)?,
            "package" => {
                let fields: Vec<&str> = rest.split_whitespace().collect();
                let [name, version, file, digest, signer] = fields.as_slice() else { return Err(bad()) };
                super::archive::check_path(file)?;
                let signer = (*signer != "-").then(|| (*signer).to_owned());
                let (version, digest) = (Version::parse(version)?, unhex_array(digest).ok_or_else(bad)?);
                let listed = Listed {
                    name: (*name).to_owned(),
                    version,
                    file: (*file).to_owned(),
                    digest,
                    signer,
                    requires: Vec::new(),
                    turborel: None,
                };
                self.packages.push(listed);
            }
            "requires" => {
                let (name, range) = rest.split_once(' ').ok_or_else(bad)?;
                Range::parse(range)?;
                self.packages.last_mut().ok_or_else(bad)?.requires.push((name.to_owned(), range.trim().to_owned()));
            }
            "turborel" => {
                Range::parse(rest)?;
                self.packages.last_mut().ok_or_else(bad)?.turborel = Some(rest.trim().to_owned());
            }
            _ => return Err(bad()),
        }
        Ok(())
    }

    /// Its text.
    pub fn write(&self) -> String {
        let mut text = format!("{HEADER} {FORMAT}\nexpires {}\n", format_time(self.expires));
        for p in &self.packages {
            let signer = p.signer.as_deref().unwrap_or("-");
            let _ = writeln!(text, "package {} {} {} {} {signer}", p.name, p.version, p.file, hex(&p.digest));
            for (name, range) in &p.requires {
                let _ = writeln!(text, "requires {name} {range}");
            }
            if let Some(range) = &p.turborel {
                let _ = writeln!(text, "turborel {range}");
            }
        }
        text
    }

    /// The index `text` is, once `signature`, the text of its signature
    /// file, verifies by a key `trust` holds and it has not expired at
    /// `now`, in seconds since 1970 UTC.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T08 if its signature is missing, does not verify or is
    /// by a key not trusted, whether or not PLUGINS.REQUIRE_SIGNED is ON,
    /// or it has expired; 38T07 if it is malformed.
    pub fn verified(text: &[u8], signature: &str, (crypto, trust): (&dyn PackageCrypto, &Trust), now: u64) -> Result<Index> {
        let (key, signed) = signature.trim().split_once(' ').ok_or_else(|| untrusted("the registry's index has no signature"))?;
        let key = unhex_array(key).ok_or_else(|| untrusted("the registry's index is signed by no key"))?;
        let signed = unhex_array(signed).ok_or_else(|| untrusted("the registry's index has no signature"))?;
        if !trust.trusts(&key) {
            return Err(untrusted(&format!("the registry's index is signed by {}, a key not in PLUGINS.TRUSTED_KEYS", hex(&key))));
        }
        if !crypto.verify(&key, text, &signed) {
            return Err(untrusted("the signature of the registry's index does not verify"));
        }
        let index = Index::parse(std::str::from_utf8(text).map_err(|_| malformed("the registry's index is not UTF-8"))?)?;
        if index.expires <= now {
            return Err(untrusted(&format!(
                "the registry's index expired at {}; the registry must publish a fresh one",
                format_time(index.expires)
            )));
        }
        Ok(index)
    }

    /// The text of the signature file of `text`, signed by `signer`.
    pub fn sign(text: &str, signer: &dyn Signer) -> String {
        format!("{} {}\n", hex(&signer.public_key()), hex(&signer.sign(text.as_bytes())))
    }

    /// The versions of `name` it lists, as from `registry`.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if a range it lists is not one.
    pub fn candidates(&self, registry: &str, name: &str) -> Result<Vec<Candidate>> {
        self.packages.iter().filter(|p| p.name == name).map(|p| candidate(registry, p)).collect()
    }
}

/// The candidate `p`, listed by `registry`.
fn candidate(registry: &str, p: &Listed) -> Result<Candidate> {
    let requires = p.requires.iter().map(|(n, r)| Ok((n.clone(), Range::parse(r)?))).collect::<Result<_>>()?;
    let turborel = p.turborel.as_deref().map(Range::parse).transpose()?;
    let origin = Origin::Registry { registry: registry.to_owned(), file: p.file.clone(), digest: p.digest, signer: p.signer.clone() };
    Ok(Candidate { name: p.name.clone(), version: p.version.clone(), requires, turborel, origin })
}

/// The seconds since 1970 UTC now.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The registries of PLUGINS.REGISTRIES, in order, and the pins of
/// PLUGINS.PINS, each `name=URL`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registries {
    /// The registries' URLs, the first tried first.
    pub urls: Vec<String>,
    /// The names pinned, each to a registry.
    pub pins: Vec<(String, String)>,
}

impl Registries {
    /// What `registries` and `pins` say.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if a pin is not `name=URL`.
    pub fn parse(registries: &str, pins: &str) -> Result<Registries> {
        let split = |text: &str| -> Vec<String> {
            text.split(LIST_SEPARATOR).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
        };
        let pins = split(pins)
            .into_iter()
            .map(|p| {
                p.split_once('=').map(|(n, u)| (n.trim().to_owned(), u.trim().to_owned())).ok_or_else(|| {
                    crate::error::SqlError::new(crate::error::Code::Data, format!("[EXTENSION] the pin {p} is not name=URL (PLUGINS.PINS)"))
                })
            })
            .collect::<Result<_>>()?;
        Ok(Registries { urls: split(registries), pins })
    }

    /// The registry `name` is pinned to, if any.
    pub fn pinned(&self, name: &str) -> Option<&str> {
        self.pins.iter().find(|(n, _)| n == name).map(|(_, u)| u.as_str())
    }
}

/// The indexes of the registries, each with its URL, in priority order,
/// as a source of versions.
#[derive(Debug, Clone)]
pub struct IndexSource {
    /// The indexes, the first of the registry tried first.
    pub indexes: Vec<(String, Index)>,
    /// The pins.
    pub pins: Vec<(String, String)>,
}

/// `url` with no `/` at its end, as compared.
fn bare(url: &str) -> &str {
    url.trim_end_matches('/')
}

impl Source for IndexSource {
    fn versions(&self, name: &str) -> Result<Vec<Candidate>> {
        let pin = self.pins.iter().find(|(n, _)| n == name).map(|(_, u)| bare(u));
        for (url, index) in &self.indexes {
            if pin.is_some_and(|p| p != bare(url)) {
                continue;
            }
            let found = index.candidates(url, name)?;
            if !found.is_empty() {
                return Ok(found);
            }
        }
        Ok(Vec::new())
    }
}

/// `a / b`, rounded down; 0 of a zero `b`.
fn div(a: u64, b: u64) -> u64 {
    a.checked_div(b).unwrap_or(0)
}

/// `a % b`; 0 of a zero `b`.
fn rem(a: u64, b: u64) -> u64 {
    a.checked_rem(b).unwrap_or(0)
}

/// The days from 1970-01-01 to the date `y`-`m`-`d` of the proleptic
/// Gregorian calendar, if it is on or after it.
fn days_from_civil(y: u64, m: u64, d: u64) -> Option<u64> {
    let y = if m <= 2 { y.checked_sub(1)? } else { y };
    let era = div(y, 400);
    let yoe = y.checked_sub(era.checked_mul(400)?)?;
    let shifted = if m > 2 { m.checked_sub(3)? } else { m.checked_add(9)? };
    let doy = div(153_u64.checked_mul(shifted)?.checked_add(2)?, 5).checked_add(d)?.checked_sub(1)?;
    let doe = yoe.checked_mul(365)?.checked_add(div(yoe, 4))?.checked_sub(div(yoe, 100))?.checked_add(doy)?;
    era.checked_mul(146_097)?.checked_add(doe)?.checked_sub(719_468)
}

/// The seconds since 1970 of `text`, `YYYY-MM-DDTHH:MM:SSZ`, if it is
/// one.
pub fn parse_time(text: &str) -> Option<u64> {
    let text = text.trim().strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let numbers = |s: &str, sep: char| -> Option<Vec<u64>> { s.split(sep).map(|n| n.parse().ok()).collect() };
    let (date, time) = (numbers(date, '-')?, numbers(time, ':')?);
    let ([year, month, day], [hour, minute, second]) = (<[u64; 3]>::try_from(date).ok()?, <[u64; 3]>::try_from(time).ok()?);
    let valid = (1..=12).contains(&month) && (1..=31).contains(&day) && hour < 24 && minute < 60 && second < 61;
    if !valid {
        return None;
    }
    let clock = hour.checked_mul(3600)?.checked_add(minute.checked_mul(60)?)?.checked_add(second)?;
    days_from_civil(year, month, day)?.checked_mul(DAY)?.checked_add(clock)
}

/// `seconds` since 1970 as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_time(seconds: u64) -> String {
    let (days, rest) = (div(seconds, DAY), rem(seconds, DAY));
    let z = days.saturating_add(719_468);
    let era = div(z, 146_097);
    let doe = z.saturating_sub(era.saturating_mul(146_097));
    let yoe = div(doe.saturating_sub(div(doe, 1460)).saturating_add(div(doe, 36_524)).saturating_sub(div(doe, 146_096)), 365);
    let doy = doe.saturating_sub(yoe.saturating_mul(365).saturating_add(div(yoe, 4)).saturating_sub(div(yoe, 100)));
    let mp = div(doy.saturating_mul(5).saturating_add(2), 153);
    let d = doy.saturating_sub(div(mp.saturating_mul(153).saturating_add(2), 5)).saturating_add(1);
    let m = if mp < 10 { mp.saturating_add(3) } else { mp.saturating_sub(9) };
    let y = yoe.saturating_add(era.saturating_mul(400)).saturating_add(u64::from(m <= 2));
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", div(rest, 3600), div(rem(rest, 3600), 60), rem(rest, 60))
}

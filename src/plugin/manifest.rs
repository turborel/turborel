//! \[EXTENSION\] A plug-in's manifest, format 1 (RULES.md D168): its
//! name, version, protocol, executables, and the objects it provides,
//! each with its SQL signature. `parser/manifest.rs` gives its grammar;
//! this checks what the grammar cannot.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use crate::ast::TypeName;
use crate::error::Result;
use crate::parser::Parser;
use crate::vector::Element;

use super::errors::invalid_manifest;

/// The words of a metric's PROPERTIES.
pub const SYMMETRIC: &str = "SYMMETRIC";
/// See `SYMMETRIC`.
pub const NON_NEGATIVE: &str = "NON_NEGATIVE";
/// See `SYMMETRIC`.
pub const TRIANGLE: &str = "TRIANGLE";
/// The set functions a plug-in's aggregate may not be named, as one of
/// their name would replace the standard's [ISO/IEC 9075:1992 §6.5].
const STANDARD_AGGREGATES: [&str; 5] = ["COUNT", "SUM", "AVG", "MAX", "MIN"];

/// The kinds of object a plug-in provides, each with its code in protocol
/// 1 (`payload.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    /// A scalar function.
    Scalar,
    /// An aggregate function.
    Aggregate,
    /// A table function.
    Table,
    /// A procedure.
    Procedure,
    /// A vector distance metric.
    Metric,
    /// An embedding model's plug-in.
    Embedding,
    /// An authentication mechanism of the server.
    Authenticator,
    /// A backup target of a scheme.
    Target,
    /// A partitioning strategy.
    Partition,
}

impl ObjectKind {
    /// Every kind, in the order of their codes.
    pub const ALL: [ObjectKind; 9] = [
        ObjectKind::Scalar,
        ObjectKind::Aggregate,
        ObjectKind::Table,
        ObjectKind::Procedure,
        ObjectKind::Metric,
        ObjectKind::Embedding,
        ObjectKind::Authenticator,
        ObjectKind::Target,
        ObjectKind::Partition,
    ];

    /// Its code in protocol 1.
    pub fn code(self) -> u8 {
        match self {
            ObjectKind::Scalar => 1,
            ObjectKind::Aggregate => 2,
            ObjectKind::Table => 3,
            ObjectKind::Procedure => 4,
            ObjectKind::Metric => 5,
            ObjectKind::Embedding => 6,
            ObjectKind::Authenticator => 7,
            ObjectKind::Target => 8,
            ObjectKind::Partition => 9,
        }
    }

    /// The kind of code `code`, if any.
    pub fn of_code(code: u8) -> Option<ObjectKind> {
        Self::ALL.into_iter().find(|k| k.code() == code)
    }

    /// Its name, as INFORMATION_SCHEMA.EXTENSION_OBJECTS shows it.
    pub fn word(self) -> &'static str {
        match self {
            ObjectKind::Scalar => "FUNCTION",
            ObjectKind::Aggregate => "AGGREGATE",
            ObjectKind::Table => "TABLE FUNCTION",
            ObjectKind::Procedure => "PROCEDURE",
            ObjectKind::Metric => "METRIC",
            ObjectKind::Embedding => "EMBEDDING MODEL",
            ObjectKind::Authenticator => "AUTHENTICATOR",
            ObjectKind::Target => "TARGET",
            ObjectKind::Partition => "PARTITION STRATEGY",
        }
    }
}

/// An object declared, as written.
#[derive(Debug, Clone)]
pub struct Declared {
    /// Its kind.
    pub kind: ObjectKind,
    /// Its name.
    pub name: String,
    /// Where it begins in the manifest.
    pub at: usize,
    /// Its declaration's text.
    pub text: String,
    /// Its parameters' types.
    pub parameters: Vec<TypeName>,
    /// Its result's type, of a function or aggregate.
    pub returns: Option<TypeName>,
    /// Its columns, of a table function.
    pub columns: Vec<(String, TypeName)>,
    /// DETERMINISTIC was written.
    pub deterministic: bool,
    /// Its BATCH, if written.
    pub batch: Option<u32>,
    /// READS SQL DATA was written, of a table function.
    pub reads: bool,
    /// MODIFIES SQL DATA was written, of a procedure.
    pub modifies: bool,
    /// A metric's FORMATS.
    pub formats: Vec<String>,
    /// A metric's PROPERTIES.
    pub properties: Vec<String>,
    /// A model's DIMENSIONS.
    pub dimensions: Option<u32>,
    /// A model's ELEMENT.
    pub element: Option<String>,
}

impl Declared {
    /// A declaration of `kind` and `name`, at `at`, of nothing more yet.
    pub fn new(kind: ObjectKind, name: String, at: usize) -> Declared {
        Declared {
            kind,
            name,
            at,
            text: String::new(),
            parameters: Vec::new(),
            returns: None,
            columns: Vec::new(),
            deterministic: false,
            batch: None,
            reads: false,
            modifies: false,
            formats: Vec::new(),
            properties: Vec::new(),
            dimensions: None,
            element: None,
        }
    }

    /// The element formats of a metric: FLOAT32 and FLOAT64 unless it
    /// names others.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T06 of a word no format's.
    pub fn metric_formats(&self) -> Result<Vec<Element>> {
        if self.formats.is_empty() {
            return Ok(vec![Element::Float32, Element::Float64]);
        }
        self.formats
            .iter()
            .map(|f| Element::named(f).ok_or_else(|| invalid_manifest(&format!("{f} is no vector element format"))))
            .collect()
    }

    /// Whether a metric declares the property `word`.
    pub fn has_property(&self, word: &str) -> bool {
        self.properties.iter().any(|p| p == word)
    }
}

/// An item of a manifest, as written.
#[derive(Debug, Clone)]
pub enum Item {
    /// NAME.
    Name(String),
    /// VERSION.
    Version(String),
    /// PROTOCOL.
    Protocol(u32),
    /// EXECUTABLE, for a platform if written.
    Executable(Option<String>, String),
    /// REQUIRES: another package, and the range of its versions taken.
    Requires(String, String),
    /// TURBOREL: the range of the database's versions it runs on.
    Turborel(String),
    /// UPGRADE FROM: a range of versions upgraded from, and the script of
    /// SQL that does it, relative to the directory.
    Upgrade(String, String),
    /// An object declared.
    Object(Box<Declared>),
}

/// A manifest, checked.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Its text.
    pub text: String,
    /// NAME.
    pub name: String,
    /// VERSION, empty if not written.
    pub version: String,
    /// The executables, each for a platform or, of none, for any other.
    pub executables: Vec<(Option<String>, String)>,
    /// The objects, in order.
    pub objects: Vec<Declared>,
    /// REQUIRES: the packages it depends on, each with a version range.
    pub requires: Vec<(String, String)>,
    /// TURBOREL: the range of the database's versions, if written.
    pub turborel: Option<String>,
    /// UPGRADE FROM: each range of old versions and its script.
    pub upgrades: Vec<(String, String)>,
}

impl Manifest {
    /// The manifest `text` is.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 where it breaks the grammar; SQLSTATE 38T06 where it
    /// breaks a rule of format 1, the rule named.
    pub fn parse(text: &str) -> Result<Manifest> {
        let items = Parser::new(text)?.manifest()?;
        let mut m = Manifest {
            text: text.to_owned(),
            name: String::new(),
            version: String::new(),
            executables: Vec::new(),
            objects: Vec::new(),
            requires: Vec::new(),
            turborel: None,
            upgrades: Vec::new(),
        };
        let mut protocol = None;
        for item in items {
            match item {
                Item::Name(n) => once(&mut m.name, n, "NAME")?,
                Item::Version(v) => once(&mut m.version, v, "VERSION")?,
                Item::Protocol(p) => {
                    if protocol.replace(p).is_some() {
                        return Err(invalid_manifest("PROTOCOL is written twice"));
                    }
                }
                Item::Executable(os, path) => m.executables.push((os, path)),
                Item::Object(d) => m.objects.push(*d),
                Item::Requires(name, range) => m.requires.push((name, range)),
                Item::Turborel(range) => {
                    if m.turborel.replace(range).is_some() {
                        return Err(invalid_manifest("TURBOREL is written twice"));
                    }
                }
                Item::Upgrade(range, script) => m.upgrades.push((range, script)),
            }
        }
        if protocol != Some(super::PROTOCOL) {
            return Err(invalid_manifest(&format!("PROTOCOL must be written, and be {}", super::PROTOCOL)));
        }
        m.check()?;
        Ok(m)
    }

    /// The rules the grammar does not hold.
    fn check(&self) -> Result<()> {
        if self.name.is_empty() {
            return Err(invalid_manifest("NAME must be written"));
        }
        check_executables(&self.executables)?;
        for (_, script) in &self.upgrades {
            inside(script, "SCRIPT")?;
        }
        let mut seen = HashSet::new();
        for d in &self.objects {
            if !seen.insert((d.kind, d.name.clone())) {
                return Err(invalid_manifest(&format!("{} {} is declared twice", d.kind.word(), d.name)));
            }
            check_object(d)?;
        }
        Ok(())
    }

    /// The executable of the platform `os`, relative to its directory: the
    /// one for it, else the one for any.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T05 if neither is written.
    pub fn executable(&self, os: &str) -> Result<PathBuf> {
        let mine = self.executables.iter().find(|(o, _)| o.as_deref() == Some(os));
        let any = self.executables.iter().find(|(o, _)| o.is_none());
        let (_, path) = mine.or(any).ok_or_else(|| super::errors::unavailable(&format!("its manifest names no executable for {os}")))?;
        Ok(PathBuf::from(path))
    }

    /// The codes of the kinds it declares.
    pub fn kinds(&self) -> Vec<u8> {
        let mut kinds: Vec<u8> = self.objects.iter().map(|d| d.kind.code()).collect();
        kinds.sort_unstable();
        kinds.dedup();
        kinds
    }
}

/// `value` written to `slot`, written once.
fn once(slot: &mut String, value: String, item: &str) -> Result<()> {
    if !slot.is_empty() {
        return Err(invalid_manifest(&format!("{item} is written twice")));
    }
    *slot = value;
    Ok(())
}

/// That at least one executable is written, at most one each platform
/// and one for any, each a path inside the directory.
fn check_executables(all: &[(Option<String>, String)]) -> Result<()> {
    if all.is_empty() {
        return Err(invalid_manifest("EXECUTABLE must be written"));
    }
    let mut platforms = HashSet::new();
    for (os, path) in all {
        if !platforms.insert(os.clone()) {
            return Err(invalid_manifest(&format!("EXECUTABLE is written twice for {}", os.as_deref().unwrap_or("any platform"))));
        }
        inside(path, "EXECUTABLE")?;
    }
    Ok(())
}

/// That `path`, of the item `item`, is relative and stays inside the
/// plug-in's directory.
///
/// # Errors
///
/// SQLSTATE 38T06 if not.
pub fn inside(path: &str, item: &str) -> Result<()> {
    let within = Path::new(path).components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if path.is_empty() || !within {
        return Err(invalid_manifest(&format!("{item} {path} is not a path inside the plug-in's directory")));
    }
    Ok(())
}

/// The rules of one declaration.
fn check_object(d: &Declared) -> Result<()> {
    let named = |what: &str| invalid_manifest(&format!("{} {} {what}", d.kind.word(), d.name));
    if d.batch == Some(0) {
        return Err(named("has a BATCH of 0"));
    }
    match d.kind {
        ObjectKind::Aggregate => {
            if d.parameters.len() != 1 {
                return Err(named("takes one argument"));
            }
            if STANDARD_AGGREGATES.contains(&d.name.as_str()) {
                return Err(named("would replace a standard set function"));
            }
            Ok(())
        }
        ObjectKind::Metric => {
            d.metric_formats()?;
            let known = [SYMMETRIC, NON_NEGATIVE, TRIANGLE];
            match d.properties.iter().find(|p| !known.contains(&p.as_str())) {
                Some(p) => Err(named(&format!("has no property {p}"))),
                None => Ok(()),
            }
        }
        ObjectKind::Embedding => {
            if d.dimensions == Some(0) {
                return Err(named("has no dimensions"));
            }
            match d.element.as_deref().filter(|e| Element::named(e).is_none()) {
                Some(e) => Err(named(&format!("has no element format {e}"))),
                None => Ok(()),
            }
        }
        ObjectKind::Scalar
        | ObjectKind::Table
        | ObjectKind::Procedure
        | ObjectKind::Authenticator
        | ObjectKind::Target
        | ObjectKind::Partition => Ok(()),
    }
}

#[cfg(test)]
mod tests;

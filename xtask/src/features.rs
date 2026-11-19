//! `cargo xtask features [--from DIR]`: writes the rows of the Definition
//! Schema's SQL_CONFORMANCE table, `src/system/definition/conformance.tsv`,
//! which the views SQL_FEATURES, SQL_PARTS and SQL_PACKAGES read
//! (RULES.md D37). Each row is a part, package, feature or subfeature of
//! the standards [ISO/IEC 9075-11:2006 draft §6.43], supported where
//! CONFORMANCE.md's table of claimed features names it, so the views say
//! what CONFORMANCE.md says. `verify` writes the rows again in memory and
//! fails if the file differs (`check`).
//!
//! With `--from DIR`, the parts, features and subfeatures are read from
//! the standards' texts in DIR (`features/taxonomy.rs`); without it, from
//! the file itself, whose element columns were read from them, so the
//! claims can change where the texts are not. It reads local files and
//! uses no network (R-TEST-5), and only std (R-TOOL-2).
//!
//! The file is a line of each element, its columns those of
//! SQL_CONFORMANCE in order, separated by tabs, an empty one the null
//! value: TYPE, ID, SUB_ID, NAME, SUB_NAME, IS_SUPPORTED, IS_VERIFIED_BY
//! and COMMENTS.

mod claims;
mod taxonomy;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::error::{Error, Result};
use crate::files;

/// The file written, from the workspace root.
pub const TABLE: &str = "src/system/definition/conformance.tsv";
/// The document whose claims it shows.
const CONFORMANCE: &str = "CONFORMANCE.md";
/// What separates a line's columns.
const SEPARATOR: char = '\t';
/// The columns of a line.
const COLUMNS: usize = 8;
/// [ISO/IEC 9075-11:2006 draft §6.43 Description 4] SUB_ID and SUB_NAME
/// of what is not a subfeature: a single space.
const NOT_SUB: &str = " ";
/// What joins a subfeature's identifier to its feature's: `E011-01`.
const SUB_MARK: char = '-';
/// IS_SUPPORTED's values [§6.43 Description 5].
const YES: &str = "YES";
const NO: &str = "NO";

/// [ISO/IEC 9075-11:2006 draft §6.43 Description 2] What a conformance
/// element is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Part,
    Package,
    Feature,
    Subfeature,
}

impl Kind {
    /// Its TYPE.
    fn name(self) -> &'static str {
        match self {
            Kind::Part => "PART",
            Kind::Package => "PACKAGE",
            Kind::Feature => "FEATURE",
            Kind::Subfeature => "SUBFEATURE",
        }
    }

    /// The kind whose TYPE is `name`.
    fn named(name: &str) -> Result<Kind> {
        [Kind::Part, Kind::Package, Kind::Feature, Kind::Subfeature]
            .into_iter()
            .find(|k| k.name() == name)
            .ok_or_else(|| Error::new(format!("{TABLE}: '{name}' is no TYPE of SQL_CONFORMANCE")))
    }
}

/// A conformance element, as the standards' texts name it: the columns of
/// SQL_CONFORMANCE that do not depend on what is claimed.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Element {
    pub kind: Kind,
    pub id: String,
    pub sub_id: String,
    pub name: String,
    pub sub_name: String,
}

impl Element {
    /// How CONFORMANCE.md names it: a feature's identifier, a subfeature's
    /// joined to its feature's, as `E011-01`.
    fn key(&self) -> String {
        match self.kind {
            Kind::Subfeature => format!("{}{SUB_MARK}{}", self.id, self.sub_id),
            Kind::Part | Kind::Package | Kind::Feature => self.id.clone(),
        }
    }

    /// Its name as CONFORMANCE.md writes it: a subfeature's own.
    fn own_name(&self) -> &str {
        match self.kind {
            Kind::Subfeature => &self.sub_name,
            Kind::Part | Kind::Package | Kind::Feature => &self.name,
        }
    }
}

/// Writes the file, of the elements of the texts in `from`, or of those
/// it holds.
pub fn generate(root: &Path, from: Option<&Path>) -> Result<()> {
    let elements = match from {
        Some(dir) => taxonomy::read_all(dir)?,
        None => elements(&files::read(root, TABLE)?)?,
    };
    let text = render(&elements, &claims::read(&files::read(root, CONFORMANCE)?)?)?;
    let path = root.join(TABLE);
    fs::write(&path, &text).map_err(|e| Error::new(format!("cannot write {}: {e}", path.display())))?;
    println!("features: wrote {} rows to {TABLE}", elements.len());
    Ok(())
}

/// Checks that the file holds what CONFORMANCE.md claims (RULES.md
/// R-TRACE-3).
pub fn check(root: &Path) -> Result<()> {
    let current = files::read(root, TABLE)?;
    let wanted = render(&elements(&current)?, &claims::read(&files::read(root, CONFORMANCE)?)?)?;
    if current.lines().eq(wanted.lines()) {
        println!("features: ok ({TABLE} shows {CONFORMANCE}'s claims)");
        Ok(())
    } else {
        Err(Error::new(format!("R-TRACE-3: {TABLE} does not show {CONFORMANCE}'s claimed features; run `cargo xtask features`")))
    }
}

/// The elements of the file's `text`, its first five columns.
fn elements(text: &str) -> Result<Vec<Element>> {
    text.lines()
        .map(|line| {
            let fields: Vec<&str> = line.split(SEPARATOR).collect();
            // The columns of SQL_CONFORMANCE, `COLUMNS` of them.
            let [kind, id, sub_id, name, sub_name, _, _, _] = fields.as_slice() else {
                return Err(Error::new(format!("{TABLE}: '{line}' has not {COLUMNS} columns")));
            };
            let to = |s: &&str| (*s).to_owned();
            Ok(Element { kind: Kind::named(kind)?, id: to(id), sub_id: to(sub_id), name: to(name), sub_name: to(sub_name) })
        })
        .collect()
}

/// The file's text: a line of each of `elements`, sorted, supported where
/// `claims` names it or the feature it is a subfeature of [ISO/IEC
/// 9075-11:2006 draft §6.43 Description 7], with the claim's comment.
///
/// # Errors
///
/// A claim names no element, or names one by another name.
fn render(elements: &[Element], claims: &[claims::Claim]) -> Result<String> {
    let by_key: BTreeMap<String, &Element> = elements.iter().map(|e| (e.key(), e)).collect();
    let mut claimed = BTreeMap::new();
    for claim in claims {
        let element = by_key
            .get(&claim.key)
            .ok_or_else(|| Error::new(format!("{CONFORMANCE} claims {}, which no standard's taxonomy lists", claim.key)))?;
        if element.own_name() != claim.name {
            let message = format!("{CONFORMANCE} names {} '{}', which the standard names '{}'", claim.key, claim.name, element.own_name());
            return Err(Error::new(message));
        }
        claimed.insert(claim.key.clone(), claim.comment.as_str());
    }
    let mut sorted = elements.to_vec();
    sorted.sort();
    let mut text = String::new();
    for e in &sorted {
        let comment = claimed.get(&e.key()).or_else(|| (e.kind == Kind::Subfeature).then(|| claimed.get(&e.id)).flatten());
        let supported = if comment.is_some() { YES } else { NO };
        let fields = [e.kind.name(), &e.id, &e.sub_id, &e.name, &e.sub_name, supported, "", comment.copied().unwrap_or_default()];
        text.push_str(&fields.join(&SEPARATOR.to_string()));
        text.push('\n');
    }
    Ok(text)
}

/// A feature of `id` and `name`, or a subfeature if `id` has a `SUB_MARK`,
/// of the feature `parent` names.
pub fn feature(id: &str, name: &str, parent: Option<&str>) -> Element {
    match id.split_once(SUB_MARK) {
        Some((feature, sub)) => Element {
            kind: Kind::Subfeature,
            id: feature.to_owned(),
            sub_id: sub.to_owned(),
            name: parent.unwrap_or_default().to_owned(),
            sub_name: name.to_owned(),
        },
        None => Element {
            kind: Kind::Feature,
            id: id.to_owned(),
            sub_id: NOT_SUB.to_owned(),
            name: name.to_owned(),
            sub_name: NOT_SUB.to_owned(),
        },
    }
}

/// Part `number` of ISO/IEC 9075, named `name`.
pub fn part(number: &str, name: &str) -> Element {
    Element { kind: Kind::Part, id: number.to_owned(), sub_id: NOT_SUB.to_owned(), name: name.to_owned(), sub_name: NOT_SUB.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(key: &str, name: &str) -> claims::Claim {
        claims::Claim { key: key.to_owned(), name: name.to_owned(), comment: format!("of {key}") }
    }

    fn sample() -> Vec<Element> {
        vec![
            feature("T321-01", "User-defined functions with no overloading", Some("Basic SQL-invoked routines")),
            feature("T321", "Basic SQL-invoked routines", None),
            part("2", "Foundation (SQL/Foundation)"),
            feature("T031", "BOOLEAN data type", None),
            feature("T011", "Timestamp in Information Schema", None),
        ]
    }

    /// Verifies: [TOOL] R-TRACE-3 the rows are sorted, parts first, each
    /// feature before its subfeatures; a claimed feature is YES with the
    /// claim's comment, its subfeature YES with it too [ISO/IEC
    /// 9075-11:2006 draft §6.43 Description 7], an unclaimed one NO with
    /// no comment; IS_VERIFIED_BY is null; and a feature's SUB_ID and
    /// SUB_NAME are a single space [Description 4].
    #[test]
    fn claims_make_the_rows_supported() {
        let text = render(&sample(), &[claim("T321", "Basic SQL-invoked routines")]).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "PART\t2\t \tFoundation (SQL/Foundation)\t \tNO\t\t");
        assert_eq!(lines[1], "FEATURE\tT011\t \tTimestamp in Information Schema\t \tNO\t\t");
        assert_eq!(lines[3], "FEATURE\tT321\t \tBasic SQL-invoked routines\t \tYES\t\tof T321");
        assert_eq!(
            lines[4],
            "SUBFEATURE\tT321\t01\tBasic SQL-invoked routines\tUser-defined functions with no overloading\tYES\t\tof T321"
        );
        assert!(lines.iter().all(|l| l.split(SEPARATOR).count() == COLUMNS));
        let sub = render(&sample(), &[claim("T321-01", "User-defined functions with no overloading")]).unwrap();
        assert!(sub.contains("FEATURE\tT321\t \tBasic SQL-invoked routines\t \tNO"), "{sub}");
    }

    /// Verifies: [TOOL] R-TRACE-3 the file read back gives the elements it
    /// was written of, so writing it again of them and the same claims
    /// gives the same text.
    #[test]
    fn the_file_reads_back_as_written() {
        let claims = [claim("T031", "BOOLEAN data type")];
        let text = render(&sample(), &claims).unwrap();
        let read = elements(&text).unwrap();
        let mut sorted = sample();
        sorted.sort();
        assert_eq!(read, sorted);
        assert_eq!(render(&read, &claims).unwrap(), text);
        assert!(elements("FEATURE\tT031").is_err());
        assert!(elements("FEATURE\tT031\t \tx\t \tNO\t").is_err());
        assert!(elements("THING\tT031\t \tx\t \tNO\t\t").is_err());
    }

    /// Verifies: [TOOL] R-TRACE-3 a claim of what no taxonomy lists, or by
    /// a name the standard does not give it, is refused.
    #[test]
    fn a_wrong_claim_is_refused() {
        let unknown = render(&sample(), &[claim("T999", "Nothing")]).unwrap_err().to_string();
        assert!(unknown.contains("T999"), "{unknown}");
        let misnamed = render(&sample(), &[claim("T031", "Boolean type")]).unwrap_err().to_string();
        assert!(misnamed.contains("'BOOLEAN data type'"), "{misnamed}");
    }

    /// Verifies: [TOOL] R-TRACE-3 the check passes of a tree whose file
    /// shows CONFORMANCE.md's claims, fails once a claim is added without
    /// writing the file again, and passes after `generate` writes it.
    #[test]
    fn a_stale_file_fails_the_check() {
        let root = std::env::temp_dir().join(format!("xtask-features-{}", std::process::id()));
        fs::create_dir_all(root.join("src/system/definition")).unwrap();
        let document = |claimed: &str| {
            format!("# Conformance\n\n## Features claimed\n\n| Feature | Name | Claimed in |\n|---|---|---|\n{claimed}\n\n## Next\n")
        };
        fs::write(root.join(CONFORMANCE), document("")).unwrap();
        fs::write(root.join(TABLE), render(&sample(), &[]).unwrap()).unwrap();
        assert!(check(&root).is_ok());
        fs::write(root.join(CONFORMANCE), document("| T031 | BOOLEAN data type | [Truth values](#truth-values) |")).unwrap();
        let error = check(&root).unwrap_err().to_string();
        assert!(error.contains("cargo xtask features"), "{error}");
        generate(&root, None).unwrap();
        assert!(check(&root).is_ok());
        assert!(files::read(&root, TABLE).unwrap().contains("T031\t \tBOOLEAN data type\t \tYES\t\tCONFORMANCE.md, Truth values"));
        fs::remove_dir_all(&root).unwrap();
    }
}

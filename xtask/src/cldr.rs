//! `cargo xtask cldr --from CLDR_DIR UCD_DIR`: generates what the
//! collations `UNICODE_<LOCALE>` read (RULES.md D60) from CLDR 38 in
//! CLDR_DIR and Unicode 13.0 in UCD_DIR. It reads local files and uses no
//! network (R-TEST-5), and only std (R-TOOL-2).
//!
//! `src/value/collation/cldr38.rules` holds the rules of every collation
//! type of every locale but those for search, a section each. The rules
//! stay as CLDR writes them, so each can be read against its source:
//! comments and blank lines are taken out, `[normalization on]`, which the
//! collations always do, is left out, and an import names the locale and
//! type of a section of the file. A type a collation imports but no name
//! is given, as CLDR's private ones, has a section of its own. The root's
//! private Han rules gain the radical-stroke order of the ideographs.
//!
//! `src/value/collation/cldr38.groups` holds the script reordering groups
//! laid over UNICODE's primaries (`groups.rs`).

mod groups;
mod xml;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::error::{Error, Result};
use crate::unicode::{ducet, read};

/// The files made, from the workspace root.
const RULES: &str = "src/value/collation/cldr38.rules";
const GROUPS: &str = "src/value/collation/cldr38.groups";
/// Where the rules and the root's data sit in CLDR_DIR.
const COLLATION_DIR: &str = "common/collation";
/// The extension of a locale's file there.
const XML: &str = "xml";
const FRACTIONAL: &str = "common/uca/FractionalUCA.txt";
/// Unicode's file of general categories.
const UNICODE_DATA: &str = "UnicodeData.txt";
/// What the files begin with.
const RULES_HEADER: &str = "\
# The rules of the collations UNICODE_<LOCALE> (RULES.md D60), from CLDR
# 38, made by `cargo xtask cldr`. Each section begins with a line
# \"@ NAME LOCALE TYPE\", NAME \"-\" for rules only imported. The rules are
# CLDR's, under the terms of LICENSE-UNICODE.
";
const GROUPS_HEADER: &str = "\
# The script reordering groups of CLDR 38 over the primaries of UNICODE
# (RULES.md D60), made by `cargo xtask cldr`: a line for each range, its
# first and last primary and the reorder codes of its group. From CLDR's
# FractionalUCA.txt, under the terms of LICENSE-UNICODE.
";
/// The option every collation does already.
const NORMALIZATION: &str = "[normalization on]";
/// An option naming rules to import, and what names a type in its tag.
const IMPORT: &str = "[import ";
const TYPE_KEY: &str = "-u-co-";
/// The locale CLDR's tags name `und`, and the file holding it.
const UND: &str = "und";
const ROOT: &str = "root";
/// The types left out, and those only imported.
const SEARCH: &str = "search";
const PRIVATE: &str = "private-";
const STANDARD: &str = "standard";
/// The root's private Han rules, which gain the radical-stroke order.
const UNIHAN: &str = "private-unihan";
/// The types an import's tag names by their keys in a BCP 47 tag, and the
/// types they are.
const KEYS: [(&str, &str); 4] = [("phonebk", "phonebook"), ("trad", "traditional"), ("dict", "dictionary"), ("gb2312", "gb2312han")];
/// What a section's name is when the rules are only imported.
const NO_NAME: &str = "-";

/// A section of the rules file: a locale's type, its name, and its lines.
struct Section {
    name: String,
    lines: Vec<String>,
}

/// Generates the files from the data in `cldr` and `ucd`.
pub fn generate(root: &Path, cldr: &Path, ucd: &Path) -> Result<()> {
    let fractional = file(&cldr.join(FRACTIONAL))?;
    let groups = groups::text(GROUPS_HEADER, &fractional, &ducet(ucd)?, &read(ucd, UNICODE_DATA)?)?;
    write(&root.join(GROUPS), &groups)?;
    let mut sections = sections(&cldr.join(COLLATION_DIR))?;
    if let Some(unihan) = sections.get_mut(&(ROOT.to_owned(), UNIHAN.to_owned())) {
        unihan.lines.splice(0..0, groups::radicals(&fractional)?);
    }
    let mut out = String::from(RULES_HEADER);
    for ((locale, kind), section) in &sections {
        out.push_str(&["@ ", &section.name, " ", locale, " ", kind, "\n"].concat());
        for line in &section.lines {
            out.push_str(line);
            out.push('\n');
        }
    }
    write(&root.join(RULES), &out)?;
    let named = sections.values().filter(|s| s.name != NO_NAME).count();
    println!("cldr: wrote {named} collations to {RULES} and the groups to {GROUPS}");
    Ok(())
}

fn file(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))
}

fn write(path: &Path, text: &str) -> Result<()> {
    fs::write(path, text).map_err(|e| Error::new(format!("cannot write {}: {e}", path.display())))
}

/// Every section the rules file holds, by locale and type: each named
/// collation, then each imported type no name is given.
fn sections(dir: &Path) -> Result<BTreeMap<(String, String), Section>> {
    let mut files = BTreeMap::new();
    let entries = fs::read_dir(dir).map_err(|e| Error::new(format!("cannot read {}: {e}", dir.display())))?;
    for entry in entries {
        let path = entry.map_err(|e| Error::new(format!("cannot read {}: {e}", dir.display())))?.path();
        if let Some(locale) = path.file_stem().and_then(|s| s.to_str()).filter(|_| path.extension().is_some_and(|e| e == XML)) {
            files.insert(locale.to_owned(), file(&path)?);
        }
    }
    let mut out = BTreeMap::new();
    for (locale, text) in &files {
        named(locale, text, &mut out);
    }
    let mut wanted: BTreeSet<(String, String)> = out.values().flat_map(|s: &Section| imports(&s.lines)).collect();
    while let Some(key) = wanted.pop_first() {
        if out.contains_key(&key) {
            continue;
        }
        let text = files.get(&key.0).ok_or_else(|| Error::new(format!("an import of {} {}, which has no file", key.0, key.1)))?;
        let rules = xml::collations(text).into_iter().find(|c| c.kind == key.1);
        let rules = rules.ok_or_else(|| Error::new(format!("an import of {} {}, which is not there", key.0, key.1)))?;
        let lines = lines(rules.rules);
        wanted.extend(imports(&lines));
        out.insert(key, Section { name: NO_NAME.to_owned(), lines });
    }
    Ok(out)
}

/// The named collations of `locale`, whose file is `text`, added to `out`.
/// A default type the file names but lacks is its parent locale's,
/// imported.
fn named(locale: &str, text: &str, out: &mut BTreeMap<(String, String), Section>) {
    let named_default = xml::default(text);
    let default = named_default.unwrap_or(STANDARD);
    let mut kinds = BTreeSet::new();
    for collation in xml::collations(text) {
        kinds.insert(collation.kind);
        if collation.kind.starts_with(SEARCH) || collation.kind.starts_with(PRIVATE) {
            continue;
        }
        let lines = lines(collation.rules);
        if !lines.is_empty() {
            out.insert((locale.to_owned(), collation.kind.to_owned()), Section { name: name(locale, collation.kind, default), lines });
        }
    }
    if let Some((parent, _)) = locale.rsplit_once('_')
        && named_default.is_some()
        && !kinds.contains(default)
    {
        let lines = vec![[IMPORT, parent, " ", default, "]"].concat()];
        out.insert((locale.to_owned(), default.to_owned()), Section { name: name(locale, default, default), lines });
    }
}

/// The name of type `kind` of `locale`: `UNICODE_<LOCALE>` for its
/// default, `UNICODE_<LOCALE>_<TYPE>` for another, `UNICODE_<TYPE>` for
/// the root's.
fn name(locale: &str, kind: &str, default: &str) -> String {
    let kind = kind.to_uppercase().replace('-', "_");
    match (locale, kind == default.to_uppercase()) {
        (ROOT, _) => ["UNICODE_", &kind].concat(),
        (_, true) => ["UNICODE_", &locale.to_uppercase()].concat(),
        (_, false) => ["UNICODE_", &locale.to_uppercase(), "_", &kind].concat(),
    }
}

/// The lines of `rules`: comments, blank lines and `[normalization on]`
/// taken out, and each import naming a locale and a type.
fn lines(rules: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in rules.lines().map(xml::uncommented).map(str::trim).filter(|l| !l.is_empty() && *l != NORMALIZATION) {
        match line.strip_prefix(IMPORT).and_then(|rest| rest.strip_suffix(']')) {
            Some(tag) => {
                let (locale, kind) = imported(tag);
                out.push([IMPORT, &locale, " ", &kind, "]"].concat());
            }
            None => out.push(line.to_owned()),
        }
    }
    out
}

/// The locale and type an import's tag names.
fn imported(tag: &str) -> (String, String) {
    let (locale, kind) = tag.split_once(TYPE_KEY).unwrap_or((tag, STANDARD));
    let locale = if locale == UND { ROOT.to_owned() } else { locale.replace('-', "_") };
    let kind = KEYS.iter().find(|(key, _)| *key == kind).map_or(kind, |(_, full)| full);
    (locale, kind.to_owned())
}

/// The locales and types `lines` import.
fn imports(lines: &[String]) -> Vec<(String, String)> {
    let named = lines.iter().filter_map(|l| l.strip_prefix(IMPORT)?.strip_suffix(']'));
    named.filter_map(|l| l.split_once(' ')).map(|(locale, kind)| (locale.to_owned(), kind.to_owned())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [TOOL] R-TOOL-2 a locale's default type is named after
    /// the locale alone, another type after both, and the root's after its
    /// type.
    #[test]
    fn names_follow_the_default_type() {
        assert_eq!(name("sv", "reformed", "reformed"), "UNICODE_SV");
        assert_eq!(name("sv", "standard", "reformed"), "UNICODE_SV_STANDARD");
        assert_eq!(name("zh_Hant", "stroke", "stroke"), "UNICODE_ZH_HANT");
        assert_eq!(name("root", "emoji", "standard"), "UNICODE_EMOJI");
    }

    /// Verifies: [TOOL] R-TOOL-2 comments, blank lines and
    /// `[normalization on]` are taken out, and an import names a locale
    /// and type, `und` the root.
    #[test]
    fn imports_name_a_locale_and_type() {
        let read =
            lines("[normalization on]\n# note\n[import und-u-co-private-unihan]\n\n[import sr-Latn]\n&a<b # why\n[import de-u-co-phonebk]");
        assert_eq!(read, ["[import root private-unihan]", "[import sr_Latn standard]", "&a<b", "[import de phonebook]"]);
        assert_eq!(imports(&read).get(1), Some(&("sr_Latn".to_owned(), STANDARD.to_owned())));
    }

    /// Verifies: [TOOL] R-TOOL-2 a locale's collations are named but not
    /// those for search, private ones or empty ones, and a default type
    /// the file names but lacks imports its parent's. MC/DC of that
    /// import, a parent, a default named, and it lacking: `zh_Hant`
    /// against `zh` (parent), `de_AT` (named) and `sv_FI` (lacking).
    #[test]
    fn a_locales_collations_are_named() {
        let mut out = BTreeMap::new();
        let xml = "<collation type=\"search\"><cr><![CDATA[&a<s]]></cr></collation>\
                   <collation type=\"private-x\"><cr><![CDATA[&a<p]]></cr></collation>\
                   <collation type=\"standard\"><cr><![CDATA[&a<b]]></cr></collation>\
                   <collation type=\"phonebook\"></collation>";
        named("de", xml, &mut out);
        named("zh_Hant", "<defaultCollation>stroke</defaultCollation>", &mut out);
        named("de_AT", "<collations/>", &mut out);
        named("zh", "<defaultCollation>pinyin</defaultCollation>", &mut out);
        named(
            "sv_FI",
            "<defaultCollation>reformed</defaultCollation><collation type='reformed'><cr><![CDATA[&a<b]]></cr></collation>",
            &mut out,
        );
        let names: Vec<(&str, &str, &str)> = out.iter().map(|((l, k), s)| (l.as_str(), k.as_str(), s.name.as_str())).collect();
        assert_eq!(
            names,
            [("de", "standard", "UNICODE_DE"), ("sv_FI", "reformed", "UNICODE_SV_FI"), ("zh_Hant", "stroke", "UNICODE_ZH_HANT")]
        );
        assert_eq!(
            out.get(&("zh_Hant".to_owned(), "stroke".to_owned())).map(|s| s.lines.clone()),
            Some(vec!["[import zh stroke]".to_owned()])
        );
    }
}

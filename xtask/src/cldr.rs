//! `cargo xtask cldr --from DIR`: generates the tailorings the collations
//! `UNICODE_<LOCALE>` read, `src/value/collation/cldr38.rules`, from the
//! collation rules of CLDR 38 in DIR, its `common/collation` directory
//! (RULES.md D60). It reads local files and uses no network (R-TEST-5), and
//! only std (R-TOOL-2).
//!
//! The rules stay as CLDR writes them, so each can be read against its
//! source: comments and blank lines are taken out, an `[import]` is
//! replaced by the rules it names, and `[normalization on]`, which the
//! collations always do, is left out.

use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// The file's path, from the workspace root.
const RULES: &str = "src/value/collation/cldr38.rules";
/// Where the rules sit in DIR.
const COLLATION_DIR: &str = "common/collation";
/// What the file begins with.
const HEADER: &str = "\
# The tailorings of the collations UNICODE_<LOCALE> (RULES.md D60), from the
# collation rules of CLDR 38, made by `cargo xtask cldr`. Each begins with a
# line \"@ NAME LOCALE TYPE\". The rules are CLDR's, under the terms of
# LICENSE-UNICODE.
";
/// The tailorings made: each locale, its type, and the name's suffix.
/// Those whose rules reorder scripts are left out, as the collations
/// order every script as UNICODE does.
const TAILORINGS: [(&str, &str, &str); 48] = [
    ("af", "standard", "AF"),
    ("br", "standard", "BR"),
    ("ceb", "standard", "CEB"),
    ("cs", "standard", "CS"),
    ("cy", "standard", "CY"),
    ("da", "standard", "DA"),
    ("de", "phonebook", "DE_PHONEBOOK"),
    ("dsb", "standard", "DSB"),
    ("ee", "standard", "EE"),
    ("eo", "standard", "EO"),
    ("es", "standard", "ES"),
    ("es", "traditional", "ES_TRADITIONAL"),
    ("fi", "standard", "FI"),
    ("fil", "standard", "FIL"),
    ("fo", "standard", "FO"),
    ("fr_CA", "standard", "FR_CA"),
    ("gl", "standard", "GL"),
    ("ha", "standard", "HA"),
    ("haw", "standard", "HAW"),
    ("hsb", "standard", "HSB"),
    ("hu", "standard", "HU"),
    ("ig", "standard", "IG"),
    ("is", "standard", "IS"),
    ("kl", "standard", "KL"),
    ("lkt", "standard", "LKT"),
    ("ln", "standard", "LN"),
    ("lt", "standard", "LT"),
    ("lv", "standard", "LV"),
    ("mt", "standard", "MT"),
    ("nb", "standard", "NB"),
    ("nn", "standard", "NN"),
    ("om", "standard", "OM"),
    ("pl", "standard", "PL"),
    ("ro", "standard", "RO"),
    ("se", "standard", "SE"),
    ("sk", "standard", "SK"),
    ("sl", "standard", "SL"),
    ("smn", "standard", "SMN"),
    ("sq", "standard", "SQ"),
    ("sv", "standard", "SV"),
    ("tk", "standard", "TK"),
    ("to", "standard", "TO"),
    ("tr", "standard", "TR"),
    ("uz", "standard", "UZ"),
    ("vi", "standard", "VI"),
    ("wae", "standard", "WAE"),
    ("wo", "standard", "WO"),
    ("yo", "standard", "YO"),
];
/// What a collation's rules sit between.
const OPEN: &str = "<![CDATA[";
const CLOSE: &str = "]]>";
/// The option every collation does already.
const NORMALIZATION: &str = "[normalization on]";
/// An option naming rules to put in its place.
const IMPORT: &str = "[import ";
/// What names a type in an import's locale tag.
const TYPE_KEY: &str = "-u-co-";
/// How deep imports may go.
const MOST_IMPORTS: usize = 4;

/// Generates the file from the rules in `from`.
pub fn generate(root: &Path, from: &Path) -> Result<()> {
    let mut out = String::from(HEADER);
    for (locale, kind, suffix) in TAILORINGS {
        out.push_str(&["@ UNICODE_", suffix, " ", locale, " ", kind, "\n"].concat());
        for line in rules(from, locale, kind, 0)? {
            out.push_str(&line);
            out.push('\n');
        }
    }
    let path = root.join(RULES);
    fs::write(&path, &out).map_err(|e| Error::new(format!("cannot write {}: {e}", path.display())))?;
    println!("cldr: wrote {} tailorings to {RULES}", TAILORINGS.len());
    Ok(())
}

/// The lines of the rules of type `kind` of `locale`, their imports
/// replaced, `depth` imports deep.
fn rules(from: &Path, locale: &str, kind: &str, depth: usize) -> Result<Vec<String>> {
    if depth > MOST_IMPORTS {
        return Err(Error::new(format!("the imports of {locale} {kind} go too deep")));
    }
    let path = from.join(COLLATION_DIR).join(format!("{locale}.xml"));
    let xml = fs::read_to_string(&path).map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
    let text = collation(&xml, kind).ok_or_else(|| Error::new(format!("{} has no collation of type {kind}", path.display())))?;
    let mut out = Vec::new();
    for line in text.lines().map(uncommented).map(str::trim).filter(|l| !l.is_empty() && *l != NORMALIZATION) {
        match line.strip_prefix(IMPORT).and_then(|rest| rest.strip_suffix(']')) {
            Some(tag) => {
                let (imported, of_kind) = tag.split_once(TYPE_KEY).unwrap_or((tag, "standard"));
                out.extend(rules(from, &imported.replace('-', "_"), of_kind, depth.saturating_add(1))?);
            }
            None => out.push(line.to_owned()),
        }
    }
    Ok(out)
}

/// The rules of the collation of type `kind` in `xml`, not an alternative.
fn collation<'x>(xml: &'x str, kind: &str) -> Option<&'x str> {
    let wanted = format!("type=\"{kind}\"");
    let mut rest = xml;
    while let Some(at) = rest.find("<collation ") {
        let element = rest.get(at..)?;
        let tag = element.get(..element.find('>')?)?;
        let body = element.get(tag.len()..)?;
        if tag.contains(&wanted) && !tag.contains("alt=") {
            let open = body.find(OPEN)?.checked_add(OPEN.len())?;
            let close = body.find(CLOSE)?;
            return body.get(open..close);
        }
        rest = body;
    }
    None
}

/// `line` without a comment, a `#` outside quotes and what follows it.
fn uncommented(line: &str) -> &str {
    let mut quoted = false;
    for (at, c) in line.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            '#' if !quoted => return line.get(..at).unwrap_or(line),
            _ => {}
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [TOOL] R-TOOL-2 the rules of the type asked for are found,
    /// not those of an alternative or of another type.
    #[test]
    fn a_collations_rules_are_found_by_type() {
        let xml = "<collations><collation type=\"search\"><cr><![CDATA[&a<s]]></cr></collation>\
                   <collation alt=\"proposed\" type=\"standard\"><cr><![CDATA[&a<p]]></cr></collation>\
                   <collation type=\"standard\"><cr><![CDATA[&a<b]]></cr></collation></collations>";
        assert_eq!(collation(xml, "standard"), Some("&a<b"));
        assert_eq!(collation(xml, "phonebook"), None);
    }

    /// Verifies: [TOOL] R-TOOL-2 a comment is taken out, but not a `#`
    /// that is quoted.
    #[test]
    fn comments_are_taken_out_but_not_quoted_hashes() {
        assert_eq!(uncommented("&a<b # why"), "&a<b ");
        assert_eq!(uncommented("&a<'#'"), "&a<'#'");
    }
}

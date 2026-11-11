//! The collations a CLDR locale file holds, read with only what the files
//! use: each `<collation>` element's type, whether it is an alternative,
//! and the rules between `<![CDATA[` and `]]>`.

/// What a collation's rules sit between.
const OPEN: &str = "<![CDATA[";
const CLOSE: &str = "]]>";
/// What an element and the default type begin and end with.
const ELEMENT: &str = "<collation ";
const END: &str = "</collation>";
const DEFAULT: &str = "<defaultCollation>";
const DEFAULT_END: &str = "</defaultCollation>";
/// The attribute naming a collation's type, quoted either way.
const TYPE: &str = "type=";
/// The attribute of an alternative.
const ALTERNATIVE: &str = "alt=";

/// A collation of a locale file: its type and its rules.
pub struct Collation<'x> {
    pub kind: &'x str,
    pub rules: &'x str,
}

/// The collations of `xml` that are not alternatives, in the file's order.
pub fn collations(xml: &str) -> Vec<Collation<'_>> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find(ELEMENT) {
        let element = rest.get(at..).unwrap_or_default();
        let tag = element.get(..element.find('>').unwrap_or(element.len())).unwrap_or_default();
        let body = element.get(tag.len()..).unwrap_or_default();
        let inside = body.get(..body.find(END).unwrap_or(body.len())).unwrap_or_default();
        rest = body.get(inside.len()..).unwrap_or_default();
        if tag.contains(ALTERNATIVE) {
            continue;
        }
        let (Some(kind), Some(rules)) = (kind(tag), rules(inside)) else { continue };
        out.push(Collation { kind, rules });
    }
    out
}

/// The type an element's tag names.
fn kind(tag: &str) -> Option<&str> {
    let value = tag.get(tag.find(TYPE)?.checked_add(TYPE.len())?..)?;
    let quote = value.chars().next().filter(|q| matches!(q, '"' | '\''))?;
    let value = value.get(quote.len_utf8()..)?;
    value.get(..value.find(quote)?)
}

/// The rules inside an element, empty if it has none.
fn rules(inside: &str) -> Option<&str> {
    let Some(open) = inside.find(OPEN) else { return Some("") };
    let from = open.checked_add(OPEN.len())?;
    let to = inside.get(from..)?.find(CLOSE)?.checked_add(from)?;
    inside.get(from..to)
}

/// The type `xml` names its default, if it names one.
pub fn default(xml: &str) -> Option<&str> {
    let from = xml.find(DEFAULT)?.checked_add(DEFAULT.len())?;
    let to = xml.get(from..)?.find(DEFAULT_END)?.checked_add(from)?;
    xml.get(from..to).map(str::trim)
}

/// `line` without a comment, a `#` outside quotes and what follows it.
pub fn uncommented(line: &str) -> &str {
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

    /// Verifies: [TOOL] R-TOOL-2 every collation is found with its type,
    /// quoted either way and with its tag over lines, but not an
    /// alternative; one with no rules has empty rules.
    #[test]
    fn collations_are_found_by_type() {
        let xml = "<defaultCollation> pinyin </defaultCollation><collations><collation type=\"search\"><cr><![CDATA[&a<s]]></cr></collation>\
                   <collation alt=\"proposed\" type=\"standard\"><cr><![CDATA[&a<p]]></cr></collation>\
                   <collation type='standard'\n references=\"x\"><cr><![CDATA[&a<b]]></cr></collation>\
                   <collation type=\"standard\"></collation></collations>";
        let found: Vec<(&str, &str)> = collations(xml).iter().map(|c| (c.kind, c.rules)).collect();
        assert_eq!(found, [("search", "&a<s"), ("standard", "&a<b"), ("standard", "")]);
        assert_eq!(default(xml), Some("pinyin"));
        assert_eq!(default("<collations/>"), None);
    }

    /// Verifies: [TOOL] R-TOOL-2 a comment is taken out, but not a `#`
    /// that is quoted.
    #[test]
    fn comments_are_taken_out_but_not_quoted_hashes() {
        assert_eq!(uncommented("&a<b # why"), "&a<b ");
        assert_eq!(uncommented("&a<'#'"), "&a<'#'");
    }
}

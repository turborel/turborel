//! [ISO/IEC 9075-14:2006 draft §9.1 to §9.3] SQL identifiers mapped to
//! XML Names and back: each character an XML Name cannot hold written
//! `_xHHHH_`, or `_xHHHHHH_` beyond the Basic Multilingual Plane, and such
//! an escape read back as its character.
//!
//! SQL_TEXT is Unicode here, so the implementation-defined mappings of
//! SQL_TEXT to Unicode and back [§9.1 GR3, §9.3 GR4] are the identity,
//! and no character is unmappable [§9.1 GR4a, §9.3 GR4]. turborel
//! supports Feature X211, so a character is escaped if it is no XML 1.1
//! name character [§9.1 GR4e].

use super::chars::{COLON, Version, is_name_char, is_name_start};
use crate::regex::unicode::Table;

/// [§9.1 GR2] The escape variant of a mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escape {
    /// Partially escaped: a colon after the first character is kept.
    Partial,
    /// Fully escaped: every colon, and a leading `xml` in any case, is
    /// escaped.
    Full,
}

/// The escape of the character of code `code` [§9.1 GR4e ii]: four
/// uppercase hexits within the Basic Multilingual Plane, else six.
fn escaped(code: u32) -> String {
    if code <= 0xFFFF { format!("_x{code:04X}_") } else { format!("_x{code:06X}_") }
}

/// [§9.1] The XML Name the SQL identifier `identifier` maps to, by the
/// escape variant `variant`.
#[must_use]
pub fn name_of(identifier: &str, variant: Escape) -> String {
    let chars: Vec<char> = identifier.chars().collect();
    let leading_xml = chars.len() >= 3 && chars.iter().take(3).collect::<String>().eq_ignore_ascii_case("xml");
    let mut name = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let next = chars.get(i.saturating_add(1)).copied();
        let piece = if c == COLON {
            // [GR4b] A colon, escaped first or fully escaped.
            if i == 0 || variant == Escape::Full { escaped(u32::from(c)) } else { c.to_string() }
        } else if c == '_' && next == Some('x') {
            // [GR4c] An underscore before a lowercase x, which reads as
            // an escape.
            escaped(u32::from(c))
        } else if variant == Escape::Full && i == 0 && leading_xml {
            // [GR4d] The x of a leading `xml`, fully escaped.
            escaped(u32::from(c))
        } else if !is_name_char(Version::V11, c) || (i == 0 && !is_name_start(Version::V11, c)) {
            // [GR4e] A character no name holds, or no name begins with.
            escaped(u32::from(c))
        } else {
            // [GR4f] Any other, as it is.
            c.to_string()
        };
        name.push_str(&piece);
    }
    name
}

/// [§9.2] The XML Name the multi-part SQL name `parts` maps to: each part
/// mapped fully escaped \[GR3\], its periods escaped \[GR2\], the parts joined
/// by periods \[GR4\].
#[must_use]
pub fn name_of_parts(parts: &[&str]) -> String {
    let mapped: Vec<String> = parts.iter().map(|part| name_of(part, Escape::Full).replace('.', &escaped(u32::from('.')))).collect();
    mapped.join(".")
}

/// The character the escape at the start of `rest`, `_x` then `digits`
/// hexits then `_`, stands for, if it is one of an assigned character
/// [§9.3 GR2a i, GR2b i]; how many characters the escape is.
fn unescaped(rest: &[char], digits: usize, table: Option<&Table>) -> Option<(char, usize)> {
    let width = digits.checked_add(3)?;
    let escape = rest.get(..width)?;
    let [first, second, hexits @ .., last] = escape else { return None };
    if *first != '_' || *second != 'x' || *last != '_' || !hexits.iter().all(char::is_ascii_hexdigit) {
        return None;
    }
    let code = u32::from_str_radix(&hexits.iter().collect::<String>(), 16).ok()?;
    let c = char::from_u32(code)?;
    table.is_some_and(|t| t.assigned(c)).then_some((c, width))
}

/// [§9.3] The SQL identifier the XML Name `name` maps to: each escape
/// `_xHHHH_` or `_xHHHHHH_` of an assigned character [GR2a i, GR2b i]
/// read as that character, every other character as it is \[GR2c\].
/// \[GR2a ii, GR2b ii\] implementation-defined: an escape of a code point
/// that is no assigned character is kept as it is written.
#[must_use]
pub fn identifier_of(name: &str) -> String {
    let table = Table::read().ok();
    let chars: Vec<char> = name.chars().collect();
    let mut identifier = String::new();
    let mut at = 0_usize;
    while let Some(&c) = chars.get(at) {
        let rest = chars.get(at..).unwrap_or_default();
        let read = unescaped(rest, 4, table.as_ref()).or_else(|| unescaped(rest, 6, table.as_ref()));
        let (character, width) = read.unwrap_or((c, 1));
        identifier.push(character);
        at = at.saturating_add(width);
    }
    identifier
}

/// [§9.3 GR5] The `<delimited identifier>` `"SQLI"` of the identifier
/// `identifier`, SQLI: in double quotes, each double quote it holds
/// written as a `<doublequote symbol>` [ISO/IEC 9075:1992 §5.2].
#[must_use]
pub fn delimited(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

#[cfg(test)]
mod tests;

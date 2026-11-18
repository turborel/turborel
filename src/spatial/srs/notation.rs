//! The text forms ST_Angle and ST_Direction share: the keyword and
//! parenthesis tokens of the `<angle text representation>` (ISO/IEC
//! 13249-3:2016 (DIS, SC32 N2593) §16.1.21) and the `<direction text
//! representation>` (§16.2.23), and the GML measures of §16.1.15,
//! §16.1.16, §16.2.6 and §16.2.7, an element whose `uom` attribute names
//! the unit and whose content is the value.

use super::fault::{Condition, Fault, Outcome};
use super::number;
use super::units::{self, Kind};

/// The GML namespace the representations are written in.
pub(super) const GML_NAMESPACE: &str = "http://www.opengis.net/gml";
/// The `uom` of radians, EPSG unit 9101.
pub(super) const RADIAN_UOM: &str = "urn:ogc:def:uom:EPSG::9101";

/// A cursor over a text representation. Keywords match without regard
/// to case, and separators are any white space.
pub(super) struct Scanner<'t> {
    rest: &'t str,
}

impl<'t> Scanner<'t> {
    /// A cursor at the start of `text`.
    pub(super) fn new(text: &'t str) -> Scanner<'t> {
        Scanner { rest: text.trim_start() }
    }

    /// Consumes `word` if it comes next as a whole word.
    pub(super) fn keyword(&mut self, word: &str) -> bool {
        let Some(head) = self.rest.get(..word.len()) else {
            return false;
        };
        let tail = self.rest.get(word.len()..).unwrap_or_default();
        let bounded = !tail.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_');
        if head.eq_ignore_ascii_case(word) && bounded {
            self.rest = tail.trim_start();
            return true;
        }
        false
    }

    /// Consumes the character `symbol` if it comes next.
    pub(super) fn symbol(&mut self, symbol: char) -> bool {
        match self.rest.strip_prefix(symbol) {
            Some(tail) => {
                self.rest = tail.trim_start();
                true
            }
            None => false,
        }
    }

    /// Consumes a `<signed numeric literal>` and returns its finite value.
    pub(super) fn number(&mut self) -> Option<f64> {
        let length = number::literal_length(self.rest)?;
        let value = number::value(self.rest.get(..length)?)?;
        self.rest = self.rest.get(length..)?.trim_start();
        Some(value)
    }

    /// Whether the whole text has been consumed.
    pub(super) fn finished(&self) -> bool {
        self.rest.is_empty()
    }
}

/// The `uom` attribute among the attributes `attributes` of a start tag.
fn uom(attributes: &str) -> Option<&str> {
    let mut rest = attributes;
    while let Some(at) = rest.find("uom") {
        let before = rest.get(..at).unwrap_or_default();
        let after = rest.get(at.checked_add(3)?..)?.trim_start();
        rest = after;
        if !before.is_empty() && !before.ends_with(char::is_whitespace) {
            continue;
        }
        let Some(value) = after.strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        let quote = value.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let body = value.get(1..)?;
        return body.get(..body.find(quote)?);
    }
    None
}

/// The `uom` and content of the first element whose local name is
/// `local`, whatever its namespace prefix, in the GML text `text`.
pub(super) fn element<'t>(text: &'t str, local: &str) -> Option<(Option<&'t str>, &'t str)> {
    for (at, _) in text.match_indices('<') {
        let tag = text.get(at.checked_add(1)?..)?;
        let end = tag.find(|c: char| c.is_whitespace() || c == '>' || c == '/')?;
        let name = tag.get(..end)?;
        if name.rsplit(':').next() != Some(local) {
            continue;
        }
        let close = tag.find('>')?;
        let attributes = tag.get(end..close)?;
        if attributes.ends_with('/') {
            return None;
        }
        let body = tag.get(close.checked_add(1)?..)?;
        let content = body.get(..body.find("</")?)?;
        return Some((uom(attributes), content.trim()));
    }
    None
}

/// Radians per unit of the GML `uom`: an EPSG unit URN or code, or a unit
/// name. No `uom` means radians.
fn angular_factor(uom: Option<&str>) -> Outcome<f64> {
    let Some(uom) = uom else {
        return Ok(1.0_f64);
    };
    let code = uom.rsplit(':').next().unwrap_or(uom);
    let name = match code.to_ascii_lowercase().as_str() {
        "9101" | "rad" => units::RADIAN,
        "9102" | "9122" | "deg" => units::DEGREE,
        "9103" => "ARC-MINUTE",
        "9104" => "ARC-SECOND",
        "9105" | "grad" | "gon" => "GRAD",
        _ => code,
    };
    units::require(name, Kind::Angular).map(|unit| unit.factor)
}

/// The radians the GML measure element `local` in `gml` holds.
///
/// # Errors
///
/// F24 if the element or its value is missing, F14 if its unit is not an
/// angular unit.
pub(super) fn angle_measure(gml: &str, local: &str, rule: &str) -> Outcome<f64> {
    let missing = || Fault::gml(format!("[{rule}] the GML has no {local} element with a numeric value"));
    let (uom, content) = element(gml, local).ok_or_else(missing)?;
    let value = number::literal_length(content)
        .filter(|length| *length == content.len())
        .and_then(|_| number::value(content))
        .ok_or_else(missing)?;
    let factor = angular_factor(uom)?;
    let radians = value * factor;
    if radians.is_finite() { Ok(radians) } else { Err(Fault::new(Condition::InvalidGml, format!("[{rule}] the {local} value overflows"))) }
}

/// A GML measure element `name` of `radians`.
pub(super) fn measure(name: &str, radians: f64) -> String {
    format!("<gml:{name} uom=\"{RADIAN_UOM}\">{}</gml:{name}>", number::shortest(radians))
}

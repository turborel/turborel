//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §17.2.15, §17.2.16, §17.2.22,
//! §17.2.23] The representations of an ST_Vector value: its well-known
//! text, `VECTOR [Z] EMPTY` or `VECTOR [Z] (x y [z])`; its well-known
//! binary, a byte order, the code 101 or 1101 and the doubles; and its
//! GML, a `vector` element of GML 3.2 whose text is the coordinates and
//! whose srsName names its system.
//!
//! The text and binary written are those of the value's dimensions, the
//! binary little endian. An empty value is written `VECTOR EMPTY` and as
//! the code 101 alone, as `NEW ST_Vector()` is the empty set whatever
//! its z [§17.2.22 Description 1c i]. A system is named in GML as
//! `ORGANIZATION:ID` if it has an organization, else by its definition,
//! and a srsName is read as GML of a geometry reads it (`formats::gml`).

use super::ops::{Systems, fault, spatial};
use super::values::Vector;
use crate::error::{Result, SqlState};
use crate::spatial::formats::gml::{NAMESPACE, Srs, srid, xml};
use crate::spatial::srs::{Condition, Fault};

/// The WKB codes of a vector of x and y, and of x, y and z [§17.2.23 f].
const WKB_VECTOR: u32 = 101;
const WKB_VECTOR_Z: u32 = 1101;
/// The octets of a WKB header, the byte order and the code.
const WKB_HEADER: usize = 5;
/// The octets of a double.
const DOUBLE: usize = 8;

/// 2FF22 for text that is no `<vector text representation>`.
fn invalid_text(text: &str) -> crate::error::SqlError {
    fault(&Fault::new(Condition::InvalidText, format!("[§17.2.22] not a <vector text representation>: {text}")))
}

/// 2FF25 mixed coordinate dimensions [§17.2.22 Description 1d].
fn mixed() -> crate::error::SqlError {
    spatial(SqlState::SPATIAL_MIXED_COORDINATE_DIMENSIONS, "§17.2.22 Description 1d", "mixed coordinate dimensions")
}

/// The words and parentheses of `text`.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        if c == '(' || c == ')' || c.is_whitespace() {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
            if !c.is_whitespace() {
                out.push(c.to_string());
            }
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// A `<number>`, an exact or approximate numeric literal, signed.
fn number(word: &str) -> Option<f64> {
    let literal = word.chars().all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E'));
    word.parse::<f64>().ok().filter(|x| literal && x.is_finite())
}

/// [§17.2.22] The vector `text` represents, of SRID `srid`.
///
/// # Errors
///
/// SQLSTATE 2FF22 if it is no `<vector text representation>`; 2FF25 if
/// its z is not as `<just z>` says.
pub(super) fn read_text(text: &str, srid: i32) -> Result<Vector> {
    let words = tokens(text);
    let upper: Vec<String> = words.iter().map(|w| w.to_ascii_uppercase()).collect();
    let mut at = upper.iter().map(String::as_str);
    if at.next() != Some("VECTOR") {
        return Err(invalid_text(text));
    }
    let rest: Vec<&str> = at.collect();
    let (z, rest) = match rest.split_first() {
        Some((&"Z", after)) => (true, after),
        Some(_) | None => (false, rest.as_slice()),
    };
    if rest == ["EMPTY"] {
        return Ok(Vector { srid, ..Vector::EMPTY });
    }
    let [open, inner @ .., close] = rest else { return Err(invalid_text(text)) };
    if (*open, *close) != ("(", ")") {
        return Err(invalid_text(text));
    }
    let coordinates: Vec<f64> = inner.iter().map(|w| number(w)).collect::<Option<_>>().ok_or_else(|| invalid_text(text))?;
    match (coordinates.as_slice(), z) {
        (&[x, y], false) => Ok(Vector { x: Some(x), y: Some(y), z: None, srid }),
        (&[x, y, c], true) => Ok(Vector { x: Some(x), y: Some(y), z: Some(c), srid }),
        (&[_, _, _], false) | (&[_, _], true) => Err(mixed()),
        (_, true | false) => Err(invalid_text(text)),
    }
}

/// A number as text reads it back exactly.
fn shown(x: f64) -> String {
    let plain = format!("{x}");
    if plain.len() > 24 { format!("{x:e}") } else { plain }
}

/// The coordinates of a vector not empty, x, y, then z if it has one.
///
/// # Errors
///
/// SQLSTATE 2FF02 if it is not well formed [§17.2.1 Description 6].
fn coordinates(v: &Vector) -> Result<Option<Vec<f64>>> {
    match (v.x, v.y) {
        (None, None) => Ok(None),
        (Some(x), Some(y)) => Ok(Some([Some(x), Some(y), v.z].into_iter().flatten().collect())),
        (Some(_), None) | (None, Some(_)) => Err(spatial(
            SqlState::SPATIAL_INVALID_ARGUMENT,
            "§17.2.1 Description 6",
            "invalid argument: an ST_Vector value of x without y, or y without x, is not well formed",
        )),
    }
}

/// [§17.2.12] The well-known text of `v`.
///
/// # Errors
///
/// As `coordinates`.
pub(super) fn write_text(v: &Vector) -> Result<String> {
    let Some(c) = coordinates(v)? else { return Ok("VECTOR EMPTY".to_owned()) };
    let z = if v.z.is_some() { "Z " } else { "" };
    Ok(format!("VECTOR {z}({})", c.iter().map(|x| shown(*x)).collect::<Vec<_>>().join(" ")))
}

/// [§17.2.23] The vector `bytes` represent, of SRID `srid`.
///
/// # Errors
///
/// SQLSTATE 2FF23 if they are no `<well-known binary representation>`.
pub(super) fn read_binary(bytes: &[u8], srid: i32) -> Result<Vector> {
    let invalid = || spatial(SqlState::SPATIAL_INVALID_WKB, "§17.2.23", "not a <well-known binary representation> of an ST_Vector value");
    let little = match bytes.first() {
        Some(0) => false,
        Some(1) => true,
        Some(_) | None => return Err(invalid()),
    };
    let word = |at: usize, n: usize| bytes.get(at..at.checked_add(n)?);
    let code: [u8; 4] = word(1, 4).and_then(|w| w.try_into().ok()).ok_or_else(invalid)?;
    let code = if little { u32::from_le_bytes(code) } else { u32::from_be_bytes(code) };
    let count = match code {
        WKB_VECTOR => 2,
        WKB_VECTOR_Z => 3,
        _ => return Err(invalid()),
    };
    if bytes.len() == WKB_HEADER {
        return Ok(Vector { srid, ..Vector::EMPTY });
    }
    let double = |i: usize| -> Option<f64> {
        let octets: [u8; DOUBLE] = word(WKB_HEADER.checked_add(i.checked_mul(DOUBLE)?)?, DOUBLE)?.try_into().ok()?;
        Some(if little { f64::from_le_bytes(octets) } else { f64::from_be_bytes(octets) }).filter(|x| x.is_finite())
    };
    let expected = WKB_HEADER.checked_add(DOUBLE.checked_mul(count).ok_or_else(invalid)?).ok_or_else(invalid)?;
    if bytes.len() != expected {
        return Err(invalid());
    }
    let c: Vec<f64> = (0..count).map(double).collect::<Option<_>>().ok_or_else(invalid)?;
    Ok(Vector { x: c.first().copied(), y: c.get(1).copied(), z: c.get(2).copied(), srid })
}

/// [§17.2.14] The well-known binary of `v`, little endian.
///
/// # Errors
///
/// As `coordinates`.
pub(super) fn write_binary(v: &Vector) -> Result<Vec<u8>> {
    let c = coordinates(v)?;
    let code = if v.z.is_some() && c.is_some() { WKB_VECTOR_Z } else { WKB_VECTOR };
    let mut out = vec![1_u8];
    out.extend_from_slice(&code.to_le_bytes());
    for x in c.unwrap_or_default() {
        out.extend_from_slice(&x.to_le_bytes());
    }
    Ok(out)
}

/// The systems of the catalog and those supplied, as GML names them.
struct Named<'a>(Systems<'a>);

impl Srs for Named<'_> {
    fn by_text(&self, text: &str) -> Option<i32> {
        let supplied = crate::system::SRS_SEED.iter().find(|s| s.definition == text).map(|s| s.id);
        supplied.or_else(|| self.0?.systems().iter().find(|s| s.definition == text).map(|s| s.id))
    }

    fn by_code(&self, authority: &str, code: &str) -> Option<i32> {
        let code: i32 = code.trim().parse().ok()?;
        let supplied = crate::system::SRS_SEED.iter().find(|s| s.organization.eq_ignore_ascii_case(authority) && s.id == code);
        supplied.map(|s| s.id).or_else(|| {
            let of = |s: &&crate::catalog::SpatialRefSysDesc| {
                s.organization.as_ref().is_some_and(|(o, id)| o.eq_ignore_ascii_case(authority) && *id == code)
            };
            self.0?.systems().iter().find(of).map(|s| s.id)
        })
    }

    fn name(&self, srid: i32) -> Option<String> {
        if let Some(s) = crate::system::SpatialRefSys::by_id(srid) {
            return Some(format!("{}:{}", s.organization, s.id));
        }
        let s = self.0?.systems().iter().find(|s| s.id == srid)?;
        Some(s.organization.as_ref().map_or_else(|| s.definition.clone(), |(o, id)| format!("{o}:{id}")))
    }
}

/// [§17.2.15] The vector the GML `gml` represents: of SRID `srid` if one
/// is given, else of the system its srsName names, 0 if it names none.
///
/// # Errors
///
/// SQLSTATE 2FF24 if it is no vector element of two or three numbers;
/// 2FF59 if its srsName names no system known.
pub(super) fn read_gml(gml: &str, srid_given: Option<i32>, systems: Systems<'_>) -> Result<Vector> {
    let root = xml::parse(gml)?;
    if !root.name.eq_ignore_ascii_case("vector") {
        return Err(xml::invalid("expected a vector element"));
    }
    let srid = match srid_given {
        Some(s) => s,
        None => srid(root.attr("srsName"), &Named(systems))?,
    };
    let numbers: Vec<f64> = root
        .text
        .split_whitespace()
        .map(number)
        .collect::<Option<_>>()
        .ok_or_else(|| xml::invalid("a vector's coordinates are numbers"))?;
    match *numbers.as_slice() {
        [x, y] => Ok(Vector { x: Some(x), y: Some(y), z: None, srid }),
        [x, y, z] => Ok(Vector { x: Some(x), y: Some(y), z: Some(z), srid }),
        _ => Err(xml::invalid("a vector has two or three coordinates")),
    }
}

/// [§17.2.16] The GML of `v`; an empty vector has no coordinates.
///
/// # Errors
///
/// As `coordinates`.
pub(super) fn write_gml(v: &Vector, systems: Systems<'_>) -> Result<String> {
    let c = coordinates(v)?.unwrap_or_default();
    let name = if v.srid == 0_i32 { None } else { Named(systems).name(v.srid) };
    let srs = name.map(|n| format!(" srsName=\"{}\"", xml::escape(&n))).unwrap_or_default();
    let text = c.iter().map(|x| shown(*x)).collect::<Vec<_>>().join(" ");
    Ok(format!("<gml:vector xmlns:gml=\"{NAMESPACE}\"{srs}>{text}</gml:vector>"))
}

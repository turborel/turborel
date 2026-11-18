//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.14] The linear
//! referencing well-known text: reading it (`wkt/read.rs`, the
//! ST_...FromText functions and the CHARACTER LARGE OBJECT constructors)
//! and writing it (`wkt/write.rs`, what ST_AsText of §15.1.1 Description
//! 1 and its peers return).
//!
//! §15.14 gives text for ST_PositionExp, ST_LinearElement and its
//! subtypes, ST_LRM and ST_DistanceExp. Its productions for ST_LRMeasure
//! (`<measure representation>`), ST_StartValue (`<start value>`),
//! ST_Referent (`<referent>`) and the three offset expressions
//! (`<lateral offset expression text representation>` and its peers) are
//! read and written as the text of those types too \[EXTENSION\], so every
//! type has one.
//!
//! Implementation-defined choices, as §15.14 leaves a text that is not
//! producible to the implementation [§15.6.9 Description 2a]:
//!
//! - Text not producible in the BNF raises SQLSTATE 2FF22.
//! - `LRM LRMID n` and `LINEARELEMENT LEID n` alone produce an INTEGER
//!   [§15.14.2 Description 1a i, §15.14.3 Description 1c i], no value of
//!   the type, so they are refused at the top.
//! - \[EXTENSION\] What the BNF cannot write is written so: a `<text>`
//!   holding a double quote doubles it; a null linear element type, offset
//!   distance or attribute is the key word NULL; an empty ARRAY is `()`; a
//!   unit name that its `<letters>` would not end is a `<text>`.
//! - A geometry inside the text, `<point text representation>`, `<curve
//!   text representation>` or `<well-known text representation>`, is read
//!   and written by the crate's well-known text of §5.1.67
//!   (`spatial::formats::wkt`). The text carries no SRID, so a geometry
//!   read from it has the unspecified SRID 0.
//! - The `<vector text representation>` the BNF refers to "Subclause
//!   16.2.22" for is that of §17.2.22, where ST_Vector's text is.

mod lex;
mod read;
mod write;

use super::{Catalog, LrKind, LrValue, MAX_TEXT, failed};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::geometry::UNSPECIFIED_SRID;
use crate::geometry::model::Geom;
use crate::spatial::formats::wkt::{read_geom, write_geom};

// ---------------------------------------------------------------------------
// [§15.14.5 Description 1a, §17.2.22 Description 1e, §5.1.67] Key words
// ---------------------------------------------------------------------------

/// DEFAULT.
pub(crate) const DEFAULT: &str = "DEFAULT";
/// DISEXP.
pub(crate) const DISEXP: &str = "DISEXP";
/// E, a topology.
pub(crate) const E: &str = "E";
/// FID.
pub(crate) const FID: &str = "FID";
/// FROM.
pub(crate) const FROM: &str = "FROM";
/// L, a network.
pub(crate) const L: &str = "L";
/// LATERALOFFSET.
pub(crate) const LATERALOFFSET: &str = "LATERALOFFSET";
/// LEID.
pub(crate) const LEID: &str = "LEID";
/// LETYPE.
pub(crate) const LETYPE: &str = "LETYPE";
/// LINEARELEMENT.
pub(crate) const LINEARELEMENT: &str = "LINEARELEMENT";
/// LOCATION.
pub(crate) const LOCATION: &str = "LOCATION";
/// LRCURVE.
pub(crate) const LRCURVE: &str = "LRCURVE";
/// LRDIRECTEDEDGE.
pub(crate) const LRDIRECTEDEDGE: &str = "LRDIRECTEDEDGE";
/// LRFEATURE.
pub(crate) const LRFEATURE: &str = "LRFEATURE";
/// LRM.
pub(crate) const LRM: &str = "LRM";
/// LRMCONSTRAINTS.
pub(crate) const LRMCONSTRAINTS: &str = "LRMCONSTRAINTS";
/// LRMID.
pub(crate) const LRMID: &str = "LRMID";
/// LRMNAME.
pub(crate) const LRMNAME: &str = "LRMNAME";
/// LRMOFFSETUOM.
pub(crate) const LRMOFFSETUOM: &str = "LRMOFFSETUOM";
/// LRMTYPE.
pub(crate) const LRMTYPE: &str = "LRMTYPE";
/// LRMUOM.
pub(crate) const LRMUOM: &str = "LRMUOM";
/// MEASURE.
pub(crate) const MEASURE: &str = "MEASURE";
/// NAME.
pub(crate) const NAME: &str = "NAME";
/// POSEXP.
pub(crate) const POSEXP: &str = "POSEXP";
/// POSITION.
pub(crate) const POSITION: &str = "POSITION";
/// POSLATOFFDIR.
pub(crate) const POSLATOFFDIR: &str = "POSLATOFFDIR";
/// POSVEROFFDIR.
pub(crate) const POSVEROFFDIR: &str = "POSVEROFFDIR";
/// REFERENTS.
pub(crate) const REFERENTS: &str = "REFERENTS";
/// STARTVALUES.
pub(crate) const STARTVALUES: &str = "STARTVALUES";
/// TOWARDS.
pub(crate) const TOWARDS: &str = "TOWARDS";
/// TYPE.
pub(crate) const TYPE: &str = "TYPE";
/// VECTOROFFSETS.
pub(crate) const VECTOROFFSETS: &str = "VECTOROFFSETS";
/// VERTICALOFFSET.
pub(crate) const VERTICALOFFSET: &str = "VERTICALOFFSET";
/// [§17.2.22] VECTOR.
pub(crate) const VECTOR: &str = "VECTOR";
/// [§17.2.22, §5.1.67] EMPTY, which ends a geometry's text.
pub(crate) const EMPTY: &str = "EMPTY";
/// [§17.2.22] Z.
pub(crate) const Z: &str = "Z";
/// \[EXTENSION\] NULL, the null value.
pub(crate) const NULL: &str = "NULL";

/// The key words that end a `<unit of measure>`: those that may follow a
/// measure in the same production.
pub(crate) const STOP_WORDS: [&str; 2] = [FROM, TOWARDS];

/// [§15.1.11, §15.2.9, §15.3.5, §15.4.6, §15.5.6, §15.6.9, §15.9.11] The
/// value of type `expected`, or of a subtype of it, whose well-known text
/// is `text`. Units are checked against `catalog.units`, and a referent's
/// position against its location in `catalog` [§15.10.2 Description 8b].
///
/// # Errors
///
/// SQLSTATE 2FF22 (invalid well-known text representation) of text not
/// producible in the BNF or of another type; the exceptions of the
/// constructors the text calls; SQLCODE -900 past `MAX_TEXT` or
/// `MAX_DEPTH`.
pub fn from_text(text: &str, expected: LrKind, catalog: &Catalog<'_>) -> Result<LrValue> {
    if text.len() > MAX_TEXT {
        return Err(too_long());
    }
    let v = read::value(text, catalog)?;
    if v.kind().is_subtype_of(expected) {
        return Ok(v);
    }
    Err(failed(
        SqlState::SPATIAL_INVALID_WKT,
        "§15.14",
        &format!("invalid well-known text representation: the text is of an {}, not an {}", v.kind().sql_name(), expected.sql_name()),
    ))
}

/// The well-known text of `v`.
///
/// # Errors
///
/// SQLCODE -900 of a value nested past `MAX_DEPTH`, or whose text is
/// past `MAX_TEXT`; as the crate's well-known text of a geometry other
/// than a point or a linestring.
pub fn as_text(v: &LrValue) -> Result<String> {
    let text = write::value(v)?;
    if text.len() > MAX_TEXT {
        return Err(too_long());
    }
    Ok(text)
}

/// [§5.1.67] The geometry of well-known text `text`, of SRID 0.
///
/// # Errors
///
/// As `spatial::formats::wkt::read_geom`.
pub(crate) fn read_geometry(text: &str) -> Result<Geom> {
    read_geom(text, UNSPECIFIED_SRID)
}

/// [§5.1.57] The well-known text of `g`.
///
/// # Errors
///
/// As `spatial::formats::wkt::write_geom`.
pub(crate) fn write_geometry(g: &Geom) -> Result<String> {
    write_geom(g)
}

/// SQLCODE -900: text past ST_MaxLRAsText.
fn too_long() -> SqlError {
    SqlError::new(Code::Unsupported, "[DESIGN] a linear referencing text is longer than ST_MaxLRAsText (RULES.md D85)")
}

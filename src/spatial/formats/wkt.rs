//! The well-known text representation, ISO/IEC 13249-3:2016 (DIS, SC32
//! N2593) §5.1.67: [`read`] for ST_GeomFromText and the type
//! constructors, [`write`] for ST_AsText.

pub mod cursor;
pub mod curves;
pub mod lexer;
mod read;
mod write;

pub use read::read;
pub use write::write;

use crate::error::Result;
use crate::geometry::model::Geom;

/// ST_GeomFromText: the checked value `text` represents, in reference
/// system `srid` (§5.1.62).
///
/// # Errors
///
/// As [`read`]'s and the model's checks'.
pub fn read_geom(text: &str, srid: i32) -> Result<Geom> {
    super::model::to_geom(&read(text)?, srid)
}

/// ST_AsText: the canonical text of `g` (§5.1.57).
///
/// # Errors
///
/// As [`super::model::from_geom`]'s.
pub fn write_geom(g: &Geom) -> Result<String> {
    Ok(write(&super::model::from_geom(g)?))
}

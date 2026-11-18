//! GML, ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.60, §5.1.61 and
//! Table 14: [`read_geom`] for ST_GMLToSQL and ST_GeomFromGML,
//! [`write_geom`] for ST_AsGML. The reference system catalog is reached
//! through [`Srs`], which the caller gives.

pub mod curves;
mod read;
mod write;
pub mod xml;

pub use read::{Srs, read, srid};
pub use write::{NAMESPACE, read_back_dims, write};

use crate::error::Result;
use crate::geometry::model::Geom;

/// The checked value `text` represents, its SRID from its srsName.
///
/// # Errors
///
/// SQLSTATE 2FF24 for a document that is not GML of Table 14, 2FF59 for
/// an unknown reference system, and the model's checks' errors.
pub fn read_geom(text: &str, srs: &dyn Srs) -> Result<Geom> {
    let (value, name) = read(text)?;
    super::model::to_geom(&value, srid(name.as_deref(), srs)?)
}

/// The GML of `g`, with the srsName of its SRID unless that is 0.
///
/// # Errors
///
/// As [`super::model::from_geom`]'s.
pub fn write_geom(g: &Geom, srs: &dyn Srs) -> Result<String> {
    let name = if g.srid == 0_i32 { None } else { srs.name(g.srid) };
    Ok(write(&super::model::from_geom(g)?, name.as_deref()))
}

#[cfg(test)]
mod tests;

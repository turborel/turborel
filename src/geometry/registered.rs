//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.11, §20.2] A column
//! associated with a spatial reference system holds only values of that
//! system. The association is the column's row of ST_GEOMETRY_COLUMNS,
//! made by ST_REGISTERGEOMETRYCOLUMN, whose system SRS_SUPPORTED keeps
//! defined. §4.11 gives the rule as a CHECK constraint of the column,
//! `COL.ST_SRID() = SRS_ID`. It is checked as each row is written, and of
//! the rows a column holds when it is registered, an integrity
//! constraint violation (SQLSTATE 23000) if a value is of another system.
//!
//! A column of `GEOMETRY(srid)` is associated with that SRID by its type,
//! which assignment checks (`reference.rs`). No rule of the standard
//! asks that SRID, or a value's, to name a system defined: an SRID is
//! implementation-dependent [§14.1.5 Description 2] and ST_SRID(INTEGER)
//! gives a value any [§5.1.5 Description 5]. Only a lookup of a system,
//! as ST_Transform's or a unit form's, asks it.

use super::kind::payload;
use super::model::header;
use crate::catalog::{Catalog, QualifiedName, SpatialCatalog, TableDesc};
use crate::error::{Code, Result, SqlError};
use crate::storage::Row;

/// That each value of `row`, a row of `table`, is of the system its
/// column is registered with, if it is registered with one.
///
/// # Errors
///
/// SQLCODE -201, SQLSTATE 23000, if one is of another; -916 if a stored
/// value is malformed.
pub(crate) fn check_row(catalog: &dyn Catalog, table: &QualifiedName, row: &Row) -> Result<()> {
    let spatial = catalog.spatial();
    if spatial.columns().is_empty() {
        return Ok(());
    }
    let Some(desc) = catalog.table(table) else { return Ok(()) };
    check_row_of(spatial, desc, row)
}

/// As `check_row`, of `desc`'s row `row` by the registrations of
/// `spatial`.
///
/// # Errors
///
/// As `check_row`.
pub(crate) fn check_row_of(spatial: &SpatialCatalog, desc: &TableDesc, row: &Row) -> Result<()> {
    for registered in spatial.columns().iter().filter(|c| c.table == desc.name) {
        let Some(system) = registered.srs.as_deref() else { continue };
        let Some(id) = spatial.system_id(system) else { continue };
        let at = desc.columns.iter().position(|c| c.name == registered.column);
        let Some(bytes) = at.and_then(|at| row.get(at)).and_then(payload) else { continue };
        let (srid, _) = header(bytes)?;
        if srid != id {
            return Err(SqlError::new(
                Code::Constraint,
                format!(
                    "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §4.11] column {} of {} holds values of {system}, SRID {id}, not of SRID {srid}",
                    registered.column, desc.name
                ),
            ));
        }
    }
    Ok(())
}

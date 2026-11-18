//! \[EXTENSION\] The procedures that define and drop spatial reference
//! systems and register geometry columns with them: the rows of
//! ST_SPATIAL_REFERENCE_SYSTEMS and ST_GEOMETRY_COLUMNS [ISO/IEC
//! 13249-3:2016 (DIS, SC32 N2593) §20.2, §20.3], which the standard
//! describes and gives no statement to change. Each is invoked by CALL
//! and changes the catalog in the caller's transaction, as a definition
//! does (RULES.md D121):
//!
//! - `ST_DEFINESRS(name, id, organization, organization id, definition,
//!   description)`, its definition a `<spatial reference system>`
//!   [§14.1.9], else 2FF22 when it is called;
//! - `ST_DROPSRS(name)`;
//! - `ST_REGISTERGEOMETRYCOLUMN(schema, table, column, system name)`;
//! - `ST_UNREGISTERGEOMETRYCOLUMN(schema, table, column)`.

use std::sync::Arc;

use crate::catalog::{GeometryColumnDesc, QualifiedName, SpatialChange, SpatialRefSysDesc};
use crate::error::{Code, Result, SqlError};
use crate::functions::builtin::{Procedure, ProcedureContext};
use crate::lexer::MAX_IDENTIFIER_LENGTH;
use crate::system::{DEFINITION_LENGTH, ORGANIZATION_LENGTH, SRS_DESCRIPTION_LENGTH, SRS_NAME_LENGTH};
use crate::value::{DataType, Spelled, Value};

const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

/// A procedure of the spatial catalog.
#[derive(Debug, Clone, Copy)]
pub enum CatalogProcedure {
    /// Defines a system.
    DefineSrs,
    /// Drops a system.
    DropSrs,
    /// Registers a column, with a system or none.
    Register,
    /// Removes a column's registration.
    Unregister,
}

/// The procedures, built in.
pub fn procedures() -> Vec<Arc<dyn Procedure>> {
    [CatalogProcedure::DefineSrs, CatalogProcedure::DropSrs, CatalogProcedure::Register, CatalogProcedure::Unregister]
        .into_iter()
        .map(|p| -> Arc<dyn Procedure> { Arc::new(p) })
        .chain(super::support::procedures())
        .collect()
}

/// A character string of at most `n` characters.
fn varying(n: u32) -> DataType {
    DataType::varying(n)
}

/// An identifier's type.
fn identifier() -> DataType {
    varying(u32::try_from(MAX_IDENTIFIER_LENGTH).unwrap_or(u32::MAX))
}

/// The text `v` holds, none if it is null.
fn text(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::Char(s)) => Some(s.clone()),
        Some(_) | None => None,
    }
}

/// The integer `v` holds, none if it is null.
fn integer(v: Option<&Value>) -> Option<i32> {
    let Some(Value::Exact(d)) = v else { return None };
    if d.scale != 0 {
        return None;
    }
    i32::try_from(d.units).ok()
}

/// [§20.3] A null where a NOT NULL column of the table takes it.
fn required(what: &str) -> SqlError {
    SqlError::new(Code::Constraint, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.3] {what} is not null"))
}

impl CatalogProcedure {
    /// [§20.3] The system `args` define.
    fn system(args: &[Value]) -> Result<SpatialRefSysDesc> {
        let name = text(args.first()).ok_or_else(|| required("SRS_NAME"))?;
        let id = integer(args.get(1)).ok_or_else(|| required("SRS_ID"))?;
        let organization = match (text(args.get(2)), integer(args.get(3))) {
            (Some(org), Some(org_id)) => Some((org, org_id)),
            (None, None) => None,
            (Some(_), None) | (None, Some(_)) => {
                let msg = "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.3] ORGANIZATION_NULL: ORGANIZATION and ORGANIZATION_COORDSYS_ID are both null or neither";
                return Err(SqlError::new(Code::Constraint, msg));
            }
        };
        let definition = text(args.get(4)).ok_or_else(|| required("DEFINITION"))?;
        // [§14.1.9] Nothing unreadable is stored: the definition is a
        // `<spatial reference system>`, else 2FF22.
        crate::spatial::srs::system::SpatialRefSys::from_text(&definition).map_err(|f| super::routines::fault(&f))?;
        Ok(SpatialRefSysDesc { name, id, organization, definition, description: text(args.get(5)) })
    }

    /// The table and column `args` name, then the rest.
    fn column(args: &[Value]) -> Result<(QualifiedName, String)> {
        let part = |at: usize, what: &str| text(args.get(at)).ok_or_else(|| required(what));
        Ok((QualifiedName { auth: part(0, "TABLE_SCHEMA")?, ident: part(1, "TABLE_NAME")? }, part(2, "COLUMN_NAME")?))
    }
}

impl Procedure for CatalogProcedure {
    fn name(&self) -> &str {
        match self {
            Self::DefineSrs => "ST_DEFINESRS",
            Self::DropSrs => "ST_DROPSRS",
            Self::Register => "ST_REGISTERGEOMETRYCOLUMN",
            Self::Unregister => "ST_UNREGISTERGEOMETRYCOLUMN",
        }
    }

    fn parameters(&self, count: usize) -> Option<Vec<DataType>> {
        let types = match self {
            Self::DefineSrs => vec![
                varying(SRS_NAME_LENGTH),
                INTEGER,
                varying(ORGANIZATION_LENGTH),
                INTEGER,
                varying(DEFINITION_LENGTH),
                varying(SRS_DESCRIPTION_LENGTH),
            ],
            Self::DropSrs => vec![varying(SRS_NAME_LENGTH)],
            Self::Register => vec![identifier(), identifier(), identifier(), varying(SRS_NAME_LENGTH)],
            Self::Unregister => vec![identifier(), identifier(), identifier()],
        };
        (types.len() == count).then_some(types)
    }

    fn call(&self, args: &[Value], cx: &mut dyn ProcedureContext) -> Result<()> {
        let change = match self {
            Self::DefineSrs => SpatialChange::Define(Self::system(args)?),
            Self::DropSrs => SpatialChange::Drop(text(args.first()).ok_or_else(|| required("SRS_NAME"))?),
            Self::Register => {
                let (table, column) = Self::column(args)?;
                SpatialChange::Register(GeometryColumnDesc { table, column, srs: text(args.get(3)) })
            }
            Self::Unregister => {
                let (table, column) = Self::column(args)?;
                SpatialChange::Unregister(table, column)
            }
        };
        cx.define_spatial(change)
    }
}

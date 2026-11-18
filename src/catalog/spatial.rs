//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.2, §20.3] The spatial
//! reference systems defined in a database, beyond those it supplies
//! (`system/spatial/systems.rs`), and the geometry columns registered
//! with a system: the rows of ST_SPATIAL_REFERENCE_SYSTEMS and
//! ST_GEOMETRY_COLUMNS made by the procedures that define them
//! (`geometry/procedures.rs`), each checked against those tables'
//! constraints (RULES.md D121). And of §21, the linear
//! referencing methods defined and the columns registered with one
//! (`spatial/lr.rs`).

mod lr;

pub use lr::{LrColumnDesc, LrmDesc};

use super::QualifiedName;
use crate::error::{Code, Result, SqlError};
use crate::system::{SRS_SEED, SpatialRefSys};

/// A spatial reference system defined: a row of
/// ST_SPATIAL_REFERENCE_SYSTEMS [§20.3].
#[derive(Debug, Clone, PartialEq)]
pub struct SpatialRefSysDesc {
    /// SRS_NAME, its primary key.
    pub name: String,
    /// SRS_ID, unique.
    pub id: i32,
    /// ORGANIZATION and ORGANIZATION_COORDSYS_ID, both or neither.
    pub organization: Option<(String, i32)>,
    /// DEFINITION, its well-known text representation.
    pub definition: String,
    /// DESCRIPTION.
    pub description: Option<String>,
}

/// A geometry column registered: a row of ST_GEOMETRY_COLUMNS [§20.2].
#[derive(Debug, Clone, PartialEq)]
pub struct GeometryColumnDesc {
    /// The table.
    pub table: QualifiedName,
    /// The column, of a geometry type.
    pub column: String,
    /// SRS_NAME, the system associated with it; none if none is.
    pub srs: Option<String>,
}

/// A change to the systems defined and the columns registered.
#[derive(Debug, Clone, PartialEq)]
pub enum SpatialChange {
    /// A system defined.
    Define(SpatialRefSysDesc),
    /// The system of the name dropped.
    Drop(String),
    /// A column registered, in place of its registration if it had one.
    Register(GeometryColumnDesc),
    /// The column's registration removed.
    Unregister(QualifiedName, String),
    /// [§21.2.3] A linear referencing method defined.
    DefineLrm(LrmDesc),
    /// [§21.2.3] The linear referencing method of the name dropped.
    DropLrm(String),
    /// [§21.2.2] A column of a linear referencing type registered, in
    /// place of its registration if it had one.
    RegisterLr(LrColumnDesc),
    /// [§21.2.2] The column's registration removed.
    UnregisterLr(QualifiedName, String),
}

/// The systems defined, in the order of their identifiers, and the
/// columns registered, in the order of their tables and names.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpatialCatalog {
    systems: Vec<SpatialRefSysDesc>,
    columns: Vec<GeometryColumnDesc>,
    /// [§21.2.3] The linear referencing methods, in the order of their
    /// identifiers.
    lrms: Vec<LrmDesc>,
    /// [§21.2.2] The columns registered with one, in the order of their
    /// tables and names.
    lr_columns: Vec<LrColumnDesc>,
}

/// An integrity constraint violation of the definition schema's table
/// `table` (SQLSTATE 23000).
fn violated(table: &str, msg: &str) -> SqlError {
    SqlError::new(Code::Constraint, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20] {table}: {msg}"))
}

impl SpatialCatalog {
    /// It of `systems` and `columns`, each in its order.
    pub fn of(mut systems: Vec<SpatialRefSysDesc>, mut columns: Vec<GeometryColumnDesc>) -> SpatialCatalog {
        systems.sort_by_key(|s| s.id);
        columns.sort_by(|a, b| (&a.table.auth, &a.table.ident, &a.column).cmp(&(&b.table.auth, &b.table.ident, &b.column)));
        SpatialCatalog { systems, columns, lrms: Vec::new(), lr_columns: Vec::new() }
    }

    /// The systems defined.
    pub fn systems(&self) -> &[SpatialRefSysDesc] {
        &self.systems
    }

    /// The columns registered.
    pub fn columns(&self) -> &[GeometryColumnDesc] {
        &self.columns
    }

    /// The registration of `column` of `table`, if it has one.
    pub fn column(&self, table: &QualifiedName, column: &str) -> Option<&GeometryColumnDesc> {
        self.columns.iter().find(|c| &c.table == table && c.column == column)
    }

    /// It after `change`.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, if the change breaks a constraint of
    /// ST_SPATIAL_REFERENCE_SYSTEMS [§20.3] or ST_GEOMETRY_COLUMNS
    /// [§20.2]; -103 of a system supplied, which is not dropped.
    pub fn applied(&self, change: &SpatialChange) -> Result<SpatialCatalog> {
        let mut next = self.clone();
        match change {
            SpatialChange::Define(d) => {
                self.definable(d)?;
                next.systems.push(d.clone());
            }
            SpatialChange::Drop(name) => {
                self.droppable(name)?;
                next.systems.retain(|s| &s.name != name);
            }
            SpatialChange::Register(c) => {
                if let Some(name) = &c.srs
                    && !self.has_system(name)
                {
                    return Err(violated("ST_GEOMETRY_COLUMNS", &format!("SRS_SUPPORTED: no spatial reference system {name}")));
                }
                next.columns.retain(|o| o.table != c.table || o.column != c.column);
                next.columns.push(c.clone());
            }
            SpatialChange::Unregister(table, column) => next.columns.retain(|o| &o.table != table || &o.column != column),
            SpatialChange::DefineLrm(_) | SpatialChange::DropLrm(_) | SpatialChange::RegisterLr(_) | SpatialChange::UnregisterLr(..) => {
                return self.lr_applied(change);
            }
        }
        Ok(SpatialCatalog::of(next.systems, next.columns).with_lr(next.lrms, next.lr_columns))
    }

    /// Whether a system of the name `name` is supplied or defined.
    pub fn has_system(&self, name: &str) -> bool {
        self.system_id(name).is_some()
    }

    /// The identifier of the system of the name `name`, supplied or
    /// defined, if there is one.
    pub fn system_id(&self, name: &str) -> Option<i32> {
        let seeded = SRS_SEED.iter().find(|s| s.name == name).map(|s| s.id);
        seeded.or_else(|| self.systems.iter().find(|s| s.name == name).map(|s| s.id))
    }

    /// The name of the system of identifier `id`, supplied or defined, if
    /// there is one.
    pub fn system_name(&self, id: i32) -> Option<&str> {
        SpatialRefSys::by_id(id).map(|s| s.name.as_str()).or_else(|| self.systems.iter().find(|s| s.id == id).map(|s| s.name.as_str()))
    }

    /// [§20.3] That `d` breaks none of ST_SPATIAL_REFERENCE_SYSTEMS's
    /// constraints with the systems there are.
    fn definable(&self, d: &SpatialRefSysDesc) -> Result<()> {
        let table = "ST_SPATIAL_REFERENCE_SYSTEMS";
        if self.has_system(&d.name) {
            return Err(violated(table, &format!("ST_SRS_NAME_PRIMARY_KEY: a system is named {}", d.name)));
        }
        if SpatialRefSys::by_id(d.id).is_some() || self.systems.iter().any(|s| s.id == d.id) {
            return Err(violated(table, &format!("SRS_ID_UNIQUE: a system has the identifier {}", d.id)));
        }
        if let Some((org, id)) = &d.organization {
            let seeded = SRS_SEED.iter().any(|s| s.organization == org.as_str() && s.id == *id);
            if seeded || self.systems.iter().any(|s| s.organization.as_ref() == Some(&(org.clone(), *id))) {
                return Err(violated(table, &format!("ORGANIZATION_UNIQUE: {org} gave a system the identifier {id}")));
            }
        }
        Ok(())
    }

    /// That the system named `name` is defined and no column is
    /// registered with it [§20.2 SRS_SUPPORTED].
    fn droppable(&self, name: &str) -> Result<()> {
        if SRS_SEED.iter().any(|s| s.name == name) {
            return Err(SqlError::new(
                Code::SyntaxRule,
                format!(
                    "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.3] {name} is a system this implementation supplies, which is not dropped"
                ),
            ));
        }
        if !self.systems.iter().any(|s| s.name == name) {
            return Err(SqlError::new(
                Code::SyntaxRule,
                format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.3] no spatial reference system {name} is defined"),
            ));
        }
        if let Some(c) = self.columns.iter().find(|c| c.srs.as_deref() == Some(name)) {
            return Err(violated(
                "ST_GEOMETRY_COLUMNS",
                &format!("SRS_SUPPORTED: column {} of {} is registered with {name}", c.column, c.table),
            ));
        }
        Ok(())
    }

    /// It without the registrations of `table`'s columns not among
    /// `kept`: those of a table dropped, or of columns dropped [§20.2
    /// COLUMN_EXISTS].
    pub fn forget_columns(&mut self, table: &QualifiedName, kept: &[&str]) {
        self.columns.retain(|c| &c.table != table || kept.contains(&c.column.as_str()));
        self.lr_columns.retain(|c| &c.table != table || kept.contains(&c.column.as_str()));
    }
}

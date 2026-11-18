//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §21.2] The linear referencing
//! methods defined and the columns of a linear referencing type
//! registered with one: the rows of ST_LRMS and ST_LR_COLUMNS made by the
//! procedures that define them (`spatial/lr/sql/catalog.rs`), each
//! checked against those tables' constraints.

use super::{QualifiedName, SpatialCatalog, SpatialChange};
use crate::error::{Code, Result, SqlError};

/// A linear referencing method defined: a row of ST_LRMS [§21.2.3].
#[derive(Debug, Clone, PartialEq)]
pub struct LrmDesc {
    /// LRM_NAME, unique.
    pub name: String,
    /// LRM_ID, its primary key.
    pub id: i32,
    /// ORGANIZATION and ORGANIZATION_LRM_ID, both or neither.
    pub organization: Option<(String, i32)>,
    /// DEFINITION, its well-known text representation.
    pub definition: String,
    /// DESCRIPTION.
    pub description: Option<String>,
    /// LRM, the ST_LRM value DEFINITION represents, in its stored form
    /// (`spatial/lr/codec.rs`).
    pub lrm: Vec<u8>,
}

/// A column of a linear referencing type registered: a row of
/// ST_LR_COLUMNS [§21.2.2].
#[derive(Debug, Clone, PartialEq)]
pub struct LrColumnDesc {
    /// The table.
    pub table: QualifiedName,
    /// The column.
    pub column: String,
    /// LRM_ID, the method associated with it; none if none is.
    pub lrm: Option<i32>,
}

/// An integrity constraint violation of `table` (SQLSTATE 23000).
fn violated(table: &str, msg: &str) -> SqlError {
    SqlError::new(Code::Constraint, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §21.2] {table}: {msg}"))
}

impl SpatialCatalog {
    /// It with the methods `lrms` and the registrations `columns`, each
    /// put in its order.
    #[must_use]
    pub fn with_lr(mut self, mut lrms: Vec<LrmDesc>, mut columns: Vec<LrColumnDesc>) -> SpatialCatalog {
        lrms.sort_by_key(|l| l.id);
        columns.sort_by(|a, b| (&a.table.auth, &a.table.ident, &a.column).cmp(&(&b.table.auth, &b.table.ident, &b.column)));
        self.lrms = lrms;
        self.lr_columns = columns;
        self
    }

    /// The linear referencing methods defined.
    pub fn lrms(&self) -> &[LrmDesc] {
        &self.lrms
    }

    /// The columns registered with a linear referencing method or none.
    pub fn lr_columns(&self) -> &[LrColumnDesc] {
        &self.lr_columns
    }

    /// The registration of `column` of `table`, if it has one.
    pub fn lr_column(&self, table: &QualifiedName, column: &str) -> Option<&LrColumnDesc> {
        self.lr_columns.iter().find(|c| &c.table == table && c.column == column)
    }

    /// It after `change`, a change of §21.
    ///
    /// # Errors
    ///
    /// SQLCODE -201, SQLSTATE 23000, if the change breaks a constraint of
    /// ST_LRMS [§21.2.3] or ST_LR_COLUMNS [§21.2.2]; -103 of a method not
    /// defined.
    pub(super) fn lr_applied(&self, change: &SpatialChange) -> Result<SpatialCatalog> {
        let mut next = self.clone();
        match change {
            SpatialChange::DefineLrm(d) => {
                self.lrm_definable(d)?;
                next.lrms.push(d.clone());
            }
            SpatialChange::DropLrm(name) => {
                self.lrm_droppable(name)?;
                next.lrms.retain(|l| &l.name != name);
            }
            SpatialChange::RegisterLr(c) => {
                if let Some(id) = c.lrm
                    && !self.lrms.iter().any(|l| l.id == id)
                {
                    return Err(violated("ST_LR_COLUMNS", &format!("LRM_SUPPORTED: no linear referencing method has the identifier {id}")));
                }
                next.lr_columns.retain(|o| o.table != c.table || o.column != c.column);
                next.lr_columns.push(c.clone());
            }
            SpatialChange::UnregisterLr(table, column) => next.lr_columns.retain(|o| &o.table != table || &o.column != column),
            SpatialChange::Define(_) | SpatialChange::Drop(_) | SpatialChange::Register(_) | SpatialChange::Unregister(..) => {
                return self.applied(change);
            }
        }
        let (lrms, columns) = (next.lrms.clone(), next.lr_columns.clone());
        Ok(next.with_lr(lrms, columns))
    }

    /// [§21.2.3] That `d` breaks none of ST_LRMS's constraints with the
    /// methods there are.
    fn lrm_definable(&self, d: &LrmDesc) -> Result<()> {
        let table = "ST_LRMS";
        if self.lrms.iter().any(|l| l.id == d.id) {
            return Err(violated(table, &format!("LRM_ID_PRIMARY_KEY: a method has the identifier {}", d.id)));
        }
        if self.lrms.iter().any(|l| l.name == d.name) {
            return Err(violated(table, &format!("LRM_NAME_UNIQUE: a method is named {}", d.name)));
        }
        if let Some((org, id)) = &d.organization
            && self.lrms.iter().any(|l| l.organization.as_ref() == Some(&(org.clone(), *id)))
        {
            return Err(violated(table, &format!("ORGANIZATION_UNIQUE: {org} gave a method the identifier {id}")));
        }
        Ok(())
    }

    /// That the method named `name` is defined and no column is
    /// registered with it [§21.2.2 LRM_SUPPORTED].
    fn lrm_droppable(&self, name: &str) -> Result<()> {
        let Some(lrm) = self.lrms.iter().find(|l| l.name == name) else {
            return Err(SqlError::new(
                Code::SyntaxRule,
                format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §21.2.3] no linear referencing method {name} is defined"),
            ));
        };
        if let Some(c) = self.lr_columns.iter().find(|c| c.lrm == Some(lrm.id)) {
            return Err(violated("ST_LR_COLUMNS", &format!("LRM_SUPPORTED: column {} of {} is registered with {name}", c.column, c.table)));
        }
        Ok(())
    }
}

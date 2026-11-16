//! [ISO/IEC 9075-2:2023 §11.5 GR3] The default of a column of a view. A
//! view's column is of no domain and has no default of its own, so one
//! with a single counterpart column in a leaf generally underlying table
//! takes that column's default, its own or its domain's [GR3d], and any
//! other is null [GR3e]. The counterpart is found through the view's bound
//! plan (`LogicalPlan::counterparts`), a column of a base table.
//!
//! A column whose counterpart is an identity column or a generated column
//! is left unassigned [GR3a]. Such a column has no default, so the view's
//! has none either. Only the INSTEAD OF trigger of a view, which writes no
//! row of the base table, is given such a row, so nothing gives it a value
//! and it is null (`db/triggers/rerouted.rs`, `db/merge/routed.rs`).

use super::Purpose;
use super::views::{Named, Reader, Source, named};
use crate::catalog::{ColumnDefault, ColumnDesc, Principal, QualifiedName};
use crate::db::State;
use crate::error::Result;
use crate::settings;
use crate::value::datetime::Clocked;

/// [ISO/IEC 9075-2:2023 §11.5 GR3d] The columns of view `name`, as `user`
/// reads it, each with the default it takes: that of its single
/// counterpart column of a base table, if it has one; else none.
///
/// # Errors
///
/// `name` is not defined, or a view it reads breaks a rule.
pub fn defaulted_columns(db: &State, name: &QualifiedName, user: Principal<'_>) -> Result<Vec<ColumnDesc>> {
    let reader = Reader {
        purpose: Purpose::Query,
        own: &settings::NONE,
        user,
        definitions: false,
        now: Clocked::new(db.now(&settings::NONE), None),
    };
    let Named { desc, source } = named(db, name, reader)?;
    Ok(defaulted(db, desc.columns, &source))
}

/// The columns `columns` of a table read from `source`, each with the
/// default it takes, as `defaulted_columns` gives them.
pub(super) fn defaulted(db: &State, columns: Vec<ColumnDesc>, source: &Source) -> Vec<ColumnDesc> {
    let counterparts: Vec<Option<ColumnDesc>> = match source {
        // A base table's columns have their own.
        Source::Table => return columns,
        Source::Derived(plan) => plan
            .counterparts()
            .into_iter()
            .map(|c| c.and_then(|(table, k)| db.catalog.table(&table).and_then(|t| t.columns.get(k)).cloned()))
            .collect(),
        // An updatable view's column is a column of its base table.
        Source::Base(vb) => vb.map.iter().map(|&k| vb.base.columns.get(k).cloned()).collect(),
        Source::Common | Source::System(_) | Source::Unnest(_) | Source::Argument => Vec::new(),
    };
    columns
        .into_iter()
        .enumerate()
        .map(|(i, c)| ColumnDesc { default: counterparts.get(i).and_then(|cc| taken(cc.as_ref()?)), ..c })
        .collect()
}

/// The default a column of a view takes of `cc`, its counterpart column:
/// its own, else its domain's [§11.5 GR2a, GR2b].
fn taken(cc: &ColumnDesc) -> Option<ColumnDefault> {
    cc.default.clone().or_else(|| cc.domain.as_ref().and_then(|d| d.default.clone()))
}

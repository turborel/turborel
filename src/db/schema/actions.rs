//! The privileges a GRANT or REVOKE of a table's `<action>`s specifies
//! [X3.135-1986 §6.6; ISO/IEC 9075:1992 §10.3].

use crate::ast::Action;
use crate::binder::no_column;
use crate::catalog::{Actions, TableDesc};
use crate::error::Result;

use super::rule;

/// [X3.135-1986 §6.6 SR1, SR2; ISO/IEC 9075:1992 §10.3 SR4] The
/// privileges the `<action>`s `list` specify on `table`: INSERT, UPDATE or
/// REFERENCES on each column of a column list, which are columns of the
/// table, or on every column without one.
pub(in crate::db) fn granted_actions(list: &[Action], table: &TableDesc) -> Result<Actions> {
    let mut actions = Actions::default();
    for a in list {
        match a {
            Action::Select => actions.select = true,
            Action::Insert(None) => actions.insert.extend(0..table.columns.len()),
            Action::Insert(Some(names)) => actions.insert.extend(columns_of(table, names)?),
            Action::Delete => actions.delete = true,
            Action::Update(None) => actions.update.extend(0..table.columns.len()),
            Action::Update(Some(names)) => actions.update.extend(columns_of(table, names)?),
            Action::References(None) => actions.references.extend(0..table.columns.len()),
            Action::References(Some(names)) => actions.references.extend(columns_of(table, names)?),
            // [ISO/IEC 9075:1992 §11.36 SR1] USAGE is a domain's.
            Action::Usage => {
                return rule(format!("[ISO/IEC 9075:1992 §11.36 SR1] USAGE is granted on a domain, not on table {}", table.name));
            }
            Action::Execute => {
                return rule(format!("[ISO/IEC 9075-2:2023 §12.3 SR4b] EXECUTE is granted on a routine, not on table {}", table.name));
            }
            Action::Under => {
                return rule(format!("[ISO/IEC 9075-2:2023 §12.3 SR4c] UNDER on a table is of a typed table, which {} is not", table.name));
            }
        }
    }
    Ok(actions)
}

/// The positions of the columns `names` of `table` [X3.135-1986 §6.6 SR2,
/// ISO/IEC 9075:1992 §10.3 SR3].
fn columns_of(table: &TableDesc, names: &[String]) -> Result<Vec<usize>> {
    names
        .iter()
        .map(|c| {
            table.column(c).ok_or_else(|| no_column(&table.name, format!("[X3.135-1986 §6.6 SR2] {} has no column {c}", table.name), c))
        })
        .collect()
}

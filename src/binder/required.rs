//! The privileges a statement requires on a table [X3.135-1986 §6.6
//! SR3], and the table and column it reports if they are not held
//! [ISO/IEC 9075:1992 §18.1 GR3g].

use crate::catalog::{Actions, TableDesc, Who, applicable};
use crate::db::State;
use crate::error::{Code, Result, SqlError, Subject};

/// [X3.135-1986 §6.6 SR3] Whether the current privileges of `who` on
/// `table` include `needed`, as the Syntax Rule `clause` (e.g. "X3.135-1986 §8.7 SR1"), cited in full,
/// requires.
///
/// # Errors
///
/// SQLCODE -104 if they do not.
pub fn require(db: &State, who: Who<'_>, table: &TableDesc, needed: &Actions, clause: &str) -> Result<()> {
    let (held, _) = applicable(&*db.catalog, who, &table.name);
    let columns: Vec<String> = table.columns.iter().map(|c| c.name.clone()).collect();
    match held.missing(needed, &columns) {
        None => Ok(()),
        Some(missing) => {
            // [ISO/IEC 9075:1992 §18.1 GR3g] its table, and its column, if inaccessible.
            let column = held.missing_column(needed).and_then(|at| columns.get(at).cloned());
            let subject = Subject { column, ..Subject::table(table.name.parts()) };
            Err(SqlError::new(
                Code::Privilege,
                format!("[{clause}; X3.135-1986 §6.6 SR3] {} has no {missing} privilege on {}", who.auth, table.name),
            )
            .about(subject))
        }
    }
}

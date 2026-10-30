//! [ISO/IEC 9075:1992 §11.9 AR1] The REFERENCES a constraint's condition
//! needs: on each column it names of each table it reads \[AR1a\]; on one
//! column at least of each, but only where it names no column at all
//! \[AR1b\]. Which case holds is known once the whole condition is bound, so
//! the tables read without any REFERENCES are noted as it is, and refused
//! then. Split from binder.rs (RULES.md R-SIZE-2).

use std::cell::{Cell, RefCell};

use super::Context;
use crate::catalog::{QualifiedName, applicable};
use crate::error::{Code, Result, SqlError, Subject};

/// What binding a constraint's condition finds: whether it names a
/// column, and the tables it reads on which it holds no REFERENCES.
#[derive(Debug, Default)]
pub struct Referencing {
    /// A table whose columns are not columns named: the one a domain
    /// constraint is bound over, VALUE its column [§6.2 SR5].
    value: Option<QualifiedName>,
    named: Cell<bool>,
    unheld: RefCell<Vec<QualifiedName>>,
}

impl Referencing {
    /// Of a domain constraint bound over `value`, VALUE its one column,
    /// which names no column.
    pub fn of_value(value: &QualifiedName) -> Referencing {
        Referencing { value: Some(value.clone()), ..Referencing::default() }
    }

    /// The condition names a column of `table`.
    pub(super) fn column_named(&self, table: &QualifiedName) {
        if self.value.as_ref() != Some(table) {
            self.named.set(true);
        }
    }

    /// \[AR1b\] That `auth`, binding the condition, holds REFERENCES on a
    /// column at least of each table it reads, if it names no column.
    ///
    /// # Errors
    ///
    /// SQLCODE -104, of the first table it does not.
    pub fn check(&self, auth: &str) -> Result<()> {
        if self.named.get() {
            return Ok(());
        }
        match self.unheld.borrow().first() {
            Some(name) => Err(unreferenced(auth, name)),
            None => Ok(()),
        }
    }
}

/// \[AR1\] The refusal of `name` to `auth`, read by a constraint without REFERENCES.
fn unreferenced(auth: &str, name: &QualifiedName) -> SqlError {
    SqlError::new(Code::Privilege, format!("[ISO/IEC 9075:1992 §11.9 AR1] {auth} has no REFERENCES privilege on {name}"))
        .about(Subject::table(name.parts()))
}

/// \[AR1\] The table `name`, read by the constraint `cx` binds: noted in
/// `cx.referencing` if `cx.auth` holds no REFERENCES on it, to be refused
/// if the condition names no column.
///
/// # Errors
///
/// SQLCODE -104 if it holds none and there is nowhere to note it.
pub(super) fn table_read(cx: &Context<'_>, name: &QualifiedName) -> Result<()> {
    if !applicable(&*cx.db.catalog, cx.auth, name).0.references.is_empty() {
        return Ok(());
    }
    let Some(referencing) = cx.referencing else { return Err(unreferenced(cx.auth, name)) };
    referencing.unheld.borrow_mut().push(name.clone());
    Ok(())
}

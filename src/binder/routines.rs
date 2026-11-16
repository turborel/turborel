//! [ISO/IEC 9075-2:2023 §10.4, §9.18] Routine invocation, of the two
//! functions each distinct type's definition makes [§11.51 GR2]: FNUDT,
//! from its source type, and FNSDT, to it (RULES.md D69). A name not
//! qualified is found in the schema the statement's names are of, the
//! SQL-path [§9.18 SR10b] (ID047); a routine of the name is found before
//! a function registered [EXTENSION] (RULES.md D53). Which function of a
//! name a call invokes is chosen among these and the schema's SQL-invoked
//! functions (`invocation/candidates.rs`).
//!
//! Each function's body is the change of representation its cast is
//! [§11.51 GR2b], which none is, a distinct type's value being its
//! source's (UV103): FNUDT binds as its argument assigned to the source
//! type, and FNSDT as its argument, typed as the source.

use super::invocation::{Prebound, functions_named};
use super::{Binder, Scope, rule};
use crate::ast::ValueExpression;
use crate::catalog::{QualifiedName, Routine, UserTypeDesc, owner_of};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::logical::Expr;
use crate::params::ParameterType;
use crate::value::DataType;

/// A function a distinct type's definition makes: its type, and which.
type Function<'a> = (&'a UserTypeDesc, Routine);

/// Whether a call of `name` of no schema, in a statement whose names are
/// of schema `path`, is of a function of the schema, not of an aggregate
/// registered: an SQL-invoked function's or a distinct type's of the name.
pub(super) fn names_routine(db: &State, path: &str, name: &str) -> bool {
    !functions_named(db, &QualifiedName { auth: path.to_owned(), ident: name.to_owned() }).is_empty()
}

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §9.18 SR10l iii, §20.7 SR xxx, §11.51 GR2b]
    /// The invocation of `f`, a distinct type's function, with `args`, the
    /// one argument bound as `bound` unless a `?`, which takes the
    /// parameter's type: FNUDT its argument assigned to the source type,
    /// typed as the distinct type; FNSDT its argument, typed as the source.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the argument cannot be stored in the parameter.
    pub(super) fn cast_invocation(
        &self,
        f: Function<'_>,
        args: &[ValueExpression],
        bound: Prebound,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let (t, which) = f;
        let parameter = match which {
            Routine::ToDistinct => t.source.clone(),
            Routine::ToSource => t.data_type(),
        };
        let (e, given) = match (bound.into_iter().next().flatten(), args.first()) {
            (Some(done), _) => done,
            (None, Some(arg)) => self.typed(arg, ParameterType::Of(parameter.clone()), scope)?,
            (None, None) => {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10a] function {} takes one argument", t.routine(which).name)));
            }
        };
        if !super::statements::assignable(&parameter, &given) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §9.18 SR10l iii, §9.2 SR2] the parameter of function {} is of {parameter}, which an argument of {given} cannot be stored in",
                t.routine(which).name
            )));
        }
        let specific = QualifiedName { auth: t.name.auth.clone(), ident: t.routine(which).specific.clone() };
        super::invocation::note_specific(self.invoked, &specific);
        Ok(match which {
            Routine::ToDistinct => {
                let assigned = if t.source == given { e } else { Expr::Cast(Box::new(e), t.source.clone()) };
                (assigned, t.data_type())
            }
            Routine::ToSource => (e, t.source.clone()),
        })
    }

    /// Whether the statement's authorization identifier holds EXECUTE on
    /// `f` [ISO/IEC 9075-2:2023 §9.18 SR6a, SR6b]: the owner of the schema
    /// it is of, of a statement there, as the view or constraint binds it.
    fn holds_execute(&self, (t, which): Function<'_>) -> bool {
        t.execute_of(which, self.auth, owner_of(&*self.db.catalog, &t.name.auth)).0
    }

    /// [ISO/IEC 9075-2:2023 §6.13 AR2] That the statement's authorization
    /// identifier holds EXECUTE on the cast function of CAST from `from`
    /// to `to`, if a distinct type's: FNUDT to it, FNSDT from it
    /// [§11.51 GR2d, GR2e]. A CAST of a type to itself invokes none.
    ///
    /// # Errors
    ///
    /// SQLCODE -104 if it does not; -103 if the type is no longer there.
    pub(super) fn cast_function(&self, from: &DataType, to: &DataType) -> Result<()> {
        if from == to {
            return Ok(());
        }
        let (d, which) = if let DataType::Distinct(d) = to {
            (d, Routine::ToDistinct)
        } else if let DataType::Distinct(d) = from {
            (d, Routine::ToSource)
        } else {
            return Ok(());
        };
        let qn = QualifiedName { auth: d.schema.clone(), ident: d.name.clone() };
        let t = self.db.catalog.user_type(&qn).ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §6.1] there is no type {qn}")))?;
        if self.holds_execute((t, which)) {
            return Ok(());
        }
        let msg =
            format!("[ISO/IEC 9075-2:2023 §6.13 AR2] {} holds no EXECUTE on function {}.{}", self.auth, qn.auth, t.routine(which).name);
        Err(SqlError::new(Code::Privilege, msg))
    }
}

//! [ISO/IEC 9075-2:2023 §10.4, §9.18] Routine invocation, of the two
//! functions each distinct type's definition makes [§11.51 GR2]: FNUDT,
//! from its source type, and FNSDT, to it (RULES.md D69). A name not
//! qualified is found in the schema the statement's names are of, the
//! SQL-path [§9.18 SR10b] (ID047); a routine of the name is found before
//! a function registered [EXTENSION] (RULES.md D53).
//!
//! Each function's body is the change of representation its cast is
//! [§11.51 GR2b], which none is, a distinct type's value being its
//! source's (UV103): FNUDT binds as its argument assigned to the source
//! type, and FNSDT as its argument, typed as the source.

use super::{Binder, Scope, rule};
use crate::ast::ValueExpression;
use crate::catalog::{QualifiedName, Routine, UserTypeDesc, owner_of};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::logical::Expr;
use crate::params::ParameterType;
use crate::value::DataType;
use crate::value::precedence::in_precedence_list;

/// A function a distinct type's definition makes: its type, and which.
type Function<'a> = (&'a UserTypeDesc, Routine);

/// The declared type of the one SQL parameter of `f` [§11.51 GR2a].
fn parameter((t, which): Function<'_>) -> DataType {
    match which {
        Routine::ToDistinct => t.source.clone(),
        Routine::ToSource => t.data_type(),
    }
}

/// [ISO/IEC 9075-2:2023 §9.7] The place of `p`'s designator in a type
/// precedence list it is in, among those a parameter of the type of
/// another function may be: a fixed-length string's before a varying
/// one's, an exact number's before an approximate one's, both before
/// DECFLOAT's [SR5 to SR9]. One designator alone is in any other list.
fn rank(p: &DataType) -> u8 {
    match p {
        DataType::Varying { .. } | DataType::VarBinary { .. } | DataType::Approximate { .. } => 1,
        DataType::DecFloat { .. } => 2,
        DataType::Character { .. }
        | DataType::Binary { .. }
        | DataType::Exact { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Extension(_) => 0,
    }
}

/// Whether a call of `name` of no schema, in a statement whose names are
/// of schema `path`, is of a routine: none of a function registered.
pub(super) fn names_routine(db: &State, path: &str, name: &str) -> bool {
    db.catalog.user_types().into_iter().any(|t| t.name.auth == path && (t.to_distinct.name == name || t.to_source.name == name))
}

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §9.18 SR5f, SR10b] The functions named `name`
    /// of schema `schema`, else of the SQL-path: the possibly candidate
    /// routines of a call.
    fn functions_named(&self, schema: Option<&str>, name: &str) -> Vec<Function<'_>> {
        let schema = schema.unwrap_or(self.schema);
        let mut found = Vec::new();
        for t in self.db.catalog.user_types().into_iter().filter(|t| t.name.auth == schema) {
            found.extend([Routine::ToDistinct, Routine::ToSource].into_iter().filter(|w| t.routine(*w).name == name).map(|w| (t, w)));
        }
        found
    }

    /// [ISO/IEC 9075-2:2023 §10.4] The `<routine invocation>` `name`
    /// (`args`), of schema `schema` if written, if it names a routine.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if no function of the name takes `args` [§9.18 SR10],
    /// or more than one alike [§9.6]; -104 if those that do are none the
    /// statement's authorization identifier holds EXECUTE on [SR6].
    pub(super) fn routine_call(
        &self,
        (schema, name): (Option<&str>, &str),
        args: &[ValueExpression],
        scope: Scope<'_>,
    ) -> Result<Option<(Expr, DataType)>> {
        let named = self.functions_named(schema, name);
        if named.is_empty() {
            return Ok(None);
        }
        let shown = schema.map_or_else(|| name.to_owned(), |s| format!("{s}.{name}"));
        // [SR10a] each function takes one argument.
        let [arg] = args else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10a] function {shown} takes one argument, not {}", args.len())));
        };
        // [SR10h i] a `?` takes the parameter's type, of one function alone.
        let (e, t) = match named.as_slice() {
            [one] => self.typed(arg, ParameterType::Of(parameter(*one)), scope)?,
            _ if super::expression::dynamic_of(arg).is_some() => {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §9.18 SR10h] a ? argument of {shown} has no type, as more than one function is of the name"
                )));
            }
            _ => self.value_expression(arg, scope)?,
        };
        let f = self.subject_routine(&named, &t, &shown)?;
        Ok(Some(match f.1 {
            Routine::ToDistinct => {
                let source = &f.0.source;
                let assigned = if *source == t { e } else { Expr::Cast(Box::new(e), source.clone()) };
                (assigned, f.0.data_type())
            }
            Routine::ToSource => (e, f.0.source.clone()),
        }))
    }

    /// [ISO/IEC 9075-2:2023 §9.18 SR6, SR10d, SR10h; §9.6] Of `named`, the
    /// function a call of an argument of type `t` invokes: of those whose
    /// parameter's type designator is in `t`'s type precedence list, and
    /// to whose parameter `t` is assignable, the one whose designator is
    /// first in it; of those, the ones the statement's authorization
    /// identifier holds EXECUTE on.
    fn subject_routine<'f>(&self, named: &[Function<'f>], t: &DataType, shown: &str) -> Result<Function<'f>> {
        let takes = |f: &Function<'_>| in_precedence_list(&parameter(*f), t) && super::statements::assignable(&parameter(*f), t);
        let candidates: Vec<Function<'f>> = named.iter().copied().filter(takes).collect();
        let executable: Vec<Function<'f>> = candidates.iter().copied().filter(|f| self.holds_execute(*f)).collect();
        let first = executable.iter().map(|f| rank(&parameter(*f))).min();
        let subject: Vec<Function<'f>> = executable.iter().copied().filter(|f| Some(rank(&parameter(*f))) == first).collect();
        match (subject.as_slice(), candidates.is_empty()) {
            ([one], _) => Ok(*one),
            ([], true) => Err(rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10h iv] no function {shown} takes an argument of {t}"))),
            ([], false) => {
                let msg =
                    format!("[ISO/IEC 9075-2:2023 §9.18 SR6] {} holds EXECUTE on no function {shown} of an argument of {t}", self.auth);
                Err(SqlError::new(Code::Privilege, msg))
            }
            (_, _) => Err(rule(format!("[ISO/IEC 9075-2:2023 §9.6] more than one function {shown} takes an argument of {t} alike"))),
        }
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

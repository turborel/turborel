//! [ISO/IEC 9075-2:2023 §9.18 SR5f, SR6, SR10, §9.6] The functions of a
//! name in a schema, the possibly candidate routines of a `<routine
//! invocation>`: the SQL-invoked functions of an SQL routine body (Feature
//! T321) and the two each distinct type's definition makes [§11.51 GR2];
//! and of those, overloads of one name (Feature T341), the one a call
//! invokes, by the types of its arguments (RULES.md D74).
//!
//! A definition's invocations are of the routines it invoked as it was
//! defined [§4.35.2] (RULES.md D73): bound again, its candidates are those
//! of its horizon, held or not, as no other is a subject routine of it.

use super::super::{Binder, rule};
use crate::catalog::{QualifiedName, Routine, RoutineKind, SqlRoutineDesc, UserTypeDesc, owner_of};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::value::DataType;
use crate::value::precedence::{subject_routines, takes, told_apart};

/// A function of a name in a schema: an SQL-invoked function of an SQL
/// routine body, or one of a distinct type's.
#[derive(Debug, Clone, Copy)]
pub enum Candidate<'a> {
    /// An SQL-invoked function of an SQL routine body.
    Sql(&'a SqlRoutineDesc),
    /// FNUDT or FNSDT of a distinct type.
    Cast(&'a UserTypeDesc, Routine),
}

impl Candidate<'_> {
    /// The declared types of its SQL parameters.
    pub fn parameters(&self) -> Vec<DataType> {
        match self {
            Candidate::Sql(r) => r.parameter_types(),
            // [§11.51 GR2b] FNUDT's of the source type, FNSDT's of the type.
            Candidate::Cast(t, Routine::ToDistinct) => vec![t.source.clone()],
            Candidate::Cast(t, Routine::ToSource) => vec![t.data_type()],
        }
    }

    /// Its specific name.
    pub fn specific(&self) -> QualifiedName {
        match self {
            Candidate::Sql(r) => r.specific.clone(),
            Candidate::Cast(t, which) => QualifiedName { auth: t.name.auth.clone(), ident: t.routine(*which).specific.clone() },
        }
    }

    /// [ISO/IEC 9075-2:2023 §9.18 SR6, §12.3] Whether `auth` holds EXECUTE
    /// on it in `db`: the owner of its schema does [§11.60 GR1]; another if
    /// it was granted it or PUBLIC.
    pub fn executable(&self, db: &State, auth: &str) -> bool {
        match self {
            Candidate::Sql(r) => auth == owner_of(&*db.catalog, &r.name.auth) || r.execute_of(auth).0,
            Candidate::Cast(t, which) => t.execute_of(*which, auth, owner_of(&*db.catalog, &t.name.auth)).0,
        }
    }
}

/// [ISO/IEC 9075-2:2023 §9.18 SR5f] The functions named `name`: its
/// schema's SQL-invoked functions of the name, then its distinct types'.
pub fn functions_named<'a>(db: &'a State, name: &QualifiedName) -> Vec<Candidate<'a>> {
    let mut found: Vec<Candidate<'a>> =
        db.catalog.routines_named(name).into_iter().filter(|r| r.kind == RoutineKind::Function).map(Candidate::Sql).collect();
    for t in db.catalog.user_types().into_iter().filter(|t| t.name.auth == name.auth) {
        let named = [Routine::ToDistinct, Routine::ToSource].into_iter().filter(|w| t.routine(*w).name == name.ident);
        found.extend(named.map(|w| Candidate::Cast(t, w)));
    }
    found
}

/// [ISO/IEC 9075-2:2023 §11.60 SR20b] The function of `name` that a
/// function of it whose parameters are of the types `parameters` would
/// not be told apart from, if any: as an invocation of arguments of those
/// types would have both as subject routines.
pub fn alike<'a>(db: &'a State, name: &QualifiedName, parameters: &[DataType]) -> Option<Candidate<'a>> {
    let others = functions_named(db, name);
    let types: Vec<Vec<DataType>> = others.iter().map(Candidate::parameters).collect();
    let lists: Vec<&[DataType]> = types.iter().map(Vec::as_slice).collect();
    if told_apart(parameters, &lists) {
        return None;
    }
    let args: Vec<Option<DataType>> = parameters.iter().cloned().map(Some).collect();
    let taking: Vec<usize> = (0..lists.len()).filter(|&i| lists.get(i).is_some_and(|l| takes(l, &args))).collect();
    let mut set: Vec<&[DataType]> = vec![parameters];
    set.extend(taking.iter().filter_map(|&i| lists.get(i).copied()));
    let subjects = subject_routines(&set, &args);
    let other = subjects.into_iter().find(|&k| k != 0).and_then(|k| taking.get(k.checked_sub(1)?));
    other.and_then(|&i| others.get(i).copied())
}

/// The types of arguments `args`, as messages show them: `?` of each
/// `<dynamic parameter specification>`.
pub(in crate::binder) fn shown_types(args: &[Option<DataType>]) -> String {
    args.iter().map(|a| a.as_ref().map_or_else(|| "?".to_owned(), ToString::to_string)).collect::<Vec<_>>().join(", ")
}

/// Which function of a name an invocation invokes, if one of its schema.
pub(in crate::binder) enum Chosen<'a> {
    /// This one.
    One(Candidate<'a>),
    /// None of the schema, so a function registered of the name may be
    /// [§9.18 SR10h iv 1 A]; if none is, this refusal.
    Refused(SqlError),
    /// None, a definition having invoked none of the name.
    Nothing,
}

impl Binder<'_> {
    /// The subject routines its invocations are of, a definition's fixed
    /// as it was defined [§4.35.2]; none of a statement's: of a triggered
    /// action or a procedure's body, its statements' horizon.
    pub(in crate::binder) fn subjects(&self) -> Option<&[QualifiedName]> {
        if self.params.horizoned() { self.params.horizon() } else { self.horizon }.subjects()
    }

    /// [ISO/IEC 9075-2:2023 §9.18 SR6, SR10d, SR10h iv; §9.6] Of `named`,
    /// the functions of the name `shown`, the one an invocation with
    /// arguments of the types `args` invokes: of those that take them on
    /// which the statement's authorization identifier holds EXECUTE, the
    /// one subject routine. Of a definition, of those it invoked as it was
    /// defined (`subjects`), held or not.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if more than one is alike [§9.6], or a routine a
    /// definition invoked is dropped; -104 if the subject routine of a
    /// definition is one it holds no EXECUTE on.
    pub(in crate::binder) fn subject_function<'c>(
        &self,
        named: Vec<Candidate<'c>>,
        args: &[Option<DataType>],
        shown: &str,
    ) -> Result<Chosen<'c>> {
        let pool = match self.subjects() {
            Some(fixed) => {
                self.all_there(fixed)?;
                let pool: Vec<Candidate<'c>> = named.into_iter().filter(|c| fixed.contains(&c.specific())).collect();
                if pool.is_empty() {
                    return Ok(Chosen::Nothing);
                }
                pool
            }
            None => named,
        };
        let fixed = self.subjects().is_some();
        let fitting: Vec<Candidate<'c>> = pool.iter().copied().filter(|c| takes(&c.parameters(), args)).collect();
        let held: Vec<Candidate<'c>> = fitting.iter().copied().filter(|c| fixed || c.executable(self.db, self.auth)).collect();
        let types: Vec<Vec<DataType>> = held.iter().map(Candidate::parameters).collect();
        let lists: Vec<&[DataType]> = types.iter().map(Vec::as_slice).collect();
        match subject_routines(&lists, args).as_slice() {
            [one] => self.held_subject(held.get(*one).copied(), shown),
            [] if !fitting.is_empty() => Ok(Chosen::Refused(SqlError::new(
                Code::Privilege,
                format!(
                    "[ISO/IEC 9075-2:2023 §9.18 SR6] {} holds EXECUTE on no function {shown} of arguments of ({})",
                    self.auth,
                    shown_types(args)
                ),
            ))),
            [] if !pool.iter().any(|c| c.parameters().len() == args.len()) => {
                Ok(Chosen::Refused(rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10a] no function {shown} has {} parameters", args.len()))))
            }
            [] => Ok(Chosen::Refused(rule(format!(
                "[ISO/IEC 9075-2:2023 §9.18 SR10d] no function {shown} of {} parameters takes arguments of ({})",
                args.len(),
                shown_types(args)
            )))),
            [_, _, ..] => Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §9.18 SR10h iv 3, §9.6] more than one function {shown} takes arguments of ({}) alike",
                shown_types(args)
            ))),
        }
    }

    /// `c`, the one subject routine: of a definition, one the statement's
    /// authorization identifier holds EXECUTE on.
    ///
    /// # Errors
    ///
    /// SQLCODE -104 if it does not hold it [§9.18 SR6, §12.7 GR12o].
    fn held_subject<'c>(&self, c: Option<Candidate<'c>>, shown: &str) -> Result<Chosen<'c>> {
        let Some(c) = c else { return Ok(Chosen::Nothing) };
        if !c.executable(self.db, self.auth) {
            let msg = format!("[ISO/IEC 9075-2:2023 §9.18 SR6] {} holds no EXECUTE on function {shown}", self.auth);
            return Err(SqlError::new(Code::Privilege, msg));
        }
        Ok(Chosen::One(c))
    }

    /// That each routine of specific names `fixed`, a definition's subject
    /// routines, is there: one dropped is dropped with what invokes it, or
    /// not at all [§11.62 SR4c, GR1], so a definition without it binds no
    /// more, never to another function of its name.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one is not.
    fn all_there(&self, fixed: &[QualifiedName]) -> Result<()> {
        let there = |s: &QualifiedName| {
            self.db.catalog.routine(s).is_some()
                || self.db.catalog.user_types().into_iter().any(|t| {
                    t.name.auth == s.auth && [Routine::ToDistinct, Routine::ToSource].iter().any(|w| t.routine(*w).specific == s.ident)
                })
        };
        match fixed.iter().find(|s| !there(s)) {
            Some(gone) => Err(rule(format!("[ISO/IEC 9075-2:2023 §11.62 GR1] the routine of specific name {gone} it invokes is dropped"))),
            None => Ok(()),
        }
    }
}

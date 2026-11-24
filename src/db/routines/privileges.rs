//! [ISO/IEC 9075-2:2023 §12.2, §12.3, §12.7, §10.6] EXECUTE on an
//! SQL-invoked routine of an SQL routine body: granted, and revoked, with
//! what a revoke abandons [§12.7 GR12]. Each view, constraint, assertion,
//! generation expression, trigger and routine whose owner needs what it no
//! longer holds is found by binding each again as the revoke leaves the
//! privileges (`user_types/uses.rs`). A GRANT or REVOKE that designates no
//! such routine is of a distinct type's (`user_types/usage.rs`).
//!
//! \[EXTENSION\] DEBUG on such a routine is granted and revoked as EXECUTE
//! is, in a list of its own, and its revoke abandons nothing, as no
//! definition needs it (RULES.md D1000).

use std::sync::Arc;

use super::super::definitions::Defined;
use super::super::generated::Column;
use super::super::revoke::Abandoned;
use super::super::{Completion, State, rule};
use crate::ast::{Action, DropBehavior, PrivilegeDefinition, PrivilegeObject, RevokeStatement, RoutineType, TableName, TypeName};
use crate::binder::qualify;
use crate::catalog::{Catalog, QualifiedName, Routine, RoutineKind, SqlRoutineDesc, UsageDesc, UserTypeDesc, owner_of, sole};
use crate::error::{At, Code, Result, SqlError, SqlState};
use crate::system::SYSTEM_GRANTOR;
use crate::value::DataType;

/// What a GRANT or REVOKE names: its object, its kind, its routine's
/// `<data type list>` if written, and its actions, or ALL PRIVILEGES.
pub(in crate::db) type Named<'a> = (&'a TableName, PrivilegeObject, Option<&'a [TypeName]>, Option<&'a [Action]>);

/// A privilege of a routine: EXECUTE, or \[EXTENSION\] DEBUG (RULES.md
/// D1000).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Privilege {
    Execute,
    Debug,
}

impl Privilege {
    /// The privileges `actions` names, of a routine: of ALL PRIVILEGES,
    /// EXECUTE, the standard's privileges alone, so the extension changes
    /// no statement of the standard (RULES.md D1002).
    fn named(actions: Option<&[Action]>) -> Vec<Privilege> {
        let Some(list) = actions else { return vec![Privilege::Execute] };
        [Privilege::Execute, Privilege::Debug].into_iter().filter(|p| list.contains(&p.action())).collect()
    }

    /// Its `<action>`.
    fn action(self) -> Action {
        match self {
            Privilege::Execute => Action::Execute,
            Privilege::Debug => Action::Debug,
        }
    }

    /// Its key word.
    fn word(self) -> &'static str {
        match self {
            Privilege::Execute => "EXECUTE",
            Privilege::Debug => "DEBUG",
        }
    }

    /// Its grants on `desc`.
    fn grants(self, desc: &SqlRoutineDesc) -> &Vec<UsageDesc> {
        match self {
            Privilege::Execute => &desc.execute,
            Privilege::Debug => &desc.debug,
        }
    }

    /// Its grants on `desc`, to change.
    fn grants_mut(self, desc: &mut SqlRoutineDesc) -> &mut Vec<UsageDesc> {
        match self {
            Privilege::Execute => &mut desc.execute,
            Privilege::Debug => &mut desc.debug,
        }
    }

    /// Whether `who` holds it on `desc` in `catalog`, and whether
    /// grantably.
    fn of(self, catalog: &dyn Catalog, desc: &SqlRoutineDesc, who: crate::catalog::Who<'_>) -> (bool, bool) {
        match self {
            Privilege::Execute => desc.execute_of(who),
            Privilege::Debug => desc.debug_of(who, owner_of(catalog, &desc.name.auth)),
        }
    }
}

/// EXECUTE on a routine as a REVOKE leaves it.
struct Revoked {
    /// The routine's descriptor, the grants revoked and those they took
    /// gone.
    desc: SqlRoutineDesc,
    /// A grantee named held no grant of it from the revoker [§12.7 GR4].
    warned: bool,
    /// Grants not named went with those that were [GR9].
    lost: bool,
    /// What its revoking abandons.
    abandoned: Abandoned,
}

/// [ISO/IEC 9075-2:2023 §10.6 SR4] Whether a `<routine type>` of `wanted`
/// designates the routine `r`: METHOD of its kind an SQL-invoked method
/// alone [SR2], FUNCTION a function no method [§11.60 SR5].
fn of_type(wanted: RoutineType, r: &SqlRoutineDesc) -> bool {
    match (wanted, r.kind) {
        (RoutineType::Routine, RoutineKind::Function | RoutineKind::Procedure) | (RoutineType::Procedure, RoutineKind::Procedure) => true,
        (RoutineType::Function, RoutineKind::Function) => r.method.is_none(),
        (RoutineType::Method(kind), RoutineKind::Function) => r.method.as_ref().is_some_and(|(_, k)| *k == kind),
        (RoutineType::Function, RoutineKind::Procedure)
        | (RoutineType::Procedure, RoutineKind::Function)
        | (RoutineType::Method(_), RoutineKind::Procedure) => false,
    }
}

impl State {
    /// [ISO/IEC 9075-2:2023 §10.6] The SQL-invoked routine of an SQL
    /// routine body a GRANT or REVOKE by `auth` names, its names in schema
    /// `schema`, if it names a routine and one has the name: by its
    /// specific name \[SR1\], else of its name, the one of it of its
    /// `<routine type>` [SR3c ii, SR4], or the one whose parameters' types
    /// are those of its `<data type list>` [SR3c i, §9.30]. None if what it
    /// designates is a distinct type's function (`type_functions`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if not exactly one of its type is designated, of the
    /// SQL-invoked routines and the distinct types' functions together
    /// [SR3c, SR4], or it grants or revokes another privilege [§12.3 SR4d].
    pub(in crate::db) fn sql_routine_named(&self, (auth, schema): (&str, &str), named: Named<'_>) -> Result<Option<SqlRoutineDesc>> {
        let (name, object, parameters, actions) = named;
        let (specific, wanted) = match object {
            PrivilegeObject::SpecificRoutine(t) => (true, t),
            PrivilegeObject::Routine(t) => (false, t),
            PrivilegeObject::Table
            | PrivilegeObject::Domain
            | PrivilegeObject::Collation
            | PrivilegeObject::Sequence
            | PrivilegeObject::Module
            | PrivilegeObject::Graph
            | PrivilegeObject::Model
            | PrivilegeObject::XmlSchema
            | PrivilegeObject::XQueryModule
            | PrivilegeObject::Type => return Ok(None),
        };
        let qn = qualify(name, schema);
        let mut found: Vec<&SqlRoutineDesc> =
            self.catalog.routines().into_iter().filter(|r| if specific { r.specific == qn } else { r.name == qn }).collect();
        if found.is_empty() {
            return Ok(None);
        }
        // [EXTENSION] RULES.md D1000: DEBUG beside EXECUTE.
        if actions.is_some_and(|list| list.iter().any(|a| !matches!(a, Action::Execute | Action::Debug))) {
            return Err(rule("[ISO/IEC 9075-2:2023 §12.3 SR4d] of a routine, EXECUTE or DEBUG alone is granted or revoked".to_owned())
                .or_at(name.at));
        }
        let types = match parameters {
            Some(list) => Some(
                list.iter()
                    .map(|t| self.data_type_of(t, (&qn.auth, sole(&*self.catalog, auth))))
                    .collect::<Result<Vec<_>>>()
                    .or_at(name.at)?,
            ),
            None => None,
        };
        let casts = self.type_functions(&qn, (specific, Some(wanted)), types.as_deref());
        found.retain(|r| of_type(wanted, r));
        if found.is_empty() && casts.is_empty() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §10.6 SR4] routine {qn} is not of the <routine type> named")).or_at(name.at));
        }
        if let Some(types) = &types {
            found.retain(|r| r.parameter_types() == *types);
        }
        match (found.as_slice(), casts.len()) {
            ([one], 0) => Ok(Some((*one).clone())),
            ([], 1) => Ok(None),
            ([] | [_, ..], _) => {
                Err(rule(format!("[ISO/IEC 9075-2:2023 §10.6 SR3c] {qn} designates not exactly one routine")).or_at(name.at))
            }
        }
    }

    /// [ISO/IEC 9075-2:2023 §10.6 SR1, SR3c, SR4] The functions of
    /// distinct types `qn` designates: by their specific names, if
    /// `specific`, else by their names; none if `routine_type` is
    /// PROCEDURE, as none is a procedure, or there is none; of those, if
    /// `types` is given, the ones of one parameter of its one type [§9.30].
    pub(in crate::db) fn type_functions(
        &self,
        qn: &QualifiedName,
        (specific, routine_type): (bool, Option<RoutineType>),
        types: Option<&[DataType]>,
    ) -> Vec<(UserTypeDesc, Routine)> {
        let functions = routine_type.is_some_and(|t| t != RoutineType::Procedure);
        let mut found = Vec::new();
        for t in self.catalog.user_types().into_iter().filter(|t| functions && t.name.auth == qn.auth) {
            for which in [Routine::ToDistinct, Routine::ToSource] {
                let routine = t.routine(which);
                let named = if specific { routine.specific == qn.ident } else { routine.name == qn.ident };
                let parameter = if which == Routine::ToDistinct { t.source.clone() } else { t.data_type() };
                if named && types.is_none_or(|list| list == std::slice::from_ref(&parameter)) {
                    found.push((t.clone(), which));
                }
            }
        }
        found
    }

    /// [ISO/IEC 9075-2:2023 §12.2] GRANT EXECUTE, or DEBUG, `p` on the
    /// routine `desc` by `auth`: each privilege granted if `auth` holds it
    /// grantably, else a warning \[GR9\].
    ///
    /// # Errors
    ///
    /// SQLCODE -104 if `auth` holds no privilege named on it \[AR1\].
    pub(in crate::db) fn grant_on_routine(&mut self, auth: &str, mut desc: SqlRoutineDesc, p: &PrivilegeDefinition) -> Result<Defined> {
        of_schema_level(&desc).or_at(p.table.at)?;
        let mut ungranted = false;
        for privilege in Privilege::named(p.actions.as_deref()) {
            let (held, grantable) = privilege.of(&*self.catalog, &desc, sole(&*self.catalog, auth));
            if !held {
                let msg = format!("[ISO/IEC 9075-2:2023 §12.2 AR1] {auth} holds no {} on routine {}", privilege.word(), desc.name);
                return Err(SqlError::new(Code::Privilege, msg).or_at(p.table.at));
            }
            if !grantable {
                ungranted = true;
                continue;
            }
            let grants = privilege.grants_mut(&mut desc);
            for grantee in &p.grantees {
                match grants.iter_mut().find(|u| u.grantor == auth && u.grantee == *grantee) {
                    Some(u) => u.grantable |= p.grant_option,
                    None => grants.push(UsageDesc { grantor: auth.to_owned(), grantee: grantee.clone(), grantable: p.grant_option }),
                }
            }
        }
        self.put_routine(desc);
        if ungranted {
            return Ok(Defined { completion: Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED), ..Defined::default() });
        }
        Ok(Defined::default())
    }

    /// [ISO/IEC 9075-2:2023 §12.7] REVOKE EXECUTE, or DEBUG, `r` on the routine `desc`
    /// by `auth`: the grants `auth` made those it names, or their grant
    /// option alone, gone; with CASCADE, what that abandons gone too, the
    /// generated columns it loses gone already
    /// (`Session::dropping_generations`). A warning if `auth` granted it to
    /// one it names not \[GR4\].
    ///
    /// # Errors
    ///
    /// With RESTRICT, SQLCODE -103 and SQLSTATE 2B000 if it abandons
    /// anything \[GR12q\]; as each drop's.
    pub(in crate::db) fn revoke_on_routine(&mut self, auth: &str, desc: SqlRoutineDesc, r: &RevokeStatement) -> Result<Defined> {
        of_schema_level(&desc).or_at(r.table.at)?;
        let revoked = self.routine_revoked(auth, desc, r);
        if r.behavior == DropBehavior::Restrict && (revoked.lost || !revoked.abandoned.is_empty()) {
            let what = if revoked.abandoned.is_empty() { "a grant of it".to_owned() } else { revoked.abandoned.first() };
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §12.7 GR12q] revoking {} on routine {} abandons {what}: revoke it with CASCADE",
                words(r.actions.as_deref()),
                revoked.desc.name
            );
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::DEPENDENT_PRIVILEGES));
        }
        self.put_routine(revoked.desc);
        let (dropped, locks) = self.destroy(Arc::clone(&self.catalog), &revoked.abandoned)?;
        let completion = if revoked.warned { Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED) } else { Completion::Success };
        Ok(Defined { completion, dropped, locks, ..Defined::default() })
    }

    /// The generated columns REVOKE EXECUTE `r` by `auth` on the routine
    /// `desc` loses: those whose generation expressions need what it
    /// revokes [§12.7 GR12h iv].
    pub(in crate::db) fn routine_columns_revoked(&self, auth: &str, desc: SqlRoutineDesc, r: &RevokeStatement) -> Vec<Column> {
        self.routine_revoked(auth, desc, r).abandoned.columns
    }

    /// EXECUTE, or DEBUG, on `desc` as REVOKE `r` by `auth` leaves it, and
    /// what that abandons: of DEBUG alone, nothing (RULES.md D1000).
    fn routine_revoked(&self, auth: &str, mut desc: SqlRoutineDesc, r: &RevokeStatement) -> Revoked {
        let (mut warned, mut lost) = (false, false);
        let privileges = Privilege::named(r.actions.as_deref());
        for &privilege in &privileges {
            let (was_warned, was_lost) = self.revoked_of(privilege, auth, &mut desc, r);
            warned |= was_warned;
            lost |= was_lost;
        }
        if !privileges.contains(&Privilege::Execute) {
            return Revoked { desc, warned, lost, abandoned: Abandoned::default() };
        }
        let mut after = self.clone();
        let mut catalog = self.catalog.duplicate();
        catalog.put_routine(desc.clone());
        after.catalog = Arc::from(catalog);
        let abandoned = self.abandoned_in(&after, &|e| e.code == Code::Privilege);
        Revoked { desc, warned, lost, abandoned }
    }

    /// The grants of `privilege` on `desc` as REVOKE `r` by `auth` leaves
    /// them: whether a grantee named held none of it from `auth` [GR4],
    /// and whether grants not named went with those that were [GR9].
    fn revoked_of(&self, privilege: Privilege, auth: &str, desc: &mut SqlRoutineDesc, r: &RevokeStatement) -> (bool, bool) {
        let named = |u: &UsageDesc| u.grantor == auth && r.grantees.contains(&u.grantee);
        let before = privilege.grants(desc).clone();
        let warned = !r.grantees.iter().all(|g| before.iter().any(|u| u.grantor == auth && u.grantee == *g));
        if r.grant_option_for {
            privilege.grants_mut(desc).iter_mut().filter(|u| named(u)).for_each(|u| u.grantable = false);
        } else {
            privilege.grants_mut(desc).retain(|u| !named(u));
        }
        // [GR9] a grant whose grantor holds no grant option is abandoned,
        // the owner's of _SYSTEM standing [§11.60 GR1]; each pass takes one
        // at least, so there are no more than grants.
        for _ in 0..before.len() {
            let kept = desc.clone();
            privilege
                .grants_mut(desc)
                .retain(|u| u.grantor == SYSTEM_GRANTOR || privilege.of(&*self.catalog, &kept, sole(&*self.catalog, &u.grantor)).1);
            if privilege.grants(desc).len() == privilege.grants(&kept).len() {
                break;
            }
        }
        let lost = before.iter().any(|u| !named(u) && !privilege.grants(desc).contains(u));
        (warned, lost)
    }

    /// What binds in these definitions but not in `after`, as `lost` takes
    /// its refusal: the views, constraints, assertions and generated
    /// columns `unbound_in` finds, and the triggers and routines whose
    /// definitions bind no more [§12.7 SR3b, GR12o].
    pub(in crate::db) fn abandoned_in(&self, after: &State, lost: &dyn Fn(&SqlError) -> bool) -> Abandoned {
        let mut abandoned = self.unbound_in(after, lost, &|_| true);
        let gone = |r: Result<()>| r.is_err_and(|e| lost(&e));
        abandoned.triggers = self.catalog.triggers().into_iter().filter(|t| gone(after.trigger_binds(t))).map(|t| t.name.clone()).collect();
        let routines = self.catalog.routines().into_iter().filter(|r| after.catalog.routine(&r.specific).is_some());
        abandoned.routines = routines.filter(|r| gone(after.routine_binds(r))).map(|r| r.specific.clone()).collect();
        abandoned
    }

    /// `desc`, recorded.
    pub(in crate::db) fn put_routine(&mut self, desc: SqlRoutineDesc) {
        let mut catalog = self.catalog.duplicate();
        catalog.put_routine(desc);
        self.catalog = Arc::from(catalog);
    }
}

/// The privileges `actions` names, of a routine, in words.
fn words(actions: Option<&[Action]>) -> String {
    Privilege::named(actions).iter().map(|p| p.word()).collect::<Vec<_>>().join(" and ")
}

/// [ISO/IEC 9075-2:2023 §11.60 GR1; ISO/IEC 9075-4:2006 draft §10.2] That
/// the routine `desc` is a schema-level routine, of which a privilege is
/// granted or revoked: one of a module has none, EXECUTE on its module
/// invoking it.
///
/// # Errors
///
/// SQLCODE -103 if it is of a module.
fn of_schema_level(desc: &SqlRoutineDesc) -> Result<()> {
    match desc.module_name() {
        Some(m) => Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §11.60 GR1; ISO/IEC 9075-4:2006 draft §10.2] routine {} is of module {m}, whose EXECUTE privilege invokes it",
            desc.name
        ))),
        None => Ok(()),
    }
}

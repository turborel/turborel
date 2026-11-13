//! USAGE on a distinct type and EXECUTE on its routines [ISO/IEC
//! 9075-2:2023 §12.2, §12.3, §12.7]: granted, and revoked, with what a
//! revoke abandons [§12.7 GR12]. A schema whose owner is left with no
//! USAGE loses each column of a type based on the type [GR12h ii] and
//! each type based on it [GR12j], dropped with CASCADE [GR12ag]; and each
//! view, constraint, assertion and generation expression whose owner
//! needs what it no longer holds, found by binding each again as the
//! revoke leaves the privileges (`user_types/uses.rs`).

use std::sync::Arc;

use super::super::definitions::Defined;
use super::super::generated::Column;
use super::super::revoke::Abandoned;
use super::super::{Completion, State, rule};
use crate::ast::{Action, DropBehavior, PrivilegeObject, RevokeStatement, TableName, TypeName};
use crate::binder::qualify;
use crate::catalog::{QualifiedName, Routine, UsageDesc, UserTypeDesc, based_on, owner_of};
use crate::error::{At, Code, Result, SqlError, SqlState};

/// Which privilege on a distinct type, or on one of its routines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    /// USAGE on the type.
    Usage,
    /// EXECUTE on the routine.
    Execute(Routine),
}

/// What a GRANT or REVOKE names: its object, its kind, its routine's
/// `<data type list>` if written, and its actions, or ALL PRIVILEGES.
type Named<'a> = (&'a TableName, PrivilegeObject, Option<&'a [TypeName]>, Option<&'a [Action]>);

/// The grants of `held` in `desc`.
fn grants(desc: &UserTypeDesc, held: Held) -> &[UsageDesc] {
    match held {
        Held::Usage => &desc.usage,
        Held::Execute(which) => &desc.routine(which).execute,
    }
}

/// The grants of `held` in `desc`, to change.
fn grants_mut(desc: &mut UserTypeDesc, held: Held) -> &mut Vec<UsageDesc> {
    match held {
        Held::Usage => &mut desc.usage,
        Held::Execute(which) => &mut desc.routine_mut(which).execute,
    }
}

/// Whether `auth` holds `held` of `desc`, and whether grantably, its
/// schema's owner `owner` grantably.
fn holds(desc: &UserTypeDesc, held: Held, auth: &str, owner: &str) -> (bool, bool) {
    match held {
        Held::Usage => desc.usage_of(auth, owner),
        Held::Execute(which) => desc.execute_of(which, auth, owner),
    }
}

/// `held` of `desc`, as a message names it.
fn described(desc: &UserTypeDesc, held: Held) -> String {
    match held {
        Held::Usage => format!("USAGE on type {}", desc.name),
        Held::Execute(which) => format!("EXECUTE on function {}.{}", desc.name.auth, desc.routine(which).name),
    }
}

/// A privilege on a distinct type or its routine as a REVOKE leaves it.
struct Revoked {
    /// The type's descriptor, the grants revoked and those they took gone.
    desc: UserTypeDesc,
    /// Which privilege of it.
    held: Held,
    /// A grantee named held no grant of it from the revoker [§12.7 GR4].
    warned: bool,
    /// Grants not named went with those that were [GR9].
    lost: bool,
    /// The views, constraints, assertions and generated columns abandoned.
    abandoned: Abandoned,
    /// The types based on it of the schemas whose owners hold no USAGE
    /// on it now [GR12j].
    types: Vec<QualifiedName>,
}

impl State {
    /// [ISO/IEC 9075-2:2023 §12.2] GRANT USAGE ON TYPE, or EXECUTE on one of
    /// a distinct type's routines, `p` by `auth`, its names in schema
    /// `schema`: granted if `auth` holds it grantably, else a warning
    /// \[GR9\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it grants another privilege [§12.3 SR4], or names no
    /// type or routine; -104 if `auth` holds no such privilege \[AR1\].
    pub(in crate::db) fn grant_on_type(&mut self, auth: &str, schema: &str, p: &crate::ast::PrivilegeDefinition) -> Result<Defined> {
        let (mut desc, held) =
            self.privilege_object((auth, schema), (&p.table, p.object, p.parameters.as_deref(), p.actions.as_deref()))?;
        let owner = owner_of(&*self.catalog, &desc.name.auth).to_owned();
        let (held_now, grantable) = holds(&desc, held, auth, &owner);
        if !held_now {
            let msg = format!("[ISO/IEC 9075-2:2023 §12.2 AR1] {auth} holds no {}", described(&desc, held));
            return Err(SqlError::new(Code::Privilege, msg));
        }
        if !grantable {
            return Ok(Defined { completion: Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED), ..Defined::default() });
        }
        let list = grants_mut(&mut desc, held);
        for grantee in &p.grantees {
            match list.iter_mut().find(|u| u.grantor == auth && u.grantee == *grantee) {
                Some(u) => u.grantable |= p.grant_option,
                None => list.push(UsageDesc { grantor: auth.to_owned(), grantee: grantee.clone(), grantable: p.grant_option }),
            }
        }
        self.put_type(desc);
        Ok(Defined::default())
    }

    /// [ISO/IEC 9075-2:2023 §12.7] REVOKE USAGE ON TYPE, or EXECUTE on one of
    /// a distinct type's routines, `r` by `auth`, its names in schema
    /// `schema`: the grants `auth` made those it names, or their grant
    /// option alone, gone; with CASCADE, what that abandons gone too, the
    /// columns it loses gone already (`Session::dropping_generations`). A
    /// warning if `auth` granted it to one it names not \[GR4\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it revokes another privilege [§12.3 SR4], or names
    /// no type or routine; with RESTRICT, -103 and SQLSTATE 2B000 if it
    /// abandons anything \[GR12q\].
    pub(in crate::db) fn revoke_on_type(&mut self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Defined> {
        let revoked = self.type_revoked((auth, schema), r)?;
        let lost = self.columns_revoked(&revoked);
        if r.behavior == DropBehavior::Restrict
            && (revoked.lost || !revoked.abandoned.is_empty() || !revoked.types.is_empty() || !lost.is_empty())
        {
            let what = match (revoked.types.first(), lost.first()) {
                (Some(t), _) => format!("type {t}"),
                (None, Some((t, c))) => format!("column {t}.{c}"),
                (None, None) if revoked.abandoned.is_empty() => "a grant of it".to_owned(),
                (None, None) => revoked.abandoned.first(),
            };
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §12.7 GR12q] revoking {} abandons {what}: revoke it with CASCADE",
                described(&revoked.desc, revoked.held)
            );
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::DEPENDENT_PRIVILEGES));
        }
        self.put_type(revoked.desc);
        let (mut dropped, locks) = self.destroy(Arc::clone(&self.catalog), &revoked.abandoned)?;
        let mut locks = locks;
        // [GR12ag] DROP TYPE CASCADE of each type abandoned.
        for t in revoked.types.iter().filter(|t| self.catalog.user_type(t).is_some()).cloned().collect::<Vec<_>>() {
            let taken = self.drop_user_type(&t, true)?;
            locks.extend(taken.names.iter().cloned().map(|name| (name, crate::txn::LockMode::Exclusive)));
            dropped.absorb(taken);
        }
        let completion = if revoked.warned { Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED) } else { Completion::Success };
        Ok(Defined { completion, dropped, locks, ..Defined::default() })
    }

    /// The columns REVOKE `r` by `auth` of a privilege on a distinct type
    /// or its routine, its names in schema `schema`, loses: those of a
    /// type based on it of the tables of each schema whose owner it leaves
    /// with no USAGE [§12.7 GR12h ii], and the generated columns whose
    /// generation expressions need what is revoked [GR12h iv], or name a
    /// type it abandons.
    ///
    /// # Errors
    ///
    /// As `revoke_on_type`, but for RESTRICT's.
    pub(in crate::db) fn type_columns_revoked(&self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Vec<Column>> {
        Ok(self.columns_revoked(&self.type_revoked((auth, schema), r)?))
    }

    /// The columns `revoked` loses.
    fn columns_revoked(&self, revoked: &Revoked) -> Vec<Column> {
        let mut columns = revoked.abandoned.columns.clone();
        if revoked.held == Held::Usage {
            let owner = owner_of(&*self.catalog, &revoked.desc.name.auth).to_owned();
            let unheld = |s: &str| !revoked.desc.usage_of(owner_of(&*self.catalog, s), &owner).0;
            for t in self.catalog.tables().into_iter().filter(|t| unheld(&t.name.auth)) {
                let of = t.columns.iter().filter(|c| based_on(&c.data_type, &revoked.desc.name));
                columns.extend(of.map(|c| (t.name.clone(), c.name.clone())));
            }
        }
        for c in self.columns_lost_with(&revoked.types, &|_| true) {
            if !columns.contains(&c) {
                columns.push(c);
            }
        }
        columns
    }

    /// The privilege REVOKE `r` by `auth` names, its names in schema
    /// `schema`, as it leaves it, and what that abandons.
    fn type_revoked(&self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Revoked> {
        let (mut desc, held) =
            self.privilege_object((auth, schema), (&r.table, r.object, r.parameters.as_deref(), r.actions.as_deref()))?;
        let owner = owner_of(&*self.catalog, &desc.name.auth).to_owned();
        let named = |u: &UsageDesc| u.grantor == auth && r.grantees.contains(&u.grantee);
        let before = grants(&desc, held).to_vec();
        let warned = !r.grantees.iter().all(|g| before.iter().any(|u| u.grantor == auth && u.grantee == *g));
        if r.grant_option_for {
            grants_mut(&mut desc, held).iter_mut().filter(|u| named(u)).for_each(|u| u.grantable = false);
        } else {
            grants_mut(&mut desc, held).retain(|u| !named(u));
        }
        // [GR9] a grant whose grantor holds no grant option is abandoned;
        // each pass takes one at least, so there are no more than grants.
        for _ in 0..before.len() {
            let kept = desc.clone();
            grants_mut(&mut desc, held).retain(|u| holds(&kept, held, &u.grantor, &owner).1);
            if grants(&desc, held).len() == grants(&kept, held).len() {
                break;
            }
        }
        let lost = before.iter().any(|u| !named(u) && !grants(&desc, held).contains(u));
        let mut after = self.clone();
        let mut catalog = self.catalog.duplicate();
        catalog.put_user_type(desc.clone());
        after.catalog = Arc::from(catalog);
        let abandoned = self.unbound_in(&after, &|e| e.code == Code::Privilege, &|_| true);
        let types = if held == Held::Usage { after.types_unheld(&desc, &owner) } else { Vec::new() };
        Ok(Revoked { desc, held, warned, lost, abandoned, types })
    }

    /// [ISO/IEC 9075-2:2023 §12.7 GR12j] The types other than `desc`'s
    /// whose sources are based on it, of the schemas whose owners hold no
    /// USAGE on it, its schema's owner `owner`.
    fn types_unheld(&self, desc: &UserTypeDesc, owner: &str) -> Vec<QualifiedName> {
        let unheld = |s: &str| !desc.usage_of(owner_of(&*self.catalog, s), owner).0;
        let of = self.catalog.user_types().into_iter().filter(|t| t.name != desc.name && unheld(&t.name.auth));
        of.filter(|t| based_on(&t.source, &desc.name)).map(|t| t.name.clone()).collect()
    }

    /// The distinct type, and which privilege on it or its routine, a
    /// GRANT or REVOKE by `auth` names, its names in schema `schema`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its actions are not the object's [§12.3 SR4], or it
    /// names no type or routine.
    fn privilege_object(
        &self,
        (auth, schema): (&str, &str),
        (name, object, parameters, actions): Named<'_>,
    ) -> Result<(UserTypeDesc, Held)> {
        let (wanted, kind) = if object == PrivilegeObject::Type { (Action::Usage, "USAGE") } else { (Action::Execute, "EXECUTE") };
        if let Some(a) = actions.and_then(|list| list.iter().find(|a| **a != wanted)) {
            let cited = match (a, object) {
                (Action::Under, PrivilegeObject::Type) => {
                    "SR4c] UNDER is a privilege of a structured type, not of a distinct type".to_owned()
                }
                (_, PrivilegeObject::Type) => format!("SR4a] of a user-defined type, {kind} alone is granted or revoked"),
                (_, _) => format!("SR4d] of a routine, {kind} alone is granted or revoked"),
            };
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §12.3 {cited}")).or_at(name.at));
        }
        let qn = qualify(name, schema);
        if object == PrivilegeObject::Type {
            return Ok((self.user_type_named(&qn, "§12.3 SR7").or_at(name.at)?, Held::Usage));
        }
        let (desc, which) = self.routine_designated(&qn, (object, parameters), auth).or_at(name.at)?;
        Ok((desc, Held::Execute(which)))
    }

    /// [ISO/IEC 9075-2:2023 §10.6] The routine `qn` designates, of the
    /// kind `object`: by its specific name \[SR1\]; else by its name, the one
    /// function of it, or the one whose parameter's type is that of the
    /// `<data type list>` `parameters` [SR3c, §9.30], each type in it named
    /// as `auth` may.
    fn routine_designated(
        &self,
        qn: &QualifiedName,
        (object, parameters): (PrivilegeObject, Option<&[TypeName]>),
        auth: &str,
    ) -> Result<(UserTypeDesc, Routine)> {
        let mut found = Vec::new();
        for t in self.catalog.user_types().into_iter().filter(|t| t.name.auth == qn.auth) {
            for which in [Routine::ToDistinct, Routine::ToSource] {
                let routine = t.routine(which);
                let named =
                    if object == PrivilegeObject::SpecificRoutine { routine.specific == qn.ident } else { routine.name == qn.ident };
                if named {
                    found.push((t.clone(), which));
                }
            }
        }
        if let Some(list) = parameters {
            let types = list.iter().map(|t| self.data_type_of(t, (&qn.auth, auth))).collect::<Result<Vec<_>>>()?;
            let parameter =
                |(t, which): &(UserTypeDesc, Routine)| if *which == Routine::ToDistinct { t.source.clone() } else { t.data_type() };
            found.retain(|f| types.len() == 1 && types.first() == Some(&parameter(f)));
        }
        match found.as_slice() {
            [one] => Ok(one.clone()),
            [] if object == PrivilegeObject::SpecificRoutine => {
                Err(rule(format!("[ISO/IEC 9075-2:2023 §10.6 SR1] there is no routine of specific name {qn}")))
            }
            _ => Err(rule(format!("[ISO/IEC 9075-2:2023 §10.6 SR3c] {qn} designates not exactly one function"))),
        }
    }

    /// `desc`, recorded.
    fn put_type(&mut self, desc: UserTypeDesc) {
        let mut catalog = self.catalog.duplicate();
        catalog.put_user_type(desc);
        self.catalog = Arc::from(catalog);
    }
}

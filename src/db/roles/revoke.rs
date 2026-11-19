//! [ISO/IEC 9075-2:2023 §12.6, §12.7] REVOKE of roles, and DROP ROLE.
//!
//! The role authorizations a REVOKE identifies, those its grantor granted
//! of the roles named to the grantees named [GR1b], are destroyed, or with
//! ADMIN OPTION FOR made not grantable. One depends on another of its role
//! granted grantably to its grantor, to PUBLIC, or, of a grantor that is a
//! role, to a role applicable for it [GR5]; one reached from an
//! independent one by no path but through those identified is abandoned
//! [GR7]. With those identified and abandoned destroyed, every privilege is
//! settled again (`roles/settle.rs`) [GR8, GR9]. RESTRICT refuses to
//! abandon a privilege or an object [GR12q]; an abandoned role
//! authorization stays, destroyed only with CASCADE [GR12s ii].
//!
//! DROP ROLE revokes the role from each grantee, by any grantor
//! [§12.6 GR1]; \[DESIGN\] and each role and privilege granted to it or
//! by it, so no descriptor names a role no more there (RULES.md D75). With
//! RESTRICT it refuses if the role granted any, or a role authorization is
//! abandoned, as dependencies on the role are refused \[§12.6 NOTE 652\];
//! and it never drops the owner of a schema, which no statement hands on.

use std::sync::Arc;

use super::super::definitions::Defined;
use super::super::{State, rule};
use super::refuse_unknown;
use super::settle::Lost;
use crate::ast::DropBehavior;
use crate::catalog::{QualifiedName, RoleAuthorization, Roles, SYSTEM_GRANTOR, Who};
use crate::error::{At, Code, Result, SqlError, SqlState};
use crate::txn::LockMode;

/// What a REVOKE of roles or a DROP ROLE leaves [ISO/IEC 9075-2:2023 §12.7
/// GR10].
pub(super) struct RoleRevoked {
    /// The role authorizations left, and the roles.
    roles: Roles,
    /// The role authorizations abandoned, kept but with CASCADE [GR12s ii
    /// 1].
    abandoned: Vec<RoleAuthorization>,
    /// The definitions as the privileges are left, settled.
    after: State,
    /// What it abandons.
    pub(super) lost: Lost,
}

/// How a REVOKE of roles or a DROP ROLE identifies role authorizations.
struct Identified<'a> {
    /// Whether one is identified [ISO/IEC 9075-2:2023 §12.7 GR1b].
    of: &'a dyn Fn(&RoleAuthorization) -> bool,
    /// ADMIN OPTION FOR was written.
    admin_option: bool,
    /// The role dropped, if it is a DROP ROLE.
    gone: Option<&'a str>,
}

/// What DROP ROLE `name` identifies [ISO/IEC 9075-2:2023 §12.6 GR1]: each
/// authorization of the role, by any grantor; \[DESIGN\] and each granted
/// to it or by it, so none names it once it is gone (RULES.md D75).
fn dropping(name: &str) -> impl Fn(&RoleAuthorization) -> bool + '_ {
    move |x: &RoleAuthorization| x.role == name || x.grantee.as_deref() == Some(name) || x.grantor == name
}

/// [ISO/IEC 9075-2:2023 §12.7 GR5] Whether `d` directly depends on `rd`,
/// in `roles`.
fn depends(roles: &Roles, rd: &RoleAuthorization, d: &RoleAuthorization) -> bool {
    let through_role = || roles.is_role(&d.grantor) && rd.grantee.as_deref().is_some_and(|g| roles.applicable(&[&d.grantor]).contains(g));
    rd.grantable && rd.role == d.role && (rd.grantee.is_none() || rd.grantee.as_deref() == Some(d.grantor.as_str()) || through_role())
}

/// [ISO/IEC 9075-2:2023 §12.7 GR6, GR7] Which role authorizations of
/// `roles` are abandoned, those `identified` identifies by `of`: each not
/// independent nor identified that no path reaches from an independent one
/// but through one identified. One of the special grantor, a role's
/// creation's, is independent.
fn abandoned_roles(roles: &Roles, of: &dyn Fn(&RoleAuthorization) -> bool) -> Vec<bool> {
    let all = &roles.authorizations;
    let independent: Vec<bool> = all.iter().map(|d| d.grantor == SYSTEM_GRANTOR || !all.iter().any(|rd| depends(roles, rd, d))).collect();
    let mut reached: Vec<bool> = all.iter().zip(&independent).map(|(d, i)| *i && !of(d)).collect();
    loop {
        let next: Vec<bool> = all
            .iter()
            .zip(&reached)
            .map(|(d, r)| *r || (!of(d) && all.iter().zip(&reached).any(|(rd, rr)| *rr && depends(roles, rd, d))))
            .collect();
        if next == reached {
            break;
        }
        reached = next;
    }
    all.iter().zip(independent.iter().zip(&reached)).map(|(d, (i, r))| !*i && !of(d) && !*r).collect()
}

impl State {
    /// [ISO/IEC 9075-2:2023 §12.7] REVOKE \[ADMIN OPTION FOR\] each of
    /// `revoked` FROM each of `grantees`, by `a`, with `behavior`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a role named is not one; -104 if `a` holds a role
    /// named WITH ADMIN OPTION by none of its applicable roles \[AR1b\];
    /// with RESTRICT, -103 and SQLSTATE 2B000 if a privilege or object is
    /// abandoned \[GR12q\].
    pub(super) fn revoke_role(
        &mut self,
        a: &str,
        (revoked, grantees): (&[(String, usize)], &[Option<String>]),
        (admin_option, behavior): (bool, DropBehavior),
    ) -> Result<Defined> {
        let roles = self.catalog.roles();
        for (role, at) in revoked {
            refuse_unknown(roles, role).or_at(*at)?;
            if !roles.administers(a, role) {
                let msg = format!("[ISO/IEC 9075-2:2023 §12.7 AR1b] {a} holds {role} WITH ADMIN OPTION by none of its applicable roles");
                return Err(SqlError::new(Code::Privilege, msg).or_at(*at));
            }
        }
        let of =
            |x: &RoleAuthorization| x.grantor == a && grantees.contains(&x.grantee) && revoked.iter().any(|(named, _)| *named == x.role);
        let outcome = self.role_revoked(&Identified { of: &of, admin_option, gone: None })?;
        self.made(outcome, behavior, false)
    }

    /// [ISO/IEC 9075-2:2023 §12.6] DROP ROLE `name` by the cell `cell`, with
    /// `behavior`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no role `name`; -104 if no enabled
    /// identifier of the cell holds it WITH ADMIN OPTION \[AR1\]; SQLCODE
    /// -103 and SQLSTATE 2B000 if it owns a schema, or, with RESTRICT, it
    /// abandons a role authorization, a privilege or an object \[GR1\].
    pub(super) fn drop_role(&mut self, cell: Who<'_>, name: &str, behavior: DropBehavior) -> Result<Defined> {
        let roles = self.catalog.roles();
        refuse_unknown(roles, name)?;
        let admin = |x: &RoleAuthorization| x.role == name && x.grantable && x.grantee.as_deref().is_some_and(|g| cell.includes(g));
        if !roles.authorizations.iter().any(admin) {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §12.6 AR1] no enabled authorization identifier of {} holds {name} WITH ADMIN OPTION",
                cell.auth
            );
            return Err(SqlError::new(Code::Privilege, msg));
        }
        if let Some(schema) = self.catalog.schemas().into_iter().find(|s| self.catalog.schema_owner(s) == Some(name)) {
            let msg = format!(
                "[DESIGN] role {name} owns schema {schema}, and no statement gives a schema another owner, so drop the schema first"
            );
            return Err(rule(msg).with_state(SqlState::DEPENDENT_PRIVILEGES));
        }
        if behavior == DropBehavior::Restrict && self.any_grant(&|grantor, _| grantor == name) {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §12.6 NOTE 652] role {name} granted a privilege or a role, which depends on it, so it is dropped only with CASCADE"
            );
            return Err(rule(msg).with_state(SqlState::DEPENDENT_PRIVILEGES));
        }
        let outcome = self.role_revoked(&Identified { of: &dropping(name), admin_option: false, gone: Some(name) })?;
        self.made(outcome, behavior, true)
    }

    /// `revoked` made, with `behavior`: refused with RESTRICT if it abandons
    /// a privilege or an object, or, `strict`, a role authorization.
    ///
    /// # Errors
    ///
    /// As `revoke_role` and `drop_role`.
    fn made(&mut self, revoked: RoleRevoked, behavior: DropBehavior, strict: bool) -> Result<Defined> {
        if behavior == DropBehavior::Restrict {
            refuse_restricted(&revoked, strict)?;
        }
        // Reverted already, with CASCADE (`Session::revert_role_impacted`).
        for (qn, schemas) in &revoked.lost.collations {
            if let Some(t) = self.tables_of_collation(qn, None).into_iter().find(|t| schemas.contains(&t.auth)) {
                return Err(super::super::api(format!("[DESIGN] the columns of {qn} in table {t} are impacted, and not reverted")));
            }
        }
        let RoleRevoked { mut roles, abandoned, after, lost } = revoked;
        // [GR12s ii 1] destroyed only with CASCADE.
        if behavior == DropBehavior::Restrict {
            roles.authorizations.extend(abandoned);
        }
        let mut catalog = after.catalog.duplicate();
        catalog.set_roles(roles);
        let mut locks: Vec<(QualifiedName, LockMode)> = Vec::new();
        for (qn, schemas) in &lost.collations {
            let within = |s: &str| schemas.iter().any(|x| x == s);
            for d in self.catalog.domains().into_iter().filter(|d| within(&d.name.auth) && d.collation.as_ref() == Some(qn)) {
                locks.push((d.name.clone(), LockMode::AccessExclusive));
                catalog.put_domain(crate::catalog::DomainDesc { collation: None, ..d.clone() });
            }
            for view in self.views_collated(qn, &within) {
                locks.push((view.name.clone(), LockMode::AccessExclusive));
                catalog.remove_view(&view.name);
                catalog.add_views(vec![view]);
            }
        }
        let (mut dropped, destroyed) = self.destroy(Arc::from(catalog), &lost.abandoned)?;
        locks.extend(destroyed);
        // [GR12ag] DROP TYPE CASCADE of each type abandoned.
        for t in lost.types.iter().filter(|t| self.catalog.user_type(t).is_some()).cloned().collect::<Vec<_>>() {
            let taken = self.drop_user_type(&t, true)?;
            locks.extend(taken.names.iter().cloned().map(|name| (name, LockMode::AccessExclusive)));
            dropped.absorb(taken);
        }
        Ok(Defined { dropped, locks, ..Defined::default() })
    }

    /// What revoking the role authorizations `identified` identifies leaves.
    ///
    /// # Errors
    ///
    /// As `State::settled`.
    fn role_revoked(&self, identified: &Identified<'_>) -> Result<RoleRevoked> {
        let before = self.catalog.roles();
        let flags = abandoned_roles(before, identified.of);
        let mut kept = Roles {
            names: before.names.iter().filter(|n| Some(n.as_str()) != identified.gone).cloned().collect(),
            authorizations: Vec::new(),
        };
        let mut abandoned = Vec::new();
        for (a, gone) in before.authorizations.iter().zip(flags) {
            match ((identified.of)(a), identified.admin_option, gone) {
                (true, true, _) => kept.authorizations.push(RoleAuthorization { grantable: false, ..a.clone() }),
                (true, false, _) => {}
                (false, _, true) => abandoned.push(a.clone()),
                (false, _, false) => kept.authorizations.push(a.clone()),
            }
        }
        // [GR8] the privileges as the abandoned are destroyed too.
        let (after, grant) = self.settled(kept.clone(), identified.gone)?;
        let lost = self.lost_in(&after, grant);
        Ok(RoleRevoked { roles: kept, abandoned, after, lost })
    }

    /// What revoking role authorizations as REVOKE or DROP ROLE of `cell`
    /// would with CASCADE, so the generated columns it loses are dropped
    /// and the columns it impacts reverted before it is made
    /// (`Session::role_generations`, `Session::revert_role_impacted`).
    ///
    /// # Errors
    ///
    /// As `revoke_role` or `drop_role`, but for RESTRICT's.
    pub(super) fn role_losses(&self, (auth, role): (&str, Option<&str>), r: &crate::ast::RoleStatement) -> Result<Option<RoleRevoked>> {
        let roles = self.catalog.roles().clone();
        let cell = Who { auth, role, roles: &roles };
        match r {
            crate::ast::RoleStatement::Revoke { admin_option, roles: revoked, grantees, grantor, .. } => {
                let a = super::grantor_of(cell, *grantor)?;
                let of = |x: &RoleAuthorization| {
                    x.grantor == a && grantees.contains(&x.grantee) && revoked.iter().any(|(named, _)| *named == x.role)
                };
                self.role_revoked(&Identified { of: &of, admin_option: *admin_option, gone: None }).map(Some)
            }
            crate::ast::RoleStatement::Drop { name: (name, _), .. } => {
                self.role_revoked(&Identified { of: &dropping(name), admin_option: false, gone: Some(name) }).map(Some)
            }
            crate::ast::RoleStatement::Create { .. } | crate::ast::RoleStatement::Grant { .. } => Ok(None),
        }
    }
}

/// [ISO/IEC 9075-2:2023 §12.7 GR12q] That `revoked`, with RESTRICT,
/// abandons no privilege, no object and impacts no column; and, `strict`,
/// abandons no role authorization.
///
/// # Errors
///
/// SQLCODE -103 and SQLSTATE 2B000 if it does.
fn refuse_restricted(revoked: &RoleRevoked, strict: bool) -> Result<()> {
    let lost = &revoked.lost;
    let what = if let Some(g) = &lost.grant {
        g.clone()
    } else if !lost.abandoned.is_empty() {
        lost.abandoned.first()
    } else if let Some(t) = lost.types.first() {
        format!("type {t}")
    } else if let Some((qn, _)) = lost.collations.first() {
        format!("the columns of collation {qn}")
    } else if strict && !revoked.abandoned.is_empty() {
        "a role authorization granted by its admin option".to_owned()
    } else {
        return Ok(());
    };
    let msg = format!("[ISO/IEC 9075-2:2023 §12.7 GR12q] the statement would abandon {what}, so it is made only with CASCADE");
    Err(rule(msg).with_state(SqlState::DEPENDENT_PRIVILEGES))
}

//! EXECUTE on an SQL-server module [ISO/IEC 9075-4:2006 draft §10.1 to
//! §10.3]: granted, revoked, and made grantable to the owner of its
//! schema once the privileges its routines need are.
//!
//! What invokes a routine of a module needs EXECUTE on the module [§8.1
//! SR1], so a revoke abandons the routines, views, constraints, generated
//! columns and triggers whose owners it leaves without it, as a revoke of
//! EXECUTE on a routine abandons those that invoke it [§10.3 GR1 to GR7].

use std::sync::Arc;

use super::super::definitions::Defined;
use super::super::generated::Column;
use super::super::revoke::Abandoned;
use super::super::{Completion, State, rule};
use crate::ast::{Action, DropBehavior, PrivilegeDefinition, PrivilegeObject, RevokeStatement, Schema};
use crate::binder::qualify;
use crate::catalog::{ModuleDesc, QualifiedName, SYSTEM_GRANTOR, UsageDesc, owner_of, sole};
use crate::error::{At, Code, Result, SqlError, SqlState};

/// EXECUTE on a module as a revoke leaves it, and what that abandons.
struct Revoked {
    desc: ModuleDesc,
    /// Whether it named a grantee it was not granted to.
    warned: bool,
    /// Whether a grant of another grantee goes with those named.
    lost: bool,
    abandoned: Abandoned,
}

impl State {
    /// [ISO/IEC 9075-2:2023 §12.2; ISO/IEC 9075-4:2006 draft §10.2] GRANT
    /// EXECUTE ON MODULE `p` by `auth`, its names in schema `schema`:
    /// granted if `auth` holds it grantably, else a warning \[GR9\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it grants other than EXECUTE [§10.2 SR1], or there
    /// is no such module; -104 if `auth` holds no EXECUTE on it \[AR1\].
    pub(in crate::db) fn grant_on_module(&mut self, auth: &str, schema: &str, p: &PrivilegeDefinition) -> Result<Defined> {
        execute_alone(p.actions.as_deref())?;
        let mut desc = self.module_named(&qualify(&p.table, schema))?;
        let (held, grantable) = desc.execute_of(sole(&*self.catalog, auth));
        if !held {
            let msg = format!("[ISO/IEC 9075-2:2023 §12.2 AR1] {auth} holds no EXECUTE on module {}", desc.name);
            return Err(SqlError::new(Code::Privilege, msg));
        }
        if !grantable {
            return Ok(Defined { completion: Completion::Warning(SqlState::PRIVILEGE_NOT_GRANTED), ..Defined::default() });
        }
        for grantee in &p.grantees {
            match desc.execute.iter_mut().find(|u| u.grantor == auth && u.grantee == *grantee) {
                Some(u) => u.grantable |= p.grant_option,
                None => desc.execute.push(UsageDesc { grantor: auth.to_owned(), grantee: grantee.clone(), grantable: p.grant_option }),
            }
        }
        self.put_module(desc);
        Ok(Defined::default())
    }

    /// [ISO/IEC 9075-2:2023 §12.7; ISO/IEC 9075-4:2006 draft §10.3] REVOKE
    /// EXECUTE ON MODULE `r` by `auth`, its names in schema `schema`: the
    /// grants `auth` made those it names, or their grant option alone,
    /// gone; with CASCADE, what that abandons gone too, the generated
    /// columns it loses gone already (`Session::dropping_generations`). A
    /// warning if `auth` granted it to one it names not \[GR4\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it revokes other than EXECUTE, or there is no such
    /// module; with RESTRICT, -103 and SQLSTATE 2B000 if it abandons
    /// anything \[GR12q\]; as each drop's.
    pub(in crate::db) fn revoke_on_module(&mut self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Defined> {
        let revoked = self.module_revoked((auth, schema), r)?;
        if r.behavior == DropBehavior::Restrict && (revoked.lost || !revoked.abandoned.is_empty()) {
            let what = if revoked.abandoned.is_empty() { "a grant of it".to_owned() } else { revoked.abandoned.first() };
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §12.7 GR12q] revoking EXECUTE on module {} abandons {what}: revoke it with CASCADE",
                revoked.desc.name
            );
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::DEPENDENT_PRIVILEGES));
        }
        self.put_module(revoked.desc);
        let (dropped, locks) = self.destroy(Arc::clone(&self.catalog), &revoked.abandoned)?;
        let completion = if revoked.warned { Completion::Warning(SqlState::PRIVILEGE_NOT_REVOKED) } else { Completion::Success };
        Ok(Defined { completion, dropped, locks, ..Defined::default() })
    }

    /// The generated columns REVOKE EXECUTE ON MODULE `r` by `auth`, its
    /// names in schema `schema`, loses [§12.7 GR12h iv].
    ///
    /// # Errors
    ///
    /// As `revoke_on_module`'s refusals of the module and the privilege.
    pub(in crate::db) fn module_columns_revoked(&self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Vec<Column>> {
        Ok(self.module_revoked((auth, schema), r)?.abandoned.columns)
    }

    /// EXECUTE on the module REVOKE `r` by `auth` names, as it leaves it,
    /// and what that abandons.
    fn module_revoked(&self, (auth, schema): (&str, &str), r: &RevokeStatement) -> Result<Revoked> {
        execute_alone(r.actions.as_deref())?;
        let mut desc = self.module_named(&qualify(&r.table, schema)).or_at(r.table.at)?;
        let named = |u: &UsageDesc| u.grantor == auth && r.grantees.contains(&u.grantee);
        let before = desc.execute.clone();
        let warned = !r.grantees.iter().all(|g| before.iter().any(|u| u.grantor == auth && u.grantee == *g));
        if r.grant_option_for {
            desc.execute.iter_mut().filter(|u| named(u)).for_each(|u| u.grantable = false);
        } else {
            desc.execute.retain(|u| !named(u));
        }
        // [GR9] a grant whose grantor holds no grant option is abandoned,
        // the owner's of _SYSTEM standing [§9.21 GR2]; each pass takes one
        // at least, so there are no more than grants.
        for _ in 0..before.len() {
            let kept = desc.clone();
            desc.execute.retain(|u| u.grantor == SYSTEM_GRANTOR || kept.execute_of(sole(&*self.catalog, &u.grantor)).1);
            if desc.execute.len() == kept.execute.len() {
                break;
            }
        }
        let lost = before.iter().any(|u| !named(u) && !desc.execute.contains(u));
        let mut after = self.clone();
        let mut catalog = self.catalog.duplicate();
        catalog.put_module(desc.clone());
        after.catalog = Arc::from(catalog);
        let abandoned = self.abandoned_in(&after, &|e| e.code == Code::Privilege);
        Ok(Revoked { desc, warned, lost, abandoned })
    }

    /// [ISO/IEC 9075-4:2006 draft §10.1 GR1] Of each module whose owner's
    /// EXECUTE of `_SYSTEM` is not grantable, made grantable once the
    /// owner holds grantably every privilege its routines need, as a GRANT
    /// may have made it.
    pub(in crate::db) fn module_grants_upgraded(&mut self) {
        let mut upgraded = Vec::new();
        for m in self.catalog.modules() {
            let Some(system) = m.execute.iter().position(|u| u.grantor == SYSTEM_GRANTOR && !u.grantable) else { continue };
            let owner = owner_of(&*self.catalog, &m.name.auth);
            let routines = self.catalog.routines().into_iter().filter(|r| r.module_name().as_ref() == Some(&m.name));
            if routines.into_iter().all(|r| self.execute_grantable((owner, Some(m.name.clone())), &r.reads, r.invokes())) {
                let mut desc = m.clone();
                if let Some(u) = desc.execute.get_mut(system) {
                    u.grantable = true;
                }
                upgraded.push(desc);
            }
        }
        for desc in upgraded {
            self.put_module(desc);
        }
    }

    /// The grants of EXECUTE ON MODULE of `<schema definition>` `s`, by the
    /// owner of its schema, made once its modules are: whether one was not.
    ///
    /// # Errors
    ///
    /// As `grant_on_module`'s.
    pub(in crate::db) fn schema_module_grants(&mut self, s: &Schema) -> Result<bool> {
        let owner = owner_of(&*self.catalog, &s.name).to_owned();
        let mut warned = false;
        for p in s.privileges.iter().filter(|p| p.object == PrivilegeObject::Module) {
            warned |= self.grant_on_module(&owner, &s.name, p).or_at(p.table.at)?.completion != Completion::Success;
        }
        Ok(warned)
    }

    /// The module `qn`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is none [ISO/IEC 9075-2:2023 §12.3 SR1].
    fn module_named(&self, qn: &QualifiedName) -> Result<ModuleDesc> {
        let found = self.catalog.module(qn).cloned();
        found.ok_or_else(|| rule(format!("[ISO/IEC 9075-4:2006 draft §10.2] there is no module {qn}")))
    }
}

/// [ISO/IEC 9075-4:2006 draft §10.2 SR1] That `actions` are EXECUTE alone,
/// as of a module only EXECUTE is granted or revoked.
///
/// # Errors
///
/// SQLCODE -103 if one is another.
fn execute_alone(actions: Option<&[Action]>) -> Result<()> {
    if actions.is_some_and(|list| list.iter().any(|a| *a != Action::Execute)) {
        return Err(rule("[ISO/IEC 9075-4:2006 draft §10.2 SR1] of a module, EXECUTE alone is granted or revoked".to_owned()));
    }
    Ok(())
}
